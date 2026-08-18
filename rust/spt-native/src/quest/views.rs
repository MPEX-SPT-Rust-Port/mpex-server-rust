//! Quest database views derived natively at publish time (Phase 1 quest flip).
//!
//! Bug-for-bug ports of the C# that built the database half of the pre-flip
//! `QuestInvariantSlice` (now `QuestViewsOverride`,
//! `Native/RepeatableQuests/RepeatableQuestNativeRequestBuilder.cs`) — the C# bodies are the
//! authority and every quirk is preserved at its port site. The three views the override shares
//! with ragfair (`Items`, `HandbookPrices`, `FleaPrices`) are the same maps
//! [`crate::ragfair::views::derive`] already builds — `BuildViewsOverride` calls the identical
//! helpers (`PayloadProjection.BuildItemsView`, `HandbookHelper.GetTemplatePrice` over every
//! items key, `templateTable.Prices` raw) — so they ride in via the shared
//! [`RagfairDbViews`] `Arc` instead of a second derivation.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::db::models::{ExitSourceView, GlobalsRoot, LocationsRoot, TemplatesRoot};
use crate::loot::item_helper::WEAPON;
use crate::loot::models::PresetView;
use crate::quest::models::{ExitView, LevelledItemFilter, RepeatableTemplates};
use crate::ragfair::views::{RagfairDbViews, build_handbook_price_map, to_preset_view};

/// `Enum.GetNames<ELocationName>()` lowercased — copied from
/// `Models/Enums/ELocationName.cs`, declaration order. The whole key domain of
/// `BuildExtractsByLocation` (`RepeatableQuestNativeRequestBuilder.cs:221-224`).
const E_LOCATION_NAMES_LOWER: [&str; 13] = [
    "factory4_day",
    "factory4_night",
    "bigmap",
    "woods",
    "shoreline",
    "sandbox",
    "interchange",
    "lighthouse",
    "laboratory",
    "rezervbase",
    "tarkovstreets",
    "labyrinth",
    "any",
];

/// `RequirementState` member names — copied from `Models/Enums/RequirementState.cs`,
/// declaration order. Index 0 (`None`) is the C# `default(RequirementState)` an absent
/// `PassageRequirement` deserializes to.
const REQUIREMENT_STATES: [&str; 14] = [
    "None",
    "Empty",
    "TransferItem",
    "WorldEvent",
    "NotEmpty",
    "HasItem",
    "WearsItem",
    "EmptyOrSize",
    "SkillLevel",
    "Reference",
    "ScavCooperation",
    "Train",
    "Timer",
    "SecretTransferItem",
];

/// The quest-family database views derived at publish
/// (`RepeatableQuestNativeRequestBuilder.BuildViewsOverride`'s database half). The service and
/// config members of the slice keep crossing per request; only what the resident roots determine
/// lives here.
#[derive(Debug)]
pub struct QuestDbViews {
    /// The three views the quest slice shared with ragfair — `Items` / `HandbookPrices` /
    /// `FleaPrices` (`RepeatableQuestNativeRequestBuilder.cs:156-158`) are built by the identical
    /// C# helpers ragfair's derive ports, so the whole [`RagfairDbViews`] rides in by `Arc`
    /// (`base_classes` included).
    pub ragfair: Arc<RagfairDbViews>,
    /// `presetHelper.GetDefaultWeaponPresets().Values` through `PayloadProjection.ToPresetView`
    /// (`RepeatableQuestNativeRequestBuilder.cs:159`).
    pub default_weapon_presets: Vec<PresetView>,
    /// `presetHelper.GetDefaultPresetOrItemPrice` for every items-table key, in items-table order
    /// (`RepeatableQuestNativeRequestBuilder.cs:146-149`).
    pub default_preset_or_item_prices: IndexMap<String, f64>,
    /// `templateTable.RepeatableQuests.Templates!` (`RepeatableQuestNativeRequestBuilder.cs:165`).
    /// C# can never lack the block (`TemplateTable.RepeatableQuests` is `required`); an absent one
    /// on a partial root collapses to all-`None` templates — the same `null` arm every consumer
    /// (`GetClonedQuestTemplateForType`) already takes per missing template.
    pub repeatable_quest_templates: RepeatableTemplates,
    /// `completionFilters?.ItemsWhitelist ?? []` (`RepeatableQuestNativeRequestBuilder.cs:166`).
    pub completion_items_whitelist: Vec<LevelledItemFilter>,
    /// `completionFilters?.ItemsBlacklist ?? []` (`RepeatableQuestNativeRequestBuilder.cs:167`).
    pub completion_items_blacklist: Vec<LevelledItemFilter>,
    /// [`build_boss_spawns_by_location`].
    pub boss_spawns_by_location: IndexMap<String, Vec<String>>,
    /// [`build_extracts_by_location`].
    pub extracts_by_location: IndexMap<String, Vec<ExitView>>,
}

