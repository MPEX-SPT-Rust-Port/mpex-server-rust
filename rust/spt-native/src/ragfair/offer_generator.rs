//! `Generators/Ragfair/RagfairOfferGenerator.cs` — the condition randomisation and armor-plate
//! removal a dynamic offer's items go through.
//!
//! **The draw table.** One `randomise_offer_item_upd_properties` call per item class, in call
//! order. `add_missing_conditions` runs first and always, and never draws.
//!
//! | item class | draws |
//! |---|---|
//! | any, `offerCreator != FakePlayer` | **0** |
//! | any, no `dynamic.condition` base-class match | **0** |
//! | any matched tpl | **1** — [`get_chance_100`] on `conditionChance * 100`; a failed roll stops here |
//! | …then armor (`ArmorItemCanHoldMods` / plate / armored equipment) | **2** + `4 × (children with armorClass > 1)`, plus **1** if a visor child is found and **2** if its 25% roll then passes |
//! | …then weapon | **2 + 4** |
//! | …then medkit / key / food-drink / repair kit | **2** |
//! | …then fuel | **2 + 1** |
//! | …then nothing matched | **2** |
//!
//! The leading **2** is `RandomiseItemCondition`'s pair of multiplier draws at `:699-700`, and the
//! first of them is `GetDouble(Max.Min, Max.Min)` — the `Max.Min` is read *twice*. That degenerate
//! range always returns `Max.Min`, but `RandomUtil.GetDouble` (`:76-80`) still calls the generator,
//! so the draw is spent. Transcribed as-is; it is not a typo to fix here.
//!
//! [`remove_armor_plates`] costs **1** draw unconditionally — the `GetChance100` at `:512` is
//! evaluated before the `ArmorItemHasRemovablePlateSlots` gate, so an armor with no removable plate
//! slots still spends it. [`remove_banned_plates_from_preset`] draws **0**.
//!
//! Where the C# would throw, and what happens here instead:
//! - `AddMissingConditions` (`:837`) dereferences `GetItem(...).Value.Properties` unguarded — a tpl
//!   the items view does not know becomes a [`LootError`];
//! - `Condition[id]` (`:661`, `:698`) is a `Dictionary` indexer — a missing key becomes a
//!   [`LootError`] carrying the `KeyNotFoundException` text. The one reachable caller takes the id
//!   straight off that dictionary's keys, so it cannot miss;
//! - the remaining unguarded dereferences on this path (`Upd`/`Upd.Repairable` being null when
//!   written at `:794`/`:846`, and the `(double)` casts of a null `MaxDurability`, `MaxResource`,
//!   `MaxRepairResource` or `MedKit.HpResource`) have no error channel in the C# signatures the
//!   port mirrors: a missing `Upd` is materialised the way `AddUpd` would, and a missing numeric
//!   property reads as `0`. Every one of them needs a template that omits the property its own
//!   branch selected on, which real data never does.

use serde_json::json;

use super::RagfairContext;
use crate::loot::item_helper::{
    ARMOR_PLATE, ARMORED_EQUIPMENT, FUEL, LootError, WEAPON, armor_item_can_hold_mods,
    armor_item_has_removable_plate_slots, get_item, get_removable_plate_slot_ids, is_of_baseclass,
    is_of_baseclasses,
};
use crate::loot::models::{
    Item, UpdFoodDrink, UpdKey, UpdMedKit, UpdRepairKit, UpdRepairable, UpdResource,
};
use crate::loot::random_util::{get_chance_100, get_double, get_int, round_half_even};
use crate::ragfair::models::ArmorPlateBlacklistSettingsWire;

/// `Models/Enums/OfferCreator.cs` — the wire never carries it; it is a call-site constant on the
/// C# side too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferCreator {
    Player,
    Trader,
    FakePlayer,
}

/// `RagfairOfferGenerator.RandomiseOfferItemUpdProperties` (`:641-666`). The C# `userId` parameter
/// is not read by the method body, so it is not a parameter here.
///
/// `itemWithMods` is a slice rather than a `Vec`: nothing below this call resizes the list.
///
/// # Errors
///
/// Propagates [`add_missing_conditions`]'s and [`randomise_item_condition`]'s.
pub fn randomise_offer_item_upd_properties(
    ctx: &RagfairContext,
    item_with_mods: &mut [Item],
    item_details_tpl: &str,
    offer_creator: OfferCreator,
) -> Result<(), LootError> {
    // Add any missing properties to first item in array
    add_missing_conditions(ctx, &mut item_with_mods[0])?;

    if offer_creator != OfferCreator::FakePlayer {
        return Ok(());
    }

    // No condition details found, don't proceed with modifying item conditions
    let Some(parent_id) = get_dynamic_condition_id_for_tpl(ctx, item_details_tpl) else {
        return Ok(());
    };

    let condition = condition_settings(ctx, &parent_id)?;

    // Roll random chance to randomise item condition
    if get_chance_100(condition.condition_chance * 100.0) {
        randomise_item_condition(ctx, &parent_id, item_with_mods, item_details_tpl)?;
    }

    Ok(())
}

/// `RagfairOfferGenerator.GetDynamicConditionIdForTpl` (`:673-686`) — the first base class in
/// `dynamic.condition`'s **insertion order** the tpl derives from.
fn get_dynamic_condition_id_for_tpl(ctx: &RagfairContext, tpl: &str) -> Option<String> {
    // Get keys from condition config dictionary
    for base_class in ctx.dynamic.condition.keys() {
        if is_of_baseclass(ctx.items, tpl, base_class) {
            return Some(base_class.clone());
        }
    }

    None
}

/// The `Condition[id]` indexer at `:661`/`:698`.
fn condition_settings<'a>(
    ctx: &'a RagfairContext,
    condition_settings_id: &str,
) -> Result<&'a crate::ragfair::models::ConditionWire, LootError> {
    ctx.dynamic
        .condition
        .get(condition_settings_id)
        .ok_or_else(|| {
            LootError::new(format!(
                "The given key '{condition_settings_id}' was not present in the dictionary."
            ))
        })
}

