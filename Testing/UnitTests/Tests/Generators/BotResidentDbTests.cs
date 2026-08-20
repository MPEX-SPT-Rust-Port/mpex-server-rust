using System.Text;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weapons;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the resident-DB epoch protocol on the bot generation native paths: an eligible wave names
/// an epoch and never sends the views override, the kill switch and untrusted mods fall back to
/// the override, a generator built on the frozen 4.1.2 constructor always overrides, a native-side
/// epoch desync self-heals through one republish plus retry, and — the flip's core promise — a
/// resident send and an override send generate identical bots field for field. Epochs are
/// process-global (other fixtures publish too), so every assertion is relative. Mutates the shared
/// config singleton, so it restores it and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotResidentDbTests
{
    private const ulong Seed = 424242;

    private BotWaveBatcher _batcher = default!;
    private BotInventoryGenerator _botInventoryGenerator = default!;
    private BotWeaponGenerator _botWeaponGenerator = default!;
    private BotLootGenerator _botLootGenerator = default!;
    private BotEquipmentModGenerator _botEquipmentModGenerator = default!;
    private ProfileHelper _profileHelper = default!;
    private ItemHelper _itemHelper = default!;
    private WeatherHelper _weatherHelper = default!;
    private ProfileActivityService _profileActivityService = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotEquipmentModPoolService _botEquipmentModPoolService = default!;
    private BotConfig _botConfig = default!;
    private PmcConfig _pmcConfig = default!;
    private BotTable _botTable = default!;
    private ICloner _cloner = default!;
    private DatabaseMutationStamp _stamp = default!;
    private DbPublisher _publisher = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _batcher = di.GetService<BotWaveBatcher>();
        _botInventoryGenerator = di.GetService<BotInventoryGenerator>();
        _botWeaponGenerator = di.GetService<BotWeaponGenerator>();
        _botLootGenerator = di.GetService<BotLootGenerator>();
        _botEquipmentModGenerator = di.GetService<BotEquipmentModGenerator>();
        _profileHelper = di.GetService<ProfileHelper>();
        _itemHelper = di.GetService<ItemHelper>();
        _weatherHelper = di.GetService<WeatherHelper>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _botEquipmentFilterService = di.GetService<BotEquipmentFilterService>();
        _botEquipmentModPoolService = di.GetService<BotEquipmentModPoolService>();
        _botConfig = di.GetService<BotConfig>();
        _pmcConfig = di.GetService<PmcConfig>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();
        _stamp = di.GetService<DatabaseMutationStamp>();
        _publisher = di.GetService<DbPublisher>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _botConfig.DisableNativeRequestCache = false;
        _botConfig.TrustNativeRequestCacheWithMods = true;
        // leave the shared container fresher than we found it for whatever fixture runs next
        _stamp.Bump();
    }

    /// <summary>
    /// One wave through the batch path. Fails fast on a silent decline to the per-bot path or an
    /// incomplete wave before asserting anything about the send.
    /// </summary>
    private void GenerateWave(BotWaveBatcher batcher)
    {
        var wave = batcher.TryGenerateWave(
            _sessionId,
            new BotGenerationDetails
            {
                Role = "assault",
                RoleLowercase = "assault",
                Side = "Savage",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotCountToGenerate = 3,
            }
        );

        Assert.That(wave, Is.Not.Null, "the wave declined the batch path");
        Assert.That(wave!, Has.Count.EqualTo(3), "the batch generated an incomplete wave");
    }

    /// <summary>
    /// The batch path's twin for the single-bot dispatch site. Same fail-fast contract as
    /// <see cref="GenerateWave" />: a decline to the legacy path or an empty inventory fails here,
    /// before the caller asserts anything about the send.
    /// </summary>
    private void GenerateSingleBot(BotInventoryGenerator generator)
    {
        var details = new BotGenerationDetails
        {
            Role = "assault",
            RoleLowercase = "assault",
            Side = "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
        };
        var template = _cloner.Clone(_botTable.Types["assault"])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);

        var inventory = generator.GenerateInventory(new MongoId(), _sessionId, template, details);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native), "generation did not take the native path");
        Assert.That(inventory.Items, Is.Not.Empty, "the native path generated no inventory");
    }

    [Test]
    public void EligibleGenerationBuildsOffTheResidentDb()
    {
        GenerateWave(_batcher);

        Assert.That(_batcher.LastSendIncludedViewsOverride, Is.False, "an eligible wave must not send the override");
    }

    /// <summary>
    /// The single-bot dispatch site's eligible arm, on the container-resolved generator — the path
    /// every wave that declines batching lands on. Pins that DI selects the epoch-protocol
    /// constructor overload (the frozen one has no publisher, so it could only override) and that a
    /// generation stamped with the resident epoch comes back whole.
    /// </summary>
    [Test]
    public void EligibleSingleBotGenerationBuildsOffTheResidentDb()
    {
        GenerateSingleBot(_botInventoryGenerator);

        Assert.That(
            _botInventoryGenerator.LastSendIncludedViewsOverride,
            Is.False,
            "an eligible single-bot generation must not send the override"
        );
    }

    [Test]
    public void KillSwitchForcesTheViewsOverride()
    {
        _botConfig.DisableNativeRequestCache = true;
        try
        {
            GenerateWave(_batcher);

            Assert.That(_batcher.LastSendIncludedViewsOverride, Is.True, "the kill switch must force the override");
        }
        finally
        {
            _botConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ModsLoadedWithoutTheTrustFlagForceTheViewsOverride()
    {
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = BuildBatcherWithMods(DI.GetInstance(), new SptMod[] { null! });

        _botConfig.TrustNativeRequestCacheWithMods = false;
        try
        {
            GenerateWave(modded);

            Assert.That(modded.LastSendIncludedViewsOverride, Is.True, "a loaded mod without the trust flag disables residency");
        }
        finally
        {
            _botConfig.TrustNativeRequestCacheWithMods = true;
        }
    }

    [Test]
    public void TheTrustFlagKeepsTheResidentPathLiveWithModsLoaded()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }

        var modded = BuildBatcherWithMods(DI.GetInstance(), new SptMod[] { null! });

        _botConfig.TrustNativeRequestCacheWithMods = true;
        try
        {
            GenerateWave(modded);

            Assert.That(
                modded.LastSendIncludedViewsOverride,
                Is.False,
                "the trust flag should keep the resident path live despite the mod"
            );
        }
        finally
        {
            _botConfig.TrustNativeRequestCacheWithMods = true;
        }
    }

    /// <summary>
    /// The single-bot dispatch site's half of the protocol: a generator built on the frozen 4.1.2
    /// constructor has no publisher and no mod list, so every native send carries the override.
    /// </summary>
    [Test]
    public void AGeneratorBuiltOnTheFrozenConstructorAlwaysSendsTheOverride()
    {
        var frozen = BuildGeneratorWithFrozenConstructor(DI.GetInstance());

        GenerateSingleBot(frozen);

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

        GenerateWave(_batcher);

        Assert.That(_batcher.LastSendIncludedViewsOverride, Is.False, "the stale-epoch miss should have republished and retried");
    }

    /// <summary>
    /// The flip's core promise, and the gate on every resident/override mapping choice — the
    /// handbook price values, the default-preset re-key and the exp table (the mod-pool slot order
    /// is out of scope — both arms take it from the one <c>BuildSharedVarying</c> call below, so no
    /// divergence is constructible there): the same seeded wave sent once off the resident DB and
    /// once with the C#-built views override must generate identical bots, compared as normalized
    /// JSON down to every field.
    /// PMC and non-PMC waves both run — only the PMC wave draws levels from the exp table.
    /// </summary>
    [Test]
    public void AResidentSendAndAnOverrideSendProduceIdenticalBotsFieldForField()
    {
        foreach (var isPmc in new[] { true, false })
        {
            var label = isPmc ? "pmc wave resident vs override" : "assault wave resident vs override";
            var request = BuildWaveRequest(isPmc, [Seed, Seed + 1, Seed + 2]);

            var residentResult = ResidentDbDispatch.Send(
                _publisher,
                epoch =>
                {
                    request.Epoch = epoch;
                    return SptNative.GenerateBotInventoryBatch(request);
                }
            );
            Assert.That(residentResult.Bots.All(envelope => envelope.Result is not null), Is.True, $"{label}: a resident bot failed");

            request.Epoch = 0;
            request.ViewsOverride = BotPayloadProjection.BuildViewsOverride(
                _botEquipmentModGenerator.PresetHelper,
                _botLootGenerator.HandbookHelper,
                _itemHelper,
                _botWeaponGenerator.GlobalTable,
                _botEquipmentModGenerator.ItemFilterService,
                _botConfig,
                _pmcConfig,
                _botWeaponGenerator.RepairConfig,
                request.Shared.TemplateVariants!.Select(variant => variant.LootPools)
            );
            var overrideResult = SptNative.GenerateBotInventoryBatch(request);

            LootJsonAssert.AssertEqual(Serialize(residentResult), Serialize(overrideResult), label, Seed);
        }
    }

    /// <summary>
    /// One wave's request body off the live database, shared verbatim by the resident and the
    /// override send. One template variant covers the whole level range — variant banding is the
    /// caller's optimisation, not part of the resident/override contract under test.
    /// </summary>
    private GenerateBotInventoryBatchRequest BuildWaveRequest(bool isPmc, IReadOnlyList<ulong?> seeds)
    {
        var details = new BotGenerationDetails
        {
            Role = isPmc ? "pmcUSEC" : "assault",
            RoleLowercase = isPmc ? "pmcusec" : "assault",
            Side = isPmc ? "Usec" : "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
            IsPmc = isPmc,
        };

        var template = _cloner.Clone(_botTable.Types[isPmc ? "usec" : "assault"])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);
        var lootPools = BotPayloadProjection.BuildLootPools(_botLootGenerator.BotLootCacheService, template, details, _pmcConfig);
        // Non-PMC bots never draw a level, so their wave is the single [1, 1] band with no inputs
        var levelMax = isPmc ? _botWeaponGenerator.GlobalTable.Configuration.Exp.Level.ExperienceTable.Length : 1;

        return new GenerateBotInventoryBatchRequest
        {
            Epoch = 0,
            Shared = BotPayloadProjection.BuildSharedVarying(
                _sessionId,
                _profileHelper,
                _profileActivityService,
                _weatherHelper,
                _botEquipmentModPoolService,
                _itemHelper,
                _botConfig,
                isPmc ? new LevelGenerationView { LevelMin = 1, LevelMax = levelMax } : null,
                [
                    new BotTemplateVariantView
                    {
                        LevelMin = 1,
                        LevelMax = levelMax,
                        Template = BotPayloadProjection.BuildTemplateView(template),
                        LootPools = lootPools,
                    },
                ]
            ),
            Bots = [.. seeds.Select(seed => BotPayloadProjection.BuildBotSlice(new MongoId(), details, seed))],
        };
    }

    /// <summary>
    /// Every generated item carries a fresh <c>MongoId</c>, which never repeats between two calls;
    /// <c>LootIdNormalizer</c> rewrites them to positional placeholders, exactly as the parity
    /// suites compare.
    /// </summary>
    private static string Serialize(object value)
    {
        return LootIdNormalizer.Normalize(JsonSerializer.Serialize(value, JsonUtil.JsonSerializerOptionsNoIndent!));
    }

    private static BotWaveBatcher BuildBatcherWithMods(DI di, IReadOnlyList<SptMod> mods)
    {
        var constructor = typeof(BotWaveBatcher).GetConstructors().Single();

        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
            {
                if (parameter.ParameterType == typeof(IReadOnlyList<SptMod>))
                {
                    return mods;
                }

                return di.GetService(parameter.ParameterType);
            })
            .ToArray();

        return (BotWaveBatcher)constructor.Invoke(arguments);
    }

    private static BotInventoryGenerator BuildGeneratorWithFrozenConstructor(DI di)
    {
        var constructor = typeof(BotInventoryGenerator).GetConstructors().MinBy(ctor => ctor.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => di.GetService(parameter.ParameterType)).ToArray();

        return (BotInventoryGenerator)constructor.Invoke(arguments);
    }
}
