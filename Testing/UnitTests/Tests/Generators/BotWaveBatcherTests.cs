using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Constants;
using SPTarkov.Server.Core.Controllers;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Profile;
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
    private MatchBotDetailsCacheService _matchBotDetailsCacheService = default!;
    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _batcher = di.GetService<BotWaveBatcher>();
        _botConfig = di.GetService<BotConfig>();
        _matchBotDetailsCacheService = di.GetService<MatchBotDetailsCacheService>();
        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    private static BotGenerationDetails BuildWaveDetails(int count = 3, bool isPmc = false)
    {
        return new BotGenerationDetails
        {
            Role = isPmc ? Sides.PmcUsec : "assault",
            RoleLowercase = isPmc ? "pmcusec" : "assault",
            Side = isPmc ? Sides.Usec : Sides.Savage,
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotCountToGenerate = count,
            IsPmc = isPmc,
        };
    }

    [Test]
    public void ABatchedWaveProducesCompleteBots()
    {
        // Every bot starts as a clone of this, and the post-call draws are what overwrite it
        var untouched = DI.GetInstance().GetService<BotTable>().Base.Customization!;

        var wave = _batcher.TryGenerateWave(_sessionId, BuildWaveDetails());

        Assert.That(wave, Is.Not.Null, "default configuration should take the batch path");
        Assert.That(wave!, Has.Count.EqualTo(3));
        foreach (var bot in wave!)
        {
            Assert.That(bot!.Inventory?.Items, Is.Not.Empty, "bot came back without an inventory");
            Assert.That(bot.Id, Is.Not.EqualTo(default(MongoId)));

            // Voice and appearance are drawn after the native call, from the band the drawn level
            // landed in. assault.json's voice pool does not contain base.json's default, so a
            // missing voice draw is caught per bot.
            Assert.That(bot.Customization!.Voice, Is.Not.Null.And.Not.EqualTo(default(MongoId)), "a bot came back without a voice");
            Assert.That(bot.Customization.Voice, Is.Not.EqualTo(untouched.Voice), "the post-call voice draw never ran");
            Assert.That(bot.Customization.Head, Is.Not.Null.And.Not.EqualTo(default(MongoId)), "a bot came back without a head");
        }

        // The appearance pools do contain base.json's defaults, so per bot a default is a legal
        // draw and only the wave is decidable: with the appearance draw gone every bot keeps every
        // default, which is what this rejects.
        Assert.That(
            wave!.Any(bot =>
                bot!.Customization!.Head != untouched.Head
                || bot.Customization.Body != untouched.Body
                || bot.Customization.Feet != untouched.Feet
                || bot.Customization.Hands != untouched.Hands
            ),
            Is.True,
            "the post-call appearance draw never ran - the whole wave still wears the bots/base.json default"
        );

        // GenerateInventoryId reroots every bot onto a fresh equipment id - all distinct
        Assert.That(wave!.Select(bot => bot!.Inventory!.Equipment).Distinct().Count(), Is.EqualTo(3));
    }

    /// <summary>
    /// The two behaviours an assault wave never reaches: the PMC side rewrite to <c>Savage</c> the
    /// batcher copies from <c>BotController.TryGenerateSingleBot</c>, and <c>GenerateBotFinish</c>'s
    /// dogtag branch, which only fires for the roles in <c>BotConfig.BotRolesWithDogTags</c>.
    ///
    /// Also the only place the level the native side drew is observable end to end: a PMC draws a
    /// real level, and the one member that constrains where the batcher assigns it is
    /// <c>CacheBot</c>, which reads <c>Info.Level</c> (<c>MatchBotDetailsCacheService.cs:54</c>) -
    /// the dogtag branch beside it is level-independent, reading only <c>Info.Side</c> and
    /// <c>Info.GameVersion</c>. So the cached copy pins both the assignment and its ordering ahead
    /// of the caching step.
    /// </summary>
    [Test]
    public void APmcWaveIsRewrittenToSavageAndKeepsItsDogtag()
    {
        var wave = _batcher.TryGenerateWave(_sessionId, BuildWaveDetails(isPmc: true));

        // No raid configuration is set here, so the nighttime clamp cannot fire and the wave batches
        Assert.That(wave, Is.Not.Null, "a PMC wave should take the batch path");
        Assert.That(wave!, Has.Count.EqualTo(3));
        foreach (var bot in wave!)
        {
            Assert.That(bot!.Info!.Side, Is.EqualTo(Sides.Savage));
            Assert.That(bot.Inventory!.Items!.Any(item => item.SlotId == Slots.Dogtag), Is.True, "a PMC came back without a dogtag");

            Assert.That(bot.Info.Level, Is.GreaterThan(0), "a batched PMC came back without the level the native side drew");
            Assert.That(bot.Info.Experience, Is.Not.Null, "a batched PMC came back without its experience total");
            Assert.That(
                _matchBotDetailsCacheService.GetBotById(bot.Id)?.Level,
                Is.EqualTo(bot.Info.Level),
                "CacheBot ran before the envelope's level was assigned"
            );
        }
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
    /// The level draw moved into the native call, so a mod-registered BotLevelGenerator is a
    /// substitution only the per-bot path routes through - same contract as the BotGenerator one.
    /// </summary>
    [Test]
    public void ASubstitutedBotLevelGeneratorDeclinesTheBatch()
    {
        var batcher = (BotWaveBatcher)Construct(typeof(BotWaveBatcher), Construct(typeof(TestBotLevelGeneratorSubclass)));

        Assert.That(batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
    }

    /// <summary>
    /// The batch runs the equipment filter once per level band instead of once per bot, so a
    /// substituted BotEquipmentFilterService would be called a different number of times with
    /// different levels - per-bot only.
    /// </summary>
    [Test]
    public void ASubstitutedEquipmentFilterServiceDeclinesTheBatch()
    {
        var batcher = (BotWaveBatcher)Construct(typeof(BotWaveBatcher), Construct(typeof(TestBotEquipmentFilterServiceSubclass)));

        Assert.That(batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
    }

    /// <summary>
    /// GenerateBotLevel is frozen for the same reason GenerateBot is: the batch draws the level
    /// natively, so a live Harmony patch on it would never fire.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnBotLevelGeneratorDeclinesTheBatch()
    {
        var harmony = new Harmony("unit-tests.botwave-batcher.GenerateBotLevel");
        var generateBotLevel = typeof(BotLevelGenerator).GetMethod(nameof(BotLevelGenerator.GenerateBotLevel));
        Assert.That(generateBotLevel, Is.Not.Null, "frozen member BotLevelGenerator.GenerateBotLevel not found");

        try
        {
            harmony.Patch(generateBotLevel, prefix: new HarmonyMethod(typeof(BotWaveBatcherTests), nameof(Prefix)));

            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null, "a patched wave must run per bot");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The batch runs the seasonal strip once per level band instead of once per bot, so a live
    /// patch on either christmas member declines. SeasonalEventService is in the decline set
    /// member-scoped rather than whole-type, so this also pins that both lookups still resolve - a
    /// lookup that stopped resolving would drop out of the set silently.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnTheSeasonalStripDeclinesTheBatch()
    {
        string[] members =
        [
            nameof(SeasonalEventService.ChristmasEventEnabled),
            nameof(SeasonalEventService.RemoveChristmasItemsFromBotInventory),
        ];

        foreach (var name in members)
        {
            var harmony = new Harmony($"unit-tests.botwave-batcher.{name}");
            var member = typeof(SeasonalEventService).GetMethod(name);
            Assert.That(member, Is.Not.Null, $"frozen member SeasonalEventService.{name} not found");

            try
            {
                harmony.Patch(member, prefix: new HarmonyMethod(typeof(BotWaveBatcherTests), nameof(Prefix)));

                Assert.That(
                    _batcher.TryGenerateWave(_sessionId, BuildWaveDetails()),
                    Is.Null,
                    $"a wave with {name} patched must run per bot"
                );
            }
            finally
            {
                harmony.UnpatchSelf();
            }
        }
    }

    /// <summary>
    /// The batch inherits BotInventoryGenerator's whole decline set through UseLegacyPath
    /// (BotWaveBatcher.CanBatch), so a patch on the pool service - a member of that set, not of the
    /// batcher's own wave members - must de-batch the wave too. Pinned directly because the
    /// composition ("pool patch forces UseLegacyPath" plus "UseLegacyPath declines the batch") is
    /// otherwise proven only by two separate tests, which a member-scoped UseLegacyPath refactor
    /// could break without failing either.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnTheModPoolServiceDeclinesTheBatch()
    {
        var harmony = new Harmony("unit-tests.botwave-batcher.GetRequiredModsForWeaponSlot");
        var member = typeof(BotEquipmentModPoolService).GetMethod(nameof(BotEquipmentModPoolService.GetRequiredModsForWeaponSlot));
        Assert.That(member, Is.Not.Null, "frozen member BotEquipmentModPoolService.GetRequiredModsForWeaponSlot not found");

        try
        {
            harmony.Patch(member, prefix: new HarmonyMethod(typeof(BotWaveBatcherTests), nameof(Prefix)));

            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null, "a pool-service patch must de-batch the wave");
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

    /// <summary>
    /// The segmentation the variants are built from: every band edge the pre-call lookups read is a
    /// cut, and nothing else is. A missed cut ships a variant filtered at the wrong level, which is
    /// a silent generation difference rather than a crash - so these are pinned directly.
    /// </summary>
    [Test]
    public void ANonPmcWaveIsOneSegmentAtLevelOne()
    {
        var segments = BotWaveBatcher.EnumerateLevelSegments(
            BuildWaveDetails(),
            new MinMax<int>(1, 79),
            new EquipmentFilters { Blacklist = [new EquipmentFilterDetails { LevelRange = new MinMax<int>(10, 20) }] },
            [LootBand(1.5, 10.5)]
        );

        Assert.That(segments, Is.EqualTo(new List<MinMax<int>> { new(1, 1) }), "non-PMC bots never draw a level");
    }

    [Test]
    public void AWaveWithNoBandsIsOneSegmentCoveringTheRange()
    {
        var segments = BotWaveBatcher.EnumerateLevelSegments(BuildWaveDetails(isPmc: true), new MinMax<int>(5, 30), null, []);

        Assert.That(segments, Is.EqualTo(new List<MinMax<int>> { new(5, 30) }));
    }

    [Test]
    public void ABandInsideTheRangeCutsItAtBothEdges()
    {
        var segments = BotWaveBatcher.EnumerateLevelSegments(
            BuildWaveDetails(isPmc: true),
            new MinMax<int>(5, 30),
            new EquipmentFilters { Blacklist = [new EquipmentFilterDetails { LevelRange = new MinMax<int>(10, 20) }] },
            []
        );

        Assert.That(segments, Is.EqualTo(new List<MinMax<int>> { new(5, 9), new(10, 20), new(21, 30) }));
    }

    [Test]
    public void OverlappingBandsCutAtEveryEdge()
    {
        var segments = BotWaveBatcher.EnumerateLevelSegments(
            BuildWaveDetails(isPmc: true),
            new MinMax<int>(1, 15),
            new EquipmentFilters
            {
                Whitelist = [new EquipmentFilterDetails { LevelRange = new MinMax<int>(1, 10) }],
                Randomisation = [new RandomisationDetails { LevelRange = new MinMax<int>(5, 20) }],
            },
            []
        );

        Assert.That(segments, Is.EqualTo(new List<MinMax<int>> { new(1, 4), new(5, 10), new(11, 15) }));
    }

    [Test]
    public void AGapBetweenBandsIsItsOwnSegment()
    {
        var segments = BotWaveBatcher.EnumerateLevelSegments(
            BuildWaveDetails(isPmc: true),
            new MinMax<int>(1, 15),
            new EquipmentFilters
            {
                WeightingAdjustmentsByBotLevel =
                [
                    new WeightingAdjustmentDetails { LevelRange = new MinMax<int>(1, 5) },
                    new WeightingAdjustmentDetails { LevelRange = new MinMax<int>(10, 15) },
                ],
            },
            []
        );

        Assert.That(segments, Is.EqualTo(new List<MinMax<int>> { new(1, 5), new(6, 9), new(10, 15) }));
    }

    /// <summary>
    /// Loot price bands are double-valued (PmcConfig.cs:139), so the integer levels they cover start
    /// at ceil(min) and end at floor(max) - (1.5, 10.5) covers levels 2 to 10.
    /// </summary>
    [Test]
    public void ADoubleValuedLootBandCutsAtItsIntegerEdges()
    {
        var segments = BotWaveBatcher.EnumerateLevelSegments(
            BuildWaveDetails(isPmc: true),
            new MinMax<int>(1, 15),
            null,
            [LootBand(1.5, 10.5)]
        );

        Assert.That(segments, Is.EqualTo(new List<MinMax<int>> { new(1, 1), new(2, 10), new(11, 15) }));
    }

    [Test]
    public void BandsOutsideTheRangeChangeNothing()
    {
        var segments = BotWaveBatcher.EnumerateLevelSegments(
            BuildWaveDetails(isPmc: true),
            new MinMax<int>(10, 20),
            new EquipmentFilters
            {
                Blacklist =
                [
                    new EquipmentFilterDetails { LevelRange = new MinMax<int>(1, 5) },
                    new EquipmentFilterDetails { LevelRange = new MinMax<int>(50, 60) },
                ],
            },
            [LootBand(70, 80)]
        );

        Assert.That(segments, Is.EqualTo(new List<MinMax<int>> { new(10, 20) }));
    }

    /// <summary>
    /// Only the level range matters here; the three per-container bands are required members the
    /// segmentation never reads.
    /// </summary>
    private static MinMaxLootItemValue LootBand(double min, double max)
    {
        return new MinMaxLootItemValue
        {
            Min = min,
            Max = max,
            Backpack = new MinMax<double>(0, 0),
            Pocket = new MinMax<double>(0, 0),
            Vest = new MinMax<double>(0, 0),
        };
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

    /// <summary>
    /// Stands in for a mod-registered level generator: identical behaviour, different type.
    /// </summary>
    private class TestBotLevelGeneratorSubclass(GlobalTable globalTable, RandomUtil randomUtil)
        : BotLevelGenerator(globalTable, randomUtil) { }

    /// <summary>
    /// Stands in for a mod-registered equipment filter service: identical behaviour, different type.
    /// </summary>
    private class TestBotEquipmentFilterServiceSubclass(BotHelper botHelper, ProfileHelper profileHelper, BotConfig botConfig)
        : BotEquipmentFilterService(botHelper, profileHelper, botConfig) { }
}
