//! `Generators/Ragfair/RagfairAssortGenerator.cs` — the assort walk: every preset the flea sells,
//! then every sellable template, as a list of (root + children) lists.
//!
//! **There is no RNG in this module.** Nothing here draws, so a pass leaves the seeded stream
//! exactly where it found it (pinned by `the_whole_walk_leaves_the_stream_untouched`). The fresh ids
//! `replace_ids`/`remap_root_item_id` mint come from `loot::mongo_id`, a process-local counter that
//! is not part of the draw stream. Do not go looking for a draw table here.

use std::collections::HashSet;

use serde_json::json;

use super::RagfairContext;
use crate::loot::item_helper::{
    BUILT_IN_INSERTS, INVENTORY, LOOT_CONTAINER, LootError, POCKETS, SORTING_TABLE, STASH,
    STATIONARY_CONTAINER, is_valid_item, remap_root_item_id, replace_ids,
};
use crate::loot::models::{Item, PresetView, Upd};
use crate::loot::mongo_id;

/// `RagfairAssortGenerator.RagfairItemInvalidBaseTypes` (`:30-39`).
///
/// Deliberately **not** `ItemHelper::DEFAULT_INVALID_BASE_TYPES`: this list drops `MOB_CONTAINER`
/// (so secure containers reach the flea) and adds `BUILT_IN_INSERTS`. Two lists, two call sites.
pub const RAGFAIR_ITEM_INVALID_BASE_TYPES: [&str; 7] = [
    LOOT_CONTAINER, // Safe, barrel cache etc
    STASH,          // Player inventory stash
    SORTING_TABLE,
    INVENTORY,
    STATIONARY_CONTAINER,
    POCKETS,
    BUILT_IN_INSERTS,
];

/// `RagfairAssortGenerator.GenerateRagfairAssortItems` (`:45-106`) — a list of lists (item +
/// children) the flea can sell, presets first and items second.
///
/// The C# accumulates with `results = results.Union([...])` (`:47/79/102`). `IEnumerable.Union`
/// de-duplicates by reference and every list here is freshly allocated, so it never drops one and
/// keeps first-seen order: a plain push is output-equivalent. Do not "restore" the `Union`.
///
/// # Errors
///
/// Where the C# throws: a preset with no items. `RemapRootItemId` (`ItemExtensions.cs:428`)
/// dereferences `FirstOrDefault()` unguarded, and this port's `remap_root_item_id` returns instead
/// of panicking behind the FFI boundary, so the empty case is caught here.
pub fn generate_ragfair_assort_items(ctx: &RagfairContext) -> Result<Vec<Vec<Item>>, LootError> {
    let mut results: Vec<Vec<Item>> = Vec::new();

    // Get cloned items from db
    let db_items = ctx.items.iter().filter(|(tpl, item)| {
        !item
            .item_type
            .as_deref()
            .unwrap_or_default()
            .eq_ignore_ascii_case("Node")
            && !ctx.config_blacklist.contains(*tpl)
    });

    // Store processed preset tpls so we don't add them when processing non-preset items
    let mut processed_armor_items: HashSet<&str> = HashSet::new();
    let seasonal_event_active = ctx.seasonal_event_active;
    let seasonal_item_tpl_blacklist = ctx.seasonal_item_tpl_blacklist;

    let presets = get_presets_to_add(ctx);
    for preset in presets {
        // The preset's base item tpl, read off the *original* preset before the id remap below —
        // `:68` reads `preset.Items[0]`, not the clone.
        let Some(root_tpl) = preset.items.first().map(|item| item.template.as_str()) else {
            return Err(LootError::new(format!(
                "Preset {} has no items, unable to generate a ragfair assort",
                preset.id.as_deref().unwrap_or_default()
            )));
        };

        // Update Ids and clone
        let mut preset_and_mods_clone = preset.items.clone();
        replace_ids(&mut preset_and_mods_clone);
        remap_root_item_id(&mut preset_and_mods_clone);

        // Add presets base item tpl to the processed list so its skipped later on when processing
        // items
        processed_armor_items.insert(root_tpl);

        let clone_root = &mut preset_and_mods_clone[0];
        clone_root.parent_id = Some("hideout".to_owned());
        clone_root.slot_id = Some("hideout".to_owned());
        let mut upd = unlimited_upd();
        upd.extra.insert("sptPresetId".to_owned(), json!(preset.id));
        clone_root.upd = Some(upd);

        results.push(preset_and_mods_clone);
    }

    for (tpl, _) in db_items {
        if !is_valid_item(
            ctx.items,
            ctx.base_classes,
            ctx.config_blacklist,
            ctx.handbook_prices,
            ctx.flea_prices,
            tpl,
            &RAGFAIR_ITEM_INVALID_BASE_TYPES,
        ) {
            continue;
        }

        // Skip seasonal items when not in-season
        if ctx.dynamic.remove_seasonal_items_when_not_in_event
            && !seasonal_event_active
            && seasonal_item_tpl_blacklist.contains(tpl)
        {
            continue;
        }

        // Already processed
        if processed_armor_items.contains(tpl.as_str()) {
            continue;
        }

        // tpl and id must be the same so hideout recipe rewards work
        results.push(vec![create_ragfair_assort_root_item(tpl, Some(tpl))]);
    }

    Ok(results)
}

