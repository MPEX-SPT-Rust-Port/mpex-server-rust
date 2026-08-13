//! `Generators/Loot/BotLootGenerator.cs` — fill a bot's pockets, vest, backpack and secure container
//! from the pools `BotLootCacheService` resolved for its role.
//!
//! # The twelve pool reads (Task 13's projection contract)
//!
//! `BotLootCacheService.GetLootFromCache` is **not** ported: it builds its pools out of
//! `PMCLootGenerator` and the whole handbook, and it caches them per role for the process lifetime.
//! The C# caller resolves them and sends them across as [`BotLootCacheWire`], whose thirteen fields
//! are the thirteen `BotLootCache` members. `GenerateLoot` reads twelve of them — `Combined` is the
//! one `LootCacheType` it never asks for — in this order, with the arguments each call site passes
//! (`botRole` and `isPmc` are the same for every call and are what selects the cached bucket):
//!
//! | # | C# line | `LootCacheType` | [`BotLootCacheWire`] field | `itemPriceMinMax` argument |
//! |---|---|---|---|---|
//! | 1 | `:131` | `Special` | `special_items` | none |
//! | 2 | `:147` | `HealingItems` | `healing_items` | none |
//! | 3 | `:165` | `DrugItems` | `drug_items` | none |
//! | 4 | `:183` | `FoodItems` | `food_items` | none |
//! | 5 | `:201` | `DrinkItems` | `drink_items` | none |
//! | 6 | `:219` | `CurrencyItems` | `currency_items` | none |
//! | 7 | `:237` | `StimItems` | `stim_items` | none |
//! | 8 | `:255` | `GrenadeItems` | `grenade_items` | none |
//! | 9 | `:295` | `Backpack` | `backpack_loot` | `GetSingleItemLootPriceLimits(botLevel, isPmc)?.Backpack` |
//! | 10 | `:322` | `Vest` | `vest_loot` | `…?.Vest` |
//! | 11 | `:346` | `Pocket` | `pocket_loot` | `…?.Pocket` |
//! | 12 | `:369` | `Secure` (`"SecuredContainer"`) | `secure_loot` | none |
//!
//! The `itemPriceMinMax` filter on rows 9-11 (`BotLootCacheService.cs:143-167`) stays C#-side with
//! the rest of the service: it prices items through `ItemHelper.GetItemPrice`, which is handbook
//! *and* flea, and only `handbook_prices` is projected. [`get_single_item_loot_price_limits`] is
//! ported here so the limits the projection must filter with are pinned and testable on this side
//! too, but nothing on this path calls it — the pools arrive filtered.
//!
//! `GetLootFromCache` clones every non-empty pool it hands out (`:146`), which is what makes
//! [`add_loot_from_pool`]'s `pool.Remove` (`:503`) local to one call; the pools are cloned per call
//! site here for the same reason.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! [`generate_loot`] draws, in this order:
//!
//! 1. **Eleven `GetWeightedValue`s** (`:97-107`) — backpack, pocket, vest, special, healing, drugs,
//!    food, drink, currency, stims, grenades. All eleven are consumed **before** the
//!    `DisableLootOnBotTypes` check (`:110-116`) zeroes four of them, so a no-loot bot burns the
//!    same eleven draws as any other.
//! 2. Forced PMC meds (`:119-122`), for a PMC with `ForceHealingItemsIntoSecure`: two
//!    [`add_loot_from_pool`] runs over single-entry pools (`GetWeightedValue` does not draw for a
//!    one-entry map), counts 1 and 10.
//! 3. Eight [`add_loot_from_pool`] runs: special, healing, drugs, food, drink, currency, stims,
//!    grenades.
//! 4. The backpack branch (`:273`), only when the bot **has** a backpack and the count is `> 0`:
//!    1 `GetChance100` for the loose weapon (PMC only, `:276`), then
//!    [`add_loose_weapons_to_inventory_slot`], then the backpack pool.
//! 5. The vest pool, gated on the slot alone — **no count check**, deliberately asymmetric with the
//!    backpack (`:317`).
//! 6. The pocket pool, ungated.
//! 7. The secure pool, 50 items with `totalValueLimitRub = -1` (`:376`/`:380`).
//!
//! One [`add_loot_from_pool`] iteration draws: 1 `GetWeightedValue` over the pool (`:490`), then
//! `GenerateExtraPropertiesForItem`'s draws (see [`crate::bot::bot_generator_helper`]), then — for
//! a wallet tpl only — 1 `GetChance100` (`:523`) and [`create_wallet_loot`]'s draws, then
//! [`add_required_child_items_to_parent`]'s. Placement draws nothing.
//!
//! [`create_wallet_loot`] draws 1 `GetInt` for the stack count (`:621`) and then, per stack, 1
//! `GetWeightedValue` for the size (`:625`) and 1 for the currency (`:631`) — in that order.
//!
//! [`add_loose_weapons_to_inventory_slot`] draws 1 `GetArrayValue` (`:693`) and 1 `GetInt` (`:699`)
//! before anything else, then whatever [`generate_random_weapon`] draws per weapon.
use std::collections::HashSet;

use indexmap::IndexMap;

use crate::bot::BotContext;
use crate::bot::bot_generator_helper::{
    ContainerGrids, ItemAddedResult, generate_extra_properties_for_item, get_item_size,
};
use crate::bot::bot_weapon_generator::generate_random_weapon;
use crate::bot::bot_weapon_generator_helper::item_added_result_name;
use crate::bot::durability_limits_helper::is_bot_pmc;
use crate::bot::models::{
    BotGenerationDetailsWire, BotLootCacheWire, BotTypeInventoryWire, ItemCountsWire,
    ItemSpawnLimitSettingsWire, LootContainerSettingsWire, MinMaxLootItemValueWire, PmcConfigWire,
    WalletLootSettingsWire,
};
use crate::loot::container_extensions::{find_slot_for_item, try_fill_container_map_with_item};
use crate::loot::item_helper::{
    AMMO, AMMO_BOX, LootError, MONEY, add_cartridges_to_ammo_box, add_child_slot_items, get_item,
    get_randomised_ammo_stack_size, is_of_baseclass, item_requires_soft_inserts,
};
use crate::loot::models::{
    DEBUG, Diagnostic, ERROR, Item, ItemLocation, ItemRotation, ItemView, Upd, WARNING,
};
use crate::loot::mongo_id;
use crate::loot::random_util::{get_array_value, get_chance_100, get_int, get_weighted_value};

/// `EquipmentSlots` member names, as strings (see [`crate::bot::bot_weapon_generator`]).
const POCKETS: &str = "Pockets";
const TACTICAL_VEST: &str = "TacticalVest";
const BACKPACK: &str = "Backpack";
const SECURED_CONTAINER: &str = "SecuredContainer";
const FIRST_PRIMARY_WEAPON: &str = "FirstPrimaryWeapon";
const HOLSTER: &str = "Holster";

/// `ItemTpl.MEDICAL_SURV12_FIELD_SURGICAL_KIT` (`Models/Enums/ItemTpl.cs:3156`).
const MEDICAL_SURV12_FIELD_SURGICAL_KIT: &str = "5d02797c86f774203f38e30a";
/// `ItemTpl.MEDKIT_AFAK_TACTICAL_INDIVIDUAL_FIRST_AID_KIT` (`:3157`).
const MEDKIT_AFAK_TACTICAL_INDIVIDUAL_FIRST_AID_KIT: &str = "60098ad7c2240c0fe85c570a";

/// What `$"…for slots: {equipmentSlots}…"` (`:495`) actually prints: `equipmentSlots` is a
/// `HashSet<EquipmentSlots>`, which has no `ToString` override, so the interpolation yields the
/// type name. Bug-for-bug — the log line names no slots.
const EQUIPMENT_SLOTS_TO_STRING: &str =
    "System.Collections.Generic.HashSet`1[SPTarkov.Server.Core.Models.Enums.EquipmentSlots]";

/// The slot id `PlaceItemInContainer` is called with for wallet currency (`:542`).
const WALLET_GRID_SLOT_ID: &str = "main";

/// Everything `BotLootGenerator` reads that is fixed for one bot: the configs C# takes through its
/// constructor, the resolved loot pools, and the two ids the C# reaches for off objects this port
/// does not carry whole (`botInventory.Equipment`).
///
/// `botId` is **not** carried: C# threads it through every signature here only to reach
/// `BotInventoryContainerService`'s per-bot cache key, and this port's [`ContainerGrids`] *is* one
/// bot's entry, so nothing on this path has anything to key.
///
/// A bundle rather than more [`BotContext`] fields: none of it is read outside this module, and the
/// seven sibling fixtures that build a `BotContext` would otherwise all have to grow stubs for it.
pub struct BotLootConfig<'a> {
    /// `botInventory.Equipment`, the parent id a loose weapon is generated under (`:716`).
    pub equipment_id: &'a str,
    /// `botJsonTemplate.BotGeneration.Items`.
    pub item_counts: &'a ItemCountsWire,
    /// `botJsonTemplate.BotChances.WeaponModsChances`, for loose weapons (`:285`).
    pub weapon_mod_chances: &'a IndexMap<String, f64>,
    /// `BotConfig.DisableLootOnBotTypes`.
    pub disable_loot_on_bot_types: &'a HashSet<String>,
    /// `BotConfig.ItemSpawnLimits`, keyed by role (or `"pmc"`).
    pub item_spawn_limits: &'a IndexMap<String, IndexMap<String, f64>>,
    /// `BotConfig.WalletLoot`.
    pub wallet_loot: &'a WalletLootSettingsWire,
    /// `BotConfig.CurrencyStackSize` — bot role → money tpl → stack size → weight.
    pub currency_stack_size: &'a IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
    /// `PmcConfig`, narrowed.
    pub pmc: &'a PmcConfigWire,
    /// `HandbookHelper.GetTemplatePrice` — a tpl the handbook does not carry is worth 0 there.
    pub handbook_prices: &'a IndexMap<String, f64>,
    /// The resolved `BotLootCacheService` pools.
    pub loot_pools: &'a BotLootCacheWire,
}

