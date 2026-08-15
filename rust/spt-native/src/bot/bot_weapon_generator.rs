//! `Generators/Bot/BotWeaponGenerator.cs` — pick a weapon, kit it out, load it, and hand the bot
//! its spare magazines.
//!
//! Also here: the one method of `Services/Bot/BotWeaponModLimitService.cs` this file calls
//! ([`get_weapon_mod_limits`], `:22-40`), ported inline the way its sibling
//! `WeaponModHasReachedLimit` was into [`crate::bot::bot_equipment_mod_generator`].
//!
//! # Deviations
//!
//! - `GenerateWeaponResult.WeaponTemplate` rides as a tpl and `WeaponMods` is dropped; see
//!   [`GenerateWeaponResultWire`].
//! - `PickWeightedWeaponTemplateFromPool` parses its slot name into an `EquipmentSlots` before
//!   indexing; the pool map is keyed by that member name as a string here, so the parse is a
//!   membership test against [`EQUIPMENT_SLOTS`] — and, exactly as in C#, a name that is not a
//!   member logs and then reads the pool of the enum's default member, `Headwear`.
//! - `botTemplateInventory.Ammo is null` (`:135`) becomes "the ammo map is empty": `#[serde(default)]`
//!   turns an absent key into an empty map. The two error lines still fire; the C#
//!   `NullReferenceException` that follows does not, the empty map simply misses its caliber and
//!   falls through to the weapon's `DefAmmo`.
//! - `GetCompatibleCartridgesFromWeaponTemplate` (`:685`) throws on a chamber with an *empty*
//!   `Filters` list and returns null for an absent one; the flattened
//!   [`crate::loot::models::SlotView`] cannot tell those apart, so both take the null path and fall
//!   back to the magazine. Same for the magazine template's own `Slots`/`Cartridges` (`:722-723`),
//!   which C# dereferences unguarded.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! One full [`generate_random_weapon`] run draws, in this order:
//!
//! 1. `PickWeightedWeaponTemplateFromPool` (`:99`) — 1 `GetWeightedValue` over the slot's pool.
//! 2. `GetWeightedCompatibleAmmo` (`:141`) — 1 `GetWeightedValue` (`:673`), or **none** down any of
//!    the four `DefAmmo`/chamber-filter early returns.
//! 3. `ConstructWeaponBaseList` (`:144`) → `GenerateExtraPropertiesForItem`, whose draws are listed
//!    in [`crate::bot::bot_generator_helper`] — 2 `GetInt` for a weapon with durability.
//! 4. The enhancement gate (`:154`) — for a **PMC only**, 1 `GetChance100`, consumed even at a 0%
//!    chance (the `&&` short-circuits on `IsPmc`, never on the roll). When it wins,
//!    `RepairService.AddBuff` adds its 4 draws.
//! 5. `GenerateModsForWeapon` (`:187`), only when the bot's mod pool has an entry for the weapon —
//!    see [`crate::bot::bot_equipment_mod_generator`].
//! 6. `IsWeaponValid` (`:191`) draws nothing; the `GetPresetWeaponMods` fallback draws only its own
//!    `GenerateExtraPropertiesForItem` for the preset root.
//! 7. `FillExistingMagazines` (`:207`), once per `mod_magazine` item — 1 `GetInt` inside
//!    `FillMagazineWithCartridge`, or **none** for a cylinder magazine, which swaps camora ammo
//!    without drawing.
//! 8. The chamber (`:218`) and UBGL fill (`:232`) draw nothing; the UBGL's own
//!    `GetWeightedCompatibleAmmo` (`:227`) draws as in step 2.
//!
//! [`add_extra_magazines_to_inventory`] then draws: the UBGL grenade count (1 `GetWeightedValue`,
//! `:506`), then whatever the dispatched [`crate::bot::inventory_mag_gen`] implementation draws.
//! Both `AddAmmoToSecureContainer` calls draw nothing.
use indexmap::IndexMap;

