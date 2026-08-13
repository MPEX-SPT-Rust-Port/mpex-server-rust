//! `Generators/Bot/BotEquipmentModGenerator.cs:97-502` — the equipment half of the mod generator.
//!
//! The weapon half (`:503-...`) is a separate task; the helpers both halves share
//! ([`should_mod_be_spawned`], [`get_mod_item_slot_from_db_template`], [`filter_mods_by_blacklist`],
//! [`get_random_mod_tpl_from_item_db`], [`is_mod_valid_for_slot`], [`create_mod_item`]) are ported
//! here because the equipment path is the first to reach them.
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
#![allow(
    dead_code,
    reason = "consumed by the bot inventory generator in the tasks that follow"
)]

use indexmap::{IndexMap, IndexSet};

use crate::bot::BotContext;
use crate::bot::bot_generator_helper::{
    generate_extra_properties_for_item, is_item_incompatible_with_current_items,
};
use crate::bot::exhaustable_array::ExhaustableArray;
use crate::bot::models::{
    EquipmentFilterDetails, EquipmentFilters, GenerateEquipmentPropertiesWire,
};
use crate::loot::item_helper::{LootError, get_item};
use crate::loot::models::{DEBUG, Diagnostic, ERROR, Item, ItemView, SlotView, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{get_weighted_value, roll_chance};

/// `BotEquipmentModGenerator._cartridgeHolderSlots` (`:67-74`), returned by `GetAmmoContainers`
/// (`:1527-1530`).
const CARTRIDGE_HOLDER_SLOTS: [&str; 5] = [
    "mod_magazine",
    "patron_in_weapon",
    "patron_in_weapon_000",
    "patron_in_weapon_001",
    "cartridges",
];

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
/// **Deviation:** only the `default:` arm is ported. The `patron_in_weapon*` and `cartridges` arms
/// read `Properties.Chambers`/`Properties.Cartridges`, which the flattened [`ItemView`] does not
/// carry as slot objects; no equipment mod pool holds those slot names, since its keys come from
/// `Properties.Slots`. The weapon path grows them.
fn get_mod_item_slot_from_db_template<'a>(
    mod_slot: &str,
    parent_template: Option<&'a ItemView>,
) -> Option<&'a SlotView> {
    let mod_slot_lower = mod_slot.to_lowercase();

    parent_template?.slots.as_ref()?.iter().find(|slot| {
        slot.name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(&mod_slot_lower))
    })
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
                // Unread here — no fixture template has a durability — but `BotContext` carries it.
                durability: serde_json::from_value(json!({
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
                .unwrap(),
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
