using System.Reflection;
using System.Text;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Services;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.InRaid;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the resident-DB epoch protocol on the reward-loot native path: an eligible generator names
/// an epoch and never sends the views override, the kill switch and untrusted mods fall back to
/// the override, a construction without the overload always overrides, a native-side epoch desync
/// self-heals through one republish plus retry, and — the flip's core promise — a resident send
/// and an override send generate identical rewards field for field, covered on the sealed case
/// because it has the most resident-mapped views (by-tpl presets, presets-by-tpl subset). Epochs
/// are process-global (other fixtures publish too), so every assertion is relative. Mutates the
/// shared config singleton, so it restores it and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RewardLootResidentDbTests
{
    private const ulong Seed = 424242;

    private LootGenerator _generator = default!;
    private LocationConfig _locationConfig = default!;
    private DatabaseMutationStamp _stamp = default!;
    private DbPublisher _publisher = default!;
    private JsonUtil _jsonUtil = default!;
    private InventoryConfig _inventoryConfig = default!;
    private AirdropService _airdropService = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _generator = di.GetService<LootGenerator>();
        _locationConfig = di.GetService<LocationConfig>();
        _stamp = di.GetService<DatabaseMutationStamp>();
        _publisher = di.GetService<DbPublisher>();
        _jsonUtil = di.GetService<JsonUtil>();
        _inventoryConfig = di.GetService<InventoryConfig>();
        _airdropService = di.GetService<AirdropService>();
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _locationConfig.DisableNativeRequestCache = false;
        _locationConfig.TrustNativeRequestCacheWithMods = false;
        // leave the shared container fresher than we found it for whatever fixture runs next
        _stamp.Bump();
    }

    /// <summary>
    /// One seeded generation on the native path, normalized for comparison. Fails fast on a
    /// silent legacy fallback or an empty result before comparing anything.
    /// </summary>
    private string Generate(LootGenerator generator, Func<LootGenerator, IEnumerable<List<Item>>> generate)
    {
        generator.NativeTestSeed = Seed;
        try
        {
            var loot = generate(generator).ToList();

            Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native), "generation did not take the native path");
            Assert.That(loot, Is.Not.Empty, "the native path generated no reward loot");

            return LootIdNormalizer.Normalize(_jsonUtil.Serialize(loot)!);
        }
        finally
        {
            generator.NativeTestSeed = null;
        }
    }

    /// <summary>
    /// The airdrop request as AirdropService builds it - the blacklist folding it does on top of
    /// the raw config is part of the input, so the real method is invoked rather than reproduced.
    /// </summary>
    private string GenerateAirdropRandomLoot(LootGenerator generator)
    {
        var method = typeof(AirdropService).GetMethod("GetAirdropLootConfigByType", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.That(method, Is.Not.Null, "AirdropService.GetAirdropLootConfigByType not found");
        var options = (LootRequest)method!.Invoke(_airdropService, [SptAirdropTypeEnum.mixed])!;

        return Generate(generator, target => target.CreateRandomLoot(options));
    }

    [Test]
    public void EligibleGenerationBuildsOffTheResidentDb()
    {
        GenerateAirdropRandomLoot(_generator);

        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False, "an eligible generator must not send the override");
    }

    [Test]
    public void KillSwitchForcesTheViewsOverride()
    {
        _locationConfig.DisableNativeRequestCache = true;
        try
        {
            GenerateAirdropRandomLoot(_generator);

            Assert.That(_generator.LastSendIncludedViewsOverride, Is.True, "the kill switch must force the override");
        }
        finally
        {
            _locationConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ModsLoadedWithoutTheTrustFlagForceTheViewsOverride()
    {
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = BuildWithOverloadConstructor(DI.GetInstance(), new SptMod[] { null! });

        GenerateAirdropRandomLoot(modded);

        Assert.That(modded.LastSendIncludedViewsOverride, Is.True, "a loaded mod without the trust flag disables residency");
    }

    [Test]
    public void TheTrustFlagKeepsTheResidentPathLiveWithModsLoaded()
    {
        var modded = BuildWithOverloadConstructor(DI.GetInstance(), new SptMod[] { null! });

        _locationConfig.TrustNativeRequestCacheWithMods = true;
        try
        {
            GenerateAirdropRandomLoot(modded);

            Assert.That(
                modded.LastSendIncludedViewsOverride,
                Is.False,
                "the trust flag should keep the resident path live despite the mod"
            );
        }
        finally
        {
            _locationConfig.TrustNativeRequestCacheWithMods = false;
        }
    }

    [Test]
    public void AGeneratorBuiltOnTheFrozenConstructorAlwaysSendsTheOverride()
    {
        var frozen = BuildWithFrozenConstructor(DI.GetInstance());

        GenerateAirdropRandomLoot(frozen);

        Assert.That(frozen.LastSendIncludedViewsOverride, Is.True, "no publisher means no residency eligibility");
    }

    [Test]
    public void ANativeSideEpochDesyncSelfHealsThroughOneRetry()
    {
        // Settle the publisher's remembered epoch first, so the desync below is the only miss
        _publisher.EnsureCurrent();

        // Desync: a direct native publish the publisher never sees moves the resident epoch out
        // from under the epoch it remembers
        SptNative.DbPublish(Encoding.UTF8.GetBytes("{\"schema\":1,\"roots\":{}}"));

        GenerateAirdropRandomLoot(_generator);

        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False, "the stale-epoch miss should have republished and retried");
    }

    /// <summary>
    /// The flip's core promise on its most resident-mapped export: the same seed through one
    /// eligible sealed-case send and one kill-switched send must generate identical rewards,
    /// compared as normalized JSON down to every field.
    /// </summary>
    [Test]
    public void AResidentSendAndAnOverrideSendProduceIdenticalRewardsFieldForField()
    {
        var resident = Generate(_generator, target => target.GetSealedWeaponCaseLoot(_inventoryConfig.SealedAirdropContainer));
        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False);

        _locationConfig.DisableNativeRequestCache = true;
        string overrideSend;
        try
        {
            overrideSend = Generate(_generator, target => target.GetSealedWeaponCaseLoot(_inventoryConfig.SealedAirdropContainer));
        }
        finally
        {
            _locationConfig.DisableNativeRequestCache = false;
        }
        Assert.That(_generator.LastSendIncludedViewsOverride, Is.True);

        LootJsonAssert.AssertEqual(resident, overrideSend, "resident send vs views-override send", Seed);
    }

    private static LootGenerator BuildWithFrozenConstructor(DI di)
    {
        return Build(di, FindConstructor(smallest: true), null);
    }

    private static LootGenerator BuildWithOverloadConstructor(DI di, IReadOnlyList<SptMod> mods)
    {
        return Build(di, FindConstructor(smallest: false), mods);
    }

    private static ConstructorInfo FindConstructor(bool smallest)
    {
        var constructors = typeof(LootGenerator).GetConstructors().OrderBy(ctor => ctor.GetParameters().Length).ToList();

        return smallest ? constructors.First() : constructors.Last();
    }

    private static LootGenerator Build(DI di, ConstructorInfo constructor, IReadOnlyList<SptMod>? mods)
    {
        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
            {
                if (mods is not null && parameter.ParameterType == typeof(IReadOnlyList<SptMod>))
                {
                    return mods;
                }

                return di.GetService(parameter.ParameterType);
            })
            .ToArray();

        return (LootGenerator)constructor.Invoke(arguments);
    }
}