use crate::bot::BotContext;
use crate::bot::bot_equipment_mod_generator::generate_mods_for_weapon;
use crate::bot::bot_generator_helper::{
    ContainerGrids, FLASHLIGHT, TACTICAL_COMBO, generate_extra_properties_for_item,
    get_bot_equipment_role,
};
use crate::bot::bot_weapon_generator_helper::magazine_is_cylinder_related;
use crate::bot::inventory_mag_gen::{InventoryMagGen, MAG_GEN_ORDER, process_mag_gen};
use crate::bot::models::{
    BotDataWire, BotGenerationDetailsWire, BotModLimitsWire, BotTypeInventoryWire,
    GenerateWeaponRequestWire, GenerateWeaponResultWire, GenerationDataWire, ItemCountWire,
};
use crate::bot::repair_service::add_buff;
use crate::loot::item_helper::{
    ASSAULT_SCOPE, COLLIMATOR, COMPACT_COLLIMATOR, LootError, OPTIC_SCOPE, PORTABLE_RANGE_FINDER,
    SPECIAL_SCOPE, fill_magazine_with_cartridge, get_item,
};
use crate::loot::models::{DEBUG, Diagnostic, ERROR, Item, ItemView, Upd, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{get_chance_100, get_weighted_value};

/// `BotWeaponGenerator.ModMagazineSlotId` (`:44`).
const MOD_MAGAZINE_SLOT_ID: &str = "mod_magazine";

/// `ReloadMode.OnlyBarrel`.
const ONLY_BARREL: &str = "OnlyBarrel";

/// `Models/Enums/EquipmentSlots.cs`, in declaration order — `Enum.TryParse` accepts exactly these,
/// and the first is the `default(EquipmentSlots)` a failed parse falls back to.
const EQUIPMENT_SLOTS: [&str; 14] = [
    "Headwear",
    "Earpiece",
    "FaceCover",
    "ArmorVest",
    "Eyewear",
    "ArmBand",
    "TacticalVest",
    "Pockets",
    "Backpack",
    "SecuredContainer",
    "FirstPrimaryWeapon",
    "SecondPrimaryWeapon",
    "Holster",
    "Scabbard",
];

/// `EquipmentSlots.SecuredContainer`, the one slot `AddAmmoToSecureContainer` targets.
const SECURED_CONTAINER: &str = "SecuredContainer";

/// `BotWeaponGenerator.GenerateRandomWeapon` (`:64-83`).
///
/// **Deviation:** takes no [`ContainerGrids`] — nothing on this path touches a container. The
/// grids are what [`add_extra_magazines_to_inventory`] needs, and `BotInventoryGenerator` calls the
/// two in sequence.
///
/// # Errors
///
/// See [`generate_weapon_by_tpl`] and [`pick_weighted_weapon_template_from_pool`].
pub fn generate_random_weapon(
    ctx: &mut BotContext,
    equipment_slot: &str,
    bot_template_inventory: &mut BotTypeInventoryWire,
    details: &BotGenerationDetailsWire,
    weapon_parent_id: &str,
    mod_chances: &mut IndexMap<String, f64>,
) -> Result<Option<GenerateWeaponResultWire>, LootError> {
    let weapon_tpl =
        pick_weighted_weapon_template_from_pool(ctx, equipment_slot, bot_template_inventory)?;

    generate_weapon_by_tpl(
        ctx,
        &weapon_tpl,
        equipment_slot,
        bot_template_inventory,
        weapon_parent_id,
        mod_chances,
        details,
    )
}

/// `BotWeaponGenerator.PickWeightedWeaponTemplateFromPool` (`:91-100`).
///
/// # Errors
///
/// The `Equipment[key]` indexer (`:98`), which is a `KeyNotFoundException` for a bot whose template
/// has no pool for the slot, and `GetWeightedValue` on an empty one.
pub fn pick_weighted_weapon_template_from_pool(
    ctx: &mut BotContext,
    equipment_slot: &str,
    bot_template_inventory: &BotTypeInventoryWire,
) -> Result<String, LootError> {
    let key = if EQUIPMENT_SLOTS.contains(&equipment_slot) {
        equipment_slot
    } else {
        ctx.diagnostics.push(plain(
            ERROR,
            format!("Unable to parse equipment slot: {equipment_slot}"),
        ));

        // `Enum.TryParse` zeroes its out param on failure, so C# goes on to read the pool of the
        // enum's first member.
        EQUIPMENT_SLOTS[0]
    };

    let Some(weapon_pool) = bot_template_inventory.equipment.get(key) else {
        return Err(LootError::new(format!(
            "The given key '{key}' was not present in the dictionary."
        )));
    };

    get_weighted_value(weapon_pool)
}

/// `BotWeaponGenerator.GenerateWeaponByTpl` (`:113-244`). `None` is the C# `null` return: the
/// weapon tpl is missing from the database.
///
/// # Errors
///
/// Where the C# throws:
/// - the `:212-213` chain — `Chambers.Any()` on a null list is an `ArgumentNullException`, and the
///   `FirstOrDefault().Properties.Filters.FirstOrDefault().Filter` behind it is an NRE for a weapon
///   whose first chamber declares no filter;
/// - the `Equipment[botRole]` indexer inside [`get_weapon_mod_limits`];
/// - everything [`get_weighted_compatible_ammo`], [`get_preset_weapon_mods`], `AddBuff` and
///   `GenerateModsForWeapon` raise.
pub fn generate_weapon_by_tpl(
    ctx: &mut BotContext,
    weapon_tpl: &str,
    slot_name: &str,
    bot_template_inventory: &mut BotTypeInventoryWire,
    weapon_parent_id: &str,
    mod_chances: &mut IndexMap<String, f64>,
    details: &BotGenerationDetailsWire,
) -> Result<Option<GenerateWeaponResultWire>, LootError> {
    let items = ctx.items;

    let Some(weapon_item_template) = get_item(items, weapon_tpl) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-missing_item_template",
            serde_json::Value::String(weapon_tpl.to_owned()),
        ));
        ctx.diagnostics
            .push(plain(ERROR, format!("WeaponSlot -> {slot_name}")));

        return Ok(None);
    };

    // Find ammo to use when filling magazines/chamber
    if bot_template_inventory.ammo.is_empty() {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-no_ammo_found_in_bot_json",
            serde_json::Value::String(details.role_lowercase.clone()),
        ));
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-generation_failed",
            serde_json::json!({}),
        ));
    }

    let ammo_tpl = get_weighted_compatible_ammo(
        ctx,
        &bot_template_inventory.ammo,
        weapon_tpl,
        weapon_item_template,
    )?;

    // Create with just base weapon item
    let mut weapon_with_mods = construct_weapon_base_list(
        ctx,
        weapon_tpl,
        weapon_parent_id,
        slot_name,
        weapon_item_template,
        &details.role_lowercase,
    )?;

    // Chance to add randomised weapon enhancement
    if details.is_pmc && get_chance_100(ctx.weapon_has_enhancement_chance_percent) {
        // Add buff to weapon root
        add_buff(ctx.repair_kit_weapon, &mut weapon_with_mods[0])?;
    }

    // Add mods to weapon base
    if bot_template_inventory.mods.contains_key(weapon_tpl) {
        // Role to treat bot as e.g. pmc/scav/boss
        let bot_equipment_role = get_bot_equipment_role(&details.role_lowercase).to_owned();

        // Different limits if bot is boss vs scav
        let mod_limits = get_weapon_mod_limits(ctx, &bot_equipment_role)?;

        let mut request = GenerateWeaponRequestWire {
            // Weapon root id
            weapon_id: weapon_with_mods[0].id.clone(),
            // Will become hydrated array of weapon + mods
            weapon: weapon_with_mods,
            // Moved out of the bot's template, not cloned: C# aliases `botTemplateInventory.Mods`
            // here and `BotEquipmentModGenerator.cs:723/:735` write through the alias, so the
            // cached sub-mod pools have to survive into the bot's *next* weapon.
            mod_pool: std::mem::take(&mut bot_template_inventory.mods),
            parent_template: weapon_tpl.to_owned(),
            // Moved out of the caller's map for the same reason `mod_pool` is: C# aliases
            // `equipmentChances.WeaponModsChances` here and `AdjustSlotSpawnChances`
            // (`BotEquipmentModGenerator.cs:855`) writes through the alias, so a forced slot
            // chance has to survive into the bot's *next* weapon.
            mod_spawn_chances: std::mem::take(mod_chances),
            ammo_tpl: ammo_tpl.clone(),
            bot_data: BotDataWire {
                role: details.role_lowercase.clone(),
                level: details.bot_level,
                equipment_role: bot_equipment_role,
            },
            mod_limits,
            ..Default::default()
        };
        let outcome = generate_mods_for_weapon(ctx, &mut request);
        // Hand the pool and the chances back before propagating: the C# writes are already in the
        // caller's objects by the time it throws.
        bot_template_inventory.mods = std::mem::take(&mut request.mod_pool);
        *mod_chances = std::mem::take(&mut request.mod_spawn_chances);
        outcome?;

        weapon_with_mods = request.weapon;
    }

    // Use weapon preset from globals.json if weapon isn't valid
    if !is_weapon_valid(ctx, &weapon_with_mods, &details.role_lowercase) {
        // Weapon is bad, fall back to weapons preset
        weapon_with_mods = get_preset_weapon_mods(
            ctx,
            weapon_tpl,
            slot_name,
            weapon_parent_id,
            weapon_item_template,
            &details.role_lowercase,
        )?;
    }

    // Fill existing magazines to full and sync ammo type. C# clones the magazine items first and
    // puts the clones back into the list; the clone carries the same id and values, so the only
    // observable effect is the copy itself.
    let magazines: Vec<Item> = weapon_with_mods
        .iter()
        .filter(|item| item.slot_id.as_deref() == Some(MOD_MAGAZINE_SLOT_ID))
        .cloned()
        .collect();
    for magazine in magazines {
        fill_existing_magazines(ctx, &mut weapon_with_mods, &magazine, &ammo_tpl)?;
    }

    // Add cartridge(s) to gun chamber(s)
    let Some(chambers) = weapon_item_template.chambers.as_deref() else {
        // `(Properties?.Chambers).Any()` on a null list (`:212`)
        return Err(LootError::new(format!(
            "Value cannot be null: chambers of weapon: {weapon_tpl}"
        )));
    };
    if !chambers.is_empty() {
        let Some(first_chamber_filter) = chambers[0].filter.as_deref() else {
            // `.FirstOrDefault().Properties.Filters.FirstOrDefault().Filter` (`:213`)
            return Err(LootError::new(format!(
                "Object reference not set to an instance of an object: chamber filter of weapon: {weapon_tpl}"
            )));
        };

        if first_chamber_filter.contains(&ammo_tpl) {
            // Guns have variety of possible Chamber ids, patron_in_weapon/patron_in_weapon_000/…
            let chamber_slot_names: Vec<String> = chambers
                .iter()
                .map(|chamber| chamber.name.clone().unwrap_or_default())
                .collect();
            add_cartridge_to_chamber(&mut weapon_with_mods, &ammo_tpl, &chamber_slot_names)?;
        }
    }

    // Fill UBGL if found
    let ubgl_mod = weapon_with_mods
        .iter()
        .find(|item| item.slot_id.as_deref() == Some("mod_launcher"))
        .cloned();
    let mut ubgl_ammo_tpl = None;
    if let Some(ubgl_mod) = ubgl_mod {
        let Some(ubgl_template) = get_item(items, &ubgl_mod.template) else {
            return Err(LootError::new(format!(
                "Object reference not set to an instance of an object: UBGL: {} is missing from the database",
                ubgl_mod.template
            )));
        };
        // `MongoId?` assigned from a non-nullable `MongoId` is never null, so the `:230` guard
        // always passes — an unresolvable ammo lands on `MongoId.Empty` and is still filled.
        let chosen = get_weighted_compatible_ammo(
            ctx,
            &bot_template_inventory.ammo,
            &ubgl_mod.template,
            ubgl_template,
        )?;
        fill_ubgl(&mut weapon_with_mods, &ubgl_mod, &chosen);
        ubgl_ammo_tpl = Some(chosen);
    }

    Ok(Some(GenerateWeaponResultWire {
        weapon: weapon_with_mods,
        chosen_ammo_template: ammo_tpl,
        chosen_ubgl_ammo_template: ubgl_ammo_tpl,
        weapon_template: weapon_tpl.to_owned(),
    }))
}

/// `Services/Bot/BotWeaponModLimitService.GetWeaponModLimits` (`:22-40`).
///
/// # Errors
///
/// The `botConfig.Equipment[botRole]` indexer, a `KeyNotFoundException` for an unconfigured role.
fn get_weapon_mod_limits(ctx: &BotContext, bot_role: &str) -> Result<BotModLimitsWire, LootError> {
    let Some(equipment) = ctx.equipment.get(bot_role) else {
        return Err(LootError::new(format!(
            "The given key '{bot_role}' was not present in the dictionary."
        )));
    };
    let limits = equipment.weapon_mod_limits.as_ref();

    Ok(BotModLimitsWire {
        scope: ItemCountWire { count: Some(0) },
        scope_max: limits.and_then(|limits| limits.scope_limit),
        scope_base_types: [
            OPTIC_SCOPE,
            ASSAULT_SCOPE,
            COLLIMATOR,
            COMPACT_COLLIMATOR,
            SPECIAL_SCOPE,
        ]
        .map(str::to_owned)
        .to_vec(),
        flashlight_laser: ItemCountWire { count: Some(0) },
        flashlight_laser_max: limits.and_then(|limits| limits.light_laser_limit),
        flashlight_laser_base_types: [TACTICAL_COMBO, FLASHLIGHT, PORTABLE_RANGE_FINDER]
            .map(str::to_owned)
            .to_vec(),
    })
}

