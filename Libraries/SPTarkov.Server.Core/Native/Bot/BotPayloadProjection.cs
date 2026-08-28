using SPTarkov.Server.Core.Generators.Bot;
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
        BotLootCacheService botLootCacheService,
        BotConfig botConfig,
        PmcConfig pmcConfig
    )
    {
        return new GenerateBotInventoryRequest
        {
            // The dispatch site stamps the resident epoch on an eligible send; 0 rides with the
            // views override
            Epoch = 0,
            Shared = BuildSharedVarying(
                sessionId,
                profileHelper,
                profileActivityService,
                weatherHelper,
                botConfig,
                // The single-bot path keeps C# level generation and C# filtering: no draw, no
                // variant pick, so neither block rides the wire
                levelGeneration: null,
                templateVariants: null
            ),
            Bot = BuildBotSlice(botId, botGenerationDetails, testSeed),
            Template = BuildTemplateView(botJsonTemplate),
            LootPools = BuildLootPools(botLootCacheService, botJsonTemplate, botGenerationDetails, pmcConfig),
        };
    }

    /// <summary>
    /// The request members that do not vary between the bots of one wave, built once for the
    /// whole wave: the generating player's level, the raid's daylight, the live
    /// <c>EquipmentMods</c> bands (the one equipment cell a runtime writer keeps off the resident
    /// DB), and the caller's level inputs and band variants - only the caller knows the wave's
    /// level range and the bands it splits into. Every config slice, and every database view,
    /// lives on <see cref="BuildViewsOverride"/> or the resident DB.
    /// </summary>
    internal static SharedBotVarying BuildSharedVarying(
        MongoId sessionId,
        ProfileHelper profileHelper,
        ProfileActivityService profileActivityService,
        WeatherHelper weatherHelper,
        BotConfig botConfig,
        LevelGenerationView? levelGeneration,
        List<BotTemplateVariantView>? templateVariants
    )
    {
        // Raw, not defaulted: the equipment path's `?? 1` (BotInventoryGenerator.cs:614 and its six
        // siblings, effective at :937-939) and the weapon-mod path's `?? 0`
        // (BotEquipmentModGenerator.cs:546) are applied natively, where both blacklist bands are now
        // picked out of Equipment
        var pmcProfile = profileHelper.GetPmcProfile(sessionId);

        // BotInventoryGenerator.cs:192-196 - no raid means day
        var raidConfig = profileActivityService.GetProfileActivityRaidData(sessionId)?.RaidConfiguration;
        var isNightTime = raidConfig is not null && weatherHelper.IsNightTime(raidConfig.TimeVariant, raidConfig.Location!);

        return new SharedBotVarying
        {
            GeneratingPlayerLevel = pmcProfile?.Info?.Level,
            IsNightTime = isNightTime,
            LiveEquipmentMods = botConfig
                .Equipment.Where(role => role.Value?.Randomisation is not null)
                .ToDictionary(
                    role => role.Key,
                    role =>
                        role.Value!.Randomisation!.Where(band => band.EquipmentMods is not null)
                            .Select(band => new LiveEquipmentModsBand { LevelRange = band.LevelRange, EquipmentMods = band.EquipmentMods! })
                            .ToList()
                ),
            LevelGeneration = levelGeneration,
            TemplateVariants = templateVariants,
        };
    }

    /// <summary>
    /// The database half of a request, for the ineligible arm: the views and config slices an
    /// eligible send reads off the resident DB instead, built from the same services and config
    /// singletons the old flat request read. <paramref name="lootPools"/> is every pool the send can
    /// draw from - one cache on the single-bot request, one per level-band variant on the batch -
    /// and prices as their union.
    /// </summary>
    internal static BotViewsOverride BuildViewsOverride(
        PresetHelper presetHelper,
        HandbookHelper handbookHelper,
        ItemHelper itemHelper,
        GlobalTable globalTable,
        ItemFilterService itemFilterService,
        BotConfig botConfig,
        PmcConfig pmcConfig,
        RepairConfig repairConfig,
        IEnumerable<BotLootCache> lootPools
    )
    {
        return new BotViewsOverride
        {
            Items = PayloadProjection.BuildItemsView(itemHelper.TemplateTable.Items),
            // `GetPreset(id)` reads the same map `ItemPresets` is, and every default preset is one
            // of its entries, so this is the only projection of it that goes on the wire
            ItemPresets = ToPresetViews(globalTable.ItemPresets),
            DefaultPresetsByTpl = ToDefaultPresetIds(presetHelper.GetDefaultPresetByTpl()),
            HandbookPrices = BuildHandbookPrices(lootPools, handbookHelper),
            ExpTable = [.. globalTable.Configuration.Exp.Level.ExperienceTable.Select(entry => entry.Experience)],
            Bosses = botConfig.Bosses,
            Durability = botConfig.Durability,
            ItemSpawnLimits = botConfig.ItemSpawnLimits,
            WalletLoot = botConfig.WalletLoot,
            CurrencyStackSize = botConfig.CurrencyStackSize,
            SecureContainerAmmoStackCount = botConfig.SecureContainerAmmoStackCount,
            DisableLootOnBotTypes = botConfig.DisableLootOnBotTypes,
            LowProfileGasBlockTpls = botConfig.LowProfileGasBlockTpls,
            LootItemResourceRandomization = botConfig.LootItemResourceRandomization,
            // A null entry is a role the legacy path would have thrown on; dropping it makes the
            // native side take its "no equipment filters for role" exit instead
            Equipment = botConfig.Equipment.Where(role => role.Value is not null).ToDictionary(role => role.Key, role => role.Value!),
            PmcConfig = pmcConfig,
            RepairKitWeapon = repairConfig.RepairKit.Weapon,
            ConfigBlacklist = itemFilterService.GetBlacklistedItems(),
        };
    }

    /// <summary>
    /// The three request members that do vary per bot.
    /// </summary>
    internal static BotSlice BuildBotSlice(MongoId botId, BotGenerationDetails botGenerationDetails, ulong? testSeed)
    {
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
                // Only PMCs are ever given one (GenerateBotPrelude sets it inside its IsPmc branch),
                // and both native reads of it are gated on isPmc - but the field is required on the
                // wire, so a scav's null has to become the empty string rather than a missing member
                GameVersion = botGenerationDetails.GameVersion ?? string.Empty,
                Location = botGenerationDetails.Location,
                BotDifficulty = botGenerationDetails.BotDifficulty ?? string.Empty,
                ClearBotContainerCacheAfterGeneration = botGenerationDetails.ClearBotContainerCacheAfterGeneration,
            },
        };
    }

    /// <summary>
    /// The three <c>BotType</c> blocks the inventory generator reads, narrowed to
    /// <see cref="BotTemplateView"/>. Shared by the single-bot request and the batch's per-band
    /// variants, which project the very same template.
    /// </summary>
    internal static BotTemplateView BuildTemplateView(BotType botJsonTemplate)
    {
        return new BotTemplateView
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
        };
    }

    /// <summary>
    /// The twelve <c>GetLootFromCache</c> calls <c>BotLootGenerator.GenerateLoot</c> makes, in its
    /// order and with its arguments, so the service hydrates exactly as it does on the legacy path.
    /// <c>CombinedPoolLoot</c> is left at its empty default: it is the one <c>LootCacheType</c>
    /// <c>GenerateLoot</c> never asks for, and asking for it here would be a call legacy does not
    /// make.
    /// </summary>
    internal static BotLootCache BuildLootPools(
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
    /// the only tpls the native running-total ever prices. The union over every cache is
    /// collision-safe: a tpl in two pools maps to the same <c>GetTemplatePrice</c> value in both.
    /// </summary>
    internal static Dictionary<MongoId, double> BuildHandbookPrices(IEnumerable<BotLootCache> lootPools, HandbookHelper handbookHelper)
    {
        var prices = new Dictionary<MongoId, double>();

        foreach (var pool in lootPools.SelectMany(EnumeratePools))
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