/// `RagfairAssortGenerator.GetPresetsToAdd` (`:113-118`) — `showDefaultPresetsOnly` picks between
/// the default presets and every preset in `GlobalTable.ItemPresets`.
fn get_presets_to_add<'a>(ctx: &RagfairContext<'a>) -> Vec<&'a PresetView> {
    if ctx.dynamic.show_default_presets_only {
        ctx.default_presets.iter().collect()
    } else {
        ctx.item_presets.values().collect()
    }
}

/// `RagfairAssortGenerator.CreateRagfairAssortRootItem` (`:126-141`).
///
/// The `new MongoId()` fallback (`:128-131`) is **dead on this path**: the one caller passes `tpl`
/// as the id (`:101`), which is never empty. Ported bug-for-bug anyway.
fn create_ragfair_assort_root_item(tpl_id: &str, id: Option<&str>) -> Item {
    let id = match id {
        // `MongoId.IsEmpty` is the default/zeroed id; the empty string is its analog here.
        Some(id) if !id.is_empty() => id.to_owned(),
        _ => mongo_id::generate(),
    };

    Item {
        id,
        template: tpl_id.to_owned(),
        parent_id: Some("hideout".to_owned()),
        slot_id: Some("hideout".to_owned()),
        upd: Some(unlimited_upd()),
        ..Default::default()
    }
}