/// `BotWeaponGenerator.AddCartridgeToChamber` (`:253-279`).
///
/// # Errors
///
/// The `weaponWithModsList[0]` index at `:266`, reached when a preset fallback found no preset and
/// left the weapon list empty.
fn add_cartridge_to_chamber(
    weapon_with_mods_list: &mut Vec<Item>,
    ammo_template: &str,
    chamber_slot_ids: &[String],
) -> Result<(), LootError> {
    for slot_id in chamber_slot_ids {
        let Some(root_id) = weapon_with_mods_list.first().map(|item| item.id.clone()) else {
            return Err(LootError::new(
                "Index was out of range: weapon has no root item to chamber a round in",
            ));
        };
        let existing = weapon_with_mods_list
            .iter_mut()
            .find(|item| item.slot_id.as_deref() == Some(slot_id.as_str()));

        match existing {
            // Already exists, update values
            Some(existing) => {
                existing.template = ammo_template.to_owned();
                existing.upd = Some(Upd {
                    stack_objects_count: Some(1.0),
                    ..Default::default()
                });
            }
            // Not found, add new slot to weapon
            None => weapon_with_mods_list.push(Item {
                id: mongo_id::generate(),
                template: ammo_template.to_owned(),
                parent_id: Some(root_id),
                slot_id: Some(slot_id.clone()),
                upd: Some(Upd {
                    stack_objects_count: Some(1.0),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        }
    }

    Ok(())
}

/// `BotWeaponGenerator.ConstructWeaponBaseList` (`:291-310`).
///
/// # Errors
///
/// From `GenerateExtraPropertiesForItem`.
fn construct_weapon_base_list(
    ctx: &BotContext,
    weapon_template: &str,
    weapon_parent_id: &str,
    equipment_slot: &str,
    weapon_item_template: &ItemView,
    bot_role: &str,
) -> Result<Vec<Item>, LootError> {
    Ok(vec![Item {
        id: mongo_id::generate(),
        template: weapon_template.to_owned(),
        parent_id: Some(weapon_parent_id.to_owned()),
        slot_id: Some(equipment_slot.to_owned()),
        upd: generate_extra_properties_for_item(ctx, weapon_item_template, Some(bot_role), false)?,
        ..Default::default()
    }])
}

/// `BotWeaponGenerator.GetPresetWeaponMods` (`:321-362`) — an empty list where the C# finds no
/// preset.
///
/// # Errors
///
/// The `itemPreset.Items[0]` index at `:339`, applied to **every** preset in globals while
/// searching, and `GenerateExtraPropertiesForItem`'s own errors.
fn get_preset_weapon_mods(
    ctx: &mut BotContext,
    weapon_template: &str,
    equipment_slot: &str,
    weapon_parent_id: &str,
    item_template: &ItemView,
    bot_role: &str,
) -> Result<Vec<Item>, LootError> {
    // Invalid weapon generated, fallback to preset
    let item_name = item_template.name.clone().unwrap_or_default();
    ctx.diagnostics.push(localised(
        WARNING,
        "bot-weapon_generated_incorrect_using_default",
        serde_json::Value::String(format!("{weapon_template} - {item_name}")),
    ));

    // TODO: Preset weapons trigger a lot of warnings regarding missing ammo in magazines & such
    let item_presets = ctx.item_presets;
    let mut preset = None;
    for item_preset in item_presets.values() {
        let Some(root) = item_preset.items.first() else {
            return Err(LootError::new(
                "Index was out of range: preset with no items",
            ));
        };

        if root.template == weapon_template {
            preset = Some(item_preset);

            break;
        }
    }

    let Some(preset) = preset else {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-missing_weapon_preset",
            serde_json::Value::String(weapon_template.to_owned()),
        ));

        return Ok(Vec::new());
    };

    let mut weapon_mods = preset.items.clone();
    let upd = generate_extra_properties_for_item(ctx, item_template, Some(bot_role), false)?;
    let parent_item = &mut weapon_mods[0];
    parent_item.parent_id = Some(weapon_parent_id.to_owned());
    parent_item.slot_id = Some(equipment_slot.to_owned());
    parent_item.upd = upd;

    Ok(weapon_mods)
}

/// `BotWeaponGenerator.IsWeaponValid` (`:370-416`).
///
/// **The `return true` at `:412` is inside the `foreach`**, so only the first item that declares
/// required slots is ever checked; anything after it is validated by assumption. Preserved.
fn is_weapon_valid(ctx: &mut BotContext, weapon_and_children: &[Item], bot_role: &str) -> bool {
    let items = ctx.items;

    for item in weapon_and_children {
        let mod_template = get_item(items, &item.template);

        // `!x ?? false`: a missing template leaves the lifted `!` null, which coalesces to false —
        // so it does *not* continue here, it falls through to the empty required-slot list below.
        if mod_template
            .and_then(|template| template.slots.as_ref())
            .is_some_and(Vec::is_empty)
        {
            continue;
        }

        let required_slots = mod_template
            .and_then(|template| template.slots.as_ref())
            .map(|slots| {
                slots
                    .iter()
                    .filter(|slot| slot.required.unwrap_or(false))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if required_slots.is_empty() {
            // No required slots, skip to next item in weapon
            continue;
        }

        for required_slot in required_slots {
            // Check if slot exists in cache
            let occupied = weapon_and_children.iter().any(|child| {
                child.parent_id.as_deref() == Some(item.id.as_str())
                    && child.slot_id.as_deref() == required_slot.name.as_deref()
            });
            if !occupied {
                let mod_name = mod_template
                    .and_then(|template| template.name.clone())
                    .unwrap_or_default();
                ctx.diagnostics.push(localised(
                    WARNING,
                    "bot-weapons_required_slot_missing_item",
                    serde_json::json!({
                        "modSlot": required_slot.name,
                        "modName": mod_name,
                        "slotId": item.slot_id,
                        "botRole": bot_role,
                    }),
                ));

                return false;
            }
        }

        return true;
    }

    true
}

/// `BotWeaponGenerator.AddExtraMagazinesToInventory` (`:427-474`).
///
/// # Errors
///
/// The `magazineTpl.Value` deref at `:439` when no magazine and no `DefMagType` could be found, the
/// unguarded mag-gen dispatch at `:464`, and whatever the dispatched implementation raises.
pub fn add_extra_magazines_to_inventory(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    generated_weapon_result: &GenerateWeaponResultWire,
    mag_weights: &GenerationDataWire,
    inventory: &mut Vec<Item>,
    bot_role: &str,
) -> Result<(), LootError> {
    let items = ctx.items;
    let weapon_and_mods = &generated_weapon_result.weapon;
    let weapon_template = &generated_weapon_result.weapon_template;
    let magazine_tpl =
        get_magazine_template_from_weapon_template(ctx, weapon_and_mods, weapon_template, bot_role);

    let Some(magazine_tpl) = magazine_tpl else {
        return Err(LootError::new(format!(
            "Nullable object must have a value: magazine of weapon: {weapon_template}"
        )));
    };
    if get_item(items, &magazine_tpl).is_none() {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-unable_to_find_magazine_item",
            serde_json::Value::String(magazine_tpl.clone()),
        ));

        return Ok(());
    }

    let ammo_tpl = &generated_weapon_result.chosen_ammo_template;
    let Some(ammo_template) = get_item(items, ammo_tpl) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-unable_to_find_ammo_item",
            serde_json::Value::String(ammo_tpl.clone()),
        ));

        return Ok(());
    };
    let ammo_stack_max_size = ammo_template.stack_max_size.unwrap_or(0);

    // Has an UBGL
    if generated_weapon_result
        .chosen_ubgl_ammo_template
        .as_deref()
        .is_some_and(|ubgl_ammo| !ubgl_ammo.is_empty())
    {
        add_ubgl_grenades_to_bot_inventory(
            ctx,
            grids,
            weapon_and_mods,
            generated_weapon_result,
            inventory,
        )?;
    }

    process_mag_gen(
        ctx,
        grids,
        &MAG_GEN_ORDER,
        &InventoryMagGen {
            mag_counts: &mag_weights.weights,
            magazine_tpl: &magazine_tpl,
            weapon_tpl: weapon_template,
            ammo_tpl,
        },
        inventory,
    )?;

    // Add x stacks of bullets to SecuredContainer (bots use a magic mag packing skill to reload
    // instantly)
    let stack_count = ctx.secure_container_ammo_stack_count;
    add_ammo_to_secure_container(
        ctx,
        grids,
        stack_count,
        ammo_tpl,
        ammo_stack_max_size,
        inventory,
    );

    Ok(())
}

