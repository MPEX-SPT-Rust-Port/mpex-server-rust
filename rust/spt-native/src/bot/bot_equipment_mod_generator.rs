//! `Generators/Bot/BotEquipmentModGenerator.cs` — both halves of the mod generator:
//! [`generate_mods_for_equipment`] (`:97-502`) and [`generate_mods_for_weapon`] (`:503-1916`), plus
//! the one method of `Services/Bot/BotWeaponModLimitService.cs` the latter calls
//! ([`weapon_mod_has_reached_limit`], ported inline because its whole state is the
//! [`BotModLimitsWire`] counters the request already carries).
//!
//! The helpers both halves share ([`should_mod_be_spawned`], [`get_mod_item_slot_from_db_template`],
//! [`filter_mods_by_blacklist`], [`get_random_mod_tpl_from_item_db`], [`is_mod_valid_for_slot`],
//! [`create_mod_item`]) sit between the two.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! Per mod slot of [`generate_mods_for_equipment`], in this order:
//!
//! 1. `ShouldModBeSpawned` (`:166`, `:989-1014`) — **1 `RollChance`**, unless the slot is one of the
//!    ammo containers, which return `SPAWN` without drawing. A slot missing from the chance map rolls
//!    against 0 and so always fails, but the draw is still consumed.
//! 2. `FilterPlateModsForSlotByLevel` (`:218`, `:315-451`) — for a plate slot on a bot configured to
//!    filter plates by level: **1 `GetWeightedValue`** at `:350`, always consumed, *before* the
//!    `maxArmorLevel` clamp at `:353` is applied to its result. The wraparound loop that follows
//!    draws nothing.
//! 3. The compatibility walk (`:252-265`) — one `ExhaustableArray` draw (`GetInt`) per candidate
//!    until a compatible one is found or the pool empties.
//! 4. `GetRandomModTplFromItemDb` (`:270`, `:1540-1559`), only for a required slot nothing was found
//!    for — one draw per candidate over the slot's own filter.
//! 5. `CreateModItem` (`:289`) → `GenerateExtraPropertiesForItem`, whose draws are listed in
//!    [`crate::bot::bot_generator_helper`].
//! 6. The recursion at `:300` repeats all of the above for the added mod's own pool.
//!
//! Per mod slot of [`generate_mods_for_weapon`], in this order:
//!
//! 1. `ShouldModBeSpawned` (`:566`) — the same **1 `RollChance`**, ammo containers excepted.
//! 2. `ChooseModToPutIntoSlot` (`:589`, `:1021-1142`):
//!    - an ammo container other than `mod_magazine` returns `request.AmmoTpl` at `:1030` **without
//!      drawing**;
//!    - every pool filter it applies first — the default-preset lookup, the sight whitelist, the
//!      gas-block split, the magazine-capacity filter and the conflict/parent-filter passes — is a
//!      pure read;
//!    - `GetCompatibleModFromPool` (`:1244-1305`) — one `ExhaustableArray` draw (`GetInt`) per
//!      candidate until one is compatible, the pool empties, or the blocked-attempt ceiling
//!      (`round(count * 0.75)`, half to even, and the count must get **past** it) is passed;
//!    - `GetRandomModTplFromItemDb` (`:1119`), only for a required slot nothing was found for — one
//!      draw per candidate over the slot's own filter.
//! 3. `CreateModItem` (`:688`) → `GenerateExtraPropertiesForItem`. The `new MongoId()` one line
//!    above it is not off this stream.
//! 4. `FillCamora` (`:705`, `:1735-1814`), for a cylinder magazine only — one draw per candidate
//!    ammo tpl until a compatible one is found. The clones it then makes draw nothing, and it is an
//!    alternative to step 5, not an addition.
//! 5. The recursion at `:761` repeats all of the above for the added mod's own pool.
#![allow(
    dead_code,
    reason = "consumed by the bot inventory generator in the tasks that follow"
)]

use indexmap::{IndexMap, IndexSet};

use crate::bot::BotContext;
use crate::bot::bot_generator_helper::{
    generate_extra_properties_for_item, is_item_incompatible_with_current_items,
};
use crate::bot::bot_weapon_generator_helper::magazine_is_cylinder_related;
use crate::bot::exhaustable_array::ExhaustableArray;
use crate::bot::mod_pool_service::{
    get_compatible_mods_for_weapon_slot, get_mods_for_weapon_slot,
    get_required_mods_for_weapon_slot,
};
use crate::bot::models::{
    BotDataWire, BotModLimitsWire, ChooseRandomCompatibleModResult, EquipmentFilterDetails,
    EquipmentFilters, GenerateEquipmentPropertiesWire, GenerateWeaponRequestWire, ItemCountWire,
    RandomisationDetails, WeaponStatsWire,
};
use crate::loot::item_helper::{
    ASSAULT_SCOPE, IRON_SIGHT, LAUNCHER, LootError, MOUNT, OPTIC_SCOPE, SIGHTS, SPECIAL_SCOPE,
    get_item, is_of_baseclass, is_of_baseclasses,
};
use crate::loot::models::{
    DEBUG, Diagnostic, ERROR, Item, ItemView, PresetView, SlotView, WARNING,
};
use crate::loot::mongo_id;
use crate::loot::random_util::{get_weighted_value, roll_chance, round_half_even};

/// `BotEquipmentModGenerator._cartridgeHolderSlots` (`:67-74`), returned by `GetAmmoContainers`
/// (`:1527-1530`).
const CARTRIDGE_HOLDER_SLOTS: [&str; 5] = [
    "mod_magazine",
    "patron_in_weapon",
    "patron_in_weapon_000",
    "patron_in_weapon_001",
    "cartridges",
];

/// `BotEquipmentModGenerator._modSightIds` (`:46`).
const MOD_SIGHT_IDS: [&str; 2] = ["mod_sight_front", "mod_sight_rear"];

/// `BotEquipmentModGenerator._scopeIds` (`:49-58`) — slots that hold scopes.
const SCOPE_IDS: [&str; 7] = [
    "mod_scope",
    "mod_mount",
    "mod_mount_000",
    "mod_scope_000",
    "mod_scope_001",
    "mod_scope_002",
    "mod_scope_003",
];

/// The scope slots `:621` forces to 100% — a different list from [`SCOPE_IDS`].
const SCOPE_SLOTS_TO_FORCE: [&str; 5] = [
    "mod_scope",
    "mod_scope_000",
    "mod_scope_001",
    "mod_scope_002",
    "mod_scope_003",
];

/// `BotEquipmentModGenerator._muzzleIds` (`:61`) — slots that hold muzzles, and the list `:635`
/// forces to 95%.
const MUZZLE_IDS: [&str; 3] = ["mod_muzzle", "mod_muzzle_000", "mod_muzzle_001"];

/// `BotEquipmentModGenerator._stockSlots` (`:64`) — slots a weapon can store its stock in, and the
/// list `:665` forces to 100%.
const STOCK_SLOTS: [&str; 4] = [
    "mod_stock",
    "mod_stock_000",
    "mod_stock_001",
    "mod_stock_akms",
];

// The `SortModKeys` slot-name constants (`:76-85`).
const MOD_RECIEVER_KEY: &str = "mod_reciever";
const MOD_MOUNT_001_KEY: &str = "mod_mount_001";
const MOD_GAS_BLOCK_KEY: &str = "mod_gas_block";
const MOD_PISTOL_GRIP: &str = "mod_pistol_grip";
const MOD_STOCK_KEY: &str = "mod_stock";
const MOD_BARREL_KEY: &str = "mod_barrel";
const MOD_HANDGUARD_KEY: &str = "mod_handguard";
const MOD_MOUNT_KEY: &str = "mod_mount";
const MOD_SCOPE_KEY: &str = "mod_scope";
const MOD_SCOPE_000_KEY: &str = "mod_scope_000";

// The `ItemTpl` members this path names, copied verbatim from `Models/Enums/ItemTpl.cs`.
/// The M4A1 front sight with gas block (`:793`), which has no `ItemTpl` member in the C# either.
const GASBLOCK_M4A1_FRONT_SIGHT: &str = "5ae30e795acfc408fb139a0b";
/// `ItemTpl.MOUNT_NCSTAR_MPR45_BACKUP`
const MOUNT_NCSTAR_MPR45_BACKUP: &str = "5649a2464bdc2d91118b45a8";
/// `ItemTpl.RECEIVER_HK_MP5SD_9X19_UPPER`
const RECEIVER_HK_MP5SD_9X19_UPPER: &str = "5926f2e086f7745aae644231";
/// `ItemTpl.BARREL_DVL10_762X51_500MM_SUPPRESSED`
const BARREL_DVL10_762X51_500MM_SUPPRESSED: &str = "5888945a2459774bf43ba385";
/// `ItemTpl.SMG_SOYUZTM_STM9_GEN2_9X19_CARBINE`
const SMG_SOYUZTM_STM9_GEN2_9X19_CARBINE: &str = "60339954d62c9b14ed777c06";
/// `ItemTpl.HANDGUARD_AR15_LONE_STAR_ION_LITE`
const HANDGUARD_AR15_LONE_STAR_ION_LITE: &str = "5d4405f0a4b9361e6a4e6bd9";
/// The MP5 preset `GetMatchingPreset` swaps in for an MP5SD receiver (`:1469`).
const MP5SD_PRESET_ID: &str = "59411abb86f77478f702b5d2";
/// The DVL preset `GetMatchingPreset` swaps in for the suppressed barrel (`:1477`).
const DVL_SILENCED_PRESET_ID: &str = "59e8d2b386f77445830dd299";

/// `MongoId.Empty()`, the fallback `:1119` hands `GetRandomModTplFromItemDb`.
const MONGO_ID_EMPTY: &str = "000000000000000000000000";

/// `ItemHelper._removablePlateSlotIds` (`Helpers/Items/ItemHelper.cs:100`).
const REMOVABLE_PLATE_SLOT_IDS: [&str; 4] = [
    "front_plate",
    "back_plate",
    "left_side_plate",
    "right_side_plate",
];

/// `Models/Enums/ModSpawn.cs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModSpawn {
    DefaultMod,
    Spawn,
    Skip,
}

impl ModSpawn {
    /// The C# member name, which is what `ToString()` interpolates into the `:1199` reason string.
    fn name(self) -> &'static str {
        match self {
            Self::DefaultMod => "DEFAULT_MOD",
            Self::Spawn => "SPAWN",
            Self::Skip => "SKIP",
        }
    }
}

/// `Models/Spt/Bots/FilterPlateModsForSlotByLevelResult.cs:15-22`. The C# enum is named `Result`,
/// which would shadow the prelude here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateFilterResult {
    UnknownFailure,
    Success,
    NoDefaultFilter,
    NotPlateHoldingSlot,
    LacksPlateWeights,
}

impl PlateFilterResult {
    /// The C# member name, which is what `ToString()` interpolates into the `:232` debug line.
    fn name(self) -> &'static str {
        match self {
            Self::UnknownFailure => "UNKNOWN_FAILURE",
            Self::Success => "SUCCESS",
            Self::NoDefaultFilter => "NO_DEFAULT_FILTER",
            Self::NotPlateHoldingSlot => "NOT_PLATE_HOLDING_SLOT",
            Self::LacksPlateWeights => "LACKS_PLATE_WEIGHTS",
        }
    }
}

/// `Models/Spt/Bots/FilterPlateModsForSlotByLevelResult.cs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterPlateModsForSlotByLevelResult {
    pub result: PlateFilterResult,
    pub plate_mod_templates: Option<IndexSet<String>>,
}

/// `BotEquipmentModGenerator.GenerateModsForEquipment` (`:97-305`).
///
/// Three signature deviations from the C#:
/// - the parent template rides as its **tpl**, not as a view: `TemplateItem.Id` is the mod-pool key,
///   the `GetDefaultPlateTpl` receiver and half the log arguments, and a flattened [`ItemView`] row
///   carries no id of its own;
/// - `settings` is shared, not mutable — nothing on this path writes to it;
/// - the mutated `equipment` list is the return value in C#; here it is the `&mut` in, so the
///   `Ok(())` carries no payload.
///
/// # Errors
///
/// Where the C# throws: the `:270` intentional null dereference (see below), and anything
/// [`filter_plate_mods_for_slot_by_level`] or `GenerateExtraPropertiesForItem` throws.
pub fn generate_mods_for_equipment(
    ctx: &mut BotContext,
    equipment: &mut Vec<Item>,
    parent_id: &str,
    parent_tpl: &str,
    settings: &GenerateEquipmentPropertiesWire,
    specific_blacklist: &EquipmentFilterDetails,
    should_force_spawn: bool,
) -> Result<(), LootError> {
    let mut force_spawn = should_force_spawn;

    // Copied out so the items view stays readable while `ctx` is borrowed mutably for diagnostics.
    let items = ctx.items;

    let parent_name = get_item(items, parent_tpl)
        .and_then(|template| template.name.clone())
        .unwrap_or_default();

    // Get mod pool for the desired item
    let compatible_mods_pool = settings.mod_pool.get(parent_tpl);
    if compatible_mods_pool.is_none() {
        let role = &settings.bot_data.role;
        ctx.diagnostics.push(diagnostic(
            WARNING,
            format!("bot: {role} lacks a mod slot pool for item: {parent_tpl} {parent_name}"),
        ));
    }

    // Order the modpool by front plates, then backplates, then everything else. `sort_by_key` is
    // stable, as LINQ's `OrderBy` is, so the residual order is the map's insertion order.
    let mut ordered_compatible_mods_pool: Vec<(&String, &IndexSet<String>)> = compatible_mods_pool
        .map(|pool| pool.iter().collect())
        .unwrap_or_default();
    ordered_compatible_mods_pool.sort_by_key(|(slot_name, _)| {
        if slot_name.eq_ignore_ascii_case("front_plate") {
            return 0;
        }

        if slot_name.eq_ignore_ascii_case("back_plate") {
            return 1;
        }

        2
    });

    let mut front_plate_spawned = false;
    // Iterate over mod pool and choose mods to add to item
    for (mod_slot_name, mod_pool) in ordered_compatible_mods_pool {
        // Skip backplate slot if there's no front plate and bot should skip it via config
        if mod_slot_name.eq_ignore_ascii_case("back_plate")
            && settings
                .bot_equipment_config
                .skip_back_plate_if_front_plate_missing
                .unwrap_or(false)
            && !front_plate_spawned
        {
            continue;
        }

        // Get the templates slot object from db
        let parent_template = get_item(items, parent_tpl);
        let Some(item_slot_template) =
            get_mod_item_slot_from_db_template(mod_slot_name, parent_template)
        else {
            ctx.diagnostics.push(Diagnostic {
                level: ERROR.to_owned(),
                locale_key: Some("bot-mod_slot_missing_from_item".to_owned()),
                args: Some(serde_json::json!({
                    "modSlot": mod_slot_name,
                    "parentId": parent_tpl,
                    "parentName": parent_name,
                    "botRole": settings.bot_data.role,
                })),
                message: None,
            });

            continue;
        };
        let slot_required = item_slot_template.required.unwrap_or(false);
        let slot_filter = item_slot_template.filter.clone().unwrap_or_default();

        let mod_spawn_result = should_mod_be_spawned(
            item_slot_template,
            mod_slot_name,
            &settings.spawn_chances.equipment_mods,
            &settings.bot_equipment_config,
        );

        // Rolled to skip mod and it shouldn't be force-spawned
        if mod_spawn_result == ModSpawn::Skip && !force_spawn {
            continue;
        }

        // Ensure submods for nvgs all spawn together
        if mod_slot_name == "mod_nvg" {
            force_spawn = true;
        }

        // Get pool of items we can add for this slot
        let mut mod_pool_to_choose_from = mod_pool.clone();

        // Filter the pool of items in blacklist
        let filtered_mod_pool = filter_mods_by_blacklist(
            ctx,
            &mod_pool_to_choose_from,
            specific_blacklist,
            mod_slot_name,
        );
        if !filtered_mod_pool.is_empty()
        // use filtered pool as it has items in it
        {
            mod_pool_to_choose_from = filtered_mod_pool;
        }

        // Slot can hold armor plates + we are filtering possible items by bot level, handle
        if settings
            .bot_equipment_config
            .filter_plates_by_level
            .unwrap_or(false)
            && is_removable_plate_slot(&mod_slot_name.to_lowercase())
        {
            let mut front_plate_armor_class = None;
            if mod_slot_name.eq_ignore_ascii_case("back_plate")
                && settings
                    .bot_equipment_config
                    .limit_plate_class_to_front_plate_class
                    .unwrap_or(false)
            {
                let front_plate = equipment.iter().find(|item| {
                    item.slot_id
                        .as_deref()
                        .is_some_and(|slot| slot.eq_ignore_ascii_case("front_plate"))
                });

                if let Some(front_plate) = front_plate {
                    front_plate_armor_class =
                        get_item(items, &front_plate.template).and_then(|item| item.armor_class);
                }
            }

            // The *unfiltered* pool, as the C# `compatibleModsPool.GetValueOrDefault(modSlotName)`
            // at `:221` is — the blacklist filtering above is discarded for plate slots.
            let existing_plate_tpl_pool = compatible_mods_pool
                .and_then(|pool| pool.get(mod_slot_name))
                .cloned()
                .unwrap_or_default();

            let plate_slot_filtering_outcome = filter_plate_mods_for_slot_by_level(
                ctx,
                settings,
                &mod_slot_name.to_lowercase(),
                &existing_plate_tpl_pool,
                parent_tpl,
                front_plate_armor_class,
            )?;
            match plate_slot_filtering_outcome.result {
                PlateFilterResult::UnknownFailure | PlateFilterResult::NoDefaultFilter => {
                    let outcome = plate_slot_filtering_outcome.result.name();
                    ctx.diagnostics.push(diagnostic(
                        DEBUG,
                        format!(
                            "Plate slot: {mod_slot_name} selection for armor: {parent_tpl} failed: {outcome}, skipping"
                        ),
                    ));

                    continue;
                }
                PlateFilterResult::LacksPlateWeights => {
                    ctx.diagnostics.push(diagnostic(
                        WARNING,
                        format!(
                            "Plate slot: {mod_slot_name} lacks weights for armor: {parent_tpl}, unable to adjust plate choice, using existing data"
                        ),
                    ));
                }
                _ => {}
            }

            // Replace mod pool with pool of chosen plate items
            mod_pool_to_choose_from = plate_slot_filtering_outcome
                .plate_mod_templates
                .unwrap_or_default();
        }

        // Choose random mod from pool and check its compatibility
        let mut mod_tpl: Option<String> = None;
        let mut found = false;
        let mut exhaustable_mod_pool =
            ExhaustableArray::new(mod_pool_to_choose_from.into_iter().collect());
        while exhaustable_mod_pool.has_values() {
            mod_tpl = exhaustable_mod_pool.get_random_value();
            if let Some(tpl) = mod_tpl.clone()
                && !tpl.is_empty()
                && !is_item_incompatible_with_current_items(ctx, equipment, &tpl, mod_slot_name)
                    .incompatible
                    .unwrap_or(false)
            {
                found = true;
                break;
            }
        }

        // Compatible item not found but slot REQUIRES item, get random item from db
        if !found && slot_required {
            // `:270` dereferences `modTpl` without checking it: an empty pool never assigns it, so
            // this is a guaranteed throw for a required slot whose pool is empty. Ported as the
            // failure it is.
            let Some(fallback_mod_tpl) = mod_tpl.clone() else {
                return Err(LootError::new(format!(
                    "Nullable object must have a value: no mod was drawn for required slot: {mod_slot_name} on item: {parent_tpl}"
                )));
            };

            mod_tpl = get_random_mod_tpl_from_item_db(
                ctx,
                &fallback_mod_tpl,
                &slot_filter,
                mod_slot_name,
                equipment,
            );
            found = mod_tpl.is_some();
        }

        // Compatible item not found + not required - skip
        if !(found || slot_required) {
            continue;
        }

        // Get chosen mods db template and check it fits into slot
        let Some(chosen_mod_tpl) = mod_tpl else {
            return Err(LootError::new(format!(
                "Nullable object must have a value: no mod was chosen for slot: {mod_slot_name} on item: {parent_tpl}"
            )));
        };
        let mod_template = get_item(items, &chosen_mod_tpl);
        if !is_mod_valid_for_slot(
            ctx,
            mod_template.is_some(),
            &chosen_mod_tpl,
            mod_slot_name,
            parent_tpl,
        ) {
            continue;
        }

        // Generate new id to ensure all items are unique on bot
        let mod_id = mongo_id::generate();
        let mod_item = create_mod_item(
            ctx,
            &mod_id,
            &chosen_mod_tpl,
            parent_id,
            mod_slot_name,
            &settings.bot_data.role,
        )?;
        equipment.push(mod_item);

        if mod_slot_name.eq_ignore_ascii_case("front_plate") {
            front_plate_spawned = true;
        }

        // Does item being added exist in mod pool - has its own mod pool
        if settings.mod_pool.contains_key(&chosen_mod_tpl)
        // Call self again with mod being added as item to add child mods to
        {
            generate_mods_for_equipment(
                ctx,
                equipment,
                &mod_id,
                &chosen_mod_tpl,
                settings,
                specific_blacklist,
                force_spawn,
            )?;
        }
    }

    Ok(())
}

