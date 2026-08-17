using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.BaseClass;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;

namespace UnitTests.Tests.Services;

/// <summary>
/// Head-to-head wall clock of the two item base class cache builds on the same live database in one
/// process. One build is the whole shipped items table walked once, and what the native path adds
/// over that walk is a round trip: every template projected to its parent and type on the way out,
/// and one entry per cached tpl carrying that tpl's whole ancestor set on the way back.
/// <c>ItemBaseClassNativeRequestBuilder.Build</c> is timed on its own as a final phase - the C# half
/// of the outbound leg, and the cheap half, as the numbers in <c>BENCHMARK.md</c> record.
///
/// Every run hydrates a service instance built for that run, outside the stopwatch. That is the
/// production shape - <c>PostDbLoadService</c> calls hydrate once on a fresh singleton - and it
/// keeps the legacy arm honest, since its dictionary is rebuilt from nothing every time.
///
/// Run in Release; the cargo dev profile makes Debug numbers meaningless.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
[NonParallelizable]
public class ItemBaseClassBenchmarkTests
{
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private ISptLogger<ItemBaseClassService> _logger = default!;
    private TemplateTable _templateTable = default!;
    private ServerLocalisationService _serverLocalisationService = default!;
    private ItemBaseClassNativeRequestBuilder _requestBuilder = default!;
    private ItemConfig _itemConfig = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _logger = di.GetService<ISptLogger<ItemBaseClassService>>();
        _templateTable = di.GetService<TemplateTable>();
        _serverLocalisationService = di.GetService<ServerLocalisationService>();
        _requestBuilder = di.GetService<ItemBaseClassNativeRequestBuilder>();
        _itemConfig = di.GetService<ItemConfig>();
    }

    /// <summary>
    /// The one workload this service has: the bulk build over every template the database ships.
    /// </summary>
    [Test]
    public void HydrateNativeVersusLegacyCSharp()
    {
        Assert.That(_templateTable.Items, Is.Not.Empty, "the shipped database has no item templates");

        TestContext.Out.WriteLine($"item templates in the shipped table: {_templateTable.Items.Count}");

        var native = Measure(native: true);
        var legacy = Measure(native: false);

        Report("native (rust)", native);
        Report("legacy (C# 4.1.2)", legacy);
        TestContext.Out.WriteLine($"speedup (median legacy / median native): {Median(legacy) / Median(native):F2}x");

        Report("Build (request only)", MeasureRequestBuild());
        ReportPayloadShape();
    }

    /// <summary>
    /// What crosses the boundary, in entries - the request is two fields per template, the response
    /// is a set of ancestors per cached tpl. Reported after every timed phase so it perturbs none of
    /// them.
    /// </summary>
    private void ReportPayloadShape()
    {
        var service = MakeService(native: true);
        service.HydrateItemBaseClassCache();

        TestContext.Out.WriteLine(
            $"request: {_templateTable.Items.Count} templates x 2 fields  |  "
                + $"response: {service.CacheForTests.Count} tpls carrying {service.CacheForTests.Values.Sum(set => set.Count)} "
                + $"ancestor ids, plus {service.RootNodeIdsForTests.Count} root node ids"
        );
    }

    /// <param name="native">
    ///     Selects the arm through the constructor: the additive one wires the native seam, the
    ///     frozen 4.1.2 one has none and hydrates legacy unconditionally. Same selection the parity
    ///     fixture uses, and it leaves the shared <c>ItemConfig</c> untouched.
    /// </param>
    private List<double> Measure(bool native)
    {
        var timings = new List<double>(TimedRuns);
        var warmed = default(ItemBaseClassService);

        // JIT and the native library load are not measured
        for (var run = 0; run < WarmupRuns; run++)
        {
            warmed = MakeService(native);
            warmed.HydrateItemBaseClassCache();
        }

        // A benchmark that silently timed the same path twice would look like a 1.00x result
        var expected = native ? LootGenerationPath.Native : LootGenerationPath.Legacy;
        Assert.That(warmed!.LastPathTaken, Is.EqualTo(expected), "hydration did not take the path being measured");

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        for (var run = 0; run < TimedRuns; run++)
        {
            var service = MakeService(native);

            var stopwatch = Stopwatch.StartNew();
            service.HydrateItemBaseClassCache();
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
        }

        return timings;
    }

    /// <summary>
    /// The C# half of what a native call pays before any walking happens: every template in the
    /// table projected to its parent and its type. The serialise and the native-side parse are on
    /// top of this, as is the response map on the way back.
    /// </summary>
    private List<double> MeasureRequestBuild()
    {
        var timings = new List<double>(TimedRuns);

        for (var run = 0; run < WarmupRuns; run++)
        {
            _ = _requestBuilder.Build();
        }

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        for (var run = 0; run < TimedRuns; run++)
        {
            var stopwatch = Stopwatch.StartNew();
            _ = _requestBuilder.Build();
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
        }

        return timings;
    }

    private ItemBaseClassService MakeService(bool native)
    {
        if (native)
        {
            return new ItemBaseClassService(_logger, _templateTable, _serverLocalisationService, _requestBuilder, _itemConfig);
        }

        return new ItemBaseClassService(_logger, _templateTable, _serverLocalisationService);
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
