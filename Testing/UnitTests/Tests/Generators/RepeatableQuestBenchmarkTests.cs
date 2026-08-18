using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.RepeatableQuests;
using SPTarkov.Server.Core.Helpers.Quest;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Repeatable;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.RepeatableQuests;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils.Cloners;
using SPTarkov.Server.Core.Utils.Collections;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Head-to-head wall clock of the two repeatable-quest generation paths on the same live database in
/// one process, for every quest type the four generators produce. One quest is a millisecond of C#
/// work, so the native path's cost is dominated by keeping the resident DB current - which is why
/// the native side is measured twice: with <c>DatabaseMutationStamp</c> bumped before every run so
/// each pass republishes the whole database first (publish cold), and with the stamp left alone so
/// the send carries the varying fields only and generates off the already-derived resident views
/// (publish warm, the shape a stock server runs). <c>BuildViewsOverride</c> is timed on its own as
/// a fourth phase: the C# projection an ineligible (modded, untrusted) send pays per call.
///
/// Run in Release; the cargo dev profile makes Debug numbers meaningless.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
[NonParallelizable]
public class RepeatableQuestBenchmarkTests
{
    // One quest is milliseconds, so the run count matches the loot and bot fixtures rather than
    // ragfair's 5
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private static readonly string[] _questTypes = ["Elimination", "Completion", "Exploration", "Pickup"];

    private static readonly MongoId _sessionId = new("6193a720f8ee7e52e4290000");

    private EliminationQuestGenerator _eliminationQuestGenerator = default!;
    private CompletionQuestGenerator _completionQuestGenerator = default!;
    private ExplorationQuestGenerator _explorationQuestGenerator = default!;
    private PickupQuestGenerator _pickupQuestGenerator = default!;
    private RepeatableQuestHelper _repeatableQuestHelper = default!;
    private RepeatableQuestNativeRequestBuilder _builder = default!;
    private QuestConfig _questConfig = default!;
    private DatabaseMutationStamp _databaseMutationStamp = default!;
    private DbPublisher _dbPublisher = default!;
    private ICloner _cloner = default!;

    /// <summary> The pmc daily config - the one every type but Pickup is generated from </summary>
    private RepeatableQuestConfig _pmcDaily = default!;

    /// <summary> The only shipped config carrying a <c>Pickup</c> block; both paths throw without it </summary>
    private RepeatableQuestConfig _pickupConfig = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _eliminationQuestGenerator = di.GetService<EliminationQuestGenerator>();
        _completionQuestGenerator = di.GetService<CompletionQuestGenerator>();
        _explorationQuestGenerator = di.GetService<ExplorationQuestGenerator>();
        _pickupQuestGenerator = di.GetService<PickupQuestGenerator>();
        _repeatableQuestHelper = di.GetService<RepeatableQuestHelper>();
        _builder = di.GetService<RepeatableQuestNativeRequestBuilder>();
        _questConfig = di.GetService<QuestConfig>();
        _databaseMutationStamp = di.GetService<DatabaseMutationStamp>();
        _dbPublisher = di.GetService<DbPublisher>();
        _cloner = di.GetService<ICloner>();