/// `BotEquipmentModGenerator.FilterPlateModsForSlotByLevel` (`:315-451`).
///
/// The C#'s `platesFromDb` is a lazy `Select` re-enumerated at `:366`, `:381`, `:399` and `:448`;
/// nothing inside it draws, so materialising it once and re-filtering is output-equivalent. The
/// repeated *filtering* is kept, wraparound bug and all: the wrap at `:392-395` writes `Min` into the
/// level **string** while `:399` keeps filtering on the un-wrapped `chosenArmorPlateLevelDouble`, so
/// the wrap only reaches the next iteration's `+ 1` and the debug message.
///
/// # Errors
///
/// Where the C# throws: a level string the weights hold that is not an integer (`int.Parse`), a
/// plate tpl missing from the items view or without an `ArmorClass` (`item.Properties.ArmorClass
/// .Value` at `:367`, and `item.Properties` at `:399`/`:448`), and an empty plate pool reaching
/// `GetMinMaxArmorPlateClass` (`platePool[0]`).
fn filter_plate_mods_for_slot_by_level(
    ctx: &mut BotContext,
    settings: &GenerateEquipmentPropertiesWire,
    mod_slot: &str,
    existing_plate_tpl_pool: &IndexSet<String>,
    armor_item_tpl: &str,
    max_armor_level: Option<i32>,
) -> Result<FilterPlateModsForSlotByLevelResult, LootError> {
    // Copied out so the items view stays readable while `ctx` is borrowed mutably for diagnostics.
    let items = ctx.items;

    // Not pmc or not a plate slot, return original mod pool array
    if !is_removable_plate_slot(mod_slot) {
        return Ok(FilterPlateModsForSlotByLevelResult {
            result: PlateFilterResult::NotPlateHoldingSlot,
            plate_mod_templates: Some(existing_plate_tpl_pool.clone()),
        });
    }

    // Get the front/back/side weights based on bots level
    let plate_slot_weights = settings
        .bot_equipment_config
        .armor_plate_weighting
        .iter()
        .flatten()
        .find(|armor_weight| {
            settings.bot_data.level >= armor_weight.level_range.min
                && settings.bot_data.level <= armor_weight.level_range.max
        });

    // Get the specific plate slot weights (front/back/side)
    let Some(plate_weights) = plate_slot_weights.and_then(|weights| weights.values.get(mod_slot))
    else {
        // No weights, return original array of plate tpls
        return Ok(FilterPlateModsForSlotByLevelResult {
            result: PlateFilterResult::LacksPlateWeights,
            plate_mod_templates: Some(existing_plate_tpl_pool.clone()),
        });
    };

    // Choose a plate level based on weighting
    let mut chosen_armor_plate_level_string = get_weighted_value(plate_weights)?;

    // Check if the max plate value was sent over, if it's null then it shouldn't be trying to limit
    // classes
    if let Some(max_armor_level) = max_armor_level {
        let chosen_level = parse_plate_level(&chosen_armor_plate_level_string)?;
        if chosen_level > max_armor_level {
            chosen_armor_plate_level_string = max_armor_level.to_string();
        }
    }

    // Convert the array of ids into database items. The `ArmorClass` unwrap is `:367`'s `.Value`:
    // every element's predicate is evaluated either by the `Any()` at `:369` or by the `ToHashSet`
    // at `:373`, so a null class anywhere in the pool throws whichever way the branch goes.
    let plates_from_db = existing_plate_tpl_pool
        .iter()
        .map(|plate_tpl| {
            let armor_class = get_item(items, plate_tpl)
                .ok_or_else(|| {
                    LootError::new(format!(
                        "Object reference not set to an instance of an object: plate tpl: {plate_tpl} is not in the items view"
                    ))
                })?
                .armor_class
                .ok_or_else(|| {
                    LootError::new(format!(
                        "Nullable object must have a value: plate tpl: {plate_tpl} has no ArmorClass"
                    ))
                })?;

            Ok((plate_tpl.as_str(), armor_class))
        })
        .collect::<Result<Vec<(&str, i32)>, LootError>>()?;

    // Filter plates to the chosen level based on its armorClass property
    let chosen_level = parse_plate_level(&chosen_armor_plate_level_string)?;
    let mut plates_of_desired_level = plates_of_class(&plates_from_db, chosen_level);
    if !plates_of_desired_level.is_empty() {
        // Plates found
        return Ok(FilterPlateModsForSlotByLevelResult {
            result: PlateFilterResult::Success,
            plate_mod_templates: Some(to_tpl_set(&plates_of_desired_level)),
        });
    }

    // no plates found that fit requirements, lets get creative

    // Get lowest and highest plate classes available for this armor
    let min_max_armor_plate_class = get_min_max_armor_plate_class(&plates_from_db)?;

    // Increment plate class level in attempt to get usable plate
    let mut find_compatible_plate_attempts = 0;
    const MAX_ATTEMPTS: i32 = 3;
    for _ in 0..MAX_ATTEMPTS {
        let chosen_armor_plate_level_double =
            parse_plate_level(&chosen_armor_plate_level_string)? + 1;
        chosen_armor_plate_level_string = chosen_armor_plate_level_double.to_string();

        // New chosen plate class is higher than max, then set to min and check if valid
        if chosen_armor_plate_level_double > min_max_armor_plate_class.1 {
            chosen_armor_plate_level_string = min_max_armor_plate_class.0.to_string();
        }

        find_compatible_plate_attempts += 1;

        plates_of_desired_level = plates_of_class(&plates_from_db, chosen_armor_plate_level_double);
        // Valid plates found, exit
        if !plates_of_desired_level.is_empty() {
            break;
        }

        // No valid plate class found in 3 tries, attempt default plates
        if find_compatible_plate_attempts >= MAX_ATTEMPTS {
            let armor_item_name = get_item(items, armor_item_tpl)
                .and_then(|armor_item| armor_item.name.clone())
                .unwrap_or_default();
            ctx.diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "Plate filter too restrictive for armor: {armor_item_name} {armor_item_tpl}, unable to find plates of level: {chosen_armor_plate_level_string}, using items default plate"
                ),
            ));

            let default_plate = get_item(items, armor_item_tpl)
                .and_then(|armor_item| get_default_plate_tpl(armor_item, mod_slot));
            if let Some(default_plate) = default_plate {
                // Return Default Plates cause couldn't get the lowest level available from original
                // selection
                return Ok(FilterPlateModsForSlotByLevelResult {
                    result: PlateFilterResult::Success,
                    plate_mod_templates: Some(IndexSet::from([default_plate])),
                });
            }

            // No plate found after filtering AND no default plate

            // Last attempt, get default preset and see if it has a plate default
            let default_preset_plate_slot =
                get_default_preset_armor_slot(ctx, armor_item_tpl, mod_slot);
            if let Some(default_preset_plate_slot) = default_preset_plate_slot {
                // Found a plate, exit
                let plate_tpl = default_preset_plate_slot.template.clone();
                if get_item(items, &plate_tpl).is_none() {
                    return Err(LootError::new(format!(
                        "Object reference not set to an instance of an object: default preset plate tpl: {plate_tpl} is not in the items view"
                    )));
                }

                return Ok(FilterPlateModsForSlotByLevelResult {
                    result: PlateFilterResult::Success,
                    plate_mod_templates: Some(IndexSet::from([plate_tpl])),
                });
            }

            // Everything failed, no default plate or no default preset armor plate
            return Ok(FilterPlateModsForSlotByLevelResult {
                result: PlateFilterResult::NoDefaultFilter,
                plate_mod_templates: None,
            });
        }
    }

    // Only return the items ids
    Ok(FilterPlateModsForSlotByLevelResult {
        result: PlateFilterResult::Success,
        plate_mod_templates: Some(to_tpl_set(&plates_of_desired_level)),
    })
}

/// `BotEquipmentModGenerator.GetMinMaxArmorPlateClass` (`:458-482`), as `(min, max)`.
///
/// The C# sorts a copy of the pool and reads the first and last element's class; only those two
/// values are used, so the sort's instability cannot show. An empty pool indexes out of bounds
/// there, which is the error here.
fn get_min_max_armor_plate_class(plate_pool: &[(&str, i32)]) -> Result<(i32, i32), LootError> {
    let mut sorted: Vec<i32> = plate_pool.iter().map(|(_, class)| *class).collect();
    sorted.sort_unstable();

    let min = *sorted.first().ok_or_else(|| {
        LootError::new("Index was out of range: no plates to take a min/max armor class from")
    })?;
    let max = *sorted.last().ok_or_else(|| {
        LootError::new("Index was out of range: no plates to take a min/max armor class from")
    })?;

    Ok((min, max))
}

/// `BotEquipmentModGenerator.GetDefaultPresetArmorSlot` (`:490-495`), against the
/// `PresetHelper.GetDefaultPresetByTpl` projection [`BotContext::default_presets_by_tpl`] carries.
fn get_default_preset_armor_slot<'a>(
    ctx: &'a BotContext,
    armor_item_tpl: &str,
    mod_slot: &str,
) -> Option<&'a Item> {
    ctx.default_presets_by_tpl
        .get(armor_item_tpl)?
        .items
        .iter()
        .find(|item| {
            item.slot_id
                .as_deref()
                .is_some_and(|slot_id| slot_id.eq_ignore_ascii_case(mod_slot))
        })
}

// ---------------------------------------------------------------------------
// Weapon path (`:503-1916`)
// ---------------------------------------------------------------------------

/// `BotEquipmentModGenerator.GenerateModsForWeapon` (`:503-767`).
///
/// Signature deviations, all of them the ones the equipment half already made:
/// - `request.parent_template` is the parent's **tpl**, not a `TemplateItem`;
/// - the mutated `request.weapon` is the C# return value, so `Ok(())` carries no payload;
/// - `ctx` is `&mut` for the diagnostics buffer.
///
/// The per-run views the C# resolves out of its services at `:522-533` — the equipment config for
/// the bot's equipment role, the equipment blacklist, the sight whitelist and the low-profile
/// gas-block list — ride on [`BotContext`] instead of being re-resolved per recursion; they are
/// constant for a run, exactly as the C# ones are.
///
/// # Errors
///
/// Where the C# throws: a weapon tpl with no entry in the mod pool (`:536` dereferences it), an
/// equipment role missing from `botConfig.Equipment` (`:533` dereferences it, ahead of the `:781`
/// `ForceStock` deref that would otherwise be the first), a mod slot in the pool that the parent's
/// `Properties.Slots` does not carry once a mod has been picked for it (`:1204`), plus anything
/// `GenerateExtraPropertiesForItem` throws.
pub fn generate_mods_for_weapon(
    ctx: &mut BotContext,
    request: &mut GenerateWeaponRequestWire,
) -> Result<(), LootError> {
    // Copied out so the views stay readable while `ctx` is borrowed mutably for diagnostics.
    let items = ctx.items;
    let equipment = ctx.equipment;
    let bot_equip_blacklist = ctx.equipment_blacklist;

    let parent_template = get_item(items, &request.parent_template);
    let parent_name = parent_template
        .and_then(|template| template.name.clone())
        .unwrap_or_default();

    if has_no_slots_cartridges_or_chambers(parent_template) {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-unable_to_add_mods_to_weapon_missing_ammo_slot",
            serde_json::json!({
                "weaponName": parent_name,
                "weaponId": request.parent_template,
                "botRole": request.bot_data.role,
            }),
        ));

        return Ok(());
    }

    // `:533` hands this to `GetBotRandomizationDetails`, which dereferences it — and `:533` runs
    // before the `:536` pool read below, so a bot missing both fails on this one.
    let Some(bot_equip_config) = equipment.get(&request.bot_data.equipment_role) else {
        return Err(LootError::new(format!(
            "Object reference not set to an instance of an object: no equipment config for role: {}",
            request.bot_data.equipment_role
        )));
    };
    let bot_weapon_sight_whitelist = bot_equip_config.weapon_sight_whitelist.as_ref();
    let randomisation_settings =
        get_bot_randomization_details(request.bot_data.level, bot_equip_config);

    // Get pool of mods that fit weapon. `:536` reads `.Keys` off it unguarded.
    let Some(compatible_mods_pool) = request.mod_pool.get(&request.parent_template).cloned() else {
        return Err(LootError::new(format!(
            "Object reference not set to an instance of an object: no mod pool for item: {} on bot: {}",
            request.parent_template, request.bot_data.role
        )));
    };

    // Iterate over mod pool and choose mods to attach
    let sorted_mod_keys = sort_mod_keys(items, &compatible_mods_pool, &request.parent_template);
    for mod_slot in sorted_mod_keys {
        // Check weapon has slot for mod to fit in
        let Some(mods_parent_slot) = get_mod_item_slot_from_db_template(&mod_slot, parent_template)
        else {
            ctx.diagnostics.push(localised(
                ERROR,
                "bot-weapon_missing_mod_slot",
                serde_json::json!({
                    "modSlot": mod_slot,
                    "weaponId": request.parent_template,
                    "weaponName": parent_name,
                    "botRole": request.bot_data.role,
                }),
            ));

            continue;
        };

        // If the parent is a UBGL, the patron_in_weapon will be generated later - so skip it for now
        if mod_slot == "patron_in_weapon"
            && is_of_baseclass(items, &request.parent_template, LAUNCHER)
        {
            continue;
        }

        // Check spawn chance of mod
        let mod_spawn_result = should_mod_be_spawned(
            mods_parent_slot,
            &mod_slot,
            &request.mod_spawn_chances,
            bot_equip_config,
        );
        if mod_spawn_result == ModSpawn::Skip {
            continue;
        }

        let is_randomisable_slot = randomisation_settings.is_some_and(|settings| {
            settings
                .randomised_weapon_mod_slots
                .as_ref()
                .is_some_and(|slots| slots.contains(&mod_slot))
        });

        let mod_to_add = choose_mod_to_put_into_slot(
            ctx,
            &ModToSpawnRequest {
                mod_slot: &mod_slot,
                is_randomisable_slot,
                randomisation_settings,
                bot_weapon_sight_whitelist,
                bot_equip_blacklist,
                item_mod_pool: &compatible_mods_pool,
                weapon: &request.weapon,
                ammo_tpl: &request.ammo_tpl,
                parent_template: &request.parent_template,
                mod_spawn_result,
                weapon_stats: &request.weapon_stats,
                conflicting_item_tpls: &request.conflicting_item_tpls,
                bot_data: &request.bot_data,
            },
        )?;

        // Compatible mod not found
        let Some(mod_to_add_tpl) = mod_to_add else {
            continue;
        };

        let mod_to_add_template = get_item(items, &mod_to_add_tpl);
        if !is_mod_valid_for_slot(
            ctx,
            mod_to_add_template.is_some(),
            &mod_to_add_tpl,
            &mod_slot,
            &request.parent_template,
        ) {
            continue;
        }
        let Some(mod_to_add_template) = mod_to_add_template else {
            continue;
        };
        let mod_to_add_parent = mod_to_add_template.parent.clone().unwrap_or_default();

        // Skip adding mod to weapon if type limit reached
        if weapon_mod_has_reached_limit(
            ctx,
            &request.bot_data.equipment_role,
            &mod_to_add_tpl,
            mod_to_add_template,
            &mut request.mod_limits,
            &request.parent_template,
            &request.weapon,
        ) {
            continue;
        }

        // If item is a mount for scopes, set scope chance to 100%, this helps fix empty mounts
        // appearing on weapons
        if mod_slot_can_hold_scope(&mod_slot, &mod_to_add_parent) {
            // mod_mount was picked to be added to weapon, force scope chance to ensure its filled
            adjust_slot_spawn_chances(&mut request.mod_spawn_chances, &SCOPE_SLOTS_TO_FORCE, 100.0);

            // Hydrate pool of mods that fit into mount as its a randomisable slot
            if is_randomisable_slot
            // Add scope mods to modPool dictionary to ensure the mount has a scope in the pool to pick
            {
                add_compatible_mods_for_provided_mod(
                    ctx,
                    "mod_scope",
                    &mod_to_add_tpl,
                    mod_to_add_template,
                    &mut request.mod_pool,
                    bot_equip_blacklist,
                );
            }
        }

        // If picked item is muzzle adapter that can hold a child, adjust spawn chance
        if mod_slot_can_hold_muzzle_devices(&mod_slot, Some(&mod_to_add_parent)) {
            // Make chance of muzzle devices 95%, nearly certain but not guaranteed
            adjust_slot_spawn_chances(&mut request.mod_spawn_chances, &MUZZLE_IDS, 95.0);
        }

        // If front/rear sight are to be added, set opposite to 100% chance
        if mod_is_front_or_rear_sight(&mod_slot, &mod_to_add_tpl) {
            request
                .mod_spawn_chances
                .insert("mod_sight_front".to_owned(), 100.0);
            request
                .mod_spawn_chances
                .insert("mod_sight_rear".to_owned(), 100.0);
        }

        // Handguard mod can take a sub handguard mod + weapon has no UBGL (takes same slot)
        // Force spawn chance to be 100% to ensure it gets added
        if mod_slot == "mod_handguard"
            && has_slot_named(mod_to_add_template, "mod_handguard")
            && !request
                .weapon
                .iter()
                .any(|item| item.slot_id.as_deref() == Some("mod_launcher"))
        // Needed for handguards with lower
        {
            request
                .mod_spawn_chances
                .insert("mod_handguard".to_owned(), 100.0);
        }

        // If stock mod can take a sub stock mod, force spawn chance to be 100% to ensure sub-stock
        // gets added. Or if bot has stock force enabled
        if should_force_sub_stock_slots(&mod_slot, bot_equip_config, mod_to_add_template) {
            // Stock mod can take additional stocks, could be a locking device, force 100% chance
            adjust_slot_spawn_chances(&mut request.mod_spawn_chances, &STOCK_SLOTS, 100.0);
        }

        // Gather stats on mods being added to weapon
        if is_of_baseclass(items, &mod_to_add_tpl, IRON_SIGHT) {
            if mod_slot == "mod_sight_front" {
                request.weapon_stats.has_front_iron_sight = Some(true);
            } else if mod_slot == "mod_sight_rear" {
                request.weapon_stats.has_rear_iron_sight = Some(true);
            }
        } else if !request.weapon_stats.has_optic.unwrap_or(false)
            && is_of_baseclass(items, &mod_to_add_tpl, SIGHTS)
        {
            request.weapon_stats.has_optic = Some(true);
        }

        let mod_id = mongo_id::generate();
        let mod_item = create_mod_item(
            ctx,
            &mod_id,
            &mod_to_add_tpl,
            &request.weapon_id,
            &mod_slot,
            &request.bot_data.role,
        )?;
        request.weapon.push(mod_item);

        // Update conflicting item list now item has been chosen
        for conflicting_item in mod_to_add_template.conflicting_items.iter().flatten() {
            request
                .conflicting_item_tpls
                .insert(conflicting_item.clone());
        }

        // I first thought we could use the recursive generateModsForItems as previously for cylinder
        // magazines. However, the recursion doesn't go over the slots of the parent mod but over the
        // modPool which is given by the bot config where we decided to keep cartridges instead of
        // camoras. And since a CylinderMagazine only has one cartridge entry and this entry is not
        // to be filled, we need a special handling for the CylinderMagazine
        let mod_parent_name = get_item(items, &mod_to_add_parent)
            .and_then(|parent| parent.name.as_deref())
            .unwrap_or_default();
        if magazine_is_cylinder_related(mod_parent_name) {
            // We don't have child mods, we need to create the camoras for the magazines instead
            fill_camora(
                ctx,
                &mut request.weapon,
                &mut request.mod_pool,
                &mod_id,
                &mod_to_add_tpl,
                mod_to_add_template,
            );

            continue;
        }

        let mut contains_mod_in_pool = request.mod_pool.contains_key(&mod_to_add_tpl);

        // Sometimes randomised slots are missing sub-mods, if so, get values from mod pool service
        // Check for a randomisable slot + without data in modPool + item being added as additional
        // slots
        if is_randomisable_slot
            && !contains_mod_in_pool
            && mod_to_add_template
                .slots
                .as_ref()
                .is_some_and(|slots| !slots.is_empty())
        {
            let mod_from_service = get_mods_for_weapon_slot(ctx, &mod_to_add_tpl);
            if !mod_from_service.is_empty() {
                request
                    .mod_pool
                    .insert(mod_to_add_tpl.clone(), mod_from_service);
                contains_mod_in_pool = true;
            }
        }

        // Fallback when mods with REQUIRED children are not in the pool, add them and process
        if !contains_mod_in_pool && !is_randomisable_slot {
            // Check for required mods the item we've added needs to be classified as 'valid'
            let mod_from_service = get_required_mods_for_weapon_slot(ctx, &mod_to_add_tpl);
            if !mod_from_service.is_empty() {
                request
                    .mod_pool
                    .insert(mod_to_add_tpl.clone(), mod_from_service);
                contains_mod_in_pool = true;
            }
        }

        if contains_mod_in_pool {
            // Call self recursively to add mods to this mod. C# builds a fresh request around the
            // same shared `Weapon`/`ModPool`/`ModSpawnChances`/`ModLimits`/`WeaponStats`/
            // `ConflictingItemTpls` objects and swaps only these two members.
            let outer_weapon_id = std::mem::replace(&mut request.weapon_id, mod_id);
            let outer_parent_template =
                std::mem::replace(&mut request.parent_template, mod_to_add_tpl);

            let outcome = generate_mods_for_weapon(ctx, request);

            request.weapon_id = outer_weapon_id;
            request.parent_template = outer_parent_template;
            outcome?;
        }
    }

    Ok(())
}