/// `BotLootGenerator.GenerateLoot` (`BotLootGenerator.cs:68-384`).
///
/// `sessionId` is not carried: the only thing C# does with it is hand it to `GenerateRandomWeapon`,
/// which this port dropped in Task 9. `botInventory` rides as its `Items` list plus the two ids in
/// [`BotLootConfig`], and `botJsonTemplate` as `item_counts` plus the mutable
/// `bot_template_inventory` the loose-weapon path threads into `GenerateRandomWeapon`.
///
/// # Errors
///
/// Where the C# throws: an unusable weights map or a weight key that is not a number, the
/// `ItemSpawnLimits["pmc"]` indexer, and everything [`create_wallet_loot`],
/// [`randomise_money_stack_size`] and [`generate_random_weapon`] raise.
pub fn generate_loot(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    inventory: &mut Vec<Item>,
    details: &BotGenerationDetailsWire,
    bot_template_inventory: &mut BotTypeInventoryWire,
    config: &BotLootConfig,
) -> Result<(), LootError> {
    // Limits on item types to be added as loot
    let item_counts = config.item_counts;

    let weights = [
        item_counts.backpack_loot.as_ref(),
        item_counts.pocket_loot.as_ref(),
        item_counts.vest_loot.as_ref(),
        item_counts.special_items.as_ref(),
        item_counts.healing.as_ref(),
        item_counts.drugs.as_ref(),
        item_counts.food.as_ref(),
        item_counts.drink.as_ref(),
        item_counts.currency.as_ref(),
        item_counts.stims.as_ref(),
        item_counts.grenades.as_ref(),
    ];
    // `itemCounts?.X.Weights is null` for all eleven (`:79-91`). See [`weighted_count`] for what an
    // empty map means here and how it differs from the C# null it stands in for.
    if weights
        .iter()
        .any(|block| block.is_none_or(|block| block.weights.is_empty()))
    {
        ctx.diagnostics.push(localised(
            WARNING,
            "bot-unable_to_generate_bot_loot",
            serde_json::Value::String(details.role_lowercase.clone()),
        ));

        return Ok(());
    }

    // The eleven draws, in C# order, all consumed before any of them can be zeroed below.
    let mut backpack_loot_count = weighted_count(weights[0])?;
    let mut pocket_loot_count = weighted_count(weights[1])?;
    let mut vest_loot_count = weighted_count(weights[2])?;
    let special_loot_item_count = weighted_count(weights[3])?;
    let healing_item_count = weighted_count(weights[4])?;
    let drug_item_count = weighted_count(weights[5])?;
    let food_item_count = weighted_count(weights[6])?;
    let drink_item_count = weighted_count(weights[7])?;
    let mut currency_item_count = weighted_count(weights[8])?;
    let stim_item_count = weighted_count(weights[9])?;
    let grenade_count = weighted_count(weights[10])?;

    // If bot has been flagged as not having loot, set below counts to 0
    if config
        .disable_loot_on_bot_types
        .contains(&details.role_lowercase)
    {
        backpack_loot_count = 0.0;
        pocket_loot_count = 0.0;
        vest_loot_count = 0.0;
        currency_item_count = 0.0;
    }

    // Forced pmc healing loot into secure container
    if details.is_pmc && config.pmc.force_healing_items_into_secure {
        add_forced_medical_items_to_pmc_secure(
            ctx,
            grids,
            config,
            inventory,
            &details.role_lowercase,
        )?;
    }

    let mut bot_item_limits = get_item_spawn_limits_for_bot(ctx, config, &details.role_lowercase)?;

    let containers_bot_has_available = get_available_containers_bot_can_store_items_in(inventory);

    // Special items
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.special_items.clone(),
        &containers_bot_has_available,
        special_loot_item_count,
        inventory,
        &details.role_lowercase,
        Some(&mut bot_item_limits),
        0.0,
        false,
    )?;

    // Healing items / Meds
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.healing_items.clone(),
        &containers_bot_has_available,
        healing_item_count,
        inventory,
        &details.role_lowercase,
        None,
        0.0,
        details.is_pmc,
    )?;

    // Drugs
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.drug_items.clone(),
        &containers_bot_has_available,
        drug_item_count,
        inventory,
        &details.role_lowercase,
        None,
        0.0,
        details.is_pmc,
    )?;

    // Food
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.food_items.clone(),
        &containers_bot_has_available,
        food_item_count,
        inventory,
        &details.role_lowercase,
        None,
        0.0,
        details.is_pmc,
    )?;

    // Drink
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.drink_items.clone(),
        &containers_bot_has_available,
        drink_item_count,
        inventory,
        &details.role_lowercase,
        None,
        0.0,
        details.is_pmc,
    )?;

    // Currency
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.currency_items.clone(),
        &containers_bot_has_available,
        currency_item_count,
        inventory,
        &details.role_lowercase,
        None,
        0.0,
        details.is_pmc,
    )?;

    // Stims
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.stim_items.clone(),
        &containers_bot_has_available,
        stim_item_count,
        inventory,
        &details.role_lowercase,
        Some(&mut bot_item_limits),
        0.0,
        details.is_pmc,
    )?;

    // Grenades
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.grenade_items.clone(),
        // Can't use containersBotHasEquipped as we don't want grenades added to backpack
        &[POCKETS.to_owned(), TACTICAL_VEST.to_owned()],
        grenade_count,
        inventory,
        &details.role_lowercase,
        None,
        0.0,
        details.is_pmc,
    )?;

    // Backpack - generate loot if they have one
    if containers_bot_has_available
        .iter()
        .any(|slot| slot == BACKPACK)
        && backpack_loot_count > 0.0
    {
        // Add randomly generated weapon to PMC backpacks
        if details.is_pmc && get_chance_100(config.pmc.loose_weapon_in_backpack_chance_percent) {
            add_loose_weapons_to_inventory_slot(
                ctx,
                grids,
                config,
                inventory,
                BACKPACK,
                details,
                bot_template_inventory,
            )?;
        }

        let backpack_loot_rouble_total = if details.is_pmc {
            get_rouble_value(
                &config.pmc.loot_settings.backpack,
                details.bot_level,
                details.location.as_deref(),
            )
        } else {
            0.0
        };

        add_loot_from_pool(
            ctx,
            grids,
            config,
            config.loot_pools.backpack_loot.clone(),
            &[BACKPACK.to_owned()],
            backpack_loot_count,
            inventory,
            &details.role_lowercase,
            Some(&mut bot_item_limits),
            backpack_loot_rouble_total,
            details.is_pmc,
        )?;
    }

    let vest_loot_rouble_total = if details.is_pmc {
        get_rouble_value(
            &config.pmc.loot_settings.vest,
            details.bot_level,
            details.location.as_deref(),
        )
    } else {
        0.0
    };

    // TacticalVest - generate loot if they have one. Note the asymmetry with the backpack above:
    // no `vestLootCount > 0` check, so a zeroed count still runs the pool (and immediately falls
    // out of the loop).
    if containers_bot_has_available
        .iter()
        .any(|slot| slot == TACTICAL_VEST)
    {
        add_loot_from_pool(
            ctx,
            grids,
            config,
            config.loot_pools.vest_loot.clone(),
            &[TACTICAL_VEST.to_owned()],
            vest_loot_count,
            inventory,
            &details.role_lowercase,
            Some(&mut bot_item_limits),
            vest_loot_rouble_total,
            details.is_pmc,
        )?;
    }

    let pocket_loot_rouble_total = if details.is_pmc {
        get_rouble_value(
            &config.pmc.loot_settings.pocket,
            details.bot_level,
            details.location.as_deref(),
        )
    } else {
        0.0
    };

    // Pockets
    add_loot_from_pool(
        ctx,
        grids,
        config,
        config.loot_pools.pocket_loot.clone(),
        &[POCKETS.to_owned()],
        pocket_loot_count,
        inventory,
        &details.role_lowercase,
        Some(&mut bot_item_limits),
        pocket_loot_rouble_total,
        details.is_pmc,
    )?;

    // Secure - only add if not a pmc or is pmc and flag is true
    if !details.is_pmc || config.pmc.add_secure_container_loot_from_bot_config {
        add_loot_from_pool(
            ctx,
            grids,
            config,
            config.loot_pools.secure_loot.clone(),
            &[SECURED_CONTAINER.to_owned()],
            50.0,
            inventory,
            &details.role_lowercase,
            None,
            // Negative, so the `> 0` budget check at `:600` never runs: the secure container is
            // capped by its 50 items and its grid, not by value.
            -1.0,
            details.is_pmc,
        )?;
    }

    Ok(())
}

/// One of the eleven `GetWeightedValue` draws at `:97-107`. `GenerationData.Weights` is keyed by
/// `double` in C#; here the drawn key is parsed back out, as everywhere else in the bot port.
///
/// **Divergence, deliberate.** C# distinguishes a *null* `Weights` from an empty one: the guard at
/// `:79-91` is a null check, so `"grenades": {}` (null `Weights`) warns and returns, while
/// `"grenades": {"weights": {}}` (empty but present) sails past it and throws inside
/// `GetWeightedValue`. `GenerationDataWire.weights` is `#[serde(default)]`, so absent, `null` and
/// `{}` all arrive as one empty map and the two cases are not distinguishable on the wire — both
/// take the warn-and-return exit. Making the field an `Option` would restore the distinction, but
/// only for a bot json with a literally empty weights object, which no shipped bot has, and it
/// would drag `bot_weapon_generator`'s magazine block into the same question. A lootless bot is the
/// safer of the two outcomes behind FFI.
///
/// # Errors
///
/// From `get_weighted_value`, plus the key parse the C# deserializer does up front. A block the
/// caller did not null-check is a `LootError` rather than a panic; the caller does check.
fn weighted_count(
    weights: Option<&crate::bot::models::GenerationDataWire>,
) -> Result<f64, LootError> {
    let Some(weights) = weights else {
        return Err(LootError::new(
            "Object reference not set to an instance of an object.",
        ));
    };

    let chosen = get_weighted_value(&weights.weights)?;

    chosen.parse().map_err(|_| {
        LootError::new(format!(
            "Item count weighting key is not a number: {chosen}"
        ))
    })
}

/// `BotLootGenerator.GetSingleItemLootPriceLimits` (`:386-399`).
///
/// Not called on this path — the pools arrive already filtered, see the module doc. Ported so the
/// limits the C# projection must filter with are pinned on this side too.
#[allow(
    dead_code,
    reason = "see the doc comment: pinned for the C# projection, exercised only by this module's tests"
)]
pub fn get_single_item_loot_price_limits(
    pmc: &PmcConfigWire,
    bot_level: i32,
    is_pmc: bool,
) -> Option<&MinMaxLootItemValueWire> {
    // TODO - extend to other bot types
    if !is_pmc {
        return None;
    }

    pmc.loot_item_limits_rub
        .iter()
        .find(|min_max| f64::from(bot_level) >= min_max.min && f64::from(bot_level) <= min_max.max)
}

