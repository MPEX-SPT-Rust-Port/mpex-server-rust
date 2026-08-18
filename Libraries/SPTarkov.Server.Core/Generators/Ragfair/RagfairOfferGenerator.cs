using System.Diagnostics;
using System.Reflection;
using HarmonyLib;
using Microsoft.Extensions.Logging;
using SPTarkov.Common.Extensions;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.Commerce;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Ragfair;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Ragfair;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Ragfair;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services;
using SPTarkov.Server.Core.Services.Commerce;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Generators.Ragfair;

[Injectable]
public class RagfairOfferGenerator(
    ISptLogger<RagfairOfferGenerator> logger,
    TemplateTable templateTable,
    TradersTable traderTable,
    GlobalTable globalTable,
    HashUtil hashUtil,
    RandomUtil randomUtil,
    TimeUtil timeUtil,
    RagfairServerHelper ragfairServerHelper,
    ProfileHelper profileHelper,
    HandbookHelper handbookHelper,
    BotHelper botHelper,
    SaveServer saveServer,
    PresetHelper presetHelper,
    RagfairAssortGenerator ragfairAssortGenerator,
    RagfairOfferService ragfairOfferService,
    RagfairPriceService ragfairPriceService,
    ServerLocalisationService localisationService,
    PaymentHelper paymentHelper,
    ItemHelper itemHelper,
    BotConfig botConfig,
    RagfairConfig ragfairConfig,
    ICloner cloner
)
{
    protected List<TplWithFleaPrice>? AllowedFleaPriceItemsForBarter;

    private readonly DatabaseMutationStamp? _databaseMutationStamp;
    private readonly IReadOnlyList<SptMod>? _loadedMods;

    /// Internal counter to ensure each offer created has a unique value for its intId property
    protected int OfferCounter;

    /// <summary>
    ///     Which implementation the most recent generation call ran - the spt-native path or the
    ///     retained 4.1.2 C# path. Test seam; also handy in a debugger.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     Test-only seed forwarded as <see cref="RagfairVaryingFields.TestSeed"/> on every
    ///     native request.
    /// </summary>
    internal ulong? NativeTestSeed { get; set; }

    /// <summary>
    ///     The native side caches the parsed invariant slice under the stamp value it was sent
    ///     with; this is the stamp of the last slice it accepted, so an unchanged stamp can skip
    ///     the slice entirely. Null until a slice is sent under an eligible cache. Internal set:
    ///     the desync test seam.
    /// </summary>
    internal long? LastSentSliceStamp { get; set; }

    /// <summary>
    ///     Whether the most recent native send carried the invariant slice. Test seam.
    /// </summary>
    internal bool LastSendIncludedSlice { get; private set; }

    /// <summary>
    ///     The constructor the container uses: the frozen 4.1.2 one plus the mutation stamp and
    ///     the loaded-mod list the request-slice cache eligibility gate reads. Additive and
    ///     apicompat-verified.
    /// </summary>
    public RagfairOfferGenerator(
        ISptLogger<RagfairOfferGenerator> logger,
        TemplateTable templateTable,
        TradersTable traderTable,
        GlobalTable globalTable,
        HashUtil hashUtil,
        RandomUtil randomUtil,
        TimeUtil timeUtil,
        RagfairServerHelper ragfairServerHelper,
        ProfileHelper profileHelper,
        HandbookHelper handbookHelper,
        BotHelper botHelper,
        SaveServer saveServer,
        PresetHelper presetHelper,
        RagfairAssortGenerator ragfairAssortGenerator,
        RagfairOfferService ragfairOfferService,
        RagfairPriceService ragfairPriceService,
        ServerLocalisationService localisationService,
        PaymentHelper paymentHelper,
        ItemHelper itemHelper,
        BotConfig botConfig,
        RagfairConfig ragfairConfig,
        ICloner cloner,
        DatabaseMutationStamp databaseMutationStamp,
        IReadOnlyList<SptMod> loadedMods
    )
        : this(
            logger,
            templateTable,
            traderTable,
            globalTable,
            hashUtil,
            randomUtil,
            timeUtil,
            ragfairServerHelper,
            profileHelper,
            handbookHelper,
            botHelper,
            saveServer,
            presetHelper,
            ragfairAssortGenerator,
            ragfairOfferService,
            ragfairPriceService,
            localisationService,
            paymentHelper,
            itemHelper,
            botConfig,
            ragfairConfig,
            cloner
        )
    {
        _databaseMutationStamp = databaseMutationStamp;
        _loadedMods = loadedMods;
    }

    /// <summary>
    ///     Whether a slice-less send is ever allowed: the stamp exists, the kill switch is off,
    ///     and either no mods are loaded or the user vouched their mods don't write tables
    ///     directly. A generator built on the frozen constructor has neither the stamp nor the
    ///     mod list and never caches.
    /// </summary>
    private bool SliceCacheEligible()
    {
        // Both null together (the frozen constructor) - checking each keeps the trust flag from ever
        // vouching for a mod list this instance was never handed
        if (_databaseMutationStamp is null || _loadedMods is null || ragfairConfig.DisableNativeRequestCache)
        {
            return false;
        }

        return _loadedMods.Count == 0 || ragfairConfig.TrustNativeRequestCacheWithMods;
    }

    /// <summary>
    ///     The 4.1.2 members a mod can Harmony-patch, across this class and the three collaborators
    ///     the native path folds in. Public, protected and protected-internal methods declared on
    ///     each - exactly the surface the apicompat gate freezes, statics included.
    ///     <see cref="GenerateDynamicOffers"/> itself is excluded: a patch on the dispatcher wraps
    ///     whichever path runs and does not need the legacy body. Everything else is never called
    ///     natively, so a patch on one would silently do nothing - including the dead-but-frozen
    ///     <c>GetRating</c> and <c>GetAvatarUrl</c>.
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. new[] { typeof(RagfairOfferGenerator), typeof(RagfairPriceService), typeof(RagfairServerHelper), typeof(RagfairAssortGenerator) }
            .SelectMany(type =>
                type.GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
            )
            // Property accessors and operators are IsSpecialName; constructors are not returned at all
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            .Where(method => method != typeof(RagfairOfferGenerator).GetMethod(nameof(GenerateDynamicOffers))),
    ];

    /// <summary>
    ///     The legacy path runs when forced by config, when any of the frozen 4.1.2 members carries a
    ///     live Harmony patch, or when a mod has substituted one of the collaborators the native path
    ///     folded in - running the retained C# implementation is the only way those hooks and
    ///     replacements can take effect with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (ragfairConfig.ForceLegacyRagfairGeneration)
        {
            return true;
        }

        if (
            _hookableMembers.Any(member =>
                Harmony.GetPatchInfo(member) is { } patches
                && (
                    patches.Prefixes.Count > 0
                    || patches.Postfixes.Count > 0
                    || patches.Transpilers.Count > 0
                    || patches.Finalizers.Count > 0
                )
            )
        )
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return ragfairPriceService.GetType() != typeof(RagfairPriceService)
            || ragfairServerHelper.GetType() != typeof(RagfairServerHelper)
            || ragfairAssortGenerator.GetType() != typeof(RagfairAssortGenerator);
    }

    /// <summary>
    ///     Create a flea offer and store it in the Ragfair server offers array
    /// </summary>
    /// <param name="details">Data needed to create a flea offer</param>
    /// <returns>RagfairOffer</returns>
    public RagfairOffer CreateAndAddFleaOffer(CreateFleaOfferDetails details)
    {
        // Create offer object
        var offer = CreateOffer(details);

        // Flag offer with creator type
        offer.CreatedBy = details.Creator;

        // Add offer into server storage
        ragfairOfferService.AddOffer(offer);

        return offer;
    }

    /// <summary>
    ///     Create an offer object ready to send to ragfairOfferService.addOffer()
    /// </summary>
    /// <param name="details">Data needed to create a flea offer</param>
    /// <returns>RagfairOffer</returns>
    protected RagfairOffer CreateOffer(CreateFleaOfferDetails details)
    {
        var offerRequirements = details.BarterScheme.Select(barter =>
        {
            var offerRequirement = new OfferRequirement
            {
                TemplateId = barter.Template,
                Count = Math.Round(barter.Count.Value, 2),
                OnlyFunctional = barter.OnlyFunctional ?? false,
            };

            // Dogtags define level and side
            if (barter.Level != null)
            {
                offerRequirement.Level = barter.Level;
                offerRequirement.Side = barter.Side;
            }

            return offerRequirement;
        });

        var rootItem = details.Items.FirstOrDefault();

        // Hydrate ammo boxes with cartridges + ensure only 1 item is present (ammo box)
        // On offer refresh don't re-add cartridges to ammo box that already has cartridges
        if (details.Items.Count == 1 && itemHelper.IsOfBaseclass(details.Items[0].Template, BaseClasses.AMMO_BOX))
        {
            itemHelper.AddCartridgesToAmmoBox(details.Items, itemHelper.GetItem(rootItem.Template).Value);
        }

        var roubleListingPrice = Math.Round(ConvertOfferRequirementsIntoRoubles(offerRequirements));
        var singleItemListingPrice = details.SellInOnePiece ? roubleListingPrice / details.Quantity : roubleListingPrice;

        var offer = new RagfairOffer
        {
            Id = new MongoId(),
            InternalId = OfferCounter,
            User =
                details.Creator == OfferCreator.Player
                    ? CreatePlayerUserDataForFleaOffer(details.UserId)
                    : CreateUserDataForFleaOffer(details.UserId, details.Creator == OfferCreator.Trader),
            Root = rootItem.Id,
            Items = details.Items,
            ItemsCost = Math.Round(handbookHelper.GetTemplatePrice(rootItem.Template)), // Handbook price
            Requirements = offerRequirements,
            RequirementsCost = Math.Round(singleItemListingPrice),
            SummaryCost = roubleListingPrice,
            StartTime = details.Time,
            EndTime = GetOfferEndTime(details.Creator, details.UserId, details.Time),
            LoyaltyLevel = details.LoyalLevel,
            SellInOnePiece = details.SellInOnePiece,
            Locked = false,
            Quantity = details.Quantity,
        };

        OfferCounter++;

        return offer;
    }

    /// <summary>
    ///     Create the user object stored inside each flea offer object
    /// </summary>
    /// <param name="userId">User creating the offer</param>
    /// <param name="isTrader">Is the user creating the offer a trader</param>
    /// <returns>RagfairOfferUser</returns>
    protected RagfairOfferUser CreateUserDataForFleaOffer(MongoId userId, bool isTrader)
    {
        // Trader offer
        if (isTrader)
        {
            return new RagfairOfferUser { Id = userId, MemberType = MemberCategory.Trader };
        }

        // 'Fake' pmc offer
        return new RagfairOfferUser
        {
            Id = userId,
            MemberType = MemberCategory.Default,
            Nickname = botHelper.GetPmcNicknameOfMaxLength(botConfig.BotNameLengthLimit),
            Rating = randomUtil.GetDouble(ragfairConfig.Dynamic.Rating.Min, ragfairConfig.Dynamic.Rating.Max),
            IsRatingGrowing = randomUtil.GetBool(),
            Avatar = null,
            Aid = hashUtil.GenerateAccountId(),
        };
    }

    /// <summary>
    /// Create the user object stored inside each flea offer object
    /// </summary>
    /// <param name="userId">Player id</param>
    /// <returns>OfferUser object</returns>
    protected RagfairOfferUser CreatePlayerUserDataForFleaOffer(MongoId userId)
    {
        var playerProfile = profileHelper.GetPmcProfile(userId);
        return new RagfairOfferUser
        {
            Id = playerProfile.Id.Value,
            MemberType = playerProfile.Info.MemberCategory,
            SelectedMemberCategory = playerProfile.Info.SelectedMemberCategory,
            Nickname = playerProfile.Info.Nickname,
            Rating = playerProfile.RagfairInfo.Rating ?? 0,
            IsRatingGrowing = playerProfile.RagfairInfo.IsRatingGrowing,
            Avatar = null,
            Aid = playerProfile.Aid,
        };
    }

    /// <summary>
    ///     Calculate the offer price that's listed on the flea listing
    /// </summary>
    /// <param name="offerRequirements"> barter requirements for offer </param>
    /// <returns> rouble cost of offer </returns>
    protected double ConvertOfferRequirementsIntoRoubles(IEnumerable<OfferRequirement> offerRequirements)
    {
        return offerRequirements.Sum(requirement =>
            paymentHelper.IsMoneyTpl(requirement.TemplateId)
                ? Math.Round(CalculateRoublePrice(requirement.Count.Value, requirement.TemplateId))
                : ragfairPriceService.GetFleaPriceForItem(requirement.TemplateId) * requirement.Count.Value
        );
    }

    /// <summary>
    ///     Get avatar url from trader table in db
    /// </summary>
    /// <param name="isTrader"> Is user we're getting avatar for a trader </param>
    /// <param name="userId"> Persons id to get avatar of </param>
    /// <returns> Url of avatar as String </returns>
    protected string GetAvatarUrl(bool isTrader, MongoId userId)
    {
        if (isTrader)
        {
            return traderTable.GetTrader(userId).Base.Avatar;
        }

        return "/files/trader/avatar/unknown.jpg";
    }

    /// <summary>
    ///     Convert a count of currency into roubles
    /// </summary>
    /// <param name="currencyCount"> Amount of currency to convert into roubles </param>
    /// <param name="currencyType"> Type of currency (euro/dollar/rouble) </param>
    /// <returns> Count of roubles </returns>
    protected double CalculateRoublePrice(double currencyCount, MongoId currencyType)
    {
        if (currencyType == Money.ROUBLES)
        {
            return currencyCount;
        }

        return handbookHelper.InRoubles(currencyCount, currencyType);
    }

    /// <summary>
    ///     Get a flea trading rating for the passed in user
    /// </summary>
    /// <param name="userId"> User to get flea rating of </param>
    /// <returns> Flea rating value </returns>
    protected double? GetRating(MongoId userId)
    {
        // Player offer
        if (profileHelper.IsPlayer(userId))
        {
            return saveServer.GetProfile(userId).CharacterData?.PmcData?.RagfairInfo?.Rating;
        }

        // Trader offer
        if (ragfairServerHelper.IsTrader(userId))
        {
            return 1;
        }

        // Generated pmc offer
        return randomUtil.GetDouble(ragfairConfig.Dynamic.Rating.Min, ragfairConfig.Dynamic.Rating.Max);
    }

    /// <summary>
    ///     Get number of section until offer should expire
    /// </summary>
    /// <param name="creatorType"></param>
    /// <param name="userId"> ID of the offer owner </param>
    /// <param name="time"> Time the offer is posted in seconds </param>
    /// <returns> Number of seconds until offer expires </returns>
    protected long GetOfferEndTime(OfferCreator creatorType, MongoId userId, long time)
    {
        if (creatorType == OfferCreator.Player)
        {
            // Player offer = current time + offerDurationTimeInHour;
            var offerDurationTimeHours = globalTable.Configuration.RagFair.OfferDurationTimeInHour;
            return (long)(timeUtil.GetTimeStamp() + Math.Round(offerDurationTimeHours * TimeUtil.OneHourAsSeconds));
        }

        if (creatorType == OfferCreator.Trader)
        {
            return (long)traderTable.GetTrader(userId).Base.NextResupply;
        }

        var randomSpread = randomUtil.GetDouble(ragfairConfig.Dynamic.EndTimeSeconds.Min, ragfairConfig.Dynamic.EndTimeSeconds.Max);

        // Fake-player offer
        return (long)Math.Round(time + randomSpread);
    }

    /// <summary>
    ///     Create multiple offers for items by using a unique list of items we've generated previously
    /// </summary>
    /// <param name="expiredOffers"> Optional, expired offers to regenerate </param>
    public void GenerateDynamicOffers(IEnumerable<List<Item>>? expiredOffers = null)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;

            GenerateDynamicOffersLegacy(expiredOffers);

            return;
        }

        LastPathTaken = LootGenerationPath.Native;

        var stamp = _databaseMutationStamp?.Current ?? 0;
        var eligible = SliceCacheEligible();
        var sendSlice = !eligible || LastSentSliceStamp != stamp;

        FramedOffersResult result;
        try
        {
            result = SptNative.GenerateDynamicOffers(BuildNativeRequest(sendSlice, stamp, expiredOffers));
            LastSendIncludedSlice = sendSlice;
        }
        catch (NativeStaleEpochException)
        {
            // The native cache does not hold the slice this stamp names - resend it whole
            result = SptNative.GenerateDynamicOffers(BuildNativeRequest(true, stamp, expiredOffers));
            LastSendIncludedSlice = true;
        }

        LastSentSliceStamp = eligible ? stamp : null;

        // The native side decided these templates are unsellable and, unlike everything else it
        // touched, that decision belongs to the live database (RagfairServerHelper.cs:61). A write
        // that actually flips a value changes the projected slice, so it bumps the stamp - guarded,
        // or re-reported already-false templates would invalidate the cache every pass
        var flippedCanSell = false;
        foreach (var tpl in result.RejectedCanSellTemplates)
        {
            if (
                templateTable.Items.TryGetValue(tpl, out var template)
                && template.Properties is not null
                && template.Properties.CanSellOnRagfair != false
            )
            {
                template.Properties.CanSellOnRagfair = false;
                flippedCanSell = true;
            }
        }

        if (flippedCanSell)
        {
            _databaseMutationStamp?.Bump();
        }

        // Legacy inserts each offer as it creates it; the holder's live per-template cap runs the
        // same way either way, it just sees the whole batch at once here
        foreach (var offer in result.Offers)
        {
            ragfairOfferService.AddOffer(offer);
        }

        // CreateOffer increments the counter per offer created, not per offer the holder accepted
        OfferCounter += result.Offers.Count;
    }

    /// <summary>
    ///     One native request for this pass, with the invariant slice included only when the
    ///     native cache cannot already be holding it.
    /// </summary>
    private GenerateDynamicOffersRequest BuildNativeRequest(bool sendSlice, long stamp, IEnumerable<List<Item>>? expiredOffers)
    {
        return RagfairPayloadProjection.BuildRequest(
            sendSlice
                ? RagfairPayloadProjection.BuildInvariantSlice(
                    templateTable,
                    handbookHelper,
                    ragfairPriceService.TraderHelper,
                    presetHelper,
                    ragfairAssortGenerator.ItemFilterService,
                    ragfairAssortGenerator.SeasonalEventService,
                    botHelper.BotTable,
                    itemHelper,
                    botConfig,
                    ragfairConfig
                )
                : null,
            stamp,
            expiredOffers,
            timeUtil.GetTimeStamp(),
            OfferCounter,
            NativeTestSeed
        );
    }

    /// <summary>
    ///     The retained 4.1.2 implementation of <see cref="GenerateDynamicOffers"/>, run when
    ///     <see cref="UseLegacyPath"/> says a mod needs real baseline semantics.
    /// </summary>
    /// <param name="expiredOffers"> Optional, expired offers to regenerate </param>
    private void GenerateDynamicOffersLegacy(IEnumerable<List<Item>>? expiredOffers = null)
    {
        var replacingExpiredOffers = expiredOffers is not null && expiredOffers.Any();

        var stopwatch = Stopwatch.StartNew();
        // get assort items from param if they exist, otherwise grab freshly generated assorts
        var assortItemsToProcess = replacingExpiredOffers ? expiredOffers ?? [] : ragfairAssortGenerator.GenerateRagfairAssortItems();
        stopwatch.Stop();
        if (logger.IsLogEnabled(LogLevel.Debug) && stopwatch.ElapsedMilliseconds > 0)
        {
            logger.Debug($"Took {stopwatch.ElapsedMilliseconds}ms to GetRagfairAssorts - {assortItemsToProcess.Count()} items");
        }

        stopwatch.Restart();
        var tasks = new List<Task>();
        foreach (var assortItemWithChildren in assortItemsToProcess)
        {
            tasks.Add(
                Task.Factory.StartNew(() =>
                {
                    CreateOffersFromAssort(assortItemWithChildren, replacingExpiredOffers, ragfairConfig.Dynamic);
                })
            );
        }

        Task.WaitAll(tasks.ToArray());
        stopwatch.Stop();
        if (logger.IsLogEnabled(LogLevel.Debug))
        {
            logger.Debug($"Took {stopwatch.ElapsedMilliseconds}ms to CreateOffersFromAssort");
        }
    }

    /// <summary>
    ///     Generates offers from an item and it's children on the flea market
    /// </summary>
    /// <param name="assortItemWithChildren"> Item with its children to process into offers </param>
    /// <param name="isExpiredOffer"> Is an expired offer </param>
    /// <param name="config"> Ragfair dynamic config </param>
    protected void CreateOffersFromAssort(List<Item> assortItemWithChildren, bool isExpiredOffer, Dynamic config)
    {
        var rootItem = assortItemWithChildren.FirstOrDefault();
        var itemToSellDetails = itemHelper.GetItem(rootItem.Template);

        // Only perform checks on newly generated items, skip expired items being refreshed
        if (!(isExpiredOffer || ragfairServerHelper.IsItemValidRagfairItem(itemToSellDetails)))
        {
            return;
        }

        // Armor presets can hold plates above the allowed flea level, remove if necessary
        var isPreset = rootItem?.Upd?.SptPresetId is not null && presetHelper.IsPreset(rootItem.Upd.SptPresetId.Value);
        if (!isExpiredOffer && isPreset && ragfairConfig.Dynamic.Blacklist.EnableBsgList)
        {
            RemoveBannedPlatesFromPreset(assortItemWithChildren, ragfairConfig.Dynamic.Blacklist.ArmorPlate);
        }

        // Get number of offers to create
        // Limit to 1 offer when processing expired - like-for-like replacement
        var offerCount = isExpiredOffer ? 1 : ragfairServerHelper.GetOfferCountByBaseType(itemToSellDetails.Value.Parent);

        for (var index = 0; index < offerCount; index++)
        {
            // Clone the item so we don't have shared references and generate new item IDs
            var clonedAssort = cloner.Clone(assortItemWithChildren);
            itemHelper.ReparentItemAndChildren(clonedAssort[0], clonedAssort);

            // Clear unnecessary properties
            clonedAssort[0].ParentId = null;
            clonedAssort[0].SlotId = null;

            CreateSingleOfferForItem(
                new MongoId(),
                clonedAssort,
                isPreset,
                itemToSellDetails.Value,
                isExpiredOffer,
                OfferCreator.FakePlayer
            );
        }
    }

    /// <summary>
    ///     Iterate over an items children and look for plates above desired level and remove them
    /// </summary>
    /// <param name="presetWithChildren"> Preset to check for plates </param>
    /// <param name="plateSettings"> Settings </param>
    /// <returns> True if plates removed </returns>
    protected bool RemoveBannedPlatesFromPreset(List<Item> presetWithChildren, ArmorPlateBlacklistSettings plateSettings)
    {
        if (!itemHelper.ArmorItemCanHoldMods(presetWithChildren[0].Template))
        // Cant hold armor inserts, skip
        {
            return false;
        }

        var plateSlots = presetWithChildren
            .Where(item => itemHelper.GetRemovablePlateSlotIds().Contains(item.SlotId?.ToLowerInvariant()))
            .ToList();
        if (plateSlots.Count == 0)
        // Has no plate slots e.g. "front_plate", exit
        {
            return false;
        }

        var removedPlate = false;
        foreach (var plateSlot in plateSlots)
        {
            var plateDetails = itemHelper.GetItem(plateSlot.Template).Value;
            if (plateSettings.IgnoreSlots.Contains(plateSlot.SlotId.ToLowerInvariant()))
            {
                continue;
            }

            var plateArmorLevel = plateDetails.Properties.ArmorClass ?? 0;
            if (plateArmorLevel > plateSettings.MaxProtectionLevel)
            {
                presetWithChildren.Splice(presetWithChildren.IndexOf(plateSlot), 1);
                removedPlate = true;
            }
        }

        return removedPlate;
    }

    /// <summary>
    ///     Create one flea offer for a specific item
    /// </summary>
    /// <param name="sellerId"> ID of seller</param>
    /// <param name="itemWithChildren"> Item to create offer for</param>
    /// <param name="isPreset"> Is item a weapon preset</param>
    /// <param name="itemToSellDetails"> Raw DB item details </param>
    /// <param name="isExpiredOffer">Offer being created is to replace an expired, existing offer</param>
    /// <param name="offerCreator">What type of entity created this offer</param>
    protected void CreateSingleOfferForItem(
        MongoId sellerId,
        List<Item> itemWithChildren,
        bool isPreset,
        TemplateItem itemToSellDetails,
        bool isExpiredOffer,
        OfferCreator offerCreator
    )
    {
        var rootItem = itemWithChildren.FirstOrDefault();

        // Get randomised amount to list on flea
        var desiredStackSize = ragfairServerHelper.CalculateDynamicStackCount(rootItem.Template, isPreset);

        // Reset stack count to 1 from whatever it was prior
        rootItem.Upd.StackObjectsCount = 1;

        if (!isExpiredOffer && itemHelper.ArmorItemCanHoldMods(rootItem.Template))
        {
            // Run randomised chance to remove removable plates from new offers(not expired)
            RemoveArmorPlates(itemWithChildren, rootItem);
        }

        var isBarterOffer = randomUtil.GetChance100(ragfairConfig.Dynamic.Barter.ChancePercent);
        var isPackOffer =
            !isBarterOffer
            && randomUtil.GetChance100(ragfairConfig.Dynamic.Pack.ChancePercent)
            && itemWithChildren.Count == 1
            && itemHelper.IsOfBaseclasses(rootItem.Template, ragfairConfig.Dynamic.Pack.ItemTypeWhitelist);

        List<BarterScheme> barterScheme;
        if (isPackOffer)
        {
            // Set pack size
            desiredStackSize = randomUtil.GetInt(ragfairConfig.Dynamic.Pack.ItemCountMin, ragfairConfig.Dynamic.Pack.ItemCountMax);

            // Don't randomise pack items
            barterScheme = CreateCurrencyBarterScheme(itemWithChildren, isPackOffer, desiredStackSize);
        }
        else if (isBarterOffer)
        {
            // Apply randomised properties
            RandomiseOfferItemUpdProperties(sellerId, itemWithChildren, itemToSellDetails, offerCreator);
            barterScheme = CreateBarterBarterScheme(itemWithChildren, ragfairConfig.Dynamic.Barter);
            if (ragfairConfig.Dynamic.Barter.MakeSingleStackOnly)
            {
                var rootBarterItem = itemWithChildren.FirstOrDefault();
                if (rootBarterItem?.Upd != null)
                {
                    rootBarterItem.Upd.StackObjectsCount = 1;
                }
            }
        }
        else
        {
            // Not barter or pack offer
            // Apply randomised properties
            RandomiseOfferItemUpdProperties(sellerId, itemWithChildren, itemToSellDetails, offerCreator);
            barterScheme = CreateCurrencyBarterScheme(itemWithChildren, false);
        }

        var createOfferDetails = new CreateFleaOfferDetails
        {
            UserId = sellerId,
            Time = timeUtil.GetTimeStamp(),
            Items = itemWithChildren,
            BarterScheme = barterScheme,
            LoyalLevel = 1,
            Quantity = desiredStackSize,
            Creator = offerCreator,
            SellInOnePiece = isPackOffer, // sellAsOnePiece - pack offer
        };

        CreateAndAddFleaOffer(createOfferDetails);
    }

    /// <summary>
    /// Run % check to remove removable armor plates from item
    /// </summary>
    /// <param name="itemWithChildren">Armor item</param>
    /// <param name="rootItem">Root armor item</param>
    protected void RemoveArmorPlates(List<Item> itemWithChildren, Item rootItem)
    {
        var armorConfig = ragfairConfig.Dynamic.Armor;

        var shouldRemovePlates = randomUtil.GetChance100(armorConfig.RemoveRemovablePlateChance);
        if (!shouldRemovePlates || !itemHelper.ArmorItemHasRemovablePlateSlots(rootItem.Template))
        {
            return;
        }

        var offerItemPlatesToRemove = itemWithChildren.Where(item =>
            armorConfig.PlateSlotIdToRemovePool.Contains(item.SlotId?.ToLowerInvariant())
        );

        // Latest first, to ensure we don't move later items off by 1 each time we remove an item below it
        var indexesToRemove = offerItemPlatesToRemove.Select(plateItem => itemWithChildren.IndexOf(plateItem)).ToHashSet();
        foreach (var index in indexesToRemove.OrderByDescending(x => x))
        {
            itemWithChildren.RemoveAt(index);
        }
    }

    /// <summary>
    ///     Generate trader offers on flea using the traders assort data
    /// </summary>
    /// <param name="traderId"> Trader to generate offers for </param>
    public void GenerateFleaOffersForTrader(MongoId traderId)
    {
        // Purge
        ragfairOfferService.RemoveAllOffersByTrader(traderId);

        var time = timeUtil.GetTimeStamp();
        var trader = traderTable.GetTrader(traderId);
        var assortsClone = cloner.Clone(trader.Assort);

        // Trader assorts / assort items are missing
        if (assortsClone?.Items?.Count is null or 0)
        {
            logger.Error(localisationService.GetText("ragfair-no_trader_assorts_cant_generate_flea_offers", trader.Base.Nickname));
            return;
        }

        var blacklist = ragfairConfig.Dynamic.Blacklist;
        var childAssortItems = assortsClone.Items.Where(x => !string.Equals(x.ParentId, "hideout", StringComparison.Ordinal)).ToList();
        foreach (var item in assortsClone.Items)
        {
            // We only want to process 'base/root' items, no children
            if (item.SlotId != "hideout")
            // skip mod items
            {
                continue;
            }

            // Run blacklist check on trader offers
            if (blacklist.TraderItems)
            {
                var itemDetails = itemHelper.GetItem(item.Template);
                if (!itemDetails.Key)
                {
                    logger.Warning(localisationService.GetText("ragfair-tpl_not_a_valid_item", item.Template));
                    continue;
                }

                // Don't include items that BSG has blacklisted from flea
                if (blacklist.EnableBsgList && !(itemDetails.Value?.Properties?.CanSellOnRagfair ?? false))
                {
                    continue;
                }
            }

            var isPreset = presetHelper.IsPreset(item.Id);
            var items = isPreset
                ? ragfairServerHelper.GetPresetItems(item)
                : [item, .. itemHelper.FindAndReturnChildrenByAssort(item.Id, childAssortItems)];

            if (!assortsClone.BarterScheme.TryGetValue(item.Id, out var barterScheme))
            {
                logger.Warning(
                    localisationService.GetText(
                        "ragfair-missing_barter_scheme",
                        new
                        {
                            itemId = item.Id,
                            tpl = item.Template,
                            name = trader.Base.Nickname,
                        }
                    )
                );
                continue;
            }

            var barterSchemeItems = barterScheme[0];
            if (!assortsClone.LoyalLevelItems.TryGetValue(item.Id, out var loyalLevel))
            {
                logger.Warning(
                    localisationService.GetText(
                        "ragfair-missing_loyal_level_item",
                        new
                        {
                            itemId = item.Id,
                            tpl = item.Template,
                            name = trader.Base.Nickname,
                        }
                    )
                );
                continue;
            }

            var createOfferDetails = new CreateFleaOfferDetails
            {
                UserId = traderId,
                Time = time,
                Items = items,
                BarterScheme = barterSchemeItems,
                LoyalLevel = loyalLevel,
                Quantity = (int?)item.Upd?.StackObjectsCount ?? 1,
                Creator = OfferCreator.Trader,
            };
            CreateAndAddFleaOffer(createOfferDetails);

            // Refresh complete, reset flag to false
            trader.Base.RefreshTraderRagfairOffers = false;
        }
    }

    /// <summary>
    ///     Get array of an item with its mods + condition properties (e.g. durability) <br />
    ///     Apply randomisation adjustments to condition if item base is found in ragfair.json/dynamic/condition
    /// </summary>
    /// <param name="userId"> ID of owner of item </param>
    /// <param name="itemWithMods"> Item and mods, get condition of first item (only first array item is modified) </param>
    /// <param name="itemDetails"> DB details of first item</param>
    /// <param name="offerCreator"></param>
    protected void RandomiseOfferItemUpdProperties(
        MongoId userId,
        IEnumerable<Item> itemWithMods,
        TemplateItem itemDetails,
        OfferCreator offerCreator
    )
    {
        // Add any missing properties to first item in array
        AddMissingConditions(itemWithMods.First());

        if (offerCreator is OfferCreator.FakePlayer)
        {
            var parentId = GetDynamicConditionIdForTpl(itemDetails.Id);
            if (parentId == null)
            // No condition details found, don't proceed with modifying item conditions
            {
                return;
            }

            // Roll random chance to randomise item condition
            if (randomUtil.GetChance100(ragfairConfig.Dynamic.Condition[parentId.Value].ConditionChance * 100))
            {
                RandomiseItemCondition(parentId.Value, itemWithMods, itemDetails);
            }
        }
    }

    /// <summary>
    ///     Get the relevant condition id if item tpl matches in ragfair.json/condition
    /// </summary>
    /// <param name="tpl"> Item to look for matching condition object</param>
    /// <returns> Condition ID </returns>
    protected MongoId? GetDynamicConditionIdForTpl(MongoId tpl)
    {
        // Get keys from condition config dictionary
        var configConditions = ragfairConfig.Dynamic.Condition.Keys;
        foreach (var baseClass in configConditions)
        {
            if (itemHelper.IsOfBaseclass(tpl, baseClass))
            {
                return baseClass;
            }
        }

        return null;
    }

    /// <summary>
    ///     Alter an items condition based on its item base type
    /// </summary>
    /// <param name="conditionSettingsId"> Also the parentID of item being altered </param>
    /// <param name="itemWithMods"> Item to adjust condition details of </param>
    /// <param name="itemDetails"> DB Item details of first item in list </param>
    protected void RandomiseItemCondition(MongoId conditionSettingsId, IEnumerable<Item> itemWithMods, TemplateItem itemDetails)
    {
        var rootItem = itemWithMods.First();

        var itemConditionValues = ragfairConfig.Dynamic.Condition[conditionSettingsId];
        var maxMultiplier = randomUtil.GetDouble(itemConditionValues.Max.Min, itemConditionValues.Max.Min);
        var currentMultiplier = randomUtil.GetDouble(itemConditionValues.Current.Min, itemConditionValues.Current.Max);

        // Randomise armor + plates + armor related things
        if (
            itemHelper.ArmorItemCanHoldMods(rootItem.Template)
            || itemHelper.IsOfBaseclasses(rootItem.Template, [BaseClasses.ARMOR_PLATE, BaseClasses.ARMORED_EQUIPMENT])
        )
        {
            RandomiseArmorDurabilityValues(itemWithMods, currentMultiplier, maxMultiplier);

            // Add hits to visor
            var visorMod = itemWithMods.FirstOrDefault(item =>
                item.ParentId == BaseClasses.ARMORED_EQUIPMENT.ToString() && item.SlotId == "mod_equipment_000"
            );
            if (visorMod != null && randomUtil.GetChance100(25))
            {
                visorMod.AddUpd();

                visorMod.Upd.FaceShield = new UpdFaceShield { Hits = randomUtil.GetInt(1, 3) };
            }

            return;
        }

        // Randomise Weapons
        if (itemHelper.IsOfBaseclass(itemDetails.Id, BaseClasses.WEAPON))
        {
            RandomiseWeaponDurability(itemWithMods.First(), itemDetails, maxMultiplier, currentMultiplier);

            return;
        }

        if (rootItem.Upd?.MedKit != null)
        {
            // Randomize health
            var hpResource = Math.Round((double)rootItem.Upd.MedKit.HpResource * maxMultiplier);
            rootItem.Upd.MedKit.HpResource = hpResource == 0D ? 1D : hpResource;
            return;
        }

        if (rootItem.Upd?.Key != null && itemDetails.Properties.MaximumNumberOfUsage > 1)
        {
            // Randomize key uses
            rootItem.Upd.Key.NumberOfUsages = (int?)Math.Round(itemDetails.Properties.MaximumNumberOfUsage.Value * (1 - maxMultiplier));
            return;
        }

        if (rootItem.Upd?.FoodDrink != null)
        {
            // randomize food/drink value
            var hpPercent = Math.Round((double)itemDetails.Properties.MaxResource * maxMultiplier);
            rootItem.Upd.FoodDrink.HpPercent = hpPercent == 0D ? 1D : hpPercent;

            return;
        }

        if (rootItem.Upd?.RepairKit != null)
        {
            // randomize repair kit (armor/weapon) uses
            var resource = Math.Round((double)itemDetails.Properties.MaxRepairResource * maxMultiplier);
            rootItem.Upd.RepairKit.Resource = resource == 0D ? 1D : resource;

            return;
        }

        if (itemHelper.IsOfBaseclass(itemDetails.Id, BaseClasses.FUEL))
        {
            var totalCapacity = itemDetails.Properties.MaxResource;

            // Randomise multi between value in config and 1 (100%)
            var randomisedMulti = randomUtil.GetDouble(maxMultiplier, 1);
            var remainingFuel = Math.Round((double)totalCapacity * randomisedMulti);
            rootItem.Upd.Resource = new UpdResource { UnitsConsumed = totalCapacity - remainingFuel, Value = remainingFuel };
        }
    }

    /// <summary>
    ///     Adjust an items durability/maxDurability value
    /// </summary>
    /// <param name="item"> Item (weapon/armor) to adjust </param>
    /// <param name="itemDbDetails"> Item details from DB </param>
    /// <param name="maxMultiplier"> Value to multiply max durability by </param>
    /// <param name="currentMultiplier"> Value to multiply current durability by </param>
    protected void RandomiseWeaponDurability(Item item, TemplateItem itemDbDetails, double maxMultiplier, double currentMultiplier)
    {
        // Max
        var baseMaxDurability = itemDbDetails.Properties.MaxDurability;
        var lowestMaxDurability = randomUtil.GetDouble(maxMultiplier, 1) * baseMaxDurability;
        var chosenMaxDurability = Math.Round(randomUtil.GetDouble((double)lowestMaxDurability, (double)baseMaxDurability));

        // Current
        var lowestCurrentDurability = randomUtil.GetDouble(currentMultiplier, 1) * chosenMaxDurability;
        var chosenCurrentDurability = Math.Round(randomUtil.GetDouble(lowestCurrentDurability, chosenMaxDurability));

        item.Upd.Repairable.Durability = chosenCurrentDurability == 0 ? 1D : chosenCurrentDurability; // Never var value become 0
        item.Upd.Repairable.MaxDurability = chosenMaxDurability;
    }

    /// <summary>
    ///     Randomise the durability values for an armors plates and soft inserts
    /// </summary>
    /// <param name="armorWithMods"> Armor item with its child mods </param>
    /// <param name="currentMultiplier"> Chosen multiplier to use for current durability value </param>
    /// <param name="maxMultiplier"> Chosen multiplier to use for max durability value </param>
    protected void RandomiseArmorDurabilityValues(IEnumerable<Item> armorWithMods, double currentMultiplier, double maxMultiplier)
    {
        foreach (var armorItem in armorWithMods)
        {
            var itemDbDetails = itemHelper.GetItem(armorItem.Template).Value;
            if (itemDbDetails.Properties.ArmorClass > 1)
            {
                armorItem.AddUpd();

                var baseMaxDurability = itemDbDetails.Properties.MaxDurability;
                var lowestMaxDurability = randomUtil.GetDouble(maxMultiplier, 1) * baseMaxDurability;
                var chosenMaxDurability = Math.Round(randomUtil.GetDouble((double)lowestMaxDurability, (double)baseMaxDurability));

                var lowestCurrentDurability = randomUtil.GetDouble(currentMultiplier, 1) * chosenMaxDurability;
                var chosenCurrentDurability = Math.Round(randomUtil.GetDouble(lowestCurrentDurability, chosenMaxDurability));

                armorItem.Upd.Repairable = new UpdRepairable
                {
                    Durability = chosenCurrentDurability == 0D ? 1D : chosenCurrentDurability, // Never var value become 0
                    MaxDurability = chosenMaxDurability,
                };
            }
        }
    }

    /// <summary>
    ///     Add missing conditions to an item if needed. <br />
    ///     Durability for repairable items. <br />
    ///     HpResource for medical items.
    /// </summary>
    /// <param name="item"> Item to add conditions to </param>
    protected void AddMissingConditions(Item item)
    {
        var props = itemHelper.GetItem(item.Template).Value.Properties;
        var isRepairable = props.Durability != null;
        var isMedkit = props.MaxHpResource != null;
        var isKey = props.MaximumNumberOfUsage != null;
        var isConsumable = props.MaxResource > 1 && props.FoodUseTime != null;
        var isRepairKit = props.MaxRepairResource != null;

        if (isRepairable && props.Durability > 0)
        {
            item.Upd.Repairable = new UpdRepairable { Durability = props.Durability, MaxDurability = props.Durability };

            return;
        }

        if (isMedkit && props.MaxHpResource > 0)
        {
            item.Upd.MedKit = new UpdMedKit { HpResource = props.MaxHpResource };

            return;
        }

        if (isKey)
        {
            item.Upd.Key = new UpdKey { NumberOfUsages = 0 };

            return;
        }

        // Food/drink
        if (isConsumable)
        {
            item.Upd.FoodDrink = new UpdFoodDrink { HpPercent = props.MaxResource };

            return;
        }

        if (isRepairKit)
        {
            item.Upd.RepairKit = new UpdRepairKit { Resource = props.MaxRepairResource };
        }
    }

    /// <summary>
    ///     Create a barter-based barter scheme, if not possible, fall back to making barter scheme currency based
    /// </summary>
    /// <param name="offerItems"> Items for sale in offer </param>
    /// <param name="barterConfig"> Barter config from ragfairConfig.Dynamic.barter </param>
    /// <returns> Barter scheme </returns>
    protected List<BarterScheme> CreateBarterBarterScheme(IEnumerable<Item> offerItems, BarterDetails barterConfig)
    {
        // Get flea price of item being sold
        var priceOfOfferItem = ragfairPriceService.GetDynamicOfferPriceForOffer(offerItems, Money.ROUBLES, false);

        // Don't make items under a designated rouble value into barter offers
        if (priceOfOfferItem < barterConfig.MinRoubleCostToBecomeBarter)
        {
            return CreateCurrencyBarterScheme(offerItems, false);
        }

        // Get a randomised number of barter items to list offer for
        var barterItemCount = randomUtil.GetInt(barterConfig.ItemCountMin, barterConfig.ItemCountMax);

        // Get desired cost of individual item offer will be listed for e.g. offer = 15k, item count = 3, desired item cost = 5k
        var desiredItemCostRouble = Math.Round(priceOfOfferItem / barterItemCount);

        // Rouble amount to go above/below when looking for an item (Wiggle cost of item a little)
        var offerCostVarianceRoubles = desiredItemCostRouble * barterConfig.PriceRangeVariancePercent / 100;

        // Dict of items and their flea price (cached on first use)
        var itemFleaPrices = GetFleaPricesAsArray();

        // Filter possible barters to items that match the price range + not itself
        var min = desiredItemCostRouble - offerCostVarianceRoubles;
        var max = desiredItemCostRouble + offerCostVarianceRoubles;
        var rootOfferItem = offerItems.FirstOrDefault();

        var itemsInsidePriceBounds = itemFleaPrices.Where(itemAndPrice =>
            itemAndPrice.Price >= min && itemAndPrice.Price <= max && itemAndPrice.Tpl != rootOfferItem.Template // Don't allow the item being sold to be chosen
        );

        // No items on flea have a matching price, fall back to currency
        if (!itemsInsidePriceBounds.Any())
        {
            return CreateCurrencyBarterScheme(offerItems, false);
        }

        // Choose random item from price-filtered flea items
        var randomItem = randomUtil.GetArrayValue(itemsInsidePriceBounds);

        return [new BarterScheme { Count = barterItemCount, Template = randomItem.Tpl }];
    }

    /// <summary>
    ///     Get an array of flea prices + item tpl, cached in generator class inside `allowedFleaPriceItemsForBarter`
    /// </summary>
    /// <returns> List with tpl/price values </returns>
    protected List<TplWithFleaPrice> GetFleaPricesAsArray()
    {
        // Generate if needed
        if (AllowedFleaPriceItemsForBarter == null)
        {
            var fleaPrices = templateTable.Prices;

            // Only get prices for items that also exist in items.json
            var filteredFleaItems = fleaPrices
                .Select(kvTpl => new TplWithFleaPrice { Tpl = kvTpl.Key, Price = kvTpl.Value })
                .Where(item => itemHelper.GetItem(item.Tpl).Key);

            var itemTypeBlacklist = ragfairConfig.Dynamic.Barter.ItemTypeBlacklist;
            var itemTplBlacklist = ragfairConfig.Dynamic.Barter.ItemTplBlacklist;
            AllowedFleaPriceItemsForBarter = filteredFleaItems
                .Where(item => !itemHelper.IsOfBaseclasses(item.Tpl, itemTypeBlacklist))
                .Where(item => !itemTplBlacklist.Contains(item.Tpl))
                .ToList();
        }

        return AllowedFleaPriceItemsForBarter;
    }

    /// <summary>
    ///     Create a random currency-based barter scheme for an array of items
    /// </summary>
    /// <param name="offerWithChildren"> Items on offer </param>
    /// <param name="isPackOffer"> Is the barter scheme being created for a pack offer </param>
    /// <param name="multiplier"> What to multiply the resulting price by </param>
    /// <returns> Barter scheme for offer </returns>
    protected List<BarterScheme> CreateCurrencyBarterScheme(IEnumerable<Item> offerWithChildren, bool isPackOffer, double multiplier = 1)
    {
        var currency = ragfairServerHelper.GetDynamicOfferCurrency();
        var price = ragfairPriceService.GetDynamicOfferPriceForOffer(offerWithChildren, currency, isPackOffer) * multiplier;

        return [new BarterScheme { Count = price, Template = currency }];
    }
}
