using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Presets;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Profile;

namespace SPTarkov.Server.Core.Native.Bot;

/// <summary>
/// Assembles the <see cref="GenerateBotInventoryRequest"/> for one bot out of the live database,
/// services and config - everything <c>BotInventoryGenerator</c> and the three generators below it
/// would have read for themselves.
///
/// The services are parameters rather than injected members so the caller keeps its own 4.1.2
/// constructor: this class is a projection, not a component.
/// </summary>
internal static class BotPayloadProjection
{
    internal static GenerateBotInventoryRequest BuildRequest(
        MongoId botId,
        MongoId sessionId,
        BotType botJsonTemplate,
        BotGenerationDetails botGenerationDetails,
        ulong? testSeed,
        ProfileHelper profileHelper,
        ProfileActivityService profileActivityService,
        WeatherHelper weatherHelper,
        BotGeneratorHelper botGeneratorHelper,
        BotEquipmentFilterService botEquipmentFilterService,
        BotLootCacheService botLootCacheService,
        PresetHelper presetHelper,
        ItemFilterService itemFilterService,
        HandbookHelper handbookHelper,
        ItemHelper itemHelper,
        GlobalTable globalTable,
        BotConfig botConfig,
        PmcConfig pmcConfig,
        RepairConfig repairConfig
    )
    {
        // BotInventoryGenerator.cs:260 - the `?? 1` is what a session with no profile lands on, and
        // it is the level the equipment blacklist is resolved with at :583
        var generatingPlayerLevel = profileHelper.GetPmcProfile(sessionId)?.Info?.Level ?? 1;
        var equipmentRole = botGeneratorHelper.GetBotEquipmentRole(botGenerationDetails.RoleLowercase);

        // BotInventoryGenerator.cs:192-196 - no raid means day
        var raidConfig = profileActivityService.GetProfileActivityRaidData(sessionId)?.RaidConfiguration;
        var isNightTime = raidConfig is not null && weatherHelper.IsNightTime(raidConfig.TimeVariant, raidConfig.Location!);

        // `GetPreset(id)` reads the same map `ItemPresets` is, and every default preset is one of
        // its entries, so this is the only projection of it that goes on the wire
        var presets = ToPresetViews(globalTable.ItemPresets);
        var lootPools = BuildLootPools(botLootCacheService, botJsonTemplate, botGenerationDetails, pmcConfig);

        return new GenerateBotInventoryRequest
        {
            BotId = botId,
            TestSeed = testSeed,
            Details = new BotGenerationDetailsView
            {
                Role = botGenerationDetails.Role,
                RoleLowercase = botGenerationDetails.RoleLowercase,
                Side = botGenerationDetails.Side,
                BotLevel = botGenerationDetails.BotLevel,
                IsPmc = botGenerationDetails.IsPmc,
                IsPlayerScav = botGenerationDetails.IsPlayerScav,
                GameVersion = botGenerationDetails.GameVersion,
                Location = botGenerationDetails.Location,
                // Interpolated into log lines only, so an unset difficulty is the empty string
                // rather than a member Rust would fail to find
                BotDifficulty = botGenerationDetails.BotDifficulty ?? string.Empty,
                ClearBotContainerCacheAfterGeneration = botGenerationDetails.ClearBotContainerCacheAfterGeneration,
            },
            Template = new BotTemplateView
            {
                Inventory = new BotTypeInventoryView
                {
                    // Enumeration order is the slot order the equipment loop walks, so ToDictionary
                    // has to keep it - it does, both maps being insertion ordered
                    Equipment = botJsonTemplate.BotInventory.Equipment.ToDictionary(slot => slot.Key.ToString(), slot => slot.Value),
                    Ammo = botJsonTemplate.BotInventory.Ammo,
                    Items = botJsonTemplate.BotInventory.Items,
                    Mods = botJsonTemplate.BotInventory.Mods,
                },
                Chances = botJsonTemplate.BotChances,
                Generation = botJsonTemplate.BotGeneration,
            },
            GeneratingPlayerLevel = generatingPlayerLevel,
            IsNightTime = isNightTime,
            // A null entry is a role the legacy path would have thrown on; dropping it makes the
            // native side take its "no equipment filters for role" exit instead
            Equipment = botConfig.Equipment.Where(role => role.Value is not null).ToDictionary(role => role.Key, role => role.Value!),
            Bosses = botConfig.Bosses,
            Durability = botConfig.Durability,
            ItemSpawnLimits = botConfig.ItemSpawnLimits,
            WalletLoot = botConfig.WalletLoot,
            CurrencyStackSize = botConfig.CurrencyStackSize,
            SecureContainerAmmoStackCount = botConfig.SecureContainerAmmoStackCount,
            DisableLootOnBotTypes = botConfig.DisableLootOnBotTypes,
            LowProfileGasBlockTpls = botConfig.LowProfileGasBlockTpls,
            LootItemResourceRandomization = botConfig.LootItemResourceRandomization,
            PmcConfig = pmcConfig,
            RepairKitWeapon = repairConfig.RepairKit.Weapon,
            EquipmentBlacklist =
                botEquipmentFilterService.GetBotEquipmentBlacklist(equipmentRole, generatingPlayerLevel) ?? new EquipmentFilterDetails(),
            LootPools = lootPools,
            ItemPresets = presets,
            DefaultPresetsByTpl = ToDefaultPresetIds(presetHelper.GetDefaultPresetByTpl()),
            ConfigBlacklist = itemFilterService.GetBlacklistedItems(),
            HandbookPrices = BuildHandbookPrices(lootPools, handbookHelper),
            Items = PayloadProjection.BuildItemsView(itemHelper.TemplateTable.Items),
        };
    }