/// `LootContainerSettingsExtensions.GetRoubleValue` (`Extensions/LootContainerSettingsExtensions.cs:10-50`),
/// including its `GetContainerRoubleTotalByLevel` fallback of **1** for a level no band covers.
fn get_rouble_value(
    settings: &LootContainerSettingsWire,
    bot_level: i32,
    location_id: Option<&str>,
) -> f64 {
    let rouble_total_by_level = settings
        .total_rub_by_level
        .iter()
        .find(|min_max| bot_level >= min_max.min && bot_level <= min_max.max)
        .map_or(1.0, |min_max| min_max.value);

    let Some(location_id) = location_id else {
        return rouble_total_by_level;
    };

    // Get multiplier for map, use default if map not found
    let Some(multiplier) = settings
        .location_multiplier
        .get(location_id)
        .or_else(|| settings.location_multiplier.get("default"))
    else {
        return rouble_total_by_level;
    };

    rouble_total_by_level * multiplier
}

/// `BotLootGenerator.GetAvailableContainersBotCanStoreItemsIn` (`:406-421`).
///
/// A `Vec` rather than a set: the order decides which container
/// [`ContainerGrids::add_item_with_children_to_equipment_slot`] tries first, and the C#
/// `HashSet<EquipmentSlots>` enumerates in insertion order for a set this small.
pub fn get_available_containers_bot_can_store_items_in(bot_inventory: &[Item]) -> Vec<String> {
    let mut result = vec![POCKETS.to_owned()];

    if bot_inventory
        .iter()
        .any(|item| item.slot_id.as_deref() == Some(TACTICAL_VEST))
    {
        result.push(TACTICAL_VEST.to_owned());
    }

    if bot_inventory
        .iter()
        .any(|item| item.slot_id.as_deref() == Some(BACKPACK))
    {
        result.push(BACKPACK.to_owned());
    }

    result
}

/// `BotLootGenerator.AddForcedMedicalItemsToPmcSecure` (`:429-447`).
///
/// # Errors
///
/// From [`add_loot_from_pool`].
pub fn add_forced_medical_items_to_pmc_secure(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    config: &BotLootConfig,
    bot_inventory: &mut Vec<Item>,
    bot_role: &str,
) -> Result<(), LootError> {
    // surv12
    add_loot_from_pool(
        ctx,
        grids,
        config,
        IndexMap::from([(MEDICAL_SURV12_FIELD_SURGICAL_KIT.to_owned(), 1.0)]),
        &[SECURED_CONTAINER.to_owned()],
        1.0,
        bot_inventory,
        bot_role,
        None,
        0.0,
        true,
    )?;

    // AFAK
    add_loot_from_pool(
        ctx,
        grids,
        config,
        IndexMap::from([(
            MEDKIT_AFAK_TACTICAL_INDIVIDUAL_FIRST_AID_KIT.to_owned(),
            1.0,
        )]),
        &[SECURED_CONTAINER.to_owned()],
        10.0,
        bot_inventory,
        bot_role,
        None,
        0.0,
        true,
    )
}

/// `BotLootGenerator.AddLootFromPool` (`:461-609`).
///
/// `pool` is taken by value: C# is handed a clone per call (`BotLootCacheService.cs:146`) and
/// removes from it (`:503`), which must not reach the cache.
///
/// # Errors
///
/// From `get_weighted_value`, [`create_wallet_loot`] and [`add_required_child_items_to_parent`].
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per C# parameter; the names are the parity contract"
)]
pub fn add_loot_from_pool(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    config: &BotLootConfig,
    mut pool: IndexMap<String, f64>,
    equipment_slots: &[String],
    total_item_count: f64,
    inventory_to_add_items_to: &mut Vec<Item>,
    bot_role: &str,
    mut item_spawn_limits: Option<&mut ItemSpawnLimitSettingsWire>,
    total_value_limit_rub: f64,
    is_pmc: bool,
) -> Result<(), LootError> {
    // Loot pool has items
    if pool.is_empty() {
        return Ok(());
    }

    let items = ctx.items;
    let mut current_total_rub = 0.0;

    let mut fit_item_into_container_attempts = 0;
    let mut i = 0_i32;
    while f64::from(i) < total_item_count {
        // Pool can become empty if item spawn limits keep removing items
        if pool.is_empty() {
            return Ok(());
        }

        let weighted_item_tpl = get_weighted_value(&pool)?;
        let Some(item_to_add_template) = get_item(items, &weighted_item_tpl) else {
            ctx.diagnostics.push(plain(
                WARNING,
                format!(
                    "Unable to process item tpl: {weighted_item_tpl} for slots: \
                     {EQUIPMENT_SLOTS_TO_STRING} on bot: {bot_role}"
                ),
            ));

            i += 1;
            continue;
        };

        if let Some(limits) = item_spawn_limits.as_deref_mut()
            && item_has_reached_spawn_limit(
                ctx,
                item_to_add_template,
                &weighted_item_tpl,
                bot_role,
                limits,
            )
        {
            // Remove item from pool to prevent it being picked again
            pool.shift_remove(&weighted_item_tpl);

            // `i--` then `continue` — the C# for-loop's `i++` puts it straight back, so the
            // iteration is retried rather than spent.
            continue;
        }

        let new_root_item_id = mongo_id::generate();
        let mut item_with_children_to_add = vec![Item {
            id: new_root_item_id.clone(),
            template: weighted_item_tpl.clone(),
            upd: generate_extra_properties_for_item(
                ctx,
                item_to_add_template,
                Some(bot_role),
                true,
            )?,
            ..Default::default()
        }];

        // Is Simple-Wallet / WZ wallet
        if config
            .wallet_loot
            .wallet_tpl_pool
            .contains(&weighted_item_tpl)
        {
            let add_currency_to_wallet = get_chance_100(config.wallet_loot.chance_percent);
            if add_currency_to_wallet {
                // Create the currency items we want to add to wallet
                let mut items_to_add = create_wallet_loot(config, &new_root_item_id)?;

                // Get the container grid for the wallet
                let container_grid = get_container_slot_map(items, &weighted_item_tpl);

                // Check if all the chosen currency items fit into wallet. MUST clone the grid
                // before passing it in, the check fills as it goes.
                let can_add_to_container =
                    can_place_items_in_container(ctx, &mut container_grid.clone(), &items_to_add);
                if can_add_to_container {
                    let mut container_grid = container_grid;
                    // Add each currency to wallet
                    for item_to_add in &mut items_to_add {
                        place_item_in_container(
                            ctx,
                            &mut container_grid,
                            item_to_add,
                            &new_root_item_id,
                            WALLET_GRID_SLOT_ID,
                        );
                    }

                    item_with_children_to_add.extend(items_to_add.into_iter().flatten());
                }
            }
        }

        // Some items (ammoBox/ammo) need extra changes
        add_required_child_items_to_parent(
            ctx,
            config,
            &weighted_item_tpl,
            &mut item_with_children_to_add,
            is_pmc,
            bot_role,
        )?;

        // Attempt to add item to container(s)
        let item_added_result = grids.add_item_with_children_to_equipment_slot(
            ctx,
            equipment_slots,
            &new_root_item_id,
            &weighted_item_tpl,
            &mut item_with_children_to_add,
            inventory_to_add_items_to,
        );

        // Handle when fitting item fails
        if item_added_result != ItemAddedResult::Success {
            if item_added_result == ItemAddedResult::NoContainers {
                // Bot has no container to put item in, exit
                ctx.diagnostics.push(plain(
                    DEBUG,
                    format!(
                        "Unable to add: {total_item_count} items to bot as it lacks a container to include them"
                    ),
                ));

                break;
            }

            fit_item_into_container_attempts += 1;
            if fit_item_into_container_attempts >= 4 {
                let name = item_to_add_template.name.clone().unwrap_or_default();
                let slots = equipment_slots.join(",");
                let reason = item_added_result_name(item_added_result);
                ctx.diagnostics.push(plain(
                    DEBUG,
                    format!(
                        "Failed placing item: {weighted_item_tpl} - {name}: {i} of: \
                         {total_item_count} items into: {bot_role} containers: {slots}. \
                         Tried: {fit_item_into_container_attempts} times, reason: {reason}, skipping"
                    ),
                ));

                break;
            }

            // Try again, failed but still under attempt limit
            i += 1;
            continue;
        }

        // Item added okay, reset counter for next item
        fit_item_into_container_attempts = 0;

        // Stop adding items to bots pool if rolling total is over total limit
        if total_value_limit_rub > 0.0 {
            current_total_rub += get_template_price(config, &weighted_item_tpl);
            if current_total_rub > total_value_limit_rub {
                break;
            }
        }

        i += 1;
    }

    Ok(())
}

/// `HandbookHelper.GetTemplatePrice` (`Helpers/Profile/HandbookHelper.cs:106-139`) — a tpl the
/// handbook does not price is worth 0.
fn get_template_price(config: &BotLootConfig, tpl: &str) -> f64 {
    config.handbook_prices.get(tpl).copied().unwrap_or(0.0)
}

/// `BotLootGenerator.CreateWalletLoot` (`:616-640`).
///
/// # Errors
///
/// From `get_weighted_value`, plus the `int.Parse` at `:633` — a stack-size weight key that is not a
/// number is a `FormatException` there.
pub fn create_wallet_loot(
    config: &BotLootConfig,
    wallet_id: &str,
) -> Result<Vec<Vec<Item>>, LootError> {
    let mut result = Vec::new();

    // Choose how many stacks of currency will be added to wallet
    let item_count = get_int(
        config.wallet_loot.item_count.min,
        config.wallet_loot.item_count.max,
    );
    for _ in 0..item_count {
        // Choose the size of the currency stack - default is 5k, 10k, 15k, 20k, 25k
        let chosen_stack_count = get_weighted_value(&config.wallet_loot.stack_size_weight)?;
        let stack_objects_count: f64 = chosen_stack_count.parse().map_err(|_| {
            LootError::new(format!(
                "Wallet stack size weighting key is not a number: {chosen_stack_count}"
            ))
        })?;

        result.push(vec![Item {
            id: mongo_id::generate(),
            template: get_weighted_value(&config.wallet_loot.currency_weight)?,
            parent_id: Some(wallet_id.to_owned()),
            upd: Some(Upd {
                stack_objects_count: Some(stack_objects_count),
                ..Default::default()
            }),
            ..Default::default()
        }]);
    }

    Ok(result)
}

