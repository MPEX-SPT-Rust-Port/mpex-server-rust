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

use crate::db::models::{GlobalsRoot, TemplatesRoot};
use crate::ragfair::views::RagfairDbViews;

/// The bot-family database views derived at publish — only what the resident roots determine
/// lives here; the config and per-request members keep crossing per call. The mod-pool slot order
/// is not among them: it is the template's own `Properties.Slots` order, derived where the pool is
/// ([`crate::bot::mod_pool_service`]), so there is nothing to project.
#[derive(Debug)]
pub struct BotDbViews {
    /// The views the bot family shares with ragfair (`items`, `item_presets`,
    /// `default_presets_by_tpl`, `base_classes`, …) — the whole [`RagfairDbViews`] rides in by
    /// `Arc`, the same embedding as [`crate::quest::views::QuestDbViews::ragfair`].
    pub ragfair: Arc<RagfairDbViews>,
    /// [`RagfairDbViews::default_presets_by_tpl`] re-keyed to each view's preset id
    /// (C# `ToDefaultPresetIds`, `BotPayloadProjection.cs:392-395`).
    pub default_preset_ids_by_tpl: IndexMap<String, String>,
    /// `globals.config.exp.level.exp_table[].exp` (`BotWaveBatcher.cs:179-186`).
    pub exp_table: Vec<i32>,
    /// bodyTpl → fixed hands tpl for every customization item whose `SavageBody` entry is
    /// `IsNotRandom` — `SetBotAppearance`'s hands rule collapsed to one map;
    /// `BotPayloadProjection.BuildBodyToFixedHands` is the override twin, keep them identical.
    pub body_to_fixed_hands: IndexMap<String, String>,
}

/// Derived at publish once globals + ragfair views are resident.
/// - default_preset_ids_by_tpl: re-key of ragfair.default_presets_by_tpl to each
///   view's preset id (C# ToDefaultPresetIds, BotPayloadProjection.cs:392-395).
/// - exp_table: globals.config.exp.level.exp_table[].exp (BotWaveBatcher.cs:179-186).
/// - body_to_fixed_hands: SetBotAppearance's IsNotRandom hands rule (BotGenerator.cs:616-624).
///
/// Total over empty roots; kept `Result`-shaped so a future hard failure aborts the publish the
/// way ragfair's does.
pub fn derive(
    globals: &GlobalsRoot,
    templates: &TemplatesRoot,
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

    // BotWaveBatcher.cs:179-186: `expTable.Select(entry => entry.Experience)`.
    let exp_table = globals
        .config
        .exp
        .level
        .exp_table
        .iter()
        .map(|entry| entry.exp)
        .collect();

    let body_to_fixed_hands = templates
        .customization
        .iter()
        .filter_map(|(tpl, item)| {
            // SetBotAppearance: FirstOrDefault(c => c.Key == chosenBodyTemplate?.Name.Trim()) —
            // exact string equality, so a map lookup on the trimmed name is equivalent.
            let name = item.name.as_deref()?.trim();
            let entry = globals.config.customization.body.get(name)?;
            entry
                .is_not_random
                .then(|| (tpl.clone(), entry.hands.clone()))
        })
        .collect();

    Ok(BotDbViews {
        ragfair: Arc::clone(ragfair),
        default_preset_ids_by_tpl,
        exp_table,
        body_to_fixed_hands,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::models::{TemplatesRoot, TradersRoot};
    use crate::loot::item_helper::{MOD, WEAPON};

    const WEAPON_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const SCOPE_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const MAGAZINE_TPL: &str = "dddddddddddddddddddddddd";
    const PRESET_ID: &str = "111111111111111111111111";

    /// The items table this module's `derive` needs, which is only whatever
    /// [`crate::ragfair::views::derive`] wants: a weapon a default preset can point at, plus the
    /// mods and base-class nodes that keep it resolvable. The slots are incidental shape.
    fn fixture_templates() -> TemplatesRoot {
        serde_json::from_value(json!({
            "items": {
                WEAPON: {"_type": "Node", "_props": {}},
                MOD: {"_type": "Node", "_props": {}},
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
    fn derive_builds_the_preset_ids_and_the_exp_table() {
        let templates = fixture_templates();
        let globals = fixture_globals();
        let ragfair = Arc::new(
            crate::ragfair::views::derive(&templates, &TradersRoot::default(), &globals)
                .expect("ragfair views derive"),
        );

        let views = derive(&globals, &templates, &ragfair).expect("bot views derive");

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

        let views = derive(&globals, &templates, &ragfair).expect("bot views derive");

        assert!(views.default_preset_ids_by_tpl.is_empty());
        assert!(views.exp_table.is_empty());
        assert!(views.body_to_fixed_hands.is_empty());
    }

    const FIXED_BODY_TPL: &str = "222222222222222222222222";
    const RANDOM_BODY_TPL: &str = "333333333333333333333333";
    const UNKNOWN_BODY_TPL: &str = "444444444444444444444444";
    const NAMELESS_BODY_TPL: &str = "555555555555555555555555";
    const PADDED_BODY_TPL: &str = "666666666666666666666666";
    const FIXED_HANDS_TPL: &str = "777777777777777777777777";
    const PADDED_HANDS_TPL: &str = "888888888888888888888888";

    /// The five `SetBotAppearance` hands cases: an `IsNotRandom` match, a random match, a name with
    /// no `SavageBody` entry, an entry with no `_name` at all, and a name only the `.trim()` makes
    /// match.
    #[test]
    fn derive_maps_only_the_is_not_random_bodies_to_their_fixed_hands() {
        let templates: TemplatesRoot = serde_json::from_value(json!({
            "customization": {
                FIXED_BODY_TPL: {"_name": "FixedBody"},
                RANDOM_BODY_TPL: {"_name": "RandomBody"},
                UNKNOWN_BODY_TPL: {"_name": "NoSavageBodyEntry"},
                NAMELESS_BODY_TPL: {"_id": NAMELESS_BODY_TPL},
                PADDED_BODY_TPL: {"_name": "  PaddedBody  "}
            }
        }))
        .expect("fixture parses");
        let globals: GlobalsRoot = serde_json::from_value(json!({
            "config": {"Customization": {"SavageBody": {
                "FixedBody": {"hands": FIXED_HANDS_TPL, "isNotRandom": true},
                "RandomBody": {"hands": "999999999999999999999999", "isNotRandom": false},
                "PaddedBody": {"hands": PADDED_HANDS_TPL, "isNotRandom": true}
            }}}
        }))
        .expect("fixture parses");
        let ragfair = Arc::new(
            crate::ragfair::views::derive(&templates, &TradersRoot::default(), &globals)
                .expect("ragfair views derive"),
        );

        let views = derive(&globals, &templates, &ragfair).expect("bot views derive");

        // Keyed by the customization *tpl*, valued by that body's fixed hands tpl. The random
        // body, the unmatched name and the nameless entry are all absent — each one takes
        // SetBotAppearance's weighted-draw branch instead.
        let expected: IndexMap<String, String> = IndexMap::from([
            (FIXED_BODY_TPL.to_owned(), FIXED_HANDS_TPL.to_owned()),
            (PADDED_BODY_TPL.to_owned(), PADDED_HANDS_TPL.to_owned()),
        ]);
        assert_eq!(views.body_to_fixed_hands, expected);
    }
}
