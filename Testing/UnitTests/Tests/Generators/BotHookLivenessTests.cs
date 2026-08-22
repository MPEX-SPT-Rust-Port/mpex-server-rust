using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the mod hook contract across the five types in the decline set - the four classes the native
/// bot path replaces, plus BotEquipmentModPoolService, which it no longer consults now that Rust owns
/// the mod pools: a Harmony patch on a hookable member of any of them must actually fire during
/// generation - patch detection routes the call to the legacy path. A patch on the dispatcher itself
/// is the exception, it wraps whichever path runs. Harmony patches are process-wide, so every patch
/// is removed in a finally and the fixture never runs in parallel with others.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotHookLivenessTests
{
    // Weapons, money loot and mod-bearing equipment are all chance rolls, so a patch on a member
    // that only some bots reach needs more than one bot before it is allowed to be called dead
    private const int MaxBots = 10;

    private static bool _patchFired;
    private static bool _prefixFired;
    private static bool _postfixFired;

    private BotInventoryGenerator _botInventoryGenerator = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotTable _botTable = default!;
    private ICloner _cloner = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _botInventoryGenerator = di.GetService<BotInventoryGenerator>();
        _botEquipmentFilterService = di.GetService<BotEquipmentFilterService>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    /// <summary>
    /// SortModKeys is a public helper rather than one of the entry points - it is here to prove the
    /// hookable set was widened past the methods the native path directly replaces.
    /// </summary>
    [Test]
    public void HarmonyPatchOnBotEquipmentModGeneratorFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(BotEquipmentModGenerator), nameof(BotEquipmentModGenerator.SortModKeys));
    }

    [Test]
    public void HarmonyPatchOnBotWeaponGeneratorFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(BotWeaponGenerator), nameof(BotWeaponGenerator.PickWeightedWeaponTemplateFromPool));
    }

    [Test]
    public void HarmonyPatchOnBotLootGeneratorFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(BotLootGenerator), nameof(BotLootGenerator.RandomiseMoneyStackSize));
    }

    [Test]
    public void HarmonyPatchOnBotInventoryGeneratorFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(BotInventoryGenerator), nameof(BotInventoryGenerator.GenerateEquipment));
    }

    /// <summary>
    /// Rust owns the mod pools outright since ABI 32 - contents and ordering both - so a mod
    /// patching the service can only take effect on the legacy path. Before ABI 32 such a patch was
    /// silently ignored, which is the Broken-ledger entry this closes.
    /// </summary>
    [Test]
    public void HarmonyPatchOnBotEquipmentModPoolServiceFiresAndForcesTheLegacyPath()
    {
        // GetRequiredModsForWeaponSlot, not one of the other three getters: this fixture builds a
        // level-1 assault, bot.json gives assault no randomisation block at all, and every other
        // legacy reach into the service is behind a randomisation gate. This one sits on the
        // complementary !isRandomisableSlot branch, which is the branch this bot takes.
        AssertPatchForcesLegacyPath(typeof(BotEquipmentModPoolService), nameof(BotEquipmentModPoolService.GetRequiredModsForWeaponSlot));
    }

    /// <summary>
    /// The dispatcher is deliberately not in the hookable set: a patch on it wraps whichever path
    /// runs, so it keeps the native body and still sees the call.
    /// </summary>
    [Test]
    public void HarmonyPatchOnGenerateInventoryWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.bot-hook-liveness.dispatcher");
        var target = typeof(BotInventoryGenerator).GetMethod(nameof(BotInventoryGenerator.GenerateInventory));
        Assert.That(target, Is.Not.Null, "GenerateInventory not found");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(BotHookLivenessTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(BotHookLivenessTests), nameof(Postfix))
            );

            var inventory = Generate();

            Assert.That(
                _botInventoryGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Native),
                "a patch on the dispatcher forced legacy"
            );
            Assert.That(inventory.Items, Is.Not.Empty);
            Assert.That(_prefixFired, Is.True, "prefix on GenerateInventory never ran");
            Assert.That(_postfixFired, Is.True, "postfix on GenerateInventory never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    private void AssertPatchForcesLegacyPath(Type declaringType, string methodName)
    {
        var harmony = new Harmony($"unit-tests.bot-hook-liveness.{declaringType.Name}.{methodName}");
        var target = declaringType.GetMethod(methodName);
        Assert.That(target, Is.Not.Null, $"frozen member {declaringType.Name}.{methodName} not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(BotHookLivenessTests), nameof(PatchFired)));

            for (var i = 0; i < MaxBots && !_patchFired; i++)
            {
                var inventory = Generate();

                Assert.That(
                    _botInventoryGenerator.LastPathTaken,
                    Is.EqualTo(LootGenerationPath.Legacy),
                    $"a patch on {declaringType.Name}.{methodName} did not force the legacy path"
                );
                Assert.That(inventory.Items, Is.Not.Empty);
            }

            Assert.That(_patchFired, Is.True, $"postfix on {declaringType.Name}.{methodName} never ran across {MaxBots} bots");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    private BotBaseInventory Generate()
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

        return _botInventoryGenerator.GenerateInventory(new MongoId(), _sessionId, template, details);
    }

    private static void PatchFired()
    {
        _patchFired = true;
    }

    private static void Prefix()
    {
        _prefixFired = true;
    }

    private static void Postfix()
    {
        _postfixFired = true;
    }
}
