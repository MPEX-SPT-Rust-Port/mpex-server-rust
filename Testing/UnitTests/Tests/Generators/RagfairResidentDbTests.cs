using System.Reflection;
using System.Text;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Ragfair;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the resident-DB epoch protocol on the ragfair native path: an eligible generator names an
/// epoch and never sends the views override, an unmoved stamp skips the republish, a bump forces
/// one, the kill switch and untrusted mods fall back to the override, a construction without the
/// overload always overrides, and a native-side epoch desync self-heals through one republish plus
/// retry. Epochs are process-global (other fixtures publish too), so every assertion is relative.
/// Mutates the shared config singleton and the live offer holder, so it restores both and never
/// runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairResidentDbTests
{
    private RagfairOfferGenerator _generator = default!;
    private RagfairOfferService _offerService = default!;
    private RagfairConfig _ragfairConfig = default!;
    private DatabaseMutationStamp _stamp = default!;
    private DbPublisher _publisher = default!;
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
        _publisher = di.GetService<DbPublisher>();
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
    /// The first pass after a stamp movement can bump the stamp again through the
    /// <c>RejectedCanSellTemplates</c> replay (a real flip is a database write). Two passes settle
    /// it: by the second, every rejected template is already false and no further bump is owed, so
    /// the publisher's remembered stamp matches the live one.
    /// </summary>
    private void SettleStampAndPublish()
    {
        _generator.GenerateDynamicOffers();
        ClearOffers();
        _generator.GenerateDynamicOffers();
        ClearOffers();
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
    public void ASecondUnchangedPassSkipsTheRepublishAndGeneratesTheSameOffers()
    {
        _stamp.Bump(); // force a republish first
        SettleStampAndPublish();

        GenerateWithASeededHolder();
        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False, "an eligible generator must not send the override");
        var firstPass = OfferSignatures();
        var epochAfterFirst = _publisher.EnsureCurrent();

        ClearOffers();
        GenerateWithASeededHolder();
        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False);
        var secondPass = OfferSignatures();

        Assert.That(_publisher.EnsureCurrent(), Is.EqualTo(epochAfterFirst), "an unmoved stamp must not republish");
        Assert.That(secondPass, Is.EqualTo(firstPass));
    }

    /// <summary>
    /// The native pass itself is seeded, but the holder is not: <c>AddOffer</c> spends an unseeded
    /// per-template cap draw (<c>RagfairOfferHolder.cs:153-163</c>) that decides which offers
    /// survive, so two passes only compare if that draw runs off a fresh fixed stream too.
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
    /// The signature above is coarse by construction - a diverged view that moved a durability roll
    /// or an <c>Upd</c> field would project identically through it. This is the fine-grained pin,
    /// and the flip's whole promise in one process: the same item through the expired-offer vehicle
    /// (the one <see cref="RagfairParityTests"/> uses, and what <c>RagfairServer.cs:79</c> calls),
    /// generated once off the natively-derived resident views and once off the C#-built override,
    /// compared as normalized JSON down to every field.
    /// </summary>
    [Test]
    public void AResidentSendAndAnOverrideSendProduceAnIdenticalOfferFieldForField()
    {
        var item = BuildSingleItem();

        var resident = GenerateOneOffer(item);
        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False);

        _ragfairConfig.DisableNativeRequestCache = true;
        string overrideSend;
        try
        {
            overrideSend = GenerateOneOffer(item);
        }
        finally
        {
            _ragfairConfig.DisableNativeRequestCache = false;
        }
        Assert.That(_generator.LastSendIncludedViewsOverride, Is.True);

        LootJsonAssert.AssertEqual(resident, overrideSend, "resident send vs views-override send", 424242);
    }

    [Test]
    public void ABumpForcesARepublish()
    {
        SettleStampAndPublish();
        var epochBefore = _publisher.EnsureCurrent();

        _stamp.Bump();
        _generator.GenerateDynamicOffers();

        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False);
        Assert.That(_publisher.EnsureCurrent(), Is.GreaterThan(epochBefore), "a bumped stamp must republish");
    }

    [Test]
    public void TheKillSwitchAlwaysSendsTheOverride()
    {
        _ragfairConfig.DisableNativeRequestCache = true;
        try
        {
            _generator.GenerateDynamicOffers();
            Assert.That(_generator.LastSendIncludedViewsOverride, Is.True);
            ClearOffers();
            _generator.GenerateDynamicOffers();

            Assert.That(_generator.LastSendIncludedViewsOverride, Is.True);
        }
        finally
        {
            _ragfairConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ANativeSideEpochDesyncSelfHealsThroughOneRetry()
    {
        SettleStampAndPublish();

        // Desync: a direct native publish the publisher never sees moves the resident epoch out
        // from under the epoch it remembers
        SptNative.DbPublish(Encoding.UTF8.GetBytes("{\"schema\":1,\"roots\":{}}"));

        _generator.GenerateDynamicOffers();

        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False, "the stale-epoch miss should have republished and retried");
        Assert.That(_offerService.GetOffers(), Is.Not.Empty);
    }

    [Test]
    public void AGeneratorBuiltOnTheFrozenConstructorAlwaysSendsTheOverride()
    {
        var di = DI.GetInstance();
        var frozen = BuildWithFrozenConstructor(di);
        frozen.NativeTestSeed = 424242;

        frozen.GenerateDynamicOffers();
        ClearOffers();
        frozen.GenerateDynamicOffers();

        Assert.That(frozen.LastSendIncludedViewsOverride, Is.True, "no publisher means no residency eligibility");
    }

    [Test]
    public void AGeneratorWithModsLoadedAlwaysSendsTheOverride()
    {
        var di = DI.GetInstance();
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = BuildWithOverloadConstructor(di, new SptMod[] { null! });
        modded.NativeTestSeed = 424242;

        modded.GenerateDynamicOffers();
        ClearOffers();
        modded.GenerateDynamicOffers();

        Assert.That(modded.LastSendIncludedViewsOverride, Is.True, "a loaded mod without the trust flag disables residency");
    }

    [Test]
    public void TheTrustFlagKeepsTheResidentPathLiveWithModsLoaded()
    {
        var di = DI.GetInstance();
        var modded = BuildWithOverloadConstructor(di, new SptMod[] { null! });
        modded.NativeTestSeed = 424242;

        _ragfairConfig.TrustNativeRequestCacheWithMods = true;
        try
        {
            modded.GenerateDynamicOffers();
            ClearOffers();
            modded.GenerateDynamicOffers();

            Assert.That(
                modded.LastSendIncludedViewsOverride,
                Is.False,
                "the trust flag should keep the resident path live despite the mod"
            );
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
