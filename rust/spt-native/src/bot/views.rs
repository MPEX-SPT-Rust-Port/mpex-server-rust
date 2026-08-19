//! Bot database views derived natively at publish time (Phase 1 bot flip).
//!
//! Bug-for-bug ports of the C# that builds the database members of the bot payloads
//! (`Native/Bot/BotPayloadProjection.cs`) — the C# bodies are the authority and every quirk is
//! preserved at its port site. The items and preset views the bot family shares with ragfair are
//! the maps [`crate::ragfair::views::derive`] already builds, so they ride in via the shared
//! [`RagfairDbViews`] `Arc` instead of a second derivation — the same embedding as
//! [`crate::quest::views::QuestDbViews`].

use std::sync::Arc;

use indexmap::IndexMap;

use crate::bot::mod_pool_service::{gear_slot_pool, weapon_slot_pool};
use crate::db::models::{GlobalsRoot, TemplatesRoot};
use crate::ragfair::views::RagfairDbViews;

/// The bot-family database views derived at publish — only what the resident roots determine
/// lives here; the config and per-request members keep crossing per call.
#[derive(Debug)]
pub struct BotDbViews {
    /// The views the bot family shares with ragfair (`items`, `item_presets`,
    /// `default_presets_by_tpl`, `base_classes`, …) — the whole [`RagfairDbViews`] rides in by
    /// `Arc`, the same embedding as [`crate::quest::views::QuestDbViews::ragfair`].
    pub ragfair: Arc<RagfairDbViews>,
    /// [`RagfairDbViews::default_presets_by_tpl`] re-keyed to each view's preset id
    /// (C# `ToDefaultPresetIds`, `BotPayloadProjection.cs:392-395`).
    pub default_preset_ids_by_tpl: IndexMap<String, String>,
    /// [`build_mod_pool_slot_order`].
    pub mod_pool_slot_order: IndexMap<String, Vec<usize>>,
    /// `globals.config.exp.level.exp_table[].exp` (`BotWaveBatcher.cs:179-186`).
    pub exp_table: Vec<i32>,
}

/// Derived at publish once templates + globals + ragfair views are resident.
/// - default_preset_ids_by_tpl: re-key of ragfair.default_presets_by_tpl to each
///   view's preset id (C# ToDefaultPresetIds, BotPayloadProjection.cs:392-395).
/// - mod_pool_slot_order: 1:1 port of BuildModPoolSlotOrder
///   (BotPayloadProjection.cs:322-369) — a pure walk over templates.items reading
///   only each item's Properties.Slots (content contract: mod_pool_service.rs:1-31).
///   Port the C# loop body exactly, enumeration order included; the Task 5 identity
///   test over the real database is the equivalence gate.
/// - exp_table: globals.config.exp.level.exp_table[].exp (BotWaveBatcher.cs:179-186).
///
/// Total over empty roots; kept `Result`-shaped so a future hard failure aborts the publish the
/// way ragfair's does.
pub fn derive(
    templates: &TemplatesRoot,
    globals: &GlobalsRoot,
    ragfair: &Arc<RagfairDbViews>,
) -> Result<BotDbViews, String> {
    // ToDefaultPresetIds (BotPayloadProjection.cs:392-395) over GetDefaultPresetByTpl — whose
    // port ragfair.default_presets_by_tpl is. `Preset.Id` is a non-nullable MongoId in C#, so
    // the view's `id` is always present (ragfair::views::to_preset_view).
    let default_preset_ids_by_tpl = ragfair
        .default_presets_by_tpl
        .iter()
        .map(|(tpl, preset)| (tpl.clone(), preset.id.clone().unwrap_or_default()))
        .collect();

    let mod_pool_slot_order = build_mod_pool_slot_order(templates, ragfair);

    // BotWaveBatcher.cs:179-186: `expTable.Select(entry => entry.Experience)`.
    let exp_table = globals
        .config
        .exp
        .level
        .exp_table
        .iter()
        .map(|entry| entry.exp)
        .collect();

    Ok(BotDbViews {
        ragfair: Arc::clone(ragfair),
        default_preset_ids_by_tpl,
        mod_pool_slot_order,
        exp_table,
    })
}

