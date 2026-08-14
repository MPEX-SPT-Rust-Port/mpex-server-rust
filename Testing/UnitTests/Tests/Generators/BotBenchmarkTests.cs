using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weapons;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Head-to-head wall clock of the two bot generation paths on the same live database in one process,
/// on one bot per call - the unit the server actually pays. The native path projects the whole items
/// table and every global preset into the request on every call, so BuildRequest is timed on its own
/// as well: that number is the floor under the native path, and the share it takes of it bounds what
/// any projection-side fix could buy. Run in Release; the cargo dev profile makes Debug numbers
/// meaningless.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
[NonParallelizable]
public class BotBenchmarkTests
{
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private static readonly string[] _roles = ["assault", "usec"];

    private BotInventoryGenerator _botInventoryGenerator = default!;
    private BotWeaponGenerator _botWeaponGenerator = default!;
    private BotLootGenerator _botLootGenerator = default!;
    private BotEquipmentModGenerator _botEquipmentModGenerator = default!;
    private BotGeneratorHelper _botGeneratorHelper = default!;
    private ProfileHelper _profileHelper = default!;
    private ItemHelper _itemHelper = default!;
    private WeatherHelper _weatherHelper = default!;
    private ProfileActivityService _profileActivityService = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotInventoryContainerService _botInventoryContainerService = default!;
    private BotConfig _botConfig = default!;
    private PmcConfig _pmcConfig = default!;
    private BotTable _botTable = default!;
    private ICloner _cloner = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _botInventoryGenerator = di.GetService<BotInventoryGenerator>();
        _botWeaponGenerator = di.GetService<BotWeaponGenerator>();
        _botLootGenerator = di.GetService<BotLootGenerator>();
        _botEquipmentModGenerator = di.GetService<BotEquipmentModGenerator>();
        _botGeneratorHelper = di.GetService<BotGeneratorHelper>();
        _profileHelper = di.GetService<ProfileHelper>();
        _itemHelper = di.GetService<ItemHelper>();
        _weatherHelper = di.GetService<WeatherHelper>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _botEquipmentFilterService = di.GetService<BotEquipmentFilterService>();
        _botInventoryContainerService = di.GetService<BotInventoryContainerService>();
        _botConfig = di.GetService<BotConfig>();
        _pmcConfig = di.GetService<PmcConfig>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    [Test]
    public void GenerateInventoryNativeVersusLegacyCSharp()
    {
        foreach (var role in _roles)
        {
            var native = Measure(role, forceLegacy: false, LootGenerationPath.Native);
            var legacy = Measure(role, forceLegacy: true, LootGenerationPath.Legacy);
            var projection = MeasureProjection(role);

            Report($"{role} native (rust)", native);
            Report($"{role} legacy (C# 4.1.2)", legacy);
            Report($"{role} BuildRequest only", projection);
            TestContext.Out.WriteLine(
                $"{role, -8} speedup (median legacy / median native): {Median(legacy) / Median(native):F2}x  "
                    + $"projection share of native median: {Median(projection) / Median(native) * 100:F1}%"
            );
        }
    }

    /// <summary>
    /// One bot per timed run. The case is rebuilt outside the stopwatch because the legacy path
    /// mutates the template it is handed (the forced-armband pool, the equipment mod pool), so a
    /// template reused across runs would stop being the one BotGenerator hands over at :284.
    /// </summary>
    private List<double> Measure(string role, bool forceLegacy, LootGenerationPath expected)
    {
        var original = _botConfig.ForceLegacyBotGeneration;
        _botConfig.ForceLegacyBotGeneration = forceLegacy;
        var timings = new List<double>(TimedRuns);

        try
        {
            // JIT, the native library load and BotLootCacheService hydration are not measured
            for (var run = 0; run < WarmupRuns; run++)
            {
                GenerateOne(role);
            }

            // A benchmark that silently timed the same path twice would look like a 1.00x result
            Assert.That(_botInventoryGenerator.LastPathTaken, Is.EqualTo(expected), "generation did not take the path being measured");

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            for (var run = 0; run < TimedRuns; run++)
            {
                var (template, details) = BuildCase(role);
                var botId = new MongoId();

                var stopwatch = Stopwatch.StartNew();
                _ = _botInventoryGenerator.GenerateInventory(botId, _sessionId, template, details);
                stopwatch.Stop();

                _botInventoryContainerService.ClearCache(botId);
                timings.Add(stopwatch.Elapsed.TotalMilliseconds);
            }

            return timings;
        }
        finally
        {
            _botConfig.ForceLegacyBotGeneration = original;
        }
    }

