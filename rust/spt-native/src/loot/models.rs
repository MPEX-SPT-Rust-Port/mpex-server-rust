//! Wire models for the loot generator.
//!
//! Two families live here:
//!
//! * DB/EFT models mirroring the C# records under `SPTarkov.Server.Core.Models` — field names are
//!   pinned to the exact `JsonPropertyName` of the record they replace. Every one of them carries a
//!   `#[serde(flatten)] extra` map so mod-added fields survive the trip through Rust, matching the
//!   `[JsonExtensionData]` property that `Tools/Ceciler` injects into those types. Nullability
//!   mirrors the C# declaration, and absent stays absent on the way out (C# serializes with
//!   `JsonIgnoreCondition.WhenWritingNull`).
//! * Request/response envelopes — a new contract between the C# caller and this crate, so they are
//!   plain camelCase with no passthrough map.

use std::collections::{BTreeMap, HashMap, HashSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Mod-added fields captured on the way in and replayed on the way out.
type Extra = serde_json::Map<String, serde_json::Value>;

// ---------------------------------------------------------------------------
// DB/EFT wire models
// ---------------------------------------------------------------------------

/// `Models/Eft/Common/Vector3.cs` — `float` members, so `f32` here keeps the serialized
/// representation identical to C#'s. The `[JsonConstructor]` takes the three axes as plain
/// (non-`required`) parameters, so C# substitutes `0` for any the JSON omits instead of throwing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Vector3 {
    #[serde(rename = "x", default)]
    pub x: f32,
    #[serde(rename = "y", default)]
    pub y: f32,
    #[serde(rename = "z", default)]
    pub z: f32,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Item.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename = "_id")]
    pub id: String,
    /// C# declares this non-nullable but not `required`, so a missing key defaults rather than
    /// throwing.
    #[serde(rename = "_tpl", default)]
    pub template: String,
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(rename = "slotId", skip_serializing_if = "Option::is_none")]
    pub slot_id: Option<String>,
    /// Polymorphic: a number (cartridge slot index) or an [`ItemLocation`] object. `item_helper`
    /// writes numbers, `add_loot_to_container` writes objects, nothing ever reads it back.
    #[serde(rename = "location", skip_serializing_if = "Option::is_none")]
    pub location: Option<serde_json::Value>,
    #[serde(rename = "desc", skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(rename = "upd", skip_serializing_if = "Option::is_none")]
    pub upd: Option<Upd>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Item.cs` — write-only in this crate (it is only ever constructed and
/// stuffed into [`Item::location`]), so it needs no passthrough map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemLocation {
    #[serde(rename = "x", skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(rename = "y", skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// Non-`required` in C#, so a missing key lands on the zero value (`Horizontal`).
    #[serde(rename = "r", default)]
    pub r: ItemRotation,
    #[serde(rename = "isSearched", skip_serializing_if = "Option::is_none")]
    pub is_searched: Option<bool>,
    #[serde(rename = "rotation", skip_serializing_if = "Option::is_none")]
    pub rotation: Option<bool>,
}

/// `Models/Eft/Common/Tables/Item.cs` — serialized as a string by `JsonStringEnumConverter`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemRotation {
    #[default]
    Horizontal,
    Vertical,
}

/// `Models/Eft/Common/Tables/Item.cs` — the C# record carries no `JsonPropertyName` on its members,
/// so the wire names are the property names verbatim. Only the one member the generator touches is
/// typed; the rest ride along in `extra`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Upd {
    #[serde(
        rename = "StackObjectsCount",
        default,
        deserialize_with = "deserialize_string_or_number",
        skip_serializing_if = "Option::is_none"
    )]
    pub stack_objects_count: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Mirrors the `[JsonConverter(typeof(StringToNumberFactoryConverter))]` on C#'s
/// `Upd.StackObjectsCount`. The loose-loot payload reaches this crate as the raw bytes of a
/// location's `looseLoot.json` (`LooseLootPayload.RawJson`, to skip a 42 MB parse-and-re-encode),
/// so that converter never runs on the native path and a stringly-typed count in the database
/// (lighthouse ships two, `"20"`) would fail the whole request deserialize. Blank,
/// `__REPLACEME__` and otherwise unparseable strings fall to `None`, as the converter does.
fn deserialize_string_or_number<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(text.trim().parse::<f64>().ok()),
        Some(other) => other.as_f64().map(Some).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "expected a number or numeric string, found {other}"
            ))
        }),
    }
}

/// `Models/Eft/Common/LooseLoot.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpawnpointTemplate {
    #[serde(rename = "Id", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "IsContainer", skip_serializing_if = "Option::is_none")]
    pub is_container: Option<bool>,
    #[serde(rename = "useGravity", skip_serializing_if = "Option::is_none")]
    pub use_gravity: Option<bool>,
    #[serde(rename = "randomRotation", skip_serializing_if = "Option::is_none")]
    pub random_rotation: Option<bool>,
    #[serde(rename = "Position", skip_serializing_if = "Option::is_none")]
    pub position: Option<Vector3>,
    #[serde(rename = "Rotation", skip_serializing_if = "Option::is_none")]
    pub rotation: Option<Vector3>,
    #[serde(rename = "IsAlwaysSpawn", skip_serializing_if = "Option::is_none")]
    pub is_always_spawn: Option<bool>,
    #[serde(rename = "IsGroupPosition", skip_serializing_if = "Option::is_none")]
    pub is_group_position: Option<bool>,
    /// Passthrough only — the generator never reads group positions, so they stay untyped.
    #[serde(rename = "GroupPositions", skip_serializing_if = "Option::is_none")]
    pub group_positions: Option<serde_json::Value>,
    #[serde(rename = "Root", skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(rename = "Items", skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<SptLootItem>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/LooseLoot.cs` — `record SptLootItem : Item`. Rust has no record inheritance,
/// so the base is flattened in; `Item`'s own `extra` still catches everything unknown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SptLootItem {
    #[serde(flatten)]
    pub item: Item,
    #[serde(rename = "composedKey", skip_serializing_if = "Option::is_none")]
    pub composed_key: Option<String>,
}