/// `BotWeaponGenerator.AddUbglGrenadesToBotInventory` (`:483-510`).
///
/// # Errors
///
/// The unguarded `ubglMod.Template` at `:492` — a result flagged as having UBGL ammo but no
/// `mod_launcher` item — and the dispatch at `:506`.
fn add_ubgl_grenades_to_bot_inventory(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    weapon_mods: &[Item],
    generated_weapon_result: &GenerateWeaponResultWire,
    inventory: &mut Vec<Item>,
) -> Result<(), LootError> {
    // Find ubgl mod item + get details of it from db
    let Some(ubgl_mod) = weapon_mods
        .iter()
        .find(|item| item.slot_id.as_deref() == Some("mod_launcher"))
    else {
        return Err(LootError::new(
            "Object reference not set to an instance of an object: weapon has UBGL ammo but no mod_launcher",
        ));
    };
    let ubgl_tpl = ubgl_mod.template.clone();

    // Define min/max of how many grenades bot will have
    let ubgl_min_max: IndexMap<String, f64> =
        IndexMap::from([("1".to_owned(), 1.0), ("2".to_owned(), 1.0)]);

    let ubgl_ammo_tpl = generated_weapon_result
        .chosen_ubgl_ammo_template
        .clone()
        .unwrap_or_default();

    // Add grenades to bot inventory
    process_mag_gen(
        ctx,
        grids,
        &MAG_GEN_ORDER,
        &InventoryMagGen {
            mag_counts: &ubgl_min_max,
            magazine_tpl: &ubgl_tpl,
            weapon_tpl: &ubgl_tpl,
            ammo_tpl: &ubgl_ammo_tpl,
        },
        inventory,
    )?;

    // Store extra grenades in secure container
    add_ammo_to_secure_container(ctx, grids, 5, &ubgl_ammo_tpl, 20, inventory);

    Ok(())
}

/// `BotWeaponGenerator.AddAmmoToSecureContainer` (`:520-542`).
fn add_ammo_to_secure_container(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    stack_count: i32,
    ammo_tpl: &str,
    stack_size: i32,
    inventory: &mut Vec<Item>,
) {
    let container = [SECURED_CONTAINER.to_owned()];
    for _ in 0..stack_count {
        let id = mongo_id::generate();
        let mut item = [Item {
            id: id.clone(),
            template: ammo_tpl.to_owned(),
            upd: Some(Upd {
                stack_objects_count: Some(f64::from(stack_size)),
                ..Default::default()
            }),
            ..Default::default()
        }];

        grids.add_item_with_children_to_equipment_slot(
            ctx, &container, &id, ammo_tpl, &mut item, inventory,
        );
    }
}

/// `BotWeaponGenerator.GetMagazineTemplateFromWeaponTemplate` (`:551-588`). `None` is the C# null:
/// no magazine on the weapon and no `DefMagType` to fall back to.
fn get_magazine_template_from_weapon_template(
    ctx: &mut BotContext,
    weapon_mods: &[Item],
    weapon_tpl: &str,
    bot_role: &str,
) -> Option<String> {
    let magazine = weapon_mods
        .iter()
        .find(|item| item.slot_id.as_deref() == Some(MOD_MAGAZINE_SLOT_ID));
    if let Some(magazine) = magazine {
        return Some(magazine.template.clone());
    }

    let items = ctx.items;
    let weapon_template = get_item(items, weapon_tpl);
    let default_mag_tpl_id = weapon_template.and_then(|weapon| weapon.def_mag_type.clone());

    // Edge case - magazineless chamber loaded weapons don't have magazines, e.g. mp18. Return the
    // default mag tpl
    if weapon_template.and_then(|weapon| weapon.reload_mode.as_deref()) == Some(ONLY_BARREL) {
        return default_mag_tpl_id;
    }

    // Log error if no magazine AND not a chamber loaded weapon (e.g. shotgun revolver). A null
    // `IsChamberLoad` leaves the lifted `!` null, which coalesces to false — no warning.
    if weapon_template.and_then(|weapon| weapon.is_chamber_load) == Some(false) {
        // Shouldn't happen
        ctx.diagnostics.push(localised(
            WARNING,
            "bot-weapon_missing_magazine_or_chamber",
            serde_json::json!({"weaponId": weapon_tpl, "botRole": bot_role}),
        ));
    }

    let weapon_name = weapon_template
        .and_then(|weapon| weapon.name.clone())
        .unwrap_or_default();
    let default_display = default_mag_tpl_id.clone().unwrap_or_default();
    ctx.diagnostics.push(plain(
        DEBUG,
        format!(
            "[{bot_role}] Unable to find magazine for weapon: {weapon_tpl} {weapon_name}, using mag template default: {default_display}."
        ),
    ));

    default_mag_tpl_id
}

/// `BotWeaponGenerator.GetWeightedCompatibleAmmo` (`:596-674`).
///
/// **Deviation:** takes the weapon's tpl alongside its row, because a flattened [`ItemView`] has no
/// id of its own and `weaponTemplate.Id` reaches a log line here.
///
/// # Errors
///
/// A null caliber, which C# hands to `Dictionary.TryGetValue` as a null key
/// (`ArgumentNullException`); the three unguarded `DefAmmo.Value` derefs at `:630`, `:651` and
/// `:669`; the `cartridgePool[magazineCaliberData]` indexer at `:656`; and the chamber-filter chain
/// at `:622`.
fn get_weighted_compatible_ammo(
    ctx: &mut BotContext,
    cartridge_pool: &IndexMap<String, IndexMap<String, f64>>,
    weapon_tpl: &str,
    weapon_template: &ItemView,
) -> Result<String, LootError> {
    let items = ctx.items;
    let weapon_name = weapon_template.name.clone().unwrap_or_default();

    let Some(desired_caliber) = get_weapon_caliber(ctx, weapon_tpl, weapon_template)? else {
        return Err(LootError::new(format!(
            "Value cannot be null: caliber of weapon: {weapon_tpl}"
        )));
    };

    let mut cartridge_pool_for_weapon = match cartridge_pool.get(&desired_caliber) {
        Some(pool) if !pool.is_empty() => pool,
        _ => {
            ctx.diagnostics.push(localised(
                DEBUG,
                "bot-no_caliber_data_for_weapon_falling_back_to_default",
                serde_json::json!({
                    "weaponId": weapon_tpl,
                    "weaponName": weapon_name,
                    "defaultAmmo": weapon_template.def_ammo,
                }),
            ));

            if let Some(def_ammo) = weapon_template.def_ammo.as_deref() {
                return Ok(def_ammo.to_owned());
            }

            // last ditch attempt to get default ammo tpl
            let first_chamber_filter = weapon_template
                .chambers
                .as_deref()
                .and_then(|chambers| chambers.first())
                .and_then(|chamber| chamber.filter.as_deref());
            let Some(first_chamber_filter) = first_chamber_filter else {
                return Err(LootError::new(format!(
                    "Object reference not set to an instance of an object: chamber filter of weapon: {weapon_tpl}"
                )));
            };

            // `FirstOrDefault` over an empty filter is `MongoId.Empty`
            return Ok(first_chamber_filter.first().cloned().unwrap_or_default());
        }
    };

    // Get cartridges the weapons first chamber allow
    let compatible_cartridges_in_template =
        get_compatible_cartridges_from_weapon_template(items, weapon_template);
    if compatible_cartridges_in_template.is_empty() {
        // No chamber data found in weapon, send default
        return def_ammo_or_error(weapon_tpl, weapon_template);
    }

    // Inner join the weapons allowed + passed in cartridge pool to get compatible cartridges
    let mut compatible_cartridges: IndexMap<String, f64> = IndexMap::new();
    for (tpl, weight) in cartridge_pool_for_weapon {
        if compatible_cartridges_in_template.contains(tpl) {
            compatible_cartridges.insert(tpl.clone(), *weight);
        }
    }

    // No cartridges found, try and get something that's compatible with the gun
    if compatible_cartridges.is_empty() {
        // Get cartridges from the weapons first magazine in filters
        let compatible_cartridges_in_magazine =
            get_compatible_cartridges_from_magazine_template(items, weapon_template);
        if compatible_cartridges_in_magazine.is_empty() {
            // No compatible cartridges found in magazine, use default
            return def_ammo_or_error(weapon_tpl, weapon_template);
        }

        // Get the caliber data from the first compatible round in the magazine
        let magazine_caliber_data = get_item(items, &compatible_cartridges_in_magazine[0])
            .and_then(|cartridge| cartridge.caliber.clone());
        let Some(magazine_caliber_data) = magazine_caliber_data else {
            return Err(LootError::new(format!(
                "Object reference not set to an instance of an object: caliber of cartridge: {}",
                compatible_cartridges_in_magazine[0]
            )));
        };
        let Some(pool) = cartridge_pool.get(&magazine_caliber_data) else {
            return Err(LootError::new(format!(
                "The given key '{magazine_caliber_data}' was not present in the dictionary."
            )));
        };
        cartridge_pool_for_weapon = pool;

        for (tpl, weight) in cartridge_pool_for_weapon {
            if compatible_cartridges_in_magazine.contains(tpl) {
                compatible_cartridges.insert(tpl.clone(), *weight);
            }
        }

        // Nothing found after also checking magazines, return default ammo
        if compatible_cartridges.is_empty() {
            return def_ammo_or_error(weapon_tpl, weapon_template);
        }
    }

    get_weighted_value(&compatible_cartridges)
}

/// The three `weaponTemplate.Properties.DefAmmo.Value` derefs (`:630`, `:651`, `:669`), each an
/// `InvalidOperationException` for a weapon that declares none.
fn def_ammo_or_error(weapon_tpl: &str, weapon_template: &ItemView) -> Result<String, LootError> {
    weapon_template.def_ammo.clone().ok_or_else(|| {
        LootError::new(format!(
            "Nullable object must have a value: DefAmmo of weapon: {weapon_tpl}"
        ))
    })
}