/// `RagfairOfferGenerator.RandomiseItemCondition` (`:694-774`). The branch order is the contract —
/// armor, weapon, medkit, key, food/drink, repair kit, fuel — and every arm but the first returns
/// as soon as it fires.
///
/// # Errors
///
/// A `conditionSettingsId` the config does not carry (`KeyNotFoundException` in C#).
fn randomise_item_condition(
    ctx: &RagfairContext,
    condition_settings_id: &str,
    item_with_mods: &mut [Item],
    item_details_tpl: &str,
) -> Result<(), LootError> {
    let item_condition_values = condition_settings(ctx, condition_settings_id)?;
    // `:699` reads `Max.Min` for both bounds. A degenerate range, and still one draw.
    let max_multiplier = get_double(item_condition_values.max.min, item_condition_values.max.min);
    let current_multiplier = get_double(
        item_condition_values.current.min,
        item_condition_values.current.max,
    );

    let root_tpl = item_with_mods[0].template.clone();

    // Randomise armor + plates + armor related things
    if armor_item_can_hold_mods(ctx.items, &root_tpl)
        || is_of_baseclasses(ctx.items, &root_tpl, &[ARMOR_PLATE, ARMORED_EQUIPMENT])
    {
        randomise_armor_durability_values(ctx, item_with_mods, current_multiplier, max_multiplier);

        // Add hits to visor.
        //
        // Dead branch, ported as written: `:712` compares a child's *parent item id* against the
        // ARMORED_EQUIPMENT *base class tpl*, and a parent id is another item's `_id`, never a
        // base class. No real offer ever satisfies it, so `Upd.FaceShield` is never written here.
        let visor_mod = item_with_mods.iter_mut().find(|item| {
            item.parent_id.as_deref() == Some(ARMORED_EQUIPMENT)
                && item.slot_id.as_deref() == Some("mod_equipment_000")
        });
        if let Some(visor_mod) = visor_mod
            && get_chance_100(25.0)
        {
            let upd = visor_mod.upd.get_or_insert_default();
            // No typed `UpdFaceShield` in this crate; `Upd` serialises its members verbatim.
            upd.extra
                .insert("FaceShield".to_owned(), json!({ "Hits": get_int(1, 3) }));
        }

        return Ok(());
    }

    // Randomise Weapons
    if is_of_baseclass(ctx.items, item_details_tpl, WEAPON) {
        randomise_weapon_durability(
            ctx,
            &mut item_with_mods[0],
            item_details_tpl,
            max_multiplier,
            current_multiplier,
        );

        return Ok(());
    }

    let item_details = get_item(ctx.items, item_details_tpl);
    let root_item = &mut item_with_mods[0];

    if let Some(med_kit) = root_item.upd.as_mut().and_then(|upd| upd.med_kit.as_mut()) {
        // Randomize health
        let hp_resource = round_half_even(med_kit.hp_resource.unwrap_or_default() * max_multiplier);
        med_kit.hp_resource = Some(if hp_resource == 0.0 { 1.0 } else { hp_resource });

        return Ok(());
    }

    let maximum_number_of_usage = item_details.and_then(|details| details.maximum_number_of_usage);
    if let Some(key) = root_item.upd.as_mut().and_then(|upd| upd.key.as_mut())
        && maximum_number_of_usage.is_some_and(|uses| uses > 1)
    {
        // Randomize key uses
        key.number_of_usages = Some(round_half_even(
            f64::from(maximum_number_of_usage.unwrap_or_default()) * (1.0 - max_multiplier),
        ) as i32);

        return Ok(());
    }

    let max_resource = item_details.and_then(|details| details.max_resource);
    if let Some(food_drink) = root_item
        .upd
        .as_mut()
        .and_then(|upd| upd.food_drink.as_mut())
    {
        // randomize food/drink value
        let hp_percent =
            round_half_even(f64::from(max_resource.unwrap_or_default()) * max_multiplier);
        food_drink.hp_percent = Some(if hp_percent == 0.0 { 1.0 } else { hp_percent });

        return Ok(());
    }

    let max_repair_resource = item_details.and_then(|details| details.max_repair_resource);
    if let Some(repair_kit) = root_item
        .upd
        .as_mut()
        .and_then(|upd| upd.repair_kit.as_mut())
    {
        // randomize repair kit (armor/weapon) uses
        let resource = round_half_even(max_repair_resource.unwrap_or_default() * max_multiplier);
        repair_kit.resource = Some(if resource == 0.0 { 1.0 } else { resource });

        return Ok(());
    }

    if is_of_baseclass(ctx.items, item_details_tpl, FUEL) {
        let total_capacity = f64::from(max_resource.unwrap_or_default());

        // Randomise multi between value in config and 1 (100%)
        let randomised_multi = get_double(max_multiplier, 1.0);
        let remaining_fuel = round_half_even(total_capacity * randomised_multi);
        root_item.upd.get_or_insert_default().resource = Some(UpdResource {
            units_consumed: Some(total_capacity - remaining_fuel),
            value: Some(remaining_fuel),
        });
    }

    Ok(())
}

/// `RagfairOfferGenerator.RandomiseWeaponDurability` (`:783-796`) — four draws, and a durability
/// that lands on zero is lifted to one.
fn randomise_weapon_durability(
    ctx: &RagfairContext,
    item: &mut Item,
    item_db_tpl: &str,
    max_multiplier: f64,
    current_multiplier: f64,
) {
    // Max
    let base_max_durability = get_item(ctx.items, item_db_tpl)
        .and_then(|details| details.max_durability)
        .unwrap_or_default();
    let lowest_max_durability = get_double(max_multiplier, 1.0) * base_max_durability;
    let chosen_max_durability =
        round_half_even(get_double(lowest_max_durability, base_max_durability));

    // Current
    let lowest_current_durability = get_double(current_multiplier, 1.0) * chosen_max_durability;
    let chosen_current_durability =
        round_half_even(get_double(lowest_current_durability, chosen_max_durability));

    let repairable = item
        .upd
        .get_or_insert_default()
        .repairable
        .get_or_insert_default();
    // Never var value become 0
    repairable.durability = Some(if chosen_current_durability == 0.0 {
        1.0
    } else {
        chosen_current_durability
    });
    repairable.max_durability = Some(chosen_max_durability);
}

/// `RagfairOfferGenerator.RandomiseArmorDurabilityValues` (`:804-827`) — four draws **per child**
/// whose template has `armorClass > 1`, so the draw count follows the child list. Note the
/// parameter order: `current` first, then `max`, and the first draw uses `max`.
fn randomise_armor_durability_values(
    ctx: &RagfairContext,
    armor_with_mods: &mut [Item],
    current_multiplier: f64,
    max_multiplier: f64,
) {
    for armor_item in armor_with_mods.iter_mut() {
        let item_db_details = get_item(ctx.items, &armor_item.template);
        if item_db_details.is_some_and(|details| details.armor_class.is_some_and(|class| class > 1))
        {
            let upd = armor_item.upd.get_or_insert_default();

            let base_max_durability = item_db_details
                .and_then(|details| details.max_durability)
                .unwrap_or_default();
            let lowest_max_durability = get_double(max_multiplier, 1.0) * base_max_durability;
            let chosen_max_durability =
                round_half_even(get_double(lowest_max_durability, base_max_durability));

            let lowest_current_durability =
                get_double(current_multiplier, 1.0) * chosen_max_durability;
            let chosen_current_durability =
                round_half_even(get_double(lowest_current_durability, chosen_max_durability));

            upd.repairable = Some(UpdRepairable {
                // Never var value become 0
                durability: Some(if chosen_current_durability == 0.0 {
                    1.0
                } else {
                    chosen_current_durability
                }),
                max_durability: Some(chosen_max_durability),
                extra: serde_json::Map::new(),
            });
        }
    }
}

