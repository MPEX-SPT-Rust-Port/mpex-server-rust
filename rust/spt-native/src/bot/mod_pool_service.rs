//! `Services/Bot/BotEquipmentModPoolService.cs`, derived per call instead of cached.
//!
//! # Why no cache
//!
//! The C# service builds two whole-database pools once (`GenerateGearPool` / `GenerateWeaponPool`
//! → `GeneratePool`) and answers every lookup out of them. That walk is a breadth-first sweep, but
//! the entry it writes for an item comes **only from that item's own `Properties.Slots`** — the
//! queue decides *which* ids end up with an entry, never what an entry contains. So one lookup is
//! exactly:
//!
//! 1. is this tpl in the pool's key set at all, and
//! 2. if so, its own slots, filtered.
//!
//! Step 2 is [`derive_pool`] below. Step 1 is the base-class test the two `Generate*Pool` methods
//! apply to `templateTable.Items` before handing them to `GeneratePool`. `GeneratePool` also adds
//! sub-mods reached through a slot filter, which in the live database adds nothing on top of the
//! base-class test: every tpl in a weapon slot filter is a `MOD` descendant and every tpl in a gear
//! slot filter is an `ARMORED_EQUIPMENT`/`MOD` descendant, and both input sets already include
//! those classes.
//!
//! Deriving on demand is what makes `ResetWeaponPool()` irrelevant here — there is no stale state
//! for a mod to invalidate.
//!
//! **Deviation:** the C# input filter is `_type == "Item" && IsOfBaseclasses(...)`. The type half is
//! applied only when the payload carries `type` at all, so an items view built without it still
//! answers from the base-class half rather than going silently empty. The half exists to keep
//! base-class *nodes* out of the pool, and no call site looks a node up.
//!
//! # RNG calls
//!
//! None. Every function here is a pure read of the items view.
#![allow(
    dead_code,
    reason = "consumed by the equipment- and weapon-mod paths in tasks 7 and 8"
)]

use indexmap::{IndexMap, IndexSet};

use crate::bot::BotContext;
use crate::loot::item_helper::{
    ARMOR, ARMORED_EQUIPMENT, HEADWEAR, MOD, VEST, WEAPON, get_item, is_of_baseclasses,
};
use crate::loot::models::{Diagnostic, ItemView, WARNING};

/// `BotEquipmentModPoolService.GetModsForGearSlot` (`:154-157`), against the pool
/// `GenerateGearPool` (`:215-226`) would have built.
pub fn get_mods_for_gear_slot(
    ctx: &BotContext,
    item_tpl: &str,
) -> IndexMap<String, IndexSet<String>> {
    if !is_in_pool(
        ctx.items,
        item_tpl,
        &[ARMORED_EQUIPMENT, VEST, ARMOR, HEADWEAR, MOD],
    ) {
        return IndexMap::new();
    }

    derive_pool(ctx.items, item_tpl)
}

/// `BotEquipmentModPoolService.GetModsForWeaponSlot` (`:164-167`), against the pool
/// `GenerateWeaponPool` (`:202-210`) would have built.
pub fn get_mods_for_weapon_slot(
    ctx: &BotContext,
    item_tpl: &str,
) -> IndexMap<String, IndexSet<String>> {
    if !is_in_pool(ctx.items, item_tpl, &[WEAPON, MOD]) {
        return IndexMap::new();
    }

    derive_pool(ctx.items, item_tpl)
}

/// `BotEquipmentModPoolService.GetCompatibleModsForWeaponSlot` (`:135-147`) — the warning fires on
/// a miss just as it does in C#, whose "in cache" wording is kept verbatim.
pub fn get_compatible_mods_for_weapon_slot(
    ctx: &mut BotContext,
    item_tpl: &str,
    slot_name: &str,
) -> IndexSet<String> {
    if let Some(tpls_for_slot) = get_mods_for_weapon_slot(ctx, item_tpl).swap_remove(slot_name) {
        return tpls_for_slot;
    }

    ctx.diagnostics.push(Diagnostic {
        level: WARNING.to_owned(),
        locale_key: None,
        args: None,
        message: Some(format!(
            "Slot: {slot_name} not found for item: {item_tpl} in cache"
        )),
    });

    IndexSet::new()
}