/// `BotEquipmentModGenerator.ShouldForceSubStockSlots` (`:776-782`).
///
/// The C# `botEquipConfig.ForceStock` deref is unguarded, but a null config throws two hundred lines
/// earlier at `:533`, so the config is never null by the time this is reached.
fn should_force_sub_stock_slots(
    mod_slot: &str,
    bot_equip_config: &EquipmentFilters,
    mod_to_add_template: &ItemView,
) -> bool {
    // Can the stock hold child items
    let has_sub_slots = mod_to_add_template
        .slots
        .as_ref()
        .is_some_and(|slots| !slots.is_empty());

    (STOCK_SLOTS.contains(&mod_slot) && has_sub_slots)
        || bot_equip_config.force_stock.unwrap_or(false)
}

/// `BotEquipmentModGenerator.ModIsFrontOrRearSight` (`:790-800`).
fn mod_is_front_or_rear_sight(mod_slot: &str, tpl: &str) -> bool {
    // Gas block /w front sight is special case, deem it a 'front sight' too
    if mod_slot == "mod_gas_block" && tpl == GASBLOCK_M4A1_FRONT_SIGHT
    // M4A1 front sight with gas block
    {
        return true;
    }

    MOD_SIGHT_IDS.contains(&mod_slot)
}

/// `BotEquipmentModGenerator.ModSlotCanHoldScope` (`:808-811`).
fn mod_slot_can_hold_scope(mod_slot: &str, mods_parent_id: &str) -> bool {
    SCOPE_IDS.contains(&mod_slot.to_lowercase().as_str()) && mods_parent_id == MOUNT
}

/// `BotEquipmentModGenerator.AdjustSlotSpawnChances` (`:819-839`). The two null guards it logs for
/// are not expressible here — neither argument is an `Option` at either call site.
fn adjust_slot_spawn_chances(
    mod_spawn_chances: &mut IndexMap<String, f64>,
    mod_slots_to_adjust: &[&str],
    new_chance_percent: f64,
) {
    for mod_name in mod_slots_to_adjust {
        mod_spawn_chances.insert((*mod_name).to_owned(), new_chance_percent);
    }
}

/// `BotEquipmentModGenerator.ModSlotCanHoldMuzzleDevices` (`:847-850`).
fn mod_slot_can_hold_muzzle_devices(mod_slot: &str, mods_parent_id: Option<&str>) -> bool {
    // parity: parameter unused in C#
    let _ = mods_parent_id;

    MUZZLE_IDS.contains(&mod_slot.to_lowercase().as_str())
}

/// `BotEquipmentModGenerator.SortModKeys` (`:858-951`).
///
/// The C# takes and returns a `HashSet<string>`; a `Vec` here, since the keys are unique and only
/// their order matters. The residual keeps the pool's own order, as removing from a `HashSet`
/// without re-adding does.
fn sort_mod_keys(
    items: &IndexMap<String, ItemView>,
    unsorted_slot_keys: &IndexMap<String, IndexSet<String>>,
    item_tpl_with_keys_to_sort: &str,
) -> Vec<String> {
    // No need to sort with only 1 item in array
    if unsorted_slot_keys.len() <= 1 {
        return unsorted_slot_keys.keys().cloned().collect();
    }

    let is_mount = is_of_baseclass(items, item_tpl_with_keys_to_sort, MOUNT);

    // Mounts are a special case, they need scopes first before more mounts
    let leading: &[&str] = if is_mount {
        &[MOD_SCOPE_000_KEY, MOD_SCOPE_KEY, MOD_MOUNT_KEY]
    } else {
        &[
            MOD_HANDGUARD_KEY,
            MOD_BARREL_KEY,
            MOD_MOUNT_001_KEY,
            MOD_RECIEVER_KEY,
            MOD_PISTOL_GRIP,
            MOD_GAS_BLOCK_KEY,
            MOD_STOCK_KEY,
            MOD_MOUNT_KEY,
            MOD_SCOPE_KEY,
        ]
    };

    let mut sorted_keys: Vec<String> = leading
        .iter()
        .filter(|key| unsorted_slot_keys.contains_key(**key))
        .map(|key| (*key).to_owned())
        .collect();

    sorted_keys.extend(
        unsorted_slot_keys
            .keys()
            .filter(|key| !leading.contains(&key.as_str()))
            .cloned(),
    );

    sorted_keys
}

/// The pieces of `Models/Spt/Bots/ModToSpawnRequest.cs` the weapon path fills in. Every member is a
/// borrow: the C# record holds references to the very objects `GenerateModsForWeapon` goes on to
/// mutate, and nothing here writes through them.
struct ModToSpawnRequest<'a> {
    /// Slot mod will fit into.
    mod_slot: &'a str,
    /// Will generate a randomised mod pool if true.
    is_randomisable_slot: bool,
    randomisation_settings: Option<&'a RandomisationDetails>,
    bot_weapon_sight_whitelist: Option<&'a IndexMap<String, Vec<String>>>,
    /// Blacklist to prevent mods from being picked.
    bot_equip_blacklist: &'a EquipmentFilterDetails,
    /// Pool of items to pick from.
    item_mod_pool: &'a IndexMap<String, IndexSet<String>>,
    /// The weapon as it stands, ready for mods to be added.
    weapon: &'a [Item],
    /// Ammo tpl to use if slot requires a cartridge to be added (e.g. mod_magazine).
    ammo_tpl: &'a str,
    /// Tpl of the parent item the mod will go into.
    parent_template: &'a str,
    /// Should mod be spawned/skipped/use default.
    mod_spawn_result: ModSpawn,
    weapon_stats: &'a WeaponStatsWire,
    conflicting_item_tpls: &'a IndexSet<String>,
    bot_data: &'a BotDataWire,
}

/// `BotEquipmentModGenerator.ChooseModToPutIntoSlot` (`:1021-1142`). `None` is the C# `null` return;
/// the tpl it hands back can still be one the items view does not hold, which is the C#
/// `GetItem` miss the caller reports through [`is_mod_valid_for_slot`].
///
/// # Errors
///
/// Where the C# throws: an empty weapon list (`:1025` `First()`), and a mod slot the parent template
/// does not carry once the pool for it is non-empty (`:1099`/`:1204` dereference `parentSlot`).
fn choose_mod_to_put_into_slot(
    ctx: &mut BotContext,
    request: &ModToSpawnRequest,
) -> Result<Option<String>, LootError> {
    let items = ctx.items;

    // Slot mod will fill
    let parent_slot = get_item(items, request.parent_template)
        .and_then(|template| template.slots.as_deref())
        .unwrap_or_default()
        .iter()
        .find(|slot| slot.name.as_deref() == Some(request.mod_slot));
    let Some(weapon_root) = request.weapon.first() else {
        return Err(LootError::new(
            "Sequence contains no elements: the weapon has no root item",
        ));
    };
    let weapon_tpl = weapon_root.template.clone();

    // It's ammo, use predefined ammo parameter
    if CARTRIDGE_HOLDER_SLOTS.contains(&request.mod_slot) && request.mod_slot != "mod_magazine" {
        return Ok(Some(request.ammo_tpl.to_owned()));
    }

    // Ensure there's a pool of mods to pick from. A `null` pool for a *required* slot survives the
    // guard below and is dereferenced at `:1052`/`:1060`/`:1204`; it is an empty pool here instead.
    // Only `ItemModPool.GetValueOrDefault` can return null and the slot keys come from that very
    // map, so no live pool reaches it.
    let mut mod_pool = get_mod_pool_for_slot(ctx, request, &weapon_tpl)?.unwrap_or_default();
    if mod_pool.is_empty() && !parent_slot.is_some_and(|slot| slot.required.unwrap_or(false)) {
        // Nothing in mod pool + item not required
        let parent_name = get_item(items, request.parent_template)
            .and_then(|template| template.name.clone())
            .unwrap_or_default();
        let mod_slot = request.mod_slot;
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "Mod pool for optional slot: {mod_slot} on item: {parent_name} was empty, skipping mod"
            ),
        ));

        return Ok(None);
    }

    // Filter out non-whitelisted scopes, use the full mod pool if filtered pool would have no
    // elements
    if request.mod_slot.contains("mod_scope")
        && let Some(whitelist) = request.bot_weapon_sight_whitelist
        // scope pool has more than one scope
        && mod_pool.len() > 1
    {
        mod_pool = filter_sights_by_weapon_type(ctx, weapon_root, &mod_pool, whitelist);
    }

    if request.mod_slot == "mod_gas_block" {
        let low_profile = ctx.low_profile_gas_block_tpls;
        if request.weapon_stats.has_optic.unwrap_or(false) && mod_pool.len() > 1 {
            // Attempt to limit modpool to low profile gas blocks when weapon has an optic
            let only_low_profile: IndexSet<String> = mod_pool
                .iter()
                .filter(|tpl| low_profile.contains(*tpl))
                .cloned()
                .collect();
            if !only_low_profile.is_empty() {
                mod_pool = only_low_profile;
            }
        } else if request.weapon_stats.has_rear_iron_sight.unwrap_or(false) && mod_pool.len() > 1 {
            // Attempt to limit modpool to high profile gas blocks when weapon has rear iron sight +
            // no front iron sight
            let only_high_profile: IndexSet<String> = mod_pool
                .iter()
                .filter(|tpl| !low_profile.contains(*tpl))
                .cloned()
                .collect();
            if !only_high_profile.is_empty() {
                mod_pool = only_high_profile;
            }
        }
    }

    // Check if weapon has min magazine size limit
    if request.mod_slot == "mod_magazine"
        && request.is_randomisable_slot
        && request
            .randomisation_settings
            .is_some_and(|settings| settings.minimum_magazine_size.is_some())
    {
        mod_pool = get_filtered_magazine_pool_by_capacity(ctx, request, &weapon_tpl, &mod_pool);
    }

    // Pick random mod that's compatible
    let Some(parent_slot) = parent_slot else {
        return Err(LootError::new(format!(
            "Object reference not set to an instance of an object: slot: {} is not on item: {}",
            request.mod_slot, request.parent_template
        )));
    };
    let mut chosen_mod_result = get_compatible_weapon_mod_tpl_for_slot_from_pool(
        ctx,
        request,
        &mod_pool,
        parent_slot,
        request.mod_spawn_result,
        request.weapon,
        request.mod_slot,
    );
    let parent_slot_required = parent_slot.required.unwrap_or(false);
    if chosen_mod_result.slot_blocked.unwrap_or(false) && !parent_slot_required
    // Don't bother trying to fit mod, slot is completely blocked
    {
        return Ok(None);
    }

    // Log if mod chosen was incompatible
    if chosen_mod_result.incompatible.unwrap_or(false) && !parent_slot_required {
        let parent_slot_name = parent_slot.name.clone().unwrap_or_default();
        let mod_slot = request.mod_slot;
        let reason = chosen_mod_result.reason.clone().unwrap_or_default();
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "Unable to find compatible mod of type: {parent_slot_name}, in slot: {mod_slot} reason: {reason}"
            ),
        ));
    }

    // Get random mod to attach from items db for required slots if none found above
    if !chosen_mod_result.found.unwrap_or(false) && parent_slot_required {
        chosen_mod_result.chosen_template = get_random_mod_tpl_from_item_db(
            ctx,
            MONGO_ID_EMPTY,
            parent_slot.filter.as_deref().unwrap_or_default(),
            request.mod_slot,
            request.weapon,
        );
        chosen_mod_result.found = Some(true);
    }

    // Compatible item not found + not required
    if !chosen_mod_result.found.unwrap_or(false) && !parent_slot_required {
        return Ok(None);
    }

    if !chosen_mod_result.found.unwrap_or(false) {
        if parent_slot_required {
            let mod_slot = request.mod_slot;
            let parent_name = get_item(items, request.parent_template)
                .and_then(|template| template.name.clone())
                .unwrap_or_default();
            let parent_tpl = request.parent_template;
            ctx.diagnostics.push(diagnostic(
                WARNING,
                format!(
                    "Required slot unable to be filled, {mod_slot} on {parent_name} {parent_tpl} for weapon: {weapon_tpl}"
                ),
            ));
        }

        return Ok(None);
    }

    Ok(chosen_mod_result.chosen_template)
}

/// `BotEquipmentModGenerator.GetFilteredMagazinePoolByCapacity` (`:1150-1169`).
fn get_filtered_magazine_pool_by_capacity(
    ctx: &mut BotContext,
    request: &ModToSpawnRequest,
    weapon_tpl: &str,
    mod_pool: &IndexSet<String>,
) -> IndexSet<String> {
    let items = ctx.items;

    // A weapon with no entry takes C#'s `TryGetValue` default of 0, which no magazine is under.
    let min_mag_size_from_settings = request
        .randomisation_settings
        .and_then(|settings| settings.minimum_magazine_size.as_ref())
        .and_then(|sizes| sizes.get(weapon_tpl))
        .copied()
        .unwrap_or(0.0);

    let desired_magazine_tpls: IndexSet<String> = mod_pool
        .iter()
        .filter(|mag_tpl| {
            get_item(items, mag_tpl)
                .and_then(|magazine| magazine.cartridges_max_count)
                .is_some_and(|max_count| max_count >= min_mag_size_from_settings)
        })
        .cloned()
        .collect();

    if desired_magazine_tpls.is_empty() {
        ctx.diagnostics.push(diagnostic(
            WARNING,
            format!("Magazine size filter for: {weapon_tpl} was too strict, ignoring filter"),
        ));

        return mod_pool.clone();
    }

    desired_magazine_tpls
}

/// `BotEquipmentModGenerator.GetCompatibleWeaponModTplForSlotFromPool` (`:1182-1216`).
fn get_compatible_weapon_mod_tpl_for_slot_from_pool(
    ctx: &mut BotContext,
    request: &ModToSpawnRequest,
    mod_pool: &IndexSet<String>,
    parent_slot: &SlotView,
    choice_type_enum: ModSpawn,
    weapon: &[Item],
    mod_slot_name: &str,
) -> ChooseRandomCompatibleModResult {
    // Filter out incompatible mods from pool
    let mut pre_filtered_mod_pool = get_filtered_mod_pool(mod_pool, request.conflicting_item_tpls);
    if pre_filtered_mod_pool.is_empty() {
        let choice = choice_type_enum.name();
        let pool_size = mod_pool.len();

        return ChooseRandomCompatibleModResult {
            incompatible: Some(true),
            found: Some(false),
            reason: Some(format!(
                "Unable to add mod to {choice} slot: {mod_slot_name}. All: {pool_size} had conflicts"
            )),
            ..Default::default()
        };
    }

    // Filter modpool to only items that appear in parents allowed list
    let parent_filter = parent_slot.filter.as_deref().unwrap_or_default();
    pre_filtered_mod_pool.retain(|tpl| parent_filter.contains(tpl));
    if pre_filtered_mod_pool.is_empty() {
        return ChooseRandomCompatibleModResult {
            incompatible: Some(true),
            found: Some(false),
            reason: Some("No mods found in parents allowed list".to_owned()),
            ..Default::default()
        };
    }

    get_compatible_mod_from_pool(ctx, &pre_filtered_mod_pool, choice_type_enum, weapon)
}

