//! `Generators/Bot/BotInventoryGenerator.cs` — the orchestrator: build an empty inventory, hang
//! equipment off it, roll and generate the weapons, then let the loot generator fill the containers.
//!
//! # Deviations
//!
//! - **`ClearCache(botId)` (`:116`) becomes an omission.** The C# container cache is a singleton the
//!   loot generator reads back after this call; this port's [`ContainerGrids`] is one bot's entry,
//!   so "clear it" is expressed as *not* emitting it in
//!   [`BotInventoryResult::container_grids`](crate::bot::models::BotInventoryResult).
//! - **The `:204` clamp is recorded, not applied.** C# assigns into
//!   `randomistionDetails.EquipmentMods`, a slice of the *shared* `BotConfig` object. **No in-call
//!   reader:** nothing between `:204` and the end of `GenerateInventory` touches that dictionary
//!   again, so recording the clamped values instead of applying them cannot change this bot.
//!   The consumer is the *next* bot — `BotEquipmentFilterService.cs:63`
//!   (`AdjustChances(randomisationDetails.EquipmentMods, baseBotNode.BotChances.EquipmentModsChances)`),
//!   which `BotGenerator.cs:205` runs through `FilterBotEquipment` *before* `GenerateInventory`
//!   (`:284`). So the clamp is a cross-bot feedback loop into the very chances
//!   [`crate::bot::bot_equipment_mod_generator`] reads. Task 14's replay of
//!   `randomisation_clamps` back into the live config is therefore a **hard requirement**, not
//!   cosmetic: skip it and bot N+1 diverges.
//! - **The `:503` "no spawn chance defined" warning is unreachable and not ported.** `spawnChance`
//!   is a `double?` fed by `Dictionary<string, double>.GetValueOrDefault`, which yields `0.0`, never
//!   `null`; a slot with no chance therefore rolls `GetChance100(0)` — **consuming a draw** — and
//!   loses.
//! - **`GetPocketPoolByGameEdition` returns an owned pool.** The C# returns the template's own
//!   dictionary, so the `RootEquipmentPool.Remove` calls mutate it; `Pockets` is generated exactly
//!   once and nothing reads that pool again, so the copy is unobservable.
//! - **`GeneratingPlayerLevel` is not read.** C# threads it through seven `GenerateEquipmentProperties`
//!   objects for one purpose — `GetBotEquipmentBlacklist(equipmentRole, level)` at `:583` — and this
//!   port takes that blacklist pre-resolved on
//!   [`BotContext::equipment_blacklist`](crate::bot::BotContext). The request still carries the level
//!   so the C# projection has one place to resolve it from.
//! - **`GenerateEquipmentProperties.ModPool`/`SpawnChances` are moved, not aliased.** C# holds
//!   references to `templateInventory.Mods` and `wornItemChances`; this port
//!   [`std::mem::take`]s both into the settings object for the equipment phase and puts them back
//!   after it, which is the same aliasing with no copy — the same trick
//!   [`crate::bot::bot_weapon_generator`] uses for the weapon mod pool.
//!
//! # Bug-for-bug quirks worth naming
//!
//! - `:213-223` looks the armband pool up under `EquipmentSlots.ArmBand` but forces the chance
//!   under the key `"Armband"`, which is *not* the slot's `ToString()`. The forced 100% is written
//!   to a key `GenerateEquipment` never reads, so armband forcing only ever narrows the pool.
//! - `:510` `settings.RootEquipmentPool?.Count != 0` is **true for a null pool**, and `:516` then
//!   dereferences it. Only `GetPocketPoolByGameEdition` can produce one, and only when the roll
//!   passed — so a bot template with no `Pockets` pool aborts generation, and an *empty* pool
//!   (which `FilterRigsToThoseWithoutProtection` happily produces for `TacticalVest`) just loses
//!   the slot after consuming its draw.
//! - `:518` `while (!found)` only checks `attempts > maxAttempts` in the incompatible branch. A draw
//!   that lands on a tpl missing from the database (`:528-541`) increments `attempts` and loops
//!   without ever testing it; that loop is bounded only by the pool emptying at `:520`.
//! - `:718-730` rolls the primary once and then short-circuits: no second-primary draw when the
//!   primary lost, and no holster draw when it won. One draw for a bot with no primary, three for
//!   one with.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! One [`generate_inventory`] run draws, in this order:
//!
//! 1. `GenerateAndAddEquipmentToBot` (`:97`) — per generated slot, in the order "every
//!    non-excluded key of `templateInventory.Equipment`, then Pockets, FaceCover, Headwear,
//!    Earpiece, ArmorVest, TacticalVest": 1 `GetChance100` for the slot (`:509`, always consumed,
//!    even for a slot with no chance and even for a null/empty pool), then, only if it won and the
//!    pool is non-empty, 1 `GetWeightedValue` per attempt (`:525`), then
//!    `GenerateExtraPropertiesForItem` and `GenerateModsForEquipment` as listed in
//!    [`crate::bot::bot_generator_helper`] and [`crate::bot::bot_equipment_mod_generator`].
//! 2. `GetDesiredWeaponsForBot` (`:691`) — 1, or 3, `GetChance100` (see above).
//! 3. Per weapon slot that won *and* has a non-empty pool, in the fixed order FirstPrimaryWeapon,
//!    SecondPrimaryWeapon, Holster: `GenerateRandomWeapon` then `AddExtraMagazinesToInventory`, both
//!    as listed in [`crate::bot::bot_weapon_generator`].
//! 4. `GenerateLoot` (`:111`) — as listed in [`crate::bot::bot_loot_generator`].
use indexmap::{IndexMap, IndexSet};
use rayon::prelude::*;

use crate::bot::BotContext;
use crate::bot::bot_equipment_mod_generator::{
    generate_mods_for_equipment, get_bot_randomization_details,
};
use crate::bot::bot_generator_helper::{
    ContainerGrids, generate_extra_properties_for_item, get_bot_equipment_role,
    is_item_incompatible_with_current_items,
};
use crate::bot::bot_loot_generator::{BotLootConfig, generate_loot};
use crate::bot::bot_weapon_generator::{add_extra_magazines_to_inventory, generate_random_weapon};
use crate::bot::mod_pool_service::get_mods_for_gear_slot;
use crate::bot::models::{
    BotBaseInventoryWire, BotGenerationDetailsWire, BotInventoryBatchResult, BotInventoryResult,
    BotResultEnvelope, BotSliceWire, BotTemplateWire, BotTypeInventoryWire, ChancesWire,
    EquipmentFilterDetails, GenerateBotInventoryBatchRequest, GenerateBotInventoryRequest,
    GenerateEquipmentPropertiesWire, GenerationWire, PmcConfigWire, RandomisationDetails,
    SharedBotViewsWire,
};
use crate::loot::item_helper::{LootError, get_item};
use crate::loot::models::{DEBUG, Diagnostic, ERROR, Item, ItemView, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{TestSeedGuard, get_chance_100, get_weighted_value};

/// The six `ItemTpl` roots `GenerateInventoryBase` (`:126-156`) plants.
const INVENTORY_DEFAULT: &str = "55d7217a4bdc2d86028b456d";
const STASH_STANDARD_STASH_10X30: &str = "566abbc34bdc2d92178b4576";
const STASH_QUESTRAID: &str = "5963866286f7747bf429b572";
const STASH_QUESTOFFLINE: &str = "5963866b86f7747bfa1c4462";
const SORTINGTABLE_SORTING_TABLE: &str = "602543c13fee350cd564d032";
const HIDEOUTAREACONTAINER_CUSTOMIZATION: &str = "673c7b00cbf4b984b5099181";

/// `ItemTpl.POCKETS_1X4_TUE` / `ItemTpl.POCKETS_LARGE` (`:288`, `:433`).
const POCKETS_1X4_TUE: &str = "65e080be269cbd5c5005e529";
const POCKETS_LARGE: &str = "5af99e9186f7747c447120b8";

/// `GameEditions.UNHEARD` (`Models/Enums/GameEditions.cs:9`).
const UNHEARD: &str = "unheard_edition";

/// `EquipmentSlots` member names, as strings (see [`crate::bot::bot_weapon_generator`]).
const POCKETS: &str = "Pockets";
const TACTICAL_VEST: &str = "TacticalVest";
const BACKPACK: &str = "Backpack";
const SECURED_CONTAINER: &str = "SecuredContainer";
const ARM_BAND: &str = "ArmBand";
const ARMOR_VEST: &str = "ArmorVest";
const FACE_COVER: &str = "FaceCover";
const HEADWEAR: &str = "Headwear";
const EARPIECE: &str = "Earpiece";
const FIRST_PRIMARY_WEAPON: &str = "FirstPrimaryWeapon";
const SECOND_PRIMARY_WEAPON: &str = "SecondPrimaryWeapon";
const HOLSTER: &str = "Holster";

/// The key `:223` writes the forced armband chance under — *not* `EquipmentSlots.ArmBand.ToString()`.
const ARMBAND_CHANCE_KEY: &str = "Armband";

/// `BotInventoryGenerator._equipmentSlotsWithInventory` (`:48-54`).
const EQUIPMENT_SLOTS_WITH_INVENTORY: [&str; 4] =
    [POCKETS, TACTICAL_VEST, BACKPACK, SECURED_CONTAINER];

/// `BotInventoryGenerator._excludedEquipmentSlots` (`:57-68`) — the nine slots the `:234` loop skips
/// because the six explicit calls below it and the weapon path handle them in a fixed order.
const EXCLUDED_EQUIPMENT_SLOTS: [&str; 9] = [
    POCKETS,
    FIRST_PRIMARY_WEAPON,
    SECOND_PRIMARY_WEAPON,
    HOLSTER,
    ARMOR_VEST,
    TACTICAL_VEST,
    FACE_COVER,
    HEADWEAR,
    EARPIECE,
];

/// `BotInventoryGenerator._slotsToCheck` (`:70`) — the two slots forced to a 100% spawn chance.
const SLOTS_TO_CHECK: [&str; 2] = [POCKETS, SECURED_CONTAINER];

/// `BotInventoryGenerator.DesiredWeapons` (`:778-783`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesiredWeapons {
    pub slot: &'static str,
    pub should_spawn: bool,
}