/// `BotEquipmentModPoolService.GetRequiredModsForWeaponSlot` (`:174-197`) — a direct read of the
/// template, never the pool, so no base-class test applies and a required slot with an empty filter
/// still gets an (empty) entry.
///
/// **Deviation:** C# dereferences `slot.Properties!.Filters!.FirstOrDefault()!.Filter!` and throws
/// on a required slot without a filter; that lands here as the same empty entry, since a panic
/// behind the FFI boundary is worse and no live template hits it.
pub fn get_required_mods_for_weapon_slot(
    ctx: &BotContext,
    item_tpl: &str,
) -> IndexMap<String, IndexSet<String>> {
    let mut result: IndexMap<String, IndexSet<String>> = IndexMap::new();

    let slots = get_item(ctx.items, item_tpl)
        .and_then(|item| item.slots.as_deref())
        .unwrap_or_default();

    for slot in slots.iter().filter(|slot| slot.required.unwrap_or(false)) {
        let entry = result
            .entry(slot.name.clone().unwrap_or_default())
            .or_default();

        entry.extend(slot.filter.iter().flatten().cloned());
    }

    result
}

/// The per-item half of `GeneratePool` (`:53-119`): each slot with a non-empty first filter becomes
/// an entry keyed by the slot name. Slots sharing a name merge, as C#'s `GetOrAdd` does.
fn derive_pool(
    items: &IndexMap<String, ItemView>,
    item_tpl: &str,
) -> IndexMap<String, IndexSet<String>> {
    let mut pool: IndexMap<String, IndexSet<String>> = IndexMap::new();

    let slots = get_item(items, item_tpl)
        .and_then(|item| item.slots.as_deref())
        .unwrap_or_default();

    for slot in slots {
        // No mod items in whitelist, skip
        let compatible_mods = slot.filter.as_deref().unwrap_or_default();
        if compatible_mods.is_empty() {
            continue;
        }

        pool.entry(slot.name.clone().unwrap_or_default())
            .or_default()
            .extend(compatible_mods.iter().cloned());
    }

    pool
}