/// `BotPayloadProjection.BuildModPoolSlotOrder` (`BotPayloadProjection.cs:322-369`) — the mod
/// pools' slot-name enumeration order per template, as indices into that template's slots. The
/// C# ran it against `BotEquipmentModPoolService`'s cached pools; here the pools are
/// [`crate::bot::mod_pool_service`]'s on-demand derivations over the ragfair items view (the
/// `BuildItemsView` projection the C# service's inputs round-trip through), in database order
/// with no projected order applied — the Task 5 identity test over the real database is the
/// gate that the C# pools enumerate the same way.
fn build_mod_pool_slot_order(
    templates: &TemplatesRoot,
    ragfair: &RagfairDbViews,
) -> IndexMap<String, Vec<usize>> {
    let mut order = IndexMap::new();

    // foreach (var (tpl, template) in templates) — itemHelper.TemplateTable.Items
    // (BotPayloadProjection.cs:116), insertion order.
    for (tpl, template) in &templates.items {
        let mut pool = gear_slot_pool(&ragfair.items, tpl, None);
        if pool.is_empty() {
            pool = weapon_slot_pool(&ragfair.items, tpl, None);
        }

        // Order cannot matter below two slot names, and a pool that size subsumes the
        // "template has two or more slots" check
        if pool.len() < 2 {
            continue;
        }

        // C# materialises `Properties.Slots` (an IEnumerable) to index it; `slots` here indexes
        // the same array the projected `slots` view is a 1:1 Select of.
        let Some(slots) = template
            .properties
            .as_ref()
            .and_then(|properties| properties.slots.as_deref())
        else {
            continue;
        };

        let mut indices = Vec::with_capacity(pool.len());
        for slot_name in pool.keys() {
            // First occurrence, matching the GetOrAdd merge of same-named slots
            if let Some(index) = slots
                .iter()
                .position(|slot| slot.name.as_deref() == Some(slot_name.as_str()))
            {
                indices.push(index);
            }
        }

        order.insert(tpl.clone(), indices);
    }

    order
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::models::TradersRoot;
    use crate::loot::item_helper::{MOD, WEAPON};

    const WEAPON_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const SCOPE_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const MAGAZINE_TPL: &str = "dddddddddddddddddddddddd";
    const PRESET_ID: &str = "111111111111111111111111";

    /// A weapon whose slots exercise every branch of `BuildModPoolSlotOrder`'s inner loop, plus
    /// slotless mods (their pools are empty, so `pool.Count < 2` skips them).
    fn fixture_templates() -> TemplatesRoot {
        serde_json::from_value(json!({
            "items": {
                WEAPON: {"_type": "Node", "_props": {}},
                MOD: {"_type": "Node", "_props": {}},
                // Slot 0 has an empty filter (never pooled), 1 and 2 are pooled, 3 repeats
                // slot 1's name (GetOrAdd merges it into the first occurrence).
                WEAPON_TPL: {"_parent": WEAPON, "_type": "Item", "_props": {"Slots": [
                    {"_name": "mod_stock", "_props": {"filters": [{"Filter": []}]}},
                    {"_name": "mod_magazine", "_props": {"filters": [{"Filter": [MAGAZINE_TPL]}]}},
                    {"_name": "mod_scope", "_props": {"filters": [{"Filter": [SCOPE_TPL]}]}},
                    {"_name": "mod_magazine", "_props": {"filters": [{"Filter": [MAGAZINE_TPL]}]}}
                ]}},
                MAGAZINE_TPL: {"_parent": MOD, "_type": "Item", "_props": {}},
                SCOPE_TPL: {"_parent": MOD, "_type": "Item", "_props": {}}
            }
        }))
        .expect("fixture parses")
    }

    fn fixture_globals() -> GlobalsRoot {
        serde_json::from_value(json!({
            "ItemPresets": {
                PRESET_ID: {
                    "_id": PRESET_ID,
                    "_name": "weapon_default",
                    "_encyclopedia": WEAPON_TPL,
                    "_items": [{"_id": "000000000000000000000000", "_tpl": WEAPON_TPL}]
                }
            },
            "config": {"exp": {"level": {"exp_table": [{"exp": 10}, {"exp": 20}, {"exp": 30}]}}}
        }))
        .expect("fixture parses")
    }

    #[test]
    fn derive_builds_the_slot_order_the_preset_ids_and_the_exp_table() {
        let templates = fixture_templates();
        let globals = fixture_globals();
        let ragfair = Arc::new(
            crate::ragfair::views::derive(&templates, &TradersRoot::default(), &globals)
                .expect("ragfair views derive"),
        );

        let views = derive(&templates, &globals, &ragfair).expect("bot views derive");

        // BuildModPoolSlotOrder over the fixture, traced through the C# body: the nodes fail the
        // pool's `_type == "Item"` half, the slotless mods pool empty (`pool.Count < 2` skips
        // both), and the weapon's pool is [mod_magazine, mod_scope] — mod_stock's filter is
        // empty and the duplicate mod_magazine merged — whose first-occurrence slot indices are
        // 1 and 2 (0 is mod_stock; 3 is the merged duplicate).
        let expected: IndexMap<String, Vec<usize>> =
            IndexMap::from([(WEAPON_TPL.to_owned(), vec![1, 2])]);
        assert_eq!(views.mod_pool_slot_order, expected);

        // ToDefaultPresetIds: the tpl keeps its key, the value becomes the preset's own id.
        let expected_ids: IndexMap<String, String> =
            IndexMap::from([(WEAPON_TPL.to_owned(), PRESET_ID.to_owned())]);
        assert_eq!(views.default_preset_ids_by_tpl, expected_ids);

        assert_eq!(views.exp_table, vec![10, 20, 30]);
        assert!(Arc::ptr_eq(&views.ragfair, &ragfair));
    }

    #[test]
    fn derive_is_total_over_empty_roots() {
        let templates = TemplatesRoot::default();
        let globals = GlobalsRoot::default();
        let ragfair = Arc::new(
            crate::ragfair::views::derive(&templates, &TradersRoot::default(), &globals)
                .expect("ragfair views derive"),
        );

        let views = derive(&templates, &globals, &ragfair).expect("bot views derive");

        assert!(views.mod_pool_slot_order.is_empty());
        assert!(views.default_preset_ids_by_tpl.is_empty());
        assert!(views.exp_table.is_empty());
    }
}
