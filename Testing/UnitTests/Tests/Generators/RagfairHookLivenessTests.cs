using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Ragfair;
using SPTarkov.Server.Core.Helpers.Ragfair;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Ragfair;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Ragfair;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the mod hook contract across all four classes the native ragfair path replaces: a Harmony
/// patch on a frozen 4.1.2 member of any of them must route generation to the legacy path, because
/// that is the only body the patch can hook. A patch on the dispatcher itself is the exception, it
/// wraps whichever path runs. Harmony patches are process-wide, so every patch is removed in a
/// finally and the fixture never runs in parallel with others.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairHookLivenessTests
{
    private static bool _patchFired;
    private static bool _prefixFired;
    private static bool _postfixFired;

    private RagfairOfferGenerator _ragfairOfferGenerator = default!;
    private RagfairOfferService _ragfairOfferService = default!;
    private TemplateTable _templateTable = default!;

    private HashSet<MongoId> _idsBefore = [];

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _ragfairOfferGenerator = di.GetService<RagfairOfferGenerator>();
        _ragfairOfferService = di.GetService<RagfairOfferService>();
        _templateTable = di.GetService<TemplateTable>();

        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = new MongoId() });
    }

    [SetUp]
    public void SetUp()
    {
        _idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
    }

    /// <summary>
    /// A public member of the ported class that neither path's dynamic-offer pass ever calls - it is
    /// here to prove the hookable set was not narrowed to the dispatcher's callees.
    /// </summary>
    [Test]
    public void HarmonyPatchOnGenerateFleaOffersForTraderForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(
            typeof(RagfairOfferGenerator),
            nameof(RagfairOfferGenerator.GenerateFleaOffersForTrader),
            expectFired: false
        );
    }

    /// <summary>
    /// The dead-but-frozen member (:319): nothing calls it, and it still has to flip the path.
    /// </summary>
    [Test]
    public void HarmonyPatchOnGetRatingForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(RagfairOfferGenerator), "GetRating", expectFired: false);
    }

    [Test]
    public void HarmonyPatchOnRagfairPriceServiceFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(RagfairPriceService), nameof(RagfairPriceService.GetDynamicItemPrice), expectFired: true);
    }

    [Test]
    public void HarmonyPatchOnRagfairServerHelperFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(RagfairServerHelper), nameof(RagfairServerHelper.CalculateDynamicStackCount), expectFired: true);
    }

    /// <summary>
    /// Only a full pass calls this one (:434), and the fixture runs the cheap expired-offer pass, so
    /// the live patch is proven by the flip alone.
    /// </summary>
    [Test]
    public void HarmonyPatchOnRagfairAssortGeneratorForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(
            typeof(RagfairAssortGenerator),
            nameof(RagfairAssortGenerator.GenerateRagfairAssortItems),
            expectFired: false
        );
    }

    /// <summary>
    /// The dispatcher is deliberately not in the hookable set: a patch on it wraps whichever path
    /// runs, so it keeps the native body and still sees the call.
    /// </summary>
    [Test]
    public void HarmonyPatchOnGenerateDynamicOffersWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.ragfair-hook-liveness.dispatcher");
        var target = AccessTools.Method(typeof(RagfairOfferGenerator), nameof(RagfairOfferGenerator.GenerateDynamicOffers));
        Assert.That(target, Is.Not.Null, "GenerateDynamicOffers not found");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(RagfairHookLivenessTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(RagfairHookLivenessTests), nameof(Postfix))
            );

            Generate();

            Assert.That(
                _ragfairOfferGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Native),
                "a patch on the dispatcher forced legacy"
            );
            Assert.That(AddedOffers(), Is.Not.Empty);
            Assert.That(_prefixFired, Is.True, "prefix on GenerateDynamicOffers never ran");
            Assert.That(_postfixFired, Is.True, "postfix on GenerateDynamicOffers never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
            PurgeAddedOffers();
        }
    }

    /// <summary>
    /// The hookable set is built by reflection, so a rename or a visibility change would silently
    /// shrink it. Pin its shape instead of its exact size.
    /// </summary>
    [Test]
    public void TheHookableMemberSetCoversAllFourClassesAndExcludesOnlyTheDispatcher()
    {
        var members =
            (List<MethodBase>)
                typeof(RagfairOfferGenerator).GetField("_hookableMembers", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

        Assert.That(members, Is.Not.Empty);
        Assert.Multiple(() =>
        {
            foreach (
                var type in new[]
                {
                    typeof(RagfairOfferGenerator),
                    typeof(RagfairPriceService),
                    typeof(RagfairServerHelper),
                    typeof(RagfairAssortGenerator),
                }
            )
            {
                Assert.That(members.Any(member => member.DeclaringType == type), $"no hookable members found on {type.Name}");
            }

            Assert.That(
                members.Select(member => member.DeclaringType).Distinct().Count(),
                Is.EqualTo(4),
                "the hookable set spans types other than the four the native path folds in"
            );
            Assert.That(
                members.Any(member => member.Name == nameof(RagfairOfferGenerator.GenerateDynamicOffers)),
                Is.False,
                "the dispatcher must not be in its own hookable set"
            );
            Assert.That(members.Any(member => member.IsSpecialName), Is.False, "property accessors leaked into the hookable set");
            Assert.That(members.Any(member => member.Name == "GetRating"), "the dead-but-frozen GetRating fell out of the hookable set");
            Assert.That(
                members.Any(member => member.Name == "GetAvatarUrl"),
                "the dead-but-frozen GetAvatarUrl fell out of the hookable set"
            );
        });
    }

    private void AssertPatchForcesLegacyPath(Type declaringType, string methodName, bool expectFired)
    {
        var harmony = new Harmony($"unit-tests.ragfair-hook-liveness.{declaringType.Name}.{methodName}");
        var target = AccessTools.Method(declaringType, methodName);
        Assert.That(target, Is.Not.Null, $"frozen member {declaringType.Name}.{methodName} not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(RagfairHookLivenessTests), nameof(PatchFired)));

            // Members no pass reaches can never set _patchFired, so this is what proves the patch is
            // actually live rather than a silently failed install
            Assert.That(
                Harmony.GetPatchInfo(target)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {declaringType.Name}.{methodName} was not registered"
            );

            Generate();

            Assert.That(
                _ragfairOfferGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Legacy),
                $"a patch on {declaringType.Name}.{methodName} did not force the legacy path"
            );
            Assert.That(AddedOffers(), Is.Not.Empty);

            if (expectFired)
            {
                Assert.That(_patchFired, Is.True, $"postfix on {declaringType.Name}.{methodName} never ran on the legacy path");
            }
        }
        finally
        {
            harmony.UnpatchSelf();
            PurgeAddedOffers();
        }
    }

    /// <summary>
    /// One expired-offer regeneration pass over a single assort-shaped item - the cheap vehicle, and
    /// the one <c>RagfairServer.cs:79</c> uses.
    /// </summary>
    private void Generate()
    {
        var item = BuildSingleItem();

        // A tpl the pre-generated flea already stocks can be rejected outright by the holder's
        // per-template cap (RagfairOfferHolder.cs:153-163), which would make "offers were added"
        // fail for reasons that have nothing to do with the patch
        PurgeFakePlayerOffersForTemplate(item[0].Template);

        _ragfairOfferGenerator.GenerateDynamicOffers([item]);
    }

    private List<RagfairOffer> AddedOffers()
    {
        return _ragfairOfferService.GetOffers().Where(offer => !_idsBefore.Contains(offer.Id)).ToList();
    }

    /// <summary>
    /// Offers left behind would make the next case's holder spend a per-template cap draw
    /// (RagfairOfferHolder.cs:153-163) that only one of the two paths pays for.
    /// </summary>
    private void PurgeAddedOffers()
    {
        foreach (var offer in AddedOffers())
        {
            _ragfairOfferService.RemoveOfferById(offer.Id);
        }
    }

    /// <summary>
    /// Empties the holder of every fake-player offer whose root item is <paramref name="tpl"/>, so
    /// the next AddOffer for it finds no cap to hit. Trader and player offers are left alone - the
    /// cap never reads them. Not restored afterwards: it is test-session state.
    /// </summary>
    private void PurgeFakePlayerOffersForTemplate(MongoId tpl)
    {
        var offerIds =
            _ragfairOfferService.GetOffersOfType(tpl)?.Where(offer => offer.IsFakePlayerOffer()).Select(offer => offer.Id).ToList() ?? [];
        foreach (var offerId in offerIds)
        {
            _ragfairOfferService.RemoveOfferById(offerId);
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

    private static void PatchFired()
    {
        _patchFired = true;
    }

    private static void Prefix()
    {
        _prefixFired = true;
    }

    private static void Postfix()
    {
        _postfixFired = true;
    }
}