/// `BotWeaponGenerator.GetCompatibleCartridgesFromWeaponTemplate` (`:681-693`).
fn get_compatible_cartridges_from_weapon_template(
    items: &IndexMap<String, ItemView>,
    weapon_template: &ItemView,
) -> Vec<String> {
    let cartridges = weapon_template
        .chambers
        .as_deref()
        .and_then(|chambers| chambers.first())
        .and_then(|chamber| chamber.filter.clone());
    if let Some(cartridges) = cartridges {
        return cartridges;
    }

    // Fallback to the magazine if possible, e.g. for revolvers
    get_compatible_cartridges_from_magazine_template(items, weapon_template)
}

/// `BotWeaponGenerator.GetCompatibleCartridgesFromMagazineTemplate` (`:701-726`).
fn get_compatible_cartridges_from_magazine_template(
    items: &IndexMap<String, ItemView>,
    weapon_template: &ItemView,
) -> Vec<String> {
    // Get the first magazine's template from the weapon
    let magazine_slot = weapon_template.slots.as_deref().and_then(|slots| {
        slots
            .iter()
            .find(|slot| slot.name.as_deref() == Some(MOD_MAGAZINE_SLOT_ID))
    });
    let Some(magazine_slot) = magazine_slot else {
        return Vec::new();
    };

    let magazine_tpl = magazine_slot
        .filter
        .as_deref()
        .and_then(|filter| filter.first())
        .map_or("", String::as_str);
    let Some(magazine_template) = get_item(items, magazine_tpl) else {
        return Vec::new();
    };

    // Try to get cartridges from slots array first, if none found, try Cartridges array
    magazine_template
        .slots
        .as_deref()
        .and_then(|slots| slots.first())
        .and_then(|slot| slot.filter.clone())
        .or_else(|| {
            magazine_template
                .cartridges
                .as_deref()
                .and_then(|cartridges| cartridges.first())
                .and_then(|cartridge| cartridge.filter.clone())
        })
        .unwrap_or_default()
}

/// `BotWeaponGenerator.GetWeaponCaliber` (`:733-755`).
///
/// # Errors
///
/// The `Chambers.First().Properties.Filters.First()` chain at `:749`, reached only by a weapon that
/// declares a `LinkedWeapon` and neither caliber.
fn get_weapon_caliber(
    ctx: &BotContext,
    weapon_tpl: &str,
    weapon_template: &ItemView,
) -> Result<Option<String>, LootError> {
    if let Some(caliber) = weapon_template.caliber.as_deref().filter(|c| !c.is_empty()) {
        return Ok(Some(caliber.to_owned()));
    }

    if let Some(ammo_caliber) = weapon_template
        .ammo_caliber
        .as_deref()
        .filter(|c| !c.is_empty())
    {
        // 9x18pmm has a typo, should be Caliber9x18PM
        return Ok(Some(if ammo_caliber == "Caliber9x18PMM" {
            "Caliber9x18PM".to_owned()
        } else {
            ammo_caliber.to_owned()
        }));
    }

    if weapon_template
        .linked_weapon
        .as_deref()
        .is_some_and(|linked| !linked.is_empty())
    {
        let filter = weapon_template
            .chambers
            .as_deref()
            .and_then(|chambers| chambers.first())
            .and_then(|chamber| chamber.filter.as_deref());
        let Some(filter) = filter else {
            return Err(LootError::new(format!(
                "Sequence contains no elements: chambers of weapon: {weapon_tpl}"
            )));
        };

        let ammo_in_chamber = filter.first().map_or("", String::as_str);

        return Ok(get_item(ctx.items, ammo_in_chamber).and_then(|ammo| ammo.caliber.clone()));
    }

    Ok(None)
}

/// `BotWeaponGenerator.FillExistingMagazines` (`:763-787`).
///
/// # Errors
///
/// From `FillMagazineWithCartridge`: a cartridge with no `StackMaxSize`.
fn fill_existing_magazines(
    ctx: &mut BotContext,
    weapon_mods: &mut Vec<Item>,
    magazine: &Item,
    cartridge_template: &str,
) -> Result<(), LootError> {
    let items = ctx.items;
    let Some(magazine_template) = get_item(items, &magazine.template) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "bot-unable_to_find_magazine_item",
            serde_json::Value::String(magazine.template.clone()),
        ));

        return Ok(());
    };

    // Magazine, usually. **Deviation:** C# (`:774-779`) dereferences `parentDbItem.Name` unguarded
    // and NREs for a magazine whose parent tpl is missing from the database; an absent parent or
    // name is `""` here, which `MagazineIsCylinderRelated` answers false for — the same branch a
    // normal magazine takes.
    let parent_name = magazine_template
        .parent
        .as_deref()
        .and_then(|parent| get_item(items, parent))
        .and_then(|parent| parent.name.as_deref())
        .unwrap_or_default();

    // Revolver shotgun (MTs-255-12) uses a magazine with chambers, not cartridges ("camora_xxx")
    if magazine_is_cylinder_related(parent_name) {
        fill_camoras_with_ammo(weapon_mods, &magazine.id, cartridge_template);

        Ok(())
    } else {
        add_or_update_magazines_child_with_ammo(ctx, weapon_mods, magazine, cartridge_template)
    }
}

/// `BotWeaponGenerator.FillUbgl` (`:795-807`).
fn fill_ubgl(weapon_mods: &mut Vec<Item>, ubgl_mod: &Item, ubgl_ammo_tpl: &str) {
    weapon_mods.push(Item {
        id: mongo_id::generate(),
        template: ubgl_ammo_tpl.to_owned(),
        parent_id: Some(ubgl_mod.id.clone()),
        slot_id: Some("patron_in_weapon".to_owned()),
        upd: Some(Upd {
            stack_objects_count: Some(1.0),
            ..Default::default()
        }),
        ..Default::default()
    });
}

/// `BotWeaponGenerator.AddOrUpdateMagazinesChildWithAmmo` (`:816-849`).
///
/// # Errors
///
/// From `FillMagazineWithCartridge`.
fn add_or_update_magazines_child_with_ammo(
    ctx: &mut BotContext,
    weapon_with_mods: &mut Vec<Item>,
    magazine: &Item,
    chosen_ammo_tpl: &str,
) -> Result<(), LootError> {
    let items = ctx.items;

    // Delete the existing cartridge object and create fresh below. `FirstOrDefault` + `Remove`
    // (`:823-828`) drops exactly one stack, not every one — a magazine that somehow carries two
    // keeps the second.
    let existing_cartridge = weapon_with_mods.iter().position(|item| {
        item.parent_id.as_deref() == Some(magazine.id.as_str())
            && item.slot_id.as_deref() == Some("cartridges")
    });
    if let Some(existing_cartridge) = existing_cartridge {
        weapon_with_mods.remove(existing_cartridge);
    }

    // Create array with just magazine
    let mut magazine_with_cartridges = vec![magazine.clone()];

    // Add cartridges as children to above mag array
    fill_magazine_with_cartridge(
        items,
        &mut ctx.diagnostics,
        &mut magazine_with_cartridges,
        &magazine.template,
        chosen_ammo_tpl,
        1.0,
    )?;

    // Replace existing magazine with above array of mag + cartridge stacks
    let Some(magazine_index) = weapon_with_mods
        .iter()
        .position(|item| item.id == magazine.id)
    else {
        ctx.diagnostics.push(plain(
            ERROR,
            format!(
                "Unable to add cartridges: {chosen_ammo_tpl} to magazine: {} as none found",
                magazine.id
            ),
        ));

        return Ok(());
    };

    weapon_with_mods.remove(magazine_index);

    // Insert new mag at same index position original was
    weapon_with_mods.splice(magazine_index..magazine_index, magazine_with_cartridges);

    Ok(())
}