/// `InventoryHelper.GetContainerSlotMap` (`Helpers/Profile/InventoryHelper.cs:906-916`) — a blank
/// grid sized from the container's **first** grid.
///
/// C# NREs on a tpl that is missing from the database or declares no grids; both yield an empty grid
/// here, which fails every `FindSlotForItem` and so lands on the same "does not fit" branch.
fn get_container_slot_map(
    items_view: &IndexMap<String, ItemView>,
    container_tpl: &str,
) -> Vec<Vec<u8>> {
    let Some(first_grid) = get_item(items_view, container_tpl)
        .and_then(|template| template.grids.as_ref())
        .and_then(|grids| grids.first())
    else {
        return Vec::new();
    };

    // Rows = **CellsH**, columns = **CellsV** — the axes come out swapped, and deliberately so:
    // `GetContainerSlotMap` names them backwards (`containerRowCount = CellsH`,
    // `containerColumnCount = CellsV`, `:911-913`) and then hands them to
    // `GetBlankContainerMap(horizontalSizeX: CellsV, verticalSizeY: CellsH)` (`:915`), which builds
    // `new int[verticalSizeY, horizontalSizeX]` (`ItemHelper.cs:1804-1807`) — i.e.
    // `new int[CellsH, CellsV]`. Note this is the *opposite* of
    // `ContainerGrids::add_empty_container`, whose `BotInventoryContainerService` source sizes
    // `int[CellsV, CellsH]`; the two C# helpers genuinely disagree and only square grids hide it.
    vec![
        vec![0u8; first_grid.cells_v.unwrap_or(0).max(0) as usize];
        first_grid.cells_h.unwrap_or(0).max(0) as usize
    ]
}

/// `InventoryHelper.CanPlaceItemsInContainer` (`:215-218`) over
/// `CanPlaceItemInContainer` (`:226-262`) — every item must fit, and each one that does is stamped
/// into the grid before the next is tried.
fn can_place_items_in_container(
    ctx: &mut BotContext,
    container_2d: &mut [Vec<u8>],
    items_with_children: &[Vec<Item>],
) -> bool {
    items_with_children.iter().all(|item_with_children| {
        can_place_item_in_container(ctx, container_2d, item_with_children)
    })
}

/// `InventoryHelper.CanPlaceItemInContainer` (`:226-262`).
fn can_place_item_in_container(
    ctx: &mut BotContext,
    container_2d: &mut [Vec<u8>],
    item_with_children: &[Item],
) -> bool {
    let items = ctx.items;
    let Some(root_item) = item_with_children.first() else {
        return false;
    };

    // Get x/y size of item
    let (size_x, size_y) = get_item_size(
        items,
        &root_item.template,
        &root_item.id,
        item_with_children,
        &mut ctx.diagnostics,
    );

    // Look for a place to slot item into
    let find_slot_result = find_slot_for_item(container_2d, size_x, size_y);
    if !find_slot_result.success {
        return false;
    }

    try_fill_container_map_with_item(
        container_2d,
        find_slot_result.x,
        find_slot_result.y,
        size_x,
        size_y,
        find_slot_result.rotation,
    );

    true
}

/// `InventoryHelper.PlaceItemInContainer` (`:268-315`), with the `desiredSlotId` the wallet path
/// passes (`"main"`).
fn place_item_in_container(
    ctx: &mut BotContext,
    container_2d: &mut [Vec<u8>],
    item_with_children: &mut [Item],
    container_id: &str,
    desired_slot_id: &str,
) {
    let items = ctx.items;
    let Some(root_item_added) = item_with_children.first() else {
        return;
    };

    // Get x/y size of item
    let (size_x, size_y) = get_item_size(
        items,
        &root_item_added.template,
        &root_item_added.id,
        item_with_children,
        &mut ctx.diagnostics,
    );

    // Look for a place to slot item into
    let find_slot_result = find_slot_for_item(container_2d, size_x, size_y);
    if !find_slot_result.success {
        return;
    }

    try_fill_container_map_with_item(
        container_2d,
        find_slot_result.x,
        find_slot_result.y,
        size_x,
        size_y,
        find_slot_result.rotation,
    );

    // Store details for object, including container item will be placed in
    let root_item_added = &mut item_with_children[0];
    root_item_added.parent_id = Some(container_id.to_owned());
    root_item_added.slot_id = Some(desired_slot_id.to_owned());
    root_item_added.location = serde_json::to_value(ItemLocation {
        x: Some(find_slot_result.x),
        y: Some(find_slot_result.y),
        r: if find_slot_result.rotation {
            ItemRotation::Vertical
        } else {
            ItemRotation::Horizontal
        },
        is_searched: None,
        rotation: Some(find_slot_result.rotation),
    })
    .ok();
}

/// `BotLootGenerator.AddRequiredChildItemsToParent` (`:649-671`).
///
/// C# is handed the resolved `TemplateItem` and reads its `Id`; this takes the tpl, which is what
/// the flattened [`ItemView`] cannot carry.
///
/// # Errors
///
/// From [`randomise_money_stack_size`].
pub fn add_required_child_items_to_parent(
    ctx: &mut BotContext,
    config: &BotLootConfig,
    item_to_add_tpl: &str,
    item_to_add_children_to: &mut Vec<Item>,
    is_pmc: bool,
    bot_role: &str,
) -> Result<(), LootError> {
    let items = ctx.items;

    // Fill ammo box
    if is_of_baseclass(items, item_to_add_tpl, AMMO_BOX) {
        if let Err(diagnostic) =
            add_cartridges_to_ammo_box(items, item_to_add_children_to, item_to_add_tpl)
        {
            ctx.diagnostics.push(diagnostic);
        }
    }
    // Make money a stack
    else if is_of_baseclass(items, item_to_add_tpl, MONEY) {
        let Some(money_item) = item_to_add_children_to.first_mut() else {
            return Ok(());
        };

        randomise_money_stack_size(config, bot_role, money_item)?;
    }
    // Make ammo a stack
    else if is_of_baseclass(items, item_to_add_tpl, AMMO) {
        let Some(item_template) = get_item(items, item_to_add_tpl) else {
            return Ok(());
        };
        let Some(ammo_item) = item_to_add_children_to.first_mut() else {
            return Ok(());
        };

        randomise_ammo_stack_size(is_pmc, item_template, ammo_item);
    }
    // Must add soft inserts/plates
    else if item_requires_soft_inserts(items, item_to_add_tpl) {
        *item_to_add_children_to = add_child_slot_items(
            items,
            &mut ctx.diagnostics,
            std::mem::take(item_to_add_children_to),
            item_to_add_tpl,
            None,
        );
    }

    Ok(())
}

/// `BotLootGenerator.AddLooseWeaponsToInventorySlot` (`:683-746`).
///
/// # Errors
///
/// From [`generate_random_weapon`].
pub fn add_loose_weapons_to_inventory_slot(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    config: &BotLootConfig,
    bot_inventory: &mut Vec<Item>,
    equipment_slot: &str,
    details: &BotGenerationDetailsWire,
    template_inventory: &mut BotTypeInventoryWire,
) -> Result<(), LootError> {
    // Three-to-one in favour of a primary weapon
    let chosen_weapon_type = *get_array_value(&[
        FIRST_PRIMARY_WEAPON,
        FIRST_PRIMARY_WEAPON,
        FIRST_PRIMARY_WEAPON,
        HOLSTER,
    ]);
    let randomised_weapon_count = get_int(
        config.pmc.loose_weapon_in_backpack_loot_min_max.min,
        config.pmc.loose_weapon_in_backpack_loot_min_max.max,
    );

    if randomised_weapon_count <= 0 {
        return Ok(());
    }

    for _ in 0..randomised_weapon_count {
        let generated_weapon = generate_random_weapon(
            ctx,
            chosen_weapon_type,
            template_inventory,
            details,
            config.equipment_id,
            config.weapon_mod_chances,
        )?;

        // C# dereferences a null `GenerateRandomWeapon` result before it reaches this check; a
        // missing weapon tpl takes the same "null loose weapon" exit here instead of crashing.
        let mut weapon = generated_weapon
            .map(|result| result.weapon)
            .unwrap_or_default();
        let Some(weapon_root_item) = weapon.first() else {
            ctx.diagnostics.push(plain(
                ERROR,
                format!(
                    "Generated null loose weapon: {chosen_weapon_type} for: {} level: {}, skipping",
                    details.role_lowercase, details.bot_level
                ),
            ));

            continue;
        };
        let (root_id, root_tpl) = (
            weapon_root_item.id.clone(),
            weapon_root_item.template.clone(),
        );

        let result = grids.add_item_with_children_to_equipment_slot(
            ctx,
            &[equipment_slot.to_owned()],
            &root_id,
            &root_tpl,
            &mut weapon,
            bot_inventory,
        );

        if result != ItemAddedResult::Success {
            let reason = item_added_result_name(result);
            ctx.diagnostics.push(plain(
                DEBUG,
                format!(
                    "Failed to add additional weapon: {root_id} to bot backpack, reason: {reason}"
                ),
            ));
        }
    }

    Ok(())
}

/// `BotLootGenerator.ItemHasReachedSpawnLimit` (`:755-813`).
///
/// The `itemSpawnLimits is null` arms are the caller's `Option` here; what survives is the
/// empty-`GlobalLimits` exit and the two dead branches below.
fn item_has_reached_spawn_limit(
    ctx: &mut BotContext,
    item_template: &ItemView,
    item_tpl: &str,
    bot_role: &str,
    item_spawn_limits: &mut ItemSpawnLimitSettingsWire,
) -> bool {
    // PMCs and scavs have different sections of bot config for spawn limits
    if item_spawn_limits.global_limits.is_empty() {
        // No items found in spawn limit, drop out
        return false;
    }

    // ParentId or tplid not found in spawnLimits, not a spawn limited item, skip
    let Some(id_to_check_for) = get_matching_id_from_spawn_limits(
        item_template,
        item_tpl,
        &item_spawn_limits.global_limits,
    ) else {
        return false;
    };

    // `TryAdd(id, 1)` seeds an absent counter at 1; an existing one is incremented.
    let current_limit_count = item_spawn_limits
        .current_limits
        .entry(id_to_check_for.clone())
        .and_modify(|count| *count += 1.0)
        .or_insert(1.0);
    let current_limit_count = *current_limit_count;

    // Check if over limit
    if current_limit_count > item_spawn_limits.global_limits[&id_to_check_for] {
        // Prevent edge-case of small loot pools + code trying to add limited item over and over
        // infinitely. Dead: `count > count * 10` is false for every count this can hold (it is at
        // least 1 by the time it is read), so the escape hatch never opens and the log line at
        // `:793` is unreachable — as is the `return false` under it.
        if current_limit_count > current_limit_count * 10.0 {
            ctx.diagnostics.push(localised(
                DEBUG,
                "bot-item_spawn_limit_reached_skipping_item",
                serde_json::json!({
                    "botRole": bot_role,
                    "itemName": item_template.name.clone().unwrap_or_default(),
                    "attempts": current_limit_count,
                }),
            ));

            return false;
        }

        return true;
    }

    false
}

