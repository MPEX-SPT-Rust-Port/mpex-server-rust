using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Repeatable;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.RepeatableQuests;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the halves of the <c>spt_generate_repeatable_quest</c> request the Rust side parses by name:
/// the invariant slice's property set and key casing, the varying half's wire names, and the cache
/// eligibility truth table. Mutates the shared <see cref="QuestConfig"/> singleton, so it restores
/// the flags it flips and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RepeatableQuestNativeRequestBuilderTests
{
    private RepeatableQuestNativeRequestBuilder _builder = default!;
    private QuestConfig _questConfig = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _builder = di.GetService<RepeatableQuestNativeRequestBuilder>();
        _questConfig = di.GetService<QuestConfig>();
    }

    [TearDown]
    public void TearDown()
    {
        _questConfig.TrustNativeRequestCacheWithMods = false;
        _questConfig.DisableNativeRequestCache = false;
    }

    /// <summary>
    /// <c>QuestInvariantSlice</c> in <c>rust/spt-native/src/quest/models.rs:864-922</c>, whose members
    /// are all non-<c>Option</c> bar the two collapsed completion filters: a renamed or dropped member
    /// is a parse failure on the far side, not a silent default.
    /// </summary>
    private static readonly string[] _sliceProperties =
    [
        "items",
        "handbookPrices",
        "fleaPrices",
        "defaultWeaponPresets",
        "defaultPresetOrItemPrices",
        "itemBlacklist",
        "rewardItemBlacklist",
        "bossItems",
        "seasonalItemTplBlacklist",
        "repeatableQuestTemplates",
        "completionItemsWhitelist",
        "completionItemsBlacklist",
        "bossSpawnsByLocation",
        "extractsByLocation",
        "repeatableQuestTemplateIds",
        "locationIdMap",
    ];

    /// <summary>
    /// The options the wrapper itself serialises with, so what these cases read is what the native
    /// side would have parsed.
    /// </summary>
    private static JsonElement Serialize(object payload)
    {
        return JsonSerializer.SerializeToElement(payload, SptNative.QuestJsonOptions);
    }

    [Test]
    public void TheInvariantSliceCarriesExactlyTheLockedWireProperties()
    {
        var slice = Serialize(_builder.BuildInvariantSlice());

        var written = slice.EnumerateObject().Select(property => property.Name).ToList();

        Assert.That(written, Is.EquivalentTo(_sliceProperties));
    }

    [Test]
    public void TheSliceKeepsTheQuestTemplateAndTemplateIdKeysPascalCase()
    {
        var slice = Serialize(_builder.BuildInvariantSlice());

        Assert.That(slice.GetProperty("repeatableQuestTemplates").TryGetProperty("Elimination", out _), Is.True);
        Assert.That(slice.GetProperty("repeatableQuestTemplateIds").GetProperty("pmc").TryGetProperty("Elimination", out _), Is.True);
    }

    /// <summary>
    /// The two location maps use different key domains on purpose: boss spawns are keyed by the raw
    /// mixed-case <c>LocationBase.Id</c> the elimination blacklist is compared against, extracts by the
    /// lowercased pool key <c>GetLocation</c> is called with.
    /// </summary>
    [Test]
    public void TheTwoLocationMapsUseTheirOwnKeyCasing()
    {
        var slice = Serialize(_builder.BuildInvariantSlice());

        var bossSpawnKeys = slice.GetProperty("bossSpawnsByLocation").EnumerateObject().Select(property => property.Name).ToList();
        Assert.That(bossSpawnKeys, Does.Contain("Interchange"));
        Assert.That(bossSpawnKeys, Does.Contain("Sandbox_high"));

        var extractKeys = slice.GetProperty("extractsByLocation").EnumerateObject().Select(property => property.Name).ToList();
        Assert.That(extractKeys, Is.EqualTo(extractKeys.Select(key => key.ToLowerInvariant()).ToList()));
        // The pool draws these keys from ELocationName, so every map it can name has to be reachable
        Assert.That(extractKeys, Does.Contain("rezervbase"));
        Assert.That(extractKeys, Does.Contain("factory4_day"));
    }

    /// <summary>
    /// An extract crosses as the four-member projection, not the whole <c>Exit</c> record: a
    /// PascalCase passthrough would land every member on the Rust side as <c>None</c>.
    /// </summary>
    [Test]
    public void AnExtractIsProjectedToTheFourMemberExitView()
    {
        var slice = Serialize(_builder.BuildInvariantSlice());

        var exit = slice
            .GetProperty("extractsByLocation")
            .EnumerateObject()
            .Select(property => property.Value)
            .First(extracts => extracts.GetArrayLength() > 0)[0];

        Assert.That(
            exit.EnumerateObject().Select(property => property.Name),
            Is.SubsetOf(new[] { "name", "side", "chance", "passageRequirement" })
        );
        Assert.That(exit.GetProperty("passageRequirement").ValueKind, Is.EqualTo(JsonValueKind.String));
    }

    /// <summary>
    /// <c>FromRoubles</c> reads the conversion rate out of the handbook map, so a missing currency tpl
    /// silently prices every converted reward at zero.
    /// </summary>
    [Test]
    public void TheHandbookPricesCoverTheCurrencyTpls()
    {
        var handbookPrices = Serialize(_builder.BuildInvariantSlice()).GetProperty("handbookPrices");

        foreach (var currency in Money.GetMoneyTpls())
        {
            Assert.That(
                handbookPrices.TryGetProperty(currency.ToString(), out _),
                Is.True,
                $"{currency} is missing from the handbook prices"
            );
        }
    }

    [Test]
    public void TheVaryingHalfCarriesTheLockedWireNames()
    {
        var varying = Serialize(BuildVarying(seed: null));

        Assert.That(
            varying.EnumerateObject().Select(property => property.Name),
            Is.EquivalentTo(new[] { "questType", "sessionId", "pmcLevel", "traderId", "questTypePool", "repeatableConfig" })
        );
        // A closed enum on the far side: a pool-drawn string would come back as an internal-status error
        Assert.That(varying.GetProperty("questType").GetString(), Is.EqualTo("Exploration"));
        // Test-only, and named `seed` - not `testSeed` like the older families
        Assert.That(Serialize(BuildVarying(seed: 424242)).GetProperty("seed").GetUInt64(), Is.EqualTo(424242UL));
    }

    /// <summary>
    /// Both halves carry <c>ELocationName</c>-keyed dictionaries, which the Rust side reads as
    /// string-keyed maps of location names.
    /// </summary>
    [Test]
    public void TheLocationKeyedDictionariesCrossAsLocationNames()
    {
        var varying = Serialize(BuildVarying(seed: null));

        Assert.That(
            varying
                .GetProperty("questTypePool")
                .GetProperty("pool")
                .GetProperty("Exploration")
                .GetProperty("locations")
                .TryGetProperty("bigmap", out _),
            Is.True
        );
        Assert.That(varying.GetProperty("repeatableConfig").GetProperty("locations").TryGetProperty("bigmap", out _), Is.True);
        Assert.That(varying.GetProperty("repeatableConfig").GetProperty("side").GetString(), Is.EqualTo("Pmc"));
    }

    /// <summary>
    /// The only end-to-end proof that both halves parse: property names pin the shape, but a member
    /// whose converter writes the wrong JSON type - an enum as its ordinal, say - only shows up when
    /// the native side actually reads it. The second pass also pins the cache gate.
    /// </summary>
    [Test]
    public void AFullSendGeneratesNativelyAndTheNextSendSkipsTheSlice()
    {
        var (quest, pool) = _builder.Send(
            RepeatableQuestType.Exploration,
            "6193a720f8ee7e52e4290000",
            20,
            Traders.PRAPOR,
            BuildPool(),
            _questConfig.RepeatableQuests[0],
            424242
        );

        Assert.That(_builder.LastSendIncludedSlice, Is.True);
        Assert.That(quest, Is.Not.Null);
        Assert.That(quest!.Location, Is.Not.Null);
        // The generator consumes the location it drew
        Assert.That(pool.Pool.Exploration.Locations, Has.Count.LessThan(BuildPool().Pool.Exploration.Locations!.Count));

        _builder.Send(
            RepeatableQuestType.Exploration,
            "6193a720f8ee7e52e4290000",
            20,
            Traders.PRAPOR,
            BuildPool(),
            _questConfig.RepeatableQuests[0],
            424242
        );

        Assert.That(_builder.LastSendIncludedSlice, Is.False, "the unchanged stamp should have hit the native cache");
    }

    /// <summary>
    /// <c>RepeatableQuestController.GenerateQuestPool</c> (<c>:840-884</c>) for the exploration half;
    /// the elimination targets stay empty, no exploration draw reads them.
    /// </summary>
    private QuestTypePool BuildPool()
    {
        var repeatableConfig = _questConfig.RepeatableQuests[0];
        var locations = repeatableConfig.Locations.Where(location => location.Key != ELocationName.any);

        return new QuestTypePool
        {
            Types = [.. repeatableConfig.Types],
            Pool = new QuestPool
            {
                Exploration = new ExplorationPool
                {
                    Locations = locations.ToDictionary(location => location.Key, location => location.Value),
                },
                Elimination = new EliminationPool { Targets = [] },
                Pickup = new ExplorationPool { Locations = [] },
            },
        };
    }

    [TestCase(0, false, false, true)]
    [TestCase(0, true, false, true)]
    [TestCase(0, false, true, false)]
    [TestCase(0, true, true, false)]
    [TestCase(1, false, false, false)]
    [TestCase(1, true, false, true)]
    [TestCase(1, false, true, false)]
    [TestCase(1, true, true, false)]
    public void CacheEligibilityFollowsTheModStateAndTheTwoFlags(int modCount, bool trust, bool disable, bool expected)
    {
        _questConfig.TrustNativeRequestCacheWithMods = trust;
        _questConfig.DisableNativeRequestCache = disable;

        // The gate only reads Count, so placeholder elements stand in for real mods
        var builder = BuildWithMods(Enumerable.Repeat<SptMod>(null!, modCount).ToList());

        Assert.That(builder.CacheEligible(), Is.EqualTo(expected));
    }

    private RepeatableQuestVaryingFields BuildVarying(ulong? seed)
    {
        return _builder.BuildVarying(
            RepeatableQuestType.Exploration,
            "6193a720f8ee7e52e4290000",
            20,
            Traders.PRAPOR,
            new QuestTypePool
            {
                Types = ["Exploration"],
                Pool = new QuestPool
                {
                    Exploration = new ExplorationPool
                    {
                        Locations = new Dictionary<ELocationName, List<string>> { [ELocationName.bigmap] = ["bigmap"] },
                    },
                    Elimination = new EliminationPool { Targets = [] },
                    Pickup = new ExplorationPool { Locations = [] },
                },
            },
            _questConfig.RepeatableQuests[0],
            seed
        );
    }

    private static RepeatableQuestNativeRequestBuilder BuildWithMods(IReadOnlyList<SptMod> mods)
    {
        var di = DI.GetInstance();
        var constructor = typeof(RepeatableQuestNativeRequestBuilder).GetConstructors().Single();
        var arguments = constructor
            .GetParameters()
            .Select(parameter => parameter.ParameterType == typeof(IReadOnlyList<SptMod>) ? mods : di.GetService(parameter.ParameterType))
            .ToArray();

        return (RepeatableQuestNativeRequestBuilder)constructor.Invoke(arguments);
    }
}