/// The `Upd` both assort roots carry. `Upd.UnlimitedCount` is not a named member of
/// `loot::models::Upd`, so it rides in the passthrough map under its C# wire name — the property
/// name verbatim, as `Item.cs:135` carries no `JsonPropertyName` (unlike `sptPresetId`, `:123`).
fn unlimited_upd() -> Upd {
    let mut upd = Upd {
        stack_objects_count: Some(99_999_999.0),
        ..Default::default()
    };
    upd.extra.insert("UnlimitedCount".to_owned(), json!(true));

    upd
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use indexmap::IndexMap;
    use serde_json::json;

    use super::*;
    use crate::diag::DiagSink;
    use crate::loot::item_helper::{BUILT_IN_INSERTS, ItemBaseClassCache, MOB_CONTAINER};
    use crate::loot::models::{Item, ItemView, PresetView};
    use crate::loot::random_util::{TestSeedGuard, get_double};
    use crate::ragfair::NO_NAMES;
    use crate::ragfair::RagfairContext;
    use crate::ragfair::models::DynamicConfigWire;

    const SEED: u64 = 20260813;

    const NODE_TPL: &str = "a_node_not_an_item";
    const CONFIG_BLACKLIST_TPL: &str = "config_blacklisted_item";
    const SEASONAL_TPL: &str = "seasonal_item";
    const PRESET_ROOT_TPL: &str = "weapon_that_is_a_preset_root";
    const PLAIN_A_TPL: &str = "plain_item_a";
    const PLAIN_B_TPL: &str = "plain_item_b";
    /// Parented to a base type the *item helper* default list rejects but the ragfair list does not.
    const MOB_CONTAINER_CHILD_TPL: &str = "child_of_mob_container";
    /// Parented to a base type only the ragfair list rejects.
    const BUILT_IN_INSERT_CHILD_TPL: &str = "child_of_built_in_inserts";

    const PRESET_ONLY_TPL: &str = "weapon_only_reachable_through_a_preset";
    const MOD_A_TPL: &str = "mod_a";
    const MOD_B_TPL: &str = "mod_b";

    struct Fixture {
        items: IndexMap<String, ItemView>,
        base_classes: ItemBaseClassCache,
        dynamic: DynamicConfigWire,
        prices: IndexMap<String, f64>,
        config_blacklist: HashSet<String>,
        seasonal_blacklist: HashSet<String>,
        item_presets: IndexMap<String, PresetView>,
        default_presets: Vec<PresetView>,
        preset_lists: IndexMap<String, Vec<PresetView>>,
        seasonal_event_active: bool,
    }

    impl Fixture {
        fn new() -> Self {
            let item_presets: IndexMap<String, PresetView> = serde_json::from_value(json!({
                "preset1": {"id": "preset1", "items": [
                    {"_id": "p1root", "_tpl": PRESET_ROOT_TPL},
                    {"_id": "p1modA", "_tpl": MOD_A_TPL, "parentId": "p1root",
                        "slotId": "mod_muzzle"},
                    {"_id": "p1modB", "_tpl": MOD_B_TPL, "parentId": "p1modA",
                        "slotId": "mod_sight"},
                ]},
                "preset2": {"id": "preset2", "items": [
                    {"_id": "p2root", "_tpl": PRESET_ONLY_TPL},
                    {"_id": "p2modA", "_tpl": MOD_A_TPL, "parentId": "p2root",
                        "slotId": "mod_stock"},
                ]},
            }))
            .expect("presets parse");
            // Only the second preset is a default, so `showDefaultPresetsOnly` is observable.
            let default_presets = vec![
                serde_json::from_value(json!({"id": "preset2", "items": [
                    {"_id": "p2root", "_tpl": PRESET_ONLY_TPL},
                    {"_id": "p2modA", "_tpl": MOD_A_TPL, "parentId": "p2root",
                        "slotId": "mod_stock"},
                ]}))
                .expect("default preset parses"),
            ];

            let items: IndexMap<String, ItemView> = serde_json::from_value(json!({
                NODE_TPL: {"name": "a node", "type": "Node"},
                CONFIG_BLACKLIST_TPL: {"name": "config blacklisted", "type": "Item"},
                SEASONAL_TPL: {"name": "seasonal", "type": "Item"},
                PRESET_ROOT_TPL: {"name": "preset root", "type": "Item"},
                PLAIN_A_TPL: {"name": "plain a", "type": "Item"},
                PLAIN_B_TPL: {"name": "plain b", "type": "Item"},
                MOB_CONTAINER_CHILD_TPL: {"name": "secure container", "type": "Item",
                    "parent": MOB_CONTAINER},
                BUILT_IN_INSERT_CHILD_TPL: {"name": "built in insert", "type": "Item",
                    "parent": BUILT_IN_INSERTS},
                // The base classes themselves, so the base-class walk has somewhere to land.
                MOB_CONTAINER: {"name": "mob container base", "type": "Node"},
                BUILT_IN_INSERTS: {"name": "built in inserts base", "type": "Node"},
            }))
            .expect("items view parses");
            let base_classes = ItemBaseClassCache::build(&items);

            Self {
                items,
                base_classes,
                dynamic: dynamic_config(),
                prices: [
                    CONFIG_BLACKLIST_TPL,
                    SEASONAL_TPL,
                    PRESET_ROOT_TPL,
                    PLAIN_A_TPL,
                    PLAIN_B_TPL,
                    MOB_CONTAINER_CHILD_TPL,
                    BUILT_IN_INSERT_CHILD_TPL,
                ]
                .iter()
                .map(|tpl| ((*tpl).to_owned(), 1_000.0))
                .collect(),
                config_blacklist: HashSet::from([CONFIG_BLACKLIST_TPL.to_owned()]),
                seasonal_blacklist: HashSet::from([SEASONAL_TPL.to_owned()]),
                item_presets,
                default_presets,
                preset_lists: IndexMap::new(),
                seasonal_event_active: false,
            }
        }

        fn ctx(&self) -> RagfairContext<'_> {
            RagfairContext {
                items: &self.items,
                base_classes: &self.base_classes,
                dynamic: &self.dynamic,
                item_presets: &self.item_presets,
                default_presets: &self.default_presets,
                default_presets_by_tpl: &self.item_presets,
                presets_by_tpl: &self.preset_lists,
                flea_prices: &self.prices,
                handbook_prices: &self.prices,
                highest_trader_prices: &self.prices,
                config_blacklist: &self.config_blacklist,
                seasonal_item_tpl_blacklist: &self.seasonal_blacklist,
                pmc_names_usec: &NO_NAMES,
                pmc_names_bear: &NO_NAMES,
                timestamp: 1_700_000_000,
                seasonal_event_active: self.seasonal_event_active,
                diagnostics: DiagSink::capture(),
            }
        }

        fn generate(&self) -> Vec<Vec<Item>> {
            generate_ragfair_assort_items(&self.ctx()).expect("the walk succeeds")
        }
    }

    fn dynamic_config() -> DynamicConfigWire {
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
            "condition": {},
            "stackablePercent": {"min": 10.0, "max": 100.0},
            "nonStackableCount": {"min": 1, "max": 1},
            "rating": {"min": 0.0, "max": 1.0},
            "armor": {"removeRemovablePlateChance": 0, "plateSlotIdToRemovePool": []},
            "itemPriceMultiplier": {},
            "offerCurrencyChancePercent": {"5449016a4bdc2d6f028b456f": 100.0},
            "showAsSingleStack": [],
            "removeSeasonalItemsWhenNotInEvent": true,
            "blacklist": {"damagedAmmoPacks": false, "custom": [], "enableBsgList": false,
                "enableQuestList": false, "traderItems": false,
                "armorPlate": {"maxProtectionLevel": 0, "ignoreSlots": []},
                "enableCustomItemCategoryList": false, "customItemCategoryList": []},
            "unreasonableModPrices": {},
            "generateBaseFleaPrices": {"useHandbookPrice": false, "priceMultiplier": 1.0,
                "preventPriceBeingBelowTraderBuyPrice": false, "itemTplMultiplierOverride": {},
                "itemTypeMultiplierOverride": {}, "useHideoutCraftMultiplier": false,
                "hideoutCraftMultiplier": 1.0, "generatePresetPriceByChildren": false},
        }))
        .expect("dynamic config parses")
    }

    /// The root tpl of every returned list, in order — the batch order the offer generator draws in.
    fn root_tpls(results: &[Vec<Item>]) -> Vec<&str> {
        results
            .iter()
            .map(|items| items[0].template.as_str())
            .collect()
    }

    fn upd_of(item: &Item) -> &crate::loot::models::Upd {
        item.upd.as_ref().expect("the assort root carries an Upd")
    }

    // -----------------------------------------------------------------------
    // Batch order
    // -----------------------------------------------------------------------

    #[test]
    fn the_batch_is_presets_first_then_items_in_view_order() {
        let fixture = Fixture::new();

        let results = fixture.generate();

        assert_eq!(
            root_tpls(&results),
            vec![
                // Presets, in `itemPresets` insertion order
                PRESET_ROOT_TPL,
                PRESET_ONLY_TPL,
                // Then the items view, in its own insertion order. `NODE_TPL` is a Node,
                // `CONFIG_BLACKLIST_TPL` is blacklisted, `SEASONAL_TPL` is out of season,
                // `PRESET_ROOT_TPL` was processed as a preset and
                // `BUILT_IN_INSERT_CHILD_TPL` is an invalid base type.
                PLAIN_A_TPL,
                PLAIN_B_TPL,
                MOB_CONTAINER_CHILD_TPL,
            ]
        );
    }

    #[test]
    fn show_default_presets_only_narrows_the_preset_source() {
        let mut fixture = Fixture::new();
        fixture.dynamic.show_default_presets_only = true;

        let results = fixture.generate();

        // Only preset2 is a default, so preset1's root tpl is no longer "already processed" and
        // comes back through the item loop instead — with its own tpl as its id.
        assert_eq!(
            root_tpls(&results),
            vec![
                PRESET_ONLY_TPL,
                PRESET_ROOT_TPL,
                PLAIN_A_TPL,
                PLAIN_B_TPL,
                MOB_CONTAINER_CHILD_TPL,
            ]
        );
        assert_eq!(results[1].len(), 1);
        assert_eq!(results[1][0].id, PRESET_ROOT_TPL);
    }

    // -----------------------------------------------------------------------
    // Presets
    // -----------------------------------------------------------------------

    #[test]
    fn a_preset_root_is_hideout_parented_and_unlimited() {
        let fixture = Fixture::new();

        let results = fixture.generate();
        let root = &results[0][0];

        assert_eq!(root.parent_id.as_deref(), Some("hideout"));
        assert_eq!(root.slot_id.as_deref(), Some("hideout"));
        let upd = upd_of(root);
        assert_eq!(upd.stack_objects_count, Some(99_999_999.0));
        // Neither member is named by `loot::models::Upd`; the wire names are the contract.
        assert_eq!(upd.extra["UnlimitedCount"], json!(true));
        assert_eq!(upd.extra["sptPresetId"], json!("preset1"));
        assert_eq!(
            upd_of(&results[1][0]).extra["sptPresetId"],
            json!("preset2")
        );
    }

    #[test]
    fn only_the_preset_root_is_touched_the_children_keep_their_slots() {
        let fixture = Fixture::new();

        let results = fixture.generate();

        assert_eq!(results[0].len(), 3);
        assert_eq!(results[0][1].slot_id.as_deref(), Some("mod_muzzle"));
        assert_eq!(results[0][2].slot_id.as_deref(), Some("mod_sight"));
        assert!(results[0][1].upd.is_none());
        assert!(results[0][2].upd.is_none());
    }

    #[test]
    fn a_cloned_preset_gets_fresh_ids_and_stays_internally_parented() {
        let fixture = Fixture::new();

        let results = fixture.generate();
        let clone = &results[0];
        let source_ids = ["p1root", "p1modA", "p1modB"];

        for item in clone {
            assert!(
                !source_ids.contains(&item.id.as_str()),
                "{} kept its source id",
                item.id
            );
            assert!(
                crate::loot::mongo_id::is_valid(&item.id),
                "{} is not a MongoId",
                item.id
            );
        }
        // The source list is untouched: the walk clones before it re-ids.
        assert_eq!(fixture.item_presets["preset1"].items[0].id, "p1root");

        // Every child's parent resolves inside the cloned list.
        let ids: HashSet<&str> = clone.iter().map(|item| item.id.as_str()).collect();
        for child in &clone[1..] {
            let parent = child.parent_id.as_deref().expect("a child has a parent");
            assert!(ids.contains(parent), "{parent} is not in the cloned list");
        }
        // The chain is preserved: modB still hangs off modA, not off the root.
        assert_eq!(clone[2].parent_id.as_deref(), Some(clone[1].id.as_str()));
    }

    #[test]
    fn a_presets_root_tpl_is_skipped_by_the_item_loop() {
        let fixture = Fixture::new();

        let results = fixture.generate();

        // Exactly one list has the preset's root tpl at its root — the preset's own.
        assert_eq!(
            root_tpls(&results)
                .iter()
                .filter(|tpl| **tpl == PRESET_ROOT_TPL)
                .count(),
            1
        );
        assert_eq!(results[0].len(), 3);
    }

    #[test]
    fn an_empty_preset_is_the_csharp_throw_path() {
        let mut fixture = Fixture::new();
        fixture.item_presets["preset1"].items.clear();

        let error =
            generate_ragfair_assort_items(&fixture.ctx()).expect_err("an empty preset errors");

        assert_eq!(
            error.message,
            "Preset preset1 has no items, unable to generate a ragfair assort"
        );
    }

    // -----------------------------------------------------------------------
    // Items
    // -----------------------------------------------------------------------

    #[test]
    fn an_item_assort_row_has_its_tpl_as_its_id() {
        let fixture = Fixture::new();

        let results = fixture.generate();
        let plain = &results[2];

        // "tpl and id must be the same so hideout recipe rewards work" (`:101`)
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].id, PLAIN_A_TPL);
        assert_eq!(plain[0].template, PLAIN_A_TPL);
        assert_eq!(plain[0].parent_id.as_deref(), Some("hideout"));
        assert_eq!(plain[0].slot_id.as_deref(), Some("hideout"));
        let upd = upd_of(&plain[0]);
        assert_eq!(upd.stack_objects_count, Some(99_999_999.0));
        assert_eq!(upd.extra["UnlimitedCount"], json!(true));
        // Only a preset root carries a preset id.
        assert!(!upd.extra.contains_key("sptPresetId"));
    }

    #[test]
    fn the_invalid_base_types_are_the_ragfair_list_not_the_item_helper_default() {
        let fixture = Fixture::new();

        let generated = fixture.generate();
        let results = root_tpls(&generated);

        // `BUILT_IN_INSERTS` is on the ragfair list only...
        assert!(!results.contains(&BUILT_IN_INSERT_CHILD_TPL));
        // ...and `MOB_CONTAINER` is on `_defaultInvalidBaseTypes` only.
        assert!(results.contains(&MOB_CONTAINER_CHILD_TPL));
    }

    // -----------------------------------------------------------------------
    // The seasonal skip: all three conditions have to hold
    // -----------------------------------------------------------------------

    #[test]
    fn a_seasonal_item_is_skipped_only_when_all_three_conditions_hold() {
        let all_three = Fixture::new();
        assert!(!root_tpls(&all_three.generate()).contains(&SEASONAL_TPL));

        let mut flag_off = Fixture::new();
        flag_off.dynamic.remove_seasonal_items_when_not_in_event = false;
        assert!(root_tpls(&flag_off.generate()).contains(&SEASONAL_TPL));

        let mut event_running = Fixture::new();
        event_running.seasonal_event_active = true;
        assert!(root_tpls(&event_running.generate()).contains(&SEASONAL_TPL));

        let mut not_blacklisted = Fixture::new();
        not_blacklisted.seasonal_blacklist.clear();
        assert!(root_tpls(&not_blacklisted.generate()).contains(&SEASONAL_TPL));
    }

    // -----------------------------------------------------------------------
    // No RNG
    // -----------------------------------------------------------------------

    #[test]
    fn the_whole_walk_leaves_the_stream_untouched() {
        let fixture = Fixture::new();

        let after = {
            let _guard = TestSeedGuard::install(SEED);
            fixture.generate();

            get_double(0.0, 1.0)
        };
        let untouched = {
            let _guard = TestSeedGuard::install(SEED);

            get_double(0.0, 1.0)
        };

        assert_eq!(after, untouched);
    }

    // -----------------------------------------------------------------------
    // create_ragfair_assort_root_item
    // -----------------------------------------------------------------------

    #[test]
    fn the_root_item_helper_mints_an_id_when_it_is_given_none() {
        // Dead on this path — `:101` always passes the tpl — but ported bug-for-bug.
        let minted = create_ragfair_assort_root_item(PLAIN_A_TPL, None);
        let from_empty = create_ragfair_assort_root_item(PLAIN_A_TPL, Some(""));

        assert!(crate::loot::mongo_id::is_valid(&minted.id));
        assert!(crate::loot::mongo_id::is_valid(&from_empty.id));
        assert_ne!(minted.id, from_empty.id);
        assert_eq!(minted.template, PLAIN_A_TPL);
    }
}