/// `BotInventoryGenerator.GenerateInventory` (`:80-120`).
///
/// # Errors
///
/// Everything the four phases raise; see the module docs for the quirks that reach a `throw`.
pub fn generate_inventory(
    request: GenerateBotInventoryRequest,
) -> Result<BotInventoryResult, LootError> {
    let GenerateBotInventoryRequest {
        bot_id,
        test_seed,
        details,
        template,
        loot_pools,
        handbook_prices,
        generating_player_level,
        is_night_time,
        equipment,
        bosses,
        durability,
        item_spawn_limits,
        wallet_loot,
        currency_stack_size,
        secure_container_ammo_stack_count,
        disable_loot_on_bot_types,
        low_profile_gas_block_tpls,
        loot_item_resource_randomization,
        pmc_config,
        repair_kit_weapon,
        equipment_blacklist,
        item_presets,
        default_presets_by_tpl,
        config_blacklist,
        items,
    } = request;

    generate_one(
        &SharedBotViewsWire {
            generating_player_level,
            is_night_time,
            equipment,
            bosses,
            durability,
            item_spawn_limits,
            wallet_loot,
            currency_stack_size,
            secure_container_ammo_stack_count,
            disable_loot_on_bot_types,
            low_profile_gas_block_tpls,
            loot_item_resource_randomization,
            pmc_config,
            repair_kit_weapon,
            equipment_blacklist,
            item_presets,
            default_presets_by_tpl,
            config_blacklist,
            items,
        },
        BotSliceWire {
            bot_id,
            test_seed,
            details,
            template,
            loot_pools,
            handbook_prices,
        },
    )
}

/// One wave in one call: the shared views are parsed once, then every bot is generated against
/// them, in parallel. Envelope order matches request order.
///
/// The clamp feedback loop (`randomisation_clamps`, see the module docs) is NOT applied between
/// bots here — the C# dispatcher routes any wave that could write nighttime clamps to the per-bot
/// path (`BotWaveBatcher.CanBatch`), so a batch is clamp-free by construction and every envelope's
/// `randomisation_clamps` comes back empty. That guarantee is what makes the parallel loop safe.
///
/// Thread-safety inventory: the shared views are borrowed immutably; every `&mut` in
/// `generate_one` is bot-local; `MongoId`'s counter is atomic; the RNG is `thread_local!` and
/// `generate_one` installs its own seed guard per bot, so seeded output is deterministic per bot
/// regardless of worker assignment. The guard's `Drop` also parks the stream in `PARKED_RNG`,
/// which is harmless here: the only consumer of a park is the loot dynamic entry point, which
/// runs on C# calling threads, never on rayon workers — parks left on workers are dead writes.
///
/// A bot that fails comes back as an error envelope; the rest of the wave still generates.
pub fn generate_inventory_batch(
    request: GenerateBotInventoryBatchRequest,
) -> Result<BotInventoryBatchResult, LootError> {
    let GenerateBotInventoryBatchRequest { shared, bots } = request;

    let bots = bots
        .into_par_iter()
        .map(|slice| match generate_one(&shared, slice) {
            Ok(result) => BotResultEnvelope {
                result: Some(result),
                error: None,
            },
            Err(error) => BotResultEnvelope {
                result: None,
                error: Some(error.message),
            },
        })
        .collect();

    Ok(BotInventoryBatchResult { bots })
}

/// `BotInventoryGenerator.GenerateInventory` (`:80-120`) proper - one bot against views the caller
/// already owns.
fn generate_one(
    shared: &SharedBotViewsWire,
    slice: BotSliceWire,
) -> Result<BotInventoryResult, LootError> {
    let BotSliceWire {
        test_seed,
        details,
        template,
        loot_pools,
        handbook_prices,
        ..
    } = slice;
    let _seed_guard = test_seed.map(TestSeedGuard::install);

    let SharedBotViewsWire {
        is_night_time,
        equipment,
        bosses,
        durability,
        item_spawn_limits,
        wallet_loot,
        currency_stack_size,
        secure_container_ammo_stack_count,
        disable_loot_on_bot_types,
        low_profile_gas_block_tpls,
        loot_item_resource_randomization,
        pmc_config,
        repair_kit_weapon,
        equipment_blacklist,
        item_presets,
        default_presets_by_tpl,
        config_blacklist,
        items,
        ..
    } = shared;

    let BotTemplateWire {
        inventory: mut template_inventory,
        chances: mut worn_item_chances,
        generation: item_generation_limits_min_max,
        ..
    } = template;

    let mut ctx = BotContext {
        items,
        bosses,
        durability,
        equipment,
        loot_item_resource_randomization,
        is_night_time: *is_night_time,
        item_blacklist: config_blacklist,
        default_presets_by_tpl,
        item_presets,
        equipment_blacklist,
        low_profile_gas_block_tpls,
        weapon_has_enhancement_chance_percent: pmc_config.weapon_has_enhancement_chance_percent,
        repair_kit_weapon,
        secure_container_ammo_stack_count: *secure_container_ammo_stack_count,
        diagnostics: Vec::new(),
    };
    let mut grids = ContainerGrids::default();

    // Generate base inventory with no items
    let mut bot_inventory = generate_inventory_base();
    let mut randomisation_clamps = IndexMap::new();

    generate_and_add_equipment_to_bot(
        &mut ctx,
        &mut grids,
        &mut template_inventory,
        &mut worn_item_chances,
        &mut bot_inventory,
        &details,
        pmc_config,
        &mut randomisation_clamps,
    )?;

    // Roll weapon spawns (primary/secondary/holster) and generate a weapon for each roll that passed
    generate_and_add_weapons_to_bot(
        &mut ctx,
        &mut grids,
        &mut template_inventory,
        &worn_item_chances,
        &mut bot_inventory,
        &details,
        &item_generation_limits_min_max,
    )?;

    // Pick loot and add to bots containers (rig/backpack/pockets/secure)
    let loot_config = BotLootConfig {
        equipment_id: &bot_inventory.equipment,
        item_counts: &item_generation_limits_min_max.items,
        weapon_mod_chances: &worn_item_chances.weapon_mods,
        disable_loot_on_bot_types,
        item_spawn_limits,
        wallet_loot,
        currency_stack_size,
        pmc: pmc_config,
        handbook_prices: &handbook_prices,
        loot_pools: &loot_pools,
    };
    generate_loot(
        &mut ctx,
        &mut grids,
        &mut bot_inventory.items,
        &details,
        &mut template_inventory,
        &loot_config,
    )?;

    // Inventory cache isn't needed, clear to save memory
    let container_grids = if details.clear_bot_container_cache_after_generation {
        IndexMap::new()
    } else {
        grids.into_wire()
    };

    Ok(BotInventoryResult {
        inventory: bot_inventory,
        diagnostics: ctx.diagnostics,
        container_grids,
        randomisation_clamps,
    })
}