/// Derive every quest view off the resident roots and the already-derived ragfair views. Total
/// over empty roots; kept `Result`-shaped so a future hard failure aborts the publish the way
/// ragfair's does.
pub fn derive(
    templates: &TemplatesRoot,
    globals: &GlobalsRoot,
    locations: &LocationsRoot,
    ragfair: &Arc<RagfairDbViews>,
) -> Result<QuestDbViews, String> {
    // PresetHelper.GetDefaultWeaponPresets (PresetHelper.cs:94-105): the globals map filtered on
    // encyclopedia presence + weapon base class, map order, keyed by the map key — a key/_id
    // mismatch never disqualifies an entry here (same reading as ragfair's weapon_defaults).
    let default_weapon_presets: Vec<PresetView> = globals
        .item_presets
        .values()
        .filter(|preset| {
            preset.encyclopedia.as_deref().is_some_and(|encyclopedia| {
                ragfair.base_classes.is_of_baseclass(encyclopedia, WEAPON)
            })
        })
        .map(to_preset_view)
        .collect();

    // One pass over the items table (RepeatableQuestNativeRequestBuilder.cs:146-149).
    // GetDefaultPresetOrItemPrice (PresetHelper.cs:272-282) resolves the tpl's default preset
    // exactly as GetDefaultPresetByTpl resolves it per tpl — ragfair.default_presets_by_tpl IS
    // that map (hydrated-cache semantics: every live process hydrates the weapon/equipment
    // default caches during startup assort generation before a quest can generate) — then prices
    // the preset's item tpls, or the bare tpl without one.
    let handbook_by_id = build_handbook_price_map(&templates.handbook);
    let mut default_preset_or_item_prices = IndexMap::with_capacity(templates.items.len());
    for tpl in templates.items.keys() {
        let price = match ragfair.default_presets_by_tpl.get(tpl) {
            Some(preset) => item_and_children_price(
                preset.items.iter().map(|item| item.template.as_str()),
                &handbook_by_id,
                &templates.prices,
            ),
            None => item_and_children_price(
                std::iter::once(tpl.as_str()),
                &handbook_by_id,
                &templates.prices,
            ),
        };
        default_preset_or_item_prices.insert(tpl.clone(), price);
    }

    let completion = templates
        .repeatable_quests
        .as_ref()
        .and_then(|repeatable| repeatable.data.as_ref())
        .and_then(|data| data.completion.as_ref());

    Ok(QuestDbViews {
        ragfair: Arc::clone(ragfair),
        default_weapon_presets,
        default_preset_or_item_prices,
        repeatable_quest_templates: templates
            .repeatable_quests
            .as_ref()
            .and_then(|repeatable| repeatable.templates.clone())
            .unwrap_or_default(),
        completion_items_whitelist: completion
            .map(|filters| filters.items_whitelist.clone())
            .unwrap_or_default(),
        completion_items_blacklist: completion
            .map(|filters| filters.items_blacklist.clone())
            .unwrap_or_default(),
        boss_spawns_by_location: build_boss_spawns_by_location(locations),
        extracts_by_location: build_extracts_by_location(locations),
    })
}

