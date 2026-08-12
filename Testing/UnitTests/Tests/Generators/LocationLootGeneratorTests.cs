using System.Diagnostics;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Json;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Smoke test for the whole native loot path: the payload builder reading the live database, the two
/// generation calls, and the diagnostic replay through <c>ServerLocalisationService</c>.
/// <see cref="LocationLootGeneratorNativeTests"/> pins the wire contract on synthetic data; this
/// fixture is what catches a projection that is wrong only for real game data.
/// </summary>
[TestFixture]
public class LocationLootGeneratorTests
{
    private const string LocationId = "bigmap";
    private const string SmokeLocationId = "factory4_day";

    /// <summary>
    /// Spawn point ids that only exist in the loot data a single test hands over, so a result can be
    /// traced back to the loose loot it was generated from.
    /// </summary>
    private const string RawFileMarkerId = "raw_file_marker";

    private const string TransformedMarkerId = "transformed_marker";

    /// <summary>
    /// Mean wall-clock of the deleted C# <c>GenerateLocationLoot("bigmap")</c> over 200 Release runs,
    /// recorded in the Task 12 harness commit af4f5b8c. The native path has to come in under it.
    /// </summary>
    private const double CSharpBaselineMs = 929.83;

    private const int TimedRuns = 10;

    private LocationLootGenerator _locationLootGenerator = default!;
    private JsonUtil _jsonUtil = default!;
    private TemplateTable _templateTable = default!;
    private LocationTable _locationTable = default!;
    private LocationConfig _locationConfig = default!;
    private SeasonalEventService _seasonalEventService = default!;

    private List<SpawnpointTemplate> _spawnpoints = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions SptNative serialises payloads with
        _jsonUtil = di.GetService<JsonUtil>();

        _locationLootGenerator = di.GetService<LocationLootGenerator>();
        _templateTable = di.GetService<TemplateTable>();
        _locationTable = di.GetService<LocationTable>();
        _locationConfig = di.GetService<LocationConfig>();
        _seasonalEventService = di.GetService<SeasonalEventService>();

