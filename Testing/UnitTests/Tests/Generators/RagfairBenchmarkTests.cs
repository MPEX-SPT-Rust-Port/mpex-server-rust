using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Ragfair;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Traders;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Head-to-head wall clock of the two dynamic flea offer generation paths on the same live database
/// in one process, on the two calls the server actually makes: the full pass at startup and the
/// regeneration pass <c>RagfairServer.ProcessExpiredFleaOffers</c> fires once enough offers expire.
/// A pass is tens of thousands of offers with full item trees.
/// <see cref="RagfairPayloadProjection.BuildRequest"/> is timed on its own as well, views override
/// included - since the resident-DB flip that build is the *ineligible* caller's per-call cost
/// (mods without <c>TrustNativeRequestCacheWithMods</c>, or <c>DisableNativeRequestCache</c>), not
/// part of the eligible pass. Run in Release; the cargo dev profile makes Debug
/// numbers meaningless. The "already-populated flea" premise the numbers are read against depends on
/// this fixture running before the fixtures that clear the shared holder - alphabetical order
/// currently guarantees it.
/// </summary>
[TestFixture]
[Explicit("benchmark, run on demand in Release")]
[NonParallelizable]
public class RagfairBenchmarkTests
{
    // A full pass is tens of thousands of offers, so the run count is a fraction of the other
    // fixtures' 20 - the phase is minutes, not seconds
    private const int WarmupRuns = 1;
    private const int TimedRuns = 5;

    private RagfairOfferGenerator _offerGenerator = default!;
    private RagfairOfferService _offerService = default!;
    private RagfairConfig _ragfairConfig = default!;

    private TemplateTable _templateTable = default!;
    private HandbookHelper _handbookHelper = default!;
    private TraderHelper _traderHelper = default!;
    private PresetHelper _presetHelper = default!;
    private ItemFilterService _itemFilterService = default!;
    private SeasonalEventService _seasonalEventService = default!;
    private BotTable _botTable = default!;
    private ItemHelper _itemHelper = default!;
    private BotConfig _botConfig = default!;
    private TimeUtil _timeUtil = default!;
    private DatabaseMutationStamp _databaseMutationStamp = default!;
    private DbPublisher _dbPublisher = default!;

    /// <summary>
    /// The regeneration workload: one cloned single-item list per expired offer, at the configured
    /// <c>expiredOfferThreshold</c> - the shape <c>RagfairServer.cs:69</c> hands over. Neither path
    /// mutates it: the only in-place edit legacy makes to the list it is handed is
    /// <c>RemoveBannedPlatesFromPreset</c>, and its <c>!isExpiredOffer</c> guard means an expired
    /// pass never reaches it. So the list is built once and reused across runs.
    /// </summary>
    private List<List<Item>> _expiredOffers = default!;

    /// <summary> Whatever was in the holder before the fixture ran, so the purge leaves it alone </summary>
    private HashSet<MongoId> _preExistingOfferIds = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions the native wrapper serialises the payload with
        di.GetService<JsonUtil>();

        _offerGenerator = di.GetService<RagfairOfferGenerator>();
        _offerService = di.GetService<RagfairOfferService>();
        _ragfairConfig = di.GetService<RagfairConfig>();

        _templateTable = di.GetService<TemplateTable>();
        _handbookHelper = di.GetService<HandbookHelper>();
        _traderHelper = di.GetService<TraderHelper>();
        _presetHelper = di.GetService<PresetHelper>();
        _itemFilterService = di.GetService<ItemFilterService>();
        _seasonalEventService = di.GetService<SeasonalEventService>();
        _botTable = di.GetService<BotTable>();
        _itemHelper = di.GetService<ItemHelper>();
        _botConfig = di.GetService<BotConfig>();
        _timeUtil = di.GetService<TimeUtil>();
        _databaseMutationStamp = di.GetService<DatabaseMutationStamp>();
        _dbPublisher = di.GetService<DbPublisher>();

        _preExistingOfferIds = _offerService.GetOffers().Select(offer => offer.Id).ToHashSet();