/// `BotEquipmentModGenerator.GetCompatibleModFromPool` (`:1224-1308`).
///
/// `:1281`'s `SlotBlocked = true` is commented out in the C# on purpose ("Later in code we try to
/// find replacement, but only when slotBlocked is not true"), so nothing here ever sets it either —
/// only `IsItemIncompatibleWithCurrentItems` does.
fn get_compatible_mod_from_pool(
    ctx: &mut BotContext,
    mod_pool: &IndexSet<String>,
    mod_spawn_type: ModSpawn,
    weapon: &[Item],
) -> ChooseRandomCompatibleModResult {
    let items = ctx.items;

    // Create exhaustable pool to pick mod item from
    let mut exhaustable_mod_pool = ExhaustableArray::new(mod_pool.iter().cloned().collect());

    // Create default response if no compatible item is found below
    let mut chosen_mod_result = ChooseRandomCompatibleModResult {
        incompatible: Some(true),
        found: Some(false),
        reason: Some("unknown".to_owned()),
        ..Default::default()
    };

    // Limit how many attempts to find a compatible mod can occur before giving up
    // 75% of pool size, rounded the way `Math.Round(double)` rounds: half to even
    let max_blocked_attempts = round_half_even(mod_pool.len() as f64 * 0.75);
    let mut blocked_attempt_count = 0.0;
    while let Some(chosen_tpl) = exhaustable_mod_pool.get_random_value() {
        // Not valid item, try again
        // parity: the second `continue` at `:1254-1258`, for an item found in the db whose
        // `Properties` is null, has no analog — a flattened `ItemView` row *is* the `_props`, so
        // "present but propless" is not expressible, and `slots.is_none()` would be a worse
        // divergence (it would skip the slotless mods C# accepts here). Unreachable in practice:
        // every tpl in a slot filter is a real mod template with `_props`. A `hasProperties` flag
        // on `ItemView` would make the guard portable if that ever stops holding — note that it
        // consumes another draw when it fires.
        let Some(picked_item_details) = get_item(items, &chosen_tpl) else {
            continue;
        };

        // Success - Default wanted + only 1 item in pool
        if mod_spawn_type == ModSpawn::DefaultMod && mod_pool.len() == 1 {
            chosen_mod_result.found = Some(true);
            chosen_mod_result.incompatible = Some(false);
            chosen_mod_result.chosen_template = Some(chosen_tpl);

            break;
        }

        // Check if existing weapon mods are incompatible with chosen item
        let existing_item_blocking_choice = weapon.iter().any(|item| {
            picked_item_details
                .conflicting_items
                .as_ref()
                .is_some_and(|conflicting| conflicting.contains(&item.template))
        });
        if existing_item_blocking_choice {
            // Give max of x attempts of picking a mod if blocked by another
            // OR Blocked and mod pool only had 1 item
            if blocked_attempt_count > max_blocked_attempts || mod_pool.len() == 1 {
                #[allow(
                    unused_assignments,
                    reason = "`:1280` resets the counter on the way out of the loop; dead in both languages, kept line for line"
                )]
                {
                    blocked_attempt_count = 0.0; // reset
                }
                //chosen_mod_result.slot_blocked = Some(true); // see the doc comment
                chosen_mod_result.reason = Some("Blocked".to_owned());

                break;
            }

            blocked_attempt_count += 1.0;
            // Not compatible - Try again
            continue;
        }

        // Edge case - Some mod combos will never work, make sure this isn't the case
        if weapon_mod_combo_is_incompatible(weapon, &chosen_tpl) {
            chosen_mod_result.reason = Some(format!(
                "Chosen weapon mod: {chosen_tpl} can never be compatible with existing weapon mods"
            ));

            break;
        }

        // Success
        chosen_mod_result.found = Some(true);
        chosen_mod_result.incompatible = Some(false);
        chosen_mod_result.chosen_template = Some(chosen_tpl);

        break;
    }

    chosen_mod_result
}

/// `BotEquipmentModGenerator.GetFilteredModPool` (`:1321-1324`).
fn get_filtered_mod_pool(
    mod_pool: &IndexSet<String>,
    tpl_blacklist: &IndexSet<String>,
) -> IndexSet<String> {
    mod_pool
        .iter()
        .filter(|tpl| !tpl_blacklist.contains(*tpl))
        .cloned()
        .collect()
}

/// `BotEquipmentModGenerator.GetModPoolForSlot` (`:1335-1350`). `None` is the C# `null` an item mod
/// pool without the slot returns.
///
/// # Errors
///
/// From [`get_mod_pool_for_default_slot`].
fn get_mod_pool_for_slot(
    ctx: &mut BotContext,
    request: &ModToSpawnRequest,
    weapon_tpl: &str,
) -> Result<Option<IndexSet<String>>, LootError> {
    // Mod is flagged as being default only, try and find it in globals
    if request.mod_spawn_result == ModSpawn::DefaultMod {
        return get_mod_pool_for_default_slot(ctx, request, weapon_tpl).map(Some);
    }

    if request.is_randomisable_slot {
        return Ok(Some(get_dynamic_mod_pool(
            ctx,
            request.parent_template,
            request.mod_slot,
            request.bot_equip_blacklist,
        )));
    }

    // Required mod is not default or randomisable, use existing pool
    Ok(request.item_mod_pool.get(request.mod_slot).cloned())
}

/// `BotEquipmentModGenerator.GetModPoolForDefaultSlot` (`:1358-1441`).
///
/// # Errors
///
/// Where the C# throws: the four `request.ItemModPool[request.ModSlot]` indexer reads, for a slot
/// the pool does not hold.
fn get_mod_pool_for_default_slot(
    ctx: &mut BotContext,
    request: &ModToSpawnRequest,
    weapon_tpl: &str,
) -> Result<IndexSet<String>, LootError> {
    let items = ctx.items;
    let weapon_name = get_item(items, weapon_tpl)
        .and_then(|weapon| weapon.name.clone())
        .unwrap_or_default();
    let existing_pool = || {
        request
            .item_mod_pool
            .get(request.mod_slot)
            .cloned()
            .ok_or_else(|| {
                LootError::new(format!(
                    "The given key was not present in the dictionary: {} in the item mod pool",
                    request.mod_slot
                ))
            })
    };

    let Some(matching_mod_from_preset) = get_matching_mod_from_preset(ctx, request, weapon_tpl)
    else {
        let pool = existing_pool()?;
        if pool.len() > 1 {
            let role = &request.bot_data.role;
            let mod_slot = request.mod_slot;
            ctx.diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "{role} No default: {mod_slot} mod found for: {weapon_name}, using existing pool"
                ),
            ));
        }

        // Couldn't find default in globals, use existing mod pool data
        return Ok(pool);
    };
    let matching_mod_from_preset = matching_mod_from_preset.template.clone();

    // Only filter mods down to single default item if it already exists in existing itemModPool, OR
    // the default item has no children
    // Filtering mod pool to item that wasn't already there can have problems;
    // You'd have a mod being picked without any sub-mods in its chain, possibly resulting in missing
    // required mods not being added
    // Mod is in existing mod pool
    if request
        .item_mod_pool
        .get(request.mod_slot)
        .is_some_and(|ids| ids.contains(&matching_mod_from_preset))
    // Found mod on preset + it already exists in mod pool
    {
        return Ok(IndexSet::from([matching_mod_from_preset]));
    }

    // Get an array of items that are allowed in slot from parent item
    // Check the filter of the slot to ensure a chosen mod fits
    let parent_slot_compatible_items = get_item(items, request.parent_template)
        .and_then(|template| template.slots.as_deref())
        .unwrap_or_default()
        .iter()
        .find(|slot| {
            slot.name
                .as_deref()
                .is_some_and(|name| name.to_lowercase() == request.mod_slot.to_lowercase())
        })
        .and_then(|slot| slot.filter.as_deref());

    // Mod isn't in existing pool, only add if it has no children and exists inside parent filter
    // parity: `:1399` calls `.Slots.Any()` on the preset mod's template unguarded, so a template
    // with null `Slots` throws there and reads as "no children" here. Lenient for the same reason as
    // the other four: a panic behind FFI is worse and every real mod template carries a `Slots`
    // array, empty or not.
    if parent_slot_compatible_items.is_some_and(|filter| filter.contains(&matching_mod_from_preset))
        && !get_item(items, &matching_mod_from_preset)
            .and_then(|template| template.slots.as_ref())
            .is_some_and(|slots| !slots.is_empty())
    {
        // Chosen mod has no conflicts + no children + is in parent compat list
        if !request
            .conflicting_item_tpls
            .contains(&matching_mod_from_preset)
        {
            return Ok(IndexSet::from([matching_mod_from_preset]));
        }

        // Above chosen mod had conflicts with existing weapon mods
        let role = &request.bot_data.role;
        let mod_slot = request.mod_slot;
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "{role} Chosen default: {mod_slot} mod found for: {weapon_name} weapon conflicts with item on weapon, cannot use default"
            ),
        ));

        let existing_mod_pool = existing_pool()?;
        if existing_mod_pool.len() == 1 {
            // The only item in pool isn't compatible
            ctx.diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "{role} {mod_slot} Mod pool for: {weapon_name} weapon has only incompatible items, using parent list instead"
                ),
            ));

            // Last ditch, use full pool of items minus conflicts
            let new_list_of_mods_for_slot: IndexSet<String> = parent_slot_compatible_items
                .unwrap_or_default()
                .iter()
                .filter(|tpl| !request.conflicting_item_tpls.contains(*tpl))
                .cloned()
                .collect();
            if !new_list_of_mods_for_slot.is_empty() {
                return Ok(new_list_of_mods_for_slot);
            }
        }

        // Return full mod pool
        return Ok(existing_mod_pool);
    }

    // Tried everything, return mod pool
    existing_pool()
}

/// `BotEquipmentModGenerator.GetMatchingModFromPreset` (`:1449-1455`).
fn get_matching_mod_from_preset<'a>(
    ctx: &'a BotContext,
    request: &ModToSpawnRequest,
    weapon_tpl: &str,
) -> Option<&'a Item> {
    get_matching_preset(ctx, weapon_tpl, request.parent_template)?
        .items
        .iter()
        .find(|item| {
            item.slot_id
                .as_deref()
                .is_some_and(|slot_id| slot_id.eq_ignore_ascii_case(request.mod_slot))
        })
}

/// `BotEquipmentModGenerator.GetMatchingPreset` (`:1463-1481`), against the two preset projections
/// [`BotContext`] carries: `presets_by_id` for the two edge cases, `default_presets_by_tpl` for
/// everything else.
fn get_matching_preset<'a>(
    ctx: &'a BotContext,
    weapon_tpl: &str,
    parent_item_tpl: &str,
) -> Option<&'a PresetView> {
    // Edge case - using MP5SD receiver means default mp5 handguard doesn't fit
    if parent_item_tpl == RECEIVER_HK_MP5SD_9X19_UPPER {
        return ctx.presets_by_id.get(MP5SD_PRESET_ID);
    }

    // Edge case - dvl 500mm is the silenced barrel and has specific muzzle mods
    if parent_item_tpl == BARREL_DVL10_762X51_500MM_SUPPRESSED {
        return ctx.presets_by_id.get(DVL_SILENCED_PRESET_ID);
    }

    ctx.default_presets_by_tpl.get(weapon_tpl)
}

/// `BotEquipmentModGenerator.WeaponModComboIsIncompatible` (`:1489-1498`).
fn weapon_mod_combo_is_incompatible(weapon: &[Item], mod_tpl: &str) -> bool {
    // STM-9 + AR-15 Lone Star Ion Lite handguard
    weapon
        .first()
        .is_some_and(|root| root.template == SMG_SOYUZTM_STM9_GEN2_9X19_CARBINE)
        && mod_tpl == HANDGUARD_AR15_LONE_STAR_ION_LITE
}

/// `BotEquipmentModGenerator.AddCompatibleModsForProvidedMod` (`:1631-1663`).
fn add_compatible_mods_for_provided_mod(
    ctx: &mut BotContext,
    desired_slot_name: &str,
    mod_tpl: &str,
    mod_template: &ItemView,
    mod_pool: &mut IndexMap<String, IndexMap<String, IndexSet<String>>>,
    bot_equip_blacklist: &EquipmentFilterDetails,
) {
    let desired_slot_object = mod_template
        .slots
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|slot| {
            slot.name
                .as_deref()
                .is_some_and(|name| name.contains(desired_slot_name))
        });

    let Some(supported_sub_mods) = desired_slot_object.and_then(|slot| slot.filter.as_deref())
    else {
        return;
    };
    let supported_sub_mods_set: IndexSet<String> = supported_sub_mods.iter().cloned().collect();

    // Filter mods
    let filtered_mods = filter_mods_by_blacklist(
        ctx,
        &supported_sub_mods_set,
        bot_equip_blacklist,
        desired_slot_name,
    );
    let slot_name = desired_slot_object
        .and_then(|slot| slot.name.clone())
        .unwrap_or_default();
    if filtered_mods.is_empty() {
        ctx.diagnostics.push(localised(
            WARNING,
            "bot-unable_to_filter_mods_all_blacklisted",
            serde_json::json!({
                "slotName": slot_name,
                "itemName": mod_template.name.clone().unwrap_or_default(),
            }),
        ));
    }

    mod_pool
        .entry(mod_tpl.to_owned())
        .or_default()
        .insert(slot_name, filtered_mods);
}

/// `BotEquipmentModGenerator.GetDynamicModPool` (`:1672-1692`).
fn get_dynamic_mod_pool(
    ctx: &mut BotContext,
    parent_item_id: &str,
    mod_slot: &str,
    bot_equip_blacklist: &EquipmentFilterDetails,
) -> IndexSet<String> {
    let mods_from_dynamic_pool = get_compatible_mods_for_weapon_slot(ctx, parent_item_id, mod_slot);

    if mods_from_dynamic_pool.is_empty() {
        // Mod pool has no items, don't bother doing any filtering below
        return mods_from_dynamic_pool;
    }

    let filtered_mods =
        filter_mods_by_blacklist(ctx, &mods_from_dynamic_pool, bot_equip_blacklist, mod_slot);
    if !filtered_mods.is_empty() {
        // Filtering left at least 1 item, return it
        return filtered_mods;
    }

    ctx.diagnostics.push(localised(
        WARNING,
        "bot-unable_to_filter_mod_slot_all_blacklisted",
        serde_json::json!(mod_slot),
    ));

    mods_from_dynamic_pool
}

/// `BotEquipmentModGenerator.FillCamora` (`:1735-1814`).
///
/// Two quirks of the C# are load-bearing and ported as they are: **one** ammo tpl is drawn and
/// cloned into **every** `Properties.Slots` entry of the cylinder — non-camora slots included
/// (`:1800`) — and each of those clones gets a fresh id (`:1803`).
fn fill_camora(
    ctx: &mut BotContext,
    items: &mut Vec<Item>,
    mod_pool: &mut IndexMap<String, IndexMap<String, IndexSet<String>>>,
    cylinder_mag_parent_id: &str,
    cylinder_mag_tpl: &str,
    cylinder_mag_template: &ItemView,
) {
    let template_slots = cylinder_mag_template.slots.as_deref().unwrap_or_default();

    let item_mod_pool = match mod_pool.get(cylinder_mag_tpl) {
        Some(pool) => pool.clone(),
        None => {
            ctx.diagnostics.push(localised(
                WARNING,
                "bot-unable_to_fill_camora_slot_mod_pool_empty",
                serde_json::json!({
                    "weaponId": cylinder_mag_tpl,
                    "weaponName": cylinder_mag_template.name.clone().unwrap_or_default(),
                }),
            ));

            // Attempt to generate camora slots for item
            let generated: IndexMap<String, IndexSet<String>> = template_slots
                .iter()
                .filter(|slot| {
                    slot.name
                        .as_deref()
                        .is_some_and(|name| name.starts_with("camora"))
                })
                .map(|camora| {
                    (
                        camora.name.clone().unwrap_or_default(),
                        camora.filter.iter().flatten().cloned().collect(),
                    )
                })
                .collect();
            mod_pool.insert(cylinder_mag_tpl.to_owned(), generated.clone());

            generated
        }
    };

    let mut mod_slot = "cartridges";
    const CAMORA_FIRST_SLOT: &str = "camora_000";
    let mut exhaustible_mod_pool = if let Some(cartridges) = item_mod_pool.get(mod_slot) {
        ExhaustableArray::new(cartridges.iter().cloned().collect())
    } else if item_mod_pool.contains_key(CAMORA_FIRST_SLOT) {
        mod_slot = CAMORA_FIRST_SLOT;
        ExhaustableArray::new(merge_camora_pools(&item_mod_pool).into_iter().collect())
    } else {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-missing_cartridge_slot",
            serde_json::json!(cylinder_mag_tpl),
        ));

        return;
    };

    let mut found = None;
    while let Some(mod_tpl) = exhaustible_mod_pool.get_random_value() {
        if !is_item_incompatible_with_current_items(ctx, items, &mod_tpl, mod_slot)
            .incompatible
            .unwrap_or(false)
        {
            found = Some(mod_tpl);
            break;
        }
    }

    let Some(mod_tpl) = found else {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-no_compatible_camora_ammo_found",
            serde_json::json!(mod_slot),
        ));

        return;
    };

    for slot in template_slots {
        items.push(Item {
            id: mongo_id::generate(),
            template: mod_tpl.clone(),
            parent_id: Some(cylinder_mag_parent_id.to_owned()),
            slot_id: Some(slot.name.clone().unwrap_or_default()),
            ..Default::default()
        });
    }
}

/// `BotEquipmentModGenerator.MergeCamoraPools` (`:1821-1824`).
fn merge_camora_pools(
    camoras_with_shells: &IndexMap<String, IndexSet<String>>,
) -> IndexSet<String> {
    camoras_with_shells
        .values()
        .flatten()
        .cloned()
        .collect::<IndexSet<String>>()
}

/// `BotEquipmentModGenerator.FilterSightsByWeaponType` (`:1835-1915`).
///
/// **Deviation:** a mount's scope slot with no filter is an empty allowed list here, whose `All` is
/// vacuously true — the C# `Filters.FirstOrDefault().Filter` throws on it. No live template hits it.
fn filter_sights_by_weapon_type(
    ctx: &mut BotContext,
    weapon: &Item,
    scopes: &IndexSet<String>,
    bot_weapon_sight_whitelist: &IndexMap<String, Vec<String>>,
) -> IndexSet<String> {
    let items = ctx.items;
    let weapon_details = get_item(items, &weapon.template);
    let weapon_parent = weapon_details
        .and_then(|details| details.parent.clone())
        .unwrap_or_default();
    let weapon_name = weapon_details
        .and_then(|details| details.name.clone())
        .unwrap_or_default();

    // Return original scopes array if whitelist not found
    let Some(whitelisted_sight_types) = bot_weapon_sight_whitelist.get(&weapon_parent) else {
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "Unable to find whitelist for weapon type: {weapon_parent} {weapon_name}, skipping sight filtering"
            ),
        ));

        return scopes.clone();
    };
    let whitelisted_sight_types: Vec<&str> =
        whitelisted_sight_types.iter().map(String::as_str).collect();

    // Filter items that are not directly scopes OR mounts that do not hold the type of scope we
    // allow for this weapon type
    let mut filtered_scopes_and_mods: IndexSet<String> = IndexSet::new();
    for scope_tpl in scopes {
        // Mods is a scope, check base class is allowed
        if is_of_baseclasses(items, scope_tpl, &whitelisted_sight_types) {
            // Add mod to allowed list
            filtered_scopes_and_mods.insert(scope_tpl.clone());

            continue;
        }

        // Edge case, what if item is a mount for a scope and not directly a scope?
        // Check item is mount + has child items
        let item_details = get_item(items, scope_tpl);
        let slots = item_details
            .and_then(|details| details.slots.as_deref())
            .unwrap_or_default();
        if !slots.is_empty() && is_of_baseclass(items, scope_tpl, MOUNT) {
            // Check to see if mount has a scope slot (only include primary slot, ignore the rest
            // like the backup sight slots)
            // Should only find 1 as there's currently no items with a mod_scope AND a mod_scope_000
            let scope_slots = slots.iter().filter(|slot| {
                slot.name
                    .as_deref()
                    .is_some_and(|name| name == "mod_scope" || name == "mod_scope_000")
            });

            // Mods scope slot found must allow ALL whitelisted scope types OR be a mount
            if scope_slots.into_iter().all(|slot| {
                slot.filter.iter().flatten().all(|tpl| {
                    get_item(items, tpl).is_some()
                        && (is_of_baseclasses(items, tpl, &whitelisted_sight_types)
                            || is_of_baseclass(items, tpl, MOUNT))
                })
            })
            // Add mod to allowed list
            {
                filtered_scopes_and_mods.insert(scope_tpl.clone());
            }
        }
    }

    // No mods added to return list after filtering has occurred, send back the original mod list
    if filtered_scopes_and_mods.is_empty() {
        let weapon_tpl = &weapon.template;
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "Scope whitelist too restrictive for: {weapon_tpl} {weapon_name}, skipping filter"
            ),
        ));

        return scopes.clone();
    }

    filtered_scopes_and_mods
}