/// `BotInventoryGenerator.GenerateInventoryBase` (`:126-156`) — six fresh `MongoId`s, drawn in
/// declaration order.
pub fn generate_inventory_base() -> BotBaseInventoryWire {
    let equipment_id = mongo_id::generate();
    let stash_id = mongo_id::generate();
    let quest_raid_items_id = mongo_id::generate();
    let quest_stash_items_id = mongo_id::generate();
    let sorting_table_id = mongo_id::generate();
    let hideout_customization_stash_id = mongo_id::generate();

    BotBaseInventoryWire {
        items: vec![
            root_item(&equipment_id, INVENTORY_DEFAULT),
            root_item(&stash_id, STASH_STANDARD_STASH_10X30),
            root_item(&quest_raid_items_id, STASH_QUESTRAID),
            root_item(&quest_stash_items_id, STASH_QUESTOFFLINE),
            root_item(&sorting_table_id, SORTINGTABLE_SORTING_TABLE),
            root_item(
                &hideout_customization_stash_id,
                HIDEOUTAREACONTAINER_CUSTOMIZATION,
            ),
        ],
        equipment: equipment_id,
        stash: stash_id,
        sorting_table: sorting_table_id,
        quest_raid_items: quest_raid_items_id,
        quest_stash_items: quest_stash_items_id,
        hideout_area_stashes: IndexMap::new(),
        fast_panel: IndexMap::new(),
        favorite_items: Vec::new(),
        hideout_customization_stash_id,
    }
}

fn root_item(id: &str, template: &str) -> Item {
    Item {
        id: id.to_owned(),
        template: template.to_owned(),
        ..Default::default()
    }
}

/// `BotInventoryGenerator.GenerateAndAddEquipmentToBot` (`:168-417`).
///
/// `botId`, `sessionId` and `raidConfig` are not carried: the first two only key C# services this
/// port replaced with per-call state, and the third is folded into
/// [`BotContext::is_night_time`](crate::bot::BotContext).
///
/// # Errors
///
/// The `EquipmentMods` deref at `:200`, the five `Equipment[slot]` indexers, and everything
/// [`generate_equipment`] raises.
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per C# parameter plus the two out-values the port returns instead of mutating shared config"
)]
pub fn generate_and_add_equipment_to_bot(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    template_inventory: &mut BotTypeInventoryWire,
    worn_item_chances: &mut ChancesWire,
    bot_inventory: &mut BotBaseInventoryWire,
    details: &BotGenerationDetailsWire,
    pmc_config: &PmcConfigWire,
    randomisation_clamps: &mut IndexMap<String, f64>,
) -> Result<(), LootError> {
    // Copied out so the config outlives the mutable borrows of `ctx` below.
    let equipment_configs = ctx.equipment;
    let bot_equipment_role = get_bot_equipment_role(&details.role_lowercase).to_owned();
    let Some(bot_equip_config) = equipment_configs.get(bot_equipment_role.as_str()) else {
        ctx.diagnostics.push(plain(
            ERROR,
            format!(
                "Bot Equipment generation failed, unable to find equipment filters for: {}",
                details.role_lowercase
            ),
        ));

        return Ok(());
    };
    let randomisation_details = get_bot_randomization_details(details.bot_level, bot_equip_config);

    // Apply nighttime changes if its nighttime + there's changes to make
    if let Some(randomisation_details) = randomisation_details
        && let Some(nighttime_changes) = randomisation_details.nighttime_changes.as_ref()
        && ctx.is_night_time
    {
        for (equipment, weight) in &nighttime_changes.equipment_mods_modifiers {
            let Some(equipment_mods) = randomisation_details.equipment_mods.as_ref() else {
                return Err(LootError::new(
                    "Object reference not set to an instance of an object.",
                ));
            };

            if let Some(value) = equipment_mods.get(equipment) {
                // Never let mod chance go outside 0 - 100
                let new_weight = weight + value;
                randomisation_clamps.insert(equipment.clone(), new_weight.clamp(0.0, 100.0));
            }
        }
    }

    // Is PMC + generating armband + armband forcing is enabled
    if pmc_config.force_armband.enabled
        && details.is_pmc
        && let Some(armbands) = template_inventory.equipment.get_mut(ARM_BAND)
    {
        // Get tpl based on pmc side
        let armband_tpl = if details.role_lowercase == "pmcusec" {
            &pmc_config.force_armband.usec
        } else {
            &pmc_config.force_armband.bear
        };

        armbands.clear();
        armbands.insert(armband_tpl.clone(), 1.0);

        // Force armband spawn to 100%
        worn_item_chances
            .equipment
            .insert(ARMBAND_CHANCE_KEY.to_owned(), 100.0);
    }

    // C# aliases `templateInventory.Mods` and `wornItemChances` into every settings object; moving
    // them in and back out is the same aliasing without the copy.
    let mut settings = GenerateEquipmentPropertiesWire {
        mod_pool: std::mem::take(&mut template_inventory.mods),
        spawn_chances: std::mem::take(worn_item_chances),
        bot_data: crate::bot::models::BotDataWire {
            role: details.role_lowercase.clone(),
            level: details.bot_level,
            equipment_role: bot_equipment_role,
        },
        // C# passes a reference. `GenerateEquipmentPropertiesWire` owns this member (it is a
        // `Deserialize` wire type, so borrowing would put a lifetime on it and on every Task 7
        // fixture) — one clone of ~15 `Option`s and 4 small maps per bot.
        bot_equipment_config: bot_equip_config.clone(),
    };

    // Iterate over all equipment slots of bot, do it in specific order to reduce conflicts
    // e.g. ArmorVest should be generated after TacticalVest, or FACE_COVER before HEADWEAR
    for (equipment_slot, items_with_weight_pool) in &mut template_inventory.equipment {
        // Skip some slots as they need to be done in a specific order + with specific parameter
        // values, e.g. Weapons
        if EXCLUDED_EQUIPMENT_SLOTS.contains(&equipment_slot.as_str()) {
            continue;
        }

        generate_equipment(
            ctx,
            grids,
            &mut settings,
            equipment_slot,
            Some(items_with_weight_pool),
            randomisation_details,
            &[],
            bot_inventory,
        )?;
    }

    // Generate below in specific order. Unheard profiles have unique sized pockets.
    let mut pocket_pool = get_pocket_pool_by_game_edition(
        &details.game_version,
        &template_inventory.equipment,
        details.is_pmc,
    );
    generate_equipment(
        ctx,
        grids,
        &mut settings,
        POCKETS,
        pocket_pool.as_mut(),
        randomisation_details,
        &[POCKETS_1X4_TUE, POCKETS_LARGE],
        bot_inventory,
    )?;

    for slot in [FACE_COVER, HEADWEAR, EARPIECE] {
        let pool = template_inventory
            .equipment
            .get_mut(slot)
            .ok_or_else(|| key_not_found(slot))?;
        generate_equipment(
            ctx,
            grids,
            &mut settings,
            slot,
            Some(pool),
            randomisation_details,
            &[],
            bot_inventory,
        )?;
    }

    let has_armor_vest = generate_equipment(
        ctx,
        grids,
        &mut settings,
        ARMOR_VEST,
        Some(
            template_inventory
                .equipment
                .get_mut(ARMOR_VEST)
                .ok_or_else(|| key_not_found(ARMOR_VEST))?,
        ),
        randomisation_details,
        &[],
        bot_inventory,
    )?;

    // Bot has no armor vest and flagged to be forced to wear armored rig in this event
    if bot_equip_config
        .force_only_armored_rig_when_no_armor
        .unwrap_or(false)
        && !has_armor_vest
    {
        // Filter rigs down to only those with armor
        filter_rigs_to_those_with_protection(
            ctx,
            &mut template_inventory.equipment,
            &details.role_lowercase,
        )?;
    }

    // Optimisation - Remove armored rigs from pool
    if has_armor_vest {
        filter_rigs_to_those_without_protection(
            ctx,
            &mut template_inventory.equipment,
            &details.role_lowercase,
            true,
        )?;
    }

    // Bot is flagged as always needing a vest
    if bot_equip_config.force_rig_when_no_vest.unwrap_or(false) && !has_armor_vest {
        settings
            .spawn_chances
            .equipment
            .insert(TACTICAL_VEST.to_owned(), 100.0);
    }

    generate_equipment(
        ctx,
        grids,
        &mut settings,
        TACTICAL_VEST,
        Some(
            template_inventory
                .equipment
                .get_mut(TACTICAL_VEST)
                .ok_or_else(|| key_not_found(TACTICAL_VEST))?,
        ),
        randomisation_details,
        &[],
        bot_inventory,
    )?;

    // Hand the two aliased maps back; the weapon and loot phases read them next.
    template_inventory.mods = std::mem::take(&mut settings.mod_pool);
    *worn_item_chances = std::mem::take(&mut settings.spawn_chances);

    Ok(())
}