/// `Models/Eft/Common/LooseLoot.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Spawnpoint {
    #[serde(rename = "locationId", skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    #[serde(rename = "probability", skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
    #[serde(rename = "template", skip_serializing_if = "Option::is_none")]
    pub template: Option<SpawnpointTemplate>,
    #[serde(rename = "itemDistribution", skip_serializing_if = "Option::is_none")]
    pub item_distribution: Option<Vec<LooseLootItemDistribution>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/LooseLoot.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LooseLootItemDistribution {
    #[serde(rename = "composedKey", skip_serializing_if = "Option::is_none")]
    pub composed_key: Option<ComposedKey>,
    #[serde(
        rename = "relativeProbability",
        skip_serializing_if = "Option::is_none"
    )]
    pub relative_probability: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/LooseLoot.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComposedKey {
    #[serde(rename = "key", skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/LooseLoot.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LooseLoot {
    #[serde(rename = "spawnpointCount", skip_serializing_if = "Option::is_none")]
    pub spawnpoint_count: Option<SpawnpointCount>,
    #[serde(rename = "spawnpointsForced", skip_serializing_if = "Option::is_none")]
    pub spawnpoints_forced: Option<Vec<Spawnpoint>>,
    #[serde(rename = "spawnpoints", skip_serializing_if = "Option::is_none")]
    pub spawnpoints: Option<Vec<Spawnpoint>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/LooseLoot.cs` — both members are `required` in C#.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpawnpointCount {
    #[serde(rename = "mean")]
    pub mean: f64,
    #[serde(rename = "std")]
    pub std: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticContainerData {
    #[serde(rename = "probability", skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
    #[serde(rename = "template", skip_serializing_if = "Option::is_none")]
    pub template: Option<SpawnpointTemplate>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticForced {
    /// Plain non-`required` `string` in C# (`Location.cs:121`), so a null one is dropped on the way
    /// out by `WhenWritingNull` and must not be a hard parse error on the way back in.
    #[serde(rename = "containerId", default)]
    pub container_id: String,
    #[serde(rename = "itemTpl", default)]
    pub item_tpl: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs` — note the lowercase `c` in `itemcountDistribution`.
///
/// Both members are declared non-nullable in C# yet explicitly null-checked at every read
/// (`LocationLootGenerator.cs:556,592`), because bad map data leaves them null at runtime. `Option`
/// keeps those two branches reachable — an absent distribution is not the same as an empty one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticLootDetails {
    #[serde(
        rename = "itemcountDistribution",
        skip_serializing_if = "Option::is_none"
    )]
    pub item_count_distribution: Option<Vec<ItemCountDistribution>>,
    #[serde(rename = "itemDistribution", skip_serializing_if = "Option::is_none")]
    pub item_distribution: Option<Vec<ItemDistribution>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemCountDistribution {
    #[serde(rename = "count", skip_serializing_if = "Option::is_none")]
    pub count: Option<i32>,
    #[serde(
        rename = "relativeProbability",
        skip_serializing_if = "Option::is_none"
    )]
    pub relative_probability: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemDistribution {
    /// Non-`required` `MongoId` in C#, which defaults rather than throwing on a missing key.
    #[serde(rename = "tpl", default)]
    pub tpl: String,
    #[serde(
        rename = "relativeProbability",
        skip_serializing_if = "Option::is_none"
    )]
    pub relative_probability: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticAmmoDetails {
    #[serde(rename = "tpl", skip_serializing_if = "Option::is_none")]
    pub tpl: Option<String>,
    #[serde(
        rename = "relativeProbability",
        skip_serializing_if = "Option::is_none"
    )]
    pub relative_probability: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticContainer {
    /// `IndexMap`, not `HashMap` or `BTreeMap`: `get_group_id_to_container_mappings` draws a
    /// `get_int` per group as it walks this map, and the C# `Dictionary` it stands in for walks in
    /// JSON order. A hashed order leaves generation non-reproducible; a sorted one is reproducible
    /// but hands different groups different draws than the C#.
    #[serde(rename = "containersGroups", skip_serializing_if = "Option::is_none")]
    pub containers_groups: Option<IndexMap<String, ContainerMinMax>>,
    /// Keyed lookups only, so iteration order never reaches the RNG.
    #[serde(rename = "containers", skip_serializing_if = "Option::is_none")]
    pub containers: Option<HashMap<String, ContainerData>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerMinMax {
    #[serde(rename = "minContainers", skip_serializing_if = "Option::is_none")]
    pub min_containers: Option<i32>,
    #[serde(rename = "maxContainers", skip_serializing_if = "Option::is_none")]
    pub max_containers: Option<i32>,
    #[serde(rename = "current", skip_serializing_if = "Option::is_none")]
    pub current: Option<i32>,
    #[serde(rename = "chosenCount", skip_serializing_if = "Option::is_none")]
    pub chosen_count: Option<i32>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Location.cs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerData {
    #[serde(rename = "groupId", skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---------------------------------------------------------------------------
// Request / response envelopes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LootCommon {
    /// Already lowercased by the C# caller.
    pub location_id: String,
    pub items_view: HashMap<String, ItemView>,
    pub default_presets: HashMap<String, PresetView>,
    pub money_tpls: Vec<String>,
    pub static_ammo_dist: HashMap<String, Vec<StaticAmmoDetails>>,
    pub config: LootConfigView,
    pub seasonal: SeasonalView,
    pub lootable_item_blacklist: HashSet<String>,
    pub counter: CounterState,
    /// Test-only: when present, every draw comes from a seeded xoshiro256** for the duration of
    /// the call (see `random_util::TestSeedGuard`). Never set on the production path.
    pub test_seed: Option<u64>,
}

/// The slice of `TemplateItem` the generator actually reads, flattened by the C# caller.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    pub parent: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub stack_max_size: Option<i32>,
    pub stack_min_random: Option<i32>,
    pub stack_max_random: Option<i32>,
    pub extra_size_up: Option<i32>,
    pub extra_size_down: Option<i32>,
    pub extra_size_left: Option<i32>,
    pub extra_size_right: Option<i32>,
    pub extra_size_force_add: Option<bool>,
    /// First grid only.
    pub grid_cells_h: Option<i32>,
    pub grid_cells_v: Option<i32>,
    pub stack_slot_max_count: Option<f64>,
    pub stack_slot_first_filter_first: Option<String>,
    pub cartridges_max_count: Option<f64>,
    pub cartridges_first_filter: Option<Vec<String>>,
    pub chambers_first_filter: Option<Vec<String>>,
    pub slots: Option<Vec<SlotView>>,
    pub conflicting_items: Option<Vec<String>>,
    pub caliber: Option<String>,
    pub ammo_caliber: Option<String>,
    pub def_ammo: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotView {
    pub name: Option<String>,
    pub required: Option<bool>,
    pub filter: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetView {
    pub items: Vec<Item>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LootConfigView {
    pub container_randomisation_enabled: bool,
    /// C# resolves `Maps.ContainsKey(locationId)`.
    pub location_in_randomisation_maps: bool,
    pub container_types_to_not_randomise: HashSet<String>,
    pub container_group_min_size_multiplier: f64,
    pub container_group_max_size_multiplier: f64,
    pub allow_duplicate_items_in_static_containers: bool,
    pub tpls_to_strip_child_items_from: HashSet<String>,
    pub fit_loot_into_container_attempts: i32,
    pub magazine_loot_has_ammo_chance_percent: f64,
    pub static_magazine_loot_has_ammo_chance_percent: f64,
    pub min_fill_loose_magazine_percent: f64,
    pub min_fill_static_magazine_percent: f64,
    /// Resolved per-location by C#.
    pub static_loot_multiplier: f64,
    /// Resolved per-location by C#.
    pub loose_loot_multiplier: f64,
    /// `EquipmentLootSettings`.
    pub mod_spawn_chance_percent: HashMap<String, f64>,
    /// Resolved per-location by C#.
    pub loose_loot_blacklist: HashSet<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonalView {
    pub seasonal_event_active: bool,
    pub christmas_event_enabled: bool,
    pub inactive_seasonal_items: HashSet<String>,
    pub christmas_container_ids: HashSet<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CounterState {
    /// Keyed lookups only, so iteration order never reaches the RNG or the wire.
    pub max_counts: HashMap<String, i32>,
    /// `BTreeMap`, not `HashMap`: this rides back out on both result envelopes, and a randomised
    /// iteration order would make the serialised result differ between two seeded runs. C#
    /// deserialises it into a `Dictionary` either way.
    pub tracked_counts: BTreeMap<String, i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticContainersRequest {
    #[serde(flatten)]
    pub common: LootCommon,
    /// The three `StaticContainerDetails` members are `Option` because
    /// `LocationLootGenerator.cs:105,114,121` each test theirs for null and log a map-specific
    /// error; keeping them non-optional would make those three branches unreachable.
    pub static_weapons: Option<Vec<SpawnpointTemplate>>,
    pub static_containers: Option<Vec<StaticContainerData>>,
    pub static_forced: Option<Vec<StaticForced>>,
    pub static_loot_dist: HashMap<String, StaticLootDetails>,
    pub statics: Option<StaticContainer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicLootRequest {
    #[serde(flatten)]
    pub common: LootCommon,
    pub loose_loot: LooseLoot,
}

/// Diagnostic levels, one per `logger` method the ported C# calls.
pub const DEBUG: &str = "debug";
/// See [`DEBUG`].
pub const WARNING: &str = "warning";
/// See [`DEBUG`].
pub const ERROR: &str = "error";
/// See [`DEBUG`].
pub const SUCCESS: &str = "success";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// One of [`DEBUG`], [`WARNING`], [`ERROR`], [`SUCCESS`].
    pub level: String,
    pub locale_key: Option<String>,
    /// Object; C# replays it via `ServerLocalisationService`.
    pub args: Option<serde_json::Value>,
    /// Plain interpolated messages.
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticContainersResult {
    pub spawnpoints: Vec<SpawnpointTemplate>,
    pub tracked_counts: BTreeMap<String, i32>,
    pub static_loot_item_count: i32,
    pub static_container_count: i32,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicLootResult {
    pub spawnpoints: Vec<SpawnpointTemplate>,
    pub tracked_counts: BTreeMap<String, i32>,
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every required `LootCommon` member, for splicing into an envelope test literal. `testSeed`
    /// is deliberately absent — its omission is what exercises the missing-field → `None` path.
    const COMMON_JSON: &str = r#"
        "locationId":"bigmap",
        "itemsView":{"aaaaaaaaaaaaaaaaaaaaaaaa":{"parent":"bbbbbbbbbbbbbbbbbbbbbbbb","width":2,"height":1,
            "slots":[{"name":"mod_magazine","required":false,"filter":["cccccccccccccccccccccccc"]}]}},
        "defaultPresets":{"p1":{"items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb"}]}},
        "moneyTpls":["5449016a4bdc2d6f028b456f"],
        "staticAmmoDist":{"Caliber762x39":[{"tpl":"dddddddddddddddddddddddd","relativeProbability":5}]},
        "config":{"containerRandomisationEnabled":true,"locationInRandomisationMaps":true,
            "containerTypesToNotRandomise":["eeeeeeeeeeeeeeeeeeeeeeee"],
            "containerGroupMinSizeMultiplier":0.8,"containerGroupMaxSizeMultiplier":1.2,
            "allowDuplicateItemsInStaticContainers":false,"tplsToStripChildItemsFrom":[],
            "fitLootIntoContainerAttempts":3,"magazineLootHasAmmoChancePercent":25,
            "staticMagazineLootHasAmmoChancePercent":50,"minFillLooseMagazinePercent":30,
            "minFillStaticMagazinePercent":30,"staticLootMultiplier":1.5,"looseLootMultiplier":1.1,
            "modSpawnChancePercent":{"mod_scope":25},"looseLootBlacklist":[]},
        "seasonal":{"seasonalEventActive":false,"christmasEventEnabled":false,
            "inactiveSeasonalItems":[],"christmasContainerIds":[]},
        "lootableItemBlacklist":[],
        "counter":{"maxCounts":{"ffffffffffffffffffffffff":2},"trackedCounts":{}}
    "#;

    #[test]
    fn spawnpoint_template_round_trips_unknown_fields() {
        let json = r#"{"Id":"sp_1","IsContainer":false,"useGravity":true,"randomRotation":false,
        "Position":{"x":1.5,"y":2.0,"z":-3.25},"Root":"aaaaaaaaaaaaaaaaaaaaaaaa",
        "Items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb","composedKey":"ck1","modFieldFromAMod":7}],
        "customTemplateField":"kept"}"#;
        let parsed: SpawnpointTemplate = serde_json::from_str(json).unwrap();
        let out = serde_json::to_value(&parsed).unwrap();
        assert_eq!(out["customTemplateField"], "kept");
        assert_eq!(out["Items"][0]["modFieldFromAMod"], 7);
        assert_eq!(out["useGravity"], true); // exact wire casing
        assert_eq!(out["Position"]["z"], -3.25);
    }

    #[test]
    fn absent_optional_fields_stay_absent() {
        let parsed: SpawnpointTemplate =
            serde_json::from_str(r#"{"Id":"sp_1","Items":[]}"#).unwrap();
        let out = serde_json::to_value(&parsed).unwrap();
        let object = out.as_object().unwrap();
        assert_eq!(object.keys().collect::<Vec<_>>(), vec!["Id", "Items"]);
    }

    #[test]
    fn flattened_base_item_does_not_duplicate_composed_key() {
        let parsed: SptLootItem = serde_json::from_str(
            r#"{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb",
               "composedKey":"ck1","upd":{"StackObjectsCount":3,"SpawnedInSession":true}}"#,
        )
        .unwrap();
        assert_eq!(parsed.composed_key.as_deref(), Some("ck1"));
        assert!(!parsed.item.extra.contains_key("composedKey"));
        let upd = parsed.item.upd.as_ref().unwrap();
        assert_eq!(upd.stack_objects_count, Some(3.0));
        assert_eq!(upd.extra["SpawnedInSession"], true);
    }

    /// The raw-JSON loose-loot path bypasses C#'s `StringToNumberFactoryConverter`, so this crate
    /// has to accept what the database actually ships (lighthouse: `"StackObjectsCount": "20"`).
    #[test]
    fn stack_objects_count_accepts_a_numeric_string() {
        let cases = [
            (r#"{"StackObjectsCount":"20"}"#, Some(20.0)),
            (r#"{"StackObjectsCount":20}"#, Some(20.0)),
            (r#"{"StackObjectsCount":""}"#, None),
            (r#"{"StackObjectsCount":"__REPLACEME__"}"#, None),
            (r#"{"StackObjectsCount":null}"#, None),
            ("{}", None),
        ];

        for (json, expected) in cases {
            let upd: Upd = serde_json::from_str(json).unwrap();
            assert_eq!(upd.stack_objects_count, expected, "parsing {json}");
            assert!(
                !upd.extra.contains_key("StackObjectsCount"),
                "parsing {json}"
            );
        }

        assert!(serde_json::from_str::<Upd>(r#"{"StackObjectsCount":[1]}"#).is_err());
    }

    #[test]
    fn item_location_serializes_rotation_as_a_string() {
        let out = serde_json::to_value(ItemLocation {
            x: Some(1),
            y: Some(2),
            r: ItemRotation::Vertical,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(out, serde_json::json!({"x":1,"y":2,"r":"Vertical"}));
    }

    /// None of these members is `required` in C#, so a map or mod that omits one gets the type's
    /// zero value there rather than taking down the whole request deserialize. A hard error would
    /// surface to the operator as "native library bug" when it is really just sparse game data.
    #[test]
    fn non_required_members_default_instead_of_failing_the_parse() {
        let vector: Vector3 = serde_json::from_str("{}").unwrap();
        assert_eq!((vector.x, vector.y, vector.z), (0.0, 0.0, 0.0));

        let partial: Vector3 = serde_json::from_str(r#"{"y":4.5}"#).unwrap();
        assert_eq!((partial.x, partial.y, partial.z), (0.0, 4.5, 0.0));

        let forced: StaticForced = serde_json::from_str("{}").unwrap();
        assert_eq!(forced.container_id, "");
        assert_eq!(forced.item_tpl, "");

        let distribution: ItemDistribution = serde_json::from_str("{}").unwrap();
        assert_eq!(distribution.tpl, "");
        assert_eq!(distribution.relative_probability, None);

        let location: ItemLocation = serde_json::from_str("{}").unwrap();
        assert_eq!(location.r, ItemRotation::Horizontal);

        // Both distributions are `Option`, which serde already fills with `None` on a missing key.
        // `None` is the value the generator's warning branches look for, so it must stay `None`
        // rather than becoming an empty `Vec` (see the note in the fix report).
        let details: StaticLootDetails = serde_json::from_str("{}").unwrap();
        assert!(details.item_count_distribution.is_none());
        assert!(details.item_distribution.is_none());
    }

    #[test]
    fn static_containers_request_deserializes() {
        let json = format!(
            r#"{{{COMMON_JSON},
            "staticWeapons":[{{"Id":"w1","Root":"aaaaaaaaaaaaaaaaaaaaaaaa","Items":[]}}],
            "staticContainers":[{{"probability":0.35,"template":{{"Id":"c1","IsContainer":true}}}}],
            "staticForced":[{{"containerId":"c1","itemTpl":"dddddddddddddddddddddddd"}}],
            "staticLootDist":{{"eeeeeeeeeeeeeeeeeeeeeeee":{{
                "itemcountDistribution":[{{"count":1,"relativeProbability":10}}],
                "itemDistribution":[{{"tpl":"dddddddddddddddddddddddd","relativeProbability":10}}]}}}},
            "statics":{{"containersGroups":{{"g1":{{"minContainers":1,"maxContainers":3}}}},
                "containers":{{"c1":{{"groupId":"g1"}}}}}}}}"#
        );
        let parsed: StaticContainersRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.common.location_id, "bigmap");
        assert_eq!(parsed.common.config.static_loot_multiplier, 1.5);
        assert_eq!(
            parsed.common.counter.max_counts["ffffffffffffffffffffffff"],
            2
        );
        assert_eq!(
            parsed.common.items_view["aaaaaaaaaaaaaaaaaaaaaaaa"].width,
            Some(2)
        );
        assert_eq!(parsed.static_weapons.unwrap()[0].id.as_deref(), Some("w1"));
        assert_eq!(parsed.static_containers.unwrap()[0].probability, Some(0.35));
        assert_eq!(parsed.static_forced.unwrap()[0].container_id, "c1");
        assert_eq!(
            parsed.static_loot_dist["eeeeeeeeeeeeeeeeeeeeeeee"]
                .item_count_distribution
                .as_ref()
                .unwrap()[0]
                .count,
            Some(1)
        );
        assert_eq!(
            parsed.statics.unwrap().containers_groups.unwrap()["g1"].max_containers,
            Some(3)
        );
    }

    #[test]
    fn dynamic_loot_request_deserializes() {
        let json = format!(
            r#"{{{COMMON_JSON},
            "looseLoot":{{"spawnpointCount":{{"mean":12.5,"std":2}},
                "spawnpointsForced":[{{"locationId":"f1","probability":1,"template":{{"Id":"t1"}}}}],
                "spawnpoints":[{{"locationId":"s1","probability":0.5,"template":{{"Id":"t2"}},
                    "itemDistribution":[{{"composedKey":{{"key":"ck1"}},"relativeProbability":4}}]}}]}}}}"#
        );
        let parsed: DynamicLootRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.common.money_tpls, vec!["5449016a4bdc2d6f028b456f"]);
        let loose = &parsed.loose_loot;
        assert_eq!(loose.spawnpoint_count.as_ref().unwrap().mean, 12.5);
        assert_eq!(loose.spawnpoints_forced.as_ref().unwrap().len(), 1);
        let distribution = loose.spawnpoints.as_ref().unwrap()[0]
            .item_distribution
            .as_ref()
            .unwrap();
        assert_eq!(
            distribution[0]
                .composed_key
                .as_ref()
                .unwrap()
                .key
                .as_deref(),
            Some("ck1")
        );
    }

    #[test]
    fn results_serialize_with_camel_case_keys() {
        let out = serde_json::to_value(StaticContainersResult {
            spawnpoints: vec![],
            tracked_counts: BTreeMap::from([("tpl".to_owned(), 1)]),
            static_loot_item_count: 4,
            static_container_count: 2,
            diagnostics: vec![Diagnostic {
                level: "warning".to_owned(),
                locale_key: Some("loot-missing_item".to_owned()),
                args: Some(serde_json::json!({"tpl":"x"})),
                message: None,
            }],
        })
        .unwrap();

        assert_eq!(out["staticLootItemCount"], 4);
        assert_eq!(out["staticContainerCount"], 2);
        assert_eq!(out["trackedCounts"]["tpl"], 1);
        assert_eq!(out["diagnostics"][0]["localeKey"], "loot-missing_item");
        assert_eq!(out["diagnostics"][0]["args"]["tpl"], "x");
    }
}