/// The `templateTable.Items.Values.Where(...)` filter both `Generate*Pool` methods apply before
/// building their pool — see the module docs for the two halves.
fn is_in_pool(
    items: &IndexMap<String, ItemView>,
    item_tpl: &str,
    base_class_tpls: &[&str],
) -> bool {
    let is_item_type = get_item(items, item_tpl)
        .and_then(|item| item.item_type.as_deref())
        .is_none_or(|item_type| item_type.eq_ignore_ascii_case("Item"));

    is_item_type && is_of_baseclasses(items, item_tpl, base_class_tpls)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    use crate::bot::durability_limits_helper::BotDurability;
    use crate::bot::models::{EquipmentFilters, RandomisedResourceDetails};

    const WEAPON_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const SCOPE_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const SCOPE_MOD_TPL: &str = "cccccccccccccccccccccccc";
    const MAGAZINE_TPL: &str = "dddddddddddddddddddddddd";
    const PLATE_CARRIER_TPL: &str = "eeeeeeeeeeeeeeeeeeeeeeee";
    const PLATE_TPL: &str = "ffffffffffffffffffffffff";

    struct Fixture {
        items: IndexMap<String, ItemView>,
        bosses: Vec<String>,
        durability: BotDurability,
        equipment: IndexMap<String, EquipmentFilters>,
        randomization: IndexMap<String, RandomisedResourceDetails>,
    }

    impl Fixture {
        /// A weapon with three slots (one required with a filter, one optional, one required with an
        /// empty filter), a scope that has a sub-slot of its own, and a plate carrier with two plate
        /// slots.
        fn new() -> Self {
            Self {
                items: serde_json::from_value(json!({
                    WEAPON: {"type": "Node"},
                    MOD: {"type": "Node"},
                    VEST: {"type": "Node"},
                    ARMORED_EQUIPMENT: {"type": "Node"},
                    WEAPON_TPL: {"parent": WEAPON, "type": "Item", "slots": [
                        {"name": "mod_magazine", "required": true, "filter": [MAGAZINE_TPL]},
                        {"name": "mod_scope", "required": false, "filter": [SCOPE_TPL]},
                        {"name": "mod_stock", "required": true, "filter": []},
                    ]},
                    SCOPE_TPL: {"parent": MOD, "type": "Item", "slots": [
                        {"name": "mod_scope", "filter": [SCOPE_MOD_TPL]},
                    ]},
                    SCOPE_MOD_TPL: {"parent": MOD, "type": "Item"},
                    MAGAZINE_TPL: {"parent": MOD, "type": "Item"},
                    PLATE_CARRIER_TPL: {"parent": VEST, "type": "Item", "slots": [
                        {"name": "front_plate", "required": true, "filter": [PLATE_TPL]},
                        {"name": "back_plate", "filter": [PLATE_TPL]},
                    ]},
                    PLATE_TPL: {"parent": ARMORED_EQUIPMENT, "type": "Item"},
                }))
                .unwrap(),
                bosses: Vec::new(),
                // Unread here, but `BotContext` carries it.
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
                diagnostics: Vec::new(),
            }
        }
    }

    fn pool(entries: &[(&str, &[&str])]) -> IndexMap<String, IndexSet<String>> {
        entries
            .iter()
            .map(|(slot, tpls)| {
                (
                    (*slot).to_owned(),
                    tpls.iter().map(|tpl| (*tpl).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn weapon_pool_keeps_slots_with_a_filter_and_drops_the_empty_one() {
        let fixture = Fixture::new();

        assert_eq!(
            get_mods_for_weapon_slot(&fixture.ctx(), WEAPON_TPL),
            pool(&[
                ("mod_magazine", &[MAGAZINE_TPL][..]),
                ("mod_scope", &[SCOPE_TPL]),
            ])
        );
    }

    #[test]
    fn weapon_pool_covers_a_sub_mods_own_slots() {
        let fixture = Fixture::new();

        // The scope is a MOD, so `GeneratePool` reaches it both as an input and as a sub-mod.
        assert_eq!(
            get_mods_for_weapon_slot(&fixture.ctx(), SCOPE_TPL),
            pool(&[("mod_scope", &[SCOPE_MOD_TPL][..])])
        );
    }

    #[test]
    fn gear_pool_holds_the_plate_slots() {
        let fixture = Fixture::new();

        assert_eq!(
            get_mods_for_gear_slot(&fixture.ctx(), PLATE_CARRIER_TPL),
            pool(&[
                ("front_plate", &[PLATE_TPL][..]),
                ("back_plate", &[PLATE_TPL]),
            ])
        );
    }

    /// The two pools are built from disjoint base-class sets, so a weapon is absent from the gear
    /// pool and a vest from the weapon pool even though both have slots.
    #[test]
    fn the_two_pools_do_not_overlap() {
        let fixture = Fixture::new();

        assert!(get_mods_for_gear_slot(&fixture.ctx(), WEAPON_TPL).is_empty());
        assert!(get_mods_for_weapon_slot(&fixture.ctx(), PLATE_CARRIER_TPL).is_empty());
        // A tpl missing from the view is in neither.
        assert!(get_mods_for_weapon_slot(&fixture.ctx(), "999999999999999999999999").is_empty());
    }

    /// A mod is in *both* input sets, so its own slots answer either lookup.
    #[test]
    fn a_mod_belongs_to_both_pools() {
        let fixture = Fixture::new();

        assert_eq!(
            get_mods_for_gear_slot(&fixture.ctx(), SCOPE_TPL),
            get_mods_for_weapon_slot(&fixture.ctx(), SCOPE_TPL)
        );
    }

    #[test]
    fn required_mods_include_an_empty_filter_slot_and_skip_optional_ones() {
        let fixture = Fixture::new();

        // mod_scope is optional; mod_stock is required with an empty filter, which the pool drops
        // and this does not.
        assert_eq!(
            get_required_mods_for_weapon_slot(&fixture.ctx(), WEAPON_TPL),
            pool(&[("mod_magazine", &[MAGAZINE_TPL][..]), ("mod_stock", &[])])
        );
    }

    /// No base-class test guards this one — it reads the template directly.
    #[test]
    fn required_mods_answer_for_gear_too_and_are_empty_for_an_unknown_tpl() {
        let fixture = Fixture::new();

        assert_eq!(
            get_required_mods_for_weapon_slot(&fixture.ctx(), PLATE_CARRIER_TPL),
            pool(&[("front_plate", &[PLATE_TPL][..])])
        );
        assert!(
            get_required_mods_for_weapon_slot(&fixture.ctx(), "999999999999999999999999")
                .is_empty()
        );
    }

    #[test]
    fn compatible_mods_answer_one_slot_and_warn_on_a_miss() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let found = get_compatible_mods_for_weapon_slot(&mut ctx, WEAPON_TPL, "mod_scope");

        assert_eq!(found, IndexSet::from([SCOPE_TPL.to_owned()]));
        assert!(ctx.diagnostics.is_empty());

        let missing = get_compatible_mods_for_weapon_slot(&mut ctx, WEAPON_TPL, "mod_stock");

        assert!(missing.is_empty());
        assert_eq!(ctx.diagnostics.len(), 1);
        assert_eq!(ctx.diagnostics[0].level, WARNING);
        assert_eq!(
            ctx.diagnostics[0].message.as_deref(),
            Some(format!("Slot: mod_stock not found for item: {WEAPON_TPL} in cache").as_str())
        );
    }
}