    /// <summary>
    /// The projection half of the native path on its own - the whole items table as views plus every
    /// global preset, rebuilt per bot.
    /// </summary>
    private List<double> MeasureProjection(string role)
    {
        var timings = new List<double>(TimedRuns);

        for (var run = 0; run < WarmupRuns; run++)
        {
            var (warmupTemplate, warmupDetails) = BuildCase(role);
            _ = BuildRequest(warmupTemplate, warmupDetails);
        }

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        for (var run = 0; run < TimedRuns; run++)
        {
            // Hoisted out of the timed region for the same reason Measure hoists it: the deep clone
            // and FilterBotEquipment are the caller's cost, not the projection's
            var (template, details) = BuildCase(role);

            var stopwatch = Stopwatch.StartNew();
            _ = BuildRequest(template, details);
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
        }

        return timings;
    }

    private GenerateBotInventoryRequest BuildRequest(BotType template, BotGenerationDetails details)
    {
        return BotPayloadProjection.BuildRequest(
            new MongoId(),
            _sessionId,
            template,
            details,
            null,
            _profileHelper,
            _profileActivityService,
            _weatherHelper,
            _botGeneratorHelper,
            _botEquipmentFilterService,
            _botLootGenerator.BotLootCacheService,
            _botEquipmentModGenerator.PresetHelper,
            _botEquipmentModGenerator.ItemFilterService,
            _botLootGenerator.HandbookHelper,
            _itemHelper,
            _botWeaponGenerator.GlobalTable,
            _botConfig,
            _pmcConfig,
            _botWeaponGenerator.RepairConfig
        );
    }

    private void GenerateOne(string role)
    {
        var (template, details) = BuildCase(role);
        var botId = new MongoId();

        _ = _botInventoryGenerator.GenerateInventory(botId, _sessionId, template, details);
        _botInventoryContainerService.ClearCache(botId);
    }

    /// <summary>
    /// The template and details as BotGenerator hands them to GenerateInventory at :284 - matches
    /// BotParityTests.BuildCase.
    /// </summary>
    private (BotType Template, BotGenerationDetails Details) BuildCase(string role)
    {
        var details = role switch
        {
            "assault" => new BotGenerationDetails
            {
                Role = "assault",
                RoleLowercase = "assault",
                Side = "Savage",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotLevel = 1,
            },
            // pmcUSEC/pmcBEAR are the only spellings GetBotEquipmentRole maps onto the pmc config
            "usec" => new BotGenerationDetails
            {
                Role = "pmcUSEC",
                RoleLowercase = "pmcusec",
                Side = "Usec",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotLevel = 1,
                IsPmc = true,
            },
            _ => throw new ArgumentOutOfRangeException(nameof(role), role, "no case defined"),
        };

        var template = _cloner.Clone(_botTable.Types[role])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);

        return (template, details);
    }

    private static void Report(string label, List<double> timings)
    {
        TestContext.Out.WriteLine(
            $"{label, -28} n={timings.Count}  mean={timings.Average():F2} ms  median={Median(timings):F2} ms  "
                + $"min={timings.Min():F2} ms  max={timings.Max():F2} ms"
        );
    }

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }
}