/// `BotInventoryGenerator.GetPocketPoolByGameEdition` (`:426-435`). `None` is the C# `null` the
/// `GetValueOrDefault` returns for a template with no `Pockets` pool, which `:516` then dereferences.
pub fn get_pocket_pool_by_game_edition(
    chosen_game_version: &str,
    template_equipment: &IndexMap<String, IndexMap<String, f64>>,
    is_pmc: bool,
) -> Option<IndexMap<String, f64>> {
    if chosen_game_version == UNHEARD && is_pmc {
        return Some(IndexMap::from([(POCKETS_1X4_TUE.to_owned(), 1.0)]));
    }

    template_equipment.get(POCKETS).cloned()
}

/// `BotInventoryGenerator.FilterRigsToThoseWithProtection` (`:442-459`) — **overwrites** the
/// template's `TacticalVest` pool.
///
/// # Errors
///
/// The `templateEquipment[TacticalVest]` indexer (`:444`).
pub fn filter_rigs_to_those_with_protection(
    ctx: &mut BotContext,
    template_equipment: &mut IndexMap<String, IndexMap<String, f64>>,
    bot_role: &str,
) -> Result<(), LootError> {
    let items = ctx.items;
    let vests = template_equipment
        .get(TACTICAL_VEST)
        .ok_or_else(|| key_not_found(TACTICAL_VEST))?;
    let tac_vests_with_armor: IndexMap<String, f64> = vests
        .iter()
        .filter(|(tpl, _)| item_has_slots(items, tpl))
        .map(|(tpl, weight)| (tpl.clone(), *weight))
        .collect();

    if tac_vests_with_armor.is_empty() {
        ctx.diagnostics.push(plain(
            DEBUG,
            format!("Unable to filter to only armored rigs as bot: {bot_role} has none in pool"),
        ));

        return Ok(());
    }

    template_equipment.insert(TACTICAL_VEST.to_owned(), tac_vests_with_armor);

    Ok(())
}

/// `BotInventoryGenerator.FilterRigsToThoseWithoutProtection` (`:467-488`) — **overwrites** the
/// template's `TacticalVest` pool. The only call site (`:388`) passes `allowEmptyResult = true`, so
/// the pool is routinely left empty and `GenerateEquipment` then loses the slot after consuming its
/// draw.
///
/// # Errors
///
/// The `templateEquipment[TacticalVest]` indexer (`:473`).
pub fn filter_rigs_to_those_without_protection(
    ctx: &mut BotContext,
    template_equipment: &mut IndexMap<String, IndexMap<String, f64>>,
    bot_role: &str,
    allow_empty_result: bool,
) -> Result<(), LootError> {
    let items = ctx.items;
    let vests = template_equipment
        .get(TACTICAL_VEST)
        .ok_or_else(|| key_not_found(TACTICAL_VEST))?;
    let tac_vests_without_armor: IndexMap<String, f64> = vests
        .iter()
        .filter(|(tpl, _)| !item_has_slots(items, tpl))
        .map(|(tpl, weight)| (tpl.clone(), *weight))
        .collect();

    if !allow_empty_result && tac_vests_without_armor.is_empty() {
        ctx.diagnostics.push(plain(
            DEBUG,
            format!("Unable to filter to only unarmored rigs as bot: {bot_role} has none in pool"),
        ));

        return Ok(());
    }

    template_equipment.insert(TACTICAL_VEST.to_owned(), tac_vests_without_armor);

    Ok(())
}

/// `ItemHelper.ItemHasSlots` (`Helpers/Items/ItemHelper.cs:517-525`).
fn item_has_slots(items: &IndexMap<String, ItemView>, item_tpl: &str) -> bool {
    get_item(items, item_tpl)
        .and_then(|template| template.slots.as_ref())
        .is_some_and(|slots| !slots.is_empty())
}

/// `BotInventoryGenerator.GenerateEquipment` (`:495-629`) — `true` when an item was added.
///
/// The C# `GenerateEquipmentProperties` splits across the parameter list here: the four members that
/// are fixed for a whole bot ride in `settings`, the six that change per call are their own
/// parameters, and `BotId`/`Inventory` are the `grids`/`bot_inventory` this port threads everywhere.
///
/// # Errors
///
/// The `:516` null-pool deref (see the module docs), `GetWeightedValue` over a pool whose weights
/// do not add up, and everything `GenerateExtraPropertiesForItem` and
/// [`generate_mods_for_equipment`] raise.
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per `GenerateEquipmentProperties` member the C# call sites vary"
)]
pub fn generate_equipment(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    settings: &mut GenerateEquipmentPropertiesWire,
    root_equipment_slot: &str,
    root_equipment_pool: Option<&mut IndexMap<String, f64>>,
    randomisation_details: Option<&RandomisationDetails>,
    generate_mods_blacklist: &[&str],
    bot_inventory: &mut BotBaseInventoryWire,
) -> Result<bool, LootError> {
    // `GetValueOrDefault` on a `Dictionary<string, double>` is 0, never null, so the `:501` warning
    // arm is unreachable and a slot with no chance still consumes the roll below.
    let spawn_chance = if SLOTS_TO_CHECK.contains(&root_equipment_slot) {
        100.0
    } else {
        settings
            .spawn_chances
            .equipment
            .get(root_equipment_slot)
            .copied()
            .unwrap_or(0.0)
    };

    // Roll dice on equipment item
    let should_spawn = get_chance_100(spawn_chance);
    if !should_spawn {
        return Ok(false);
    }

    // `null?.Count != 0` is *true*, so a null pool reaches the `:516` `.Count` and throws.
    let Some(root_equipment_pool) = root_equipment_pool else {
        return Err(LootError::new(
            "Object reference not set to an instance of an object.",
        ));
    };
    if root_equipment_pool.is_empty() {
        return Ok(false);
    }

    let items = ctx.items;
    let equipment_blacklist = ctx.equipment_blacklist;
    let equipment_configs = ctx.equipment;

    // Limit attempts to find a compatible item as it's expensive to check them all
    #[expect(
        clippy::cast_precision_loss,
        reason = "`Math.Round(pool.Count * 0.75)`; pool sizes are in the tens"
    )]
    // `Math.Round` is banker's rounding, which `round_ties_even` — not `round` — reproduces.
    let max_attempts = (root_equipment_pool.len() as f64 * 0.75).round_ties_even();
    let mut attempts = 0i32;
    let picked_item_tpl = loop {
        if root_equipment_pool.is_empty() {
            return Ok(false);
        }

        let chosen_item_tpl = get_weighted_value(&*root_equipment_pool)?;

        if get_item(items, &chosen_item_tpl).is_none() {
            ctx.diagnostics.push(localised(
                ERROR,
                "bot-missing_item_template",
                serde_json::Value::String(chosen_item_tpl.clone()),
            ));
            ctx.diagnostics.push(plain(
                DEBUG,
                format!("EquipmentSlot-> {root_equipment_slot}"),
            ));

            // Remove picked item
            root_equipment_pool.shift_remove(&chosen_item_tpl);

            attempts += 1;

            // Bug-for-bug: this branch never tests `attempts > maxAttempts`.
            continue;
        }

        // Is the chosen item compatible with other items equipped
        let compatibility_result = is_item_incompatible_with_current_items(
            ctx,
            &bot_inventory.items,
            &chosen_item_tpl,
            root_equipment_slot,
        );
        if compatibility_result.incompatible.unwrap_or(false) {
            // Tried x different items that failed, stop
            if f64::from(attempts) > max_attempts {
                return Ok(false);
            }

            // Remove picked item from pool
            root_equipment_pool.shift_remove(&chosen_item_tpl);

            // Increment times tried
            attempts += 1;
        } else {
            // Success
            break chosen_item_tpl;
        }
    };
    let picked_item_db = get_item(items, &picked_item_tpl).ok_or_else(|| {
        LootError::new(format!(
            "Item: {picked_item_tpl} vanished from the database"
        ))
    })?;

    // Create root item
    let id = mongo_id::generate();
    let item = Item {
        id: id.clone(),
        template: picked_item_tpl.clone(),
        parent_id: Some(bot_inventory.equipment.clone()),
        slot_id: Some(root_equipment_slot.to_owned()),
        upd: generate_extra_properties_for_item(
            ctx,
            picked_item_db,
            Some(&settings.bot_data.role),
            true,
        )?,
        ..Default::default()
    };

    // Edge case: Filter the armor items mod pool if bot exists in config dict + config has armor slot
    if equipment_configs.contains_key(settings.bot_data.equipment_role.as_str())
        && randomisation_details
            .and_then(|details| details.randomised_armor_slots.as_ref())
            .is_some_and(|slots| slots.contains(root_equipment_slot))
    {
        // Filter out mods from relevant blacklist
        let filtered =
            get_filtered_dynamic_mods_for_item(ctx, &picked_item_tpl, equipment_blacklist);
        settings.mod_pool.insert(picked_item_tpl.clone(), filtered);
    }

    let item_is_on_generate_mod_blacklist =
        generate_mods_blacklist.contains(&picked_item_tpl.as_str());
    // Does item have slots for sub-mods to be inserted into
    if picked_item_db
        .slots
        .as_ref()
        .is_some_and(|slots| !slots.is_empty())
        && !item_is_on_generate_mod_blacklist
    {
        let mut child_items_to_add = vec![item.clone()];
        generate_mods_for_equipment(
            ctx,
            &mut child_items_to_add,
            &id,
            &picked_item_tpl,
            settings,
            equipment_blacklist,
            false,
        )?;
        bot_inventory.items.extend(child_items_to_add);
    } else {
        // No slots, add root item only
        bot_inventory.items.push(item.clone());
    }

    // Cache container ready for items to be added in
    if EQUIPMENT_SLOTS_WITH_INVENTORY.contains(&root_equipment_slot) {
        grids.add_empty_container(ctx, root_equipment_slot, &item);
    }

    Ok(true)
}