/// `RagfairOfferGenerator.AddMissingConditions` (`:835-877`) — the first matching arm writes and
/// returns. No draws.
///
/// # Errors
///
/// Where `:837` dereferences a `GetItem` miss: a tpl the items view does not know.
fn add_missing_conditions(ctx: &RagfairContext, item: &mut Item) -> Result<(), LootError> {
    let props = get_item(ctx.items, &item.template).ok_or_else(|| {
        LootError::new("Object reference not set to an instance of an object.".to_owned())
    })?;

    let is_repairable = props.durability.is_some();
    let is_medkit = props.max_hp_resource.is_some();
    let is_key = props.maximum_number_of_usage.is_some();
    let is_consumable =
        props.max_resource.is_some_and(|max| max > 1) && props.food_use_time.is_some();
    let is_repair_kit = props.max_repair_resource.is_some();

    if is_repairable && props.durability.is_some_and(|durability| durability > 0.0) {
        item.upd.get_or_insert_default().repairable = Some(UpdRepairable {
            durability: props.durability,
            max_durability: props.durability,
            extra: serde_json::Map::new(),
        });

        return Ok(());
    }

    if is_medkit && props.max_hp_resource.is_some_and(|max| max > 0) {
        item.upd.get_or_insert_default().med_kit = Some(UpdMedKit {
            hp_resource: props.max_hp_resource.map(f64::from),
        });

        return Ok(());
    }

    if is_key {
        item.upd.get_or_insert_default().key = Some(UpdKey {
            number_of_usages: Some(0),
        });

        return Ok(());
    }

    // Food/drink
    if is_consumable {
        item.upd.get_or_insert_default().food_drink = Some(UpdFoodDrink {
            hp_percent: props.max_resource.map(f64::from),
        });

        return Ok(());
    }

    if is_repair_kit {
        item.upd.get_or_insert_default().repair_kit = Some(UpdRepairKit {
            resource: props.max_repair_resource,
        });
    }

    Ok(())
}

/// `RagfairOfferGenerator.RemoveBannedPlatesFromPreset` (`:381-416`). No draws.
///
/// C# iterates a snapshot of the plate slots but removes off the live list by `IndexOf`
/// (`:410`), so each removal shifts the later plates' indexes; collecting the plates by identity
/// first and re-finding each one reproduces that exactly.
pub fn remove_banned_plates_from_preset(
    ctx: &RagfairContext,
    preset_with_children: &mut Vec<Item>,
    plate_settings: &ArmorPlateBlacklistSettingsWire,
) -> bool {
    // Cant hold armor inserts, skip
    if !armor_item_can_hold_mods(ctx.items, &preset_with_children[0].template) {
        return false;
    }

    let plate_slot_ids: Vec<String> = preset_with_children
        .iter()
        .filter(|item| is_plate_slot(item))
        .map(|item| item.id.clone())
        .collect();
    // Has no plate slots e.g. "front_plate", exit
    if plate_slot_ids.is_empty() {
        return false;
    }

    let mut removed_plate = false;
    for plate_id in plate_slot_ids {
        let Some(index) = preset_with_children
            .iter()
            .position(|item| item.id == plate_id)
        else {
            continue;
        };
        let plate_slot = &preset_with_children[index];

        let plate_details = get_item(ctx.items, &plate_slot.template);
        if plate_settings
            .ignore_slots
            .contains(&lowercased_slot_id(plate_slot))
        {
            continue;
        }

        let plate_armor_level = plate_details
            .and_then(|details| details.armor_class)
            .unwrap_or(0);
        if plate_armor_level > plate_settings.max_protection_level {
            preset_with_children.remove(index);
            removed_plate = true;
        }
    }

    removed_plate
}

/// `RagfairOfferGenerator.RemoveArmorPlates` (`:508-528`). The C# takes the root item separately;
/// it is always `itemWithChildren[0]` (`:436`, `:447`).
///
/// The `GetChance100` is drawn **before** the plate-slot gate, so an armor with no removable plate
/// slots still spends it.
pub fn remove_armor_plates(ctx: &RagfairContext, item_with_children: &mut Vec<Item>) {
    let armor_config = &ctx.dynamic.armor;

    let should_remove_plates =
        get_chance_100(f64::from(armor_config.remove_removable_plate_chance));
    if !should_remove_plates
        || !armor_item_has_removable_plate_slots(ctx.items, &item_with_children[0].template)
    {
        return;
    }

    // Latest first, to ensure we don't move later items off by 1 each time we remove an item below
    // it. C# collects the indexes into a `HashSet<int>` and orders it descending, which a
    // descending sort of the (already unique) indexes matches.
    let mut indexes_to_remove: Vec<usize> = item_with_children
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            armor_config
                .plate_slot_id_to_remove_pool
                .contains(&lowercased_slot_id(item))
        })
        .map(|(index, _)| index)
        .collect();
    indexes_to_remove.sort_unstable_by(|left, right| right.cmp(left));

    for index in indexes_to_remove {
        item_with_children.remove(index);
    }
}

/// `item.SlotId?.ToLowerInvariant()`, with the null case folded to the empty string — no slot id
/// is ever a member of the sets it is tested against.
fn lowercased_slot_id(item: &Item) -> String {
    item.slot_id.as_deref().unwrap_or_default().to_lowercase()
}

