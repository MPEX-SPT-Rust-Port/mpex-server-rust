//! `PlayerScavGenerator`'s karma-driven template arithmetic: the chance adjustments and the
//! equipment blacklist. The item-limit half (`AdjustItemWeights`) deliberately stays C#-side —
//! `BotLootCacheService` hydration consumes its whitelists (spec § Seam).

// The FFI export that calls all three of these lands in the following commit.
#![allow(dead_code)]

use crate::bot::models::{BotTypeInventoryWire, ChancesWire, KarmaSettingsWire};

pub(crate) const CATEGORY: &str = "SPTarkov.Server.Core.Generators.Bot.PlayerScavGenerator";

/// `AdjustEquipmentWeights` + `AdjustWeaponModWeights`, applied to the template's chance maps.
pub(crate) fn apply_karma_chances(chances: &mut ChancesWire, karma: &KarmaSettingsWire) {
    for (equipment_slot, chance_to_add) in &karma.equipment_modifiers {
        // Adjustment value zero, nothing to do
        if *chance_to_add == 0.0 {
            continue;
        }
        *chances
            .equipment
            .entry(equipment_slot.clone())
            .or_insert(0.0) += chance_to_add;
    }
    for (mod_slot, weight) in &karma.mod_modifiers {
        if *weight == 0.0 {
            continue;
        }
        // Quirk (`AdjustWeaponModWeights`): the C# re-checks `modChangesToApply` — the dict it is
        // iterating — instead of `weaponModChances`, so the lookup is a tautology and every
        // non-zero weight lands as TryAdd(slot, 0) then `+=`. Ported as the net effect.
        *chances.weapon_mods.entry(mod_slot.clone()).or_insert(0.0) += weight;
    }
}

/// `BlacklistEquipment`: per-slot tpl removals from the inventory equipment dicts. A slot the
/// template does not carry is skipped, matching the C# `TryGetValue` guard.
pub(crate) fn apply_equipment_blacklist(
    inventory: &mut BotTypeInventoryWire,
    karma: &KarmaSettingsWire,
) {
    for (slot, blacklist) in &karma.equipment_blacklist {
        let Some(equipment_dict) = inventory.equipment.get_mut(slot) else {
            continue;
        };
        for item_to_remove in blacklist {
            equipment_dict.shift_remove(item_to_remove);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn karma(equipment: serde_json::Value, mods: serde_json::Value) -> KarmaSettingsWire {
        serde_json::from_value(json!({
            "equipmentModifiers": equipment,
            "modModifiers": mods,
            "equipmentBlacklist": {},
            "lootItemsToAddChancePercent": {},
        }))
        .unwrap()
    }

    fn chances() -> ChancesWire {
        serde_json::from_value(json!({
            "equipment": { "Headwear": 30.0 },
            "weaponMods": { "mod_scope": 10.0 },
            "equipmentMods": {},
        }))
        .unwrap()
    }

    #[test]
    fn a_zero_modifier_is_skipped_on_both_maps() {
        let mut subject = chances();
        apply_karma_chances(
            &mut subject,
            &karma(json!({ "Headwear": 0.0 }), json!({ "mod_scope": 0.0 })),
        );
        assert_eq!(subject.equipment["Headwear"], 30.0);
        assert_eq!(subject.weapon_mods["mod_scope"], 10.0);
    }

    #[test]
    fn an_existing_equipment_key_adds_and_a_new_key_inserts() {
        let mut subject = chances();
        apply_karma_chances(
            &mut subject,
            &karma(json!({ "Headwear": 5.0, "Earpiece": 40.0 }), json!({})),
        );
        assert_eq!(subject.equipment["Headwear"], 35.0);
        assert_eq!(subject.equipment["Earpiece"], 40.0);
    }

    #[test]
    fn mod_weights_add_through_the_tautology_for_present_and_absent_keys() {
        let mut subject = chances();
        apply_karma_chances(
            &mut subject,
            &karma(json!({}), json!({ "mod_scope": 5.0, "mod_stock": 7.0 })),
        );
        assert_eq!(subject.weapon_mods["mod_scope"], 15.0);
        assert_eq!(subject.weapon_mods["mod_stock"], 7.0);
    }

    #[test]
    fn blacklisted_tpls_leave_their_slot_and_a_missing_slot_is_skipped() {
        let mut inventory: BotTypeInventoryWire = serde_json::from_value(json!({
            "equipment": { "Headwear": { "tpl_helmet": 1.0, "tpl_cap": 2.0 } },
            "Ammo": {},
            "items": { "Backpack": {}, "Pockets": {}, "SecuredContainer": {}, "SpecialLoot": {}, "TacticalVest": {} },
            "mods": {},
        }))
        .unwrap();
        let karma: KarmaSettingsWire = serde_json::from_value(json!({
            "equipmentModifiers": {},
            "modModifiers": {},
            "equipmentBlacklist": { "Headwear": ["tpl_helmet"], "Earpiece": ["tpl_ears"] },
            "lootItemsToAddChancePercent": {},
        }))
        .unwrap();
        apply_equipment_blacklist(&mut inventory, &karma);
        assert_eq!(
            inventory.equipment["Headwear"].keys().collect::<Vec<_>>(),
            ["tpl_cap"]
        );
    }
}
