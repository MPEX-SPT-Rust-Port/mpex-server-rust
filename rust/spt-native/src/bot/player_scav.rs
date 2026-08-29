//! `PlayerScavGenerator.Generate` (`:51-139`) minus its profile bookkeeping: the karma-driven
//! template arithmetic (chance adjustments, equipment blacklist), the bot generation itself, and
//! the extra-loot pass that follows it. The item-limit half (`AdjustItemWeights`) deliberately
//! stays C#-side — `BotLootCacheService` hydration consumes its whitelists (spec § Seam).

use indexmap::IndexMap;

use crate::bot::bot_generator_helper::{
    ContainerGrids, ItemAddedResult, generate_extra_properties_for_item,
};
use crate::bot::bot_inventory_generator::{PreparedBot, generate_prepared_with};
use crate::bot::models::{
    BotInventoryResult, BotTypeInventoryWire, ChancesWire, GeneratePlayerScavRequest,
    KarmaSettingsWire,
};
use crate::bot::{BotContext, resolve_bot_views, resolve_equipment};
use crate::loot::item_helper::{LootEpochError, LootError, get_item};
use crate::loot::models::{DEBUG, Diagnostic, Item, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{TestSeedGuard, get_chance_100};

pub(crate) const CATEGORY: &str = "SPTarkov.Server.Core.Generators.Bot.PlayerScavGenerator";

/// The pscav container try-order: the C# `HashSet<EquipmentSlots>` literal
/// `[TacticalVest, Pockets, Backpack]` (`:92`), iterated in insertion order.
const ADDITIONAL_LOOT_CONTAINERS: [&str; 3] = ["TacticalVest", "Pockets", "Backpack"];

/// `PlayerScavGenerator.Generate` (`:51-139`), native half: karma onto the template, then the bot,
/// then the extra-loot pass (`:88-93`) that runs against the still-live container grids.
///
/// # Errors
///
/// [`LootEpochError::StaleEpoch`] when an override-less request names an epoch the resident DB
/// does not hold, otherwise everything the generation phases raise.
pub fn generate_player_scav(
    request: GeneratePlayerScavRequest,
) -> Result<BotInventoryResult, LootEpochError> {
    // Same ordering as generate_inventory: views before the seed guard, so a stale epoch never
    // touches the RNG.
    let views = resolve_bot_views(request.epoch, request.views_override)?;
    let _seed_guard = request.bot.test_seed.map(TestSeedGuard::install);
    let equipment = resolve_equipment(&views, &request.shared.live_equipment_mods);

    let mut template = request.template;
    apply_karma_chances(&mut template.chances, &request.karma);
    apply_equipment_blacklist(&mut template.inventory, &request.karma);

    let prepared = PreparedBot {
        details: request.bot.details,
        template,
        loot_pools: request.loot_pools,
    };
    let karma = request.karma;

    Ok(generate_prepared_with(
        &request.shared,
        &views,
        &equipment,
        prepared,
        |ctx, grids, items| {
            add_additional_loot(ctx, grids, items, &karma.loot_items_to_add_chance_percent)
        },
    )?)
}

/// `AddAdditionalLootToPlayerScavContainers` (`:148-198`): independent per-item rolls in insertion
/// order — NOT a weighted pool draw.
///
/// # Errors
///
/// Everything [`generate_extra_properties_for_item`] raises.
fn add_additional_loot(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    inventory_items: &mut Vec<Item>,
    possible_items_to_add: &IndexMap<String, f64>,
) -> Result<(), LootError> {
    let items = ctx.items;
    let containers: Vec<String> = ADDITIONAL_LOOT_CONTAINERS
        .iter()
        .map(|slot| (*slot).to_owned())
        .collect();

    for (tpl, chance) in possible_items_to_add {
        let should_add = get_chance_100(*chance);
        if !should_add {
            continue;
        }

        let Some(item_template) = get_item(items, tpl) else {
            ctx.diagnostics.push(Diagnostic {
                category: CATEGORY,
                level: WARNING.to_owned(),
                locale_key: Some("scav-unable_to_add_item_to_player_scav".to_owned()),
                // Quirk 1 (`:166`): `GetText` binds the `(string, object)` overload — the argument
                // is a `KeyValuePair`, which is not `IConvertible` — and that overload's
                // `{{prop}}` replacement finds no placeholders in this template, so 4.1.2 logs the
                // literal unsubstituted `%s`. `args: None` reproduces that byte-for-byte; a scalar
                // argument here would substitute the `%s` and diverge.
                args: None,
                message: None,
            });

            continue;
        };

        let root_id = mongo_id::generate();
        let mut items_to_add = vec![Item {
            id: root_id.clone(),
            template: tpl.clone(),
            upd: generate_extra_properties_for_item(ctx, item_template, Some("assault"), true)?,
            ..Default::default()
        }];

        let result = grids.add_item_with_children_to_equipment_slot(
            ctx,
            &containers,
            &root_id,
            tpl,
            &mut items_to_add,
            inventory_items,
        );

        if result != ItemAddedResult::Success {
            ctx.diagnostics.push(Diagnostic {
                category: CATEGORY,
                level: DEBUG.to_owned(),
                locale_key: None,
                args: None,
                // Quirk 2 (`:194`): the message names a keycard whatever the item actually was,
                // and interpolates the C# enum member name.
                message: Some(format!(
                    "Unable to add keycard to bot. Reason: {}",
                    c_sharp_name(result)
                )),
            });
        }
    }

    Ok(())
}

/// `ItemAddedResult.ToString()` — the C# member names, which are SCREAMING_CASE.
fn c_sharp_name(result: ItemAddedResult) -> &'static str {
    match result {
        ItemAddedResult::Unknown => "UNKNOWN",
        ItemAddedResult::Success => "SUCCESS",
        ItemAddedResult::NoSpace => "NO_SPACE",
        ItemAddedResult::NoContainers => "NO_CONTAINERS",
        ItemAddedResult::IncompatibleItem => "INCOMPATIBLE_ITEM",
    }
}

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
    use serde_json::{Value, json};

    use crate::bot::bot_inventory_generator::tests::base_request;
    use crate::bot::durability_limits_helper::BotDurability;
    use crate::bot::models::{EquipmentFilters, RandomisedResourceDetails};
    use crate::diag::DiagSink;
    use crate::loot::models::ItemView;

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
            "equipment": { "Headwear": { "tpl_helmet": 1.0, "tpl_cap": 2.0, "tpl_visor": 3.0 } },
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
        // Survivor *order* is the assertion: `swap_remove` would leave `tpl_visor` first.
        assert_eq!(
            inventory.equipment["Headwear"].keys().collect::<Vec<_>>(),
            ["tpl_cap", "tpl_visor"]
        );
        // The `TryGetValue` guard skips the absent slot rather than creating it.
        assert!(!inventory.equipment.contains_key("Earpiece"));
    }

    // -----------------------------------------------------------------------
    // The entry point
    // -----------------------------------------------------------------------

    /// A 1x1 template the extra-loot pass can actually place, and one nothing knows about.
    const EXTRA_KEYCARD_TPL: &str = "extra_keycard";
    const NEVER_TPL: &str = "never_item";

    /// The canonical single-bot override request plus the karma slice. `base_request` already
    /// carries `epoch: 0`, a full `viewsOverride` and a `testSeed`, so the pscav request is that
    /// request with one member added — which also pins both entry points to the same fixture.
    fn player_scav_request(karma: Value) -> Value {
        let mut request = base_request();
        request["viewsOverride"]["items"][EXTRA_KEYCARD_TPL] =
            json!({"name": "keycard", "width": 1, "height": 1});
        request["karma"] = karma;

        request
    }

    /// A stale epoch cannot happen on an override send, so every failure here is the family's
    /// fatal error.
    fn generate(request: Value) -> Result<BotInventoryResult, LootError> {
        generate_player_scav(serde_json::from_value(request).unwrap()).map_err(
            |error| match error {
                LootEpochError::Loot(error) => error,
                LootEpochError::StaleEpoch => panic!("unexpected stale epoch"),
            },
        )
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
    fn a_seeded_run_is_reproducible_and_karma_reaches_the_chances() {
        // `MongoId`s come from process entropy, not the seeded stream, so the comparable part of a
        // run is every field except the ids (the reading `bot_inventory_generator`'s own
        // reproducibility test takes).
        let rolled = |result: &BotInventoryResult| -> Vec<(Option<String>, String, Value)> {
            result
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
        let request = || {
            player_scav_request(json!({
                "equipmentModifiers": { "Headwear": -100.0 },
                "modModifiers": {},
                "equipmentBlacklist": {},
                "lootItemsToAddChancePercent": {},
            }))
        };

        let first = generate(request()).unwrap();
        let second = generate(request()).unwrap();

        assert_eq!(rolled(&first), rolled(&second));
        // -100 drives the Headwear slot chance to zero: no Headwear item generates.
        assert!(!worn(&first).iter().any(|(slot, _)| slot == "Headwear"));
    }

    #[test]
    fn a_certain_additional_item_is_added_and_an_impossible_one_is_not() {
        // get_chance_100 rolls get_int(1, 99): >= 99 always fires, < 1 never does.
        let result = generate(player_scav_request(json!({
            "equipmentModifiers": {},
            "modModifiers": {},
            "equipmentBlacklist": {},
            "lootItemsToAddChancePercent": { EXTRA_KEYCARD_TPL: 100.0, NEVER_TPL: 0.0 },
        })))
        .unwrap();

        // Without a worn, registered container the add is `NoContainers` — a swallowed DEBUG line,
        // not an error — so pin the container before pinning the item.
        assert!(worn(&result).iter().any(|(slot, _)| slot == "TacticalVest"));
        assert!(result.container_grids.contains_key("TacticalVest"));

        let tpls: Vec<&str> = result
            .inventory
            .items
            .iter()
            .map(|item| item.template.as_str())
            .collect();
        assert!(tpls.contains(&EXTRA_KEYCARD_TPL));
        assert!(!tpls.contains(&NEVER_TPL));
    }

    /// A second bare 1x1 template, registered only by the mid-range case below. Registration is
    /// what makes the pin mean anything: `add_additional_loot` rolls *before* it looks the tpl up,
    /// so an unregistered tpl still consumes its draw - but it can then only reach the
    /// warn-and-skip arm, which would make the absence assert pass on a hit as readily as on a
    /// miss. 1x1 with no durability/HP/stack additionally keeps a *firing* entry from drawing again
    /// inside `generate_extra_properties_for_item`, which would move the stream.
    const MID_RANGE_MISS_TPL: &str = "mid_range_trinket";

    /// Empirical (see the case below): a seed at which the 27.0 entry fires and the 3.0 one does
    /// not. It overrides `base_request`'s own `testSeed`, at which the 27.0 entry misses - so the
    /// case would pin two absences and never observe a placement.
    const MID_RANGE_SEED: u64 = 2;

    /// Where an additional-loot item landed: the worn container's own slot id, the grid inside it
    /// the item was written to, and the item's position in that grid. Deliberately not the parent
    /// id - `MongoId`s are freshly minted per run.
    fn placement(
        result: &BotInventoryResult,
        tpl: &str,
    ) -> (Option<String>, Option<String>, Value) {
        let item = result
            .inventory
            .items
            .iter()
            .find(|item| item.template == tpl)
            .expect("the additional-loot item is missing");
        let parent = result
            .inventory
            .items
            .iter()
            .find(|candidate| Some(&candidate.id) == item.parent_id.as_ref())
            .expect("the additional-loot item hangs off an unknown parent");

        (
            parent.slot_id.clone(),
            item.slot_id.clone(),
            serde_json::to_value(&item.location).unwrap(),
        )
    }

    #[test]
    fn mid_range_additional_loot_chances_are_deterministic_at_a_seed() {
        // Every shipped lootItemsToAddChancePercent value is 3-27, but get_chance_100 rolls
        // get_int(1, 99): >= 99 always fires and < 1 never does, so the 100/0 entries the sibling
        // case uses short-circuit and leave the whole shipped band untested. One draw is consumed
        // per entry whatever the outcome, so a two-entry map pins the outcomes *and* the stream
        // position. Pinned at MID_RANGE_SEED:
        //   EXTRA_KEYCARD_TPL @ 27.0 -> added
        //   MID_RANGE_MISS_TPL @ 3.0 -> not added
        let request = || {
            let mut request = player_scav_request(json!({
                "equipmentModifiers": {},
                "modModifiers": {},
                "equipmentBlacklist": {},
                "lootItemsToAddChancePercent": {
                    EXTRA_KEYCARD_TPL: 27.0,
                    MID_RANGE_MISS_TPL: 3.0,
                },
            }));
            request["viewsOverride"]["items"][MID_RANGE_MISS_TPL] =
                json!({"name": "trinket", "width": 1, "height": 1});
            request["bot"]["testSeed"] = json!(MID_RANGE_SEED);

            request
        };

        let first = generate(request()).unwrap();

        let tpls: Vec<&str> = first
            .inventory
            .items
            .iter()
            .map(|item| item.template.as_str())
            .collect();
        assert!(
            tpls.contains(&EXTRA_KEYCARD_TPL),
            "the 27.0 entry's outcome flipped at this seed: the stream moved"
        );
        assert!(
            !tpls.contains(&MID_RANGE_MISS_TPL),
            "the 3.0 entry's outcome flipped at this seed: the stream moved"
        );

        // Each generate re-seeds from `testSeed`, so the same request must repeat both outcomes and
        // place the hit identically.
        let second = generate(request()).unwrap();
        assert!(
            !second
                .inventory
                .items
                .iter()
                .any(|item| item.template == MID_RANGE_MISS_TPL),
            "the 3.0 entry fired on the re-run: the seed is not reproducing"
        );
        assert_eq!(
            placement(&first, EXTRA_KEYCARD_TPL),
            placement(&second, EXTRA_KEYCARD_TPL),
            "the same seed placed the additional loot somewhere else"
        );
    }

    /// Owned fixtures a bare [`BotContext`] can borrow from: `generate_prepared` hard-codes
    /// `DiagSink::Pipeline`, so the warning arm is only observable on the leaf call.
    struct Fixture {
        items: IndexMap<String, ItemView>,
        bosses: Vec<String>,
        durability: BotDurability,
        equipment: IndexMap<String, EquipmentFilters>,
        randomization: IndexMap<String, RandomisedResourceDetails>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                items: IndexMap::new(),
                bosses: Vec::new(),
                durability: serde_json::from_value(json!({
                    "default": {
                        "armor": {"maxDelta": 10, "minDelta": 0, "minLimitPercent": 15},
                        "weapon": {"lowestMax": 60, "highestMax": 100, "maxDelta": 10,
                            "minDelta": 0, "minLimitPercent": 15.0}
                    },
                    "botDurabilities": {},
                    "pmc": {
                        "armor": {"lowestMaxPercent": 90, "highestMaxPercent": 100, "maxDelta": 10,
                            "minDelta": 0, "minLimitPercent": 15},
                        "weapon": {"lowestMax": 95, "highestMax": 100, "maxDelta": 5,
                            "minDelta": 0, "minLimitPercent": 15.0}
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
                item_blacklist: &crate::bot::NO_BLACKLIST,
                default_presets_by_tpl: &crate::bot::NO_DEFAULT_PRESETS,
                equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                weapon_mod_equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                low_profile_gas_block_tpls: &crate::bot::NO_BLACKLIST,
                item_presets: &crate::bot::NO_PRESETS,
                weapon_has_enhancement_chance_percent: 0.0,
                repair_kit_weapon: &crate::bot::NO_BUFFS,
                secure_container_ammo_stack_count: 0,
                is_night_time: false,
                diagnostics: DiagSink::capture(),
            }
        }
    }

    #[test]
    fn an_unknown_additional_tpl_warns_and_skips() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let mut grids = ContainerGrids::default();
        let mut items = Vec::new();
        let to_add = IndexMap::from([("ghost_tpl".to_owned(), 100.0)]);

        assert!(add_additional_loot(&mut ctx, &mut grids, &mut items, &to_add).is_ok());

        assert!(items.is_empty());
        assert_eq!(ctx.diagnostics.captured().len(), 1);
        let warning = &ctx.diagnostics.captured()[0];
        assert_eq!(warning.level, WARNING);
        assert_eq!(
            warning.locale_key.as_deref(),
            Some("scav-unable_to_add_item_to_player_scav")
        );
        assert!(warning.args.is_none());
    }
}