/// `GetRemovablePlateSlotIds().Contains(item.SlotId?.ToLowerInvariant())` (`:390`).
fn is_plate_slot(item: &Item) -> bool {
    get_removable_plate_slot_ids().contains(&lowercased_slot_id(item).as_str())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use indexmap::IndexMap;
    use serde_json::json;

    use super::*;
    use crate::loot::item_helper::{ARMOR, BUILT_IN_INSERTS};
    use crate::loot::models::{ItemView, PresetView, Upd};
    use crate::loot::random_util::TestSeedGuard;
    use crate::ragfair::models::DynamicConfigWire;
    use crate::ragfair::{NO_BLACKLIST, NO_DEFAULT_PRESETS, NO_NAMES};

    const SEED: u64 = 20260813;

    const WEAPON_TPL: &str = "weapon_with_durability";
    const ARMOR_TPL: &str = "armor_vest_with_plate_slots";
    const ARMOR_NO_PLATE_SLOTS_TPL: &str = "armor_without_plate_slots";
    const PLATE_CLASS_4_TPL: &str = "class_4_plate";
    const PLATE_CLASS_6_TPL: &str = "class_6_plate";
    const SOFT_INSERT_TPL: &str = "class_3_soft_insert";
    const MEDKIT_TPL: &str = "medkit";
    const KEY_TPL: &str = "key";
    const FOOD_TPL: &str = "food";
    const REPAIR_KIT_TPL: &str = "repair_kit";
    const FUEL_TPL: &str = "fuel_can";
    const PLAIN_TPL: &str = "item_without_a_condition_entry";
    const REPAIRABLE_MEDKIT_TPL: &str = "repairable_and_medkit";

    /// The condition entry every direct `randomise_item_condition` call uses: `max` is a non-empty
    /// range, so the `Max.Min` double-read is observable.
    const CONDITION_ID: &str = "condition_settings_base_class";
    const CONDITION_MAX_MIN: f64 = 0.5;
    const CONDITION_MAX_MAX: f64 = 0.9;
    const CONDITION_CURRENT_MIN: f64 = 0.4;
    const CONDITION_CURRENT_MAX: f64 = 0.8;

    struct Fixture {
        items: IndexMap<String, ItemView>,
        dynamic: DynamicConfigWire,
        prices: IndexMap<String, f64>,
        blacklist: HashSet<String>,
        presets: IndexMap<String, PresetView>,
        preset_lists: IndexMap<String, Vec<PresetView>>,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_condition_chance(1.0)
        }

        fn with_condition_chance(condition_chance: f64) -> Self {
            Self {
                items: serde_json::from_value(json!({
                    WEAPON_TPL: {"name": "weapon", "type": "Item", "parent": WEAPON,
                        "maxDurability": 100.0, "durability": 100.0},
                    ARMOR_TPL: {"name": "armor", "type": "Item", "parent": ARMOR, "armorClass": 0,
                        "durability": 50.0, "maxDurability": 50.0,
                        "slots": [{"name": "front_plate"}, {"name": "back_plate"},
                            {"name": "left_side_plate"}, {"name": "soft_armor_front"}]},
                    ARMOR_NO_PLATE_SLOTS_TPL: {"name": "plateless armor", "type": "Item",
                        "parent": ARMOR, "armorClass": 0,
                        "slots": [{"name": "soft_armor_front"}]},
                    PLATE_CLASS_4_TPL: {"name": "class 4 plate", "type": "Item",
                        "parent": ARMOR_PLATE, "armorClass": 4, "maxDurability": 40.0},
                    PLATE_CLASS_6_TPL: {"name": "class 6 plate", "type": "Item",
                        "parent": ARMOR_PLATE, "armorClass": 6, "maxDurability": 60.0},
                    SOFT_INSERT_TPL: {"name": "soft insert", "type": "Item",
                        "parent": BUILT_IN_INSERTS, "armorClass": 3, "maxDurability": 30.0},
                    MEDKIT_TPL: {"name": "medkit", "type": "Item", "maxHpResource": 400},
                    KEY_TPL: {"name": "key", "type": "Item", "maximumNumberOfUsage": 10},
                    FOOD_TPL: {"name": "food", "type": "Item", "maxResource": 100,
                        "foodUseTime": 5.0},
                    REPAIR_KIT_TPL: {"name": "repair kit", "type": "Item",
                        "maxRepairResource": 60.0},
                    FUEL_TPL: {"name": "fuel", "type": "Item", "parent": FUEL, "maxResource": 100},
                    PLAIN_TPL: {"name": "plain", "type": "Item"},
                    REPAIRABLE_MEDKIT_TPL: {"name": "two arms", "type": "Item",
                        "durability": 50.0, "maxHpResource": 400},
                    // The base classes themselves, so the parent walk has somewhere to land.
                    WEAPON: {"name": "weapon base", "type": "Node"},
                    ARMOR: {"name": "armor base", "type": "Node"},
                    ARMOR_PLATE: {"name": "plate base", "type": "Node",
                        "parent": ARMORED_EQUIPMENT},
                    ARMORED_EQUIPMENT: {"name": "armored equipment base", "type": "Node"},
                    BUILT_IN_INSERTS: {"name": "soft insert base", "type": "Node"},
                    FUEL: {"name": "fuel base", "type": "Node"},
                }))
                .expect("items view parses"),
                dynamic: dynamic_config(condition_chance),
                prices: IndexMap::new(),
                blacklist: HashSet::new(),
                presets: IndexMap::new(),
                preset_lists: IndexMap::new(),
            }
        }

        fn ctx(&self) -> RagfairContext<'_> {
            RagfairContext {
                items: &self.items,
                dynamic: &self.dynamic,
                item_presets: &self.presets,
                default_presets: &NO_DEFAULT_PRESETS,
                default_presets_by_tpl: &self.presets,
                presets_by_tpl: &self.preset_lists,
                flea_prices: &self.prices,
                handbook_prices: &self.prices,
                highest_trader_prices: &self.prices,
                config_blacklist: &self.blacklist,
                seasonal_item_tpl_blacklist: &NO_BLACKLIST,
                pmc_names_usec: &NO_NAMES,
                pmc_names_bear: &NO_NAMES,
                timestamp: 1_700_000_000,
                seasonal_event_active: false,
                diagnostics: Vec::new(),
            }
        }
    }

    /// `armorPlate` blacklist: class 5+ banned, `back_plate` exempt.
    /// `armor`: plates always removed, and only `front_plate`/`left_side_plate` are in the pool.
    fn dynamic_config(condition_chance: f64) -> DynamicConfigWire {
        serde_json::from_value(json!({
            "useTraderPriceForOffersIfHigher": false,
            "barter": {"chancePercent": 0.0, "itemCountMin": 1, "itemCountMax": 1,
                "priceRangeVariancePercent": 0.0, "minRoubleCostToBecomeBarter": 0.0,
                "makeSingleStackOnly": false, "itemTplBlacklist": [], "itemTypeBlacklist": []},
            "pack": {"chancePercent": 0.0, "itemCountMin": 1, "itemCountMax": 1,
                "itemTypeWhitelist": []},
            "offerAdjustment": {"adjustPriceWhenBelowHandbookPrice": false,
                "maxPriceDifferenceBelowHandbookPercent": 40.0, "handbookPriceMultiplier": 1.5,
                "priceThresholdRub": 6000.0},
            "offerItemCount": {"default": {"min": 1, "max": 1}},
            "priceRanges": {"default": {"min": 1.0, "max": 1.0},
                "preset": {"min": 1.0, "max": 1.0}, "pack": {"min": 1.0, "max": 1.0}},
            "showDefaultPresetsOnly": false,
            "ignoreQualityPriceVarianceBlacklist": [],
            "endTimeSeconds": {"min": 1, "max": 2},
            // Insertion order is the match order: ARMORED_EQUIPMENT is a *grandparent* of a plate
            // and still wins over the plate's direct parent because it is listed first.
            "condition": {
                ARMORED_EQUIPMENT: {"conditionChance": condition_chance,
                    "current": {"min": 0.1, "max": 0.2}, "max": {"min": 0.3, "max": 0.4}},
                ARMOR_PLATE: {"conditionChance": condition_chance,
                    "current": {"min": 0.1, "max": 0.2}, "max": {"min": 0.3, "max": 0.4}},
                WEAPON: {"conditionChance": condition_chance,
                    "current": {"min": CONDITION_CURRENT_MIN, "max": CONDITION_CURRENT_MAX},
                    "max": {"min": CONDITION_MAX_MIN, "max": CONDITION_MAX_MAX}},
                ARMOR: {"conditionChance": condition_chance,
                    "current": {"min": CONDITION_CURRENT_MIN, "max": CONDITION_CURRENT_MAX},
                    "max": {"min": CONDITION_MAX_MIN, "max": CONDITION_MAX_MAX}},
                CONDITION_ID: {"conditionChance": condition_chance,
                    "current": {"min": CONDITION_CURRENT_MIN, "max": CONDITION_CURRENT_MAX},
                    "max": {"min": CONDITION_MAX_MIN, "max": CONDITION_MAX_MAX}},
            },
            "stackablePercent": {"min": 10.0, "max": 100.0},
            "nonStackableCount": {"min": 1, "max": 4},
            "rating": {"min": 0.0, "max": 1.0},
            "armor": {"removeRemovablePlateChance": 100,
                "plateSlotIdToRemovePool": ["front_plate", "left_side_plate"]},
            "itemPriceMultiplier": {},
            "offerCurrencyChancePercent": {"5449016a4bdc2d6f028b456f": 100.0},
            "showAsSingleStack": [],
            "removeSeasonalItemsWhenNotInEvent": false,
            "blacklist": {"damagedAmmoPacks": true, "custom": [], "enableBsgList": true,
                "enableQuestList": true, "traderItems": false,
                "armorPlate": {"maxProtectionLevel": 4, "ignoreSlots": ["back_plate"]},
                "enableCustomItemCategoryList": false, "customItemCategoryList": []},
            "unreasonableModPrices": {},
            "generateBaseFleaPrices": {"useHandbookPrice": false, "priceMultiplier": 1.0,
                "preventPriceBeingBelowTraderBuyPrice": false, "itemTplMultiplierOverride": {},
                "itemTypeMultiplierOverride": {}, "useHideoutCraftMultiplier": false,
                "hideoutCraftMultiplier": 1.0, "generatePresetPriceByChildren": false},
        }))
        .expect("dynamic config parses")
    }

    fn item(id: &str, tpl: &str) -> Item {
        Item {
            id: id.to_owned(),
            template: tpl.to_owned(),
            upd: Some(Upd::default()),
            ..Item::default()
        }
    }

    fn child(id: &str, tpl: &str, parent_id: &str, slot_id: &str) -> Item {
        Item {
            parent_id: Some(parent_id.to_owned()),
            slot_id: Some(slot_id.to_owned()),
            ..item(id, tpl)
        }
    }

    /// An armor root plus a class 6 front plate, a class 6 back plate, a class 4 left side plate
    /// and a class 3 soft insert.
    fn armor_with_plates() -> Vec<Item> {
        vec![
            item("armor_root", ARMOR_TPL),
            child("front", PLATE_CLASS_6_TPL, "armor_root", "front_plate"),
            child("back", PLATE_CLASS_6_TPL, "armor_root", "back_plate"),
            child("left", PLATE_CLASS_4_TPL, "armor_root", "left_side_plate"),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ]
    }

    /// Where the seeded stream stands after `consume` — the read-the-next-draw idiom the rest of
    /// the ragfair port pins draw counts with.
    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    /// The stream untouched, i.e. what a zero-draw arm has to leave behind.
    fn untouched_stream() -> f64 {
        stream_position_after(|| {})
    }

    /// The stream after the two `:699-700` multiplier draws plus `extra` further doubles.
    fn stream_position_after_condition_draws(extra: usize) -> f64 {
        stream_position_after(|| {
            get_double(CONDITION_MAX_MIN, CONDITION_MAX_MIN);
            get_double(CONDITION_CURRENT_MIN, CONDITION_CURRENT_MAX);
            for _ in 0..extra {
                get_double(0.0, 1.0);
            }
        })
    }

    fn seeded<T>(run: impl FnOnce() -> T) -> T {
        let _guard = TestSeedGuard::install(SEED);
        run()
    }

    fn condition(fixture: &Fixture, items: &mut [Item], tpl: &str) {
        randomise_item_condition(&fixture.ctx(), CONDITION_ID, items, tpl)
            .expect("the condition id is in the config");
    }

    // -----------------------------------------------------------------------
    // randomise_item_condition — the `Max.Min` double-read
    // -----------------------------------------------------------------------

    #[test]
    fn the_max_multiplier_is_the_max_min_bound_read_twice_and_still_costs_a_draw() {
        let fixture = Fixture::new();
        let mut items = vec![item("medkit", MEDKIT_TPL)];
        items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
            hp_resource: Some(400.0),
        });

        seeded(|| condition(&fixture, &mut items, MEDKIT_TPL));

        // 400 * 0.5, not 400 * anything in (0.5, 0.9] — the degenerate range can only return
        // `Max.Min`.
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .med_kit
                .as_ref()
                .unwrap()
                .hp_resource,
            Some(200.0)
        );
        // ...and the draw was spent anyway.
        let after = stream_position_after(|| {
            let mut items = vec![item("medkit", MEDKIT_TPL)];
            items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
                hp_resource: Some(400.0),
            });
            condition(&fixture, &mut items, MEDKIT_TPL);
        });
        assert_ne!(after, untouched_stream());
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    // -----------------------------------------------------------------------
    // randomise_item_condition — one test per branch, draw count pinned
    // -----------------------------------------------------------------------

    #[test]
    fn the_armor_branch_draws_four_times_per_child_above_armor_class_one() {
        let fixture = Fixture::new();
        let mut items = armor_with_plates();

        seeded(|| condition(&fixture, &mut items, ARMOR_TPL));

        // Root is class 0, the three mods are classes 6, 6, 4 and 3 - all above 1.
        assert!(items[0].upd.as_ref().unwrap().repairable.is_none());
        for plate in &items[1..] {
            let repairable = plate.upd.as_ref().unwrap().repairable.as_ref().unwrap();
            assert!(repairable.durability.unwrap() > 0.0);
            assert!(repairable.max_durability.unwrap() > 0.0);
        }

        let after = stream_position_after(|| {
            let mut items = armor_with_plates();
            condition(&fixture, &mut items, ARMOR_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(4 * 4));
    }

    #[test]
    fn the_armor_branch_draw_count_follows_the_child_list() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut items = vec![item("armor_root", ARMOR_TPL)];
            condition(&fixture, &mut items, ARMOR_TPL);
        });

        // A lone class 0 root qualifies nothing, so only the two multiplier draws happen.
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn a_plate_root_takes_the_armor_branch_without_holding_mods() {
        let fixture = Fixture::new();
        let mut items = vec![item("plate", PLATE_CLASS_6_TPL)];

        seeded(|| condition(&fixture, &mut items, PLATE_CLASS_6_TPL));

        assert!(items[0].upd.as_ref().unwrap().repairable.is_some());
    }

    #[test]
    fn the_weapon_branch_takes_four_draws() {
        let fixture = Fixture::new();
        let mut items = vec![item("weapon", WEAPON_TPL)];
        items[0].upd.as_mut().unwrap().repairable = Some(UpdRepairable {
            durability: Some(100.0),
            max_durability: Some(100.0),
            ..UpdRepairable::default()
        });

        seeded(|| condition(&fixture, &mut items, WEAPON_TPL));

        let repairable = items[0].upd.as_ref().unwrap().repairable.as_ref().unwrap();
        let max = repairable.max_durability.unwrap();
        let current = repairable.durability.unwrap();
        assert!((50.0..=100.0).contains(&max), "max was {max}");
        assert!(current <= max && current > 0.0, "current was {current}");
        // Rounded, both of them.
        assert_eq!(max, max.trunc());
        assert_eq!(current, current.trunc());

        let after = stream_position_after(|| {
            let mut items = vec![item("weapon", WEAPON_TPL)];
            condition(&fixture, &mut items, WEAPON_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(4));
    }

    #[test]
    fn the_medkit_branch_takes_no_further_draw_and_never_lands_on_zero() {
        let fixture = Fixture::new();
        let mut items = vec![item("medkit", MEDKIT_TPL)];
        // A resource small enough that `round(1 * 0.5)` is 0 - which the arm lifts to 1.
        items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
            hp_resource: Some(1.0),
        });

        seeded(|| condition(&fixture, &mut items, MEDKIT_TPL));

        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .med_kit
                .as_ref()
                .unwrap()
                .hp_resource,
            Some(1.0)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("medkit", MEDKIT_TPL)];
            items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
                hp_resource: Some(1.0),
            });
            condition(&fixture, &mut items, MEDKIT_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_key_branch_uses_one_minus_the_max_multiplier() {
        let fixture = Fixture::new();
        let mut items = vec![item("key", KEY_TPL)];
        items[0].upd.as_mut().unwrap().key = Some(UpdKey {
            number_of_usages: Some(0),
        });

        seeded(|| condition(&fixture, &mut items, KEY_TPL));

        // round(10 * (1 - 0.5))
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .key
                .as_ref()
                .unwrap()
                .number_of_usages,
            Some(5)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("key", KEY_TPL)];
            items[0].upd.as_mut().unwrap().key = Some(UpdKey {
                number_of_usages: Some(0),
            });
            condition(&fixture, &mut items, KEY_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn a_single_use_key_falls_through_its_arm() {
        let fixture = Fixture {
            items: serde_json::from_value(json!({
                KEY_TPL: {"name": "single use key", "type": "Item", "maximumNumberOfUsage": 1},
            }))
            .expect("items view parses"),
            ..Fixture::new()
        };
        let mut items = vec![item("key", KEY_TPL)];
        items[0].upd.as_mut().unwrap().key = Some(UpdKey {
            number_of_usages: Some(0),
        });

        seeded(|| condition(&fixture, &mut items, KEY_TPL));

        // `MaximumNumberOfUsage > 1` gates the arm, so the value is untouched.
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .key
                .as_ref()
                .unwrap()
                .number_of_usages,
            Some(0)
        );
    }

    #[test]
    fn the_food_branch_reads_max_resource_from_the_template() {
        let fixture = Fixture::new();
        let mut items = vec![item("food", FOOD_TPL)];
        items[0].upd.as_mut().unwrap().food_drink = Some(UpdFoodDrink {
            hp_percent: Some(100.0),
        });

        seeded(|| condition(&fixture, &mut items, FOOD_TPL));

        // round(100 * 0.5)
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .food_drink
                .as_ref()
                .unwrap()
                .hp_percent,
            Some(50.0)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("food", FOOD_TPL)];
            items[0].upd.as_mut().unwrap().food_drink = Some(UpdFoodDrink {
                hp_percent: Some(100.0),
            });
            condition(&fixture, &mut items, FOOD_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_repair_kit_branch_reads_max_repair_resource_from_the_template() {
        let fixture = Fixture::new();
        let mut items = vec![item("repair kit", REPAIR_KIT_TPL)];
        items[0].upd.as_mut().unwrap().repair_kit = Some(UpdRepairKit {
            resource: Some(60.0),
        });

        seeded(|| condition(&fixture, &mut items, REPAIR_KIT_TPL));

        // round(60 * 0.5)
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .repair_kit
                .as_ref()
                .unwrap()
                .resource,
            Some(30.0)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("repair kit", REPAIR_KIT_TPL)];
            items[0].upd.as_mut().unwrap().repair_kit = Some(UpdRepairKit {
                resource: Some(60.0),
            });
            condition(&fixture, &mut items, REPAIR_KIT_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_fuel_branch_takes_one_further_draw_and_splits_the_capacity() {
        let fixture = Fixture::new();
        let mut items = vec![item("fuel", FUEL_TPL)];

        seeded(|| condition(&fixture, &mut items, FUEL_TPL));

        let resource = items[0].upd.as_ref().unwrap().resource.as_ref().unwrap();
        let remaining = resource.value.unwrap();
        assert!((50.0..=100.0).contains(&remaining), "value was {remaining}");
        assert_eq!(resource.units_consumed, Some(100.0 - remaining));

        let after = stream_position_after(|| {
            let mut items = vec![item("fuel", FUEL_TPL)];
            condition(&fixture, &mut items, FUEL_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(1));
    }

    #[test]
    fn an_item_matching_no_branch_only_spends_the_two_multiplier_draws() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut items = vec![item("plain", PLAIN_TPL)];
            condition(&fixture, &mut items, PLAIN_TPL);
        });

        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn an_unknown_condition_id_is_an_error_and_costs_no_draw() {
        let fixture = Fixture::new();
        let mut items = vec![item("plain", PLAIN_TPL)];

        let error = randomise_item_condition(&fixture.ctx(), "nope", &mut items, PLAIN_TPL)
            .expect_err("an unknown condition id errors");

        assert_eq!(
            error.message,
            "The given key 'nope' was not present in the dictionary."
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("plain", PLAIN_TPL)];
            randomise_item_condition(&fixture.ctx(), "nope", &mut items, PLAIN_TPL).unwrap_err();
        });
        assert_eq!(after, untouched_stream());
    }

    // -----------------------------------------------------------------------
    // The dead visor branch
    // -----------------------------------------------------------------------

    #[test]
    fn the_visor_branch_never_fires_on_realistic_data() {
        let fixture = Fixture::new();
        // A child in the visor slot, parented the way real data parents it: to the root item's id.
        let mut items = vec![
            item("armor_root", ARMOR_TPL),
            child("visor", PLAIN_TPL, "armor_root", "mod_equipment_000"),
        ];

        seeded(|| condition(&fixture, &mut items, ARMOR_TPL));

        assert!(
            !items[1]
                .upd
                .as_ref()
                .unwrap()
                .extra
                .contains_key("FaceShield")
        );
        let after = stream_position_after(|| {
            let mut items = vec![
                item("armor_root", ARMOR_TPL),
                child("visor", PLAIN_TPL, "armor_root", "mod_equipment_000"),
            ];
            condition(&fixture, &mut items, ARMOR_TPL);
        });
        // Neither the 25% roll nor the hit count was drawn.
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_visor_branch_fires_only_when_a_parent_id_is_a_base_class_tpl() {
        let fixture = Fixture::new();
        // Only reachable by parenting the child to the ARMORED_EQUIPMENT *base class tpl*, which
        // is what `:712` compares against and what no real item's `parentId` ever holds.
        let mut items = vec![
            item("armor_root", ARMOR_TPL),
            child("visor", PLAIN_TPL, ARMORED_EQUIPMENT, "mod_equipment_000"),
        ];

        let chance_passed = seeded(|| {
            get_double(CONDITION_MAX_MIN, CONDITION_MAX_MIN);
            get_double(CONDITION_CURRENT_MIN, CONDITION_CURRENT_MAX);

            get_chance_100(25.0)
        });
        seeded(|| condition(&fixture, &mut items, ARMOR_TPL));

        let face_shield = items[1].upd.as_ref().unwrap().extra.get("FaceShield");
        assert_eq!(face_shield.is_some(), chance_passed);
        if let Some(face_shield) = face_shield {
            let hits = face_shield["Hits"].as_i64().expect("Hits is a number");
            assert!((1..=3).contains(&hits), "hits was {hits}");
        }
    }

    // -----------------------------------------------------------------------
    // randomise_offer_item_upd_properties
    // -----------------------------------------------------------------------

    #[test]
    fn a_non_fake_player_offer_adds_conditions_but_never_randomises_them() {
        let fixture = Fixture::new();

        for creator in [OfferCreator::Player, OfferCreator::Trader] {
            let mut items = vec![item("weapon", WEAPON_TPL)];

            seeded(|| {
                randomise_offer_item_upd_properties(
                    &fixture.ctx(),
                    &mut items,
                    WEAPON_TPL,
                    creator,
                )
                .expect("the weapon template is in the view");
            });

            // AddMissingConditions still ran...
            let repairable = items[0].upd.as_ref().unwrap().repairable.as_ref().unwrap();
            assert_eq!(repairable.durability, Some(100.0));
            assert_eq!(repairable.max_durability, Some(100.0));

            // ...and nothing else did.
            let after = stream_position_after(|| {
                let mut items = vec![item("weapon", WEAPON_TPL)];
                randomise_offer_item_upd_properties(
                    &fixture.ctx(),
                    &mut items,
                    WEAPON_TPL,
                    creator,
                )
                .unwrap();
            });
            assert_eq!(after, untouched_stream());
        }
    }

    #[test]
    fn a_tpl_with_no_condition_entry_costs_no_draw() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut items = vec![item("plain", PLAIN_TPL)];
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                PLAIN_TPL,
                OfferCreator::FakePlayer,
            )
            .unwrap();
        });

        assert_eq!(after, untouched_stream());
    }

    #[test]
    fn a_failed_condition_chance_stops_after_its_single_draw() {
        let fixture = Fixture::with_condition_chance(0.0);

        let after = stream_position_after(|| {
            let mut items = vec![item("weapon", WEAPON_TPL)];
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                WEAPON_TPL,
                OfferCreator::FakePlayer,
            )
            .unwrap();
        });

        assert_eq!(
            after,
            stream_position_after(|| {
                get_chance_100(0.0);
            })
        );
    }

    #[test]
    fn a_passed_condition_chance_randomises_the_condition() {
        let fixture = Fixture::new();
        let mut items = vec![item("weapon", WEAPON_TPL)];

        seeded(|| {
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                WEAPON_TPL,
                OfferCreator::FakePlayer,
            )
            .expect("the weapon template is in the view");
        });

        // The weapon arm rewrote what AddMissingConditions seeded at 100/100.
        let repairable = items[0].upd.as_ref().unwrap().repairable.as_ref().unwrap();
        assert!(repairable.max_durability.unwrap() < 100.0);

        let after = stream_position_after(|| {
            let mut items = vec![item("weapon", WEAPON_TPL)];
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                WEAPON_TPL,
                OfferCreator::FakePlayer,
            )
            .unwrap();
        });
        assert_eq!(
            after,
            stream_position_after(|| {
                get_chance_100(100.0);
                get_double(CONDITION_MAX_MIN, CONDITION_MAX_MIN);
                get_double(CONDITION_CURRENT_MIN, CONDITION_CURRENT_MAX);
                for _ in 0..4 {
                    get_double(0.0, 1.0);
                }
            })
        );
    }

    #[test]
    fn an_unknown_root_template_is_an_error_from_add_missing_conditions() {
        let fixture = Fixture::new();
        let mut items = vec![item("mystery", "no_such_tpl")];

        let error = randomise_offer_item_upd_properties(
            &fixture.ctx(),
            &mut items,
            "no_such_tpl",
            OfferCreator::FakePlayer,
        )
        .expect_err("an unknown tpl errors");

        assert_eq!(
            error.message,
            "Object reference not set to an instance of an object."
        );
    }

    // -----------------------------------------------------------------------
    // get_dynamic_condition_id_for_tpl
    // -----------------------------------------------------------------------

    #[test]
    fn the_first_matching_condition_key_in_insertion_order_wins() {
        let fixture = Fixture::new();

        // The plate's direct parent is ARMOR_PLATE, but ARMORED_EQUIPMENT - its grandparent - is
        // listed first in the config.
        assert_eq!(
            get_dynamic_condition_id_for_tpl(&fixture.ctx(), PLATE_CLASS_6_TPL),
            Some(ARMORED_EQUIPMENT.to_owned())
        );
        assert_eq!(
            get_dynamic_condition_id_for_tpl(&fixture.ctx(), WEAPON_TPL),
            Some(WEAPON.to_owned())
        );
        assert_eq!(
            get_dynamic_condition_id_for_tpl(&fixture.ctx(), PLAIN_TPL),
            None
        );
    }

    // -----------------------------------------------------------------------
    // add_missing_conditions
    // -----------------------------------------------------------------------

    #[test]
    fn the_first_matching_condition_arm_wins_and_returns() {
        let fixture = Fixture::new();
        let mut repairable_medkit = item("both", REPAIRABLE_MEDKIT_TPL);

        add_missing_conditions(&fixture.ctx(), &mut repairable_medkit).unwrap();

        let upd = repairable_medkit.upd.as_ref().unwrap();
        assert_eq!(upd.repairable.as_ref().unwrap().durability, Some(50.0));
        assert_eq!(upd.repairable.as_ref().unwrap().max_durability, Some(50.0));
        assert!(upd.med_kit.is_none(), "the medkit arm must not also fire");
    }

    #[test]
    fn every_condition_arm_writes_its_own_upd_member_without_drawing() {
        let fixture = Fixture::new();

        /// "Did this arm write the member it is supposed to write?"
        type ArmCheck = fn(&Upd) -> bool;

        let cases: [(&str, ArmCheck); 5] = [
            (WEAPON_TPL, |upd| {
                upd.repairable.as_ref().is_some_and(|repairable| {
                    repairable.durability == Some(100.0) && repairable.max_durability == Some(100.0)
                })
            }),
            (MEDKIT_TPL, |upd| {
                upd.med_kit
                    .as_ref()
                    .is_some_and(|med_kit| med_kit.hp_resource == Some(400.0))
            }),
            (KEY_TPL, |upd| {
                upd.key
                    .as_ref()
                    .is_some_and(|key| key.number_of_usages == Some(0))
            }),
            (FOOD_TPL, |upd| {
                upd.food_drink
                    .as_ref()
                    .is_some_and(|food| food.hp_percent == Some(100.0))
            }),
            (REPAIR_KIT_TPL, |upd| {
                upd.repair_kit
                    .as_ref()
                    .is_some_and(|kit| kit.resource == Some(60.0))
            }),
        ];

        for (tpl, check) in cases {
            let mut subject = item("subject", tpl);
            add_missing_conditions(&fixture.ctx(), &mut subject).unwrap();
            assert!(
                check(subject.upd.as_ref().unwrap()),
                "{tpl} was not written"
            );

            let after = stream_position_after(|| {
                let mut subject = item("subject", tpl);
                add_missing_conditions(&fixture.ctx(), &mut subject).unwrap();
            });
            assert_eq!(after, untouched_stream(), "{tpl} drew from the stream");
        }
    }

    #[test]
    fn an_item_with_no_condition_properties_is_left_alone() {
        let fixture = Fixture::new();
        let mut plain = item("plain", PLAIN_TPL);

        add_missing_conditions(&fixture.ctx(), &mut plain).unwrap();

        let upd = plain.upd.as_ref().unwrap();
        assert!(upd.repairable.is_none());
        assert!(upd.med_kit.is_none());
        assert!(upd.key.is_none());
        assert!(upd.food_drink.is_none());
        assert!(upd.repair_kit.is_none());
    }

    // -----------------------------------------------------------------------
    // remove_banned_plates_from_preset
    // -----------------------------------------------------------------------

    #[test]
    fn only_over_level_non_ignored_plates_are_removed() {
        let fixture = Fixture::new();
        let mut preset = armor_with_plates();

        let removed = remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut preset,
            &fixture.dynamic.blacklist.armor_plate,
        );

        assert!(removed);
        // The class 6 front plate went; the ignored class 6 back plate, the class 4 side plate
        // (4 is not > 4) and the soft insert stayed, in their original order.
        let ids: Vec<&str> = preset.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["armor_root", "back", "left", "soft"]);
    }

    #[test]
    fn removal_survives_the_index_shift_it_causes() {
        let fixture = Fixture::new();
        // Two removable plates, so the second one's index moves when the first is removed.
        let mut preset = vec![
            item("armor_root", ARMOR_TPL),
            child("front", PLATE_CLASS_6_TPL, "armor_root", "front_plate"),
            child("left", PLATE_CLASS_6_TPL, "armor_root", "left_side_plate"),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ];

        let removed = remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut preset,
            &fixture.dynamic.blacklist.armor_plate,
        );

        assert!(removed);
        let ids: Vec<&str> = preset.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["armor_root", "soft"]);
    }

    #[test]
    fn a_preset_that_cannot_hold_mods_or_has_no_plate_slots_is_untouched() {
        let fixture = Fixture::new();

        let mut weapon = vec![item("weapon", WEAPON_TPL)];
        assert!(!remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut weapon,
            &fixture.dynamic.blacklist.armor_plate
        ));
        assert_eq!(weapon.len(), 1);

        let mut soft_only = vec![
            item("armor_root", ARMOR_TPL),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ];
        assert!(!remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut soft_only,
            &fixture.dynamic.blacklist.armor_plate
        ));
        assert_eq!(soft_only.len(), 2);
    }

    #[test]
    fn removing_banned_plates_costs_no_draw() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut preset = armor_with_plates();
            remove_banned_plates_from_preset(
                &fixture.ctx(),
                &mut preset,
                &fixture.dynamic.blacklist.armor_plate,
            );
        });

        assert_eq!(after, untouched_stream());
    }

    // -----------------------------------------------------------------------
    // remove_armor_plates
    // -----------------------------------------------------------------------

    #[test]
    fn an_armor_with_no_removable_plate_slots_still_spends_the_chance_draw() {
        let fixture = Fixture::new();
        let mut items = vec![
            item("armor_root", ARMOR_NO_PLATE_SLOTS_TPL),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ];

        seeded(|| remove_armor_plates(&fixture.ctx(), &mut items));

        assert_eq!(items.len(), 2);
        let after = stream_position_after(|| {
            let mut items = vec![
                item("armor_root", ARMOR_NO_PLATE_SLOTS_TPL),
                child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
            ];
            remove_armor_plates(&fixture.ctx(), &mut items);
        });
        assert_eq!(
            after,
            stream_position_after(|| {
                get_chance_100(100.0);
            })
        );
    }

    #[test]
    fn plates_in_the_removal_pool_are_removed_back_to_front() {
        let fixture = Fixture::new();
        let mut items = armor_with_plates();

        seeded(|| remove_armor_plates(&fixture.ctx(), &mut items));

        // The pool holds `front_plate` and `left_side_plate`; `back_plate` is not in it.
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["armor_root", "back", "soft"]);
    }

    #[test]
    fn a_failed_chance_leaves_every_plate_in_place() {
        let fixture = Fixture {
            dynamic: dynamic_config_with_plate_chance(0),
            ..Fixture::new()
        };
        let mut items = armor_with_plates();

        seeded(|| remove_armor_plates(&fixture.ctx(), &mut items));

        assert_eq!(items.len(), 5);
    }

    fn dynamic_config_with_plate_chance(chance: i32) -> DynamicConfigWire {
        let mut dynamic = dynamic_config(1.0);
        dynamic.armor.remove_removable_plate_chance = chance;

        dynamic
    }
}
