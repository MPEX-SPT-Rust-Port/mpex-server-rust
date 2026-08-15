using System.Reflection;
using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Ragfair;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Golden parity gate on the ragfair port: the same seed must make the legacy 4.1.2 C# path and the
/// spt-native path build byte-equal offers (after <see cref="LootIdNormalizer"/>) for one item at a
/// time. Whole-pass parity is not promised - legacy fans the assort out over tasks sharing one RNG
/// (<c>RagfairOfferGenerator.cs:446</c>), so its draw interleaving is nondeterministic even under a
/// fixed seed; a full native pass is checked structurally instead.
///
/// The per-item vehicle is <c>GenerateDynamicOffers(expiredOffers)</c>: it takes the item lists
/// directly (<c>:434</c>), so the pass covers exactly the items handed in, and it is what
/// <c>RagfairServer.cs:79</c> calls. Its cost is that <c>isExpiredOffer</c> skips
/// <c>IsItemValidRagfairItem</c>, <c>RemoveBannedPlatesFromPreset</c>, <c>RemoveArmorPlates</c> and
/// the offer-count draw; those branches are covered by the Rust module tests, the replay test and
/// the structural case below.
///
/// Three pieces of live state make this fixture invasive, so it never runs in parallel:
/// <list type="number">
/// <item>
/// The shared <see cref="DI"/> container runs every <c>IOnLoad</c>, and <c>RagfairServer.Load()</c>
/// pre-generates a full flea before any test here executes. A tpl that is already stocked makes the
/// holder's <c>_fakePlayerOffers</c> lookup succeed, which fires its per-template cap draw
/// (<c>RagfairServerHelper.GetOfferCountByBaseType</c>, a seeded <c>GetInt</c>) inside
/// <c>AddOffer</c> - at a different point in the stream on each path, and possibly rejecting the
/// offer on one path but not the other. So every run first purges the holder of every *fake-player*
/// offer for its case tpl, pre-existing ones included; they are test-session state and are not
/// restored. Trader and player offers for that tpl are left alone - the cap never reads them.
/// </item>
/// <item>
/// The config flag, the two forced-branch chances, the <c>RandomUtil</c> seam and
/// <c>ProbabilityRandomSource.Current</c> are restored in a <c>finally</c>, along with the offers
/// each run adds.
/// </item>
/// <item>
/// <c>OfferCounter</c> is a live instance counter, so <c>intId</c> starts from a different value on
/// the two runs by construction. It is dropped before comparison and its contract is asserted
/// separately by <see cref="TheOfferCounterAdvancesPerOfferOnTheNativePath"/>.
/// </item>
/// </list>
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairParityTests
{
    private static readonly ulong[] _seeds = [42, 1337];

    // One tpl per item class the spec names. Resolved against the live database in BuildItem rather
    // than hard-coded, so the fixture tracks the shipped data.
    private static readonly string[] _itemClasses =
    [
        "weapon-default-preset",
        "armor-with-removable-plates",
        "ammo",
        "plain-barter-eligible",
        "pack-eligible",
        "money",
    ];

    private RagfairOfferGenerator _ragfairOfferGenerator = default!;
    private RagfairOfferService _ragfairOfferService = default!;
    private RagfairConfig _ragfairConfig = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private TemplateTable _templateTable = default!;
    private ItemHelper _itemHelper = default!;
    private PresetHelper _presetHelper = default!;
    private TimeUtil _timeUtil = default!;
    private ICloner _cloner = default!;
    private DatabaseMutationStamp _databaseMutationStamp = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _ragfairOfferGenerator = di.GetService<RagfairOfferGenerator>();
        _ragfairOfferService = di.GetService<RagfairOfferService>();
        _ragfairConfig = di.GetService<RagfairConfig>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();
        _templateTable = di.GetService<TemplateTable>();
        _itemHelper = di.GetService<ItemHelper>();
        _presetHelper = di.GetService<PresetHelper>();
        _timeUtil = di.GetService<TimeUtil>();
        _cloner = di.GetService<ICloner>();
        _databaseMutationStamp = di.GetService<DatabaseMutationStamp>();

        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = new MongoId() });
    }

    [Test]
    public void TheSameSeedGeneratesEquivalentOffersOnBothPaths(
        [ValueSource(nameof(_itemClasses))] string itemClass,
        [ValueSource(nameof(_seeds))] ulong seed
    )
    {
        var item = BuildItem(itemClass);

        var native = Generate(item, seed, forceLegacy: false, LootGenerationPath.Native);
        var legacy = Generate(item, seed, forceLegacy: true, LootGenerationPath.Legacy);

        LootJsonAssert.AssertEqual(legacy, native, $"itemClass={itemClass} tpl={item[0].Template}", seed);
    }

    /// <summary>
    /// The barter arm of <c>CreateSingleOfferForItem</c> is unreachable on shipped config
    /// (<c>barter.chancePercent</c> is 0), so <see cref="Generate"/> forces it. It is the one case
    /// that puts the whole price table's ordering under test end to end: <c>CreateBarterBarterScheme</c>
    /// filters <c>TemplateTable.Prices</c> to a price band and then index-draws over what survives,
    /// so C#'s dictionary order and the Rust <c>IndexMap</c> order have to agree across the entire
    /// table for the two paths to pick the same barter item.
    /// </summary>
    [Test]
    public void AForcedBarterOfferMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var item = BuildItem("expensive-barter-eligible");

        var native = Generate(item, seed, forceLegacy: false, LootGenerationPath.Native, ForcedBranch.Barter);
        var legacy = Generate(item, seed, forceLegacy: true, LootGenerationPath.Legacy, ForcedBranch.Barter);

        LootJsonAssert.AssertEqual(legacy, native, $"forced-barter tpl={item[0].Template}", seed);

        // Both fall-through arms of CreateBarterBarterScheme (price under
        // minRoubleCostToBecomeBarter, or no item inside the price band) produce a currency scheme,
        // which would leave the index draw this case exists for untested
        var requirementTpl = JsonNode.Parse(native)![0]!["requirements"]![0]!["_tpl"]!.GetValue<string>();
        Assert.That(
            Money.GetMoneyTpls().Select(tpl => tpl.ToString()),
            Does.Not.Contain(requirementTpl),
            $"seed={seed} fell through to a currency scheme, so the barter item draw never ran"
        );
    }

    /// <summary>
    /// The pack arm, likewise unreachable on shipped config (<c>pack.chancePercent</c> is 0.5), and
    /// likewise forced: it covers the pack stack-size draw and <c>CreateCurrencyBarterScheme</c>'s
    /// multiplier arm, neither of which any other case reaches.
    /// </summary>
    [Test]
    public void AForcedPackOfferMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var item = BuildItem("pack-eligible");

        var native = Generate(item, seed, forceLegacy: false, LootGenerationPath.Native, ForcedBranch.Pack);
        var legacy = Generate(item, seed, forceLegacy: true, LootGenerationPath.Legacy, ForcedBranch.Pack);

        LootJsonAssert.AssertEqual(legacy, native, $"forced-pack tpl={item[0].Template}", seed);

        // The pack arm is the only thing that sets it, so this is the proof the arm ran
        Assert.That(JsonNode.Parse(native)![0]!["sellInOnePiece"]!.GetValue<bool>(), Is.True, $"seed={seed} did not produce a pack offer");
    }

    /// <summary>
    /// Without this the parity cases could pass by both paths producing something seed-independent.
    /// </summary>
    [Test]
    public void ADifferentSeedProducesADifferentOffer()
    {
        var item = BuildItem("weapon-default-preset");

        var atSeed = Generate(item, 42, forceLegacy: false, LootGenerationPath.Native);
        var atSeedPlusOne = Generate(item, 43, forceLegacy: false, LootGenerationPath.Native);

        Assert.That(atSeedPlusOne, Is.Not.EqualTo(atSeed), "seed+1 produced an identical offer, so the seed is not reaching the draws");
    }

    /// <summary>
    /// Whole-pass parity is impossible (legacy's task fan-out is nondeterministic), so a full native
    /// pass is checked structurally instead: every offer well-formed, counts inside the configured
    /// bounds, end times in range.
    /// </summary>
    [Test]
    public void AFullNativePassProducesWellFormedOffersWithinTheConfiguredBounds()
    {
        var idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
        var now = _timeUtil.GetTimeStamp();
        List<MongoId> addedIds = [];

        try
        {
            _ragfairOfferGenerator.NativeTestSeed = 42;
            _ragfairOfferGenerator.GenerateDynamicOffers();

            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));

            var added = _ragfairOfferService.GetOffers().Where(offer => !idsBefore.Contains(offer.Id)).ToList();
            addedIds = added.Select(offer => offer.Id).ToList();

            Assert.That(added, Is.Not.Empty);
            Assert.Multiple(() =>
            {
                foreach (var offer in added)
                {
                    Assert.That(offer.Items, Is.Not.Empty, $"offer {offer.Id} has no items");
                    Assert.That(offer.Root, Is.EqualTo(offer.Items![0].Id), $"offer {offer.Id} root does not match its first item");
                    Assert.That(offer.Requirements, Is.Not.Empty, $"offer {offer.Id} has an empty barter scheme");
                    Assert.That(offer.SummaryCost, Is.GreaterThan(0), $"offer {offer.Id} is free");
                    Assert.That(offer.Quantity, Is.GreaterThan(0), $"offer {offer.Id} has no quantity");

                    // CreatedBy is [JsonIgnore], so it never crosses the wire - the frame reader
                    // stamps it, and the holder's per-template cap keys off this predicate
                    Assert.That(offer.IsFakePlayerOffer(), Is.True, $"offer {offer.Id} is not a fake-player offer");
                    Assert.That(
                        offer.EndTime,
                        Is.InRange(now + _ragfairConfig.Dynamic.EndTimeSeconds.Min, now + _ragfairConfig.Dynamic.EndTimeSeconds.Max + 60),
                        $"offer {offer.Id} expires outside the configured window"
                    );

                    // Every child's parent resolves inside its own offer
                    var ids = offer.Items!.Select(item => item.Id).ToHashSet();
                    foreach (var child in offer.Items!.Skip(1))
                    {
                        Assert.That(ids, Does.Contain(new MongoId(child.ParentId!)), $"offer {offer.Id} has an orphaned child");
                    }
                }
            });

            // The holder caps at GetOfferCountByBaseType per tpl, so the surviving count per tpl can
            // never exceed the configured max for that parent
            foreach (var group in added.GroupBy(offer => offer.Items![0].Template))
            {
                var parent = _templateTable.Items[group.Key].Parent;
                var bounds = _ragfairConfig.Dynamic.OfferItemCount.GetValueOrDefault(
                    parent.ToString(),
                    _ragfairConfig.Dynamic.OfferItemCount["default"]
                );

                Assert.That(group.Count(), Is.LessThanOrEqualTo(bounds.Max), $"tpl {group.Key} exceeded its configured offer cap");
            }
        }
        finally
        {
            foreach (var id in addedIds)
            {
                _ragfairOfferService.RemoveOfferById(id);
            }

            _ragfairOfferGenerator.NativeTestSeed = null;
        }
    }

    /// <summary>
    /// The native path numbers offers from the generator's live OfferCounter and advances it by the
    /// number it created - the same contract <c>CreateOffer:215</c> has on the legacy path.
    /// </summary>
    [Test]
    public void TheOfferCounterAdvancesPerOfferOnTheNativePath()
    {
        var counter = typeof(RagfairOfferGenerator).GetField("OfferCounter", BindingFlags.Instance | BindingFlags.NonPublic)!;
        var item = BuildItem("ammo");
        PurgeOffersForTemplate(item[0].Template);

        var before = (int)counter.GetValue(_ragfairOfferGenerator)!;
        var idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
        List<MongoId> addedIds = [];

        try
        {
            _ragfairOfferGenerator.NativeTestSeed = 42;
            _ragfairOfferGenerator.GenerateDynamicOffers([_cloner.Clone(item)!]);

            var added = _ragfairOfferService.GetOffers().Where(offer => !idsBefore.Contains(offer.Id)).ToList();
            addedIds = added.Select(offer => offer.Id).ToList();

            Assert.That(added, Has.Count.EqualTo(1));
            Assert.That(added[0].InternalId, Is.EqualTo(before));
            Assert.That((int)counter.GetValue(_ragfairOfferGenerator)!, Is.EqualTo(before + added.Count));
        }
        finally
        {
            foreach (var id in addedIds)
            {
                _ragfairOfferService.RemoveOfferById(id);
            }

            _ragfairOfferGenerator.NativeTestSeed = null;
        }
    }

    /// <summary>
    /// One regeneration pass over exactly one item, on one path, returning the normalized JSON of
    /// the offers it added to the holder.
    /// </summary>
    private string Generate(
        List<Item> itemWithChildren,
        ulong seed,
        bool forceLegacy,
        LootGenerationPath expected,
        ForcedBranch forcedBranch = ForcedBranch.None
    )
    {
        var originalForce = _ragfairConfig.ForceLegacyRagfairGeneration;
        var originalSource = _randomUtil.RandomSource;
        var originalProbabilitySource = ProbabilityRandomSource.Current;
        var originalBarterChance = _ragfairConfig.Dynamic.Barter.ChancePercent;
        var originalPackChance = _ragfairConfig.Dynamic.Pack.ChancePercent;
        List<MongoId> addedIds = [];

        // Must happen before the seed is installed: a stocked tpl makes AddOffer spend a seeded cap
        // draw at a different stream position on each path, and can reject the offer outright
        PurgeOffersForTemplate(itemWithChildren[0].Template);
        var idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();

        try
        {
            _ragfairConfig.ForceLegacyRagfairGeneration = forceLegacy;

            // RagfairPayloadProjection hands the live Dynamic object across per call, so one write
            // reaches both paths. The pack roll sits behind !isBarterOffer, so forcing barter also
            // takes the pack arm out - which is why the two branches are separate cases.
            if (forcedBranch == ForcedBranch.Barter)
            {
                _ragfairConfig.Dynamic.Barter.ChancePercent = 100;
            }
            else if (forcedBranch == ForcedBranch.Pack)
            {
                _ragfairConfig.Dynamic.Pack.ChancePercent = 100;
            }

            if (forceLegacy)
            {
                // One instance in both seams: one shared draw stream, mirroring the single
                // thread-local the Rust side installs for testSeed.
                var seeded = new SeededRandomSource(seed);
                _randomUtil.RandomSource = seeded;
                ProbabilityRandomSource.Current = seeded;
            }
            else
            {
                _ragfairOfferGenerator.NativeTestSeed = seed;
            }

            // Every write above lands in the projected invariant slice, and config is not one of the
            // instrumented mutation paths - so this fixture stamps its own writes, which also keeps
            // each pass on a full slice send exactly as it was before the cache existed
            _databaseMutationStamp.Bump();

            _ragfairOfferGenerator.GenerateDynamicOffers([_cloner.Clone(itemWithChildren)!]);

            // Fail fast on silent fallback before comparing anything.
            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(expected), $"generation did not take the {expected} path");

            var added = _ragfairOfferService.GetOffers().Where(offer => !idsBefore.Contains(offer.Id)).ToList();
            addedIds = added.Select(offer => offer.Id).ToList();

            // An expired-offer pass is a like-for-like replacement: exactly one offer, always
            Assert.That(added, Has.Count.EqualTo(1), $"{expected} path produced {added.Count} offers, expected 1");

            return LootIdNormalizer.Normalize(Canonicalise(_jsonUtil.Serialize(added)!));
        }
        finally
        {
            foreach (var id in addedIds)
            {
                _ragfairOfferService.RemoveOfferById(id);
            }

            _ragfairConfig.ForceLegacyRagfairGeneration = originalForce;
            _ragfairConfig.Dynamic.Barter.ChancePercent = originalBarterChance;
            _ragfairConfig.Dynamic.Pack.ChancePercent = originalPackChance;
            _randomUtil.RandomSource = originalSource;
            ProbabilityRandomSource.Current = originalProbabilitySource;
            _ragfairOfferGenerator.NativeTestSeed = null;
            // the config restores above are slice writes too
            _databaseMutationStamp.Bump();
        }
    }

    private enum ForcedBranch
    {
        None,
        Barter,
        Pack,
    }

    /// <summary>
    /// Empties the holder of every fake-player offer whose root item is <paramref name="tpl"/>, so
    /// the next <c>AddOffer</c> for it finds no <c>_fakePlayerOffers</c> entry and spends no cap
    /// draw - removing the last one drops the key. Trader and player offers for the tpl are left
    /// alone: the cap never reads them. The flea the shared container pre-generated is not restored
    /// afterwards - it is test-session state, and every fixture that reads the holder reads whatever
    /// the run before it left.
    /// </summary>
    private void PurgeOffersForTemplate(MongoId tpl)
    {
        var offerIds =
            _ragfairOfferService.GetOffersOfType(tpl)?.Where(offer => offer.IsFakePlayerOffer()).Select(offer => offer.Id).ToList() ?? [];
        foreach (var offerId in offerIds)
        {
            _ragfairOfferService.RemoveOfferById(offerId);
        }
    }

    /// <summary>
    /// Strips the two members that cannot match by construction, and only those:
    /// <list type="bullet">
    /// <item>
    /// <c>intId</c> - <c>OfferCounter</c> is process state, not generated content. Both paths
    /// advance it identically, they just start from different values.
    /// </item>
    /// <item>
    /// <c>startTime</c> - legacy re-reads the clock per offer (<c>:626</c>), the native path takes
    /// one timestamp for the batch, and the two runs happen at different moments. The seeded part is
    /// the spread <c>GetOfferEndTime</c> draws, so <c>endTime</c> is rewritten to that spread
    /// (<c>time</c> is a whole number of seconds, so <c>Math.Round(time + spread) - time</c> is
    /// exactly <c>Math.Round(spread)</c>) and the absolute start is dropped.
    /// </item>
    /// </list>
    /// The seller id is a fresh <c>MongoId</c> with no other reference in the document, so
    /// <see cref="LootIdNormalizer"/> - which only maps values it saw under an <c>_id</c> - cannot
    /// reach it; it gets a positional placeholder here instead. <c>user.aid</c>, <c>user.nickname</c>
    /// and <c>user.rating</c> are seeded draws and are deliberately left alone.
    /// </summary>
    /// <summary>
    /// Drops the sanctioned per-run gaps so two offer documents compare: the live <c>intId</c>
    /// counter, the wall-clock <c>startTime</c> (folded into <c>endTime</c> as a duration) and the
    /// freshly minted seller id. Shared with <see cref="RagfairNativeSliceCacheTests"/>, which needs
    /// the same normalisation to compare a full slice send against a cache hit.
    /// </summary>
    internal static string Canonicalise(string json)
    {
        var array = JsonNode.Parse(json)!.AsArray();
        for (var index = 0; index < array.Count; index++)
        {
            var offer = array[index]!.AsObject();

            offer.Remove("intId");

            var startTime = offer["startTime"]!.GetValue<long>();
            offer["endTime"] = offer["endTime"]!.GetValue<long>() - startTime;
            offer.Remove("startTime");

            offer["user"]!.AsObject()["id"] = $"seller-{index}";
        }

        return array.ToJsonString();
    }

    /// <summary>
    /// One assort-shaped item-with-children per class, built the way <c>RagfairAssortGenerator</c>
    /// would (<c>:79-92</c> for presets, <c>:141-156</c> for plain items) so the expired path sees
    /// exactly what a real regeneration pass hands it.
    /// </summary>
    private List<Item> BuildItem(string itemClass)
    {
        if (itemClass == "weapon-default-preset")
        {
            var preset = _presetHelper
                .GetDefaultPresets()
                .Values.First(candidate => _itemHelper.IsOfBaseclass(candidate.Items[0].Template, BaseClasses.WEAPON));
            var clone = _cloner.Clone(preset.Items)!.ReplaceIDs().ToList();
            clone.RemapRootItemId();
            clone[0].ParentId = "hideout";
            clone[0].SlotId = "hideout";
            clone[0].Upd = new Upd
            {
                StackObjectsCount = 99999999,
                UnlimitedCount = true,
                SptPresetId = preset.Id,
            };

            return clone;
        }

        var tpl = itemClass switch
        {
            // Resolved by predicate, not by literal, so a data change cannot silently make a case vacuous
            "armor-with-removable-plates" => FirstTplWhere(template => _itemHelper.ArmorItemHasRemovablePlateSlots(template.Id)),
            "ammo" => FirstTplWhere(template => template.Parent == BaseClasses.AMMO),
            "pack-eligible" => FirstTplWhere(template => _ragfairConfig.Dynamic.Pack.ItemTypeWhitelist.Contains(template.Parent)),
            "money" => Money.DOLLARS,
            "plain-barter-eligible" => FirstTplWhere(template =>
                _itemHelper.IsOfBaseclass(template.Id, BaseClasses.BARTER_ITEM) && _templateTable.Prices.ContainsKey(template.Id)
            ),
            // Same class, but priced far enough above barter.minRoubleCostToBecomeBarter that the
            // 0.8-1.2 dynamic price multiplier can never drop it under and send the forced-barter
            // case down the currency fall-through instead of the item draw it exists to test
            "expensive-barter-eligible" => FirstTplWhere(template =>
                _itemHelper.IsOfBaseclass(template.Id, BaseClasses.BARTER_ITEM)
                && _templateTable.Prices.GetValueOrDefault(template.Id) >= 250_000
            ),
            _ => throw new ArgumentOutOfRangeException(nameof(itemClass), itemClass, "no case defined"),
        };

        return
        [
            // tpl and id must be the same, as RagfairAssortGenerator.CreateRagfairAssortRootItem does
            new Item
            {
                Id = tpl,
                Template = tpl,
                ParentId = "hideout",
                SlotId = "hideout",
                Upd = new Upd { StackObjectsCount = 99999999, UnlimitedCount = true },
            },
        ];
    }

    private MongoId FirstTplWhere(Func<TemplateItem, bool> predicate)
    {
        var match = _templateTable.Items.Values.FirstOrDefault(template =>
            string.Equals(template.Type, "Item", StringComparison.OrdinalIgnoreCase)
            && template.Properties is not null
            && predicate(template)
        );

        Assert.That(match, Is.Not.Null, "no template in the live database matches this item class");

        return match!.Id;
    }
}