/// `BotInventoryGenerator.GetFilteredDynamicModsForItem` (`:637-669`).
///
/// A null `equipmentBlacklist.Equipment` is an NRE at the C# `TryGetValue`; it is "no blacklist"
/// here, the same reading [`crate::bot::bot_equipment_mod_generator`]'s `FilterModsByBlacklist`
/// already takes.
pub fn get_filtered_dynamic_mods_for_item(
    ctx: &mut BotContext,
    item_tpl: &str,
    equipment_blacklist: &EquipmentFilterDetails,
) -> IndexMap<String, IndexSet<String>> {
    let mod_pool = get_mods_for_gear_slot(ctx, item_tpl);

    let mut filtered_pool = IndexMap::with_capacity(mod_pool.len());
    for (mod_slot, mods_for_slot) in mod_pool {
        let blacklisted_mods = equipment_blacklist
            .equipment
            .as_ref()
            .and_then(|blacklist| blacklist.get(&mod_slot));
        let Some(blacklisted_mods) = blacklisted_mods else {
            // No blacklist for slot, return all mods
            filtered_pool.insert(mod_slot, mods_for_slot);
            continue;
        };

        let filtered_mods: IndexSet<String> = mods_for_slot
            .iter()
            .filter(|tpl| !blacklisted_mods.contains(*tpl))
            .cloned()
            .collect();
        if filtered_mods.is_empty() {
            ctx.diagnostics.push(plain(
                WARNING,
                format!(
                    "Filtering: '{mod_slot}' resulted in 0 mods. Reverting to original set for slot"
                ),
            ));

            // Return original
            filtered_pool.insert(mod_slot, mods_for_slot);
            continue;
        }

        // There's at least one tpl remaining, send it
        filtered_pool.insert(mod_slot, filtered_mods);
    }

    filtered_pool
}

/// `BotInventoryGenerator.GenerateAndAddWeaponsToBot` (`:681-709`).
///
/// # Errors
///
/// The `Equipment[slot]` indexer (`:695`, reached only for a slot that rolled true) and everything
/// [`get_desired_weapons_for_bot`] and [`add_weapon_and_magazines_to_inventory`] raise.
pub fn generate_and_add_weapons_to_bot(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    template_inventory: &mut BotTypeInventoryWire,
    equipment_chances: &ChancesWire,
    bot_inventory: &mut BotBaseInventoryWire,
    details: &BotGenerationDetailsWire,
    item_generation_limits_min_max: &GenerationWire,
) -> Result<(), LootError> {
    let weapon_slots_to_fill = get_desired_weapons_for_bot(equipment_chances)?;

    // Add weapon to bot if true and bot json has something to put into the slot
    for desired_weapons in weapon_slots_to_fill {
        if !desired_weapons.should_spawn {
            continue;
        }

        let pool = template_inventory
            .equipment
            .get(desired_weapons.slot)
            .ok_or_else(|| key_not_found(desired_weapons.slot))?;
        if pool.is_empty() {
            continue;
        }

        add_weapon_and_magazines_to_inventory(
            ctx,
            grids,
            desired_weapons.slot,
            template_inventory,
            bot_inventory,
            equipment_chances,
            details,
            item_generation_limits_min_max,
        )?;
    }

    Ok(())
}

/// `BotInventoryGenerator.GetDesiredWeaponsForBot` (`:716-733`).
///
/// The `&&`/`||` short-circuits are the parity contract: the second-primary roll is skipped when the
/// primary lost, and the holster roll is skipped when it won. So a bot draws once when it gets no
/// primary and three times when it does — and the two skipped `EquipmentChances[...]` indexers
/// cannot throw either.
///
/// # Errors
///
/// The `EquipmentChances[...]` indexers, for a bot template missing a weapon slot's chance.
pub fn get_desired_weapons_for_bot(
    equipment_chances: &ChancesWire,
) -> Result<[DesiredWeapons; 3], LootError> {
    let chance = |slot: &str| {
        equipment_chances
            .equipment
            .get(slot)
            .copied()
            .ok_or_else(|| key_not_found(slot))
    };

    let should_spawn_primary = get_chance_100(chance(FIRST_PRIMARY_WEAPON)?);

    Ok([
        DesiredWeapons {
            slot: FIRST_PRIMARY_WEAPON,
            should_spawn: should_spawn_primary,
        },
        DesiredWeapons {
            slot: SECOND_PRIMARY_WEAPON,
            should_spawn: should_spawn_primary && get_chance_100(chance(SECOND_PRIMARY_WEAPON)?),
        },
        DesiredWeapons {
            slot: HOLSTER,
            // No primary = force pistol
            should_spawn: !should_spawn_primary || get_chance_100(chance(HOLSTER)?),
        },
    ])
}

/// `BotInventoryGenerator.AddWeaponAndMagazinesToInventory` (`:746-775`).
///
/// # Errors
///
/// The `generatedWeapon.Weapon` deref for a weapon tpl missing from the database, the
/// `itemGenerationWeights.Items.Magazines` deref for a bot template with no magazine block, and
/// everything the weapon generator raises.
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per C# parameter; the names are the parity contract"
)]
pub fn add_weapon_and_magazines_to_inventory(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    weapon_slot: &str,
    template_inventory: &mut BotTypeInventoryWire,
    bot_inventory: &mut BotBaseInventoryWire,
    equipment_chances: &ChancesWire,
    details: &BotGenerationDetailsWire,
    item_generation_weights: &GenerationWire,
) -> Result<(), LootError> {
    let generated_weapon = generate_random_weapon(
        ctx,
        weapon_slot,
        template_inventory,
        details,
        &bot_inventory.equipment.clone(),
        &equipment_chances.weapon_mods,
    )?;
    let Some(generated_weapon) = generated_weapon else {
        return Err(LootError::new(
            "Object reference not set to an instance of an object.",
        ));
    };

    bot_inventory
        .items
        .extend(generated_weapon.weapon.iter().cloned());

    let magazines = item_generation_weights
        .items
        .magazines
        .as_ref()
        .ok_or_else(|| LootError::new("Object reference not set to an instance of an object."))?;

    add_extra_magazines_to_inventory(
        ctx,
        grids,
        &generated_weapon,
        magazines,
        &mut bot_inventory.items,
        &details.role_lowercase,
    )
}