        // One raid's worth of loot, shared by every assertion below - generation is the expensive part
        _spawnpoints = _locationLootGenerator.GenerateLocationLoot(LocationId);
    }

    [Test]
    public void GenerateLocationLootFillsAMapWithStaticAndLooseLoot()
    {
        var containers = _spawnpoints.Where(spawnpoint => spawnpoint.IsContainer ?? false).ToList();
        var loosePoints = _spawnpoints.Where(spawnpoint => !(spawnpoint.IsContainer ?? false)).ToList();

        Assert.Multiple(() =>
        {
            Assert.That(_spawnpoints, Is.Not.Empty, "no spawn points at all were generated");
            Assert.That(containers, Is.Not.Empty, "no container spawn points were generated");
            Assert.That(loosePoints, Is.Not.Empty, "no loose loot spawn points were generated");
            Assert.That(
                containers.Sum(container => container.Items!.Count()),
                Is.GreaterThan(containers.Count),
                "no loot went into any container"
            );
            Assert.That(_spawnpoints.All(spawnpoint => spawnpoint.Items?.Any() ?? false), "a spawn point came back with no items at all");
        });
    }

    /// <summary>
    /// Every tpl the native side put in the result has to be a real item: a projection that dropped or
    /// mangled a template id would show up here as loot referencing something the client cannot render.
    /// </summary>
    [Test]
    public void GeneratedItemsOnlyReferenceTemplatesThatExist()
    {
        var unknownTpls = _spawnpoints
            .SelectMany(spawnpoint => spawnpoint.Items!)
            .Select(item => item.Template)
            .Distinct()
            .Where(tpl => !_templateTable.Items.ContainsKey(tpl))
            .ToList();

        Assert.That(unknownTpls, Is.Empty, "generated loot references templates missing from the items table");
    }

    /// <summary>
    /// Ids are minted on the native side; a broken generator or a lost re-root shows up as an id C#
    /// cannot parse, or as two items in one spawn point sharing an id (which the client treats as one).
    /// Ids are only unique inside a spawn point - the same spawn point template id legitimately appears
    /// more than once across the result.
    /// </summary>
    [Test]
    public void GeneratedItemIdsAreValidAndUniqueWithinTheirSpawnpoint()
    {
        Assert.Multiple(() =>
        {
            foreach (var spawnpoint in _spawnpoints)
            {
                var ids = spawnpoint.Items!.Select(item => item.Id.ToString()).ToList();

                Assert.That(ids.All(MongoId.IsValidMongoId), $"spawn point {spawnpoint.Id} has an item id that is not a MongoId");
                Assert.That(ids.Distinct().Count(), Is.EqualTo(ids.Count), $"spawn point {spawnpoint.Id} reused an item id");
                Assert.That(
                    spawnpoint.Root is not null && MongoId.IsValidMongoId(spawnpoint.Root),
                    $"spawn point {spawnpoint.Id} has no valid root id"
                );
                Assert.That(ids, Does.Contain(spawnpoint.Root), $"spawn point {spawnpoint.Id} roots on an item it does not contain");
            }
        });
    }

    /// <summary>
    /// Quest items forced into a named container have to actually be there, otherwise the quest is
    /// uncompletable. A container that rolled zero items is skipped before the forced list is read, so
    /// only containers that got loot are checked.
    /// </summary>
    [Test]
    public void ForcedStaticItemsLandInTheirContainer()
    {
        var forced = _locationTable.GetLocation(LocationId)!.StaticContainers!.Value!.StaticForced;

        // A container id can appear more than once in the result - a lookup, not a dictionary
        var containersById = _spawnpoints
            .Where(spawnpoint => (spawnpoint.IsContainer ?? false) && spawnpoint.Items!.Count() > 1)
            .ToLookup(spawnpoint => spawnpoint.Id!, spawnpoint => spawnpoint.Items!.Select(item => item.Template).ToHashSet());

        var checkedCount = 0;

        Assert.Multiple(() =>
        {
            foreach (var forcedItem in forced)
            {
                // Container did not spawn this raid, or rolled no loot at all
                foreach (var tpls in containersById[forcedItem.ContainerId])
                {
                    checkedCount++;
                    Assert.That(tpls, Does.Contain(forcedItem.ItemTpl), $"container {forcedItem.ContainerId} is missing its forced item");
                }
            }
        });

        TestContext.Out.WriteLine($"checked {checkedCount} of {forced.Count()} forced static entries that spawned with loot");

        // Without this the test passes having asserted nothing: bigmap's one forced entry sits in a
        // probability-1 container today, but a container that rolled zero loot items trips the filter
        // above and every assertion silently disappears
        Assert.That(checkedCount, Is.GreaterThan(0), "no forced static container spawned with loot, so nothing was checked");
    }

    /// <summary>
    /// <c>lootMaxSpawnLimits</c> caps how many of a tpl a raid may contain, enforced on the native side
    /// against the counter state handed over in the payload.
    /// <para>
    /// Read this as a ceiling check, not as counter coverage. All 11 tpls bigmap limits appear only in
    /// its loose <c>spawnpointsForced</c>, and 10 of the 11 under a single distinct spawn point template
    /// id, where the forced-point id dedupe in <c>location_loot_generator.rs:1201-1213</c> already caps
    /// them at one occurrence with the counter fully broken. Only 683ed6c2e4b1dd7ec4069dc8 has two
    /// distinct ids, and their probabilities are low enough that it exercises the counter in a low
    /// single-digit percentage of runs. Nothing here sees the static-to-dynamic counter carry at all.
    /// </para>
    /// </summary>
    [Test]
    public void SpawnLimitedItemsStayUnderTheirConfiguredMaximum()
    {
        var limits = _locationConfig.LootMaxSpawnLimits[LocationId];
        Assert.That(limits, Is.Not.Empty, "bigmap has no spawn limits configured, this test asserts nothing");

        var counts = _spawnpoints
            .SelectMany(spawnpoint => spawnpoint.Items!)
            .Select(item => item.Template)
            .Where(limits.ContainsKey)
            .GroupBy(tpl => tpl)
            .ToDictionary(group => group.Key, group => group.Count());

        Assert.Multiple(() =>
        {
            foreach (var (tpl, count) in counts)
            {
                Assert.That(count, Is.LessThanOrEqualTo(limits[tpl]), $"tpl {tpl} spawned {count} times, limit is {limits[tpl]}");
            }
        });
    }

    /// <summary>
    /// The result is handed straight to the client, so anything the native side produced that
    /// <c>JsonUtil</c> cannot write - a cyclic structure, an unconvertible member - has to fail here.
    /// </summary>
    [Test]
    public void GeneratedLootSerialises()
    {
        var json = _jsonUtil.Serialize(_spawnpoints);

        Assert.That(json, Is.Not.Null.And.Not.Empty);
    }

    /// <summary>
    /// A second map, to catch anything that only holds for bigmap's data shape.
    /// </summary>
    [Test]
    public void GenerateLocationLootFillsASecondMap()
    {
        var spawnpoints = _locationLootGenerator.GenerateLocationLoot(SmokeLocationId);

        Assert.Multiple(() =>
        {
            Assert.That(spawnpoints, Is.Not.Empty, $"no spawn points were generated for {SmokeLocationId}");
            Assert.That(
                spawnpoints.SelectMany(spawnpoint => spawnpoint.Items!).All(item => _templateTable.Items.ContainsKey(item.Template)),
                $"{SmokeLocationId} loot references templates missing from the items table"
            );
        });
    }

    /// <summary>
    /// The generator is stateless per call, so a second raid on the same map generates just as much
    /// loot - a leaked spawn-limit tracker or a mutated database would show up as an empty result.
    /// </summary>
    [Test]
    public void GenerateLocationLootIsRepeatable()
    {
        var second = _locationLootGenerator.GenerateLocationLoot(LocationId);

        Assert.That(second, Has.Count.GreaterThan(_spawnpoints.Count / 2));
    }

    /// <summary>
    /// Whether the raw JSON shortcut is taken is invisible in the generated loot - the only other
    /// thing that observes it is <see cref="GenerateLocationLootBeatsTheCSharpBaseline"/>, and only on
    /// release builds. A startup transformer that changes no loot would quietly put every raid back on
    /// the 42 MB parse, which is exactly what vanilla <c>loot.json</c> used to do: it lists bigmap in
    /// two transformer-driving sections with nothing in either.
    /// </summary>
    [Test]
    public void BigmapLooseLootIsOnTheRawJsonPathAfterStartup()
    {
        if (_seasonalEventService.ChristmasEventEnabled())
        {
            Assert.Ignore("the christmas event registers a loose loot transformer that does change loot, so the typed path is correct");
        }

        var looseLoot = _locationTable.GetLocation(LocationId)!.LooseLoot!;

        Assert.Multiple(() =>
        {
            Assert.That(looseLoot.HasRawJson, "the database importer did not hand the lazy load a raw JSON source");
            Assert.That(looseLoot.HasTransformers, Is.False, "a startup transformer took loot generation off the raw JSON path");
        });
    }

    /// <summary>
    /// A location whose loose loot is untransformed goes over the boundary as the raw file JSON, which
    /// is only equivalent while nothing transforms it. A registered transformer has to put the
    /// generator back on the typed path: the file and the transformer are marked with different spawn
    /// point ids, so a result generated from the wrong one of the two is visible either way.
    /// </summary>
    [Test]
    public void ARegisteredTransformerIsHonouredInsteadOfTheRawFileJson()
    {
        var location = _locationTable.GetLocation(LocationId)!;
        var originalLooseLoot = location.LooseLoot;
        var file = Path.Combine(Path.GetTempPath(), $"looseLoot-{Guid.NewGuid():N}.json");

        try
        {
            // File-backed exactly as the database importer builds it, so the raw JSON *is* available
            // and only the registered transformer can keep the generator off it
            File.WriteAllText(file, _jsonUtil.Serialize(BuildMarkedLooseLoot(RawFileMarkerId))!);

            var lazyLoad = new LazyLoad<LooseLoot>(
                () => _jsonUtil.DeserializeFromFile<LooseLoot>(file)!,
                () => new ReadOnlyMemory<byte>(File.ReadAllBytes(file))
            );
            lazyLoad.AddTransformer(_ => BuildMarkedLooseLoot(TransformedMarkerId));
            location.LooseLoot = lazyLoad;

            var spawnpointIds = _locationLootGenerator.GenerateLocationLoot(LocationId).Select(spawnpoint => spawnpoint.Id).ToList();

            Assert.Multiple(() =>
            {
                Assert.That(spawnpointIds, Does.Contain(TransformedMarkerId), "the transformer never ran");
                Assert.That(spawnpointIds, Does.Not.Contain(RawFileMarkerId), "the raw file JSON was spliced past the transformer");
            });
        }
        finally
        {
            // The DI container is shared with every other fixture
            location.LooseLoot = originalLooseLoot;
            File.Delete(file);
        }
    }

    /// <summary>
    /// A caller that hands over its own loot data - a mod, or the transformed path above - generates
    /// from exactly that, never from the location's file.
    /// </summary>
    [Test]
    public void GenerateDynamicLootGeneratesFromTheLooseLootItWasGiven()
    {
        var location = _locationTable.GetLocation(LocationId)!;

        var spawnpoints = _locationLootGenerator.GenerateDynamicLoot(
            BuildMarkedLooseLoot(TransformedMarkerId),
            location.StaticAmmo,
            LocationId
        );

        Assert.That(spawnpoints, Has.Count.EqualTo(1), "the given loot data holds exactly one forced spawn point");
        Assert.That(spawnpoints[0].Id, Is.EqualTo(TransformedMarkerId));
    }

    /// <summary>
    /// One forced loose loot point holding roubles, tagged with <paramref name="markerId"/>. The mean
    /// spawn point count is zero, so the forced point is the only thing that can come back.
    /// </summary>
    private static LooseLoot BuildMarkedLooseLoot(string markerId)
    {
        return new LooseLoot
        {
            SpawnpointCount = new SpawnpointCount { Mean = 0, Std = 0 },
            SpawnpointsForced =
            [
                new Spawnpoint
                {
                    LocationId = markerId,
                    Probability = 1,
                    Template = new SpawnpointTemplate
                    {
                        Id = markerId,
                        Root = new MongoId().ToString(),
                        Items = [new SptLootItem { Id = new MongoId(), Template = Money.ROUBLES }],
                    },
                },
            ],
            Spawnpoints = [],
        };
    }

    /// <summary>
    /// The port only pays for itself if it is faster than the C# it replaced. The assertion is compiled
    /// out of Debug builds: the native library is built with cargo's debug profile there and runs a few
    /// times slower, which says nothing about the shipped binary. The numbers are logged either way.
    /// </summary>
    [Test]
    public void GenerateLocationLootBeatsTheCSharpBaseline()
    {
        var location = _locationTable.GetLocation(LocationId)!;
        var originalLooseLoot = location.LooseLoot!;
        var timings = new List<double>(TimedRuns);

        // Pin what is measured to the shipping default: loose loot with no transformers, generated
        // from the same raw file JSON. A seasonal event - or a mod - registering a LooseLoot
        // transformer correctly puts generation on the typed path at ~1347 ms, which is a documented
        // ceiling of that path (ARCHITECTURE.md), not something this gate should swing on for the 32
        // days a year the christmas windows cover. Deserialising is not part of the measured path, so
        // reaching for it means the gate is timing something else and has to say so.
        location.LooseLoot = new LazyLoad<LooseLoot>(
            () => throw new InvalidOperationException("the perf gate must measure the raw loose loot JSON path"),
            originalLooseLoot.ReadRawJson
        );

        try
        {
            // First call pays JIT, the native library load and the LazyLoad materialisation
            _locationLootGenerator.GenerateLocationLoot(LocationId);

            for (var run = 0; run < TimedRuns; run++)
            {
                var stopwatch = Stopwatch.StartNew();
                _locationLootGenerator.GenerateLocationLoot(LocationId);
                stopwatch.Stop();

                timings.Add(stopwatch.Elapsed.TotalMilliseconds);
            }
        }
        finally
        {
            // The DI container is shared with every other fixture
            location.LooseLoot = originalLooseLoot;
        }

        var mean = timings.Average();
        TestContext.Out.WriteLine(
            $"GenerateLocationLoot(\"{LocationId}\") over {TimedRuns} runs: mean {mean:F2} ms, min {timings.Min():F2} ms, "
                + $"max {timings.Max():F2} ms (C# baseline {CSharpBaselineMs:F2} ms)"
        );

#if !DEBUG
        Assert.That(mean, Is.LessThanOrEqualTo(CSharpBaselineMs), "native loot generation is slower than the C# it replaced");
#endif
    }
}