    /// <summary>
    /// The 20 request members that do not vary between the bots of one wave, built once for the
    /// whole wave. The role and player level are the wave's, which is what lets the equipment
    /// blacklist ride here rather than per bot.
    /// </summary>
    internal static SharedBotViews BuildSharedViews(
        MongoId sessionId,
        string roleLowercase,
        ProfileHelper profileHelper,
        ProfileActivityService profileActivityService,
        WeatherHelper weatherHelper,
        BotGeneratorHelper botGeneratorHelper,
        BotEquipmentFilterService botEquipmentFilterService,
        PresetHelper presetHelper,
        ItemFilterService itemFilterService,
        ItemHelper itemHelper,
        GlobalTable globalTable,
        BotConfig botConfig,
        PmcConfig pmcConfig,
        RepairConfig repairConfig
    )
    {
        var generatingPlayerLevel = profileHelper.GetPmcProfile(sessionId)?.Info?.Level ?? 1;
        var equipmentRole = botGeneratorHelper.GetBotEquipmentRole(roleLowercase);

        var raidConfig = profileActivityService.GetProfileActivityRaidData(sessionId)?.RaidConfiguration;
        var isNightTime = raidConfig is not null && weatherHelper.IsNightTime(raidConfig.TimeVariant, raidConfig.Location!);

        var presets = ToPresetViews(globalTable.ItemPresets);

        return new SharedBotViews
        {
            GeneratingPlayerLevel = generatingPlayerLevel,
            IsNightTime = isNightTime,
            Equipment = botConfig.Equipment.Where(role => role.Value is not null).ToDictionary(role => role.Key, role => role.Value!),
            Bosses = botConfig.Bosses,
            Durability = botConfig.Durability,
            ItemSpawnLimits = botConfig.ItemSpawnLimits,
            WalletLoot = botConfig.WalletLoot,
            CurrencyStackSize = botConfig.CurrencyStackSize,
            SecureContainerAmmoStackCount = botConfig.SecureContainerAmmoStackCount,
            DisableLootOnBotTypes = botConfig.DisableLootOnBotTypes,
            LowProfileGasBlockTpls = botConfig.LowProfileGasBlockTpls,
            LootItemResourceRandomization = botConfig.LootItemResourceRandomization,
            PmcConfig = pmcConfig,
            RepairKitWeapon = repairConfig.RepairKit.Weapon,
            EquipmentBlacklist =
                botEquipmentFilterService.GetBotEquipmentBlacklist(equipmentRole, generatingPlayerLevel) ?? new EquipmentFilterDetails(),
            ItemPresets = presets,
            DefaultPresetsByTpl = ToDefaultPresetIds(presetHelper.GetDefaultPresetByTpl()),
            ConfigBlacklist = itemFilterService.GetBlacklistedItems(),
            Items = PayloadProjection.BuildItemsView(itemHelper.TemplateTable.Items),
        };
    }