        _pmcDaily = _questConfig.RepeatableQuests.First(config => config.Side == PlayerGroup.Pmc);
        _pickupConfig = _questConfig.RepeatableQuests.First(config => config.QuestConfig.Pickup is not null);
    }

    [TearDown]
    public void TearDown()
    {
        _questConfig.ForceLegacyRepeatableQuestGeneration = false;
    }

    [Test]
    public void GenerateQuestNativeVersusLegacyCSharp()
    {
        foreach (var questType in _questTypes)
        {
            RunScenario(questType);
        }

        Report("BuildViewsOverride only", MeasureViewsOverrideProjection());
    }

    private void RunScenario(string questType)
    {
        var repeatableConfig = questType == "Pickup" ? _pickupConfig : _pmcDaily;
        var generator = GeneratorFor(questType);
        var pmcLevel = LevelForBand(repeatableConfig, questType);
        var traderId = TraderForType(repeatableConfig, questType);

        var legacy = Measure(generator, repeatableConfig, pmcLevel, traderId, forceLegacy: true);

        var epochBeforeCold = _dbPublisher.EnsureCurrent();
        var cold = Measure(generator, repeatableConfig, pmcLevel, traderId, forceLegacy: false, () => _databaseMutationStamp.Bump());
        Assert.That(
            _dbPublisher.EnsureCurrent(),
            Is.GreaterThan(epochBeforeCold),
            "the cold arm never advanced the resident-DB epoch - it measured the warm path"
        );

        var warm = Measure(generator, repeatableConfig, pmcLevel, traderId, forceLegacy: false);

        Report($"{questType} legacy (C# 4.1.2)", legacy);
        Report($"{questType} native, publish cold", cold);
        Report($"{questType} native, publish warm", warm);
        TestContext.Out.WriteLine(
            $"{questType, -12} speedup (median legacy / median native warm): {Median(legacy.Timings) / Median(warm.Timings):F2}x  "
                + $"publish cost per send (median cold - median warm): {Median(cold.Timings) - Median(warm.Timings):F2} ms"
        );
    }

    /// <param name="beforeEachRun">
    ///     Runs outside the timed region, before every warmup and timed run. The cold arm bumps
    ///     <c>DatabaseMutationStamp</c> here, so every pass republishes the whole database first;
    ///     the warm arm leaves the stamp alone and generates off the already-derived resident views.
    /// </param>
    private RunStats Measure(
        IRepeatableQuestGenerator generator,
        RepeatableQuestConfig repeatableConfig,
        int pmcLevel,
        MongoId traderId,
        bool forceLegacy,
        Action? beforeEachRun = null
    )
    {
        _questConfig.ForceLegacyRepeatableQuestGeneration = forceLegacy;

        var timings = new List<double>(TimedRuns);

        try
        {
            // JIT, the native library load and the first lazy-load deserialise are not measured
            for (var run = 0; run < WarmupRuns; run++)
            {
                beforeEachRun?.Invoke();
                _ = generator.Generate(_sessionId, pmcLevel, traderId, BuildPool(repeatableConfig, pmcLevel), repeatableConfig);
            }

            // A benchmark that silently timed the same path twice would look like a 1.00x result
            var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
            Assert.That(PathTaken(generator), Is.EqualTo(expected), "generation did not take the path being measured");

            if (!forceLegacy)
            {
                Assert.That(_builder.LastSendIncludedViewsOverride, Is.False, "both native arms must ride the resident path");
            }

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            var allocatedBefore = GC.GetTotalAllocatedBytes(precise: true);

            for (var run = 0; run < TimedRuns; run++)
            {
                // The generators consume the pool they are handed, so it is rebuilt per run -
                // outside the timed region, the way the bot fixture hoists its template clone
                var pool = BuildPool(repeatableConfig, pmcLevel);
                beforeEachRun?.Invoke();

                var stopwatch = Stopwatch.StartNew();
                _ = generator.Generate(_sessionId, pmcLevel, traderId, pool, repeatableConfig);
                stopwatch.Stop();

                timings.Add(stopwatch.Elapsed.TotalMilliseconds);
            }

            return new RunStats(timings, (GC.GetTotalAllocatedBytes(precise: true) - allocatedBefore) / (double)TimedRuns / 1024 / 1024);
        }
        finally
        {
            _questConfig.ForceLegacyRepeatableQuestGeneration = false;
        }
    }

    /// <summary>
    /// The C# projection an ineligible (modded, untrusted) send pays per call: both price maps over
    /// the whole items table, the items view, every default weapon preset, the boss spawns and
    /// extracts of every location. The serialise and the native-side parse are not in here.
    /// </summary>
    private RunStats MeasureViewsOverrideProjection()
    {
        var timings = new List<double>(TimedRuns);

        for (var run = 0; run < WarmupRuns; run++)
        {
            _ = _builder.BuildViewsOverride();
        }

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        var allocatedBefore = GC.GetTotalAllocatedBytes(precise: true);

        for (var run = 0; run < TimedRuns; run++)
        {
            var stopwatch = Stopwatch.StartNew();
            _ = _builder.BuildViewsOverride();
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
        }

        return new RunStats(timings, (GC.GetTotalAllocatedBytes(precise: true) - allocatedBefore) / (double)TimedRuns / 1024 / 1024);
    }

    /// <summary>
    /// <c>RepeatableQuestController.GenerateQuestPool</c> (<c>:840-885</c>) - what the generators are
    /// handed in production, rebuilt here.
    /// </summary>
    private QuestTypePool BuildPool(RepeatableQuestConfig repeatableConfig, int pmcLevel)
    {
        var pool = new QuestTypePool
        {
            Types = _cloner.Clone(repeatableConfig.Types)!,
            Pool = new QuestPool
            {
                Exploration = new ExplorationPool { Locations = new Dictionary<ELocationName, List<string>>() },
                Elimination = new EliminationPool { Targets = new Dictionary<string, TargetLocation>() },
                Pickup = new ExplorationPool { Locations = new Dictionary<ELocationName, List<string>>() },
            },
        };

        foreach (var (location, value) in repeatableConfig.Locations)
        {
            if (location != ELocationName.any)
            {
                pool.Pool.Exploration.Locations![location] = value;
                pool.Pool.Pickup.Locations![location] = value;
            }
        }

        pool.Pool.Pickup.Locations![ELocationName.any] = ["any"];

        var eliminationConfig = _repeatableQuestHelper.GetEliminationConfigByPmcLevel(pmcLevel, repeatableConfig)!;

        foreach (var target in new ProbabilityObjectArray<string, BossInfo>(_cloner, eliminationConfig.Targets))
        {
            if (target.Data?.IsBoss ?? false)
            {
                pool.Pool.Elimination.Targets!.Add(target.Key!, new TargetLocation { Locations = ["any"] });

                continue;
            }

            var allowedLocations =
                target.Key == "Savage"
                    ? repeatableConfig.Locations.Keys.Where(location => location != ELocationName.laboratory)
                    : repeatableConfig.Locations.Keys;

            pool.Pool.Elimination.Targets!.Add(
                target.Key!,
                new TargetLocation { Locations = allowedLocations.Select(location => location.ToString()).ToList() }
            );
        }

        return pool;
    }

    /// <summary>
    /// The midpoint of the second shipped level band for the type, so the workload tracks the data
    /// rather than an edge this fixture invents. Pickup ships no bands of its own, but
    /// <see cref="BuildPool"/> reads the elimination bands for every type, so its level has to land
    /// inside one of those.
    /// </summary>
    private static int LevelForBand(RepeatableQuestConfig repeatableConfig, string questType)
    {
        var bands = questType switch
        {
            "Elimination" or "Pickup" => repeatableConfig.QuestConfig.Elimination.Select(config => config.LevelRange).ToList(),
            "Completion" => repeatableConfig.QuestConfig.CompletionConfig.Select(config => config.LevelRange).ToList(),
            "Exploration" => repeatableConfig.QuestConfig.ExplorationConfig.Select(config => config.LevelRange).ToList(),
            _ => throw new ArgumentOutOfRangeException(nameof(questType), questType, "no level bands for this type"),
        };

        Assert.That(bands, Has.Count.GreaterThan(1), $"{questType} ships fewer than two level bands");

        return (bands[1].Min + bands[1].Max) / 2;
    }

    private static MongoId TraderForType(RepeatableQuestConfig repeatableConfig, string questType)
    {
        var traders = repeatableConfig.TraderWhitelist.Where(whitelist => whitelist.QuestTypes.Contains(questType)).ToList();

        Assert.That(traders, Is.Not.Empty, $"{questType} is whitelisted for no trader");

        return traders[0].TraderId;
    }

    private IRepeatableQuestGenerator GeneratorFor(string questType)
    {
        return questType switch
        {
            "Elimination" => _eliminationQuestGenerator,
            "Completion" => _completionQuestGenerator,
            "Exploration" => _explorationQuestGenerator,
            "Pickup" => _pickupQuestGenerator,
            _ => throw new ArgumentOutOfRangeException(nameof(questType), questType, "no generator for this type"),
        };
    }

    // The four generators share no base type, so the Task 15 path seam is reached by pattern match
    private static LootGenerationPath PathTaken(IRepeatableQuestGenerator generator)
    {
        return generator switch
        {
            EliminationQuestGenerator elimination => elimination.LastPathTaken,
            CompletionQuestGenerator completion => completion.LastPathTaken,
            ExplorationQuestGenerator exploration => exploration.LastPathTaken,
            PickupQuestGenerator pickup => pickup.LastPathTaken,
            _ => throw new ArgumentOutOfRangeException(nameof(generator), generator, "no path seam on this generator"),
        };
    }

    private static void Report(string label, RunStats stats)
    {
        TestContext.Out.WriteLine(
            $"{label, -30} n={stats.Timings.Count}  mean={stats.Timings.Average():F2} ms  median={Median(stats.Timings):F2} ms  "
                + $"min={stats.Timings.Min():F2} ms  max={stats.Timings.Max():F2} ms  alloc/run={stats.AllocatedPerRunMb:F1} MB"
        );
    }

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }

    private sealed record RunStats(List<double> Timings, double AllocatedPerRunMb);
}