/// `ItemHelper.GetItemAndChildrenPrice` (`ItemHelper.cs:419-424`): an `int` accumulator, each
/// price truncated by the `(int)` cast. Per tpl the price is `GetItemPrice` (`ItemHelper.cs:427-440`):
/// the handbook price when >= 1 (`GetStaticItemPrice` — `HandbookHelper.GetTemplatePrice`,
/// answered from the hydrated cache), else the flea price, else 0. Rust's `as i32` saturates
/// where the C# unchecked cast is unspecified — unobservable for real prices.
fn item_and_children_price<'a>(
    tpls: impl Iterator<Item = &'a str>,
    handbook_by_id: &HashMap<&str, f64>,
    flea_prices: &IndexMap<String, f64>,
) -> f64 {
    let mut total: i32 = 0;
    for tpl in tpls {
        let handbook_price = handbook_by_id.get(tpl).copied().unwrap_or(0.0);
        let price = if handbook_price >= 1.0 {
            handbook_price
        } else {
            flea_prices.get(tpl).copied().unwrap_or(0.0)
        };
        total += price as i32;
    }
    f64::from(total)
}

/// `BuildBossSpawnsByLocation` (`RepeatableQuestNativeRequestBuilder.cs:191-209`): every root
/// entry in map order — the C# iterates `GetDictionary().Values`, whose reflection order is the
/// property declaration order the root serialized in — skipped without a `base.Id` (which also
/// drops the UI-linkage `base` entry, the non-`Location` property C#'s dictionary excludes by
/// type), keyed by the raw id, values the spawn names with nameless spawns dropped.
fn build_boss_spawns_by_location(locations: &LocationsRoot) -> IndexMap<String, Vec<String>> {
    let mut boss_spawns = IndexMap::new();

    for entry in locations.locations.values() {
        let Some(base) = entry.base.as_ref() else {
            continue;
        };
        let Some(id) = base.id.as_ref() else {
            continue;
        };
        boss_spawns.insert(
            id.clone(),
            base.boss_location_spawn
                .iter()
                .filter_map(|spawn| spawn.boss_name.clone())
                .collect(),
        );
    }

    boss_spawns
}

/// `BuildExtractsByLocation` (`RepeatableQuestNativeRequestBuilder.cs:217-235`): the lowercased
/// `ELocationName` domain resolved against the root. `LocationTable.GetLocation` maps each
/// lowercased name to the property whose `JsonPropertyName` is exactly that lowercase string
/// (`LocationTable.cs:11-53,120-128`), so against the raw-keyed root a plain `get` is the same
/// lookup — `any` names no property and resolves to the omitted-map branch. A root the map does
/// not hold is omitted (that omission IS the C# null branch); a present map with no extracts is
/// an empty vec.
fn build_extracts_by_location(locations: &LocationsRoot) -> IndexMap<String, Vec<ExitView>> {
    let mut extracts = IndexMap::new();

    for location_key in E_LOCATION_NAMES_LOWER {
        let Some(entry) = locations.locations.get(location_key) else {
            continue;
        };
        extracts.insert(
            location_key.to_owned(),
            entry.all_extracts.iter().map(to_exit_view).collect(),
        );
    }

    extracts
}

/// `ToExitView` (`RepeatableQuestNativeRequestBuilder.cs:237-246`).
fn to_exit_view(exit: &ExitSourceView) -> ExitView {
    ExitView {
        name: exit.name.clone(),
        side: exit.side.clone(),
        chance: exit.chance,
        passage_requirement: passage_requirement_to_string(exit.passage_requirement.as_deref()),
    }
}