    /// <summary>
    /// The six request members that do vary per bot.
    /// </summary>
    internal static BotSlice BuildBotSlice(
        MongoId botId,
        BotType botJsonTemplate,
        BotGenerationDetails botGenerationDetails,
        ulong? testSeed,
        BotLootCacheService botLootCacheService,
        HandbookHelper handbookHelper,
        PmcConfig pmcConfig
    )
    {
        var lootPools = BuildLootPools(botLootCacheService, botJsonTemplate, botGenerationDetails, pmcConfig);

        return new BotSlice
        {
            BotId = botId,
            TestSeed = testSeed,
            Details = new BotGenerationDetailsView
            {
                Role = botGenerationDetails.Role,
                RoleLowercase = botGenerationDetails.RoleLowercase,
                Side = botGenerationDetails.Side,
                BotLevel = botGenerationDetails.BotLevel,
                IsPmc = botGenerationDetails.IsPmc,
                IsPlayerScav = botGenerationDetails.IsPlayerScav,
                GameVersion = botGenerationDetails.GameVersion,
                Location = botGenerationDetails.Location,
                BotDifficulty = botGenerationDetails.BotDifficulty ?? string.Empty,
                ClearBotContainerCacheAfterGeneration = botGenerationDetails.ClearBotContainerCacheAfterGeneration,
            },
            Template = new BotTemplateView
            {
                Inventory = new BotTypeInventoryView
                {
                    Equipment = botJsonTemplate.BotInventory.Equipment.ToDictionary(slot => slot.Key.ToString(), slot => slot.Value),
                    Ammo = botJsonTemplate.BotInventory.Ammo,
                    Items = botJsonTemplate.BotInventory.Items,
                    Mods = botJsonTemplate.BotInventory.Mods,
                },
                Chances = botJsonTemplate.BotChances,
                Generation = botJsonTemplate.BotGeneration,
            },
            LootPools = lootPools,
            HandbookPrices = BuildHandbookPrices(lootPools, handbookHelper),
        };
    }

