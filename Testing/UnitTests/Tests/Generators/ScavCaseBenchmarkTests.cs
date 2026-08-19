using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.ScavCase;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Head-to-head wall clock of the two scav case reward paths on the same live database in one
/// process, for every recipe the shipped database ships. One case is a few rewards' worth of C#
/// work, so the native path's cost is dominated by keeping the resident DB current - which is why
/// the native side is measured twice: with <c>DatabaseMutationStamp</c> bumped before every run so
/// each pass republishes the whole database first (publish cold), and with the stamp left alone so
/// the send carries the varying block only and generates off the resident views (publish warm, the
/// shape a stock server runs). Two projections are timed on their own as a final phase:
/// <c>ScavCaseNativeRequestBuilder.Build</c>, the whole epoch-0 override request an ineligible
/// (modded, untrusted) send pays per call, and <c>BuildViewsOverride</c>, its database-views half.
///
/// The first two recipes measured read high on both arms - two warmups do not settle the first
/// phases in the process, and reversing the recipe order moves the inflation with it. Read those
/// rows against the settled recipes; the scav case section of <c>BENCHMARK.md</c> records the
/// reversed-order run.
///
/// Run in Release; the cargo dev profile makes Debug numbers meaningless.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
[NonParallelizable]
public class ScavCaseBenchmarkTests
{
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private ScavCaseConfig _scavCaseConfig = default!;
    private ScavCaseNativeRequestBuilder _requestBuilder = default!;
    private DatabaseMutationStamp _databaseMutationStamp = default!;
    private DbPublisher _dbPublisher = default!;
    private List<MongoId> _recipeIds = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _scavCaseConfig = di.GetService<ScavCaseConfig>();
        _requestBuilder = di.GetService<ScavCaseNativeRequestBuilder>();
        _databaseMutationStamp = di.GetService<DatabaseMutationStamp>();
        _dbPublisher = di.GetService<DbPublisher>();
        _recipeIds = di.GetService<HideoutTable>().Production.ScavRecipes!.Select(recipe => recipe.Id).ToList();
    }

    [TearDown]
    public void TearDown()
    {
        _scavCaseConfig.ForceLegacyScavCaseGeneration = false;
    }

    /// <summary>
    /// Every shipped recipe: their end product counts differ per rarity, so between them they cover
    /// an empty rarity, a fixed count and a ranged count - the whole spread of output sizes the
    /// per-call payload has to be amortised against.
    /// </summary>
    [Test]
    public void GenerateNativeVersusLegacyCSharp()
    {
        Assert.That(_recipeIds, Is.Not.Empty, "the shipped database has no scav case recipes");

        foreach (var recipeId in _recipeIds)
        {
            var warm = Measure(recipeId, forceLegacy: false);
            var legacy = Measure(recipeId, forceLegacy: true);

            var epochBeforeCold = _dbPublisher.EnsureCurrent();
            var cold = Measure(recipeId, forceLegacy: false, () => _databaseMutationStamp.Bump());
            Assert.That(
                _dbPublisher.EnsureCurrent(),
                Is.GreaterThan(epochBeforeCold),
                "the cold arm never advanced the resident-DB epoch - it measured the warm path"
            );

            Report($"{recipeId} native, publish warm", warm);
            Report($"{recipeId} legacy (C# 4.1.2)", legacy);
            Report($"{recipeId} native, publish cold", cold);
            TestContext.Out.WriteLine(
                $"{recipeId} speedup (median legacy / median native warm): {Median(legacy) / Median(warm):F2}x  "
                    + $"publish cost per send (median cold - median warm): {Median(cold) - Median(warm):F2} ms"
            );
        }

        Report("Build (request only)", MeasureProjection(() => _requestBuilder.Build(_recipeIds[0], null)));
        Report("BuildViewsOverride only", MeasureProjection(() => _requestBuilder.BuildViewsOverride()));
    }

    /// <param name="forceLegacy">
    ///     Selects the arm through <c>ScavCaseConfig.ForceLegacyScavCaseGeneration</c>, restored in
    ///     the <c>finally</c>.
    /// </param>
    /// <param name="beforeEachRun">
    ///     Runs outside the timed region, before every warmup and timed run. The cold arm bumps
    ///     <c>DatabaseMutationStamp</c> here, so every pass republishes the whole database first;
    ///     the warm arm leaves the stamp alone and generates off the already-derived resident views.
    /// </param>
    private List<double> Measure(MongoId recipeId, bool forceLegacy, Action? beforeEachRun = null)
    {
        // The legacy path caches its two item pools on the instance, so each arm is measured off a
        // generator of its own rather than one another fixture may have left warm; the warmups below
        // build those caches, which is the state a long-lived generator serves a craft from
        var generator = BuildGenerator();
        var original = _scavCaseConfig.ForceLegacyScavCaseGeneration;
        var timings = new List<double>(TimedRuns);

        _scavCaseConfig.ForceLegacyScavCaseGeneration = forceLegacy;

        try
        {
            // JIT, the native library load and the legacy path's pool caches are not measured
            for (var run = 0; run < WarmupRuns; run++)
            {
                beforeEachRun?.Invoke();
                _ = generator.Generate(recipeId).ToList();
            }

            // A benchmark that silently timed the same path twice would look like a 1.00x result
            var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
            Assert.That(generator.LastPathTaken, Is.EqualTo(expected), "generation did not take the path being measured");

            if (!forceLegacy)
            {
                Assert.That(generator.LastSendIncludedViewsOverride, Is.False, "both native arms must ride the resident path");
            }

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            for (var run = 0; run < TimedRuns; run++)
            {
                beforeEachRun?.Invoke();

                var stopwatch = Stopwatch.StartNew();
                _ = generator.Generate(recipeId).ToList();
                stopwatch.Stop();

                timings.Add(stopwatch.Elapsed.TotalMilliseconds);
            }

            return timings;
        }
        finally
        {
            _scavCaseConfig.ForceLegacyScavCaseGeneration = original;
        }
    }

    /// <summary>
    /// The C# half of what an ineligible native call pays before any generation happens: the items
    /// view, a static price per tpl in it, every default preset and the recipe table
    /// (<c>BuildViewsOverride</c>), plus the varying block on top when the whole epoch-0 request is
    /// composed (<c>Build</c>, timed on the first recipe only - the payload is identical for every
    /// recipe bar its <c>RecipeId</c>). The serialise and the native-side parse are on top of this.
    /// </summary>
    private List<double> MeasureProjection(Action projection)
    {
        var timings = new List<double>(TimedRuns);

        for (var run = 0; run < WarmupRuns; run++)
        {
            projection();
        }

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        for (var run = 0; run < TimedRuns; run++)
        {
            var stopwatch = Stopwatch.StartNew();
            projection();
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
        }

        return timings;
    }

    /// <summary>
    /// A generator built off the container's own services on the widest constructor - the one the
    /// container itself picks, so the native seam is wired. Unseeded, the way production runs.
    /// </summary>
    private static ScavCaseRewardGenerator BuildGenerator()
    {
        var di = DI.GetInstance();
        var constructor = typeof(ScavCaseRewardGenerator).GetConstructors().MaxBy(candidate => candidate.GetParameters().Length)!;
        var arguments = constructor.GetParameters().Select(parameter => di.GetService(parameter.ParameterType)).ToArray();

        return (ScavCaseRewardGenerator)constructor.Invoke(arguments);
    }

    private static void Report(string label, List<double> timings)
    {
        TestContext.Out.WriteLine(
            $"{label, -44} n={timings.Count}  mean={timings.Average():F2} ms  median={Median(timings):F2} ms  "
                + $"min={timings.Min():F2} ms  max={timings.Max():F2} ms"
        );
    }

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }
}