/// The C# `exit.PassageRequirement.ToString()`: a non-nullable `RequirementState` read through
/// `JsonStringEnumConverter` (`LocationBase.cs:868-870`), which matches member names
/// case-insensitively — so the raw wire casing normalizes to the member name `ToString()` writes,
/// and an absent member is `default(RequirementState)` (`None`). A name no member matches could
/// never have deserialized in C# (the converter throws at database load); crossed verbatim to
/// keep the derive total.
fn passage_requirement_to_string(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return REQUIREMENT_STATES[0].to_owned();
    };
    REQUIREMENT_STATES
        .iter()
        .find(|member| member.eq_ignore_ascii_case(raw))
        .map_or_else(|| raw.to_owned(), |member| (*member).to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::models::{GlobalsRoot, LocationsRoot, TemplatesRoot, TradersRoot};
    use crate::ragfair::views::derive as ragfair_derive;

    /// `BaseClasses.WEAPON` — the node id the fixture weapon parents to.
    const WEAPON_NODE: &str = "5422acb9af1c889c16000029";

    fn templates() -> TemplatesRoot {
        serde_json::from_str(
            r#"{
            "items": {
                "5422acb9af1c889c16000029": {"_name":"weapon","_type":"Node","_parent":"","_props":{}},
                "weapon1": {"_name":"ak","_type":"Item","_parent":"5422acb9af1c889c16000029",
                            "_props":{"Width":2,"Height":1}},
                "mod1": {"_name":"mag","_type":"Item","_parent":"5448bc234bdc2d3c308b4569",
                         "_props":{"Width":1,"Height":1}},
                "noprops": {"_name":"broken","_type":"Item","_parent":""}
            },
            "handbook": {"Items":[
                {"Id":"weapon1","ParentId":"cat","Price":1000.9},
                {"Id":"mod1","ParentId":"cat","Price":0.5}
            ]},
            "prices": {"mod1":99.9},
            "repeatableQuests": {
                "templates": {},
                "data": {"Completion": {
                    "itemsWhitelist":[{"minPlayerLevel":10,"itemIds":["a"]}],
                    "itemsBlacklist":[{"minPlayerLevel":null,"itemIds":["b"]}]
                }}
            }
        }"#,
        )
        .expect("templates fixture parses")
    }

    fn globals() -> GlobalsRoot {
        serde_json::from_str(
            r#"{
            "ItemPresets": {
                "preset1": {"_id":"preset1","_name":"ak-default","_items":[
                    {"_id":"root1","_tpl":"weapon1"},
                    {"_id":"child1","_tpl":"mod1","parentId":"root1","slotId":"mod_magazine"}],
                    "_encyclopedia":"weapon1"},
                "presetNoEnc": {"_id":"presetNoEnc","_name":"ak-mod",
                    "_items":[{"_id":"root2","_tpl":"weapon1"}]},
                "preset3": {"_id":"presetX","_name":"key-mismatch",
                    "_items":[{"_id":"root3","_tpl":"weapon1"}],"_encyclopedia":"weapon1"},
                "presetM": {"_id":"presetM","_name":"mag-preset",
                    "_items":[{"_id":"rootM","_tpl":"mod1"}],"_encyclopedia":"mod1"}
            }
        }"#,
        )
        .expect("globals fixture parses")
    }

    fn locations() -> LocationsRoot {
        // Root order deliberately differs from ELocationName order (woods before factory4_day)
        // so the extracts key order proves the enum-domain iteration, not the root's.
        serde_json::from_str(
            r#"{
            "woods": {
                "base": {"Id":"Woods","BossLocationSpawn":[]},
                "allExtracts": [
                    {"Name":"ZB-1011","Side":"Pmc","Chance":100.0,"PassageRequirement":"none"},
                    {"Side":"Scav"},
                    {"Name":"Gate","PassageRequirement":"Train"}
                ]
            },
            "factory4_day": {
                "base": {"Id":"55f2d3fd4bdc2d5f408b4567","BossLocationSpawn":[
                    {"BossName":"bossTagilla"},
                    {"TriggerId":"x"},
                    {"BossName":"bossKilla"}
                ]},
                "allExtracts": []
            },
            "terminal": {"base":{"Id":"Terminal"},"allExtracts":[{"Name":"t"}]},
            "noid": {"base":{"BossLocationSpawn":[{"BossName":"ghost"}]}},
            "base": {"locations":{},"paths":[]}
        }"#,
        )
        .expect("locations fixture parses")
    }

    fn views() -> QuestDbViews {
        let templates = templates();
        let globals = globals();
        let ragfair = Arc::new(
            ragfair_derive(&templates, &TradersRoot::default(), &globals)
                .expect("ragfair derive succeeds"),
        );
        derive(&templates, &globals, &locations(), &ragfair).expect("quest derive succeeds")
    }

    #[test]
    fn ragfair_views_are_shared_by_arc() {
        let templates = templates();
        let globals = globals();
        let ragfair = Arc::new(
            ragfair_derive(&templates, &TradersRoot::default(), &globals)
                .expect("ragfair derive succeeds"),
        );
        let views =
            derive(&templates, &globals, &locations(), &ragfair).expect("quest derive succeeds");
        assert!(Arc::ptr_eq(&views.ragfair, &ragfair));
    }

    #[test]
    fn default_weapon_presets_filter_the_globals_map_by_weapon_encyclopedia() {
        // GetDefaultWeaponPresets (PresetHelper.cs:94-105): encyclopedia present + weapon base
        // class, map order, keyed by map key — preset3's key/_id mismatch never disqualifies it,
        // presetNoEnc has no encyclopedia, presetM's encyclopedia is no weapon.
        let views = views();
        assert_eq!(
            views
                .default_weapon_presets
                .iter()
                .map(|preset| preset.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["preset1", "presetX"]
        );
    }

    #[test]
    fn default_preset_or_item_prices_cover_every_items_key_with_int_truncation() {
        // GetDefaultPresetOrItemPrice (PresetHelper.cs:272-282) per items key, items-table order:
        // - WEAPON_NODE / noprops: no preset, no handbook, no flea -> 0
        // - weapon1: default preset1 [weapon1, mod1] -> (int)1000.9 + (int)99.9 = 1099
        //   (mod1's handbook 0.5 fails the >= 1 static arm and falls to the flea price)
        // - mod1: default presetM [mod1] -> (int)99.9 = 99
        let views = views();
        assert_eq!(
            views
                .default_preset_or_item_prices
                .iter()
                .collect::<Vec<_>>(),
            vec![
                (&WEAPON_NODE.to_owned(), &0.0),
                (&"weapon1".to_owned(), &1099.0),
                (&"mod1".to_owned(), &99.0),
                (&"noprops".to_owned(), &0.0),
            ]
        );
    }

    #[test]
    fn boss_spawns_by_location_key_by_base_id_in_root_order_dropping_nameless() {
        // BuildBossSpawnsByLocation (RepeatableQuestNativeRequestBuilder.cs:191-209): every root
        // entry in map order, skipped without a base Id ("noid", the UI-linkage "base"); values
        // keep spawn order with nameless spawns dropped.
        let views = views();
        assert_eq!(
            views.boss_spawns_by_location.iter().collect::<Vec<_>>(),
            vec![
                (&"Woods".to_owned(), &Vec::<String>::new()),
                (
                    &"55f2d3fd4bdc2d5f408b4567".to_owned(),
                    &vec!["bossTagilla".to_owned(), "bossKilla".to_owned()]
                ),
                (&"Terminal".to_owned(), &Vec::<String>::new()),
            ]
        );
    }

    #[test]
    fn extracts_by_location_iterate_the_elocationname_domain_in_enum_order() {
        // BuildExtractsByLocation (RepeatableQuestNativeRequestBuilder.cs:217-235): the key
        // domain is Enum.GetNames<ELocationName>() lowercased — factory4_day sorts before woods
        // even though the root orders them the other way, "terminal" is no enum member and is
        // omitted despite being in the root, every enum name the root lacks is omitted.
        let views = views();
        assert_eq!(
            views.extracts_by_location.keys().collect::<Vec<_>>(),
            vec!["factory4_day", "woods"]
        );

        // A present map with no extracts is an empty vec, not an omission
        assert!(views.extracts_by_location["factory4_day"].is_empty());

        let woods = &views.extracts_by_location["woods"];
        assert_eq!(
            woods
                .iter()
                .map(|exit| (
                    exit.name.as_deref(),
                    exit.side.as_deref(),
                    exit.chance,
                    exit.passage_requirement.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                // Raw "none" normalizes to the RequirementState member name ToString() writes
                (Some("ZB-1011"), Some("Pmc"), Some(100.0), "None"),
                // An absent PassageRequirement is default(RequirementState) = None
                (None, Some("Scav"), None, "None"),
                (Some("Gate"), None, None, "Train"),
            ]
        );
    }

    #[test]
    fn repeatable_templates_and_completion_filters_lift_off_the_templates_root() {
        let views = views();
        // "templates": {} — all four quest templates absent
        assert!(views.repeatable_quest_templates.elimination.is_none());
        assert!(views.repeatable_quest_templates.completion.is_none());

        // completionFilters?.ItemsWhitelist ?? [] (RepeatableQuestNativeRequestBuilder.cs:166-167)
        assert_eq!(views.completion_items_whitelist.len(), 1);
        assert_eq!(
            views.completion_items_whitelist[0].min_player_level,
            Some(10)
        );
        assert!(views.completion_items_whitelist[0].item_ids.contains("a"));
        assert_eq!(views.completion_items_blacklist.len(), 1);
        assert_eq!(views.completion_items_blacklist[0].min_player_level, None);
        assert!(views.completion_items_blacklist[0].item_ids.contains("b"));

        // A root with no repeatableQuests block at all stays total: default templates, empty lists
        let bare = TemplatesRoot::default();
        let ragfair = Arc::new(
            ragfair_derive(&bare, &TradersRoot::default(), &GlobalsRoot::default()).unwrap(),
        );
        let views = derive(
            &bare,
            &GlobalsRoot::default(),
            &LocationsRoot::default(),
            &ragfair,
        )
        .expect("derive is total over empty roots");
        assert!(views.repeatable_quest_templates.elimination.is_none());
        assert!(views.completion_items_whitelist.is_empty());
        assert!(views.completion_items_blacklist.is_empty());
        assert!(views.default_weapon_presets.is_empty());
        assert!(views.boss_spawns_by_location.is_empty());
        assert!(views.extracts_by_location.is_empty());
    }

    /// The shipped `repeatableQuests.json` block survives the lift whole — proves a real quest
    /// template clones through the derive, which the hand-rolled fixtures keep too small to show.
    #[test]
    fn shipped_repeatable_quest_templates_survive_the_derive() {
        let block = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/repeatableQuests.json"
        ))
        .expect("SPT_Data file readable");
        let templates: TemplatesRoot =
            serde_json::from_str(&format!(r#"{{"repeatableQuests":{block}}}"#)).unwrap();
        let ragfair = Arc::new(
            ragfair_derive(&templates, &TradersRoot::default(), &GlobalsRoot::default()).unwrap(),
        );
        let views = derive(
            &templates,
            &GlobalsRoot::default(),
            &LocationsRoot::default(),
            &ragfair,
        )
        .expect("derive succeeds");
        assert!(views.repeatable_quest_templates.elimination.is_some());
        assert_eq!(
            views.completion_items_whitelist[0].min_player_level,
            Some(1)
        );
    }
}