/// `BotLootGenerator.RandomiseMoneyStackSize` (`:821-834`).
///
/// # Errors
///
/// Where the C# throws: the missing `"default"` role fallback (`:826`), the **unguarded**
/// `currencyWeights[moneyItem.Template]` indexer (`:829`) for a money tpl the role has no weights
/// for, and the `int.Parse` at `:833`.
pub fn randomise_money_stack_size(
    config: &BotLootConfig,
    bot_role: &str,
    money_item: &mut Item,
) -> Result<(), LootError> {
    // Get all currency weights for this bot type
    let currency_weights = match config.currency_stack_size.get(bot_role) {
        Some(currency_weights) => currency_weights,
        None => config
            .currency_stack_size
            .get("default")
            .ok_or_else(|| key_not_found("default"))?,
    };

    let currency_weight = currency_weights
        .get(&money_item.template)
        .ok_or_else(|| key_not_found(&money_item.template))?;

    let chosen = get_weighted_value(currency_weight)?;
    let stack_objects_count: f64 = chosen.parse().map_err(|_| {
        LootError::new(format!(
            "Currency stack size weighting key is not a number: {chosen}"
        ))
    })?;

    money_item
        .upd
        .get_or_insert_with(Upd::default)
        .stack_objects_count = Some(stack_objects_count);

    Ok(())
}

/// `BotLootGenerator.RandomiseAmmoStackSize` (`:842-848`).
fn randomise_ammo_stack_size(_is_pmc: bool, item_template: &ItemView, ammo_item: &mut Item) {
    // parity: parameter unused in C#
    let _ = _is_pmc;

    let random_size = get_randomised_ammo_stack_size(item_template);

    ammo_item
        .upd
        .get_or_insert_with(Upd::default)
        .stack_objects_count = Some(f64::from(random_size));
}

/// `BotLootGenerator.GetItemSpawnLimitsForBot` (`:47-58`) — the zeroed running total plus an
/// untouched reference copy.
///
/// Both copies come from a second [`get_item_spawn_limits_for_bot_type`] call, exactly as the C#
/// does, so an unknown role logs its fallback warning **twice**.
///
/// # Errors
///
/// From [`get_item_spawn_limits_for_bot_type`].
pub fn get_item_spawn_limits_for_bot(
    ctx: &mut BotContext,
    config: &BotLootConfig,
    bot_role: &str,
) -> Result<ItemSpawnLimitSettingsWire, LootError> {
    // Clone limits and set all values to 0 to use as a running total
    let mut current_limits = get_item_spawn_limits_for_bot_type(ctx, config, bot_role)?;
    for limit in current_limits.values_mut() {
        *limit = 0.0;
    }

    Ok(ItemSpawnLimitSettingsWire {
        current_limits,
        global_limits: get_item_spawn_limits_for_bot_type(ctx, config, bot_role)?,
    })
}

/// `BotLootGenerator.GetItemSpawnLimitsForBotType` (`:856-871`).
///
/// # Errors
///
/// The `ItemSpawnLimits["pmc"]` indexer (`:860`), which is a `KeyNotFoundException` for a config
/// with no pmc section.
pub fn get_item_spawn_limits_for_bot_type(
    ctx: &mut BotContext,
    config: &BotLootConfig,
    bot_role: &str,
) -> Result<IndexMap<String, f64>, LootError> {
    if is_bot_pmc(Some(bot_role)) {
        return config
            .item_spawn_limits
            .get("pmc")
            .cloned()
            .ok_or_else(|| key_not_found("pmc"));
    }

    if let Some(limits) = config.item_spawn_limits.get(&bot_role.to_lowercase()) {
        return Ok(limits.clone());
    }

    ctx.diagnostics.push(localised(
        WARNING,
        "bot-unable_to_find_spawn_limits_fallback_to_defaults",
        serde_json::Value::String(bot_role.to_owned()),
    ));

    Ok(IndexMap::new())
}

/// `BotLootGenerator.GetMatchingIdFromSpawnLimits` (`:879-894`).
pub fn get_matching_id_from_spawn_limits(
    item_template: &ItemView,
    item_tpl: &str,
    spawn_limits: &IndexMap<String, f64>,
) -> Option<String> {
    if spawn_limits.contains_key(item_tpl) {
        return Some(item_tpl.to_owned());
    }

    // tplId not found in spawnLimits, check if parentId is
    let parent = item_template.parent.as_deref()?;
    if spawn_limits.contains_key(parent) {
        return Some(parent.to_owned());
    }

    // parentId and tplId not found
    None
}

/// The C# `KeyNotFoundException` message.
fn key_not_found(key: &str) -> LootError {
    LootError::new(format!(
        "The given key '{key}' was not present in the dictionary."
    ))
}

