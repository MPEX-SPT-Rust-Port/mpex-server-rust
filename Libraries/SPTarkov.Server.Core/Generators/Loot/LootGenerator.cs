using System.Reflection;
using System.Text.Json.Serialization;
using HarmonyLib;
using Microsoft.Extensions.Logging;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Services;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Services;
using SPTarkov.Server.Core.Services.Commerce;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Generators.Loot;

/// <summary>
/// Reward loot generation runs in <c>rust/spt-native/src/loot/loot_generator.rs</c> by default; the
/// entry points project the live database and config into the native payload and replay the log
/// lines the native side collected. The full 4.1.2 C# implementation is retained below as the
/// legacy path - it is the frozen mod contract (constructor, protected members and DTOs are
/// apicompat-gated against the 4.1.2 baseline) and runs instead of the native path when a Harmony
/// patch on any protected member is detected or LocationConfig.ForceLegacyLootGeneration is set, so
/// mod hooks fire with genuine baseline semantics.
///
/// A mod that constructs this class through the frozen 11 parameter constructor gets no
/// LocationConfig, so only the config flag check is skipped - hook detection still works.
/// </summary>
[Injectable]
public class LootGenerator(
    ISptLogger<LootGenerator> logger,
    TemplateTable templateTable,
    RandomUtil randomUtil,
    ItemHelper itemHelper,
    PresetHelper presetHelper,
    ItemFilterService itemFilterService,
    ServerLocalisationService serverLocalisationService,
    WeightedRandomHelper weightedRandomHelper,
    RagfairLinkedItemService ragfairLinkedItemService,
    SeasonalEventService seasonalEventService,
    ICloner cloner
)
{
    private readonly LocationConfig? _locationConfig;

    /// <summary>
    ///     The constructor the container uses: the frozen 4.1.2 one plus the config that carries the
    ///     legacy path flag. Additive and apicompat-verified: the gate passed on all six contract
    ///     assemblies for this change.
    /// </summary>
    public LootGenerator(
        ISptLogger<LootGenerator> logger,
        TemplateTable templateTable,
        RandomUtil randomUtil,
        ItemHelper itemHelper,
        PresetHelper presetHelper,
        ItemFilterService itemFilterService,
        ServerLocalisationService serverLocalisationService,
        WeightedRandomHelper weightedRandomHelper,
        RagfairLinkedItemService ragfairLinkedItemService,
        SeasonalEventService seasonalEventService,
        ICloner cloner,
        LocationConfig locationConfig
    )
        : this(
            logger,
            templateTable,
            randomUtil,
            itemHelper,
            presetHelper,
            itemFilterService,
            serverLocalisationService,
            weightedRandomHelper,
            ragfairLinkedItemService,
            seasonalEventService,
            cloner
        )
    {
        _locationConfig = locationConfig;
    }

    /// <summary>
    ///     Which implementation the most recent generation call ran - the spt-native path or the
    ///     retained 4.1.2 C# path. Test seam; also handy in a debugger.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     Test-only seed forwarded as <see cref="RewardLootDb.TestSeed"/> on every native request.
    /// </summary>
    internal ulong? NativeTestSeed { get; set; }

    /// <summary>
    ///     The 4.1.2 members a mod can Harmony-patch. Protected and declared on this class - exactly
    ///     the surface the apicompat gate freezes. Computed once; patches come and go per call, so
    ///     the check itself does not.
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. typeof(LootGenerator)
            .GetMethods(BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.DeclaredOnly)
            .Where(method => method.IsFamily),
    ];

    /// <summary>
    ///     The legacy path runs when forced by config, or when any of the frozen 4.1.2 members
    ///     carries a live Harmony patch - running the retained C# implementation is the only way
    ///     those hooks can fire with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_locationConfig?.ForceLegacyLootGeneration == true)
        {
            return true;
        }

        return _hookableMembers.Any(member =>
            Harmony.GetPatchInfo(member) is { } patches
            && (patches.Prefixes.Count > 0 || patches.Postfixes.Count > 0 || patches.Transpilers.Count > 0 || patches.Finalizers.Count > 0)
        );
    }

    /// <summary>
    ///     Everything all four native entry points read from the live database and services, built
    ///     fresh per call so a mod that swaps an item or blacklists one at runtime is picked up.
    /// </summary>
    /// <param name="defaultPresets">
    ///     <c>PresetHelper.GetDefaultPresets().Values</c>, order preserved - only random loot draws
    ///     from it, so the other three envelopes pass an empty list
    /// </param>
    /// <param name="defaultPresetsByTpl">
    ///     The tpl to default-preset map, which is not the same method at every call site:
    ///     <c>GetDefaultPresetsByTplKey()</c> for forced loot, <c>GetDefaultPresetByTpl()</c> for the
    ///     sealed case and the reward container, and unread by random loot
    /// </param>
    private RewardLootDb BuildRewardLootDb(List<PresetView> defaultPresets, Dictionary<MongoId, PresetView> defaultPresetsByTpl)
    {
        return new RewardLootDb
        {
            ItemsView = PayloadProjection.BuildItemsView(templateTable.Items),
            DefaultPresets = defaultPresets,
            DefaultPresetsByTpl = defaultPresetsByTpl,
            // Two different collections, mirroring the two C# call sites: the sealed container
            // filters read the cache a mod can add to at runtime, the reward pool reads the config
            // list. Equal until a mod calls AddItemToBlacklistCache
            GlobalBlacklist = itemFilterService.GetItemBlacklistCache(),
            ConfigBlacklist = itemFilterService.GetBlacklistedItems(),
            RewardItemBlacklist = itemFilterService.GetItemRewardBlacklist(),
            RewardBaseTypeBlacklist = itemFilterService.GetItemRewardBaseTypeBlacklist(),
            BossItems = itemFilterService.GetBossItems(),
            InactiveSeasonalItems = seasonalEventService.GetInactiveSeasonalEventItems(),
            TestSeed = NativeTestSeed,
        };
    }

    private static PresetView ToPresetView(Preset preset)
    {
        return new PresetView
        {
            Items = preset.Items,
            Id = preset.Id,
            Name = preset.Name,
            Encyclopedia = preset.Encyclopedia,
        };
    }

    private static Dictionary<MongoId, PresetView> ToPresetViews(Dictionary<MongoId, Preset> presets)
    {
        return presets.ToDictionary(preset => preset.Key, preset => ToPresetView(preset.Value));
    }

    /// <summary>
    ///     Generate a list of items based on configuration options parameter
    /// </summary>
    /// <param name="options">parameters to adjust how loot is generated</param>
    /// <returns>An array of loot items</returns>
    public IEnumerable<List<Item>> CreateRandomLoot(LootRequest options)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;
            return CreateRandomLootLegacy(options);
        }

        LastPathTaken = LootGenerationPath.Native;

        // The tpl keyed preset map is not read by this export, so it is not built
        var db = BuildRewardLootDb([.. presetHelper.GetDefaultPresets().Values.Select(ToPresetView)], []);

        var result = SptNative.CreateRandomLoot(
            new CreateRandomLootRequest
            {
                ItemsView = db.ItemsView,
                DefaultPresets = db.DefaultPresets,
                DefaultPresetsByTpl = db.DefaultPresetsByTpl,
                GlobalBlacklist = db.GlobalBlacklist,
                ConfigBlacklist = db.ConfigBlacklist,
                RewardItemBlacklist = db.RewardItemBlacklist,
                RewardBaseTypeBlacklist = db.RewardBaseTypeBlacklist,
                BossItems = db.BossItems,
                InactiveSeasonalItems = db.InactiveSeasonalItems,
                TestSeed = db.TestSeed,
                LootRequest = options,
            }
        );

        return result.Items;
    }

    private IEnumerable<List<Item>> CreateRandomLootLegacy(LootRequest options)
    {
        var result = new List<List<Item>>();

        var itemTypeCounts = InitItemLimitCounter(options.ItemLimits);

        // Handle sealed weapon containers
        var sealedWeaponCrateCount = randomUtil.GetInt(options.WeaponCrateCount.Min, options.WeaponCrateCount.Max);
        if (sealedWeaponCrateCount > 0)
        {
            // Get list of all sealed containers from db - they're all the same, just for flavor
            var itemsDb = templateTable.Items.Values;
            var sealedWeaponContainerPool = itemsDb.Where(item => item.Name.Contains("event_container_airdrop"));

            for (var index = 0; index < sealedWeaponCrateCount; index++)
            {
                // Choose one at random + add to results array
                var chosenSealedContainer = randomUtil.GetArrayValue(sealedWeaponContainerPool);
                result.Add([
                    new Item
                    {
                        Id = new MongoId(),
                        Template = chosenSealedContainer.Id,
                        Upd = new Upd { StackObjectsCount = 1, SpawnedInSession = true },
                    },
                ]);
            }
        }

        // Get items from items.json that have a type of item + not in global blacklist + base type is in whitelist
        var rewardPoolResults = GetItemRewardPool(
            options.ItemBlacklist,
            options.ItemTypeWhitelist,
            options.UseRewardItemBlacklist.GetValueOrDefault(false),
            options.AllowBossItems.GetValueOrDefault(false),
            options.BlockSeasonalItemsOutOfSeason.GetValueOrDefault(false)
        );

        // Pool has items we could add as loot, proceed
        if (rewardPoolResults.ItemPool.Any())
        {
            var randomisedItemCount = randomUtil.GetInt(options.ItemCount.Min, options.ItemCount.Max);
            for (var index = 0; index < randomisedItemCount; index++)
            {
                if (!FindAndAddRandomItemToLoot(rewardPoolResults.ItemPool, itemTypeCounts, options, result))
                // Failed to add, reduce index so we get another attempt
                {
                    index--;
                }
            }
        }

        var globalDefaultPresets = presetHelper.GetDefaultPresets().Values;

        // Filter default presets to just weapons
        var randomisedWeaponPresetCount = randomUtil.GetInt(options.WeaponPresetCount.Min, options.WeaponPresetCount.Max);
        if (randomisedWeaponPresetCount > 0)
        {
            var weaponDefaultPresets = globalDefaultPresets.Where(preset =>
                itemHelper.IsOfBaseclass(preset.Encyclopedia.Value, BaseClasses.WEAPON)
            );

            if (weaponDefaultPresets.Any())
            {
                for (var index = 0; index < randomisedWeaponPresetCount; index++)
                {
                    if (!FindAndAddRandomPresetToLoot(weaponDefaultPresets, itemTypeCounts, rewardPoolResults.Blacklist, result))
                    // Failed to add, reduce index so we get another attempt
                    {
                        index--;
                    }
                }
            }
        }

        // Filter default presets to just armors and then filter again by protection level
        var randomisedArmorPresetCount = randomUtil.GetInt(options.ArmorPresetCount.Min, options.ArmorPresetCount.Max);
        if (randomisedArmorPresetCount > 0)
        {
            var armorDefaultPresets = globalDefaultPresets.Where(preset => itemHelper.ArmorItemCanHoldMods(preset.Encyclopedia.Value));
            var levelFilteredArmorPresets = armorDefaultPresets.Where(armor => IsArmorOfDesiredProtectionLevel(armor, options));

            // Add some armors to rewards
            if (levelFilteredArmorPresets.Any())
            {
                for (var index = 0; index < randomisedArmorPresetCount; index++)
                {
                    if (!FindAndAddRandomPresetToLoot(levelFilteredArmorPresets, itemTypeCounts, rewardPoolResults.Blacklist, result))
                    // Failed to add, reduce index so we get another attempt
                    {
                        index--;
                    }
                }
            }
        }

        return result;
    }

    /// <summary>
    ///     Generate An array of items
    ///     TODO - handle ammo packs
    /// </summary>
    /// <param name="forcedLootToAdd">Dictionary of item tpls with minmax values</param>
    /// <returns>Array of Item</returns>
    public List<List<Item>> CreateForcedLoot(Dictionary<MongoId, MinMax<int>> forcedLootToAdd)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;
            return CreateForcedLootLegacy(forcedLootToAdd);
        }

        LastPathTaken = LootGenerationPath.Native;

        // The preset list is not read by this export, so it is not built
        var db = BuildRewardLootDb([], ToPresetViews(presetHelper.GetDefaultPresetsByTplKey()));

        var result = SptNative.CreateForcedLoot(
            new CreateForcedLootRequest
            {
                ItemsView = db.ItemsView,
                DefaultPresets = db.DefaultPresets,
                DefaultPresetsByTpl = db.DefaultPresetsByTpl,
                GlobalBlacklist = db.GlobalBlacklist,
                ConfigBlacklist = db.ConfigBlacklist,
                RewardItemBlacklist = db.RewardItemBlacklist,
                RewardBaseTypeBlacklist = db.RewardBaseTypeBlacklist,
                BossItems = db.BossItems,
                InactiveSeasonalItems = db.InactiveSeasonalItems,
                TestSeed = db.TestSeed,
                ForcedLoot = forcedLootToAdd,
            }
        );

        return result.Items;
    }

    private List<List<Item>> CreateForcedLootLegacy(Dictionary<MongoId, MinMax<int>> forcedLootToAdd)
    {
        var result = new List<List<Item>>();

        var defaultPresets = presetHelper.GetDefaultPresetsByTplKey();
        foreach (var (itemTpl, details) in forcedLootToAdd)
        {
            // How many of this item we want
            var randomisedItemCount = randomUtil.GetInt(details.Min, details.Max);

            // Check if item being added has a preset and use that instead
            if (defaultPresets.ContainsKey(itemTpl))
            {
                // Use default preset data
                if (defaultPresets.TryGetValue(itemTpl, out var preset))
                {
                    // Add the chosen preset as many times as randomisedItemCount states
                    for (var i = 0; i < randomisedItemCount; i++)
                    {
                        // Clone preset and alter Ids to be unique
                        var presetWithUniqueIdsClone = cloner.Clone(preset.Items).ReplaceIDs().ToList();

                        // Add to results
                        result.Add(presetWithUniqueIdsClone);
                    }
                }

                continue;
            }

            // Non-preset item to be added
            var newLootItem = new Item
            {
                Id = new MongoId(),
                Template = itemTpl,
                Upd = new Upd { StackObjectsCount = randomisedItemCount, SpawnedInSession = true },
            };
            var splitResults = itemHelper.SplitStack(newLootItem);
            foreach (var splitItem in splitResults)
            {
                // Add as separate lists
                result.Add([splitItem]);
            }
        }

        return result;
    }

    /// <summary>
    ///     Get pool of items from item db that fit passed in param criteria
    /// </summary>
    /// <param name="itemTplBlacklist">Prevent these items</param>
    /// <param name="itemTypeWhitelist">Only allow these items</param>
    /// <param name="useRewardItemBlacklist">Should item.json reward item config be used</param>
    /// <param name="allowBossItems">Should boss items be allowed in result</param>
    /// <param name="blockSeasonalItemsOutOfSeason">Prevent seasonal items appearing outside their defined season</param>
    /// <returns>results of filtering + blacklist used</returns>
    protected ItemRewardPoolResults GetItemRewardPool(
        HashSet<MongoId> itemTplBlacklist,
        HashSet<MongoId> itemTypeWhitelist,
        bool useRewardItemBlacklist,
        bool allowBossItems,
        bool blockSeasonalItemsOutOfSeason
    )
    {
        var itemsDb = templateTable.Items.Values;
        var itemBlacklist = new HashSet<MongoId>();
        itemBlacklist.UnionWith([.. itemFilterService.GetBlacklistedItems(), .. itemTplBlacklist]);

        if (useRewardItemBlacklist)
        {
            var rewardItemBlacklist = itemFilterService.GetItemRewardBlacklist();

            // Get all items that match the blacklisted types and fold into item blacklist
            var itemTypeBlacklist = itemFilterService.GetItemRewardBaseTypeBlacklist();
            var itemsMatchingTypeBlacklist = itemsDb
                .Where(templateItem => !string.IsNullOrEmpty(templateItem.Parent)) // Ignore items without parents
                .Where(templateItem => itemHelper.IsOfBaseclasses(templateItem.Parent, itemTypeBlacklist))
                .Select(templateItem => templateItem.Id);

            itemBlacklist.UnionWith([.. rewardItemBlacklist, .. itemsMatchingTypeBlacklist]);
        }

        if (!allowBossItems)
        {
            itemBlacklist.UnionWith(itemFilterService.GetBossItems());
        }

        if (blockSeasonalItemsOutOfSeason)
        {
            itemBlacklist.UnionWith(seasonalEventService.GetInactiveSeasonalEventItems());
        }

        var items = itemsDb.Where(item =>
            !itemBlacklist.Contains(item.Id)
            && string.Equals(item.Type, "item", StringComparison.OrdinalIgnoreCase)
            && !item.Properties.QuestItem.GetValueOrDefault(false)
            && itemTypeWhitelist.Contains(item.Parent)
        );

        return new ItemRewardPoolResults { ItemPool = items, Blacklist = itemBlacklist };
    }

    /// <summary>
    ///     Filter armor items by their front plates protection level - top if it's a helmet
    /// </summary>
    /// <param name="armor">Armor preset to check</param>
    /// <param name="options">Loot request options - armor level etc</param>
    /// <returns>True if item has desired armor level</returns>
    protected bool IsArmorOfDesiredProtectionLevel(Preset armor, LootRequest options)
    {
        string[] relevantSlots = ["front_plate", "helmet_top", "soft_armor_front"];
        foreach (var slotId in relevantSlots)
        {
            var armorItem = armor.Items.FirstOrDefault(item => string.Equals(item?.SlotId, slotId));
            if (armorItem is null)
            {
                continue;
            }

            var armorDetails = itemHelper.GetItem(armorItem.Template).Value;
            var armorClass = armorDetails.Properties.ArmorClass;

            return options.ArmorLevelWhitelist.Contains(armorClass.Value);
        }

        return false;
    }

    /// <summary>
    ///     Construct item limit record to hold max and current item count for each item type
    /// </summary>
    /// <param name="limits">limits as defined in config</param>
    /// <returns>record, key: item tplId, value: current/max item count allowed</returns>
    protected Dictionary<MongoId, ItemLimit> InitItemLimitCounter(Dictionary<MongoId, int> limits)
    {
        var itemTypeCounts = new Dictionary<MongoId, ItemLimit>();
        foreach (var itemTypeId in limits)
        {
            itemTypeCounts[itemTypeId.Key] = new ItemLimit { Current = 0, Max = limits[itemTypeId.Key] };
        }

        return itemTypeCounts;
    }

    /// <summary>
    ///     Find a random item in items.json and add to result array
    /// </summary>
    /// <param name="items">items to choose from</param>
    /// <param name="itemTypeCounts">item limit counts</param>
    /// <param name="options">item filters</param>
    /// <param name="result">array to add found item to</param>
    /// <returns>true if item was valid and added to pool</returns>
    protected bool FindAndAddRandomItemToLoot(
        IEnumerable<TemplateItem> items,
        Dictionary<MongoId, ItemLimit> itemTypeCounts,
        LootRequest options,
        List<List<Item>> result
    )
    {
        var randomItem = randomUtil.GetArrayValue(items);

        var itemLimitCount = itemTypeCounts.TryGetValue(randomItem.Parent, out var randomItemLimitCount);
        if (!itemLimitCount && randomItemLimitCount?.Current > randomItemLimitCount?.Max)
        {
            return false;
        }

        // Skip armors as they need to come from presets
        if (itemHelper.ArmorItemCanHoldMods(randomItem.Id))
        {
            return false;
        }

        var newLootItem = new Item
        {
            Id = new MongoId(),
            Template = randomItem.Id,
            Upd = new Upd { StackObjectsCount = 1, SpawnedInSession = true },
        };

        // Special case - handle items that need a stackcount > 1
        if (randomItem.Properties.StackMaxSize > 1)
        {
            newLootItem.Upd.StackObjectsCount = GetRandomisedStackCount(randomItem, options);
        }

        newLootItem.Template = randomItem.Id;
        result.Add([newLootItem]);

        if (randomItemLimitCount is not null)
        // Increment item count as it's in limit array
        {
            randomItemLimitCount.Current++;
        }

        // Item added okay
        return true;
    }

    /// <summary>
    ///     Get a randomised stack count for an item between its StackMinRandom and StackMaxSize values
    /// </summary>
    /// <param name="item">item to get stack count of</param>
    /// <param name="options">loot options</param>
    /// <returns>stack count</returns>
    protected int GetRandomisedStackCount(TemplateItem item, LootRequest options)
    {
        var min = item.Properties.StackMinRandom;
        var max = item.Properties.StackMaxSize;

        if (options.ItemStackLimits.TryGetValue(item.Id, out var itemLimits))
        {
            min = itemLimits.Min;
            max = itemLimits.Max;
        }

        return randomUtil.GetInt(min ?? 1, max ?? 1);
    }

    /// <summary>
    ///     Find a random item in items.json and add to result list
    /// </summary>
    /// <param name="presetPool">Presets to choose from</param>
    /// <param name="itemTypeCounts">Item limit counts</param>
    /// <param name="itemBlacklist">Items to skip</param>
    /// <param name="result">List to add chosen preset to</param>
    /// <returns>true if preset was valid and added to pool</returns>
    protected bool FindAndAddRandomPresetToLoot(
        IEnumerable<Preset> presetPool,
        Dictionary<MongoId, ItemLimit> itemTypeCounts,
        HashSet<MongoId> itemBlacklist,
        List<List<Item>> result
    )
    {
        if (!presetPool.Any())
        {
            logger.Warning(serverLocalisationService.GetText("loot-preset_pool_is_empty"));

            return false;
        }

        // Choose random preset and get details from item db using encyclopedia value (encyclopedia === tplId)
        var chosenPreset = randomUtil.GetArrayValue(presetPool);

        // No `_encyclopedia` property, not possible to reliably get root item tpl
        if (chosenPreset?.Encyclopedia is null)
        {
            if (logger.IsLogEnabled(LogLevel.Debug))
            {
                logger.Warning(serverLocalisationService.GetText("loot-chosen_preset_missing_encyclopedia_value", chosenPreset?.Id));
            }

            return false;
        }

        // Get preset root item db details via its `_encyclopedia` property
        var itemDbDetails = itemHelper.GetItem(chosenPreset.Encyclopedia.Value);
        if (!itemDbDetails.Key)
        {
            if (logger.IsLogEnabled(LogLevel.Debug))
            {
                logger.Debug($"$Unable to find preset with tpl: {chosenPreset.Encyclopedia}, skipping");
            }

            return false;
        }

        // Skip preset if root item is blacklisted
        if (itemBlacklist.Contains(chosenPreset.Items.FirstOrDefault().Template))
        {
            return false;
        }

        // Some custom mod items lack a parent property
        if (itemDbDetails.Value?.Parent is null)
        {
            logger.Error(serverLocalisationService.GetText("loot-item_missing_parentid", itemDbDetails.Value?.Name));

            return false;
        }

        // Check chosen preset hasn't exceeded spawn limit
        var hasItemLimitCount = itemTypeCounts.TryGetValue(itemDbDetails.Value.Parent, out var itemLimitCount);
        if (!hasItemLimitCount && itemLimitCount?.Current > itemLimitCount?.Max)
        {
            return false;
        }

        var presetAndModsClone = cloner.Clone(chosenPreset.Items).ReplaceIDs().ToList();
        presetAndModsClone.RemapRootItemId();

        itemHelper.SetFoundInRaid(presetAndModsClone);

        // Add chosen preset tpl to result array
        result.Add(presetAndModsClone);

        if (itemLimitCount is not null)
        // Increment item count as item has been chosen and its inside itemLimitCount dictionary
        {
            itemLimitCount.Current++;
        }

        // Item added okay
        return true;
    }

    /// <summary>
    ///     Sealed weapon containers have a weapon + associated mods inside them + assortment of other things (food/meds)
    /// </summary>
    /// <param name="containerSettings">sealed weapon container settings</param>
    /// <returns>List of items with children lists</returns>
    public List<List<Item>> GetSealedWeaponCaseLoot(SealedAirdropContainerSettings containerSettings)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;
            return GetSealedWeaponCaseLootLegacy(containerSettings);
        }

        LastPathTaken = LootGenerationPath.Native;

        // Any of the weighted weapons can be drawn, so every one of them brings its presets and the
        // items that fit it. The preset list is not read by this export, so it is not built
        var db = BuildRewardLootDb([], ToPresetViews(presetHelper.GetDefaultPresetByTpl()));
        var presetsByTpl = new Dictionary<MongoId, List<PresetView>>(containerSettings.WeaponRewardWeight.Count);
        var linkedItems = new Dictionary<MongoId, List<MongoId>>(containerSettings.WeaponRewardWeight.Count);

        foreach (var weaponTpl in containerSettings.WeaponRewardWeight.Keys)
        {
            presetsByTpl[weaponTpl] = [.. (presetHelper.GetPresets(weaponTpl) ?? []).Select(ToPresetView)];
            linkedItems[weaponTpl] = [.. ragfairLinkedItemService.GetLinkedDbItems(weaponTpl).Select(item => item.Id)];
        }

        var result = SptNative.GetSealedWeaponCaseLoot(
            new SealedWeaponCaseRequest
            {
                ItemsView = db.ItemsView,
                DefaultPresets = db.DefaultPresets,
                DefaultPresetsByTpl = db.DefaultPresetsByTpl,
                GlobalBlacklist = db.GlobalBlacklist,
                ConfigBlacklist = db.ConfigBlacklist,
                RewardItemBlacklist = db.RewardItemBlacklist,
                RewardBaseTypeBlacklist = db.RewardBaseTypeBlacklist,
                BossItems = db.BossItems,
                InactiveSeasonalItems = db.InactiveSeasonalItems,
                TestSeed = db.TestSeed,
                ContainerSettings = containerSettings,
                PresetsByTpl = presetsByTpl,
                LinkedItems = linkedItems,
            }
        );

        return result.Items;
    }

    private List<List<Item>> GetSealedWeaponCaseLootLegacy(SealedAirdropContainerSettings containerSettings)
    {
        List<List<Item>> itemsToReturn = [];

        // Choose a weapon to give to the player (weighted)
        var chosenWeaponTpl = weightedRandomHelper.GetWeightedValue(containerSettings.WeaponRewardWeight);

        // Get itemDb details of weapon
        var weaponDetailsDb = itemHelper.GetItem(chosenWeaponTpl);
        if (!weaponDetailsDb.Key)
        {
            logger.Error(serverLocalisationService.GetText("loot-non_item_picked_as_sealed_weapon_crate_reward", chosenWeaponTpl));

            return itemsToReturn;
        }

        // Get weapon preset - default or choose a random one from globals.json preset pool
        var chosenWeaponPreset = containerSettings.DefaultPresetsOnly
            ? presetHelper.GetDefaultPreset(chosenWeaponTpl)
            : randomUtil.GetArrayValue(presetHelper.GetPresets(chosenWeaponTpl));

        // No default preset found for weapon, choose a random one
        if (chosenWeaponPreset is null)
        {
            logger.Warning(serverLocalisationService.GetText("loot-default_preset_not_found_using_random", chosenWeaponTpl));
            chosenWeaponPreset = randomUtil.GetArrayValue(presetHelper.GetPresets(chosenWeaponTpl));
        }

        // Clean up Ids to ensure they're all unique and prevent collisions
        var presetAndModsClone = cloner.Clone(chosenWeaponPreset.Items).ReplaceIDs().ToList();
        presetAndModsClone.RemapRootItemId();

        // Add preset to return object
        itemsToReturn.Add(presetAndModsClone);

        // Get a random collection of weapon mods related to chosen weapon and add them to result array
        var linkedItemsToWeapon = ragfairLinkedItemService.GetLinkedDbItems(chosenWeaponTpl);
        itemsToReturn.AddRange(GetSealedContainerWeaponModRewards(containerSettings, linkedItemsToWeapon, chosenWeaponPreset));

        // Handle non-weapon mod reward types
        itemsToReturn.AddRange(GetSealedContainerNonWeaponModRewards(containerSettings, weaponDetailsDb.Value));

        return itemsToReturn;
    }

    /// <summary>
    ///     Get non-weapon mod rewards for a sealed container
    /// </summary>
    /// <param name="containerSettings">Sealed weapon container settings</param>
    /// <param name="weaponDetailsDb">Details for the weapon to reward player</param>
    /// <returns>List of item with children lists</returns>
    protected List<List<Item>> GetSealedContainerNonWeaponModRewards(
        SealedAirdropContainerSettings containerSettings,
        TemplateItem weaponDetailsDb
    )
    {
        List<List<Item>> rewards = [];

        foreach (var (rewardKey, settings) in containerSettings.RewardTypeLimits)
        {
            var rewardCount = randomUtil.GetInt(settings.Min, settings.Max);
            if (rewardCount == 0)
            {
                continue;
            }

            // Edge case - ammo boxes
            if (rewardKey == BaseClasses.AMMO_BOX)
            {
                // Get ammo boxes from db
                var ammoBoxesDetails = containerSettings.AmmoBoxWhitelist.Select(tpl =>
                {
                    var itemDetails = itemHelper.GetItem(tpl);
                    return itemDetails.Value;
                });

                // Need to find boxes that matches weapons caliber
                var weaponCaliber = weaponDetailsDb.Properties.AmmoCaliber;
                var ammoBoxesMatchingCaliber = ammoBoxesDetails.Where(x => x.Properties.AmmoCaliber == weaponCaliber);
                if (!ammoBoxesMatchingCaliber.Any())
                {
                    if (logger.IsLogEnabled(LogLevel.Debug))
                    {
                        logger.Debug($"No ammo box with caliber {weaponCaliber} found, skipping");
                    }

                    continue;
                }

                for (var index = 0; index < rewardCount; index++)
                {
                    var chosenAmmoBox = randomUtil.GetArrayValue(ammoBoxesMatchingCaliber);
                    var ammoBoxReward = new List<Item>
                    {
                        new() { Id = new MongoId(), Template = chosenAmmoBox.Id },
                    };
                    itemHelper.AddCartridgesToAmmoBox(ammoBoxReward, chosenAmmoBox);
                    rewards.Add(ammoBoxReward);
                }

                continue;
            }

            // Get all items of the desired type + not quest items + not globally blacklisted
            var rewardItemPool = templateTable.Items.Values.Where(item =>
                item.Parent == rewardKey
                && string.Equals(item.Type, "item", StringComparison.OrdinalIgnoreCase)
                && itemFilterService.IsItemBlacklisted(item.Id)
                && !(containerSettings.AllowBossItems || itemFilterService.IsBossItem(item.Id))
                && item.Properties.QuestItem is null
            );

            if (!rewardItemPool.Any())
            {
                if (logger.IsLogEnabled(LogLevel.Debug))
                {
                    logger.Debug($"No items with base type of {rewardKey} found, skipping");
                }

                continue;
            }

            for (var index = 0; index < rewardCount; index++)
            {
                // Choose a random item from pool
                var chosenRewardItem = randomUtil.GetArrayValue(rewardItemPool);
                var rewardItem = new List<Item>
                {
                    new() { Id = new MongoId(), Template = chosenRewardItem.Id },
                };

                rewards.Add(rewardItem);
            }
        }

        return rewards;
    }

    /// <summary>
    ///     Iterate over the container weaponModRewardLimits settings and create a list of weapon mods to reward player
    /// </summary>
    /// <param name="containerSettings">Sealed weapon container settings</param>
    /// <param name="linkedItemsToWeapon">All items that can be attached/inserted into weapon</param>
    /// <param name="chosenWeaponPreset">The weapon preset given to player as reward</param>
    /// <returns>List of item with children lists</returns>
    protected List<List<Item>> GetSealedContainerWeaponModRewards(
        SealedAirdropContainerSettings containerSettings,
        List<TemplateItem> linkedItemsToWeapon,
        Preset chosenWeaponPreset
    )
    {
        List<List<Item>> modRewards = [];

        foreach (var (rewardKey, settings) in containerSettings.WeaponModRewardLimits)
        {
            var rewardCount = randomUtil.GetInt(settings.Min, settings.Max);

            // Nothing to add, skip reward type
            if (rewardCount == 0)
            {
                continue;
            }

            // Get items that fulfil reward type criteria from items that fit on gun
            var relatedItems = linkedItemsToWeapon?.Where(item =>
                item?.Parent == rewardKey && !itemFilterService.IsItemBlacklisted(item.Id)
            );
            if (relatedItems is null || !relatedItems.Any())
            {
                if (logger.IsLogEnabled(LogLevel.Debug))
                {
                    logger.Debug($"No items found to fulfil reward type: {rewardKey} for weapon: {chosenWeaponPreset.Name}, skipping type");
                }

                continue;
            }

            // Find a random item of the desired type and add as reward
            for (var index = 0; index < rewardCount; index++)
            {
                var chosenItem = randomUtil.GetArrayValue(relatedItems);
                var reward = new List<Item>
                {
                    new() { Id = new MongoId(), Template = chosenItem.Id },
                };

                modRewards.Add(reward);
            }
        }

        return modRewards;
    }

    /// <summary>
    ///     Handle event-related loot containers - currently just the halloween jack-o-lanterns that give food rewards
    /// </summary>
    /// <param name="rewardContainerDetails"></param>
    /// <returns>List of item with children lists</returns>
    public List<List<Item>> GetRandomLootContainerLoot(RewardDetails rewardContainerDetails)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;
            return GetRandomLootContainerLootLegacy(rewardContainerDetails);
        }

        LastPathTaken = LootGenerationPath.Native;

        // The preset list is not read by this export, so it is not built
        var db = BuildRewardLootDb([], ToPresetViews(presetHelper.GetDefaultPresetByTpl()));

        var result = SptNative.GetRandomLootContainerLoot(
            new RandomLootContainerRequest
            {
                ItemsView = db.ItemsView,
                DefaultPresets = db.DefaultPresets,
                DefaultPresetsByTpl = db.DefaultPresetsByTpl,
                GlobalBlacklist = db.GlobalBlacklist,
                ConfigBlacklist = db.ConfigBlacklist,
                RewardItemBlacklist = db.RewardItemBlacklist,
                RewardBaseTypeBlacklist = db.RewardBaseTypeBlacklist,
                BossItems = db.BossItems,
                InactiveSeasonalItems = db.InactiveSeasonalItems,
                TestSeed = db.TestSeed,
                RewardDetails = rewardContainerDetails,
                // Every tpl PresetHelper.HasPreset answers true for, which the native side probes per
                // chosen reward
                PresetTpls = [.. presetHelper.GetTplsWithPresets()],
            }
        );

        return result.Items;
    }

    private List<List<Item>> GetRandomLootContainerLootLegacy(RewardDetails rewardContainerDetails)
    {
        List<List<Item>> itemsToReturn = [];

        // Get random items and add to newItemRequest
        for (var index = 0; index < rewardContainerDetails.RewardCount; index++)
        {
            // Pick random reward from pool, add to request object
            var chosenRewardItemTpl = PickRewardItem(rewardContainerDetails);

            if (presetHelper.HasPreset(chosenRewardItemTpl))
            {
                var preset = presetHelper.GetDefaultPreset(chosenRewardItemTpl);

                // Ensure preset has unique ids and is cloned so we don't alter the preset data stored in memory
                var presetAndMods = preset.Items.ReplaceIDs().ToList();

                presetAndMods.RemapRootItemId();
                itemsToReturn.Add(presetAndMods);

                continue;
            }

            List<Item> rewardItem = [new() { Id = new MongoId(), Template = chosenRewardItemTpl }];
            itemsToReturn.Add(rewardItem);
        }

        return itemsToReturn;
    }

    /// <summary>
    ///     Pick a reward item based on the reward details data
    /// </summary>
    /// <param name="rewardContainerDetails"></param>
    /// <returns>Single tpl</returns>
    protected MongoId PickRewardItem(RewardDetails rewardContainerDetails)
    {
        if (rewardContainerDetails.RewardTplPool is not null && rewardContainerDetails.RewardTplPool.Count > 0)
        {
            return weightedRandomHelper.GetWeightedValue(rewardContainerDetails.RewardTplPool);
        }

        return randomUtil.GetArrayValue(
            GetItemRewardPool([], rewardContainerDetails.RewardTypePool, true, true, false).ItemPool.Select(item => item.Id)
        );
    }

    protected record ItemRewardPoolResults
    {
        public IEnumerable<TemplateItem> ItemPool { get; set; }

        public HashSet<MongoId> Blacklist { get; set; }
    }
}

public class ItemLimit
{
    [JsonPropertyName("current")]
    public int Current { get; set; }

    [JsonPropertyName("max")]
    public int Max { get; set; }
}
