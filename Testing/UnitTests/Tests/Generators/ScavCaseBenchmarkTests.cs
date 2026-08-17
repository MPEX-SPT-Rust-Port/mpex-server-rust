using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.ScavCase;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Head-to-head wall clock of the two scav case reward paths on the same live database in one
/// process, for every recipe the shipped database ships. One case is a few rewards' worth of C#
/// work, so the native path's cost is dominated by the request it has to hand across the boundary -
/// the whole items view and a static price for every tpl in it, projected and serialised per call
/// with a handful of output items to amortise it against. <c>ScavCaseNativeRequestBuilder.Build</c>
/// is timed on its own as a final phase: the C# half of that transport.
///
/// The first recipe measured reads ~2x high on both arms - two warmups do not settle the first phase
/// in the process, and reversing the recipe order moves the inflation with it. Read that row against
/// the settled recipes; the scav case section of <c>BENCHMARK.md</c> records the reversed-order run.
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
    private List<MongoId> _recipeIds = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _scavCaseConfig = di.GetService<ScavCaseConfig>();
        _requestBuilder = di.GetService<ScavCaseNativeRequestBuilder>();
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
            var native = Measure(recipeId, forceLegacy: false);
            var legacy = Measure(recipeId, forceLegacy: true);

            Report($"{recipeId} native (rust)", native);
            Report($"{recipeId} legacy (C# 4.1.2)", legacy);
            TestContext.Out.WriteLine($"{recipeId} speedup (median legacy / median native): {Median(legacy) / Median(native):F2}x");
        }

        Report("Build (request only)", MeasureRequestBuild());
    }

    /// <param name="forceLegacy">
    ///     Selects the arm through <c>ScavCaseConfig.ForceLegacyScavCaseGeneration</c>, restored in
    ///     the <c>finally</c>.
    /// </param>
    private List<double> Measure(MongoId recipeId, bool forceLegacy)
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
                _ = generator.Generate(recipeId).ToList();
            }

            // A benchmark that silently timed the same path twice would look like a 1.00x result
            var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
            Assert.That(generator.LastPathTaken, Is.EqualTo(expected), "generation did not take the path being measured");

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            for (var run = 0; run < TimedRuns; run++)
            {
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
    /// The C# half of what a native call pays before any generation happens: the items view, a
    /// static price per tpl in it, every default preset, the blacklists and the recipe table. The
    /// serialise and the native-side parse are on top of this. Timed on the first recipe only - the
    /// payload is identical for every recipe bar its <c>RecipeId</c>.
    /// </summary>
    private List<double> MeasureRequestBuild()
    {
        var timings = new List<double>(TimedRuns);

        for (var run = 0; run < WarmupRuns; run++)
        {
            _ = _requestBuilder.Build(_recipeIds[0], null);
        }

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        for (var run = 0; run < TimedRuns; run++)
        {
            var stopwatch = Stopwatch.StartNew();
            _ = _requestBuilder.Build(_recipeIds[0], null);
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
