using System.Reflection;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Ragfair;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Traders;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Services;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the stamp-gated native slice cache: a hit skips the slice, a bump resends it, the kill
/// switch disables it, a construction without the overload never caches, and a native-side
/// desync self-heals through one retry. Mutates the shared config singleton and the live offer
/// holder, so it restores both and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairNativeSliceCacheTests
{
    private RagfairOfferGenerator _generator = default!;
    private RagfairOfferService _offerService = default!;
    private RagfairConfig _ragfairConfig = default!;
    private DatabaseMutationStamp _stamp = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private TemplateTable _templateTable = default!;
    private ICloner _cloner = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _generator = di.GetService<RagfairOfferGenerator>();
        _offerService = di.GetService<RagfairOfferService>();
        _ragfairConfig = di.GetService<RagfairConfig>();
        _stamp = di.GetService<DatabaseMutationStamp>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();
        _templateTable = di.GetService<TemplateTable>();
        _cloner = di.GetService<ICloner>();
        _generator.NativeTestSeed = 424242;
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _generator.NativeTestSeed = null;
        _ragfairConfig.DisableNativeRequestCache = false;
        // A full pass leaves tens of thousands of offers behind, and the holder's per-base-type cap
        // reads what is already in it - a saturated holder would reject the single offer the next
        // fixture's cases expect
        ClearOffers();
        // leave the shared container fresher than we found it for whatever fixture runs next
        _stamp.Bump();
    }

    [SetUp]
    public void SetUp()
    {
        ClearOffers();
    }

    private void ClearOffers()
    {
        foreach (var offer in _offerService.GetOffers().ToList())
        {
            _offerService.RemoveOfferById(offer.Id);
        }
    }

    /// <summary>
    /// Everything seeded draws decide about an offer, ignoring the sanctioned per-run gaps
    /// (minted MongoIds, the intId counter, wall-clock start/end times).
    /// </summary>
    private List<string> OfferSignatures()
    {
        return _offerService
            .GetOffers()
            .OrderBy(offer => offer.InternalId)
            .Select(offer =>
                string.Join(
                    '|',
                    offer.Items!.First().Template.ToString(),
                    offer.Items!.Count.ToString(),
                    string.Join(',', offer.Requirements!.Select(req => $"{req.TemplateId}:{req.Count}")),
                    offer.SellInOnePiece.ToString(),
                    (offer.EndTime - offer.StartTime).ToString()
                )
            )
            .ToList();
    }

    [Test]
    public void ASecondUnchangedPassSkipsTheSliceAndGeneratesTheSameOffers()
    {
        _stamp.Bump(); // force a full send first
        GenerateWithASeededHolder();
        Assert.That(_generator.LastSendIncludedSlice, Is.True);
        var fullSend = OfferSignatures();

        ClearOffers();
        GenerateWithASeededHolder();
        Assert.That(_generator.LastSendIncludedSlice, Is.False, "the unchanged stamp should have hit the cache");
        var cacheHit = OfferSignatures();

        Assert.That(cacheHit, Is.EqualTo(fullSend));
    }

    /// <summary>
    /// The native pass itself is seeded, but the holder is not: <c>AddOffer</c> spends an unseeded
    /// per-template cap draw (<c>RagfairOfferHolder.cs:153-163</c>) that decides which offers
    /// survive, so the two passes only compare if that draw runs off a fresh fixed stream too.
    /// </summary>
    private void GenerateWithASeededHolder()
    {
        var original = _randomUtil.RandomSource;
        try
        {
            _randomUtil.RandomSource = new SeededRandomSource(424242);
            _generator.GenerateDynamicOffers();
        }
        finally
        {
            _randomUtil.RandomSource = original;
        }
    }

    /// <summary>
    /// The signature above is coarse by construction - a stale slice that moved a durability roll or
    /// an <c>Upd</c> field would project identically through it. This is the fine-grained pin: one
    /// item through the expired-offer vehicle (the same one <see cref="RagfairParityTests"/> uses, and
    /// what <c>RagfairServer.cs:79</c> calls), full slice send versus cache hit, compared as
    /// normalized JSON down to every field. A whole pass cannot be compared this way - tens of
    /// thousands of offers is gigabytes of <c>JsonNode</c>.
    /// </summary>
    [Test]
    public void ACacheHitProducesAnIdenticalOfferFieldForField()
    {
        var item = BuildSingleItem();

        _stamp.Bump(); // force a full send first
        var fullSend = GenerateOneOffer(item);
        Assert.That(_generator.LastSendIncludedSlice, Is.True);

        var cacheHit = GenerateOneOffer(item);
        Assert.That(_generator.LastSendIncludedSlice, Is.False, "the unchanged stamp should have hit the cache");

        LootJsonAssert.AssertEqual(fullSend, cacheHit, "slice-cache full send vs hit", 424242);
    }

    [Test]
    public void ABumpForcesTheSliceToBeResent()
    {
        _generator.GenerateDynamicOffers();
        _stamp.Bump();

        ClearOffers();
        _generator.GenerateDynamicOffers();

        Assert.That(_generator.LastSendIncludedSlice, Is.True);
    }

    [Test]
    public void TheKillSwitchAlwaysSendsTheSlice()
    {
        _ragfairConfig.DisableNativeRequestCache = true;
        try
        {
            _generator.GenerateDynamicOffers();
            ClearOffers();
            _generator.GenerateDynamicOffers();

            Assert.That(_generator.LastSendIncludedSlice, Is.True);
        }
        finally
        {
            _ragfairConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ANativeSideDesyncSelfHealsThroughOneRetry()
    {
        // Desync: park a slice under a stamp the generator will never claim, then lie to the
        // generator that its current stamp was already sent
        var di = DI.GetInstance();
        ParkForeignSlice(di);
        _generator.LastSentSliceStamp = _stamp.Current;

        _generator.GenerateDynamicOffers();

        Assert.That(_generator.LastSendIncludedSlice, Is.True, "the stale-slice miss should have retried with the slice");
        Assert.That(_offerService.GetOffers(), Is.Not.Empty);
    }

    [Test]
    public void AGeneratorBuiltOnTheFrozenConstructorNeverCaches()
    {
        var di = DI.GetInstance();
        var frozen = BuildWithFrozenConstructor(di);
        frozen.NativeTestSeed = 424242;

        frozen.GenerateDynamicOffers();
        ClearOffers();
        frozen.GenerateDynamicOffers();

        Assert.That(frozen.LastSendIncludedSlice, Is.True, "no stamp service means no cache eligibility");
    }

    [Test]
    public void AGeneratorWithModsLoadedAlwaysSendsTheSlice()
    {
        var di = DI.GetInstance();
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = BuildWithOverloadConstructor(di, new SptMod[] { null! });
        modded.NativeTestSeed = 424242;

        modded.GenerateDynamicOffers();
        ClearOffers();
        modded.GenerateDynamicOffers();

        Assert.That(modded.LastSendIncludedSlice, Is.True, "a loaded mod without the trust flag disables the cache");
    }

    [Test]
    public void TheTrustFlagKeepsTheCacheLiveWithModsLoaded()
    {
        var di = DI.GetInstance();
        var modded = BuildWithOverloadConstructor(di, new SptMod[] { null! });
        modded.NativeTestSeed = 424242;

        // Not a slice input, so no bump is owed on the way back out
        _ragfairConfig.TrustNativeRequestCacheWithMods = true;
        try
        {
            _stamp.Bump(); // force a full send first
            modded.GenerateDynamicOffers();
            ClearOffers();
            modded.GenerateDynamicOffers();

            Assert.That(modded.LastSendIncludedSlice, Is.False, "the trust flag should keep the cache live despite the mod");
        }
        finally
        {
            _ragfairConfig.TrustNativeRequestCacheWithMods = false;
        }
    }

    /// <summary>
    /// One expired-offer regeneration pass, normalized for comparison. The holder is empty (SetUp
    /// cleared it) and the offer is removed again afterwards, so neither pass pays a per-template
    /// cap draw the other does not.
    /// </summary>
    private string GenerateOneOffer(List<Item> item)
    {
        var idsBefore = _offerService.GetOffers().Select(offer => offer.Id).ToHashSet();
        var originalSource = _randomUtil.RandomSource;
        List<MongoId> addedIds = [];

        try
        {
            _randomUtil.RandomSource = new SeededRandomSource(424242);
            // The pass consumes the list it is handed, so each run gets its own copy
            _generator.GenerateDynamicOffers([_cloner.Clone(item)!]);

            var added = _offerService.GetOffers().Where(offer => !idsBefore.Contains(offer.Id)).ToList();
            addedIds = added.Select(offer => offer.Id).ToList();

            Assert.That(added, Has.Count.EqualTo(1), $"an expired-offer pass produced {added.Count} offers, expected 1");

            return LootIdNormalizer.Normalize(RagfairParityTests.Canonicalise(_jsonUtil.Serialize(added)!));
        }
        finally
        {
            foreach (var id in addedIds)
            {
                _offerService.RemoveOfferById(id);
            }

            _randomUtil.RandomSource = originalSource;
        }
    }

    /// <summary>
    /// One assort-shaped row, exactly as RagfairAssortGenerator.CreateRagfairAssortRootItem builds
    /// it (:126-141) - id and tpl deliberately identical.
    /// </summary>
    private List<Item> BuildSingleItem()
    {
        var tpl = _templateTable
            .Items.Values.First(template =>
                string.Equals(template.Type, "Item", StringComparison.OrdinalIgnoreCase)
                && template.Properties?.CanSellOnRagfair == true
                && _templateTable.Prices.ContainsKey(template.Id)
            )
            .Id;

        return
        [
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

    /// <summary>
    /// Leaves the native slice cache holding a stamp no generator will ever name, so the next
    /// slice-less request for any other stamp misses and reports stale.
    /// </summary>
    private static void ParkForeignSlice(DI di)
    {
        var slice = RagfairPayloadProjection.BuildInvariantSlice(
            di.GetService<TemplateTable>(),
            di.GetService<HandbookHelper>(),
            di.GetService<TraderHelper>(),
            di.GetService<PresetHelper>(),
            di.GetService<ItemFilterService>(),
            di.GetService<SeasonalEventService>(),
            di.GetService<BotTable>(),
            di.GetService<ItemHelper>(),
            di.GetService<BotConfig>(),
            di.GetService<RagfairConfig>()
        );

        var request = RagfairPayloadProjection.BuildRequest(
            slice,
            long.MaxValue - 1,
            null,
            di.GetService<TimeUtil>().GetTimeStamp(),
            0,
            424242
        );

        SptNative.GenerateDynamicOffers(request);
    }

    private static RagfairOfferGenerator BuildWithFrozenConstructor(DI di)
    {
        return Build(di, FindConstructor(smallest: true), null);
    }

    private static RagfairOfferGenerator BuildWithOverloadConstructor(DI di, IReadOnlyList<SptMod> mods)
    {
        return Build(di, FindConstructor(smallest: false), mods);
    }

    private static ConstructorInfo FindConstructor(bool smallest)
    {
        var constructors = typeof(RagfairOfferGenerator).GetConstructors().OrderBy(ctor => ctor.GetParameters().Length).ToList();

        return smallest ? constructors.First() : constructors.Last();
    }

    private static RagfairOfferGenerator Build(DI di, ConstructorInfo constructor, IReadOnlyList<SptMod>? mods)
    {
        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
            {
                if (mods is not null && parameter.ParameterType == typeof(IReadOnlyList<SptMod>))
                {
                    return mods;
                }

                return di.GetService(parameter.ParameterType);
            })
            .ToArray();

        return (RagfairOfferGenerator)constructor.Invoke(arguments);
    }
}
