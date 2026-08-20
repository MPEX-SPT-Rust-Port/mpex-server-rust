using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Json;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the wire contract between the loot payload records and the two native generation exports.
/// The requests built here are the shape <c>LocationLootGenerator</c> sends, so a renamed member or
/// a changed type on either side of the boundary fails in this fixture instead of at runtime.
/// </summary>
[TestFixture]
public class LocationLootGeneratorNativeTests
{
    private const string TestLocationId = "bigmap";
    private const string ContainerSpawnpointId = "c1";
    private const string ForcedSpawnpointId = "forced_1";

    private static readonly MongoId _containerTpl = new("111111111111111111111111");
    private static readonly MongoId _moneyTpl = new("333333333333333333333333");

    private JsonUtil _jsonUtil = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        // Publishes the static JsonSerializerOptions the wrappers serialise payloads with, exactly
        // as the server's DI container does at startup.
        _jsonUtil = new JsonUtil([new SptJsonConverterRegistrator()]);
    }

    [Test]
    public void StaticContainerRequestRoundTripsThroughTheNativeLibrary()
    {
        var result = SptNative.GenerateStaticContainers(BuildStaticRequest());

        Assert.That(result.StaticContainerCount, Is.EqualTo(1));
        // The container item itself plus the single money item drawn into it.
        Assert.That(result.StaticLootItemCount, Is.EqualTo(2));
        Assert.That(result.Spawnpoints, Has.Count.EqualTo(1));

        var container = result.Spawnpoints[0];
        Assert.That(container.Id, Is.EqualTo(ContainerSpawnpointId));
        // Rust re-roots the container with an id it generated itself; C# must be able to parse it.
        Assert.That(new MongoId(container.Root!).ToString(), Is.EqualTo(container.Root));

        var items = container.Items!.ToList();
        Assert.That(items, Has.Count.EqualTo(2));
        Assert.That(items[0].Template, Is.EqualTo(_containerTpl));
        // Upd members Rust does not model ride along in its passthrough map.
        Assert.That(items[0].Upd!.UnlimitedCount, Is.True);

        var money = items[1];
        Assert.That(money.Template, Is.EqualTo(_moneyTpl));
        Assert.That(money.SlotId, Is.EqualTo("main"));
        Assert.That(money.ParentId, Is.EqualTo(container.Root));
        // serde writes `150.0` where System.Text.Json writes `150`; only the parsed value matters.
        Assert.That(money.Upd!.StackObjectsCount, Is.InRange(100, 200));
    }

    [Test]
    public void GeneratedItemLocationRoundTripsThroughJsonUtil()
    {
        var result = SptNative.GenerateStaticContainers(BuildStaticRequest());
        var money = result.Spawnpoints[0].Items!.Last();

        var location = _jsonUtil.Deserialize<ItemLocation>(((JsonElement)money.Location!).GetRawText())!;

        // A 1x1 item in an empty 2x2 grid always lands unrotated in the first cell.
        Assert.That(location.X, Is.EqualTo(0));
        Assert.That(location.Y, Is.EqualTo(0));
        Assert.That(location.R, Is.EqualTo(ItemRotation.Horizontal));

        // `r` is a string on both sides of the boundary, never the enum's ordinal.
        var reserialised = JsonNode.Parse(_jsonUtil.Serialize(location)!)!;
        Assert.That(reserialised["r"]!.GetValue<string>(), Is.EqualTo("Horizontal"));
        var vertical = JsonNode.Parse(_jsonUtil.Serialize(new ItemLocation { R = ItemRotation.Vertical })!)!;
        Assert.That(vertical["r"]!.GetValue<string>(), Is.EqualTo("Vertical"));
    }

    [Test]
    public void ModAddedFieldsOnTheInputTemplateSurviveTheRoundTrip()
    {
        // Mod-added fields reach Rust through the [JsonExtensionData] property Ceciler injects into
        // the Models records on release builds. A debug build has no such property, so the field is
        // placed on the wire by hand here.
        var request = JsonNode.Parse(_jsonUtil.Serialize(BuildStaticRequest())!)!;
        request["viewsOverride"]!["staticContainers"]![0]!["template"]!["modAddedField"] = "kept";

        var result = SptNative.Generate<JsonNode>(LootExport.StaticContainers, Encoding.UTF8.GetBytes(request.ToJsonString()));

        Assert.That(result["spawnpoints"]![0]!["modAddedField"]!.GetValue<string>(), Is.EqualTo("kept"));
    }

    [Test]
    public void SpawnLimitsCrossTheBoundaryInBothDirections()
    {
        var request = BuildStaticRequest();
        request.Varying.Counter = new CounterState
        {
            MaxCounts = new Dictionary<MongoId, int> { [_moneyTpl] = 5 },
            TrackedCounts = new Dictionary<MongoId, int> { [_moneyTpl] = 4 },
        };

        var result = SptNative.GenerateStaticContainers(request);

        // The one money draw is counted on top of the four the request came in with.
        Assert.That(result.TrackedCounts[_moneyTpl], Is.EqualTo(5));
    }

    [Test]
    public void DynamicLootRequestRoundTripsThroughTheNativeLibrary()
    {
        var result = SptNative.GenerateDynamicLoot(BuildDynamicRequest());

        Assert.That(result.Spawnpoints, Has.Count.EqualTo(1));
        var forced = result.Spawnpoints[0];
        Assert.That(forced.Id, Is.EqualTo(ForcedSpawnpointId));
        Assert.That(new MongoId(forced.Root!).ToString(), Is.EqualTo(forced.Root));
        Assert.That(forced.Items!.Single().Template, Is.EqualTo(_moneyTpl));
        Assert.That(result.TrackedCounts, Is.Empty);
    }

    /// <summary>
    /// The raw form of the looseLoot payload reaches the wire byte for byte: a member the C# models do
    /// not declare and an explicit null both survive it, and neither would have survived the typed
    /// writer. This is what makes splicing the file safe - the raw JSON is the more faithful of the
    /// two, not the lossier.
    /// </summary>
    [Test]
    public void RawLooseLootJsonIsWrittenIntoTheRequestVerbatim()
    {
        var request = BuildDynamicRequest();
        var rawJson = JsonNode.Parse(_jsonUtil.Serialize(request.Varying.LooseLoot.Typed)!)!;
        rawJson["modAddedField"] = "kept";
        rawJson["modAddedNull"] = null;

        request.Varying.LooseLoot = LooseLootPayload.FromRawJson(Encoding.UTF8.GetBytes(rawJson.ToJsonString()));
        var serialised = JsonNode.Parse(_jsonUtil.Serialize(request)!)!;

        Assert.Multiple(() =>
        {
            Assert.That(
                JsonNode.DeepEquals(serialised["varying"]!["looseLoot"], rawJson),
                "the raw JSON was not written through unchanged"
            );
            // Spliced into a normal request, not in place of one
            Assert.That(serialised["varying"]!["locationId"]!.GetValue<string>(), Is.EqualTo(TestLocationId));
        });
    }

    /// <summary>
    /// The typed form is what every caller handing over a <c>LooseLoot</c> still gets: the wrapper is
    /// invisible on the wire.
    /// </summary>
    [Test]
    public void TheTypedLooseLootPayloadSerialisesAsThePlainModelDid()
    {
        var request = BuildDynamicRequest();

        var throughTheRequest = JsonNode.Parse(_jsonUtil.Serialize(request)!)!["varying"]!["looseLoot"];
        var onItsOwn = JsonNode.Parse(_jsonUtil.Serialize(request.Varying.LooseLoot.Typed)!);

        Assert.That(JsonNode.DeepEquals(throughTheRequest, onItsOwn), "the wrapper changed the JSON the model serialises to");
    }

    /// <summary>
    /// The spliced bytes have to be JSON the native side reads the same way as the typed form - a
    /// quoted or otherwise mangled raw value would fail to parse over there.
    /// </summary>
    [Test]
    public void ARawLooseLootRequestGeneratesTheSameSpawnpointsAsTheTypedOne()
    {
        var typedRequest = BuildDynamicRequest();
        var rawRequest = BuildDynamicRequest();
        rawRequest.Varying.LooseLoot = LooseLootPayload.FromRawJson(
            Encoding.UTF8.GetBytes(_jsonUtil.Serialize(typedRequest.Varying.LooseLoot.Typed)!)
        );

        var fromTyped = SptNative.GenerateDynamicLoot(typedRequest);
        var fromRaw = SptNative.GenerateDynamicLoot(rawRequest);

        // Item ids are minted per call, so only the spawn point identity and its loot can be compared
        Assert.That(
            fromRaw.Spawnpoints.Select(spawnpoint => spawnpoint.Id),
            Is.EqualTo(fromTyped.Spawnpoints.Select(spawnpoint => spawnpoint.Id))
        );
        Assert.That(fromRaw.Spawnpoints.Single().Items!.Single().Template, Is.EqualTo(_moneyTpl));
    }

    [Test]
    public void AGenerationFailureSurfacesTheNativeMessage()
    {
        var request = BuildStaticRequest();
        // The container still draws items, but its loot distribution is gone.
        request.ViewsOverride!.StaticLootDist = [];

        var error = Assert.Throws<InvalidOperationException>(() => SptNative.GenerateStaticContainers(request));

        Assert.That(error!.Message, Does.Contain($"Container: {_containerTpl} is missing from staticLoot.json"));
        Assert.That(error.Message, Does.Not.Contain("native library bug"));
    }

    [Test]
    public void AnUnparseableRequestIsReportedAsANativeBug()
    {
        var error = Assert.Throws<InvalidOperationException>(() =>
            SptNative.Generate<JsonNode>(LootExport.DynamicLoot, Encoding.UTF8.GetBytes("{\"locationId\":"))
        );

        Assert.That(error!.Message, Does.Contain("native library bug, not corrupt game data"));
        // The serde parse error rides back in the same buffer the success path uses.
        Assert.That(error.Message, Does.Contain("EOF while parsing"));
    }

    [Test]
    public void TestSeedIsOnTheWireOnlyWhenSet()
    {
        var withSeed = BuildStaticRequest();
        withSeed.Varying.TestSeed = 42;

        Assert.That(_jsonUtil.Serialize(withSeed), Does.Contain("\"testSeed\":42"));
        Assert.That(_jsonUtil.Serialize(BuildStaticRequest()), Does.Not.Contain("testSeed"));
    }

    [Test]
    public void TheSameTestSeedYieldsIdenticalResults()
    {
        // MongoIds are minted from the process-wide counter, not the seeded RNG — strip them.
        static string StripMongoIds(string json)
        {
            return Regex.Replace(json, "[0-9a-f]{24}", "<id>");
        }

        // Swept over seeds rather than repeated on one: a fixed seed replays the same draw values,
        // so an ordering hazard whose draws coincide under that seed would stay invisible however
        // often it ran.
        for (ulong seed = 0; seed < 10; seed++)
        {
            var requestA = BuildGroupedStaticRequest();
            requestA.Varying.TestSeed = seed;
            var requestB = BuildGroupedStaticRequest();
            requestB.Varying.TestSeed = seed;

            var resultA = SptNative.GenerateStaticContainers(requestA);
            var resultB = SptNative.GenerateStaticContainers(requestB);

            // Guards the fixture itself: one spawnpoint means only the guaranteed container came
            // back and the container-group path never ran, which would make the comparison hollow.
            Assert.That(resultA.Spawnpoints, Has.Count.GreaterThan(1), $"seed {seed} never reached the container-group path");
            Assert.That(resultA.TrackedCounts, Is.Not.Empty, $"seed {seed} left trackedCounts out of the comparison");
            // The whole result, not just the spawnpoints: `trackedCounts` is the other map whose
            // iteration order the seed has to pin down, and the counts ride along with it.
            Assert.That(
                StripMongoIds(_jsonUtil.Serialize(resultA)!),
                Is.EqualTo(StripMongoIds(_jsonUtil.Serialize(resultB)!)),
                $"seed {seed} did not reproduce"
            );
        }
    }

    /// <summary>
    /// <see cref="BuildStaticRequest"/> plus three randomisable containers split across two groups
    /// whose bounds differ. <see cref="BuildStaticRequest"/> leaves <c>Statics</c> null, so
    /// generation returns straight after the guaranteed container and never reaches the
    /// container-group code the seed most needs to pin down.
    /// </summary>
    private static StaticContainersRequest BuildGroupedStaticRequest()
    {
        var request = BuildStaticRequest();
        var views = request.ViewsOverride!;

        views.StaticContainers =
        [
            .. views.StaticContainers!,
            BuildRandomisableContainer("r1"),
            BuildRandomisableContainer("r2"),
            BuildRandomisableContainer("r3"),
        ];
        views.Statics = new StaticContainer
        {
            ContainersGroups = new Dictionary<string, ContainerMinMax>
            {
                ["g1"] = new ContainerMinMax { MinContainers = 1, MaxContainers = 2 },
                ["g2"] = new ContainerMinMax { MinContainers = 1, MaxContainers = 2 },
            },
            Containers = new Dictionary<string, ContainerData>
            {
                ["r1"] = new ContainerData { GroupId = "g1" },
                ["r2"] = new ContainerData { GroupId = "g1" },
                ["r3"] = new ContainerData { GroupId = "g2" },
            },
        };

        // Ceilings high enough never to bite, purely so tpls land in `trackedCounts` — it stays
        // empty under the base fixture, which would hide its ordering from the comparison.
        request.Varying.Counter = new CounterState
        {
            MaxCounts = new Dictionary<MongoId, int> { [_moneyTpl] = 9999, [_containerTpl] = 9999 },
            TrackedCounts = [],
        };

        return request;
    }

    private static StaticContainerData BuildRandomisableContainer(string spawnpointId)
    {
        return new StaticContainerData
        {
            // Under 1, so the container is randomised rather than guaranteed
            Probability = 0.5f,
            Template = new SpawnpointTemplate
            {
                Id = spawnpointId,
                IsContainer = true,
                Root = new MongoId().ToString(),
                Items = [new SptLootItem { Id = new MongoId(), Template = _containerTpl }],
            },
        };
    }

    /// <summary>
    /// One guaranteed container holding a 2x2 grid, with money as the only thing it can draw —
    /// sent as an override at epoch 0, the shape an ineligible caller puts on the wire.
    /// <c>Statics</c> is left null so the nullable-statics branch is exercised too.
    /// </summary>
    private static StaticContainersRequest BuildStaticRequest()
    {
        return new StaticContainersRequest
        {
            Epoch = 0,
            ViewsOverride = new LootViewsOverride
            {
                ItemsView = BuildItemsView(),
                DefaultPresets = [],
                StaticWeapons = [],
                StaticContainers =
                [
                    new StaticContainerData
                    {
                        Probability = 1,
                        Template = new SpawnpointTemplate
                        {
                            Id = ContainerSpawnpointId,
                            IsContainer = true,
                            Root = new MongoId().ToString(),
                            Items =
                            [
                                new SptLootItem
                                {
                                    Id = new MongoId(),
                                    Template = _containerTpl,
                                    Upd = new Upd { UnlimitedCount = true },
                                },
                            ],
                        },
                    },
                ],
                StaticForced = [],
                StaticLootDist = new Dictionary<MongoId, StaticLootDetails>
                {
                    [_containerTpl] = new StaticLootDetails
                    {
                        ItemCountDistribution = [new ItemCountDistribution { Count = 1, RelativeProbability = 1 }],
                        ItemDistribution = [new ItemDistribution { Tpl = _moneyTpl, RelativeProbability = 1 }],
                    },
                },
                Statics = null,
                Config = BuildConfig(),
                ChristmasContainerIds = [],
            },
            Varying = new LootVarying
            {
                LocationId = TestLocationId,
                MoneyTpls = [_moneyTpl],
                StaticAmmoDist = [],
                Seasonal = BuildSeasonal(),
                LootableItemBlacklist = [],
                Counter = new CounterState { MaxCounts = [], TrackedCounts = [] },
            },
        };
    }

    /// <summary>
    /// A single forced loose loot point holding money, and no random points to draw. The dynamic
    /// override carries no statics members.
    /// </summary>
    private static DynamicLootRequest BuildDynamicRequest()
    {
        return new DynamicLootRequest
        {
            Epoch = 0,
            ViewsOverride = new LootViewsOverride
            {
                ItemsView = BuildItemsView(),
                DefaultPresets = [],
                Config = BuildConfig(),
                ChristmasContainerIds = [],
            },
            Varying = new DynamicLootVarying
            {
                LocationId = TestLocationId,
                MoneyTpls = [_moneyTpl],
                StaticAmmoDist = [],
                Seasonal = BuildSeasonal(),
                LootableItemBlacklist = [],
                Counter = new CounterState { MaxCounts = [], TrackedCounts = [] },
                LooseLoot = new LooseLoot
                {
                    SpawnpointCount = new SpawnpointCount { Mean = 0, Std = 0 },
                    SpawnpointsForced =
                    [
                        new Spawnpoint
                        {
                            LocationId = ForcedSpawnpointId,
                            Probability = 1,
                            Template = new SpawnpointTemplate
                            {
                                Id = ForcedSpawnpointId,
                                Root = new MongoId().ToString(),
                                Items = [new SptLootItem { Id = new MongoId(), Template = _moneyTpl }],
                            },
                        },
                    ],
                    Spawnpoints = [],
                },
            },
        };
    }

    /// <summary>
    /// The base class nodes the tpl walk needs, the 2x2 container and the money it holds.
    /// </summary>
    private static Dictionary<MongoId, ItemView> BuildItemsView()
    {
        return new Dictionary<MongoId, ItemView>
        {
            [BaseClasses.ITEM] = new ItemView(),
            [BaseClasses.MONEY] = new ItemView { Parent = BaseClasses.ITEM },
            [_containerTpl] = new ItemView
            {
                Parent = BaseClasses.ITEM,
                Width = 1,
                Height = 1,
                GridCellsH = 2,
                GridCellsV = 2,
            },
            [_moneyTpl] = new ItemView
            {
                Parent = BaseClasses.MONEY,
                Width = 1,
                Height = 1,
                StackMaxSize = 500000,
                StackMinRandom = 100,
                StackMaxRandom = 200,
            },
        };
    }

    private static LootConfigView BuildConfig()
    {
        return new LootConfigView
        {
            ContainerRandomisationEnabled = true,
            LocationInRandomisationMaps = true,
            ContainerTypesToNotRandomise = [],
            ContainerGroupMinSizeMultiplier = 1,
            ContainerGroupMaxSizeMultiplier = 1,
            AllowDuplicateItemsInStaticContainers = true,
            TplsToStripChildItemsFrom = [],
            FitLootIntoContainerAttempts = 3,
            MagazineLootHasAmmoChancePercent = 0,
            StaticMagazineLootHasAmmoChancePercent = 0,
            MinFillLooseMagazinePercent = 0,
            MinFillStaticMagazinePercent = 0,
            StaticLootMultiplier = 1,
            LooseLootMultiplier = 1,
            ModSpawnChancePercent = [],
            LooseLootBlacklist = [],
        };
    }

    private static SeasonalView BuildSeasonal()
    {
        return new SeasonalView
        {
            SeasonalEventActive = false,
            ChristmasEventEnabled = false,
            InactiveSeasonalItems = [],
        };
    }
}