    /// <summary>
    /// The twelve <c>GetLootFromCache</c> calls <c>BotLootGenerator.GenerateLoot</c> makes, in its
    /// order and with its arguments, so the service hydrates exactly as it does on the legacy path.
    /// <c>CombinedPoolLoot</c> is left at its empty default: it is the one <c>LootCacheType</c>
    /// <c>GenerateLoot</c> never asks for, and asking for it here would be a call legacy does not
    /// make.
    /// </summary>
    private static BotLootCache BuildLootPools(
        BotLootCacheService botLootCacheService,
        BotType botJsonTemplate,
        BotGenerationDetails botGenerationDetails,
        PmcConfig pmcConfig
    )
    {
        var role = botGenerationDetails.RoleLowercase;
        var isPmc = botGenerationDetails.IsPmc;
        var priceLimits = GetSingleItemLootPriceLimits(pmcConfig, botGenerationDetails.BotLevel, isPmc);

        return new BotLootCache
        {
            SpecialItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.Special, botJsonTemplate),
            HealingItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.HealingItems, botJsonTemplate),
            DrugItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.DrugItems, botJsonTemplate),
            FoodItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.FoodItems, botJsonTemplate),
            DrinkItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.DrinkItems, botJsonTemplate),
            CurrencyItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.CurrencyItems, botJsonTemplate),
            StimItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.StimItems, botJsonTemplate),
            GrenadeItems = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.GrenadeItems, botJsonTemplate),
            // The three price-banded pools: the filter lives in the service, which prices through
            // ItemHelper.GetItemPrice - handbook *and* flea - so it cannot move to the native side
            BackpackLoot = botLootCacheService.GetLootFromCache(
                role,
                isPmc,
                LootCacheType.Backpack,
                botJsonTemplate,
                priceLimits?.Backpack
            ),
            VestLoot = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.Vest, botJsonTemplate, priceLimits?.Vest),
            PocketLoot = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.Pocket, botJsonTemplate, priceLimits?.Pocket),
            SecureLoot = botLootCacheService.GetLootFromCache(role, isPmc, LootCacheType.Secure, botJsonTemplate),
        };
    }

    /// <summary>
    /// <c>BotLootGenerator.GetSingleItemLootPriceLimits</c>, which is protected there and so cannot
    /// be called from here.
    /// </summary>
    private static MinMaxLootItemValue? GetSingleItemLootPriceLimits(PmcConfig pmcConfig, int botLevel, bool isPmc)
    {
        if (!isPmc)
        {
            return null;
        }

        return pmcConfig.LootItemLimitsRub.FirstOrDefault(minMaxValue => botLevel >= minMaxValue.Min && botLevel <= minMaxValue.Max);
    }

    /// <summary>
    /// <c>HandbookHelper.GetTemplatePrice</c> for every tpl that can be drawn out of a loot pool -
    /// the only tpls the native running-total ever prices.
    /// </summary>
    private static Dictionary<MongoId, double> BuildHandbookPrices(BotLootCache lootPools, HandbookHelper handbookHelper)
    {
        var prices = new Dictionary<MongoId, double>();

        foreach (var pool in EnumeratePools(lootPools))
        {
            foreach (var tpl in pool.Keys)
            {
                if (!prices.ContainsKey(tpl))
                {
                    prices[tpl] = handbookHelper.GetTemplatePrice(tpl);
                }
            }
        }

        return prices;
    }

    private static IEnumerable<Dictionary<MongoId, double>> EnumeratePools(BotLootCache lootPools)
    {
        yield return lootPools.SpecialItems;
        yield return lootPools.HealingItems;
        yield return lootPools.DrugItems;
        yield return lootPools.FoodItems;
        yield return lootPools.DrinkItems;
        yield return lootPools.CurrencyItems;
        yield return lootPools.StimItems;
        yield return lootPools.GrenadeItems;
        yield return lootPools.BackpackLoot;
        yield return lootPools.VestLoot;
        yield return lootPools.PocketLoot;
        yield return lootPools.SecureLoot;
    }

    /// <summary>
    /// The default preset of each tpl as its id. <c>PresetHelper</c> resolves every default out of
    /// <c>GlobalTable.ItemPresets</c> - both cache halves are filtered from it and the fallback
    /// indexes it directly - so the id always hits <c>ItemPresets</c> on the far side.
    /// </summary>
    private static Dictionary<MongoId, MongoId> ToDefaultPresetIds(Dictionary<MongoId, Preset> defaultPresets)
    {
        return defaultPresets.ToDictionary(preset => preset.Key, preset => preset.Value.Id);
    }

    private static Dictionary<MongoId, PresetView> ToPresetViews(Dictionary<MongoId, Preset> presets)
    {
        return presets.ToDictionary(
            preset => preset.Key,
            preset => new PresetView
            {
                Items = preset.Value.Items,
                Id = preset.Value.Id,
                Name = preset.Value.Name,
                Encyclopedia = preset.Value.Encyclopedia,
            }
        );
    }
}
