using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Controllers;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Match;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the batched wave path: a default wave comes back complete, and each reason the batcher
/// declines hands the caller null so it falls through to the per-bot path. Mutates the shared config
/// singleton, so it restores it and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotWaveBatcherTests
{
    private BotWaveBatcher _batcher = default!;
    private BotConfig _botConfig = default!;
    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _batcher = di.GetService<BotWaveBatcher>();
        _botConfig = di.GetService<BotConfig>();
        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    private static BotGenerationDetails BuildWaveDetails(int count = 3)
    {
        return new BotGenerationDetails
        {
            Role = "assault",
            Side = "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotCountToGenerate = count,
        };
    }

    [Test]
    public void ABatchedWaveProducesCompleteBots()
    {
        var wave = _batcher.TryGenerateWave(_sessionId, BuildWaveDetails());

        Assert.That(wave, Is.Not.Null, "default configuration should take the batch path");
        Assert.That(wave!, Has.Count.EqualTo(3));
        foreach (var bot in wave!)
        {
            Assert.That(bot!.Inventory?.Items, Is.Not.Empty, "bot came back without an inventory");
            Assert.That(bot.Id, Is.Not.EqualTo(default(MongoId)));
        }

        // GenerateInventoryId reroots every bot onto a fresh equipment id - all distinct
        Assert.That(wave!.Select(bot => bot!.Inventory!.Equipment).Distinct().Count(), Is.EqualTo(3));
    }

    [Test]
    public void ForcePerBotGenerationDeclinesTheBatch()
    {
        _botConfig.ForcePerBotGeneration = true;
        try
        {
            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
        }
        finally
        {
            _botConfig.ForcePerBotGeneration = false;
        }
    }

    [Test]
    public void ForceLegacyBotGenerationDeclinesTheBatch()
    {
        _botConfig.ForceLegacyBotGeneration = true;
        try
        {
            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
        }
        finally
        {
            _botConfig.ForceLegacyBotGeneration = false;
        }
    }

    /// <summary>
    /// The negative control for the substitution test below: hand-building the batcher off the
    /// container's own services is not by itself a reason to decline.
    /// </summary>
    [Test]
    public void AHandBuiltBatcherWithStockServicesTakesTheBatchPath()
    {
        var batcher = (BotWaveBatcher)Construct(typeof(BotWaveBatcher));

        var wave = batcher.TryGenerateWave(_sessionId, BuildWaveDetails());

        Assert.That(wave, Is.Not.Null, "a hand-built batcher with stock services should still batch");
        Assert.That(wave!, Has.Count.EqualTo(3));
        Assert.That(wave!.All(bot => bot!.Inventory?.Items?.Count > 0), Is.True, "a bot came back without an inventory");
    }

    /// <summary>
    /// A mod registering its own BotGenerator with a higher TypePriority hands the container a
    /// subclass, which only the per-bot path routes through.
    /// </summary>
    [Test]
    public void ASubstitutedBotGeneratorDeclinesTheBatch()
    {
        var batcher = (BotWaveBatcher)Construct(typeof(BotWaveBatcher), Construct(typeof(TestBotGeneratorSubclass)));

        Assert.That(batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
    }

    /// <summary>
    /// A live Harmony patch on a frozen member of BotGenerator means a mod expects per-bot
    /// semantics. Harmony patches are process-wide, so the patch is removed in a finally.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnGenerateBotDeclinesTheBatch()
    {
        var harmony = new Harmony("unit-tests.botwave-batcher.GenerateBot");
        var generateBot = typeof(BotGenerator).GetMethod("GenerateBot", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.That(generateBot, Is.Not.Null, "frozen member BotGenerator.GenerateBot not found");

        try
        {
            harmony.Patch(generateBot, prefix: new HarmonyMethod(typeof(BotWaveBatcherTests), nameof(Prefix)));

            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null, "a patched wave must run per bot");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The nighttime equipment clamp is a cross-bot feedback loop through the live BotConfig that
    /// only the per-bot path replays, so a nighttime wave whose role carries nighttime modifiers
    /// declines. The same config by day still batches - the decline is about the clamp firing, not
    /// about the config existing.
    /// </summary>
    [Test]
    public void ANighttimeWaveWithNighttimeChangesDeclinesTheBatch()
    {
        var di = DI.GetInstance();
        var raidData = di.GetService<ProfileActivityService>().GetProfileActivityRaidData(_sessionId);
        var weatherHelper = di.GetService<WeatherHelper>();

        // Premise: these inputs must read as night; factory4_night is night at any hour. If this
        // assert fails, fix the raid configuration inputs, not the dispatcher.
        var raidConfig = new GetRaidConfigurationRequestData { Location = "factory4_night", TimeVariant = DateTimeEnum.CURR };
        Assert.That(weatherHelper.IsNightTime(raidConfig.TimeVariant, raidConfig.Location!), Is.True);

        var equipConfig = _botConfig.Equipment["assault"]!;
        var previousRandomisation = equipConfig.Randomisation;
        equipConfig.Randomisation =
        [
            new RandomisationDetails
            {
                LevelRange = new MinMax<int> { Min = 1, Max = 99 },
                EquipmentMods = new Dictionary<string, double> { ["mod_nvg"] = 50 },
                NighttimeChanges = new NighttimeChanges { EquipmentModsModifiers = new Dictionary<string, float> { ["mod_nvg"] = 30 } },
            },
        ];
        raidData.RaidConfiguration = raidConfig;
        try
        {
            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null, "nighttime clamp wave must run per bot");

            // Same config by day still batches - the fallback is about the clamp, not the config
            raidData.RaidConfiguration = null;
            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Not.Null);
        }
        finally
        {
            equipConfig.Randomisation = previousRandomisation;
            raidData.RaidConfiguration = null;
        }
    }

    /// <summary>
    /// The production wiring: the container must pick BotController's additive constructor, not the
    /// frozen 4.1.2 one. Without this the dispatch branch is a silent no-op and every other test
    /// here still passes, because they all call the batcher directly.
    /// </summary>
    [Test]
    public void AResolvedBotControllerGetsTheBatcher()
    {
        var controller = DI.GetInstance().GetService<BotController>();
        var batcher = typeof(BotController).GetField("_botWaveBatcher", BindingFlags.Instance | BindingFlags.NonPublic);

        Assert.That(
            batcher!.GetValue(controller),
            Is.Not.Null,
            "the container fell back to the frozen 14-parameter constructor - waves never batch"
        );
    }

    private static void Prefix() { }

    private static object Construct(Type type, params object[] substitutes)
    {
        var constructor = type.GetConstructors().Single();
        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
                substitutes.FirstOrDefault(substitute => parameter.ParameterType.IsInstanceOfType(substitute))
                ?? DI.GetInstance().GetService(parameter.ParameterType)
            )
            .ToArray();

        return constructor.Invoke(arguments);
    }

    /// <summary>
    /// Stands in for a mod-registered generator: identical behaviour, different type.
    /// </summary>
    private class TestBotGeneratorSubclass(
        ISptLogger<BotGenerator> logger,
        TemplateTable templateTable,
        GlobalTable globalTable,
        BotTable botTable,
        RandomUtil randomUtil,
        BotInventoryGenerator botInventoryGenerator,
        BotLevelGenerator botLevelGenerator,
        BotEquipmentFilterService botEquipmentFilterService,
        WeightedRandomHelper weightedRandomHelper,
        BotHelper botHelper,
        SeasonalEventService seasonalEventService,
        ItemFilterService itemFilterService,
        BotNameService botNameService,
        BotConfig botConfig,
        PmcConfig pmcConfig,
        ICloner cloner
    )
        : BotGenerator(
            logger,
            templateTable,
            globalTable,
            botTable,
            randomUtil,
            botInventoryGenerator,
            botLevelGenerator,
            botEquipmentFilterService,
            weightedRandomHelper,
            botHelper,
            seasonalEventService,
            itemFilterService,
            botNameService,
            botConfig,
            pmcConfig,
            cloner
        ) { }
}