        var cloner = di.GetService<ICloner>();
        _expiredOffers = di.GetService<RagfairAssortGenerator>()
            .GenerateRagfairAssortItems()
            // Expired offers are single items; the assort's multi-item entries are presets
            .Where(assortItemWithChildren => assortItemWithChildren.Count == 1)
            .Take(_ragfairConfig.Dynamic.ExpiredOfferThreshold)
            .Select(assortItemWithChildren => cloner.Clone(assortItemWithChildren)!)
            .ToList();
    }

    [Test]
    public void GenerateDynamicOffersNativeVersusLegacyCSharp()
    {
        TestContext.Out.WriteLine($"expired offers handed to the regeneration pass: {_expiredOffers.Count}");

        RunScenario("full pass", null);
        RunScenario("regeneration pass", _expiredOffers);
        MeasureForcedPublish();
        RunResidentDbScenario();
    }

    /// <summary>
    /// The republish cost in isolation - the four resident roots (templates, traders, globals,
    /// locations) projected, copied across the FFI, parsed and view-derived by <see cref="DbPublisher.ForcePublish"/>,
    /// with no generation pass attached. This is the whole per-mutation cost the epoch protocol
    /// pays; the warm resident arm below is what every pass pays once it is paid.
    /// </summary>
    private void MeasureForcedPublish()
    {
        for (var run = 0; run < WarmupRuns; run++)
        {
            _ = _dbPublisher.ForcePublish();
        }

        var timings = new List<double>(TimedRuns);
        for (var run = 0; run < TimedRuns; run++)
        {
            var stopwatch = Stopwatch.StartNew();
            _ = _dbPublisher.ForcePublish();
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
        }

        TestContext.Out.WriteLine(
            $"{"publish (4 roots, forced)", -36} n={timings.Count}  mean={timings.Average():F2} ms  median={Median(timings):F2} ms  "
                + $"min={timings.Min():F2} ms  max={timings.Max():F2} ms"
        );
    }

    /// <summary>
    /// What the stamp-gated resident DB buys on the regeneration pass. Cold bumps
    /// <see cref="DatabaseMutationStamp"/> before every run, so every pass republishes the whole
    /// database first; warm leaves the stamp alone, so the send carries the varying fields only
    /// and the native side generates off the views it already derived.
    /// </summary>
    private void RunResidentDbScenario()
    {
        var cold = Measure(forceLegacy: false, LootGenerationPath.Native, _expiredOffers, () => _databaseMutationStamp.Bump());
        Assert.That(_offerGenerator.LastSendIncludedViewsOverride, Is.False, "the cold arm still rides the resident path");

        var warm = Measure(forceLegacy: false, LootGenerationPath.Native, _expiredOffers);
        Assert.That(_offerGenerator.LastSendIncludedViewsOverride, Is.False, "the warm arm must generate off the resident views");

        Report("regeneration pass publish cold", cold);
        Report("regeneration pass publish warm", warm);
        TestContext.Out.WriteLine(
            $"{"resident db", -20} speedup (median cold / median warm): {Median(cold.Timings) / Median(warm.Timings):F2}x  "
                + $"warm share of cold median: {Median(warm.Timings) / Median(cold.Timings) * 100:F1}%"
        );
    }

    private void RunScenario(string scenario, List<List<Item>>? expiredOffers)
    {
        var native = Measure(forceLegacy: false, LootGenerationPath.Native, expiredOffers);
        var legacy = Measure(forceLegacy: true, LootGenerationPath.Legacy, expiredOffers);
        var projection = MeasureProjection(expiredOffers);

        Report($"{scenario} native (rust)", native);
        Report($"{scenario} legacy (C# 4.1.2)", legacy);
        Report($"{scenario} BuildRequest only", projection);
        TestContext.Out.WriteLine(
            $"{scenario, -20} speedup (median legacy / median native): {Median(legacy.Timings) / Median(native.Timings):F2}x  "
                + $"projection share of native median: {Median(projection.Timings) / Median(native.Timings) * 100:F1}%"
        );
    }

    /// <param name="beforeEachRun"> Runs outside the timed region, before every warmup and timed run </param>
    private RunStats Measure(bool forceLegacy, LootGenerationPath expected, List<List<Item>>? expiredOffers, Action? beforeEachRun = null)
    {
        var original = _ragfairConfig.ForceLegacyRagfairGeneration;
        _ragfairConfig.ForceLegacyRagfairGeneration = forceLegacy;
        var timings = new List<double>(TimedRuns);
        var offerCount = 0;

        try
        {
            // JIT, the native library load and the first lazy-load deserialise are not measured
            for (var run = 0; run < WarmupRuns; run++)
            {
                beforeEachRun?.Invoke();
                GenerateAndPurge(expiredOffers);
            }

            // A benchmark that silently timed the same path twice would look like a 1.00x result
            Assert.That(_offerGenerator.LastPathTaken, Is.EqualTo(expected), "generation did not take the path being measured");

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            var allocatedBefore = GC.GetTotalAllocatedBytes(precise: true);
            var startWorkingSet = Environment.WorkingSet;
            var peakWorkingSet = startWorkingSet;

            for (var run = 0; run < TimedRuns; run++)
            {
                beforeEachRun?.Invoke();

                var stopwatch = Stopwatch.StartNew();
                _offerGenerator.GenerateDynamicOffers(expiredOffers);
                stopwatch.Stop();

                timings.Add(stopwatch.Elapsed.TotalMilliseconds);

                // Outside the timed region. The holder rejects offers over its per-template cap, so
                // a run that inherited the previous run's offers would measure rejection, not
                // generation
                offerCount = PurgeAddedOffers();
                peakWorkingSet = Math.Max(peakWorkingSet, Environment.WorkingSet);
            }

            return new RunStats(
                timings,
                offerCount,
                (GC.GetTotalAllocatedBytes(precise: true) - allocatedBefore) / (double)TimedRuns / 1024 / 1024,
                peakWorkingSet / 1024.0 / 1024.0,
                (peakWorkingSet - startWorkingSet) / 1024.0 / 1024.0
            );
        }
        finally
        {
            _ragfairConfig.ForceLegacyRagfairGeneration = original;
        }
    }

    /// <summary>
    /// The projection half of the native path on its own - the whole items table as views, every
    /// preset, and both price maps, rebuilt per pass.
    /// </summary>
    private RunStats MeasureProjection(List<List<Item>>? expiredOffers)
    {
        var timings = new List<double>(TimedRuns);

        for (var run = 0; run < WarmupRuns; run++)
        {
            _ = BuildRequest(expiredOffers);
        }

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        var allocatedBefore = GC.GetTotalAllocatedBytes(precise: true);
        var startWorkingSet = Environment.WorkingSet;
        var peakWorkingSet = startWorkingSet;

        for (var run = 0; run < TimedRuns; run++)
        {
            var stopwatch = Stopwatch.StartNew();
            _ = BuildRequest(expiredOffers);
            stopwatch.Stop();

            timings.Add(stopwatch.Elapsed.TotalMilliseconds);
            peakWorkingSet = Math.Max(peakWorkingSet, Environment.WorkingSet);
        }

        return new RunStats(
            timings,
            0,
            (GC.GetTotalAllocatedBytes(precise: true) - allocatedBefore) / (double)TimedRuns / 1024 / 1024,
            peakWorkingSet / 1024.0 / 1024.0,
            (peakWorkingSet - startWorkingSet) / 1024.0 / 1024.0
        );
    }

    private GenerateDynamicOffersRequest BuildRequest(List<List<Item>>? expiredOffers)
    {
        return RagfairPayloadProjection.BuildRequest(
            RagfairPayloadProjection.BuildViewsOverride(_templateTable, _handbookHelper, _traderHelper, _presetHelper, _itemHelper),
            0,
            expiredOffers,
            _timeUtil.GetTimeStamp(),
            0,
            null,
            _ragfairConfig,
            _itemFilterService,
            _seasonalEventService,
            _botTable,
            _botConfig
        );
    }

    private void GenerateAndPurge(List<List<Item>>? expiredOffers)
    {
        _offerGenerator.GenerateDynamicOffers(expiredOffers);
        _ = PurgeAddedOffers();
    }

    /// <summary>
    /// Empties the holder of everything the pass just put in it and reports how many offers that
    /// was - the offer count the path produced *and the holder accepted*, which is the number the
    /// timings are per.
    /// </summary>
    private int PurgeAddedOffers()
    {
        var offerIds = _offerService
            .GetOffers()
            .Select(offer => offer.Id)
            .Where(offerId => !_preExistingOfferIds.Contains(offerId))
            .ToList();
        foreach (var offerId in offerIds)
        {
            _offerService.RemoveOfferById(offerId);
        }

        return offerIds.Count;
    }

    private static void Report(string label, RunStats stats)
    {
        TestContext.Out.WriteLine(
            $"{label, -36} n={stats.Timings.Count}  mean={stats.Timings.Average():F2} ms  median={Median(stats.Timings):F2} ms  "
                + $"min={stats.Timings.Min():F2} ms  max={stats.Timings.Max():F2} ms"
        );
        TestContext.Out.WriteLine(
            $"{"", -36} offers={stats.OfferCount}  alloc/run={stats.AllocatedPerRunMb:F1} MB  "
                + $"peak RSS={stats.PeakWorkingSetMb:F0} MB (+{stats.WorkingSetGrowthMb:F0} MB over the phase)"
        );
    }

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }

    /// <param name="OfferCount"> Offers the holder held after the last timed run, before the purge </param>
    /// <param name="PeakWorkingSetMb">
    ///     Process-wide and cumulative - whichever phase runs later inherits the pages the earlier
    ///     ones left resident, so <paramref name="WorkingSetGrowthMb"/> is the figure to compare
    ///     across phases.
    /// </param>
    private sealed record RunStats(
        List<double> Timings,
        int OfferCount,
        double AllocatedPerRunMb,
        double PeakWorkingSetMb,
        double WorkingSetGrowthMb
    );
}
