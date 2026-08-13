using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Ragfair;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Ragfair;
using SPTarkov.Server.Core.Helpers.Traders;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Ragfair;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Commerce;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the reasons dynamic offer generation falls back to the retained 4.1.2 C# implementation that
/// are not Harmony patches (those are <see cref="RagfairHookLivenessTests"/>): the config flag and a
/// mod-substituted collaborator. Plus the one database write the native path has to replay back into
/// the live template table. Mutates the shared config singleton and the live offer holder, so it
/// restores what it can and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairPathDispatchTests
{
    private RagfairOfferGenerator _ragfairOfferGenerator = default!;
    private RagfairOfferService _ragfairOfferService = default!;
    private RagfairConfig _ragfairConfig = default!;
    private TemplateTable _templateTable = default!;

    private HashSet<MongoId> _idsBefore = [];

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _ragfairOfferGenerator = di.GetService<RagfairOfferGenerator>();
        _ragfairOfferService = di.GetService<RagfairOfferService>();
        _ragfairConfig = di.GetService<RagfairConfig>();
        _templateTable = di.GetService<TemplateTable>();

        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = new MongoId() });
    }

    [SetUp]
    public void SetUp()
    {
        _idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
    }

    /// <summary>
    /// The negative control: a stock container, no force flag and no patches take the native path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        try
        {
            Generate(_ragfairOfferGenerator);

            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
            Assert.That(AddedOffers(), Is.Not.Empty, "the native path added no offers");
        }
        finally
        {
            PurgeAddedOffers();
        }
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        var original = _ragfairConfig.ForceLegacyRagfairGeneration;
        try
        {
            _ragfairConfig.ForceLegacyRagfairGeneration = true;

            Generate(_ragfairOfferGenerator);

            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(AddedOffers(), Is.Not.Empty, "the legacy path added no offers");
        }
        finally
        {
            _ragfairConfig.ForceLegacyRagfairGeneration = original;
            PurgeAddedOffers();
        }
    }

    /// <summary>
    /// The negative control for the three substitution cases below: hand-building the generator off
    /// the container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltGeneratorWithStockServicesTakesTheNativePath()
    {
        var generator = BuildGenerator();
        try
        {
            Generate(generator);

            Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        }
        finally
        {
            PurgeAddedOffers();
        }
    }

    /// <summary>
    /// A mod registering its own RagfairPriceService with a higher TypePriority hands the container a
    /// subclass, whose overrides only the C# path can run.
    /// </summary>
    [Test]
    public void AReplacedPriceServiceRoutesToTheLegacyPath()
    {
        AssertSubstitutionForcesLegacyPath(typeof(TestRagfairPriceServiceSubclass));
    }

    /// <inheritdoc cref="AReplacedPriceServiceRoutesToTheLegacyPath"/>
    [Test]
    public void AReplacedServerHelperRoutesToTheLegacyPath()
    {
        AssertSubstitutionForcesLegacyPath(typeof(TestRagfairServerHelperSubclass));
    }

    /// <inheritdoc cref="AReplacedPriceServiceRoutesToTheLegacyPath"/>
    [Test]
    public void AReplacedAssortGeneratorRoutesToTheLegacyPath()
    {
        AssertSubstitutionForcesLegacyPath(typeof(TestRagfairAssortGeneratorSubclass));
    }

    /// <summary>
    /// The one database write this port replays: IsItemValidRagfairItem flags a custom-blacklisted
    /// template as unsellable by players (RagfairServerHelper.cs:61). That happens inside Rust, so
    /// the native path has to write it back.
    /// </summary>
    [Test]
    public void ACustomBlacklistedTemplateIsFlaggedUnsellableAfterANativePass()
    {
        var tpl = BuildSingleItem()[0].Template;
        var template = _templateTable.Items[tpl];
        var originalCanSell = template.Properties!.CanSellOnRagfair;
        var originalCustom = _ragfairConfig.Dynamic.Blacklist.Custom;

        try
        {
            _ragfairConfig.Dynamic.Blacklist.Custom = [.. originalCustom, tpl];
            _ragfairOfferGenerator.NativeTestSeed = 42;

            // A full pass, not the expired path: the expired path never runs the validity check (:473)
            _ragfairOfferGenerator.GenerateDynamicOffers();

            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
            Assert.That(template.Properties.CanSellOnRagfair, Is.False, "the CanSellOnRagfair replay did not reach the live template");
        }
        finally
        {
            template.Properties.CanSellOnRagfair = originalCanSell;
            _ragfairConfig.Dynamic.Blacklist.Custom = originalCustom;
            _ragfairOfferGenerator.NativeTestSeed = null;
            PurgeAddedOffers();
        }
    }

    /// <summary>
    /// One expired-offer regeneration pass over a single assort-shaped item - the cheap vehicle, and
    /// the one <c>RagfairServer.cs:79</c> uses.
    /// </summary>
    private void Generate(RagfairOfferGenerator generator)
    {
        var item = BuildSingleItem();

        // A tpl the pre-generated flea already stocks can be rejected outright by the holder's
        // per-template cap (RagfairOfferHolder.cs:153-163), which would make "offers were added"
        // fail for reasons that have nothing to do with dispatch
        PurgeFakePlayerOffersForTemplate(item[0].Template);

        generator.GenerateDynamicOffers([item]);
    }

    private void AssertSubstitutionForcesLegacyPath(Type substituteType)
    {
        var generator = BuildGenerator(Construct(substituteType));
        try
        {
            Generate(generator);

            Assert.That(
                generator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Legacy),
                $"a substituted {substituteType.Name} did not force legacy"
            );
        }
        finally
        {
            PurgeAddedOffers();
        }
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

    /// <summary>
    /// A RagfairOfferGenerator built by hand off the container's own services, with the given
    /// instances substituted for the parameters they fit - the shape DI would hand it if a mod had
    /// registered them.
    /// </summary>
    private static RagfairOfferGenerator BuildGenerator(params object[] substitutes)
    {
        return (RagfairOfferGenerator)Construct(typeof(RagfairOfferGenerator), substitutes);
    }

    private static object Construct(Type type, params object[] substitutes)
    {
        var constructor = type.GetConstructors().Single();
        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
                substitutes.FirstOrDefault(substitute => parameter.ParameterType.IsInstanceOfType(substitute))
                ?? DI.GetInstance().GetService(parameter.ParameterType)
            )
            .ToArray();

        return constructor.Invoke(arguments);
    }

    /// <summary>
    /// Stands in for a mod-registered price service: identical behaviour, different type.
    /// </summary>
    private class TestRagfairPriceServiceSubclass(
        ISptLogger<RagfairPriceService> logger,
        TemplateTable templateTable,
        HideoutTable hideoutTable,
        RandomUtil randomUtil,
        HandbookHelper handbookHelper,
        TraderHelper traderHelper,
        PresetHelper presetHelper,
        ItemHelper itemHelper,
        ServerLocalisationService serverLocalisationService,
        RagfairConfig ragfairConfig
    )
        : RagfairPriceService(
            logger,
            templateTable,
            hideoutTable,
            randomUtil,
            handbookHelper,
            traderHelper,
            presetHelper,
            itemHelper,
            serverLocalisationService,
            ragfairConfig
        ) { }

    /// <inheritdoc cref="TestRagfairPriceServiceSubclass"/>
    private class TestRagfairServerHelperSubclass(
        GlobalTable globalTable,
        TradersTable traderTable,
        RandomUtil randomUtil,
        TimeUtil timeUtil,
        ItemHelper itemHelper,
        WeightedRandomHelper weightedRandomHelper,
        MailSendService mailSendService,
        ServerLocalisationService localisationService,
        RagfairConfig ragfairConfig,
        ICloner cloner
    )
        : RagfairServerHelper(
            globalTable,
            traderTable,
            randomUtil,
            timeUtil,
            itemHelper,
            weightedRandomHelper,
            mailSendService,
            localisationService,
            ragfairConfig,
            cloner
        ) { }

    /// <inheritdoc cref="TestRagfairPriceServiceSubclass"/>
    private class TestRagfairAssortGeneratorSubclass(
        TemplateTable templateTable,
        ItemHelper itemHelper,
        PresetHelper presetHelper,
        SeasonalEventService seasonalEventService,
        ItemFilterService itemFilterService,
        RagfairConfig ragfairConfig,
        ICloner cloner
    ) : RagfairAssortGenerator(templateTable, itemHelper, presetHelper, seasonalEventService, itemFilterService, ragfairConfig, cloner) { }
}
