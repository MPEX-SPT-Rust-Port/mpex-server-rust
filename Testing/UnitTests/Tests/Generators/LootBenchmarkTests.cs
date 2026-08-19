using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Head-to-head wall clock of the two shipping loot paths on the same live database in one process:
/// the spt-native default against the retained 4.1.2 C# implementation
/// (<see cref="LocationConfig.ForceLegacyLootGeneration"/>). Run in Release - the cargo dev profile
/// makes Debug numbers meaningless.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
public class LootBenchmarkTests
{
    private const string LocationId = "bigmap";
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private LocationLootGenerator _locationLootGenerator = default!;
    private LocationConfig _locationConfig = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _locationLootGenerator = di.GetService<LocationLootGenerator>();
        _locationConfig = di.GetService<LocationConfig>();
        _ = di.GetService<LocationTable>();
    }

    [Test]
    public void NativeVersusLegacyCSharp()
    {
        var native = Measure(forceLegacy: false, LootGenerationPath.Native);
        var legacy = Measure(forceLegacy: true, LootGenerationPath.Legacy);

        Report("native (rust)", native);
        Report("legacy (C# 4.1.2)", legacy);
        TestContext.Out.WriteLine($"speedup (median legacy / median native): {Median(legacy.Timings) / Median(native.Timings):F2}x");
        TestContext.Out.WriteLine(
            $"managed allocation per run: native {native.AllocatedPerRunMb:F1} MB, legacy {legacy.AllocatedPerRunMb:F1} MB "
                + $"({legacy.AllocatedPerRunMb / native.AllocatedPerRunMb:F2}x)"
        );

        ReportBinarySizes();
    }

    /// <summary>
    /// Peak process memory for one path, alone in its own process. Resident pages the other path left
    /// behind cannot be told apart from this one's inside a single run - the managed heap never hands
    /// them back - so these two tests are meant to be run one per <c>dotnet test</c> invocation and
    /// compared against each other, not against the in-process figures above.
    /// </summary>
    [Test]
    public void NativePeakWorkingSet()
    {
        ReportPeakWorkingSet("native (rust)", Measure(forceLegacy: false, LootGenerationPath.Native));
    }

    [Test]
    public void LegacyPeakWorkingSet()
    {
        ReportPeakWorkingSet("legacy (C# 4.1.2)", Measure(forceLegacy: true, LootGenerationPath.Legacy));
    }

    private static void ReportPeakWorkingSet(string label, RunStats stats)
    {
        using var process = Process.GetCurrentProcess();

        // What is still resident once the transients are collectable, versus the high-water mark
        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        TestContext.Out.WriteLine(
            $"{label, -20} process peak RSS={process.PeakWorkingSet64 / 1024.0 / 1024.0:F0} MB  "
                + $"settled RSS={Environment.WorkingSet / 1024.0 / 1024.0:F0} MB  "
                + $"managed heap={GC.GetTotalMemory(forceFullCollection: false) / 1024.0 / 1024.0:F0} MB  "
                + $"alloc/run={stats.AllocatedPerRunMb:F1} MB"
        );
    }

    /// <summary>
    /// What the port costs and replaces on disk. The legacy C# path still ships inside
    /// <c>SPTarkov.Server.Core.dll</c> - it is the frozen mod contract - so the native library is
    /// additive today rather than a saving.
    /// </summary>
    private static void ReportBinarySizes()
    {
        string[] binaries = [OperatingSystem.IsWindows() ? "spt_native.dll" : "libspt_native.so", "SPTarkov.Server.Core.dll"];

        foreach (var binary in binaries)
        {
            var file = new FileInfo(Path.Combine(AppContext.BaseDirectory, binary));
            TestContext.Out.WriteLine(
                file.Exists
                    ? $"{binary, -26} {file.Length / 1024.0 / 1024.0:F2} MB"
                    : $"{binary, -26} not present in {AppContext.BaseDirectory}"
            );
        }
    }

    private RunStats Measure(bool forceLegacy, LootGenerationPath expected)
    {
        var original = _locationConfig.ForceLegacyLootGeneration;
        _locationConfig.ForceLegacyLootGeneration = forceLegacy;
        var timings = new List<double>(TimedRuns);

        try
        {
            // JIT, the native library load and the first lazy-load deserialise are not measured
            for (var run = 0; run < WarmupRuns; run++)
            {
                _locationLootGenerator.GenerateLocationLoot(LocationId);
            }

            // A benchmark that silently timed the same path twice would look like a 1.00x result
            Assert.That(_locationLootGenerator.LastPathTaken, Is.EqualTo(expected), "generation did not take the path being measured");

            // Settle the heap once before the phase - collecting between timed runs would distort
            // the wall clock this fixture exists to measure
            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            var allocatedBefore = GC.GetTotalAllocatedBytes(precise: true);
            var collectionsBefore = (GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2));
            var startWorkingSet = Environment.WorkingSet;
            var peakWorkingSet = startWorkingSet;

            for (var run = 0; run < TimedRuns; run++)
            {
                var stopwatch = Stopwatch.StartNew();
                _locationLootGenerator.GenerateLocationLoot(LocationId);
                stopwatch.Stop();

                timings.Add(stopwatch.Elapsed.TotalMilliseconds);

                // Read outside the timed region. Sampling once per run only catches a peak that
                // survives to the end of a run, which is what a raid start actually holds
                peakWorkingSet = Math.Max(peakWorkingSet, Environment.WorkingSet);
            }

            return new RunStats(
                timings,
                (GC.GetTotalAllocatedBytes(precise: true) - allocatedBefore) / (double)TimedRuns / 1024 / 1024,
                peakWorkingSet / 1024.0 / 1024.0,
                (peakWorkingSet - startWorkingSet) / 1024.0 / 1024.0,
                (
                    GC.CollectionCount(0) - collectionsBefore.Item1,
                    GC.CollectionCount(1) - collectionsBefore.Item2,
                    GC.CollectionCount(2) - collectionsBefore.Item3
                )
            );
        }
        finally
        {
            _locationConfig.ForceLegacyLootGeneration = original;
        }
    }

    private static void Report(string label, RunStats stats)
    {
        var (gen0, gen1, gen2) = stats.Collections;
        TestContext.Out.WriteLine(
            $"{label, -20} n={stats.Timings.Count}  mean={stats.Timings.Average():F2} ms  median={Median(stats.Timings):F2} ms  "
                + $"min={stats.Timings.Min():F2} ms  max={stats.Timings.Max():F2} ms"
        );
        TestContext.Out.WriteLine(
            $"{"", -20} alloc/run={stats.AllocatedPerRunMb:F1} MB  peak RSS={stats.PeakWorkingSetMb:F0} MB "
                + $"(+{stats.WorkingSetGrowthMb:F0} MB over the phase)  GC gen0/1/2={gen0}/{gen1}/{gen2}"
        );
    }

    /// <param name="AllocatedPerRunMb">
    ///     Managed allocation only - what the native side allocates on the Rust heap never reaches the
    ///     GC, so peak RSS is the only figure that sees both.
    /// </param>
    /// <param name="PeakWorkingSetMb">
    ///     Whole-process resident set, so it carries everything the earlier phase left resident (the
    ///     managed heap does not hand pages back). <paramref name="WorkingSetGrowthMb"/> is the figure
    ///     to compare across phases; the absolute peak is only meaningful for the later one.
    /// </param>
    private record RunStats(
        List<double> Timings,
        double AllocatedPerRunMb,
        double PeakWorkingSetMb,
        double WorkingSetGrowthMb,
        (int Gen0, int Gen1, int Gen2) Collections
    );

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }
}