/// `BotWeaponModLimitService.WeaponModHasReachedLimit`
/// (`Services/Bot/BotWeaponModLimitService.cs:55-136`), ported inline: it is the only method of that
/// service the weapon path calls, and its state is the [`BotModLimitsWire`] counters the request
/// already carries.
fn weapon_mod_has_reached_limit(
    ctx: &mut BotContext,
    bot_role: &str,
    mod_tpl: &str,
    mod_template: &ItemView,
    mod_limits: &mut BotModLimitsWire,
    mods_parent_tpl: &str,
    weapon: &[Item],
) -> bool {
    let items = ctx.items;

    // If mod or mods parent is the NcSTAR MPR45 Backup mount, allow it as it looks cool
    if mods_parent_tpl == MOUNT_NCSTAR_MPR45_BACKUP || mod_tpl == MOUNT_NCSTAR_MPR45_BACKUP {
        // If weapon already has a longer ranged scope on it, allow ncstar to be spawned
        return !weapon.iter().any(|item| {
            is_of_baseclasses(
                items,
                &item.template,
                &[ASSAULT_SCOPE, OPTIC_SCOPE, SPECIAL_SCOPE],
            )
        });
    }

    let scope_base_types: Vec<&str> = mod_limits
        .scope_base_types
        .iter()
        .map(String::as_str)
        .collect();

    // Mods parent is scope and mod is scope, allow it (adds those mini-sights to the tops of sights)
    let mod_is_scope = is_of_baseclasses(items, mod_tpl, &scope_base_types);
    if is_of_baseclasses(items, mods_parent_tpl, &scope_base_types) && mod_is_scope {
        return false;
    }

    // If mod is a scope, Exit early
    if mod_is_scope {
        let scope_max = mod_limits.scope_max;

        return weapon_mod_limit_reached(
            ctx,
            mod_tpl,
            &mut mod_limits.scope,
            scope_max,
            bot_role,
            "scope",
        );
    }

    // Don't allow multiple mounts on a weapon (except when mount is on another mount)
    // Fail when:
    // Over or at scope limit on weapon
    // Item being added is a mount but the parent item is NOT a mount (Allows red dot sub-mounts on
    // mounts)
    // Mount has one slot and it is for a mod_scope
    if scope_limit_reached(mod_limits)
        && has_exactly_one_slot(mod_template)
        && is_of_baseclass(items, mod_tpl, MOUNT)
        && !is_of_baseclass(items, mods_parent_tpl, MOUNT)
        && has_slot_named(mod_template, "mod_scope")
    {
        return true;
    }

    // If mod is a light/laser, return if limit reached
    let flashlight_laser_base_types: Vec<&str> = mod_limits
        .flashlight_laser_base_types
        .iter()
        .map(String::as_str)
        .collect();
    if is_of_baseclasses(items, mod_tpl, &flashlight_laser_base_types) {
        let flashlight_laser_max = mod_limits.flashlight_laser_max;

        return weapon_mod_limit_reached(
            ctx,
            mod_tpl,
            &mut mod_limits.flashlight_laser,
            flashlight_laser_max,
            bot_role,
            "light/laser",
        );
    }

    // Mod is a mount that can hold only flashlights ad limit is reached (don't want to add empty
    // mounts if limit is reached)
    scope_limit_reached(mod_limits)
        && has_exactly_one_slot(mod_template)
        && is_of_baseclass(items, mod_tpl, MOUNT)
        && has_slot_named(mod_template, "mod_flashlight")
}

/// `BotWeaponModLimitService.WeaponModLimitReached` (`:147-170`).
///
/// The C# `currentCount.Count++` is a lifted `int?` increment: a null count stays null, and a null
/// count also fails the `>=` test, so it is never limited and never counted. `Option::map` is that
/// same lift.
fn weapon_mod_limit_reached(
    ctx: &mut BotContext,
    mod_tpl: &str,
    current_count: &mut ItemCountWire,
    max_limit: Option<i32>,
    bot_role: &str,
    mod_type: &str,
) -> bool {
    // No limit, ignore
    if max_limit.is_none_or(|limit| limit == 0) {
        return false;
    }

    // Has mod limit for bot type been reached
    if let (Some(count), Some(limit)) = (current_count.count, max_limit)
        && count >= limit
    {
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "[{bot_role}] {mod_type} limit reached! tried to add: {mod_tpl} but {mod_type} count is: {count}"
            ),
        ));

        return true;
    }

    // Increment mod count limit
    current_count.count = current_count.count.map(|count| count + 1);

    false
}

/// `modLimits.Scope.Count >= modLimits.ScopeMax` on two `int?`s: false unless both are set.
fn scope_limit_reached(mod_limits: &BotModLimitsWire) -> bool {
    matches!(
        (mod_limits.scope.count, mod_limits.scope_max),
        (Some(count), Some(max)) if count >= max
    )
}

/// `modTemplate.Properties?.Slots?.Count() == 1` — false when either is null.
fn has_exactly_one_slot(template: &ItemView) -> bool {
    template
        .slots
        .as_ref()
        .is_some_and(|slots| slots.len() == 1)
}

/// `template.Properties.Slots.Any(slot => slot.Name == name)`.
fn has_slot_named(template: &ItemView, name: &str) -> bool {
    template
        .slots
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|slot| slot.name.as_deref() == Some(name))
}

/// `TemplateItemExtensions.HasNoSlotsCartridgesOrChambers` (`:67-78`). A tpl the items view does not
/// hold takes the `Properties is null` arm, where the C# would have thrown one line earlier.
fn has_no_slots_cartridges_or_chambers(template: Option<&ItemView>) -> bool {
    let Some(template) = template else {
        return true;
    };

    // The C# precedence is `Slots is null || (Slots empty && Cartridges empty && Chambers empty)`.
    let is_empty =
        |slots: &Option<Vec<SlotView>>| slots.as_ref().is_none_or(|slots| slots.is_empty());

    template.slots.is_none()
        || (is_empty(&template.slots)
            && is_empty(&template.cartridges)
            && is_empty(&template.chambers))
}

/// `BotHelper.GetBotRandomizationDetails` (`Helpers/Bot/BotHelper.cs:74-81`).
fn get_bot_randomization_details(
    bot_level: i32,
    bot_equip_config: &EquipmentFilters,
) -> Option<&RandomisationDetails> {
    bot_equip_config
        .randomisation
        .iter()
        .flatten()
        .find(|details| {
            bot_level >= details.level_range.min && bot_level <= details.level_range.max
        })
}

/// `BotEquipmentModGenerator.ShouldModBeSpawned` (`:989-1014`).
fn should_mod_be_spawned(
    item_slot: &SlotView,
    mod_slot_name: &str,
    mod_spawn_chances: &IndexMap<String, f64>,
    bot_equip_config: &EquipmentFilters,
) -> ModSpawn {
    let slot_required = item_slot.required;
    if CARTRIDGE_HOLDER_SLOTS.contains(&mod_slot_name)
    // Always force mags/cartridges in weapon to spawn
    {
        return ModSpawn::Spawn;
    }

    let spawn_mod = roll_chance(
        mod_spawn_chances
            .get(&mod_slot_name.to_lowercase())
            .copied()
            .unwrap_or_default(),
    );
    if !spawn_mod
        && (slot_required.unwrap_or(false)
            || bot_equip_config
                .weapon_slot_ids_to_make_required
                .as_ref()
                .is_some_and(|slots| slots.contains(mod_slot_name)))
    // Edge case: Mod is required but spawn chance roll failed, choose default mod spawn for slot
    {
        return ModSpawn::DefaultMod;
    }

    if spawn_mod {
        ModSpawn::Spawn
    } else {
        ModSpawn::Skip
    }
}

/// `BotEquipmentModGenerator.GetModItemSlotFromDbTemplate` (`:959-979`).
///
/// The `patron_in_weapon*` and `cartridges` arms read `Properties.Chambers`/`Properties.Cartridges`,
/// which [`ItemView`] carries as slot lists for this method's sake — note the chamber arm matches by
/// `Contains`, not equality, so `patron_in_weapon` also finds `patron_in_weapon_000`.
fn get_mod_item_slot_from_db_template<'a>(
    mod_slot: &str,
    parent_template: Option<&'a ItemView>,
) -> Option<&'a SlotView> {
    let mod_slot_lower = mod_slot.to_lowercase();
    let parent_template = parent_template?;

    match mod_slot_lower.as_str() {
        "patron_in_weapon" | "patron_in_weapon_000" | "patron_in_weapon_001" => {
            parent_template.chambers.as_ref()?.iter().find(|chamber| {
                chamber
                    .name
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase().contains(&mod_slot_lower))
            })
        }
        "cartridges" => parent_template.cartridges.as_ref()?.iter().find(|slot| {
            slot.name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&mod_slot_lower))
        }),
        _ => parent_template.slots.as_ref()?.iter().find(|slot| {
            slot.name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&mod_slot_lower))
        }),
    }
}

/// `BotEquipmentModGenerator.FilterModsByBlacklist` (`:1701-1723`).
fn filter_mods_by_blacklist(
    ctx: &BotContext,
    mod_tpl_pool: &IndexSet<String>,
    bot_equip_blacklist: &EquipmentFilterDetails,
    mod_slot: &str,
) -> IndexSet<String> {
    if mod_tpl_pool.is_empty() {
        // Mod pool has no items, don't bother doing any filtering below
        return mod_tpl_pool.clone();
    }

    // Get item blacklist and mod equipment blacklist as one Set
    let slot_blacklist = bot_equip_blacklist
        .equipment
        .as_ref()
        .and_then(|equipment| equipment.get(mod_slot));

    mod_tpl_pool
        .iter()
        .filter(|tpl| {
            !ctx.item_blacklist.contains(*tpl)
                && !slot_blacklist.is_some_and(|blacklist| blacklist.contains(*tpl))
        })
        .cloned()
        .collect()
}

/// `BotEquipmentModGenerator.GetRandomModTplFromItemDb` (`:1540-1559`), taking the slot's already
/// resolved `Properties.Filters.First().Filter`.
///
/// **Deviation:** a slot without a filter is an empty pool here, where the C# `Filters.First()`
/// throws. Same call as `mod_pool_service::get_required_mods_for_weapon_slot` makes, for the same
/// reason: a panic behind the FFI boundary is worse and no live template hits it.
fn get_random_mod_tpl_from_item_db(
    ctx: &mut BotContext,
    fallback_mod_tpl: &str,
    allowed_items: &[String],
    mod_slot: &str,
    items: &[Item],
) -> Option<String> {
    // Find mod item that fits slot from sorted mod array
    let mut exhaustable_mod_pool = ExhaustableArray::new(allowed_items.to_vec());
    let mut tmp_mod_tpl = fallback_mod_tpl.to_owned();
    while exhaustable_mod_pool.has_values() {
        if let Some(candidate) = exhaustable_mod_pool.get_random_value() {
            tmp_mod_tpl = candidate;
            if !is_item_incompatible_with_current_items(ctx, items, &tmp_mod_tpl, mod_slot)
                .incompatible
                .unwrap_or(false)
            {
                return Some(tmp_mod_tpl);
            }
        }
    }

    // No mod found, return fallback
    Some(tmp_mod_tpl)
}

/// `BotEquipmentModGenerator.IsModValidForSlot` (`:1570-1622`).
///
/// **Deviation:** the C# distinguishes `GetItem`'s two failure shapes — a null template (error log)
/// and a `false` found flag (warning log, required slots only). `get_item` collapses both into
/// `None`, which lands on the first, the one a missing tpl actually produces.
fn is_mod_valid_for_slot(
    ctx: &mut BotContext,
    mod_found_in_db: bool,
    mod_tpl: &str,
    mod_slot: &str,
    parent_tpl: &str,
) -> bool {
    // Mod lacks db template object
    if !mod_found_in_db {
        ctx.diagnostics.push(Diagnostic {
            level: ERROR.to_owned(),
            locale_key: Some("bot-no_item_template_found_when_adding_mod".to_owned()),
            args: Some(serde_json::json!({
                "modId": mod_tpl,
                "modSlot": mod_slot,
            })),
            message: None,
        });
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!("Item -> {parent_tpl}; Slot -> {mod_slot}"),
        ));

        return false;
    }

    // Mod was found in db
    true
}

/// `BotEquipmentModGenerator.CreateModItem` (`:1510-1520`).
fn create_mod_item(
    ctx: &BotContext,
    mod_id: &str,
    mod_tpl: &str,
    parent_id: &str,
    mod_slot: &str,
    bot_role: &str,
) -> Result<Item, LootError> {
    let mod_template = get_item(ctx.items, mod_tpl).ok_or_else(|| {
        LootError::new(format!(
            "Object reference not set to an instance of an object: mod tpl: {mod_tpl} is not in the items view"
        ))
    })?;

    Ok(Item {
        id: mod_id.to_owned(),
        template: mod_tpl.to_owned(),
        parent_id: Some(parent_id.to_owned()),
        slot_id: Some(mod_slot.to_owned()),
        upd: generate_extra_properties_for_item(ctx, mod_template, Some(bot_role), false)?,
        ..Default::default()
    })
}

/// `TemplateItemExtensions.GetDefaultPlateTpl` (`Extensions/TemplateItemExtensions.cs:53-60`).
fn get_default_plate_tpl(armor_item: &ItemView, mod_slot: &str) -> Option<String> {
    armor_item
        .slots
        .as_ref()?
        .iter()
        .find(|slot| {
            slot.name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(mod_slot))
        })?
        .plate
        .clone()
}

/// `ItemHelper.IsRemovablePlateSlot` (`Helpers/Items/ItemHelper.cs:1670-1673`). Both call sites
/// lowercase the name before the call, as the C# helper itself does.
fn is_removable_plate_slot(slot_name: &str) -> bool {
    REMOVABLE_PLATE_SLOT_IDS.contains(&slot_name.to_lowercase().as_str())
}

/// One re-run of the `:366`/`:399` `Where`, which the lazy C# sequence performs afresh each time.
fn plates_of_class<'a>(plates_from_db: &[(&'a str, i32)], armor_class: i32) -> Vec<&'a str> {
    plates_from_db
        .iter()
        .filter(|(_, class)| *class == armor_class)
        .map(|(tpl, _)| *tpl)
        .collect()
}

fn to_tpl_set(plates: &[&str]) -> IndexSet<String> {
    plates.iter().map(|tpl| (*tpl).to_owned()).collect()
}

/// `int.Parse` on a plate level held as a string by the weights map.
fn parse_plate_level(level: &str) -> Result<i32, LootError> {
    level.parse::<i32>().map_err(|_| {
        LootError::new(format!(
            "Input string was not in a correct format: armor plate level: {level}"
        ))
    })
}

