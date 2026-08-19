using System.Reflection;
using HarmonyLib;
using SPTarkov.Common.Extensions;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Hideout;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Hideout;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.ScavCase;
using SPTarkov.Server.Core.Services;
using SPTarkov.Server.Core.Services.Commerce;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Generators;

/// <summary>
/// Scav case reward generation runs in <c>rust/spt-native</c> by default; the request builder
/// projects the live database and config into the native payload. The full 4.1.2 C# implementation
/// is retained below as the legacy path - it is the frozen mod contract (constructor and protected
/// members are apicompat-gated against the 4.1.2 baseline) and runs instead of the native path when
/// a Harmony patch on any frozen member is detected, when a mod substituted the generator, when the
/// frozen constructor built the instance or when ScavCaseConfig.ForceLegacyScavCaseGeneration is
/// set, so mod hooks fire with genuine baseline semantics.
/// </summary>
[Injectable]
public class ScavCaseRewardGenerator(
    ISptLogger<ScavCaseRewardGenerator> logger,
    HideoutTable hideoutTable,
    TemplateTable templateTable,
    RandomUtil randomUtil,
    ItemHelper itemHelper,
    PresetHelper presetHelper,
    RagfairPriceService ragfairPriceService,
    SeasonalEventService seasonalEventService,
    ItemFilterService itemFilterService,
    ServerLocalisationService localisationService,
    ScavCaseConfig scavCaseConfig,
    ICloner cloner
)
{
    protected List<TemplateItem> DbAmmoItemsCache = [];
    protected List<TemplateItem> DbItemsCache = [];

    private readonly ScavCaseNativeRequestBuilder? _requestBuilder;
    private readonly IReadOnlyList<SptMod>? _loadedMods;
    private readonly DbPublisher? _dbPublisher;

    /// <summary>
    ///     The frozen 4.1.2 constructor plus the native request builder. Additive and
    ///     apicompat-verified; without the epoch-protocol services, every native send carries the
    ///     views override.
    /// </summary>
    public ScavCaseRewardGenerator(
        ISptLogger<ScavCaseRewardGenerator> logger,
        HideoutTable hideoutTable,
        TemplateTable templateTable,
        RandomUtil randomUtil,
        ItemHelper itemHelper,
        PresetHelper presetHelper,
        RagfairPriceService ragfairPriceService,
        SeasonalEventService seasonalEventService,
        ItemFilterService itemFilterService,
        ServerLocalisationService localisationService,
        ScavCaseConfig scavCaseConfig,
        ICloner cloner,
        ScavCaseNativeRequestBuilder requestBuilder
    )
        : this(
            logger,
            hideoutTable,
            templateTable,
            randomUtil,
            itemHelper,
            presetHelper,
            ragfairPriceService,
            seasonalEventService,
            itemFilterService,
            localisationService,
            scavCaseConfig,
            cloner
        )
    {
        _requestBuilder = requestBuilder;
    }

    /// <summary>
    ///     The constructor the container uses: the builder overload above plus the epoch-protocol
    ///     services. Additive and apicompat-verified.
    /// </summary>
    public ScavCaseRewardGenerator(
        ISptLogger<ScavCaseRewardGenerator> logger,
        HideoutTable hideoutTable,
        TemplateTable templateTable,
        RandomUtil randomUtil,
        ItemHelper itemHelper,
        PresetHelper presetHelper,
        RagfairPriceService ragfairPriceService,
        SeasonalEventService seasonalEventService,
        ItemFilterService itemFilterService,
        ServerLocalisationService localisationService,
        ScavCaseConfig scavCaseConfig,
        ICloner cloner,
        ScavCaseNativeRequestBuilder requestBuilder,
        IReadOnlyList<SptMod> loadedMods,
        DbPublisher dbPublisher
    )
        : this(
            logger,
            hideoutTable,
            templateTable,
            randomUtil,
            itemHelper,
            presetHelper,
            ragfairPriceService,
            seasonalEventService,
            itemFilterService,
            localisationService,
            scavCaseConfig,
            cloner,
            requestBuilder
        )
    {
        _loadedMods = loadedMods;
        _dbPublisher = dbPublisher;
    }

    /// <summary>
    ///     Which implementation the most recent generation call ran - the spt-native path or the
    ///     retained 4.1.2 C# path. Test seam; also handy in a debugger.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     Test-only seed forwarded as <see cref="ScavCaseVarying.TestSeed"/> on every native
    ///     request.
    /// </summary>
    internal ulong? NativeTestSeed { get; set; }

    /// <summary>
    ///     Whether the most recent native send carried the C#-built views override rather than
    ///     naming a resident-DB epoch. Test seam.
    /// </summary>
    internal bool LastSendIncludedViewsOverride { get; private set; }

    /// <summary>
    ///     Whether an override-less send off the resident DB is ever allowed: the services exist,
    ///     the kill switch is off, and either no mods are loaded or the user vouched their mods
    ///     don't write tables directly. A generator built without the epoch-protocol services has
    ///     neither and always sends the override.
    /// </summary>
    private bool ResidentDbEligible()
    {
        return ResidentDbDispatch.Eligible(
            _dbPublisher,
            _loadedMods?.Count,
            scavCaseConfig.DisableNativeRequestCache,
            scavCaseConfig.TrustNativeRequestCacheWithMods
        );
    }

    /// <summary>
    ///     The 4.1.2 members a mod can Harmony-patch. Public, protected and protected-internal
    ///     methods declared on this class - exactly the surface the apicompat gate freezes, statics
    ///     included. <see cref="Generate"/> is excluded: it is the dispatcher now, and a patch on it
    ///     wraps whichever path runs. Everything else is never called natively, so a patch on one
    ///     would silently do nothing.
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. typeof(ScavCaseRewardGenerator)
            .GetMethods(
                BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
            )
            // Property accessors and operators are IsSpecialName; constructors are not returned at all
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            .Where(method => method.Name != nameof(Generate)),
    ];

    /// <summary>
    ///     The legacy path runs when the frozen 4.1.2 constructor built this instance (it has no
    ///     native seam to dispatch to), when forced by config, when any of the frozen 4.1.2 members
    ///     carries a live Harmony patch, or when a mod has substituted the generator itself - running
    ///     the retained C# implementation is the only way those hooks and replacements can take
    ///     effect with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_requestBuilder is null || scavCaseConfig.ForceLegacyScavCaseGeneration)
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
        return GetType() != typeof(ScavCaseRewardGenerator);
    }

    /// <summary>
    ///     Create an array of rewards that will be given to the player upon completing their scav case build
    /// </summary>
    /// <param name="recipeId">recipe of the scav case craft</param>
    /// <returns>Product array</returns>
    public IEnumerable<List<Item>> Generate(MongoId recipeId)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;

            return GenerateLegacy(recipeId);
        }

        LastPathTaken = LootGenerationPath.Native;

        var varying = _requestBuilder!.BuildVarying(recipeId, NativeTestSeed);
        List<List<Item>> result;
        if (!ResidentDbEligible())
        {
            LastSendIncludedViewsOverride = true;
            result = SptNative
                .GenerateScavCaseRewards(
                    new ScavCaseRewardsRequest
                    {
                        Epoch = 0,
                        ViewsOverride = _requestBuilder.BuildViewsOverride(),
                        Varying = varying,
                    }
                )
                .Result;
        }
        else
        {
            result = ResidentDbDispatch.Send(
                _dbPublisher!,
                epoch => SptNative.GenerateScavCaseRewards(new ScavCaseRewardsRequest { Epoch = epoch, Varying = varying }).Result
            );
            LastSendIncludedViewsOverride = false;
        }

        return result;
    }

    private IEnumerable<List<Item>> GenerateLegacy(MongoId recipeId)
    {
        CacheDbItems();

        // Get scavcase details from hideout/scavcase.json
        var scavCaseDetails = hideoutTable.Production.ScavRecipes.FirstOrDefault(r => r.Id == recipeId);
        var rewardItemCounts = GetScavCaseRewardCountsAndPrices(scavCaseDetails);

        // Get items that fit the price criteria as set by the scavCase config
        var commonPricedItems = GetFilteredItemsByPrice(DbItemsCache, rewardItemCounts.Common);
        var rarePricedItems = GetFilteredItemsByPrice(DbItemsCache, rewardItemCounts.Rare);
        var superRarePricedItems = GetFilteredItemsByPrice(DbItemsCache, rewardItemCounts.Superrare);

        // Get randomly picked items from each item collection, the count range of which is defined in hideout/scavcase.json
        var randomlyPickedCommonRewards = PickRandomRewards(commonPricedItems, rewardItemCounts.Common, RewardRarity.Common);

        var randomlyPickedRareRewards = PickRandomRewards(rarePricedItems, rewardItemCounts.Rare, RewardRarity.Rare);

        var randomlyPickedSuperRareRewards = PickRandomRewards(superRarePricedItems, rewardItemCounts.Superrare, RewardRarity.SuperRare);

        // Add randomised stack sizes to ammo and money rewards
        var commonRewards = RandomiseContainerItemRewards(randomlyPickedCommonRewards, RewardRarity.Common);
        var rareRewards = RandomiseContainerItemRewards(randomlyPickedRareRewards, RewardRarity.Rare);
        var superRareRewards = RandomiseContainerItemRewards(randomlyPickedSuperRareRewards, RewardRarity.SuperRare);

        var result = commonRewards.Concat(rareRewards).Concat(superRareRewards);

        return result;
    }

    /// <summary>
    ///     Get all db items that are not blacklisted in scavcase config or global blacklist
    ///     Store in class field
    /// </summary>
    protected void CacheDbItems()
    {
        // Get an array of seasonal items that should not be shown right now as seasonal event is not active
        var inactiveSeasonalItems = seasonalEventService.GetInactiveSeasonalEventItems();
        if (!DbItemsCache.Any())
        {
            DbItemsCache = templateTable
                .Items.Values.Where(item =>
                {
                    // Base "Item" item has no parent, ignore it
                    if (item.Parent == MongoId.Empty())
                    {
                        return false;
                    }

                    if (item.Type == "Node")
                    {
                        return false;
                    }

                    if (item.Properties.QuestItem ?? false)
                    {
                        return false;
                    }

                    // Skip item if item id is on blacklist
                    if (
                        item.Type != "Item"
                        || scavCaseConfig.RewardItemBlacklist.Contains(item.Id)
                        || itemFilterService.IsItemBlacklisted(item.Id)
                    )
                    {
                        return false;
                    }

                    // Globally reward-blacklisted
                    if (itemFilterService.IsItemRewardBlacklisted(item.Id))
                    {
                        return false;
                    }

                    if (!scavCaseConfig.AllowBossItemsAsRewards && itemFilterService.IsBossItem(item.Id))
                    {
                        return false;
                    }

                    // Skip item if parent id is blacklisted
                    if (itemHelper.IsOfBaseclasses(item.Id, scavCaseConfig.RewardItemParentBlacklist))
                    {
                        return false;
                    }

                    if (inactiveSeasonalItems.Contains(item.Id))
                    {
                        return false;
                    }

                    return true;
                })
                .ToList();
        }

        if (!DbAmmoItemsCache.Any())
        {
            DbAmmoItemsCache = templateTable
                .Items.Values.Where(item =>
                {
                    // Base "Item" item has no parent, ignore it
                    if (item.Parent == MongoId.Empty())
                    {
                        return false;
                    }

                    if (item.Type != "Item")
                    {
                        return false;
                    }

                    // Not ammo, skip
                    if (!itemHelper.IsOfBaseclass(item.Id, BaseClasses.AMMO))
                    {
                        return false;
                    }

                    // Skip item if item id is on blacklist
                    if (scavCaseConfig.RewardItemBlacklist.Contains(item.Id) || itemFilterService.IsItemBlacklisted(item.Id))
                    {
                        return false;
                    }

                    // Globally reward-blacklisted
                    if (itemFilterService.IsItemRewardBlacklisted(item.Id))
                    {
                        return false;
                    }

                    if (!scavCaseConfig.AllowBossItemsAsRewards && itemFilterService.IsBossItem(item.Id))
                    {
                        return false;
                    }

                    // Skip seasonal items
                    if (inactiveSeasonalItems.Contains(item.Id))
                    {
                        return false;
                    }

                    // Skip ammo that doesn't stack as high as value in config
                    if (item.Properties.StackMaxSize < scavCaseConfig.AmmoRewards.MinStackSize)
                    {
                        return false;
                    }

                    return true;
                })
                .ToList();
        }
    }

    /// <summary>
    ///     Pick a number of items to be rewards, the count is defined by the values in `itemFilters` param
    /// </summary>
    /// <param name="items">item pool to pick rewards from</param>
    /// <param name="itemFilters">how the rewards should be filtered down (by item count)</param>
    /// <param name="rarity">Rarity of reward</param>
    /// <returns></returns>
    protected List<TemplateItem> PickRandomRewards(List<TemplateItem> items, RewardCountAndPriceDetails itemFilters, string rarity)
    {
        List<TemplateItem> result = [];

        var rewardWasMoney = false;
        var rewardWasAmmo = false;
        var randomCount = randomUtil.GetInt((int)itemFilters.MinCount, (int)itemFilters.MaxCount);
        for (var i = 0; i < randomCount; i++)
        {
            if (RewardShouldBeMoney() && !rewardWasMoney)
            {
                // Only allow one reward to be money
                result.Add(GetRandomMoney());
                if (!scavCaseConfig.AllowMultipleMoneyRewardsPerRarity)
                {
                    rewardWasMoney = true;
                }
            }
            else if (RewardShouldBeAmmo() && !rewardWasAmmo)
            {
                // Only allow one reward to be ammo
                result.Add(GetRandomAmmo(rarity));
                if (!scavCaseConfig.AllowMultipleAmmoRewardsPerRarity)
                {
                    rewardWasAmmo = true;
                }
            }
            else
            {
                result.Add(randomUtil.GetArrayValue(items));
            }
        }

        return result;
    }

    /// <summary>
    ///     Choose if money should be a reward based on the moneyRewardChancePercent config chance in scavCaseConfig
    /// </summary>
    /// <returns>true if reward should be money</returns>
    protected bool RewardShouldBeMoney()
    {
        return randomUtil.GetChance100(scavCaseConfig.MoneyRewards.MoneyRewardChancePercent);
    }

    /// <summary>
    ///     Choose if ammo should be a reward based on the ammoRewardChancePercent config chance in scavCaseConfig
    /// </summary>
    /// <returns>true if reward should be ammo</returns>
    protected bool RewardShouldBeAmmo()
    {
        return randomUtil.GetChance100(scavCaseConfig.AmmoRewards.AmmoRewardChancePercent);
    }

    /// <summary>
    ///     Choose from rouble/dollar/euro at random
    /// </summary>
    protected TemplateItem GetRandomMoney()
    {
        List<TemplateItem> money = [];
        var items = templateTable.Items;
        money.Add(items[Money.ROUBLES]);
        money.Add(items[Money.EUROS]);
        money.Add(items[Money.DOLLARS]);
        money.Add(items[Money.GP]);

        return randomUtil.GetArrayValue(money);
    }

    /// <summary>
    ///     Get a random ammo from items.json that is not in the ammo blacklist AND inside the price range defined in scavcase.json config
    /// </summary>
    /// <param name="rarity">The rarity desired ammo reward is for</param>
    /// <returns>random ammo item from items.json</returns>
    protected TemplateItem GetRandomAmmo(string rarity)
    {
        var possibleAmmoPool = DbAmmoItemsCache.Where(ammo =>
        {
            // Is ammo handbook price between desired range
            var handbookPrice = ragfairPriceService.GetStaticPriceForItem(ammo.Id);
            if (
                scavCaseConfig.AmmoRewards.AmmoRewardValueRangeRub.TryGetValue(rarity, out var matchingAmmoRewardForRarity)
                && handbookPrice >= matchingAmmoRewardForRarity.Min
                && handbookPrice <= matchingAmmoRewardForRarity.Max
            )
            {
                return true;
            }

            return false;
        });

        if (!possibleAmmoPool.Any())
        {
            // Filtered pool is empty
            logger.Warning(localisationService.GetText("scavcase-no_cartridges_found_matching_price"));
        }

        // Get a random ammo and return it
        return randomUtil.GetArrayValue(possibleAmmoPool);
    }

    /// <summary>
    ///     Take all the rewards picked create the Product object array ready to return to calling code.
    ///     Also add a stack count to ammo and money
    /// </summary>
    /// <param name="rewardItems">items to convert</param>
    /// <param name="rarity">The rarity desired ammo reward is for</param>
    /// <returns>Product array</returns>
    protected List<List<Item>> RandomiseContainerItemRewards(IEnumerable<TemplateItem> rewardItems, string rarity)
    {
        // Each array is an item + children
        List<List<Item>> result = [];
        foreach (var rewardItemDb in rewardItems)
        {
            List<Item> resultItem =
            [
                new()
                {
                    Id = new MongoId(),
                    Template = rewardItemDb.Id,
                    Upd = null,
                },
            ];
            var rootItem = resultItem.FirstOrDefault();

            if (itemHelper.IsOfBaseclass(rewardItemDb.Id, BaseClasses.AMMO_BOX))
            {
                itemHelper.AddCartridgesToAmmoBox(resultItem, rewardItemDb);
            }
            // Armor or weapon = use default preset from globals.json
            else if (
                itemHelper.ArmorItemHasRemovableOrSoftInsertSlots(rewardItemDb.Id)
                || itemHelper.IsOfBaseclass(rewardItemDb.Id, BaseClasses.WEAPON)
            )
            {
                var preset = presetHelper.GetDefaultPreset(rewardItemDb.Id);
                if (preset is null)
                {
                    logger.Warning($"No preset for item: {rewardItemDb.Id} {rewardItemDb.Name}, skipping");

                    continue;
                }

                // Ensure preset has unique ids and is cloned so we don't alter the preset data stored in memory
                var presetAndMods = cloner.Clone(preset.Items).ReplaceIDs().ToList();
                presetAndMods.RemapRootItemId();

                resultItem = presetAndMods;
            }
            else if (itemHelper.IsOfBaseclasses(rewardItemDb.Id, [BaseClasses.AMMO, BaseClasses.MONEY]))
            {
                rootItem.Upd = new Upd { StackObjectsCount = GetRandomAmountRewardForScavCase(rewardItemDb, rarity) };
            }

            result.Add(resultItem);
        }

        return result;
    }

    /// <summary>
    /// </summary>
    /// <param name="dbItems">all items from the items.json</param>
    /// <param name="itemFilters">controls how the dbItems will be filtered and returned (handbook price)</param>
    /// <returns>filtered dbItems array</returns>
    protected List<TemplateItem> GetFilteredItemsByPrice(List<TemplateItem> dbItems, RewardCountAndPriceDetails itemFilters)
    {
        return dbItems
            .Where(item =>
            {
                var handbookPrice = ragfairPriceService.GetStaticPriceForItem(item.Id);
                if (handbookPrice >= itemFilters.MinPriceRub && handbookPrice <= itemFilters.MaxPriceRub)
                {
                    return true;
                }

                return false;
            })
            .ToList();
    }

    /// <summary>
    ///     Gathers the reward min and max count params for each reward quality level from config and scavcase.json into a single object
    /// </summary>
    /// <param name="scavCaseDetails">production.json/scavRecipes object</param>
    /// <returns>ScavCaseRewardCountsAndPrices object</returns>
    protected ScavCaseRewardCountsAndPrices GetScavCaseRewardCountsAndPrices(ScavRecipe scavCaseDetails)
    {
        return new ScavCaseRewardCountsAndPrices
        {
            // Create reward min/max counts for each type
            Common = new RewardCountAndPriceDetails
            {
                MinCount = scavCaseDetails.EndProducts.Common.Min,
                MaxCount = scavCaseDetails.EndProducts.Common.Max,
                MinPriceRub = scavCaseConfig.RewardItemValueRangeRub[RewardRarity.Common].Min,
                MaxPriceRub = scavCaseConfig.RewardItemValueRangeRub[RewardRarity.Common].Max,
            },
            Rare = new RewardCountAndPriceDetails
            {
                MinCount = scavCaseDetails.EndProducts.Rare.Min,
                MaxCount = scavCaseDetails.EndProducts.Rare.Max,
                MinPriceRub = scavCaseConfig.RewardItemValueRangeRub[RewardRarity.Rare].Min,
                MaxPriceRub = scavCaseConfig.RewardItemValueRangeRub[RewardRarity.Rare].Max,
            },
            Superrare = new RewardCountAndPriceDetails
            {
                MinCount = scavCaseDetails.EndProducts.Superrare.Min,
                MaxCount = scavCaseDetails.EndProducts.Superrare.Max,
                MinPriceRub = scavCaseConfig.RewardItemValueRangeRub[RewardRarity.SuperRare].Min,
                MaxPriceRub = scavCaseConfig.RewardItemValueRangeRub[RewardRarity.SuperRare].Max,
            },
        };
    }

    /// <summary>
    ///     Randomises the size of ammo and money stacks
    /// </summary>
    /// <param name="itemToCalculate">ammo or money item</param>
    /// <param name="rarity">rarity (common/rare/superrare)</param>
    /// <returns>value to set stack count to</returns>
    protected int GetRandomAmountRewardForScavCase(TemplateItem itemToCalculate, string rarity)
    {
        var parentId = itemToCalculate.Parent;

        if (parentId == BaseClasses.AMMO)
        {
            return GetRandomisedAmmoRewardStackSize(itemToCalculate);
        }
        else if (parentId == BaseClasses.MONEY)
        {
            return GetRandomisedMoneyRewardStackSize(itemToCalculate, rarity);
        }
        else
        {
            return 1;
        }
    }

    /// <summary>
    ///     Randomises the size of ammo stacks
    /// </summary>
    /// <param name="itemToCalculate">ammo or money item</param>
    /// <returns>value to set stack count to</returns>
    protected int GetRandomisedAmmoRewardStackSize(TemplateItem itemToCalculate)
    {
        return randomUtil.GetInt(scavCaseConfig.AmmoRewards.MinStackSize, itemToCalculate.Properties.StackMaxSize ?? 0);
    }

    /// <summary>
    ///     Randomises the size of money stacks
    /// </summary>
    /// <param name="itemToCalculate">ammo or money item</param>
    /// <param name="rarity">rarity (common/rare/superrare)</param>
    /// <returns>value to set stack count to</returns>
    protected int GetRandomisedMoneyRewardStackSize(TemplateItem itemToCalculate, string rarity)
    {
        var id = itemToCalculate.Id;

        if (id == Money.ROUBLES)
        {
            return randomUtil.GetInt(
                scavCaseConfig.MoneyRewards.RubCount.GetByJsonProperty<MinMax<int>>(rarity).Min,
                scavCaseConfig.MoneyRewards.RubCount.GetByJsonProperty<MinMax<int>>(rarity).Max
            );
        }
        else if (id == Money.EUROS)
        {
            return randomUtil.GetInt(
                scavCaseConfig.MoneyRewards.EurCount.GetByJsonProperty<MinMax<int>>(rarity).Min,
                scavCaseConfig.MoneyRewards.EurCount.GetByJsonProperty<MinMax<int>>(rarity).Max
            );
        }
        else if (id == Money.DOLLARS)
        {
            return randomUtil.GetInt(
                scavCaseConfig.MoneyRewards.UsdCount.GetByJsonProperty<MinMax<int>>(rarity).Min,
                scavCaseConfig.MoneyRewards.UsdCount.GetByJsonProperty<MinMax<int>>(rarity).Max
            );
        }
        else if (id == Money.GP)
        {
            return randomUtil.GetInt(
                scavCaseConfig.MoneyRewards.GpCount.GetByJsonProperty<MinMax<int>>(rarity).Min,
                scavCaseConfig.MoneyRewards.GpCount.GetByJsonProperty<MinMax<int>>(rarity).Max
            );
        }
        else
        {
            return 1;
        }
    }
}

public record RewardRarity
{
    public const string Common = "common";
    public const string Rare = "rare";
    public const string SuperRare = "superrare";
}
