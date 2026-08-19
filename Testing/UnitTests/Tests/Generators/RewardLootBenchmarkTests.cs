using System.Diagnostics;
using System.Reflection;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Services;
using SPTarkov.Server.Core.Services.InRaid;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Head-to-head wall clock of the two reward loot paths on the same live database in one process,
/// on the airdrop request the server actually generates. The native arm here is resident-DB
/// eligible, so each call is an epoch send against the published database - the C#-built views
/// override rides along only on ineligible sends - and the ratio this prints is that resident
/// path against the legacy C#. Run in Release; the cargo dev profile makes Debug numbers
/// meaningless.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
[NonParallelizable]
public class RewardLootBenchmarkTests
{
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private LootGenerator _lootGenerator = default!;
    private LocationConfig _locationConfig = default!;
    private LootRequest _airdropRequest = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _lootGenerator = di.GetService<LootGenerator>();
        _locationConfig = di.GetService<LocationConfig>();

        var airdropService = di.GetService<AirdropService>();
        var method = typeof(AirdropService).GetMethod("GetAirdropLootConfigByType", BindingFlags.Instance | BindingFlags.NonPublic);
        _airdropRequest = (LootRequest)method!.Invoke(airdropService, [SptAirdropTypeEnum.mixed])!;
    }

    [Test]
    public void CreateRandomLootNativeVersusLegacyCSharp()
    {
        var native = Measure(forceLegacy: false, LootGenerationPath.Native);
        var legacy = Measure(forceLegacy: true, LootGenerationPath.Legacy);

        Report("native (rust)", native);
        Report("legacy (C# 4.1.2)", legacy);
        TestContext.Out.WriteLine($"speedup (median legacy / median native): {Median(legacy) / Median(native):F2}x");
    }

    private List<double> Measure(bool forceLegacy, LootGenerationPath expected)
    {
        var original = _locationConfig.ForceLegacyLootGeneration;
        _locationConfig.ForceLegacyLootGeneration = forceLegacy;
        var timings = new List<double>(TimedRuns);

        try
        {
            // JIT, the native library load and the first lazy-load deserialise are not measured
            for (var run = 0; run < WarmupRuns; run++)
            {
                _ = _lootGenerator.CreateRandomLoot(_airdropRequest).ToList();
            }

            // A benchmark that silently timed the same path twice would look like a 1.00x result
            Assert.That(_lootGenerator.LastPathTaken, Is.EqualTo(expected), "generation did not take the path being measured");

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            for (var run = 0; run < TimedRuns; run++)
            {
                var stopwatch = Stopwatch.StartNew();
                _ = _lootGenerator.CreateRandomLoot(_airdropRequest).ToList();
                stopwatch.Stop();

                timings.Add(stopwatch.Elapsed.TotalMilliseconds);
            }

            return timings;
        }
        finally
        {
            _locationConfig.ForceLegacyLootGeneration = original;
        }
    }

    private static void Report(string label, List<double> timings)
    {
        TestContext.Out.WriteLine(
            $"{label, -20} n={timings.Count}  mean={timings.Average():F2} ms  median={Median(timings):F2} ms  "
                + $"min={timings.Min():F2} ms  max={timings.Max():F2} ms"
        );
    }

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }
}