/// A plain interpolated log line, the shape most of the ported call sites use.
fn diagnostic(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

/// A `ServerLocalisationService.GetText` line: the key plus the arguments the C# passes with it (a
/// bare value for the `%s` keys, an object whose members match the C# anonymous type otherwise) —
/// the same shape `loot::location_loot_generator` uses.
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
    use std::collections::HashSet;

    use serde_json::json;

    use super::*;

    use crate::bot::durability_limits_helper::BotDurability;
    use crate::bot::models::RandomisedResourceDetails;
    use crate::loot::models::PresetView;
    use crate::loot::random_util::{TestSeedGuard, get_int};

    const SEED: u64 = 42;
    /// A seed whose weighted plate draw lands above the clamp, so the clamp has work to do.
    const CLAMP_SEED: u64 = 1;

    const CARRIER: &str = "aaaaaaaaaaaaaaaaaaaaaaa1";
    const REQUIRED_ARMOR: &str = "aaaaaaaaaaaaaaaaaaaaaaa2";
    const PRESET_ARMOR: &str = "aaaaaaaaaaaaaaaaaaaaaaa3";
    const PLATE_C2: &str = "bbbbbbbbbbbbbbbbbbbbbbb2";
    const PLATE_C3: &str = "bbbbbbbbbbbbbbbbbbbbbbb3";
    const PLATE_C4: &str = "bbbbbbbbbbbbbbbbbbbbbbb4";
    const PLATE_C5: &str = "bbbbbbbbbbbbbbbbbbbbbbb5";
    const PLATE_C6: &str = "bbbbbbbbbbbbbbbbbbbbbbb6";
    /// Class 1, so no weighted draw ever selects it — it is only reachable as a slot default.
    const DEFAULT_PLATE: &str = "bbbbbbbbbbbbbbbbbbbbbbb1";
    const PRESET_PLATE: &str = "bbbbbbbbbbbbbbbbbbbbbbb7";
    const HELMET: &str = "ccccccccccccccccccccccc1";
    const NVG: &str = "ccccccccccccccccccccccc2";
    const NVG_MOUNT: &str = "ccccccccccccccccccccccc3";
    const ROOT_ID: &str = "ffffffffffffffffffffffff";

    /// Unread by either fixture — no template here has a durability — but `BotContext` carries it.
    fn durability_config() -> BotDurability {
        serde_json::from_value(json!({
            "default": {
                "armor": {"maxDelta": 0, "minDelta": 0, "minLimitPercent": 0},
                "weapon": {"lowestMax": 0, "highestMax": 0, "maxDelta": 0,
                           "minDelta": 0, "minLimitPercent": 0}
            },
            "botDurabilities": {},
            "pmc": {
                "armor": {"lowestMaxPercent": 0, "highestMaxPercent": 0, "maxDelta": 0,
                          "minDelta": 0, "minLimitPercent": 0},
                "weapon": {"lowestMax": 0, "highestMax": 0, "maxDelta": 0,
                           "minDelta": 0, "minLimitPercent": 0}
            }
        }))
        .unwrap()
    }

    struct Fixture {
        items: IndexMap<String, ItemView>,
        bosses: Vec<String>,
        durability: BotDurability,
        equipment: IndexMap<String, EquipmentFilters>,
        randomization: IndexMap<String, RandomisedResourceDetails>,
        item_blacklist: HashSet<String>,
        default_presets_by_tpl: IndexMap<String, PresetView>,
    }

    impl Fixture {
        /// A plate carrier with three plate slots (plates of class 2-6, a class-1 slot default on
        /// the front), a helmet whose `mod_nvg` chain is two levels deep, an armor whose required
        /// slot has an empty pool, and an armor whose default preset carries a plate.
        fn new() -> Self {
            let plate_filter = json!([PLATE_C2, PLATE_C3, PLATE_C4, PLATE_C5, PLATE_C6]);

            Self {
                items: serde_json::from_value(json!({
                    CARRIER: {"name": "plate_carrier", "slots": [
                        {"name": "front_plate", "required": true, "filter": plate_filter,
                         "plate": DEFAULT_PLATE},
                        {"name": "back_plate", "required": false, "filter": plate_filter},
                        {"name": "left_side_plate", "required": false, "filter": [PLATE_C4]},
                    ]},
                    REQUIRED_ARMOR: {"name": "required_armor", "slots": [
                        {"name": "front_plate", "required": true, "filter": [PLATE_C2]},
                    ]},
                    PRESET_ARMOR: {"name": "preset_armor", "slots": [
                        {"name": "front_plate", "required": false, "filter": [PLATE_C2]},
                    ]},
                    PLATE_C2: {"name": "plate_c2", "armorClass": 2},
                    PLATE_C3: {"name": "plate_c3", "armorClass": 3},
                    PLATE_C4: {"name": "plate_c4", "armorClass": 4},
                    PLATE_C5: {"name": "plate_c5", "armorClass": 5},
                    PLATE_C6: {"name": "plate_c6", "armorClass": 6},
                    DEFAULT_PLATE: {"name": "plate_default", "armorClass": 1},
                    PRESET_PLATE: {"name": "plate_preset", "armorClass": 3},
                    HELMET: {"name": "helmet", "slots": [
                        {"name": "mod_nvg", "required": false, "filter": [NVG]},
                    ]},
                    NVG: {"name": "nvg", "slots": [
                        {"name": "mod_nvg_mount", "required": false, "filter": [NVG_MOUNT]},
                    ]},
                    NVG_MOUNT: {"name": "nvg_mount"},
                }))
                .unwrap(),
                bosses: Vec::new(),
                durability: durability_config(),
                equipment: IndexMap::new(),
                randomization: IndexMap::new(),
                item_blacklist: HashSet::new(),
                default_presets_by_tpl: serde_json::from_value(json!({
                    PRESET_ARMOR: {"items": [
                        {"_id": "eeeeeeeeeeeeeeeeeeeeeee1", "_tpl": PRESET_ARMOR},
                        {"_id": "eeeeeeeeeeeeeeeeeeeeeee2", "_tpl": PRESET_PLATE,
                         "parentId": "eeeeeeeeeeeeeeeeeeeeeee1", "slotId": "front_plate"},
                    ]},
                }))
                .unwrap(),
            }
        }

        fn ctx(&self) -> BotContext<'_> {
            BotContext {
                items: &self.items,
                bosses: &self.bosses,
                durability: &self.durability,
                equipment: &self.equipment,
                loot_item_resource_randomization: &self.randomization,
                is_night_time: false,
                item_blacklist: &self.item_blacklist,
                default_presets_by_tpl: &self.default_presets_by_tpl,
                presets_by_id: &crate::bot::NO_PRESETS,
                equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                low_profile_gas_block_tpls: &crate::bot::NO_BLACKLIST,
                item_presets: &crate::bot::NO_PRESETS,
                weapon_has_enhancement_chance_percent: 0.0,
                repair_kit_weapon: &crate::bot::NO_BUFFS,
                secure_container_ammo_stack_count: 0,
                diagnostics: Vec::new(),
            }
        }
    }

    /// Insertion order puts `back_plate` first, so a run that starts with the front plate proves the
    /// `:115-130` ordering; `left_side_plate` last proves the residual keeps map order.
    fn settings() -> GenerateEquipmentPropertiesWire {
        serde_json::from_value(json!({
            "modPool": {
                CARRIER: {
                    "back_plate": [PLATE_C2, PLATE_C3, PLATE_C4, PLATE_C5, PLATE_C6],
                    "front_plate": [PLATE_C2, PLATE_C3, PLATE_C4, PLATE_C5, PLATE_C6],
                    "left_side_plate": [PLATE_C4],
                },
                REQUIRED_ARMOR: {"front_plate": []},
                HELMET: {"mod_nvg": [NVG]},
                NVG: {"mod_nvg_mount": [NVG_MOUNT]},
            },
            "spawnChances": {"equipmentMods": {
                "front_plate": 100.0,
                "back_plate": 100.0,
                "left_side_plate": 100.0,
                "mod_nvg": 100.0,
                "mod_nvg_mount": 0.0,
            }},
            "botData": {"role": "assault", "level": 20, "equipmentRole": "assault"},
            "botEquipmentConfig": {
                "filterPlatesByLevel": true,
                "skipBackPlateIfFrontPlateMissing": true,
                "limitPlateClassToFrontPlateClass": true,
                "armorPlateWeighting": [{
                    "levelRange": {"min": 1, "max": 100},
                    "values": {
                        "front_plate": {"2": 10.0, "3": 20.0, "4": 30.0, "5": 25.0, "6": 15.0},
                        "back_plate": {"2": 10.0, "3": 20.0, "4": 30.0, "5": 25.0, "6": 15.0},
                        "left_side_plate": {"4": 100.0},
                    },
                }],
            },
        }))
        .unwrap()
    }

    fn root(tpl: &str) -> Vec<Item> {
        vec![Item {
            id: ROOT_ID.to_owned(),
            template: tpl.to_owned(),
            slot_id: Some("ArmorVest".to_owned()),
            ..Default::default()
        }]
    }

    /// `(slotId, tpl, parent)` with generated ids replaced by their index in the list, so a run is
    /// comparable across the fresh `MongoId`s every mod gets.
    fn normalized(equipment: &[Item]) -> Vec<(String, String, String)> {
        let ids: IndexMap<&str, String> = equipment
            .iter()
            .enumerate()
            .map(|(index, item)| (item.id.as_str(), format!("#{index}")))
            .collect();

        equipment
            .iter()
            .map(|item| {
                (
                    item.slot_id.clone().unwrap_or_default(),
                    item.template.clone(),
                    item.parent_id
                        .as_deref()
                        .and_then(|parent| ids.get(parent).cloned())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    fn armor_class(fixture: &Fixture, tpl: &str) -> i32 {
        fixture.items[tpl].armor_class.unwrap()
    }

    #[test]
    fn seeded_carrier_run_is_pinned() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let settings = settings();
        let mut equipment = root(CARRIER);

        let _guard = TestSeedGuard::install(SEED);
        generate_mods_for_equipment(
            &mut ctx,
            &mut equipment,
            ROOT_ID,
            CARRIER,
            &settings,
            &EquipmentFilterDetails::default(),
            false,
        )
        .unwrap();

        assert_eq!(
            normalized(&equipment),
            vec![
                ("ArmorVest".to_owned(), CARRIER.to_owned(), String::new()),
                (
                    "front_plate".to_owned(),
                    PLATE_C5.to_owned(),
                    "#0".to_owned()
                ),
                (
                    "back_plate".to_owned(),
                    PLATE_C5.to_owned(),
                    "#0".to_owned()
                ),
                (
                    "left_side_plate".to_owned(),
                    PLATE_C4.to_owned(),
                    "#0".to_owned()
                ),
            ]
        );
    }

    /// `limitPlateClassToFrontPlateClass` clamps the back plate to the front plate's class, so it can
    /// never come out higher.
    #[test]
    fn the_back_plate_never_outclasses_the_front_plate() {
        let fixture = Fixture::new();
        let settings = settings();

        for seed in 0..64 {
            let mut ctx = fixture.ctx();
            let mut equipment = root(CARRIER);

            let _guard = TestSeedGuard::install(seed);
            generate_mods_for_equipment(
                &mut ctx,
                &mut equipment,
                ROOT_ID,
                CARRIER,
                &settings,
                &EquipmentFilterDetails::default(),
                false,
            )
            .unwrap();

            let class_in = |slot: &str| {
                equipment
                    .iter()
                    .find(|item| item.slot_id.as_deref() == Some(slot))
                    .map(|item| armor_class(&fixture, &item.template))
            };

            assert!(class_in("back_plate").unwrap() <= class_in("front_plate").unwrap());
        }
    }

    /// `:350` draws before `:353` clamps, so the weighted draw is consumed whatever the clamp does
    /// with it.
    #[test]
    fn the_weighted_plate_draw_is_consumed_before_the_clamp_applies() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let settings = settings();
        let pool: IndexSet<String> = settings.mod_pool[CARRIER]["front_plate"].clone();

        let weights = &settings
            .bot_equipment_config
            .armor_plate_weighting
            .as_ref()
            .unwrap()[0]
            .values["front_plate"];
        let baseline = {
            let _guard = TestSeedGuard::install(CLAMP_SEED);
            let drawn = get_weighted_value(weights).unwrap();
            (drawn, get_int(1, 1000))
        };

        let _guard = TestSeedGuard::install(CLAMP_SEED);
        let outcome = filter_plate_mods_for_slot_by_level(
            &mut ctx,
            &settings,
            "front_plate",
            &pool,
            CARRIER,
            Some(2),
        )
        .unwrap();

        assert_eq!(outcome.result, PlateFilterResult::Success);
        // Clamped to 2 whatever was drawn, but the draw still happened: the next value off the
        // stream is the one that followed the weighted draw in the baseline.
        assert_eq!(
            outcome.plate_mod_templates,
            Some(IndexSet::from([PLATE_C2.to_owned()]))
        );
        assert_eq!(get_int(1, 1000), baseline.1);
        assert_ne!(baseline.0, "2", "the clamp has to have something to clamp");
    }

    /// `:386-444`: +1 per attempt, wrapping the *string* to `Min` while the filter keeps testing the
    /// un-wrapped value.
    #[test]
    fn the_plate_class_wraparound_wraps_to_min() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        // A single weight draws nothing and pins the chosen class to one above the pool's max.
        settings
            .bot_equipment_config
            .armor_plate_weighting
            .as_mut()
            .unwrap()[0]
            .values["front_plate"] = IndexMap::from([("6".to_owned(), 1.0)]);
        let pool = IndexSet::from([PLATE_C2.to_owned(), PLATE_C3.to_owned()]);

        let outcome = filter_plate_mods_for_slot_by_level(
            &mut ctx,
            &settings,
            "front_plate",
            &pool,
            CARRIER,
            None,
        )
        .unwrap();

        // Attempt 1: 6 + 1 = 7 > max 3, so the string wraps to min 2 while the filter still asks for
        // 7 and finds nothing. Attempt 2: 2 + 1 = 3, which the pool has.
        assert_eq!(outcome.result, PlateFilterResult::Success);
        assert_eq!(
            outcome.plate_mod_templates,
            Some(IndexSet::from([PLATE_C3.to_owned()]))
        );
    }

    /// Three attempts that find nothing fall through to the slot's own default plate.
    #[test]
    fn three_failed_attempts_fall_back_to_the_slot_default_plate() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        settings
            .bot_equipment_config
            .armor_plate_weighting
            .as_mut()
            .unwrap()[0]
            .values["front_plate"] = IndexMap::from([("6".to_owned(), 1.0)]);
        let pool = IndexSet::from([PLATE_C2.to_owned()]);

        let outcome = filter_plate_mods_for_slot_by_level(
            &mut ctx,
            &settings,
            "front_plate",
            &pool,
            CARRIER,
            None,
        )
        .unwrap();

        assert_eq!(outcome.result, PlateFilterResult::Success);
        assert_eq!(
            outcome.plate_mod_templates,
            Some(IndexSet::from([DEFAULT_PLATE.to_owned()]))
        );
        // `:409-414`, with the wrapped-to-min level string the loop left behind.
        assert_eq!(ctx.diagnostics.len(), 1);
        assert_eq!(ctx.diagnostics[0].level, DEBUG);
        assert_eq!(
            ctx.diagnostics[0].message.as_deref(),
            Some(
                format!(
                    "Plate filter too restrictive for armor: plate_carrier {CARRIER}, unable to find plates of level: 2, using items default plate"
                )
                .as_str()
            )
        );
    }

    /// No slot default either: the default preset's own plate is the last resort.
    #[test]
    fn the_default_preset_plate_is_the_last_resort() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        settings
            .bot_equipment_config
            .armor_plate_weighting
            .as_mut()
            .unwrap()[0]
            .values["front_plate"] = IndexMap::from([("6".to_owned(), 1.0)]);
        let pool = IndexSet::from([PLATE_C2.to_owned()]);

        let outcome = filter_plate_mods_for_slot_by_level(
            &mut ctx,
            &settings,
            "front_plate",
            &pool,
            PRESET_ARMOR,
            None,
        )
        .unwrap();

        assert_eq!(outcome.result, PlateFilterResult::Success);
        assert_eq!(
            outcome.plate_mod_templates,
            Some(IndexSet::from([PRESET_PLATE.to_owned()]))
        );
    }

    /// Neither default: the caller is told to skip the slot.
    #[test]
    fn no_default_plate_and_no_preset_is_no_default_filter() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        settings
            .bot_equipment_config
            .armor_plate_weighting
            .as_mut()
            .unwrap()[0]
            .values["front_plate"] = IndexMap::from([("6".to_owned(), 1.0)]);
        let pool = IndexSet::from([PLATE_C2.to_owned()]);

        let outcome = filter_plate_mods_for_slot_by_level(
            &mut ctx,
            &settings,
            "front_plate",
            &pool,
            REQUIRED_ARMOR,
            None,
        )
        .unwrap();

        assert_eq!(outcome.result, PlateFilterResult::NoDefaultFilter);
        assert_eq!(outcome.plate_mod_templates, None);
    }

    /// A slot with no weights for the bot's level keeps the pool it was handed, and says so.
    #[test]
    fn a_slot_without_weights_returns_the_original_pool() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        settings.bot_data.level = 200;
        let pool = IndexSet::from([PLATE_C2.to_owned()]);

        let outcome = filter_plate_mods_for_slot_by_level(
            &mut ctx,
            &settings,
            "front_plate",
            &pool,
            CARRIER,
            None,
        )
        .unwrap();

        assert_eq!(outcome.result, PlateFilterResult::LacksPlateWeights);
        assert_eq!(outcome.plate_mod_templates, Some(pool));
    }

    #[test]
    fn a_non_plate_slot_is_returned_untouched() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let settings = settings();
        let pool = IndexSet::from([NVG.to_owned()]);

        let outcome = filter_plate_mods_for_slot_by_level(
            &mut ctx, &settings, "mod_nvg", &pool, HELMET, None,
        )
        .unwrap();

        assert_eq!(outcome.result, PlateFilterResult::NotPlateHoldingSlot);
        assert_eq!(outcome.plate_mod_templates, Some(pool));
    }

    /// `:270` dereferences a `MongoId?` that an empty pool never assigned.
    #[test]
    fn an_empty_pool_on_a_required_slot_is_the_intentional_null_deref() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        settings.bot_equipment_config.filter_plates_by_level = Some(false);
        let mut equipment = root(REQUIRED_ARMOR);

        let _guard = TestSeedGuard::install(SEED);
        let error = generate_mods_for_equipment(
            &mut ctx,
            &mut equipment,
            ROOT_ID,
            REQUIRED_ARMOR,
            &settings,
            &EquipmentFilterDetails::default(),
            false,
        )
        .unwrap_err();

        assert!(
            error.message.contains("Nullable object must have a value"),
            "{}",
            error.message
        );
        assert_eq!(equipment.len(), 1, "nothing should have been added");
    }

    /// `mod_nvg_mount` rolls against a 0% chance, but `mod_nvg` forced the spawn at `:180-183`, so
    /// the grandchild lands anyway.
    #[test]
    fn the_nvg_chain_forces_its_grandchildren_to_spawn() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let settings = settings();
        let mut equipment = root(HELMET);

        let _guard = TestSeedGuard::install(SEED);
        generate_mods_for_equipment(
            &mut ctx,
            &mut equipment,
            ROOT_ID,
            HELMET,
            &settings,
            &EquipmentFilterDetails::default(),
            false,
        )
        .unwrap();

        assert_eq!(
            normalized(&equipment),
            vec![
                ("ArmorVest".to_owned(), HELMET.to_owned(), String::new()),
                ("mod_nvg".to_owned(), NVG.to_owned(), "#0".to_owned()),
                (
                    "mod_nvg_mount".to_owned(),
                    NVG_MOUNT.to_owned(),
                    "#1".to_owned()
                ),
            ]
        );
    }

    /// The caller's own `shouldForceSpawn` overrides the roll the same way.
    #[test]
    fn force_spawn_overrides_a_failed_roll() {
        let fixture = Fixture::new();
        let mut settings = settings();
        settings.spawn_chances.equipment_mods["mod_nvg"] = 0.0;

        for (force_spawn, expected) in [(false, 1), (true, 3)] {
            let mut ctx = fixture.ctx();
            let mut equipment = root(HELMET);

            let _guard = TestSeedGuard::install(SEED);
            generate_mods_for_equipment(
                &mut ctx,
                &mut equipment,
                ROOT_ID,
                HELMET,
                &settings,
                &EquipmentFilterDetails::default(),
                force_spawn,
            )
            .unwrap();

            assert_eq!(equipment.len(), expected, "forceSpawn: {force_spawn}");
        }
    }

    /// The blacklist is applied per slot, and a pool it would empty is kept whole (`:190-194`).
    #[test]
    fn the_blacklist_filters_the_pool_but_never_empties_it() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();
        let pool: IndexSet<String> = IndexSet::from([PLATE_C2.to_owned(), PLATE_C3.to_owned()]);
        let blacklist: EquipmentFilterDetails = serde_json::from_value(json!({
            "equipment": {"front_plate": [PLATE_C2]},
        }))
        .unwrap();

        assert_eq!(
            filter_mods_by_blacklist(&ctx, &pool, &blacklist, "front_plate"),
            IndexSet::from([PLATE_C3.to_owned()])
        );
        // Another slot's entry does not apply.
        assert_eq!(
            filter_mods_by_blacklist(&ctx, &pool, &blacklist, "back_plate"),
            pool
        );
    }

    /// `:229-236` names the outcome bare, as C#'s enum `ToString()` does — no quotes.
    #[test]
    fn a_failed_plate_selection_is_reported_and_the_slot_skipped() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        // REQUIRED_ARMOR has neither a slot default plate nor a default preset, and a single weight
        // pins the class above anything its pool holds.
        settings.mod_pool[REQUIRED_ARMOR]["front_plate"] = IndexSet::from([PLATE_C2.to_owned()]);
        settings
            .bot_equipment_config
            .armor_plate_weighting
            .as_mut()
            .unwrap()[0]
            .values["front_plate"] = IndexMap::from([("6".to_owned(), 1.0)]);
        let mut equipment = root(REQUIRED_ARMOR);

        let _guard = TestSeedGuard::install(SEED);
        generate_mods_for_equipment(
            &mut ctx,
            &mut equipment,
            ROOT_ID,
            REQUIRED_ARMOR,
            &settings,
            &EquipmentFilterDetails::default(),
            false,
        )
        .unwrap();

        assert_eq!(equipment.len(), 1, "the slot should have been skipped");
        let reported = ctx
            .diagnostics
            .last()
            .expect("the failed selection is reported");
        assert_eq!(reported.level, DEBUG);
        assert_eq!(
            reported.message.as_deref(),
            Some(
                format!(
                    "Plate slot: front_plate selection for armor: {REQUIRED_ARMOR} failed: NO_DEFAULT_FILTER, skipping"
                )
                .as_str()
            )
        );
    }

    /// A pool with no entry for the parent tpl warns and adds nothing.
    #[test]
    fn a_missing_mod_pool_warns_and_adds_nothing() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let settings = settings();
        let mut equipment = root(PRESET_ARMOR);

        let _guard = TestSeedGuard::install(SEED);
        generate_mods_for_equipment(
            &mut ctx,
            &mut equipment,
            ROOT_ID,
            PRESET_ARMOR,
            &settings,
            &EquipmentFilterDetails::default(),
            false,
        )
        .unwrap();

        assert_eq!(equipment.len(), 1);
        assert_eq!(ctx.diagnostics.len(), 1);
        assert_eq!(ctx.diagnostics[0].level, WARNING);
        assert_eq!(
            ctx.diagnostics[0].message.as_deref(),
            Some(
                format!("bot: assault lacks a mod slot pool for item: {PRESET_ARMOR} preset_armor")
                    .as_str()
            )
        );
    }

    /// A pool key with no matching slot on the parent template is an error and a skip (`:147-164`).
    #[test]
    fn a_pool_key_with_no_slot_on_the_template_is_reported() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut settings = settings();
        settings.mod_pool[HELMET].insert(
            "mod_equipment".to_owned(),
            IndexSet::from([NVG_MOUNT.to_owned()]),
        );
        let mut equipment = root(HELMET);

        let _guard = TestSeedGuard::install(SEED);
        generate_mods_for_equipment(
            &mut ctx,
            &mut equipment,
            ROOT_ID,
            HELMET,
            &settings,
            &EquipmentFilterDetails::default(),
            false,
        )
        .unwrap();

        let reported = ctx
            .diagnostics
            .iter()
            .find(|entry| entry.locale_key.as_deref() == Some("bot-mod_slot_missing_from_item"))
            .expect("the missing slot is reported");
        assert_eq!(reported.level, ERROR);
        assert_eq!(reported.args.as_ref().unwrap()["modSlot"], "mod_equipment");
        assert_eq!(reported.args.as_ref().unwrap()["parentId"], HELMET);
    }

    #[test]
    fn min_max_armor_plate_class_spans_the_pool() {
        assert_eq!(
            get_min_max_armor_plate_class(&[("a", 4), ("b", 2), ("c", 6)]).unwrap(),
            (2, 6)
        );
        assert!(get_min_max_armor_plate_class(&[]).is_err());
    }

    #[test]
    fn ammo_container_slots_spawn_without_drawing() {
        let slot: SlotView = serde_json::from_value(json!({"name": "mod_magazine"})).unwrap();
        let chances = IndexMap::new();
        let config = EquipmentFilters::default();

        let baseline = {
            let _guard = TestSeedGuard::install(SEED);
            get_int(1, 1000)
        };

        let _guard = TestSeedGuard::install(SEED);
        assert_eq!(
            should_mod_be_spawned(&slot, "mod_magazine", &chances, &config),
            ModSpawn::Spawn
        );
        assert_eq!(get_int(1, 1000), baseline, "the ammo slot consumed a draw");
    }

    /// The weapon half (`:503-1916`). Its own fixture: an M4-shaped weapon with a receiver → mount →
    /// scope chain, a stock that takes a sub-stock, a barrel with a muzzle slot, two magazines, a
    /// chamber, plus a triple-scope rail for the mod limits and a revolver for the camora path.
    mod weapon {
        use super::*;

        use crate::bot::models::GenerateWeaponRequestWire;
        use crate::loot::item_helper::{ASSAULT_SCOPE, IRON_SIGHT, MOD, MOUNT, SIGHTS, WEAPON};

        /// `BaseClasses.ASSAULT_RIFLE`, the base class the sight whitelist is keyed by.
        const RIFLE_CLASS: &str = "5447b5f14bdc2d61278b4567";

        const M4: &str = "aaaaaaaaaaaaaaaaaaaaaaa1";
        const RECEIVER: &str = "aaaaaaaaaaaaaaaaaaaaaaa2";
        const SCOPE_MOUNT: &str = "aaaaaaaaaaaaaaaaaaaaaaa3";
        const SCOPE: &str = "aaaaaaaaaaaaaaaaaaaaaaa4";
        /// A sight that is not an assault scope, so the whitelist filters it out.
        const SCOPE_ALT: &str = "aaaaaaaaaaaaaaaaaaaaaaa5";
        const FRONT_SIGHT: &str = "aaaaaaaaaaaaaaaaaaaaaaa6";
        const REAR_SIGHT: &str = "aaaaaaaaaaaaaaaaaaaaaaa7";
        const STOCK: &str = "aaaaaaaaaaaaaaaaaaaaaaa8";
        const STOCK_SUB: &str = "aaaaaaaaaaaaaaaaaaaaaaa9";
        const GRIP: &str = "aaaaaaaaaaaaaaaaaaaaaab1";
        const BARREL: &str = "aaaaaaaaaaaaaaaaaaaaaab2";
        const MUZZLE: &str = "aaaaaaaaaaaaaaaaaaaaaab3";
        const MAGAZINE: &str = "aaaaaaaaaaaaaaaaaaaaaab4";
        const MAGAZINE_SMALL: &str = "aaaaaaaaaaaaaaaaaaaaaab5";
        const AMMO: &str = "aaaaaaaaaaaaaaaaaaaaaab6";
        const REVOLVER: &str = "aaaaaaaaaaaaaaaaaaaaaab7";
        const CYLINDER_CLASS: &str = "aaaaaaaaaaaaaaaaaaaaaab8";
        const CYLINDER: &str = "aaaaaaaaaaaaaaaaaaaaaab9";
        const SHELL: &str = "aaaaaaaaaaaaaaaaaaaaaac1";
        const RAIL_WEAPON: &str = "aaaaaaaaaaaaaaaaaaaaaac2";
        const SCOPE_B: &str = "aaaaaaaaaaaaaaaaaaaaaac3";
        const SCOPE_C: &str = "aaaaaaaaaaaaaaaaaaaaaac4";
        const WEAPON_ROOT_ID: &str = "dddddddddddddddddddddddd";

        struct WeaponFixture {
            items: IndexMap<String, ItemView>,
            bosses: Vec<String>,
            durability: BotDurability,
            equipment: IndexMap<String, EquipmentFilters>,
            randomization: IndexMap<String, RandomisedResourceDetails>,
            item_blacklist: HashSet<String>,
            default_presets_by_tpl: IndexMap<String, PresetView>,
            presets_by_id: IndexMap<String, PresetView>,
            equipment_blacklist: EquipmentFilterDetails,
            low_profile_gas_block_tpls: HashSet<String>,
        }

        impl WeaponFixture {
            fn new() -> Self {
                Self {
                    items: serde_json::from_value(json!({
                        WEAPON: {"type": "Node"},
                        MOD: {"type": "Node"},
                        MOUNT: {"parent": MOD, "type": "Node"},
                        SIGHTS: {"parent": MOD, "type": "Node"},
                        IRON_SIGHT: {"parent": SIGHTS, "type": "Node"},
                        ASSAULT_SCOPE: {"parent": SIGHTS, "type": "Node"},
                        RIFLE_CLASS: {"parent": WEAPON, "type": "Node"},
                        M4: {"parent": RIFLE_CLASS, "type": "Item", "name": "m4a1", "slots": [
                            {"name": "mod_reciever", "required": true, "filter": [RECEIVER]},
                            {"name": "mod_magazine", "filter": [MAGAZINE, MAGAZINE_SMALL]},
                            {"name": "mod_pistol_grip", "filter": [GRIP]},
                            {"name": "mod_stock", "filter": [STOCK]},
                        ], "chambers": [
                            {"name": "patron_in_weapon", "required": true, "filter": [AMMO]},
                        ]},
                        RECEIVER: {"parent": MOD, "type": "Item", "name": "receiver", "slots": [
                            {"name": "mod_scope", "filter": [SCOPE_MOUNT, SCOPE]},
                            {"name": "mod_barrel", "filter": [BARREL]},
                            {"name": "mod_sight_front", "filter": [FRONT_SIGHT]},
                            {"name": "mod_sight_rear", "filter": [REAR_SIGHT]},
                        ]},
                        SCOPE_MOUNT: {"parent": MOUNT, "type": "Item", "name": "scope_mount",
                                      "slots": [{"name": "mod_scope", "filter": [SCOPE]}]},
                        SCOPE: {"parent": ASSAULT_SCOPE, "type": "Item", "name": "scope"},
                        SCOPE_ALT: {"parent": SIGHTS, "type": "Item", "name": "scope_alt"},
                        FRONT_SIGHT: {"parent": IRON_SIGHT, "type": "Item", "name": "front_sight"},
                        REAR_SIGHT: {"parent": IRON_SIGHT, "type": "Item", "name": "rear_sight"},
                        STOCK: {"parent": MOD, "type": "Item", "name": "stock", "slots": [
                            {"name": "mod_stock_000", "filter": [STOCK_SUB]},
                        ]},
                        STOCK_SUB: {"parent": MOD, "type": "Item", "name": "stock_sub"},
                        GRIP: {"parent": MOD, "type": "Item", "name": "grip"},
                        BARREL: {"parent": MOD, "type": "Item", "name": "barrel", "slots": [
                            {"name": "mod_muzzle", "filter": [MUZZLE]},
                        ]},
                        MUZZLE: {"parent": MOD, "type": "Item", "name": "muzzle"},
                        MAGAZINE: {"parent": MOD, "type": "Item", "name": "magazine",
                                   "cartridgesMaxCount": 30},
                        MAGAZINE_SMALL: {"parent": MOD, "type": "Item", "name": "magazine_small",
                                         "cartridgesMaxCount": 10},
                        AMMO: {"parent": MOD, "type": "Item", "name": "ammo"},
                        REVOLVER: {"parent": RIFLE_CLASS, "type": "Item", "name": "revolver",
                                   "slots": [
                            {"name": "mod_magazine", "required": true, "filter": [CYLINDER]},
                        ]},
                        // Only its `_name` is read, by `MagazineIsCylinderRelated`.
                        CYLINDER_CLASS: {"parent": MOD, "type": "Node", "name": "CylinderMagazine"},
                        CYLINDER: {"parent": CYLINDER_CLASS, "type": "Item", "name": "cylinder",
                                   "slots": [
                            {"name": "camora_000", "filter": [SHELL]},
                            {"name": "camora_001", "filter": [SHELL]},
                            // Not a camora: `:1800` clones the chosen shell into it anyway.
                            {"name": "mod_sight_rear", "filter": [REAR_SIGHT]},
                        ]},
                        SHELL: {"parent": MOD, "type": "Item", "name": "shell"},
                        RAIL_WEAPON: {"parent": RIFLE_CLASS, "type": "Item", "name": "rail_weapon",
                                      "slots": [
                            {"name": "mod_scope", "filter": [SCOPE]},
                            {"name": "mod_scope_000", "filter": [SCOPE_B]},
                            {"name": "mod_scope_001", "filter": [SCOPE_C]},
                        ]},
                        SCOPE_B: {"parent": ASSAULT_SCOPE, "type": "Item", "name": "scope_b"},
                        SCOPE_C: {"parent": ASSAULT_SCOPE, "type": "Item", "name": "scope_c"},
                    }))
                    .unwrap(),
                    bosses: Vec::new(),
                    durability: durability_config(),
                    equipment: IndexMap::from([(
                        "assault".to_owned(),
                        EquipmentFilters::default(),
                    )]),
                    randomization: IndexMap::new(),
                    item_blacklist: HashSet::new(),
                    default_presets_by_tpl: IndexMap::new(),
                    presets_by_id: IndexMap::new(),
                    equipment_blacklist: EquipmentFilterDetails::default(),
                    low_profile_gas_block_tpls: HashSet::new(),
                }
            }

            fn ctx(&self) -> BotContext<'_> {
                BotContext {
                    items: &self.items,
                    bosses: &self.bosses,
                    durability: &self.durability,
                    equipment: &self.equipment,
                    loot_item_resource_randomization: &self.randomization,
                    is_night_time: false,
                    item_blacklist: &self.item_blacklist,
                    default_presets_by_tpl: &self.default_presets_by_tpl,
                    presets_by_id: &self.presets_by_id,
                    item_presets: &crate::bot::NO_PRESETS,
                    equipment_blacklist: &self.equipment_blacklist,
                    low_profile_gas_block_tpls: &self.low_profile_gas_block_tpls,
                    weapon_has_enhancement_chance_percent: 0.0,
                    repair_kit_weapon: &crate::bot::NO_BUFFS,
                    secure_container_ammo_stack_count: 0,
                    diagnostics: Vec::new(),
                }
            }
        }

        fn request(
            weapon_tpl: &str,
            mod_pool: serde_json::Value,
            chances: serde_json::Value,
        ) -> GenerateWeaponRequestWire {
            serde_json::from_value(json!({
                "weapon": [{"_id": WEAPON_ROOT_ID, "_tpl": weapon_tpl, "slotId": "FirstPrimaryWeapon"}],
                "modPool": mod_pool,
                "weaponId": WEAPON_ROOT_ID,
                "parentTemplate": weapon_tpl,
                "modSpawnChances": chances,
                "ammoTpl": AMMO,
                "botData": {"role": "assault", "level": 20, "equipmentRole": "assault"},
                "modLimits": {
                    "scope": {"count": 0},
                    "scopeMax": 2,
                    "scopeBaseTypes": [ASSAULT_SCOPE],
                    "flashlightLaser": {"count": 0},
                    "flashlightLaserMax": 1,
                    "flashlightLaserBaseTypes": [],
                },
                "weaponStats": {},
                "conflictingItemTpls": [],
            }))
            .unwrap()
        }

        /// Every slot the M4 tree can fill, at a chance of 100.
        fn m4_request() -> GenerateWeaponRequestWire {
            request(
                M4,
                json!({
                    M4: {
                        "mod_reciever": [RECEIVER],
                        "mod_magazine": [MAGAZINE, MAGAZINE_SMALL],
                        "mod_pistol_grip": [GRIP],
                        "mod_stock": [STOCK],
                        "patron_in_weapon": [AMMO],
                    },
                    RECEIVER: {
                        "mod_scope": [SCOPE_MOUNT, SCOPE],
                        "mod_barrel": [BARREL],
                        "mod_sight_front": [FRONT_SIGHT],
                        "mod_sight_rear": [REAR_SIGHT],
                    },
                    SCOPE_MOUNT: {"mod_scope": [SCOPE]},
                    STOCK: {"mod_stock_000": [STOCK_SUB]},
                    BARREL: {"mod_muzzle": [MUZZLE]},
                }),
                json!({
                    "mod_reciever": 100.0, "mod_magazine": 100.0, "mod_pistol_grip": 100.0,
                    "mod_stock": 100.0, "mod_stock_000": 100.0, "mod_scope": 100.0,
                    "mod_barrel": 100.0, "mod_muzzle": 100.0, "mod_sight_front": 100.0,
                    "mod_sight_rear": 100.0,
                }),
            )
        }

        /// `(slotId, tpl, parent)` with generated ids replaced by their index, as the equipment
        /// tests do.
        fn normalized(weapon: &[Item]) -> Vec<(String, String, String)> {
            let ids: IndexMap<&str, String> = weapon
                .iter()
                .enumerate()
                .map(|(index, item)| (item.id.as_str(), format!("#{index}")))
                .collect();

            weapon
                .iter()
                .map(|item| {
                    (
                        item.slot_id.clone().unwrap_or_default(),
                        item.template.clone(),
                        item.parent_id
                            .as_deref()
                            .and_then(|parent| ids.get(parent).cloned())
                            .unwrap_or_default(),
                    )
                })
                .collect()
        }

        #[test]
        fn seeded_m4_run_is_pinned() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut request = m4_request();

            let _guard = TestSeedGuard::install(SEED);
            generate_mods_for_weapon(&mut ctx, &mut request).unwrap();

            // `SortModKeys` puts the receiver, grip and stock ahead of the pool's own order, and the
            // receiver's own keys put the barrel and scope ahead of the two sights.
            assert_eq!(
                normalized(&request.weapon),
                vec![
                    (
                        "FirstPrimaryWeapon".to_owned(),
                        M4.to_owned(),
                        String::new()
                    ),
                    (
                        "mod_reciever".to_owned(),
                        RECEIVER.to_owned(),
                        "#0".to_owned()
                    ),
                    ("mod_barrel".to_owned(), BARREL.to_owned(), "#1".to_owned()),
                    ("mod_muzzle".to_owned(), MUZZLE.to_owned(), "#2".to_owned()),
                    (
                        "mod_scope".to_owned(),
                        SCOPE_MOUNT.to_owned(),
                        "#1".to_owned()
                    ),
                    ("mod_scope".to_owned(), SCOPE.to_owned(), "#4".to_owned()),
                    (
                        "mod_sight_front".to_owned(),
                        FRONT_SIGHT.to_owned(),
                        "#1".to_owned()
                    ),
                    (
                        "mod_sight_rear".to_owned(),
                        REAR_SIGHT.to_owned(),
                        "#1".to_owned()
                    ),
                    (
                        "mod_pistol_grip".to_owned(),
                        GRIP.to_owned(),
                        "#0".to_owned()
                    ),
                    ("mod_stock".to_owned(), STOCK.to_owned(), "#0".to_owned()),
                    (
                        "mod_stock_000".to_owned(),
                        STOCK_SUB.to_owned(),
                        "#9".to_owned()
                    ),
                    (
                        "mod_magazine".to_owned(),
                        MAGAZINE.to_owned(),
                        "#0".to_owned()
                    ),
                    (
                        "patron_in_weapon".to_owned(),
                        AMMO.to_owned(),
                        "#0".to_owned()
                    ),
                ]
            );
            // Every mod has its own fresh id.
            let ids: HashSet<&str> = request.weapon.iter().map(|item| item.id.as_str()).collect();
            assert_eq!(ids.len(), request.weapon.len());
            // An iron sight in each sight slot, and the scope is the optic.
            assert_eq!(request.weapon_stats.has_front_iron_sight, Some(true));
            assert_eq!(request.weapon_stats.has_rear_iron_sight, Some(true));
            assert_eq!(request.weapon_stats.has_optic, Some(true));
        }

        /// `:618-630`: a mount picked for a scope slot forces every scope slot to 100%, and `:641`
        /// forces the opposite sight. Both are visible on the request the caller keeps.
        #[test]
        fn picking_a_mount_and_a_sight_forces_their_sibling_slots_to_full_chance() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut request = m4_request();
            // Only the front sight is allowed to roll; the rear one is forced by `:643-644`.
            request
                .mod_spawn_chances
                .insert("mod_sight_rear".to_owned(), 0.0);
            request
                .mod_spawn_chances
                .insert("mod_scope".to_owned(), 0.0);

            let _guard = TestSeedGuard::install(SEED);
            generate_mods_for_weapon(&mut ctx, &mut request).unwrap();

            assert_eq!(request.mod_spawn_chances["mod_sight_front"], 100.0);
            assert_eq!(request.mod_spawn_chances["mod_sight_rear"], 100.0);
            // The mount was skipped, so the scope slots were never forced.
            assert_eq!(request.mod_spawn_chances["mod_scope"], 0.0);
            assert!(
                request
                    .weapon
                    .iter()
                    .any(|item| item.template == REAR_SIGHT),
                "the forced rear sight should have spawned"
            );

            // With the scope slot allowed to roll, the mount lands and forces all five scope slots.
            let mut ctx = fixture.ctx();
            let mut request = m4_request();
            let _guard = TestSeedGuard::install(SEED);
            generate_mods_for_weapon(&mut ctx, &mut request).unwrap();

            for slot in SCOPE_SLOTS_TO_FORCE {
                assert_eq!(request.mod_spawn_chances[slot], 100.0, "{slot}");
            }
            // The barrel's muzzle slot took the 95% of `:637`.
            assert_eq!(request.mod_spawn_chances["mod_muzzle"], 95.0);
            assert_eq!(request.mod_spawn_chances["mod_muzzle_000"], 95.0);
            // The stock takes a sub-stock, so `:666` forced its slots too.
            assert_eq!(request.mod_spawn_chances["mod_stock_akms"], 100.0);
        }

        /// `:847` never reads `modsParentId`, so a wrong one changes nothing.
        #[test]
        fn muzzle_slots_ignore_the_parent_id() {
            assert!(mod_slot_can_hold_muzzle_devices("mod_muzzle", None));
            assert!(mod_slot_can_hold_muzzle_devices(
                "mod_muzzle",
                Some("not-a-parent-id-at-all")
            ));
            assert!(mod_slot_can_hold_muzzle_devices(
                "MOD_MUZZLE_001",
                Some(MOD)
            ));
            assert!(!mod_slot_can_hold_muzzle_devices("mod_scope", Some(MOUNT)));

            // The scope test, by contrast, does read it.
            assert!(mod_slot_can_hold_scope("mod_scope", MOUNT));
            assert!(!mod_slot_can_hold_scope("mod_scope", MOD));
        }

        /// `BotWeaponModLimitService`: the third scope is refused and the counter stops at the max.
        #[test]
        fn the_scope_limit_stops_the_third_scope() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut request = request(
                RAIL_WEAPON,
                json!({RAIL_WEAPON: {
                    "mod_scope": [SCOPE],
                    "mod_scope_000": [SCOPE_B],
                    "mod_scope_001": [SCOPE_C],
                }}),
                json!({"mod_scope": 100.0, "mod_scope_000": 100.0, "mod_scope_001": 100.0}),
            );

            let _guard = TestSeedGuard::install(SEED);
            generate_mods_for_weapon(&mut ctx, &mut request).unwrap();

            assert_eq!(
                normalized(&request.weapon),
                vec![
                    (
                        "FirstPrimaryWeapon".to_owned(),
                        RAIL_WEAPON.to_owned(),
                        String::new()
                    ),
                    ("mod_scope".to_owned(), SCOPE.to_owned(), "#0".to_owned()),
                    (
                        "mod_scope_000".to_owned(),
                        SCOPE_B.to_owned(),
                        "#0".to_owned()
                    ),
                ]
            );
            assert_eq!(request.mod_limits.scope.count, Some(2));
            let reported = ctx
                .diagnostics
                .last()
                .expect("the refused scope is reported");
            assert_eq!(reported.level, DEBUG);
            assert_eq!(
                reported.message.as_deref(),
                Some(
                    format!(
                        "[assault] scope limit reached! tried to add: {SCOPE_C} but scope count is: 2"
                    )
                    .as_str()
                )
            );
        }

        /// A null `ItemCount.Count` is a lifted `int?`: never limited, never incremented.
        #[test]
        fn a_null_mod_count_is_never_limited_and_never_incremented() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut count = ItemCountWire { count: None };

            assert!(!weapon_mod_limit_reached(
                &mut ctx,
                SCOPE,
                &mut count,
                Some(1),
                "assault",
                "scope"
            ));
            assert_eq!(count.count, None);

            // No limit set at all is an early false that does not count either.
            let mut count = ItemCountWire { count: Some(5) };
            assert!(!weapon_mod_limit_reached(
                &mut ctx, SCOPE, &mut count, None, "assault", "scope"
            ));
            assert!(!weapon_mod_limit_reached(
                &mut ctx,
                SCOPE,
                &mut count,
                Some(0),
                "assault",
                "scope"
            ));
            assert_eq!(count.count, Some(5));
            assert!(ctx.diagnostics.is_empty());
        }

        /// `:1800-1813`: one ammo tpl, cloned into **every** slot of the cylinder — the non-camora
        /// slot included — each clone with a fresh id.
        #[test]
        fn the_camora_path_clones_one_shell_into_every_slot() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut request = request(
                REVOLVER,
                json!({REVOLVER: {"mod_magazine": [CYLINDER]}}),
                json!({"mod_magazine": 100.0}),
            );

            let _guard = TestSeedGuard::install(SEED);
            generate_mods_for_weapon(&mut ctx, &mut request).unwrap();

            assert_eq!(
                normalized(&request.weapon),
                vec![
                    (
                        "FirstPrimaryWeapon".to_owned(),
                        REVOLVER.to_owned(),
                        String::new()
                    ),
                    (
                        "mod_magazine".to_owned(),
                        CYLINDER.to_owned(),
                        "#0".to_owned()
                    ),
                    ("camora_000".to_owned(), SHELL.to_owned(), "#1".to_owned()),
                    ("camora_001".to_owned(), SHELL.to_owned(), "#1".to_owned()),
                    // Not a camora, and filled with a shell all the same.
                    (
                        "mod_sight_rear".to_owned(),
                        SHELL.to_owned(),
                        "#1".to_owned()
                    ),
                ]
            );
            let ids: HashSet<&str> = request.weapon.iter().map(|item| item.id.as_str()).collect();
            assert_eq!(ids.len(), request.weapon.len(), "a clone reused an id");
            // The cylinder had no pool entry, so one was generated from its camora slots.
            assert_eq!(
                request.mod_pool[CYLINDER].keys().collect::<Vec<_>>(),
                vec!["camora_000", "camora_001"]
            );
            let reported = ctx
                .diagnostics
                .iter()
                .find(|entry| {
                    entry.locale_key.as_deref()
                        == Some("bot-unable_to_fill_camora_slot_mod_pool_empty")
                })
                .expect("the empty camora pool is reported");
            assert_eq!(reported.level, WARNING);
            assert_eq!(reported.args.as_ref().unwrap()["weaponId"], CYLINDER);
        }

        /// The `cartridges` entry wins over the camora pools when the mod pool carries one.
        #[test]
        fn the_camora_path_prefers_a_cartridges_pool() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut weapon = vec![Item {
                id: WEAPON_ROOT_ID.to_owned(),
                template: REVOLVER.to_owned(),
                ..Default::default()
            }];
            let mut mod_pool: IndexMap<String, IndexMap<String, IndexSet<String>>> =
                serde_json::from_value(json!({CYLINDER: {"cartridges": [SHELL]}})).unwrap();

            let _guard = TestSeedGuard::install(SEED);
            fill_camora(
                &mut ctx,
                &mut weapon,
                &mut mod_pool,
                WEAPON_ROOT_ID,
                CYLINDER,
                &fixture.items[CYLINDER],
            );

            assert_eq!(weapon.len(), 4);
            assert!(weapon[1..].iter().all(|item| item.template == SHELL));
            assert!(ctx.diagnostics.is_empty());
        }

        /// A cylinder whose pool holds neither `cartridges` nor `camora_000` is reported and skipped.
        #[test]
        fn a_cylinder_without_a_cartridge_pool_is_reported() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut weapon = Vec::new();
            let mut mod_pool: IndexMap<String, IndexMap<String, IndexSet<String>>> =
                serde_json::from_value(json!({CYLINDER: {"mod_sight_rear": [REAR_SIGHT]}}))
                    .unwrap();

            fill_camora(
                &mut ctx,
                &mut weapon,
                &mut mod_pool,
                WEAPON_ROOT_ID,
                CYLINDER,
                &fixture.items[CYLINDER],
            );

            assert!(weapon.is_empty());
            assert_eq!(ctx.diagnostics.len(), 1);
            assert_eq!(ctx.diagnostics[0].level, ERROR);
            assert_eq!(
                ctx.diagnostics[0].locale_key.as_deref(),
                Some("bot-missing_cartridge_slot")
            );
            assert_eq!(ctx.diagnostics[0].args, Some(json!(CYLINDER)));
        }

        /// The whitelist keeps whitelisted sight base classes and mounts that hold only those.
        #[test]
        fn the_sight_whitelist_filters_by_weapon_base_class() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let weapon = Item {
                id: WEAPON_ROOT_ID.to_owned(),
                template: M4.to_owned(),
                ..Default::default()
            };
            let scopes: IndexSet<String> = [SCOPE, SCOPE_ALT, SCOPE_MOUNT, GRIP]
                .iter()
                .map(|tpl| (*tpl).to_owned())
                .collect();
            let whitelist: IndexMap<String, Vec<String>> =
                IndexMap::from([(RIFLE_CLASS.to_owned(), vec![ASSAULT_SCOPE.to_owned()])]);

            assert_eq!(
                filter_sights_by_weapon_type(&mut ctx, &weapon, &scopes, &whitelist),
                IndexSet::from([SCOPE.to_owned(), SCOPE_MOUNT.to_owned()])
            );
            assert!(ctx.diagnostics.is_empty());

            // A weapon type with no whitelist entry keeps the whole pool, and says so.
            let unlisted: IndexMap<String, Vec<String>> =
                IndexMap::from([("999999999999999999999999".to_owned(), Vec::new())]);
            assert_eq!(
                filter_sights_by_weapon_type(&mut ctx, &weapon, &scopes, &unlisted),
                scopes
            );
            assert_eq!(ctx.diagnostics.len(), 1);
            assert_eq!(ctx.diagnostics[0].level, DEBUG);
            assert_eq!(
                ctx.diagnostics[0].message.as_deref(),
                Some(
                    format!(
                        "Unable to find whitelist for weapon type: {RIFLE_CLASS} m4a1, skipping sight filtering"
                    )
                    .as_str()
                )
            );

            // A whitelist that leaves nothing behind is ignored.
            let empty: IndexMap<String, Vec<String>> =
                IndexMap::from([(RIFLE_CLASS.to_owned(), vec![IRON_SIGHT.to_owned()])]);
            let only_grip: IndexSet<String> = IndexSet::from([GRIP.to_owned()]);
            assert_eq!(
                filter_sights_by_weapon_type(&mut ctx, &weapon, &only_grip, &empty),
                only_grip
            );
        }

        /// `:858-951`'s ordering table, both arms.
        #[test]
        fn sort_mod_keys_follows_the_ordering_table() {
            let fixture = WeaponFixture::new();
            let pool = |keys: &[&str]| -> IndexMap<String, IndexSet<String>> {
                keys.iter()
                    .map(|key| ((*key).to_owned(), IndexSet::new()))
                    .collect()
            };

            // Not a mount: handguard, barrel, mount_001, reciever, pistol grip, gas block, stock,
            // mount, scope — then everything else in the pool's own order.
            assert_eq!(
                sort_mod_keys(
                    &fixture.items,
                    &pool(&[
                        "mod_muzzle",
                        "mod_scope",
                        "mod_stock",
                        "mod_gas_block",
                        "mod_reciever",
                        "mod_barrel",
                        "mod_handguard",
                        "mod_magazine",
                        "mod_pistol_grip",
                        "mod_mount",
                        "mod_mount_001",
                    ]),
                    M4
                ),
                vec![
                    "mod_handguard",
                    "mod_barrel",
                    "mod_mount_001",
                    "mod_reciever",
                    "mod_pistol_grip",
                    "mod_gas_block",
                    "mod_stock",
                    "mod_mount",
                    "mod_scope",
                    "mod_muzzle",
                    "mod_magazine",
                ]
            );

            // A mount wants scopes before more mounts.
            assert_eq!(
                sort_mod_keys(
                    &fixture.items,
                    &pool(&["mod_mount", "mod_scope", "mod_tactical", "mod_scope_000"]),
                    SCOPE_MOUNT
                ),
                vec!["mod_scope_000", "mod_scope", "mod_mount", "mod_tactical"]
            );

            // One key sorts to itself.
            assert_eq!(
                sort_mod_keys(&fixture.items, &pool(&["mod_scope"]), M4),
                vec!["mod_scope"]
            );
        }

        /// The chamber and cartridge arms `:959-979` grew for this path.
        #[test]
        fn slot_lookup_reads_chambers_and_cartridges() {
            let fixture = WeaponFixture::new();
            let m4 = Some(&fixture.items[M4]);

            // The chamber arm matches by `Contains`, so the bare name finds a numbered chamber too.
            assert_eq!(
                get_mod_item_slot_from_db_template("patron_in_weapon", m4)
                    .and_then(|slot| slot.name.as_deref()),
                Some("patron_in_weapon")
            );
            assert!(get_mod_item_slot_from_db_template("cartridges", m4).is_none());
            assert_eq!(
                get_mod_item_slot_from_db_template("MOD_MAGAZINE", m4)
                    .and_then(|slot| slot.name.as_deref()),
                Some("mod_magazine")
            );
            assert!(get_mod_item_slot_from_db_template("mod_scope", m4).is_none());

            let magazine: ItemView = serde_json::from_value(json!({
                "cartridges": [{"name": "cartridges", "filter": [AMMO]}],
            }))
            .unwrap();
            assert_eq!(
                get_mod_item_slot_from_db_template("cartridges", Some(&magazine))
                    .and_then(|slot| slot.filter.clone()),
                Some(vec![AMMO.to_owned()])
            );
        }

        /// `:505-520`: a weapon with no slots, cartridges or chambers is reported and left alone.
        #[test]
        fn a_weapon_without_slots_is_reported_and_untouched() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut request = request(SHELL, json!({SHELL: {}}), json!({}));

            generate_mods_for_weapon(&mut ctx, &mut request).unwrap();

            assert_eq!(request.weapon.len(), 1);
            assert_eq!(ctx.diagnostics.len(), 1);
            assert_eq!(ctx.diagnostics[0].level, ERROR);
            assert_eq!(
                ctx.diagnostics[0].locale_key.as_deref(),
                Some("bot-unable_to_add_mods_to_weapon_missing_ammo_slot")
            );
            assert_eq!(
                ctx.diagnostics[0].args.as_ref().unwrap()["weaponName"],
                "shell"
            );
        }

        /// A pool key with no matching slot on the weapon is an error and a skip (`:541-557`).
        #[test]
        fn a_pool_key_with_no_slot_on_the_weapon_is_reported() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let mut request = request(
                M4,
                json!({M4: {"mod_launcher": [MUZZLE]}}),
                json!({"mod_launcher": 100.0}),
            );

            let _guard = TestSeedGuard::install(SEED);
            generate_mods_for_weapon(&mut ctx, &mut request).unwrap();

            assert_eq!(request.weapon.len(), 1);
            assert_eq!(ctx.diagnostics.len(), 1);
            assert_eq!(
                ctx.diagnostics[0].locale_key.as_deref(),
                Some("bot-weapon_missing_mod_slot")
            );
            assert_eq!(
                ctx.diagnostics[0].args.as_ref().unwrap()["modSlot"],
                "mod_launcher"
            );
        }

        /// `:536` and `:533` both dereference something a missing key leaves null, and `:533` is
        /// the one that runs first.
        #[test]
        fn a_missing_pool_or_equipment_role_is_the_null_deref() {
            let fixture = WeaponFixture::new();

            let mut ctx = fixture.ctx();
            let mut request = request(M4, json!({}), json!({}));
            let error = generate_mods_for_weapon(&mut ctx, &mut request).unwrap_err();
            assert!(
                error.message.contains("no mod pool for item"),
                "{}",
                error.message
            );

            let mut ctx = fixture.ctx();
            let mut request = m4_request();
            request.bot_data.equipment_role = "pmc".to_owned();
            let error = generate_mods_for_weapon(&mut ctx, &mut request).unwrap_err();
            assert!(
                error.message.contains("no equipment config for role: pmc"),
                "{}",
                error.message
            );

            // Both missing: `GetBotRandomizationDetails` at `:533` throws before the `:536` pool
            // read is reached.
            let mut ctx = fixture.ctx();
            let mut both_missing = m4_request();
            both_missing.mod_pool.clear();
            both_missing.bot_data.equipment_role = "pmc".to_owned();
            let error = generate_mods_for_weapon(&mut ctx, &mut both_missing).unwrap_err();
            assert!(
                error.message.contains("no equipment config for role: pmc"),
                "{}",
                error.message
            );
        }

        /// `forceStock` makes a stock slot forced even when the stock takes no sub-stock.
        #[test]
        fn force_stock_forces_slots_a_childless_stock_would_not() {
            let stock: ItemView = serde_json::from_value(json!({})).unwrap();
            let with_children: ItemView =
                serde_json::from_value(json!({"slots": [{"name": "mod_stock_000"}]})).unwrap();
            let forced: EquipmentFilters =
                serde_json::from_value(json!({"forceStock": true})).unwrap();
            let plain = EquipmentFilters::default();

            assert!(!should_force_sub_stock_slots("mod_stock", &plain, &stock));
            assert!(should_force_sub_stock_slots(
                "mod_stock",
                &plain,
                &with_children
            ));
            assert!(should_force_sub_stock_slots(
                "mod_pistol_grip",
                &forced,
                &stock
            ));
        }

        /// `:1242-1290`: 75% of the pool size, rounded half to even, and the counter reset that
        /// `:1280` performs on the way out of the loop.
        #[test]
        fn a_pool_of_conflicting_mods_gives_up_after_the_blocked_ceiling() {
            let mut items: IndexMap<String, ItemView> = serde_json::from_value(json!({
                M4: {"parent": RIFLE_CLASS, "type": "Item", "name": "m4a1"},
            }))
            .unwrap();
            // Six mods, every one of them conflicting with the weapon already on the gun.
            let conflicting: Vec<String> = (0..6)
                .map(|index| format!("bbbbbbbbbbbbbbbbbbbbbbb{index}"))
                .collect();
            for tpl in &conflicting {
                items.insert(
                    tpl.clone(),
                    serde_json::from_value(json!({"parent": MOD, "conflictingItems": [M4]}))
                        .unwrap(),
                );
            }
            let fixture = WeaponFixture {
                items,
                ..WeaponFixture::new()
            };
            let mut ctx = fixture.ctx();
            let weapon = vec![Item {
                id: WEAPON_ROOT_ID.to_owned(),
                template: M4.to_owned(),
                ..Default::default()
            }];
            let pool: IndexSet<String> = conflicting.iter().cloned().collect();

            let _guard = TestSeedGuard::install(SEED);
            let outcome = get_compatible_mod_from_pool(&mut ctx, &pool, ModSpawn::Spawn, &weapon);

            // Math.Round(6 * 0.75) = 4.5 -> 4 (half to even), and the count has to get *past* it.
            assert_eq!(outcome.found, Some(false));
            assert_eq!(outcome.reason.as_deref(), Some("Blocked"));
            assert_eq!(outcome.chosen_template, None);
            // `:1281` is commented out on purpose: nothing here sets it.
            assert_eq!(outcome.slot_blocked, None);
        }

        /// An empty pool after the conflict filter names the pool size it started with.
        #[test]
        fn a_fully_conflicting_pool_reports_its_size() {
            let fixture = WeaponFixture::new();
            let mut ctx = fixture.ctx();
            let request = m4_request();
            let conflicting: IndexSet<String> =
                IndexSet::from([SCOPE.to_owned(), SCOPE_MOUNT.to_owned()]);
            let slot: SlotView =
                serde_json::from_value(json!({"name": "mod_scope", "filter": [SCOPE]})).unwrap();

            let outcome = get_compatible_weapon_mod_tpl_for_slot_from_pool(
                &mut ctx,
                &ModToSpawnRequest {
                    mod_slot: "mod_scope",
                    is_randomisable_slot: false,
                    randomisation_settings: None,
                    bot_weapon_sight_whitelist: None,
                    bot_equip_blacklist: &fixture.equipment_blacklist,
                    item_mod_pool: &IndexMap::new(),
                    weapon: &request.weapon,
                    ammo_tpl: AMMO,
                    parent_template: M4,
                    mod_spawn_result: ModSpawn::DefaultMod,
                    weapon_stats: &request.weapon_stats,
                    conflicting_item_tpls: &conflicting,
                    bot_data: &request.bot_data,
                },
                &conflicting,
                &slot,
                ModSpawn::DefaultMod,
                &request.weapon,
                "mod_scope",
            );

            assert_eq!(outcome.found, Some(false));
            assert_eq!(
                outcome.reason.as_deref(),
                Some("Unable to add mod to DEFAULT_MOD slot: mod_scope. All: 2 had conflicts")
            );
        }
    }

    /// A required slot whose roll failed asks for the default mod instead of skipping — and the roll
    /// is consumed either way.
    #[test]
    fn a_required_slot_that_fails_its_roll_asks_for_the_default_mod() {
        let required: SlotView =
            serde_json::from_value(json!({"name": "front_plate", "required": true})).unwrap();
        let optional: SlotView =
            serde_json::from_value(json!({"name": "front_plate", "required": false})).unwrap();
        let chances = IndexMap::from([("front_plate".to_owned(), 0.0)]);
        let config = EquipmentFilters::default();

        let _guard = TestSeedGuard::install(SEED);
        assert_eq!(
            should_mod_be_spawned(&required, "front_plate", &chances, &config),
            ModSpawn::DefaultMod
        );
        assert_eq!(
            should_mod_be_spawned(&optional, "front_plate", &chances, &config),
            ModSpawn::Skip
        );
    }
}