fn plain(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

fn localised(level: &str, locale_key: &str, args: serde_json::Value) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args: Some(args),
        message: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::bot::durability_limits_helper::BotDurability;
    use crate::bot::models::{EquipmentFilters, RandomisedResourceDetails};
    use crate::loot::models::PresetView;
    use crate::loot::random_util::{TestSeedGuard, get_double};

    const SEED: u64 = 42;

    const POCKETS_TPL: &str = "cont_pockets";
    const VEST_TPL: &str = "cont_vest";
    const BACKPACK_TPL: &str = "cont_backpack";
    const SECURE_TPL: &str = "cont_secure";
    const WALLET_TPL: &str = "wallet";
    /// A deliberately **non-square** wallet: 3 cells horizontally, 2 vertically. Every vanilla
    /// wallet is 2x2, which hides `GetContainerSlotMap`'s axis swap; this one does not.
    const WALLET_TALL_TPL: &str = "wallet_tall";
    const ROUBLES: &str = "money_rub";
    const DOLLARS: &str = "money_usd";
    const RIFLE: &str = "rifle";
    const MAG: &str = "mag";
    const AMMO_PS: &str = "ammo_ps";
    const RIFLE_CALIBER: &str = "Caliber762x39";
    const AMMO_BOX_TPL: &str = "ammo_box";

    struct Fixture {
        items: IndexMap<String, ItemView>,
        bosses: Vec<String>,
        durability: BotDurability,
        equipment: IndexMap<String, EquipmentFilters>,
        randomization: IndexMap<String, RandomisedResourceDetails>,
        presets: IndexMap<String, PresetView>,
        item_counts: ItemCountsWire,
        item_spawn_limits: IndexMap<String, IndexMap<String, f64>>,
        wallet_loot: WalletLootSettingsWire,
        currency_stack_size: IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
        pmc: PmcConfigWire,
        handbook_prices: IndexMap<String, f64>,
        loot_pools: BotLootCacheWire,
        disable_loot_on_bot_types: HashSet<String>,
        mod_chances: IndexMap<String, f64>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                items: serde_json::from_value(json!({
                    POCKETS_TPL: {"grids": [{"name": "main", "cellsH": 4, "cellsV": 4}]},
                    VEST_TPL: {"grids": [{"name": "main", "cellsH": 4, "cellsV": 4}]},
                    BACKPACK_TPL: {"grids": [{"name": "main", "cellsH": 4, "cellsV": 4}]},
                    SECURE_TPL: {"grids": [{"name": "main", "cellsH": 1, "cellsV": 1}]},
                    "special1": {"name": "special", "width": 1, "height": 1},
                    "heal1": {"name": "medkit", "width": 1, "height": 1},
                    "drug1": {"name": "drug", "width": 1, "height": 1},
                    "food1": {"name": "food", "width": 1, "height": 1},
                    "drink1": {"name": "drink", "width": 1, "height": 1},
                    "stim1": {"name": "stim", "width": 1, "height": 1},
                    "grenade1": {"name": "grenade", "width": 1, "height": 1},
                    "bp1": {"name": "backpack loot", "width": 1, "height": 1},
                    "pocket1": {"name": "pocket loot", "width": 1, "height": 1},
                    "vestloot1": {"name": "vest loot", "width": 1, "height": 1},
                    "secureloot1": {"name": "secure loot", "width": 1, "height": 1},
                    ROUBLES: {"name": "roubles", "parent": MONEY, "width": 1, "height": 1},
                    DOLLARS: {"name": "dollars", "parent": MONEY, "width": 1, "height": 1},
                    AMMO_PS: {"name": "PS", "parent": AMMO, "caliber": RIFLE_CALIBER,
                        "stackMaxSize": 60, "stackMinRandom": 5, "stackMaxRandom": 10,
                        "width": 1, "height": 1},
                    AMMO_BOX_TPL: {"name": "ammo box", "parent": AMMO_BOX, "width": 1, "height": 1,
                        "stackSlotMaxCount": 20, "stackSlotFirstFilterFirst": AMMO_PS},
                    WALLET_TPL: {"name": "wallet", "width": 1, "height": 1,
                        "grids": [{"name": "main", "cellsH": 2, "cellsV": 2}]},
                    WALLET_TALL_TPL: {"name": "tall wallet", "width": 1, "height": 1,
                        "grids": [{"name": "main", "cellsH": 3, "cellsV": 2}]},
                    RIFLE: {"name": "rifle", "parent": crate::loot::item_helper::WEAPON,
                        "width": 3, "height": 1, "weapClass": "assaultRifle",
                        "maxDurability": 100.0, "caliber": RIFLE_CALIBER, "defAmmo": AMMO_PS,
                        "defMagType": MAG, "reloadMode": "ExternalMagazine",
                        "isChamberLoad": false,
                        "chambers": [{"name": "patron_in_weapon", "filter": [AMMO_PS]}],
                        "slots": [{"name": "mod_magazine", "required": true, "filter": [MAG]}]},
                    crate::loot::item_helper::MAGAZINE: {"name": "magazine"},
                    MAG: {"name": "30-round mag", "parent": crate::loot::item_helper::MAGAZINE,
                        "cartridgesMaxCount": 30, "cartridgesFirstFilter": [AMMO_PS],
                        "reloadMagType": "ExternalMagazine", "width": 1, "height": 1,
                        "cartridges": [{"name": "cartridges", "filter": [AMMO_PS]}]},
                }))
                .unwrap(),
                bosses: Vec::new(),
                durability: serde_json::from_value(json!({
                    "default": {"armor": {"maxDelta": 10, "minDelta": 0, "minLimitPercent": 15},
                        "weapon": {"lowestMax": 60, "highestMax": 100, "maxDelta": 10,
                                   "minDelta": 0, "minLimitPercent": 15.0}},
                    "botDurabilities": {},
                    "pmc": {"armor": {"lowestMaxPercent": 90, "highestMaxPercent": 100,
                                      "maxDelta": 10, "minDelta": 0, "minLimitPercent": 15},
                        "weapon": {"lowestMax": 95, "highestMax": 100, "maxDelta": 5,
                                   "minDelta": 0, "minLimitPercent": 15.0}},
                }))
                .unwrap(),
                equipment: serde_json::from_value(json!({
                    "pmc": {"weaponModLimits": {"scopeLimit": 2, "lightLaserLimit": 1}},
                    "assault": {"weaponModLimits": {"scopeLimit": 2, "lightLaserLimit": 1}},
                }))
                .unwrap(),
                randomization: IndexMap::new(),
                presets: serde_json::from_value(json!({
                    "p_rifle": {"id": "p_rifle", "items": [
                        {"_id": "preset_root", "_tpl": RIFLE},
                        {"_id": "preset_mag", "_tpl": MAG, "parentId": "preset_root",
                         "slotId": "mod_magazine"},
                    ]},
                }))
                .unwrap(),
                // Single-entry weights: `GetWeightedValue` returns without drawing, so the pinned
                // run below is not hostage to the count draws (`counts_two_entry` exercises those).
                item_counts: serde_json::from_value(json!({
                    "backpackLoot": {"weights": {"2": 1}},
                    "pocketLoot": {"weights": {"1": 1}},
                    "vestLoot": {"weights": {"1": 1}},
                    "specialItems": {"weights": {"1": 1}},
                    "healing": {"weights": {"1": 1}},
                    "drugs": {"weights": {"1": 1}},
                    "food": {"weights": {"1": 1}},
                    "drink": {"weights": {"1": 1}},
                    "currency": {"weights": {"1": 1}},
                    "stims": {"weights": {"1": 1}},
                    "grenades": {"weights": {"1": 1}},
                }))
                .unwrap(),
                item_spawn_limits: IndexMap::from([
                    ("pmc".to_owned(), IndexMap::new()),
                    ("assault".to_owned(), IndexMap::new()),
                ]),
                wallet_loot: serde_json::from_value(json!({
                    "chancePercent": 100.0,
                    "itemCount": {"min": 2, "max": 2},
                    "stackSizeWeight": {"5000": 1, "10000": 1},
                    "currencyWeight": {ROUBLES: 1},
                    "walletTplPool": [WALLET_TPL, WALLET_TALL_TPL],
                }))
                .unwrap(),
                currency_stack_size: serde_json::from_value(json!({
                    "default": {ROUBLES: {"1000": 1, "2000": 1}},
                }))
                .unwrap(),
                pmc: serde_json::from_value(json!({
                    "forceHealingItemsIntoSecure": false,
                    "looseWeaponInBackpackChancePercent": 0.0,
                    "looseWeaponInBackpackLootMinMax": {"min": 1, "max": 1},
                    "addSecureContainerLootFromBotConfig": true,
                    "lootItemLimitsRub": [
                        {"min": 1, "max": 20, "backpack": {"min": 0, "max": 5000},
                         "pocket": {"min": 0, "max": 1000}, "vest": {"min": 0, "max": 2000}},
                    ],
                    "lootSettings": {
                        "backpack": {"totalRubByLevel": [{"min": 1, "max": 99, "value": 100000}],
                                     "locationMultiplier": {"default": 1.0}},
                        "vest": {"totalRubByLevel": [{"min": 1, "max": 99, "value": 100000}],
                                 "locationMultiplier": {"default": 1.0}},
                        "pocket": {"totalRubByLevel": [{"min": 1, "max": 99, "value": 100000}],
                                   "locationMultiplier": {"default": 1.0}},
                    },
                }))
                .unwrap(),
                handbook_prices: IndexMap::from([("bp1".to_owned(), 500.0)]),
                loot_pools: serde_json::from_value(json!({
                    "specialItems": {"special1": 1},
                    "healingItems": {"heal1": 1},
                    "drugItems": {"drug1": 1},
                    "foodItems": {"food1": 1},
                    "drinkItems": {"drink1": 1},
                    "currencyItems": {ROUBLES: 1},
                    "stimItems": {"stim1": 1},
                    "grenadeItems": {"grenade1": 1},
                    "backpackLoot": {"bp1": 1},
                    "pocketLoot": {"pocket1": 1},
                    "vestLoot": {"vestloot1": 1},
                    "secureLoot": {"secureloot1": 1},
                }))
                .unwrap(),
                disable_loot_on_bot_types: HashSet::from(["bossnoloot".to_owned()]),
                mod_chances: IndexMap::new(),
            }
        }

        fn ctx(&self) -> BotContext<'_> {
            BotContext {
                items: &self.items,
                bosses: &self.bosses,
                durability: &self.durability,
                equipment: &self.equipment,
                loot_item_resource_randomization: &self.randomization,
                item_blacklist: &crate::bot::NO_BLACKLIST,
                default_presets_by_tpl: &crate::bot::NO_PRESETS,
                presets_by_id: &crate::bot::NO_PRESETS,
                item_presets: &self.presets,
                equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                low_profile_gas_block_tpls: &crate::bot::NO_BLACKLIST,
                weapon_has_enhancement_chance_percent: 0.0,
                repair_kit_weapon: &crate::bot::NO_BUFFS,
                secure_container_ammo_stack_count: 0,
                is_night_time: false,
                diagnostics: Vec::new(),
            }
        }

        fn config(&self) -> BotLootConfig<'_> {
            BotLootConfig {
                equipment_id: "equipment1",
                item_counts: &self.item_counts,
                weapon_mod_chances: &self.mod_chances,
                disable_loot_on_bot_types: &self.disable_loot_on_bot_types,
                item_spawn_limits: &self.item_spawn_limits,
                wallet_loot: &self.wallet_loot,
                currency_stack_size: &self.currency_stack_size,
                pmc: &self.pmc,
                handbook_prices: &self.handbook_prices,
                loot_pools: &self.loot_pools,
            }
        }
    }

    fn details(role: &str, is_pmc: bool) -> BotGenerationDetailsWire {
        serde_json::from_value(json!({
            "role": role, "roleLowercase": role, "side": "Usec", "botLevel": 15,
            "isPmc": is_pmc, "isPlayerScav": false, "gameVersion": "standard",
            "location": "bigmap", "botDifficulty": "normal",
            "clearBotContainerCacheAfterGeneration": true,
        }))
        .unwrap()
    }

    fn container(id: &str, tpl: &str, slot: &str) -> Item {
        serde_json::from_value(json!({"_id": id, "_tpl": tpl, "parentId": "equipment1",
                                      "slotId": slot}))
        .unwrap()
    }

    /// A bot wearing all four containers, with the grids registered the way
    /// `BotInventoryGenerator` registers them.
    fn bot_with_containers(ctx: &BotContext) -> (ContainerGrids, Vec<Item>) {
        let mut grids = ContainerGrids::default();
        let mut inventory = Vec::new();
        for (id, tpl, slot) in [
            ("pockets1", POCKETS_TPL, POCKETS),
            ("vest1", VEST_TPL, TACTICAL_VEST),
            ("backpack1", BACKPACK_TPL, BACKPACK),
            ("secure1", SECURE_TPL, SECURED_CONTAINER),
        ] {
            let item = container(id, tpl, slot);
            grids.add_empty_container(ctx, slot, &item);
            inventory.push(item);
        }

        (grids, inventory)
    }

    /// `(tpl, slotId, parent)` with generated ids replaced by their index, so a run is comparable
    /// across the fresh `MongoId`s every loot item gets.
    fn normalized(items: &[Item]) -> Vec<(String, String, String)> {
        let ids: IndexMap<&str, String> = items
            .iter()
            .enumerate()
            .map(|(index, item)| (item.id.as_str(), format!("#{index}")))
            .collect();

        items
            .iter()
            .map(|item| {
                (
                    item.template.clone(),
                    item.slot_id.clone().unwrap_or_default(),
                    item.parent_id
                        .as_deref()
                        .map(|parent| {
                            ids.get(parent)
                                .cloned()
                                .unwrap_or_else(|| parent.to_owned())
                        })
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    // -----------------------------------------------------------------------
    // generate_loot
    // -----------------------------------------------------------------------

    #[test]
    fn a_seeded_pmc_run_fills_every_container_from_its_pools() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        generate_loot(
            &mut ctx,
            &mut grids,
            &mut inventory,
            &details("pmcusec", true),
            &mut BotTypeInventoryWire::default(),
            &fixture.config(),
        )
        .unwrap();

        // The four containers, then one item per pool in C# call order: special, healing, drugs,
        // food, drink, currency, stims, grenades, backpack (x2), vest, pockets, secure. The first
        // eight all land in the pockets (`#0`): `GetAvailableContainersBotCanStoreItemsIn` puts
        // `Pockets` first and the grid has room. The backpack pool is slot-restricted to `#2`, the
        // vest pool to `#1`, and grenades never see the backpack at all (`:261`).
        assert_eq!(
            normalized(&inventory),
            vec![
                (
                    POCKETS_TPL.to_owned(),
                    POCKETS.to_owned(),
                    "equipment1".to_owned()
                ),
                (
                    VEST_TPL.to_owned(),
                    TACTICAL_VEST.to_owned(),
                    "equipment1".to_owned()
                ),
                (
                    BACKPACK_TPL.to_owned(),
                    BACKPACK.to_owned(),
                    "equipment1".to_owned()
                ),
                (
                    SECURE_TPL.to_owned(),
                    SECURED_CONTAINER.to_owned(),
                    "equipment1".to_owned()
                ),
                ("special1".to_owned(), "main".to_owned(), "#0".to_owned()),
                ("heal1".to_owned(), "main".to_owned(), "#0".to_owned()),
                ("drug1".to_owned(), "main".to_owned(), "#0".to_owned()),
                ("food1".to_owned(), "main".to_owned(), "#0".to_owned()),
                ("drink1".to_owned(), "main".to_owned(), "#0".to_owned()),
                (ROUBLES.to_owned(), "main".to_owned(), "#0".to_owned()),
                ("stim1".to_owned(), "main".to_owned(), "#0".to_owned()),
                ("grenade1".to_owned(), "main".to_owned(), "#0".to_owned()),
                ("bp1".to_owned(), "main".to_owned(), "#2".to_owned()),
                ("bp1".to_owned(), "main".to_owned(), "#2".to_owned()),
                ("vestloot1".to_owned(), "main".to_owned(), "#1".to_owned()),
                ("pocket1".to_owned(), "main".to_owned(), "#0".to_owned()),
                ("secureloot1".to_owned(), "main".to_owned(), "#3".to_owned()),
            ]
        );
    }

    #[test]
    fn a_disabled_role_still_consumes_all_eleven_count_draws() {
        let mut fixture = Fixture::new();
        // Two entries per block, so every one of the eleven actually draws.
        fixture.item_counts = serde_json::from_value(json!({
            "backpackLoot": {"weights": {"1": 1, "2": 2}},
            "pocketLoot": {"weights": {"1": 1, "2": 2}},
            "vestLoot": {"weights": {"1": 1, "2": 2}},
            "specialItems": {"weights": {"1": 1, "2": 2}},
            "healing": {"weights": {"1": 1, "2": 2}},
            "drugs": {"weights": {"1": 1, "2": 2}},
            "food": {"weights": {"1": 1, "2": 2}},
            "drink": {"weights": {"1": 1, "2": 2}},
            "currency": {"weights": {"1": 1, "2": 2}},
            "stims": {"weights": {"1": 1, "2": 2}},
            "grenades": {"weights": {"1": 1, "2": 2}},
        }))
        .unwrap();
        // Empty pools, so nothing past the eleven draws consumes anything.
        fixture.loot_pools = BotLootCacheWire::default();
        fixture.disable_loot_on_bot_types = HashSet::from(["bossnoloot".to_owned()]);

        let weights: IndexMap<String, f64> =
            IndexMap::from([("1".to_owned(), 1.0), ("2".to_owned(), 2.0)]);
        let eleven_draws = stream_position_after(|| {
            for _ in 0..11 {
                get_weighted_value(&weights).unwrap();
            }
        });

        for role in ["assault", "bossnoloot"] {
            let mut ctx = fixture.ctx();
            let mut inventory = Vec::new();
            let after_run = stream_position_after(|| {
                generate_loot(
                    &mut ctx,
                    &mut ContainerGrids::default(),
                    &mut inventory,
                    &details(role, false),
                    &mut BotTypeInventoryWire::default(),
                    &fixture.config(),
                )
                .unwrap();
            });

            // The zeroing at `:110-116` happens after the draws, never instead of them.
            assert_eq!(after_run, eleven_draws, "role: {role}");
        }
    }

    #[test]
    fn a_missing_weights_block_warns_and_draws_nothing() {
        let mut fixture = Fixture::new();
        fixture.item_counts.grenades = None;
        let mut ctx = fixture.ctx();
        let mut inventory = Vec::new();

        let after_run = stream_position_after(|| {
            generate_loot(
                &mut ctx,
                &mut ContainerGrids::default(),
                &mut inventory,
                &details("assault", false),
                &mut BotTypeInventoryWire::default(),
                &fixture.config(),
            )
            .unwrap();
        });

        assert_eq!(after_run, stream_position_after(|| {}));
        assert!(inventory.is_empty());
        assert_eq!(
            ctx.diagnostics[0].locale_key.as_deref(),
            Some("bot-unable_to_generate_bot_loot")
        );
    }

    #[test]
    fn the_vest_runs_without_a_count_check_but_the_backpack_does_not() {
        let mut fixture = Fixture::new();
        fixture.item_counts = serde_json::from_value(json!({
            "backpackLoot": {"weights": {"0": 1}},
            "pocketLoot": {"weights": {"0": 1}},
            "vestLoot": {"weights": {"0": 1}},
            "specialItems": {"weights": {"0": 1}},
            "healing": {"weights": {"0": 1}},
            "drugs": {"weights": {"0": 1}},
            "food": {"weights": {"0": 1}},
            "drink": {"weights": {"0": 1}},
            "currency": {"weights": {"0": 1}},
            "stims": {"weights": {"0": 1}},
            "grenades": {"weights": {"0": 1}},
        }))
        .unwrap();
        // A PMC, so the backpack branch would also roll for a loose weapon if it ran at all.
        fixture.pmc.loose_weapon_in_backpack_chance_percent = 100.0;

        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        generate_loot(
            &mut ctx,
            &mut grids,
            &mut inventory,
            &details("pmcusec", true),
            &mut BotTypeInventoryWire::default(),
            &fixture.config(),
        )
        .unwrap();

        // A zero backpack count skips the whole branch — including the loose weapon — while the
        // zero vest count still enters `AddLootFromPool` and falls straight out of its loop. Only
        // the secure container, whose 50 is hardcoded, adds anything.
        assert_eq!(
            normalized(&inventory)
                .iter()
                .map(|(tpl, _, _)| tpl.clone())
                .collect::<Vec<_>>(),
            vec![
                POCKETS_TPL,
                VEST_TPL,
                BACKPACK_TPL,
                SECURE_TPL,
                "secureloot1"
            ]
        );

        // Same run with the vest count raised and the backpack count still zero: the vest branch
        // is the one that places an item, which is the positive half of the asymmetry — the gate
        // at `:317` is the slot, not the count, so only the backpack's `> 0` (`:273`) can skip.
        fixture.item_counts.vest_loot =
            serde_json::from_value(json!({"weights": {"1": 1}})).unwrap();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        generate_loot(
            &mut ctx,
            &mut grids,
            &mut inventory,
            &details("pmcusec", true),
            &mut BotTypeInventoryWire::default(),
            &fixture.config(),
        )
        .unwrap();

        let vest_item = &inventory[4];
        assert_eq!(vest_item.template, "vestloot1");
        assert_eq!(vest_item.parent_id.as_deref(), Some("vest1"));
        // Still no backpack loot and still no loose weapon, despite the 100% chance.
        assert!(
            !inventory
                .iter()
                .any(|item| item.template == "bp1" || item.template == RIFLE)
        );
    }

    // -----------------------------------------------------------------------
    // add_loot_from_pool
    // -----------------------------------------------------------------------

    #[test]
    fn an_item_over_its_spawn_limit_is_dropped_from_the_pool_and_the_loop_ends() {
        let mut fixture = Fixture::new();
        fixture.item_spawn_limits = IndexMap::from([(
            "assault".to_owned(),
            IndexMap::from([("special1".to_owned(), 1.0)]),
        )]);
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let mut limits = get_item_spawn_limits_for_bot(&mut ctx, &config, "assault").unwrap();
        let _guard = TestSeedGuard::install(SEED);

        // A one-tpl pool, a limit of 1 and a count of 20: the second draw trips the limit, empties
        // the pool and the `i--`/`continue` pair then exits on the empty-pool check instead of
        // spinning. The dead escape hatch at `:789` (`count > count * 10`) never opens.
        add_loot_from_pool(
            &mut ctx,
            &mut grids,
            &config,
            IndexMap::from([("special1".to_owned(), 1.0)]),
            &[POCKETS.to_owned()],
            20.0,
            &mut inventory,
            "assault",
            Some(&mut limits),
            0.0,
            false,
        )
        .unwrap();

        assert_eq!(inventory.len(), 5);
        assert_eq!(inventory[4].template, "special1");
        // One under the limit, one over it.
        assert_eq!(limits.current_limits["special1"], 2.0);
        assert_eq!(limits.global_limits["special1"], 1.0);
    }

    #[test]
    fn the_rouble_budget_stops_the_run_once_it_is_exceeded() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        // `bp1` is priced at 500; a 1200 budget fits two and breaks on the third.
        add_loot_from_pool(
            &mut ctx,
            &mut grids,
            &config,
            IndexMap::from([("bp1".to_owned(), 1.0)]),
            &[BACKPACK.to_owned()],
            10.0,
            &mut inventory,
            "assault",
            None,
            1200.0,
            false,
        )
        .unwrap();

        assert_eq!(inventory.len() - 4, 3);
    }

    #[test]
    fn a_negative_budget_never_gates_the_run() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        // The secure container's `-1` (`:380`) fails the `> 0` gate at `:600`, so the run is
        // bounded by the 16-cell grid rather than by value.
        add_loot_from_pool(
            &mut ctx,
            &mut grids,
            &config,
            IndexMap::from([("bp1".to_owned(), 1.0)]),
            &[BACKPACK.to_owned()],
            50.0,
            &mut inventory,
            "assault",
            None,
            -1.0,
            false,
        )
        .unwrap();

        assert_eq!(inventory.len() - 4, 16);
    }

    // -----------------------------------------------------------------------
    // create_wallet_loot
    // -----------------------------------------------------------------------

    #[test]
    fn a_wallet_gets_its_currency_stacks_placed_inside_its_own_grid() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        add_loot_from_pool(
            &mut ctx,
            &mut grids,
            &config,
            IndexMap::from([(WALLET_TPL.to_owned(), 1.0)]),
            &[BACKPACK.to_owned()],
            1.0,
            &mut inventory,
            "assault",
            None,
            0.0,
            false,
        )
        .unwrap();

        let wallet_id = inventory[4].id.clone();
        assert_eq!(inventory[4].template, WALLET_TPL);
        assert_eq!(inventory.len() - 4, 3);

        let currency: Vec<_> = inventory[5..]
            .iter()
            .map(|item| {
                (
                    item.template.as_str(),
                    item.parent_id.as_deref().unwrap(),
                    item.slot_id.as_deref().unwrap(),
                    item.location.clone().unwrap(),
                    item.upd.as_ref().unwrap().stack_objects_count.unwrap(),
                )
            })
            .collect();
        assert_eq!(
            currency,
            vec![
                (
                    ROUBLES,
                    wallet_id.as_str(),
                    "main",
                    json!({"x": 0, "y": 0, "r": "Horizontal", "rotation": false}),
                    5000.0
                ),
                (
                    ROUBLES,
                    wallet_id.as_str(),
                    "main",
                    json!({"x": 1, "y": 0, "r": "Horizontal", "rotation": false}),
                    10000.0
                ),
            ]
        );
    }

    /// `GetContainerSlotMap` (`InventoryHelper.cs:906-916`) sizes the wallet grid as
    /// `new int[CellsH, CellsV]` — rows from **CellsH**, columns from **CellsV** — the opposite of
    /// `BotInventoryContainerService`'s `int[CellsV, CellsH]`. A 3x2 wallet is 3 rows of 2 columns,
    /// so the third 1x1 stack wraps to `(0, 1)`; the un-swapped 2 rows of 3 columns would put it at
    /// `(2, 0)` instead. Every vanilla wallet is square, which is exactly why this is worth pinning.
    #[test]
    fn a_non_square_wallet_uses_cells_h_for_rows_and_cells_v_for_columns() {
        let mut fixture = Fixture::new();
        fixture.wallet_loot.item_count = crate::bot::repair_service::MinMax { min: 3, max: 3 };
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        add_loot_from_pool(
            &mut ctx,
            &mut grids,
            &config,
            IndexMap::from([(WALLET_TALL_TPL.to_owned(), 1.0)]),
            &[BACKPACK.to_owned()],
            1.0,
            &mut inventory,
            "assault",
            None,
            0.0,
            false,
        )
        .unwrap();

        assert_eq!(inventory[4].template, WALLET_TALL_TPL);
        let positions: Vec<_> = inventory[5..]
            .iter()
            .map(|item| {
                let location = item.location.as_ref().unwrap();
                (
                    location["x"].as_i64().unwrap(),
                    location["y"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(positions, vec![(0, 0), (1, 0), (0, 1)]);
    }

    #[test]
    fn wallet_stacks_that_do_not_fit_are_dropped_whole() {
        let mut fixture = Fixture::new();
        // Five stacks into a 2x2 grid: `CanPlaceItemsInContainer` fails and none are added.
        fixture.wallet_loot.item_count = crate::bot::repair_service::MinMax { min: 5, max: 5 };
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        add_loot_from_pool(
            &mut ctx,
            &mut grids,
            &config,
            IndexMap::from([(WALLET_TPL.to_owned(), 1.0)]),
            &[BACKPACK.to_owned()],
            1.0,
            &mut inventory,
            "assault",
            None,
            0.0,
            false,
        )
        .unwrap();

        assert_eq!(inventory.len() - 4, 1);
        assert_eq!(inventory[4].template, WALLET_TPL);
    }

    // -----------------------------------------------------------------------
    // add_required_child_items_to_parent
    // -----------------------------------------------------------------------

    #[test]
    fn money_with_no_weight_for_its_tpl_is_the_unguarded_indexer() {
        let fixture = Fixture::new();
        let config = fixture.config();
        let mut money = Item {
            id: "money1".to_owned(),
            template: DOLLARS.to_owned(),
            ..Default::default()
        };

        // `currencyWeights[moneyItem.Template]` (`:829`) has no `TryGetValue` around it, and the
        // default role's map only prices roubles.
        let error = randomise_money_stack_size(&config, "assault", &mut money).unwrap_err();

        assert_eq!(
            error.message,
            format!("The given key '{DOLLARS}' was not present in the dictionary.")
        );
    }

    #[test]
    fn money_ammo_and_ammo_boxes_each_take_their_own_arm() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let _guard = TestSeedGuard::install(SEED);

        let mut money = vec![Item {
            id: "m1".to_owned(),
            template: ROUBLES.to_owned(),
            ..Default::default()
        }];
        add_required_child_items_to_parent(
            &mut ctx, &config, ROUBLES, &mut money, false, "assault",
        )
        .unwrap();
        assert_eq!(
            money[0].upd.as_ref().unwrap().stack_objects_count,
            Some(1000.0)
        );

        let mut ammo = vec![Item {
            id: "a1".to_owned(),
            template: AMMO_PS.to_owned(),
            ..Default::default()
        }];
        add_required_child_items_to_parent(&mut ctx, &config, AMMO_PS, &mut ammo, true, "assault")
            .unwrap();
        // `GetRandomisedAmmoStackSize` between StackMinRandom and min(StackMaxRandom, 60).
        assert_eq!(ammo[0].upd.as_ref().unwrap().stack_objects_count, Some(6.0));

        let mut box_items = vec![Item {
            id: "b1".to_owned(),
            template: AMMO_BOX_TPL.to_owned(),
            ..Default::default()
        }];
        add_required_child_items_to_parent(
            &mut ctx,
            &config,
            AMMO_BOX_TPL,
            &mut box_items,
            false,
            "assault",
        )
        .unwrap();
        assert_eq!(box_items.len(), 2);
        assert_eq!(box_items[1].template, AMMO_PS);
    }

    // -----------------------------------------------------------------------
    // add_loose_weapons_to_inventory_slot
    // -----------------------------------------------------------------------

    #[test]
    fn a_loose_weapon_lands_in_the_backpack_with_a_loaded_magazine() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let config = fixture.config();
        let (mut grids, mut inventory) = bot_with_containers(&ctx);
        let mut template_inventory: BotTypeInventoryWire = serde_json::from_value(json!({
            "equipment": {"FirstPrimaryWeapon": {RIFLE: 1}, "Holster": {RIFLE: 1}},
            "Ammo": {RIFLE_CALIBER: {AMMO_PS: 1}},
            "items": {}, "mods": {},
        }))
        .unwrap();
        let _guard = TestSeedGuard::install(SEED);

        add_loose_weapons_to_inventory_slot(
            &mut ctx,
            &mut grids,
            &config,
            &mut inventory,
            BACKPACK,
            &details("pmcusec", true),
            &mut template_inventory,
        )
        .unwrap();

        // Weapon root in the backpack grid, magazine on the weapon, cartridges in the magazine,
        // one round in the chamber.
        let added: Vec<_> = inventory[4..]
            .iter()
            .map(|item| (item.template.as_str(), item.slot_id.as_deref().unwrap()))
            .collect();
        assert_eq!(
            added,
            vec![
                (RIFLE, "main"),
                (MAG, "mod_magazine"),
                (AMMO_PS, "cartridges"),
                (AMMO_PS, "patron_in_weapon"),
            ]
        );
        assert_eq!(inventory[4].parent_id.as_deref(), Some("backpack1"));
    }

    // -----------------------------------------------------------------------
    // spawn limits
    // -----------------------------------------------------------------------

    #[test]
    fn an_unknown_role_warns_twice_and_gets_empty_limits() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let config = fixture.config();

        let limits = get_item_spawn_limits_for_bot(&mut ctx, &config, "cursedassault").unwrap();

        assert!(limits.current_limits.is_empty());
        assert!(limits.global_limits.is_empty());
        // Two calls to `GetItemSpawnLimitsForBotType`, so two warnings.
        assert_eq!(
            ctx.diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.locale_key.as_deref()
                    == Some("bot-unable_to_find_spawn_limits_fallback_to_defaults"))
                .count(),
            2
        );
    }

    #[test]
    fn the_running_total_starts_zeroed_and_the_reference_copy_does_not() {
        let mut fixture = Fixture::new();
        fixture.item_spawn_limits = IndexMap::from([(
            "assault".to_owned(),
            IndexMap::from([("special1".to_owned(), 3.0)]),
        )]);
        let mut ctx = fixture.ctx();
        let config = fixture.config();

        let limits = get_item_spawn_limits_for_bot(&mut ctx, &config, "assault").unwrap();

        assert_eq!(limits.current_limits["special1"], 0.0);
        assert_eq!(limits.global_limits["special1"], 3.0);
    }

    #[test]
    fn spawn_limits_match_on_the_parent_when_the_tpl_is_absent() {
        let fixture = Fixture::new();
        let template = &fixture.items[ROUBLES];

        assert_eq!(
            get_matching_id_from_spawn_limits(
                template,
                ROUBLES,
                &IndexMap::from([(ROUBLES.to_owned(), 1.0)])
            ),
            Some(ROUBLES.to_owned())
        );
        assert_eq!(
            get_matching_id_from_spawn_limits(
                template,
                ROUBLES,
                &IndexMap::from([(MONEY.to_owned(), 1.0)])
            ),
            Some(MONEY.to_owned())
        );
        assert_eq!(
            get_matching_id_from_spawn_limits(template, ROUBLES, &IndexMap::new()),
            None
        );
    }

    #[test]
    fn a_pmc_role_reads_the_pmc_limits_and_a_missing_section_throws() {
        let mut fixture = Fixture::new();
        fixture.item_spawn_limits = IndexMap::from([(
            "pmc".to_owned(),
            IndexMap::from([("special1".to_owned(), 2.0)]),
        )]);
        let mut ctx = fixture.ctx();

        assert_eq!(
            get_item_spawn_limits_for_bot_type(&mut ctx, &fixture.config(), "pmcbear").unwrap()["special1"],
            2.0
        );

        let mut fixture = Fixture::new();
        fixture.item_spawn_limits = IndexMap::new();
        let mut ctx = fixture.ctx();
        assert_eq!(
            get_item_spawn_limits_for_bot_type(&mut ctx, &fixture.config(), "pmcbear")
                .unwrap_err()
                .message,
            "The given key 'pmc' was not present in the dictionary."
        );
    }

    // -----------------------------------------------------------------------
    // price limits / rouble totals
    // -----------------------------------------------------------------------

    #[test]
    fn single_item_price_limits_are_pmc_only_and_level_banded() {
        let fixture = Fixture::new();

        assert!(get_single_item_loot_price_limits(&fixture.pmc, 15, false).is_none());
        assert_eq!(
            get_single_item_loot_price_limits(&fixture.pmc, 15, true)
                .unwrap()
                .backpack
                .max,
            5000.0
        );
        // No band covers level 50.
        assert!(get_single_item_loot_price_limits(&fixture.pmc, 50, true).is_none());
    }

    #[test]
    fn an_uncovered_level_falls_back_to_a_rouble_total_of_one() {
        let fixture = Fixture::new();

        assert_eq!(
            get_rouble_value(&fixture.pmc.loot_settings.backpack, 15, Some("bigmap")),
            100_000.0
        );
        // No `totalRubByLevel` band -> 1, then multiplied by the default location multiplier.
        assert_eq!(
            get_rouble_value(&fixture.pmc.loot_settings.backpack, 500, Some("bigmap")),
            1.0
        );
    }

    #[test]
    fn available_containers_are_pockets_first_then_whatever_is_worn() {
        let bare: Vec<Item> = Vec::new();
        assert_eq!(
            get_available_containers_bot_can_store_items_in(&bare),
            vec![POCKETS]
        );

        let worn = vec![
            container("backpack1", BACKPACK_TPL, BACKPACK),
            container("vest1", VEST_TPL, TACTICAL_VEST),
        ];
        assert_eq!(
            get_available_containers_bot_can_store_items_in(&worn),
            vec![POCKETS, TACTICAL_VEST, BACKPACK]
        );
    }
}