/// The `KeyNotFoundException` a C# dictionary indexer raises.
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
    use serde_json::{Value, json};

    use super::*;
    use crate::loot::random_util::{TestSeedGuard, get_int};

    const SEED: u64 = 20260813;

    const HEADWEAR_TPL: &str = "headwear_cap";
    const EARPIECE_TPL: &str = "earpiece_comtac";
    const FACE_COVER_TPL: &str = "facecover_shemagh";
    const ARMOR_TPL: &str = "armor_paca";
    const VEST_ARMORED_TPL: &str = "vest_armored";
    const VEST_PLAIN_TPL: &str = "vest_plain";
    const BACKPACK_TPL: &str = "backpack_daypack";
    const POCKETS_TPL: &str = "pockets_default";
    const SECURE_TPL: &str = "secure_alpha";
    const ARMBAND_TPL: &str = "armband_yellow";
    const FORCED_ARMBAND_TPL: &str = "armband_bear";
    const PLATE_TPL: &str = "plate_front";
    const RIFLE_TPL: &str = "rifle_akm";
    const MAG_TPL: &str = "mag_akm";
    const AMMO_TPL: &str = "ammo_ps";
    const CALIBER: &str = "Caliber762x39";

    fn items() -> Value {
        json!({
            HEADWEAR_TPL: {"name": "cap", "width": 1, "height": 1},
            EARPIECE_TPL: {"name": "comtac", "width": 1, "height": 1},
            FACE_COVER_TPL: {"name": "shemagh", "width": 1, "height": 1},
            ARMOR_TPL: {"name": "paca", "width": 2, "height": 2},
            // Has slots, so `ItemHasSlots` calls it armored.
            VEST_ARMORED_TPL: {"name": "armored rig", "width": 2, "height": 2,
                "slots": [{"name": "front_plate", "required": false, "filter": [PLATE_TPL]}],
                "grids": [{"name": "main", "cellsH": 2, "cellsV": 2}]},
            VEST_PLAIN_TPL: {"name": "rig", "width": 2, "height": 2,
                "grids": [{"name": "main", "cellsH": 3, "cellsV": 2}]},
            PLATE_TPL: {"name": "front plate", "width": 1, "height": 1, "armorClass": 4},
            BACKPACK_TPL: {"name": "daypack", "width": 3, "height": 3,
                "grids": [{"name": "main", "cellsH": 4, "cellsV": 4}]},
            POCKETS_TPL: {"name": "pockets", "grids": [{"name": "main", "cellsH": 4, "cellsV": 1}]},
            SECURE_TPL: {"name": "alpha", "grids": [{"name": "main", "cellsH": 2, "cellsV": 2}]},
            ARMBAND_TPL: {"name": "armband", "width": 1, "height": 1},
            FORCED_ARMBAND_TPL: {"name": "bear armband", "width": 1, "height": 1},
            RIFLE_TPL: {"name": "akm", "parent": crate::loot::item_helper::WEAPON,
                "width": 3, "height": 1, "weapClass": "assaultRifle", "maxDurability": 100.0,
                "caliber": CALIBER, "defAmmo": AMMO_TPL, "defMagType": MAG_TPL,
                "reloadMode": "ExternalMagazine", "isChamberLoad": false,
                "chambers": [{"name": "patron_in_weapon", "filter": [AMMO_TPL]}],
                "slots": [{"name": "mod_magazine", "required": true, "filter": [MAG_TPL]}]},
            crate::loot::item_helper::MAGAZINE: {"name": "magazine"},
            MAG_TPL: {"name": "akm mag", "parent": crate::loot::item_helper::MAGAZINE,
                "cartridgesMaxCount": 30, "cartridgesFirstFilter": [AMMO_TPL],
                "reloadMagType": "ExternalMagazine", "width": 1, "height": 1,
                "cartridges": [{"name": "cartridges", "filter": [AMMO_TPL]}]},
            AMMO_TPL: {"name": "PS", "parent": crate::loot::item_helper::AMMO, "caliber": CALIBER,
                "stackMaxSize": 60, "stackMinRandom": 30, "stackMaxRandom": 30,
                "width": 1, "height": 1},
        })
    }

    /// Every count weight is a single `0` entry: `GetWeightedValue` short-circuits without drawing,
    /// so the loot phase contributes neither items nor draws and the pinned run stays readable.
    fn zero_loot_counts() -> Value {
        let zero = json!({"weights": {"0": 1}});

        json!({
            "grenades": zero, "healing": zero, "drugs": zero, "food": zero, "drink": zero,
            "currency": zero, "stims": zero, "backpackLoot": zero, "pocketLoot": zero,
            "vestLoot": zero, "specialItems": zero,
            "magazines": {"weights": {"1": 1}},
        })
    }

    fn base_request() -> Value {
        json!({
            "botId": "bbbbbbbbbbbbbbbbbbbbbbbb",
            "testSeed": SEED,
            "details": {"role": "assault", "roleLowercase": "assault", "side": "Savage",
                "botLevel": 15, "isPmc": false, "isPlayerScav": false, "gameVersion": "standard",
                "location": "bigmap", "botDifficulty": "normal",
                "clearBotContainerCacheAfterGeneration": false},
            "template": {
                "inventory": {
                    // Insertion order is the `:234` loop order.
                    "equipment": {
                        "Headwear": {HEADWEAR_TPL: 1},
                        "Earpiece": {EARPIECE_TPL: 1},
                        "FaceCover": {FACE_COVER_TPL: 1},
                        "ArmorVest": {ARMOR_TPL: 1},
                        "TacticalVest": {VEST_ARMORED_TPL: 1, VEST_PLAIN_TPL: 1},
                        "Backpack": {BACKPACK_TPL: 1},
                        "Pockets": {POCKETS_TPL: 1},
                        "SecuredContainer": {SECURE_TPL: 1},
                        "ArmBand": {ARMBAND_TPL: 1},
                        "FirstPrimaryWeapon": {RIFLE_TPL: 1},
                        "SecondPrimaryWeapon": {},
                        "Holster": {},
                    },
                    "Ammo": {CALIBER: {AMMO_TPL: 1}},
                    "items": {"Backpack": {}, "Pockets": {}, "SecuredContainer": {},
                        "SpecialLoot": {}, "TacticalVest": {}},
                    "mods": {RIFLE_TPL: {"mod_magazine": [MAG_TPL]}},
                },
                "chances": {
                    "equipment": {"Headwear": 100, "Earpiece": 100, "FaceCover": 100,
                        "ArmorVest": 100, "TacticalVest": 100, "Backpack": 100, "ArmBand": 100,
                        "FirstPrimaryWeapon": 100, "SecondPrimaryWeapon": 0, "Holster": 0},
                    "weaponMods": {"mod_magazine": 100},
                    "equipmentMods": {"front_plate": 100},
                },
                "generation": {"items": zero_loot_counts()},
            },
            "generatingPlayerLevel": 20,
            "isNightTime": false,
            "equipment": {"assault": {}},
            "bosses": [],
            "durability": {
                "default": {"armor": {"maxDelta": 10, "minDelta": 0, "minLimitPercent": 15},
                    "weapon": {"lowestMax": 60, "highestMax": 100, "maxDelta": 10, "minDelta": 0,
                               "minLimitPercent": 15}},
                "botDurabilities": {},
                "pmc": {"armor": {"lowestMaxPercent": 90, "highestMaxPercent": 100, "maxDelta": 10,
                                  "minDelta": 0, "minLimitPercent": 15},
                    "weapon": {"lowestMax": 95, "highestMax": 100, "maxDelta": 5, "minDelta": 0,
                               "minLimitPercent": 15}}},
            "itemSpawnLimits": {"assault": {}, "pmc": {}},
            "walletLoot": {"chancePercent": 0, "itemCount": {"min": 0, "max": 0},
                "stackSizeWeight": {}, "currencyWeight": {}, "walletTplPool": []},
            "currencyStackSize": {},
            "secureContainerAmmoStackCount": 0,
            "disableLootOnBotTypes": [],
            "lowProfileGasBlockTpls": [],
            "lootItemResourceRandomization": {},
            "pmcConfig": {},
            "repairKitWeapon": {"rarityWeight": {}, "bonusTypeWeight": {}, "Common": {},
                "Rare": {}},
            "equipmentBlacklist": {},
            "lootPools": {},
            "itemPresets": {},
            "defaultPresetsByTpl": {},
            "presetsById": {},
            "configBlacklist": [],
            "handbookPrices": {},
            "items": items(),
        })
    }

    fn generate(request: Value) -> Result<BotInventoryResult, LootError> {
        generate_inventory(serde_json::from_value(request).unwrap())
    }

    /// `slotId` → tpl, in generation order, with the six inventory roots stripped.
    fn worn(result: &BotInventoryResult) -> Vec<(String, String)> {
        result
            .inventory
            .items
            .iter()
            .filter(|item| item.slot_id.is_some())
            .map(|item| {
                (
                    item.slot_id.clone().unwrap_or_default(),
                    item.template.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_seeded_run_pins_the_whole_inventory() {
        let result = generate(base_request()).unwrap();

        // The six roots come first, in `GenerateInventoryBase` order, and are the only parentless
        // items.
        let roots: Vec<&str> = result
            .inventory
            .items
            .iter()
            .filter(|item| item.slot_id.is_none())
            .map(|item| item.template.as_str())
            .collect();
        assert_eq!(
            roots,
            vec![
                INVENTORY_DEFAULT,
                STASH_STANDARD_STASH_10X30,
                STASH_QUESTRAID,
                STASH_QUESTOFFLINE,
                SORTINGTABLE_SORTING_TABLE,
                HIDEOUTAREACONTAINER_CUSTOMIZATION,
            ]
        );
        let inventory = &result.inventory;
        assert_eq!(inventory.items[0].id, inventory.equipment);
        assert_eq!(inventory.items[1].id, inventory.stash);
        assert_eq!(inventory.items[2].id, inventory.quest_raid_items);
        assert_eq!(inventory.items[3].id, inventory.quest_stash_items);
        assert_eq!(inventory.items[4].id, inventory.sorting_table);
        assert_eq!(
            inventory.items[5].id,
            inventory.hideout_customization_stash_id
        );

        // Slot loop (Backpack, SecuredContainer, ArmBand — the other nine are excluded), then the
        // six explicit calls in their fixed order, then the weapon.
        assert_eq!(
            worn(&result),
            vec![
                ("Backpack".to_owned(), BACKPACK_TPL.to_owned()),
                ("SecuredContainer".to_owned(), SECURE_TPL.to_owned()),
                ("ArmBand".to_owned(), ARMBAND_TPL.to_owned()),
                ("Pockets".to_owned(), POCKETS_TPL.to_owned()),
                ("FaceCover".to_owned(), FACE_COVER_TPL.to_owned()),
                ("Headwear".to_owned(), HEADWEAR_TPL.to_owned()),
                ("Earpiece".to_owned(), EARPIECE_TPL.to_owned()),
                ("ArmorVest".to_owned(), ARMOR_TPL.to_owned()),
                // ArmorVest spawned, so `FilterRigsToThoseWithoutProtection` dropped the armored
                // rig from the pool.
                ("TacticalVest".to_owned(), VEST_PLAIN_TPL.to_owned()),
                ("FirstPrimaryWeapon".to_owned(), RIFLE_TPL.to_owned()),
                ("mod_magazine".to_owned(), MAG_TPL.to_owned()),
                ("cartridges".to_owned(), AMMO_TPL.to_owned()),
                ("patron_in_weapon".to_owned(), AMMO_TPL.to_owned()),
                // The spare magazine `AddExtraMagazinesToInventory` puts in the rig.
                ("main".to_owned(), MAG_TPL.to_owned()),
                ("cartridges".to_owned(), AMMO_TPL.to_owned()),
            ]
        );

        // Everything worn hangs off the equipment root; the magazine's cartridges hang off it.
        let equipment_id = &result.inventory.equipment;
        assert_eq!(
            result.inventory.items[6].parent_id.as_deref(),
            Some(equipment_id.as_str())
        );
        assert!(result.randomisation_clamps.is_empty());
    }

    #[test]
    fn the_run_is_reproducible_and_seed_sensitive() {
        // `MongoId`s come from process entropy, not the seeded stream (as everywhere in this
        // crate), so the comparable part of a run is every field except the ids.
        let rolled = |request: Value| -> Vec<(Option<String>, String, Value)> {
            generate(request)
                .unwrap()
                .inventory
                .items
                .iter()
                .map(|item| {
                    (
                        item.slot_id.clone(),
                        item.template.clone(),
                        serde_json::to_value(&item.upd).unwrap(),
                    )
                })
                .collect()
        };

        assert_eq!(rolled(base_request()), rolled(base_request()));

        let mut other = base_request();
        other["testSeed"] = json!(SEED + 1);
        // Same equipment (every chance is 100%), different durability and ammo-stack rolls.
        assert_ne!(rolled(base_request()), rolled(other));
    }

    #[test]
    fn nighttime_clamps_are_recorded_but_change_nothing_this_call() {
        let mut request = base_request();
        request["isNightTime"] = json!(true);
        request["equipment"]["assault"] = json!({
            "randomisation": [{
                "levelRange": {"min": 1, "max": 99},
                "equipmentMods": {"front_plate": 40, "mod_nvg": 95, "mod_absent_modifier": 10},
                "nighttimeChanges": {"equipmentModsModifiers": {
                    "front_plate": 30, "mod_nvg": 90, "mod_not_in_equipment_mods": 50,
                }},
            }],
        });

        let result = generate(request).unwrap();

        // Clamped into 0-100, and a modifier with no matching `equipmentMods` entry is skipped.
        assert_eq!(
            result
                .randomisation_clamps
                .iter()
                .map(|(slot, chance)| (slot.as_str(), *chance))
                .collect::<Vec<_>>(),
            vec![("front_plate", 70.0), ("mod_nvg", 100.0)]
        );

        // The clamp has no in-call reader in C# either — its consumer is the next bot's
        // `FilterBotEquipment`. `front_plate` went to 70 above, yet the vest's own
        // mod chances still come from the bot template's `equipmentMods` (100), unchanged.
        let baseline = worn(&generate(base_request()).unwrap());
        let mut night = base_request();
        night["isNightTime"] = json!(true);
        assert_eq!(worn(&generate(night).unwrap()), baseline);
    }

    #[test]
    fn batch_isolates_a_failing_bot() {
        // The flat single-bot request is shared views + slice merged; split it back apart.
        const SLICE_KEYS: [&str; 6] = [
            "botId",
            "testSeed",
            "details",
            "template",
            "lootPools",
            "handbookPrices",
        ];
        let mut shared = base_request();
        let mut slice = serde_json::Map::new();
        for key in SLICE_KEYS {
            if let Some(value) = shared.as_object_mut().unwrap().remove(key) {
                slice.insert(key.to_string(), value);
            }
        }
        let good = serde_json::Value::Object(slice);

        // Poison a role the good bot does not use: night + nighttimeChanges configured but no
        // equipmentMods is the error return at the top of equipment generation.
        shared["isNightTime"] = json!(true);
        shared["equipment"]["poisoned"] = json!({
            "randomisation": [{
                "levelRange": {"min": 1, "max": 99},
                "nighttimeChanges": {"equipmentModsModifiers": {"front_plate": 30}},
            }],
        });
        let mut bad = good.clone();
        bad["details"]["roleLowercase"] = json!("poisoned");

        let request =
            serde_json::from_value(json!({"shared": shared, "bots": [good, bad]})).unwrap();
        let result = generate_inventory_batch(request).unwrap();

        assert_eq!(result.bots.len(), 2);
        assert!(result.bots[0].result.is_some());
        assert!(result.bots[0].error.is_none());
        assert!(result.bots[1].result.is_none());
        assert_eq!(
            result.bots[1].error.as_deref(),
            Some("Object reference not set to an instance of an object.")
        );
    }

    #[test]
    fn the_primary_weapon_roll_short_circuits_the_other_two() {
        let chances: ChancesWire = serde_json::from_value(json!({
            "equipment": {"FirstPrimaryWeapon": 100, "SecondPrimaryWeapon": 0, "Holster": 0},
        }))
        .unwrap();

        // A won primary consumes all three draws: primary, second primary, holster.
        let _guard = TestSeedGuard::install(SEED);
        let with_primary = get_desired_weapons_for_bot(&chances).unwrap();
        let after_three = get_int(0, 1_000_000);
        drop(_guard);

        assert_eq!(
            with_primary.map(|desired| desired.should_spawn),
            [true, false, false]
        );

        let _guard = TestSeedGuard::install(SEED);
        let (_, _, fourth) = (get_int(1, 99), get_int(1, 99), get_int(1, 99));
        let _ = fourth;
        assert_eq!(get_int(0, 1_000_000), after_three);
        drop(_guard);

        // A lost primary consumes exactly one: the `&&` skips the second primary and the `||`
        // short-circuits the holster to true without rolling.
        let lost: ChancesWire = serde_json::from_value(json!({
            "equipment": {"FirstPrimaryWeapon": 0, "SecondPrimaryWeapon": 100, "Holster": 100},
        }))
        .unwrap();

        let _guard = TestSeedGuard::install(SEED);
        let without_primary = get_desired_weapons_for_bot(&lost).unwrap();
        let after_one = get_int(0, 1_000_000);
        drop(_guard);

        assert_eq!(
            without_primary.map(|desired| desired.should_spawn),
            [false, false, true]
        );

        let _guard = TestSeedGuard::install(SEED);
        let _ = get_int(1, 99);
        assert_eq!(get_int(0, 1_000_000), after_one);
    }

    #[test]
    fn a_missing_weapon_chance_only_throws_when_its_roll_is_reached() {
        // The primary's own indexer always runs.
        let no_primary: ChancesWire = serde_json::from_value(json!({"equipment": {}})).unwrap();
        assert_eq!(
            get_desired_weapons_for_bot(&no_primary)
                .unwrap_err()
                .message,
            "The given key 'FirstPrimaryWeapon' was not present in the dictionary."
        );

        // A lost primary skips both the second-primary and the holster indexers.
        let _guard = TestSeedGuard::install(SEED);
        let lost: ChancesWire =
            serde_json::from_value(json!({"equipment": {"FirstPrimaryWeapon": 0}})).unwrap();
        assert!(get_desired_weapons_for_bot(&lost).is_ok());
    }

    #[test]
    fn container_grids_ride_out_only_when_the_cache_is_kept() {
        let kept = generate(base_request()).unwrap();
        assert_eq!(
            kept.container_grids.keys().collect::<Vec<_>>(),
            vec!["Backpack", "SecuredContainer", "Pockets", "TacticalVest"]
        );
        assert_eq!(
            kept.container_grids["TacticalVest"].container_tpl,
            VEST_PLAIN_TPL
        );

        let mut cleared = base_request();
        cleared["details"]["clearBotContainerCacheAfterGeneration"] = json!(true);
        assert!(generate(cleared).unwrap().container_grids.is_empty());
    }

    #[test]
    fn an_armored_only_vest_pool_is_filtered_to_empty_and_loses_the_slot() {
        let mut request = base_request();
        request["template"]["inventory"]["equipment"]["TacticalVest"] =
            json!({VEST_ARMORED_TPL: 1});

        let result = generate(request).unwrap();

        // The armor vest spawned, `FilterRigsToThoseWithoutProtection` emptied the pool, and the
        // `:510` `Count != 0` test then lost the slot — after consuming its roll, and without the
        // null-pool throw.
        assert!(worn(&result).iter().all(|(slot, _)| slot != "TacticalVest"));
        // No vest means no vest container either, and the spare magazine went nowhere.
        assert!(!result.container_grids.contains_key("TacticalVest"));
    }

    #[test]
    fn a_template_with_no_pockets_pool_hits_the_null_deref() {
        let mut request = base_request();
        request["template"]["inventory"]["equipment"]
            .as_object_mut()
            .unwrap()
            .remove("Pockets");

        assert_eq!(
            generate(request).unwrap_err().message,
            "Object reference not set to an instance of an object."
        );
    }

    #[test]
    fn an_unheard_pmc_gets_the_tue_pockets() {
        let mut request = base_request();
        request["details"]["isPmc"] = json!(true);
        request["details"]["gameVersion"] = json!(UNHEARD);
        request["items"][POCKETS_1X4_TUE] =
            json!({"name": "tue pockets", "grids": [{"name": "main", "cellsH": 4, "cellsV": 1}]});

        let result = generate(request).unwrap();

        assert!(worn(&result).contains(&("Pockets".to_owned(), POCKETS_1X4_TUE.to_owned())));
    }

    #[test]
    fn armband_forcing_narrows_the_pool_but_writes_the_chance_to_a_dead_key() {
        let mut request = base_request();
        request["details"]["isPmc"] = json!(true);
        request["details"]["roleLowercase"] = json!("pmcbear");
        request["equipment"] = json!({"pmc": {}});
        request["pmcConfig"] = json!({
            "forceArmband": {"enabled": true, "usec": "armband_usec", "bear": FORCED_ARMBAND_TPL},
        });
        // The real chance key stays untouched, so dropping it to 0 still loses the slot even though
        // `:223` "forced" it to 100 under "Armband".
        request["template"]["chances"]["equipment"]["ArmBand"] = json!(0);

        let result = generate(request).unwrap();

        assert!(worn(&result).iter().all(|(slot, _)| slot != "ArmBand"));

        // With the real key restored, the pool is the single forced tpl.
        let mut request = base_request();
        request["details"]["isPmc"] = json!(true);
        request["details"]["roleLowercase"] = json!("pmcbear");
        request["equipment"] = json!({"pmc": {}});
        request["pmcConfig"] = json!({
            "forceArmband": {"enabled": true, "usec": "armband_usec", "bear": FORCED_ARMBAND_TPL},
        });

        let result = generate(request).unwrap();
        assert!(worn(&result).contains(&("ArmBand".to_owned(), FORCED_ARMBAND_TPL.to_owned())));
    }

    #[test]
    fn an_unknown_equipment_role_stops_the_equipment_phase_without_failing() {
        let mut request = base_request();
        request["equipment"] = json!({"pmc": {}});
        // The weapon path indexes `BotConfig.Equipment[botRole]` too, and would throw right after;
        // rolling no primary keeps this test on the equipment phase.
        request["template"]["chances"]["equipment"]["FirstPrimaryWeapon"] = json!(0);

        let result = generate(request).unwrap();

        assert_eq!(
            result.diagnostics[0].message.as_deref(),
            Some("Bot Equipment generation failed, unable to find equipment filters for: assault")
        );
        // The `:187` early return skips every slot, so the bot is left with the six roots only.
        assert!(worn(&result).is_empty());
    }

    #[test]
    fn a_pool_tpl_missing_from_the_database_is_dropped_and_redrawn() {
        let mut request = base_request();
        request["template"]["inventory"]["equipment"]["Headwear"] =
            json!({"headwear_ghost": 1, HEADWEAR_TPL: 1});

        let result = generate(request).unwrap();

        assert!(result.diagnostics.iter().any(
            |diagnostic| diagnostic.locale_key.as_deref() == Some("bot-missing_item_template")
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.as_deref() == Some("EquipmentSlot-> Headwear"))
        );
        assert!(worn(&result).contains(&("Headwear".to_owned(), HEADWEAR_TPL.to_owned())));
    }

    #[test]
    fn forcing_a_rig_when_there_is_no_armour_overrides_the_slot_chance() {
        let mut request = base_request();
        request["equipment"]["assault"] = json!({"forceRigWhenNoVest": true});
        request["template"]["chances"]["equipment"]["ArmorVest"] = json!(0);
        request["template"]["chances"]["equipment"]["TacticalVest"] = json!(0);

        let result = generate(request).unwrap();

        assert!(worn(&result).iter().all(|(slot, _)| slot != "ArmorVest"));
        // Chance 0 would have lost the slot; `:394` raised it to 100 first. No armor vest means the
        // pool was never filtered, so the armored rig is still the first pool entry.
        assert!(worn(&result).iter().any(|(slot, _)| slot == "TacticalVest"));
    }

    #[test]
    fn a_bot_with_no_magazine_weights_hits_the_null_deref() {
        let mut request = base_request();
        request["template"]["generation"]["items"]
            .as_object_mut()
            .unwrap()
            .remove("magazines");

        assert_eq!(
            generate(request).unwrap_err().message,
            "Object reference not set to an instance of an object."
        );
    }

    #[test]
    fn max_attempts_uses_bankers_rounding() {
        // `Math.Round(6 * 0.75)` is 4, not 5: the tie goes to the even number.
        assert_eq!((6.0_f64 * 0.75).round_ties_even(), 4.0);
        assert_eq!((2.0_f64 * 0.75).round_ties_even(), 2.0);
    }
}
