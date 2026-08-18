using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Services.Ragfair;

namespace UnitTests.Tests.Services;

/// <summary>
/// Head-to-head wall clock of the two ragfair linked item table builds on the same live database in
/// one process. One build is the whole shipped items table walked once, and what the native path
/// adds over that walk is a round trip: every template projected to its parent and its slot, chamber
/// and cartridge filter ids on the way out, and the whole id-to-id-set table on the way back.
/// <c>RagfairLinkedItemNativeRequestBuilder.Build</c> is timed on its own as a final phase - the C#
/// half of the outbound leg, as the numbers in <c>BENCHMARK.md</c> record.
///
/// The build is lazy: <c>BuildLinkedItemTable</c> is protected, and <c>GetLinkedItems</c> triggers it
/// on the first cache miss. That is the timed call, so each measurement includes the miss and the
/// indexer read that follows the build - both negligible against a full-table walk.
///
/// Quirk 1 makes the build single-shot per instance (the final copy loop uses
/// <c>Dictionary.Add</c>), so every run builds its own service outside the stopwatch and asks it for
/// exactly one tpl. Same construction the parity fixture uses: the frozen 4.1.2 constructor has no
/// native seam and builds legacy unconditionally, so neither arm touches the shared
/// <see cref="RagfairConfig"/>.
///
/// Run in Release; the cargo dev profile makes Debug numbers meaningless.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
[NonParallelizable]
public class RagfairLinkedItemBenchmarkTests
{
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private TemplateTable _templateTable = default!;
    private ItemHelper _itemHelper = default!;
    private ISptLogger<RagfairLinkedItemService> _logger = default!;
    private RagfairLinkedItemNativeRequestBuilder _requestBuilder = default!;
    private RagfairConfig _ragfairConfig = default!;

    /// <summary>
    /// Every template in the items table keys an entry on both paths, so any tpl triggers the build
    /// and survives the indexer read afterwards.
    /// </summary>
    private MongoId _anyTableTpl;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _templateTable = di.GetService<TemplateTable>();
        _itemHelper = di.GetService<ItemHelper>();
        _logger = di.GetService<ISptLogger<RagfairLinkedItemService>>();
        _requestBuilder = di.GetService<RagfairLinkedItemNativeRequestBuilder>();
        _ragfairConfig = di.GetService<RagfairConfig>();

        _anyTableTpl = _templateTable.Items.Keys.First();
    }

    /// <summary>
    /// The one workload this service has: the bulk build over every template the database ships.
    /// </summary>
    [Test]
    public void BuildNativeVersusLegacyCSharp()
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
    /// What crosses the boundary, in entries - the request is four fields per template with the
    /// filter ids flattened under three of them, the response is a set of linked tpls per key.
    /// Reported after every timed phase so it perturbs none of them.
    /// </summary>
    private void ReportPayloadShape()
    {
        var request = _requestBuilder.Build();
        var requestFilterIds = request.ItemsView.Values.Sum(view =>
            FilterIdCount(view.Slots) + FilterIdCount(view.Chambers) + FilterIdCount(view.Cartridges)
        );

        var service = MakeService(native: true);
        service.GetLinkedItems(_anyTableTpl);

        TestContext.Out.WriteLine(
            $"request: {request.ItemsView.Count} templates x 4 fields carrying {requestFilterIds} filter ids  |  "
                + $"response: {service.CacheForTests.Count} tpls carrying "
                + $"{service.CacheForTests.Values.Sum(set => set.Count)} linked ids"
        );
    }

    private static int FilterIdCount(List<RagfairLinkedSlotView>? slots)
    {
        return (slots ?? []).Sum(slot => slot.Filter?.Count ?? 0);
    }

    /// <param name="native">
    ///     Selects the arm through the constructor: the additive one wires the native seam, the
    ///     frozen 4.1.2 one has none and builds legacy unconditionally. Same selection the parity
    ///     fixture uses, and it leaves the shared <c>RagfairConfig</c> untouched.
    /// </param>
    private List<double> Measure(bool native)
    {
        var timings = new List<double>(TimedRuns);
        var warmed = default(RagfairLinkedItemService);

        // JIT and the native library load are not measured
        for (var run = 0; run < WarmupRuns; run++)
        {
            warmed = MakeService(native);
            warmed.GetLinkedItems(_anyTableTpl);
        }

        // A benchmark that silently timed the same path twice would look like a 1.00x result
        var expected = native ? LootGenerationPath.Native : LootGenerationPath.Legacy;
        Assert.That(warmed!.LastPathTaken, Is.EqualTo(expected), "the build did not take the path being measured");

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        for (var run = 0; run < TimedRuns; run++)
        {
            // Quirk 1: a warm instance cannot be rebuilt, so every run gets its own
            var service = MakeService(native);

            var stopwatch = Stopwatch.StartNew();
            service.GetLinkedItems(_anyTableTpl);
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
        }

        return timings;
    }

    /// <summary>
    /// The C# half of what a native call pays before any walking happens: every template in the
    /// table projected to its parent and its flattened slot, chamber and cartridge filter ids. The
    /// serialise and the native-side parse are on top of this, as is the response table on the way
    /// back.
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

    private RagfairLinkedItemService MakeService(bool native)
    {
        if (native)
        {
            return new RagfairLinkedItemService(_templateTable, _itemHelper, _logger, _requestBuilder, _ragfairConfig);
        }

        return new RagfairLinkedItemService(_templateTable, _itemHelper, _logger);
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