/// `BotWeaponGenerator.FillCamorasWithAmmo` (`:857-881`).
fn fill_camoras_with_ammo(weapon_mods: &mut [Item], magazine_id: &str, ammo_tpl: &str) {
    // For CylinderMagazine we exchange the ammo in the "camoras". **Deviation:** C# `:862` calls
    // `SlotId.StartsWith` unguarded and NREs on an item with a null `slotId`; such an item is
    // skipped here.
    for camora in weapon_mods.iter_mut().filter(|item| {
        item.parent_id.as_deref() == Some(magazine_id)
            && item
                .slot_id
                .as_deref()
                .is_some_and(|slot_id| slot_id.starts_with("camora"))
    }) {
        camora.template = ammo_tpl.to_owned();
        match camora.upd.as_mut() {
            Some(upd) => upd.stack_objects_count = Some(1.0),
            None => {
                camora.upd = Some(Upd {
                    stack_objects_count: Some(1.0),
                    ..Default::default()
                });
            }
        }
    }
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
    use crate::bot::repair_service::BonusSettings;
    use crate::loot::item_helper::LAUNCHER;
    use crate::loot::models::PresetView;
    use crate::loot::random_util::{TestSeedGuard, get_double};

    const SEED: u64 = 42;

    const MAGAZINE_PARENT: &str = "aaaaaaaaaaaaaaaaaaaaaaa1";
    /// A weapon with a **required** `mod_magazine` slot: bare, it never validates.
    const RIFLE: &str = "aaaaaaaaaaaaaaaaaaaaaaa2";
    /// The same weapon with the slot optional, so it validates without a preset.
    const RIFLE_LOOSE: &str = "aaaaaaaaaaaaaaaaaaaaaaa3";
    /// A rifle carrying an underbarrel grenade launcher, through its preset.
    const RIFLE_UBGL: &str = "aaaaaaaaaaaaaaaaaaaaaaa4";
    const MAG: &str = "aaaaaaaaaaaaaaaaaaaaaaa5";
    const AMMO_PS: &str = "aaaaaaaaaaaaaaaaaaaaaaa6";
    const AMMO_BP: &str = "aaaaaaaaaaaaaaaaaaaaaaa7";
    const UBGL: &str = "aaaaaaaaaaaaaaaaaaaaaaa8";
    const GRENADE: &str = "aaaaaaaaaaaaaaaaaaaaaaa9";
    /// A magazine with a **required** base slot and an optional extension slot, so the
    /// required-only pool `:735` caches differs from the full pool `:723` would.
    const MAG_MODULAR: &str = "aaaaaaaaaaaaaaaaaaaaaab3";
    const MAG_BASE: &str = "aaaaaaaaaaaaaaaaaaaaaab4";
    const MAG_EXTENSION: &str = "aaaaaaaaaaaaaaaaaaaaaab5";
    const VEST: &str = "aaaaaaaaaaaaaaaaaaaaaab1";
    const SECURE: &str = "aaaaaaaaaaaaaaaaaaaaaab2";

    const RIFLE_CALIBER: &str = "Caliber762x39";
    const UBGL_CALIBER: &str = "Caliber40x46";

    struct Fixture {
        items: IndexMap<String, ItemView>,
        bosses: Vec<String>,
        durability: BotDurability,
        equipment: IndexMap<String, EquipmentFilters>,
        randomization: IndexMap<String, RandomisedResourceDetails>,
        presets: IndexMap<String, PresetView>,
        repair_kit: BonusSettings,
        enhancement_chance: f64,
        secure_container_ammo_stack_count: i32,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                items: serde_json::from_value(json!({
                    MAGAZINE_PARENT: {"name": "Magazine"},
                    LAUNCHER: {"name": "Launcher"},
                    RIFLE: {
                        "name": "AK", "weapClass": "assaultRifle", "maxDurability": 100.0,
                        "caliber": RIFLE_CALIBER, "defAmmo": AMMO_PS, "defMagType": MAG,
                        "reloadMode": "ExternalMagazine", "isChamberLoad": false,
                        "width": 4, "height": 2,
                        "chambers": [{"name": "patron_in_weapon", "filter": [AMMO_PS, AMMO_BP]}],
                        "slots": [{"name": "mod_magazine", "required": true, "filter": [MAG]}],
                    },
                    RIFLE_LOOSE: {
                        "name": "AK-loose", "weapClass": "assaultRifle", "maxDurability": 100.0,
                        "caliber": RIFLE_CALIBER, "defAmmo": AMMO_PS, "defMagType": MAG,
                        "reloadMode": "ExternalMagazine", "isChamberLoad": false,
                        "width": 4, "height": 2,
                        "chambers": [{"name": "patron_in_weapon", "filter": [AMMO_PS, AMMO_BP]}],
                        "slots": [{"name": "mod_magazine", "required": false,
                                   "filter": [MAG, MAG_MODULAR]}],
                    },
                    RIFLE_UBGL: {
                        "name": "AK-ubgl", "weapClass": "assaultRifle", "maxDurability": 100.0,
                        "caliber": RIFLE_CALIBER, "defAmmo": AMMO_PS, "defMagType": MAG,
                        "reloadMode": "ExternalMagazine", "isChamberLoad": false,
                        "width": 4, "height": 2,
                        "chambers": [{"name": "patron_in_weapon", "filter": [AMMO_PS, AMMO_BP]}],
                        "slots": [{"name": "mod_magazine", "required": true, "filter": [MAG]},
                                  {"name": "mod_launcher", "filter": [UBGL]}],
                    },
                    MAG: {"name": "30-round mag", "parent": MAGAZINE_PARENT,
                          "cartridgesMaxCount": 30, "cartridgesFirstFilter": [AMMO_PS],
                          "reloadMagType": "ExternalMagazine", "width": 1, "height": 1,
                          "cartridges": [{"name": "cartridges", "filter": [AMMO_PS, AMMO_BP]}]},
                    MAG_MODULAR: {"name": "modular mag", "parent": MAGAZINE_PARENT,
                        "cartridgesMaxCount": 30, "reloadMagType": "ExternalMagazine",
                        "width": 1, "height": 1,
                        "cartridges": [{"name": "cartridges", "filter": [AMMO_PS, AMMO_BP]}],
                        "slots": [
                            {"name": "mod_mag_base", "required": true, "filter": [MAG_BASE]},
                            {"name": "mod_magazine_extension", "filter": [MAG_EXTENSION]},
                        ]},
                    MAG_BASE: {"name": "mag base"},
                    MAG_EXTENSION: {"name": "mag extension"},
                    AMMO_PS: {"name": "PS", "caliber": RIFLE_CALIBER, "stackMaxSize": 60,
                              "width": 1, "height": 1},
                    AMMO_BP: {"name": "BP", "caliber": RIFLE_CALIBER, "stackMaxSize": 60,
                              "width": 1, "height": 1},
                    UBGL: {"name": "GP-25", "parent": LAUNCHER, "caliber": UBGL_CALIBER,
                           "defAmmo": GRENADE, "cartridgesMaxCount": 1,
                           "reloadMode": "ExternalMagazine"},
                    GRENADE: {"name": "VOG-25", "caliber": UBGL_CALIBER, "stackMaxSize": 1,
                              "stackMinRandom": 1, "stackMaxRandom": 1, "width": 1, "height": 1},
                    VEST: {"grids": [{"name": "main", "cellsH": 4, "cellsV": 4}]},
                    SECURE: {"grids": [{"name": "secure", "cellsH": 3, "cellsV": 3}]},
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
                    "assault": {"weaponModLimits": {"scopeLimit": 2, "lightLaserLimit": 1}},
                    "pmc": {"weaponModLimits": {"scopeLimit": 1}},
                }))
                .unwrap(),
                randomization: IndexMap::new(),
                presets: serde_json::from_value(json!({
                    "p_rifle": {"id": "p_rifle", "items": [
                        {"_id": "preset_root", "_tpl": RIFLE},
                        {"_id": "preset_mag", "_tpl": MAG, "parentId": "preset_root",
                         "slotId": "mod_magazine"},
                    ]},
                    "p_ubgl": {"id": "p_ubgl", "items": [
                        {"_id": "preset_root", "_tpl": RIFLE_UBGL},
                        {"_id": "preset_mag", "_tpl": MAG, "parentId": "preset_root",
                         "slotId": "mod_magazine"},
                        {"_id": "preset_ubgl", "_tpl": UBGL, "parentId": "preset_root",
                         "slotId": "mod_launcher"},
                    ]},
                }))
                .unwrap(),
                repair_kit: serde_json::from_value(json!({
                    "rarityWeight": {"Common": 1},
                    "bonusTypeWeight": {"DamageReduction": 1},
                    "Common": {"DamageReduction": {"valuesMinMax": {"min": 1.0, "max": 2.0},
                        "activeDurabilityPercentMinMax": {"min": 20, "max": 30}}},
                    "Rare": {},
                }))
                .unwrap(),
                enhancement_chance: 0.0,
                secure_container_ammo_stack_count: 0,
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
                default_presets_by_tpl: &crate::bot::NO_DEFAULT_PRESETS,
                item_presets: &self.presets,
                equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                low_profile_gas_block_tpls: &crate::bot::NO_BLACKLIST,
                weapon_has_enhancement_chance_percent: self.enhancement_chance,
                repair_kit_weapon: &self.repair_kit,
                secure_container_ammo_stack_count: self.secure_container_ammo_stack_count,
                mod_pool_slot_order: &crate::bot::NO_MOD_POOL_ORDER,
                is_night_time: false,
                diagnostics: Vec::new(),
            }
        }
    }

    /// A bot template whose only ammo is the rifle+UBGL calibers and whose mod pool is empty, so
    /// `GenerateModsForWeapon` is skipped and the preset fallback is what dresses the weapon.
    fn bot_template() -> BotTypeInventoryWire {
        serde_json::from_value(json!({
            "equipment": {"FirstPrimaryWeapon": {RIFLE: 1}},
            "Ammo": {RIFLE_CALIBER: {AMMO_PS: 1, AMMO_BP: 1}, UBGL_CALIBER: {GRENADE: 1}},
            "items": {}, "mods": {},
        }))
        .unwrap()
    }

    fn details(role: &str, is_pmc: bool) -> BotGenerationDetailsWire {
        serde_json::from_value(json!({
            "role": role, "roleLowercase": role, "side": "Savage", "botLevel": 15,
            "isPmc": is_pmc, "isPlayerScav": false, "gameVersion": "standard",
            "location": "bigmap", "botDifficulty": "normal",
            "clearBotContainerCacheAfterGeneration": true,
        }))
        .unwrap()
    }

    fn no_chances() -> IndexMap<String, f64> {
        IndexMap::new()
    }

    fn item(id: &str, tpl: &str, parent: Option<&str>, slot: Option<&str>) -> Item {
        serde_json::from_value(json!({"_id": id, "_tpl": tpl, "parentId": parent, "slotId": slot}))
            .unwrap()
    }

    /// Every item as `(tpl, parentId, slotId)`, which is the shape that does not depend on the
    /// generated `MongoId`s.
    fn shape(items: &[Item]) -> Vec<(&str, Option<&str>, Option<&str>)> {
        items
            .iter()
            .map(|item| {
                (
                    item.template.as_str(),
                    item.parent_id.as_deref(),
                    item.slot_id.as_deref(),
                )
            })
            .collect()
    }

    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    fn bot_with_containers(ctx: &BotContext, vest_tpl: &str) -> (ContainerGrids, Vec<Item>) {
        let vest = item("vest1", vest_tpl, None, Some("TacticalVest"));
        let secure = item("secure1", SECURE, None, Some("SecuredContainer"));
        let mut grids = ContainerGrids::default();
        grids.add_empty_container(ctx, "TacticalVest", &vest);
        grids.add_empty_container(ctx, "SecuredContainer", &secure);

        (grids, vec![vest, secure])
    }

    // -----------------------------------------------------------------------
    // generate_weapon_by_tpl
    // -----------------------------------------------------------------------

    /// A weapon whose required magazine slot nothing filled falls back to its preset, which is then
    /// loaded: magazine filled to capacity, one round chambered.
    #[test]
    fn a_seeded_weapon_is_a_preset_with_a_full_magazine_and_a_chambered_round() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let _guard = TestSeedGuard::install(SEED);

        let result = generate_weapon_by_tpl(
            &mut ctx,
            RIFLE,
            "FirstPrimaryWeapon",
            &mut bot_template(),
            "equipment-id",
            &mut no_chances(),
            &details("assault", false),
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.weapon_template, RIFLE);
        assert_eq!(result.chosen_ammo_template, AMMO_PS);
        assert_eq!(result.chosen_ubgl_ammo_template, None);

        assert_eq!(
            shape(&result.weapon),
            vec![
                (RIFLE, Some("equipment-id"), Some("FirstPrimaryWeapon")),
                (MAG, Some("preset_root"), Some("mod_magazine")),
                (AMMO_PS, Some("preset_mag"), Some("cartridges")),
                (AMMO_PS, Some("preset_root"), Some("patron_in_weapon")),
            ]
        );
        // The preset's own ids survive the clone; only the two added items are fresh.
        assert_eq!(result.weapon[0].id, "preset_root");
        assert_eq!(result.weapon[1].id, "preset_mag");
        assert_eq!(result.weapon[2].id.len(), 24);
        assert_eq!(result.weapon[3].id.len(), 24);

        // Magazine filled to its 30-round capacity, chamber holding exactly one.
        assert_eq!(
            result.weapon[2]
                .upd
                .as_ref()
                .and_then(|upd| upd.stack_objects_count),
            Some(30.0)
        );
        assert_eq!(
            result.weapon[3]
                .upd
                .as_ref()
                .and_then(|upd| upd.stack_objects_count),
            Some(1.0)
        );

        // The preset root carries the durability rolled for it, not the discarded bare weapon's.
        assert_eq!(
            serde_json::to_value(result.weapon[0].upd.as_ref().unwrap()).unwrap(),
            json!({"Repairable": {"Durability": 88.0, "MaxDurability": 96.0}})
        );

        assert_eq!(
            ctx.diagnostics
                .iter()
                .filter_map(|diagnostic| diagnostic.locale_key.as_deref())
                .collect::<Vec<_>>(),
            vec![
                "bot-weapons_required_slot_missing_item",
                "bot-weapon_generated_incorrect_using_default"
            ]
        );
    }

    #[test]
    fn a_valid_weapon_keeps_its_own_root_and_never_reaches_a_preset() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let _guard = TestSeedGuard::install(SEED);

        let result = generate_weapon_by_tpl(
            &mut ctx,
            RIFLE_LOOSE,
            "Holster",
            &mut bot_template(),
            "equipment-id",
            &mut no_chances(),
            &details("assault", false),
        )
        .unwrap()
        .unwrap();

        // No magazine to fill, but the chamber is still loaded.
        assert_eq!(
            shape(&result.weapon),
            vec![
                (RIFLE_LOOSE, Some("equipment-id"), Some("Holster")),
                (
                    AMMO_PS,
                    Some(result.weapon[0].id.as_str()),
                    Some("patron_in_weapon")
                ),
            ]
        );
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn a_weapon_tpl_missing_from_the_database_is_the_null_return() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let result = generate_weapon_by_tpl(
            &mut ctx,
            "cccccccccccccccccccccccc",
            "Holster",
            &mut bot_template(),
            "equipment-id",
            &mut no_chances(),
            &details("assault", false),
        )
        .unwrap();

        assert!(result.is_none());
        assert_eq!(
            ctx.diagnostics[0].locale_key.as_deref(),
            Some("bot-missing_item_template")
        );
        assert_eq!(
            ctx.diagnostics[1].message.as_deref(),
            Some("WeaponSlot -> Holster")
        );
    }

    // -----------------------------------------------------------------------
    // The enhancement gate (`:154`)
    // -----------------------------------------------------------------------

    #[test]
    fn a_pmc_weapon_is_buffed_at_a_hundred_percent() {
        let mut fixture = Fixture::new();
        fixture.enhancement_chance = 100.0;
        let mut ctx = fixture.ctx();
        let _guard = TestSeedGuard::install(SEED);

        let result = generate_weapon_by_tpl(
            &mut ctx,
            RIFLE_LOOSE,
            "Holster",
            &mut bot_template(),
            "equipment-id",
            &mut no_chances(),
            &details("pmcUSEC", true),
        )
        .unwrap()
        .unwrap();

        let upd = serde_json::to_value(result.weapon[0].upd.as_ref().unwrap()).unwrap();
        assert!(upd.get("Buff").is_some(), "{upd}");
    }

    /// The `&&` short-circuits on `IsPmc`, never on the roll, so a 0% chance still moves the
    /// stream — and a non-PMC does not.
    #[test]
    fn the_enhancement_roll_is_consumed_at_zero_percent_and_skipped_for_a_scav() {
        let mut fixture = Fixture::new();
        fixture.enhancement_chance = 0.0;
        let ctx_source = |fixture: &Fixture, is_pmc: bool, role: &str| {
            let mut ctx = fixture.ctx();
            generate_weapon_by_tpl(
                &mut ctx,
                RIFLE_LOOSE,
                "Holster",
                &mut bot_template(),
                "equipment-id",
                &mut no_chances(),
                &details(role, is_pmc),
            )
            .unwrap()
            .unwrap()
        };

        let pmc = {
            let result = ctx_source(&fixture, true, "pmcUSEC");
            let upd = serde_json::to_value(result.weapon[0].upd.as_ref().unwrap()).unwrap();
            assert!(upd.get("Buff").is_none(), "{upd}");
            stream_position_after(|| {
                ctx_source(&fixture, true, "pmcUSEC");
            })
        };
        let scav = stream_position_after(|| {
            ctx_source(&fixture, false, "assault");
        });

        // Both bots draw the same two durability values and the same ammo; only the PMC also burns
        // the enhancement roll, so the streams cannot agree.
        assert_ne!(pmc, scav);
        assert_eq!(
            pmc,
            stream_position_after(|| {
                ctx_source(&fixture, false, "assault");
                get_chance_100(0.0);
            })
        );
    }

    // -----------------------------------------------------------------------
    // is_weapon_valid (`:370-416`)
    // -----------------------------------------------------------------------

    /// **`return true` sits inside the `foreach` at `:412`**: the first item with required slots
    /// decides for the whole weapon, so a second item missing one is never looked at.
    #[test]
    fn only_the_first_item_with_required_slots_is_validated() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let satisfied = item("w1", RIFLE, None, Some("FirstPrimaryWeapon"));
        let magazine = item("m1", MAG, Some("w1"), Some("mod_magazine"));
        let starved = item("w2", RIFLE, None, Some("Holster"));

        // Satisfied first: the starved second weapon is never reached.
        assert!(is_weapon_valid(
            &mut ctx,
            &[satisfied.clone(), magazine.clone(), starved.clone()],
            "assault"
        ));
        assert!(ctx.diagnostics.is_empty());

        // Starved first: the same list in the other order fails.
        assert!(!is_weapon_valid(
            &mut ctx,
            &[starved, satisfied, magazine],
            "assault"
        ));
        assert_eq!(
            ctx.diagnostics[0].locale_key.as_deref(),
            Some("bot-weapons_required_slot_missing_item")
        );
    }

    // -----------------------------------------------------------------------
    // add_extra_magazines_to_inventory
    // -----------------------------------------------------------------------

    fn magazine_weights(count: &str) -> GenerationDataWire {
        serde_json::from_value(json!({"weights": {count: 1}, "whitelist": {}})).unwrap()
    }

    #[test]
    fn spare_magazines_and_secure_container_ammo_land_in_the_bots_containers() {
        let mut fixture = Fixture::new();
        fixture.secure_container_ammo_stack_count = 2;
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_containers(&ctx, VEST);
        let _guard = TestSeedGuard::install(SEED);

        let generated = generate_weapon_by_tpl(
            &mut ctx,
            RIFLE,
            "FirstPrimaryWeapon",
            &mut bot_template(),
            "equipment-id",
            &mut no_chances(),
            &details("assault", false),
        )
        .unwrap()
        .unwrap();

        add_extra_magazines_to_inventory(
            &mut ctx,
            &mut grids,
            &generated,
            &magazine_weights("2"),
            &mut inventory,
            "assault",
        )
        .unwrap();

        // Two spare magazines in the vest, each with its cartridge stack.
        let magazines: Vec<&Item> = inventory
            .iter()
            .filter(|item| item.template == MAG)
            .collect();
        assert_eq!(magazines.len(), 2);
        assert!(magazines.iter().all(|magazine| {
            magazine.parent_id.as_deref() == Some("vest1")
                && magazine.slot_id.as_deref() == Some("main")
        }));

        // Two full stacks of the chosen ammo in the secure container.
        let secure: Vec<&Item> = inventory
            .iter()
            .filter(|item| item.parent_id.as_deref() == Some("secure1"))
            .collect();
        assert_eq!(secure.len(), 2);
        assert!(secure.iter().all(|stack| {
            stack.template == generated.chosen_ammo_template
                && stack.upd.as_ref().and_then(|upd| upd.stack_objects_count) == Some(60.0)
        }));
    }

    /// A vest with one free cell takes one magazine; the weapon is already on its default magazine,
    /// so `ExternalInventoryMagGen.cs:92` breaks out instead of retrying.
    #[test]
    fn a_full_vest_stops_the_magazine_loop_cleanly() {
        let mut fixture = Fixture::new();
        fixture.items.insert(
            "tinyvest".to_owned(),
            serde_json::from_value(json!({"grids": [{"name": "main", "cellsH": 1, "cellsV": 1}]}))
                .unwrap(),
        );
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_containers(&ctx, "tinyvest");
        let _guard = TestSeedGuard::install(SEED);

        let generated = generate_weapon_by_tpl(
            &mut ctx,
            RIFLE,
            "FirstPrimaryWeapon",
            &mut bot_template(),
            "equipment-id",
            &mut no_chances(),
            &details("assault", false),
        )
        .unwrap()
        .unwrap();

        add_extra_magazines_to_inventory(
            &mut ctx,
            &mut grids,
            &generated,
            &magazine_weights("4"),
            &mut inventory,
            "assault",
        )
        .unwrap();

        assert_eq!(
            inventory.iter().filter(|item| item.template == MAG).count(),
            1
        );
    }

    // -----------------------------------------------------------------------
    // Mod pool aliasing (`BotEquipmentModGenerator.cs:723/:735`)
    // -----------------------------------------------------------------------

    /// A bot template whose mod pool has the weapon, so `GenerateModsForWeapon` runs and its
    /// sub-mod caching writes land somewhere observable.
    fn bot_template_with_mods() -> BotTypeInventoryWire {
        serde_json::from_value(json!({
            "equipment": {"FirstPrimaryWeapon": {RIFLE_LOOSE: 1}},
            "Ammo": {RIFLE_CALIBER: {AMMO_PS: 1, AMMO_BP: 1}, UBGL_CALIBER: {GRENADE: 1}},
            "items": {},
            "mods": {RIFLE_LOOSE: {"mod_magazine": [MAG_MODULAR]}},
        }))
        .unwrap()
    }

    fn mod_chances() -> IndexMap<String, f64> {
        IndexMap::from([
            ("mod_magazine".to_owned(), 100.0),
            ("mod_mag_base".to_owned(), 100.0),
            ("mod_magazine_extension".to_owned(), 100.0),
        ])
    }

    fn generate_into(
        ctx: &mut BotContext,
        template: &mut BotTypeInventoryWire,
    ) -> GenerateWeaponResultWire {
        generate_weapon_by_tpl(
            ctx,
            RIFLE_LOOSE,
            "FirstPrimaryWeapon",
            template,
            "equipment-id",
            &mut mod_chances(),
            &details("assault", false),
        )
        .unwrap()
        .unwrap()
    }

    /// C# aliases `botTemplateInventory.Mods` into `GenerateWeaponRequest.ModPool`, and
    /// `:735` writes the required-mod pool of every sub-mod it adds back into it. The template is
    /// cloned **per bot** (`BotGenerator.cs:147`), not per weapon, so those writes have to still be
    /// there for the bot's next weapon.
    #[test]
    fn sub_mod_pool_writes_reach_the_callers_template() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut template = bot_template_with_mods();
        let _guard = TestSeedGuard::install(SEED);

        let weapon = generate_into(&mut ctx, &mut template);

        // The magazine was added, and its required base slot filled from the cached pool.
        assert!(
            weapon
                .weapon
                .iter()
                .any(|item| item.template == MAG_MODULAR)
        );
        assert!(weapon.weapon.iter().any(|item| item.template == MAG_BASE));

        // `:735` cached the magazine's *required-only* pool into the bot's own template.
        let cached = template
            .mods
            .get(MAG_MODULAR)
            .expect("magazine pool written back into the caller's template");
        assert_eq!(cached.keys().collect::<Vec<_>>(), vec!["mod_mag_base"]);
        assert_eq!(
            cached["mod_mag_base"].iter().collect::<Vec<_>>(),
            vec![MAG_BASE]
        );

        // And the weapon's own entry is still there — the take/restore is not a swap.
        assert!(template.mods.contains_key(RIFLE_LOOSE));
    }

    /// The next weapon reads what the last one cached instead of recomputing it: seed the pool with
    /// the *full* sub-mod pool `:723` would have written and the optional extension slot, which the
    /// required-only fallback never yields, gets filled.
    #[test]
    fn a_cached_sub_mod_pool_is_what_the_next_weapon_uses() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut template = bot_template_with_mods();
        template.mods.insert(
            MAG_MODULAR.to_owned(),
            serde_json::from_value(json!({
                "mod_mag_base": [MAG_BASE],
                "mod_magazine_extension": [MAG_EXTENSION],
            }))
            .unwrap(),
        );
        let _guard = TestSeedGuard::install(SEED);

        let weapon = generate_into(&mut ctx, &mut template);

        assert!(
            weapon
                .weapon
                .iter()
                .any(|item| item.template == MAG_EXTENSION),
            "the cached pool's optional slot was ignored: {:?}",
            shape(&weapon.weapon)
        );
        // The cached entry is left as it was found, not overwritten by the fallback.
        assert_eq!(
            template.mods[MAG_MODULAR].keys().collect::<Vec<_>>(),
            vec!["mod_mag_base", "mod_magazine_extension"]
        );
    }

    // -----------------------------------------------------------------------
    // The UBGL path
    // -----------------------------------------------------------------------

    #[test]
    fn a_launcher_gets_its_own_ammo_and_a_pocketful_of_grenades() {
        let mut fixture = Fixture::new();
        fixture.secure_container_ammo_stack_count = 0;
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_containers(&ctx, VEST);
        let _guard = TestSeedGuard::install(SEED);

        let generated = generate_weapon_by_tpl(
            &mut ctx,
            RIFLE_UBGL,
            "FirstPrimaryWeapon",
            &mut bot_template(),
            "equipment-id",
            &mut no_chances(),
            &details("assault", false),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            generated.chosen_ubgl_ammo_template.as_deref(),
            Some(GRENADE)
        );
        // The grenade is chambered in the launcher, not in the rifle.
        let chambered = generated
            .weapon
            .iter()
            .find(|item| item.template == GRENADE)
            .unwrap();
        assert_eq!(chambered.parent_id.as_deref(), Some("preset_ubgl"));
        assert_eq!(chambered.slot_id.as_deref(), Some("patron_in_weapon"));

        add_extra_magazines_to_inventory(
            &mut ctx,
            &mut grids,
            &generated,
            &magazine_weights("1"),
            &mut inventory,
            "assault",
        )
        .unwrap();

        // `AddUbglGrenadesToBotInventory` adds 1-2 grenades to the vest ahead of the magazines, and
        // 5 stacks of 20 to the secure container.
        let vest_grenades = inventory
            .iter()
            .filter(|item| item.template == GRENADE && item.parent_id.as_deref() == Some("vest1"))
            .count();
        assert!((1..=2).contains(&vest_grenades), "{vest_grenades}");
        assert_eq!(
            inventory
                .iter()
                .filter(
                    |item| item.template == GRENADE && item.parent_id.as_deref() == Some("secure1")
                )
                .count(),
            5
        );
    }
}
