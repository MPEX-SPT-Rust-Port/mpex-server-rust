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

use indexmap::{IndexMap, IndexSet};
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
    #[serde(rename = "Repairable", skip_serializing_if = "Option::is_none")]
    pub repairable: Option<UpdRepairable>,
    #[serde(rename = "Buff", skip_serializing_if = "Option::is_none")]
    pub buff: Option<UpdBuff>,
    #[serde(rename = "Togglable", skip_serializing_if = "Option::is_none")]
    pub togglable: Option<UpdTogglable>,
    #[serde(rename = "Foldable", skip_serializing_if = "Option::is_none")]
    pub foldable: Option<UpdFoldable>,
    #[serde(rename = "FireMode", skip_serializing_if = "Option::is_none")]
    pub fire_mode: Option<UpdFireMode>,
    #[serde(rename = "MedKit", skip_serializing_if = "Option::is_none")]
    pub med_kit: Option<UpdMedKit>,
    #[serde(rename = "FoodDrink", skip_serializing_if = "Option::is_none")]
    pub food_drink: Option<UpdFoodDrink>,
    #[serde(rename = "Light", skip_serializing_if = "Option::is_none")]
    pub light: Option<UpdLight>,
    #[serde(rename = "Key", skip_serializing_if = "Option::is_none")]
    pub key: Option<UpdKey>,
    #[serde(rename = "Resource", skip_serializing_if = "Option::is_none")]
    pub resource: Option<UpdResource>,
    #[serde(rename = "RepairKit", skip_serializing_if = "Option::is_none")]
    pub repair_kit: Option<UpdRepairKit>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Item.cs:310-315` — read by
/// `item_helper::get_item_quality_modifier`'s key arm.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdKey {
    #[serde(rename = "NumberOfUsages", skip_serializing_if = "Option::is_none")]
    pub number_of_usages: Option<i32>,
}

/// `Models/Eft/Common/Tables/Item.cs:317-324`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdResource {
    #[serde(rename = "Value", skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(rename = "UnitsConsumed", skip_serializing_if = "Option::is_none")]
    pub units_consumed: Option<f64>,
}

/// `Models/Eft/Common/Tables/Item.cs:389-393`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdRepairKit {
    #[serde(rename = "Resource", skip_serializing_if = "Option::is_none")]
    pub resource: Option<f64>,
}

/// `Models/Eft/Common/Tables/Item.cs:212-216` — written by
/// `bot_generator_helper::generate_extra_properties_for_item` for hinged, night-vision and face
/// shield items.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdTogglable {
    #[serde(rename = "On", skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// `Models/Eft/Common/Tables/Item.cs:292-296`. Also read by `InventoryHelper.GetItemSize`'s folding
/// arithmetic, which the container-grid port mirrors.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdFoldable {
    #[serde(rename = "Folded", skip_serializing_if = "Option::is_none")]
    pub folded: Option<bool>,
}

/// `Models/Eft/Common/Tables/Item.cs:298-302`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdFireMode {
    #[serde(rename = "FireMode", skip_serializing_if = "Option::is_none")]
    pub fire_mode: Option<String>,
}

/// `Models/Eft/Common/Tables/Item.cs:272-276`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdMedKit {
    #[serde(rename = "HpResource", skip_serializing_if = "Option::is_none")]
    pub hp_resource: Option<f64>,
}

/// `Models/Eft/Common/Tables/Item.cs:304-308`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdFoodDrink {
    #[serde(rename = "HpPercent", skip_serializing_if = "Option::is_none")]
    pub hp_percent: Option<f64>,
}

/// `Models/Eft/Common/Tables/Item.cs:326-333`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdLight {
    #[serde(rename = "IsActive", skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
    #[serde(rename = "SelectedMode", skip_serializing_if = "Option::is_none")]
    pub selected_mode: Option<i32>,
}

/// `Models/Eft/Common/Tables/Item.cs` — written by the bot generator's durability rolls and read
/// by `repair_service::add_buff`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdRepairable {
    #[serde(rename = "Durability", skip_serializing_if = "Option::is_none")]
    pub durability: Option<f64>,
    #[serde(rename = "MaxDurability", skip_serializing_if = "Option::is_none")]
    pub max_durability: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Item.cs` — written by `repair_service::add_buff`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdBuff {
    #[serde(rename = "Rarity", skip_serializing_if = "Option::is_none")]
    pub rarity: Option<String>,
    #[serde(rename = "BuffType", skip_serializing_if = "Option::is_none")]
    pub buff_type: Option<RepairBuffType>,
    #[serde(rename = "Value", skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(
        rename = "ThresholdDurability",
        skip_serializing_if = "Option::is_none"
    )]
    pub threshold_durability: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Enums/RepairBuffType.cs`. `UpdBuff.BuffType` carries a `[JsonStringEnumConverter]`, so
/// the wire form is the variant name verbatim — which is also what serde emits by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairBuffType {
    WeaponSpread,
    DamageReduction,
    MalfunctionProtections,
    WeaponDamage,
    ArmorEfficiency,
    DurabilityImprovement,
}

impl RepairBuffType {
    /// `Enum.Parse<RepairBuffType>(name)` — case-sensitive, as the C# overload without an
    /// `ignoreCase` argument is. `None` where the C# throws `ArgumentException`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "WeaponSpread" => Some(Self::WeaponSpread),
            "DamageReduction" => Some(Self::DamageReduction),
            "MalfunctionProtections" => Some(Self::MalfunctionProtections),
            "WeaponDamage" => Some(Self::WeaponDamage),
            "ArmorEfficiency" => Some(Self::ArmorEfficiency),
            "DurabilityImprovement" => Some(Self::DurabilityImprovement),
            _ => None,
        }
    }
}

/// Mirrors the `[JsonConverter(typeof(StringToNumberFactoryConverter))]` on C#'s
/// `Upd.StackObjectsCount`. The loose-loot payload reaches this crate as the raw bytes of a
/// location's `looseLoot.json` (`LooseLootPayload.RawJson`, to skip a 42 MB parse-and-re-encode),
/// so that converter never runs on the native path and a stringly-typed count in the database
/// (lighthouse ships two, `"20"`) would fail the whole request deserialize. Blank,
/// `__REPLACEME__` and otherwise unparseable strings fall to `None`, as the converter does.
///
/// `double.Parse(value, InvariantCulture)` runs with `NumberStyles.Float | AllowThousands`, so
/// `"1,000"` is 1000; stripping the invariant group separator mirrors that for every real payload.
/// It is looser only on inputs C# rejects outright (`",5"` throws and yields `None` there, 5 here),
/// none of which appear in the database.
///
/// Not mirrored: C#'s `Upd.StackObjectsCount` setter rounds with
/// `Math.Round(value, 0, MidpointRounding.AwayFromZero)`. Unobservable — `item_helper::split_stack`
/// is the only reader and every count that reaches it has already been through that setter, so the
/// values are integral, and everything written here re-enters C# through the same setter.
pub(crate) fn deserialize_string_or_number<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => {
            Ok(text.trim().replace(',', "").parse::<f64>().ok())
        }
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

/// The per-call half of both location-loot envelopes: services, per-raid state, and the
/// caller-supplied `staticAmmoDist` (a frozen public-API parameter, so the resident DB can never
/// stand in for it). `moneyTpls` is service-backed (`ItemHelper.GetMoneyTpls`) and rides here
/// under the Phases-2/4 carve-out. The config view left for the views side in Phase 4: the
/// resident arm resolves it per call off the `spt-location` stem.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LootVarying {
    /// Already lowercased by the C# caller; the resident arm's key into the locations root and into
    /// every map-keyed member of the `spt-location` config stem.
    pub location_id: String,
    pub money_tpls: Vec<String>,
    pub static_ammo_dist: HashMap<String, Vec<StaticAmmoDetails>>,
    pub seasonal: SeasonalView,
    pub lootable_item_blacklist: HashSet<String>,
    pub counter: CounterState,
    /// Test-only: when present, every draw comes from a seeded xoshiro256** for the duration of
    /// the call (see `random_util::TestSeedGuard`). Never set on the production path.
    pub test_seed: Option<u64>,
}

/// The distrust fallback (spec § Exports): the C#-built database half plus the two config-backed
/// members the resident arm resolves off the configs root, present iff the caller is ineligible
/// for residency. The statics members are present on static-container sends and absent on dynamic
/// sends, mirroring the two old envelopes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LootViewsWire {
    /// `IndexMap` only so the whole loot module shares one map type with [`RewardViewsWire`] — this
    /// envelope's view is looked up by key, never iterated, so the order never reaches the RNG.
    pub items_view: IndexMap<String, ItemView>,
    /// `GetDefaultPresetByTpl()` projected Items-only (`LocationLootGenerator.BuildViewsOverride`).
    pub default_presets: IndexMap<String, PresetView>,
    /// `BuildConfigView(locationId)` — the C#-resolved view of the `spt-location` config. The
    /// resident arm builds the same thing per call (`resolve_loot_config_view`).
    pub config: LootConfigView,
    /// `SeasonalEventConfig.ChristmasContainerIds`; the resident arm reads it off the
    /// `spt-seasonalevents` stem. Spawn point ids, not tpls.
    pub christmas_container_ids: HashSet<String>,
    pub static_weapons: Option<Vec<SpawnpointTemplate>>,
    pub static_containers: Option<Vec<StaticContainerData>>,
    pub static_forced: Option<Vec<StaticForced>>,
    pub static_loot_dist: Option<HashMap<String, StaticLootDetails>>,
    pub statics: Option<StaticContainer>,
}

/// The slice of `TemplateItem` the generator actually reads, flattened by the C# caller.
/// `Serialize` exists for the phase-1 equivalence harness (`tests/phase1_ragfair_views.rs`),
/// which serializes a natively-derived view to compare against the C#-built one — the
/// `serde(rename)`s make that serialization mirror the wire names.
#[derive(Debug, Default, Serialize, Deserialize)]
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
    /// `TemplateItem.Properties.Chambers` (`TemplateItem.cs:794-795`) as slot objects.
    /// [`ItemView::chambers_first_filter`] flattens the first chamber's filter, which is all the loot
    /// port needs; `GetModItemSlotFromDbTemplate`'s `patron_in_weapon*` arm needs the slots
    /// themselves.
    pub chambers: Option<Vec<SlotView>>,
    /// `TemplateItem.Properties.Cartridges` as slot objects — the `cartridges` arm of the same
    /// method. [`ItemView::cartridges_max_count`] / [`ItemView::cartridges_first_filter`] stay: they
    /// flatten the *first* cartridge, and nothing reads a `MaxCount` off this list.
    pub cartridges: Option<Vec<SlotView>>,
    pub conflicting_items: Option<Vec<String>>,
    pub caliber: Option<String>,
    pub ammo_caliber: Option<String>,
    pub def_ammo: Option<String>,
    /// `TemplateItem.Properties.ReloadMode` (`TemplateItem.cs:807-808`) — the `ReloadMode` enum
    /// member name verbatim (`ExternalMagazine`, `InternalMagazine`, `OnlyBarrel`,
    /// `ExternalMagazineWithInternalReloadSupport`), which is how `JsonStringEnumConverter` writes
    /// it. Only `OnlyBarrel` is ever compared against.
    pub reload_mode: Option<String>,
    /// `TemplateItem.Properties.ReloadMagType` (`TemplateItem.cs:568-569`), same enum. Only
    /// `InternalMagazine` is ever compared against.
    pub reload_mag_type: Option<String>,
    /// `TemplateItem.Properties.IsChamberLoad` (`TemplateItem.cs:771`).
    pub is_chamber_load: Option<bool>,
    /// `TemplateItem.Properties.DefMagType` (`TemplateItem.cs:780`) — what
    /// `TemplateItemExtensions.GetWeaponsDefaultMagazineTpl` returns.
    pub def_mag_type: Option<String>,
    /// `TemplateItem.Properties.LinkedWeapon` (`TemplateItem.cs:1484-1485`). Only its emptiness is
    /// read, by `BotWeaponGenerator.GetWeaponCaliber`.
    pub linked_weapon: Option<String>,
    /// `TemplateItem._name` — the sealed-crate pool is found by substring on it
    /// (`LootGenerator.cs:57`).
    pub name: Option<String>,
    /// `TemplateItem._type` — the `"item"` type filter (`LootGenerator.cs:245,568`).
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    /// `TemplateItem.Properties.ArmorClass`, declared `int?` (`TemplateItem.cs:668`).
    pub armor_class: Option<i32>,
    /// `TemplateItem.Properties.QuestItem`. Null and `false` are not interchangeable:
    /// `LootGenerator.cs:246` reads `GetValueOrDefault(false)`, `:571` tests `is null`.
    pub quest_item: Option<bool>,

    // -- Added for the bot port (`bot::bot_generator_helper`). Every one is an `Option`, so a loot
    // -- payload that omits them still deserializes.
    /// `TemplateItem.Properties.MaxDurability` (`TemplateItem.cs:659`).
    pub max_durability: Option<f64>,
    /// `TemplateItem.Properties.WeapClass`, wire name `weapClass` (`TemplateItem.cs:695`). Only its
    /// presence is read — it is what marks a template "is weapon".
    pub weap_class: Option<String>,
    /// `TemplateItem.Properties.HasHinge` (`TemplateItem.cs:492`).
    pub has_hinge: Option<bool>,
    /// `TemplateItem.Properties.Foldable` (`TemplateItem.cs:580`).
    pub foldable: Option<bool>,
    /// `TemplateItem.Properties.FoldedSlot` (`TemplateItem.cs:831`).
    pub folded_slot: Option<String>,
    /// `TemplateItem.Properties.SizeReduceRight` (`TemplateItem.cs:586`).
    pub size_reduce_right: Option<i32>,
    /// `TemplateItem.Properties.WeapFireType`, wire name `weapFireType` (`TemplateItem.cs:746`).
    /// A `HashSet<string>` in C#; a `Vec` here because it is drawn from by index and a set
    /// deserialized from a JSON array keeps that array's order in C# too.
    pub weap_fire_type: Option<Vec<String>>,
    /// `TemplateItem.Properties.MaxHpResource` (`TemplateItem.cs:1205`).
    pub max_hp_resource: Option<i32>,
    /// `TemplateItem.Properties.MaxResource` (`TemplateItem.cs:330`).
    pub max_resource: Option<i32>,
    /// `TemplateItem.Properties.FoodUseTime`, wire name `foodUseTime` (`TemplateItem.cs:1092`).
    pub food_use_time: Option<f64>,
    /// `TemplateItem.Properties.FaceShieldComponent` (`TemplateItem.cs:987`).
    pub face_shield_component: Option<bool>,
    /// The seven `Blocks*` props `TemplateItem.Blocks` (`TemplateItem.cs:48-63`) is built from,
    /// plus `BlocksArmorVest`, which that dictionary pointedly omits.
    pub blocks_earpiece: Option<bool>,
    pub blocks_eyewear: Option<bool>,
    pub blocks_face_cover: Option<bool>,
    pub blocks_headwear: Option<bool>,
    pub blocks_folding: Option<bool>,
    pub blocks_collapsible: Option<bool>,
    /// Wire name `blockLeftStance` — the C# prop is `BlockLeftStance`, not `Blocks…`
    /// (`TemplateItem.cs:767`).
    pub block_left_stance: Option<bool>,
    /// `TemplateItem.Properties.BlocksArmorVest` (`TemplateItem.cs:604`).
    pub blocks_armor_vest: Option<bool>,
    /// `TemplateItem.Properties.Grids` (`TemplateItem.cs:339`) in full. [`ItemView::grid_cells_h`]
    /// / [`ItemView::grid_cells_v`] flatten the *first* grid only, which is all the loot port needs;
    /// a bot container is walked grid by grid, so it needs every one of them.
    pub grids: Option<Vec<GridView>>,

    // -- Added for the ragfair port (`ragfair::offer_generator`, `ragfair::server_helper`).
    /// `TemplateItem.Properties.Durability` — `AddMissingConditions` (`RagfairOfferGenerator.cs:838`)
    /// decides "is repairable" on its presence and "> 0" on its value.
    pub durability: Option<f64>,
    /// `TemplateItem.Properties.MaximumNumberOfUsage` — key uses (`:740/:743`).
    pub maximum_number_of_usage: Option<i32>,
    /// `TemplateItem.Properties.MaxRepairResource` — repair kit uses (`:759/:842`).
    pub max_repair_resource: Option<f64>,
    /// `TemplateItem.Properties.CanSellOnRagfair` — the BSG flea blacklist
    /// (`RagfairServerHelper.cs:53`). The custom-blacklist arm at `:61` writes it back to `false`;
    /// that write leaves Rust as `rejectedCanSellTemplates` for the C# side to replay.
    pub can_sell_on_ragfair: Option<bool>,
}

/// `Models/Eft/Common/Tables/TemplateItem.cs:1641-1681` — `Grid` plus its `_props`, flattened.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridView {
    /// `Grid._name` — becomes the placed item's `slotId`.
    pub name: Option<String>,
    pub cells_h: Option<i32>,
    pub cells_v: Option<i32>,
    pub filters: Option<Vec<GridFilterView>>,
}

/// `Models/Eft/Common/Tables/TemplateItem.cs:1683-1693`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridFilterView {
    pub filter: Option<Vec<String>>,
    pub excluded_filter: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotView {
    pub name: Option<String>,
    pub required: Option<bool>,
    pub filter: Option<Vec<String>>,
    /// `Slot.Properties.Filters.First().Plate` (`TemplateItem.cs`), flattened alongside `filter` the
    /// way the rest of this view flattens `_props` — `TemplateItemExtensions.GetDefaultPlateTpl`
    /// reads it, and nothing else in the port does.
    pub plate: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetView {
    pub items: Vec<Item>,
    /// `Preset.Id` — debug message argument (`LootGenerator.cs:400`).
    pub id: Option<String>,
    /// `Preset.Name` — debug message argument (`LootGenerator.cs:633`).
    pub name: Option<String>,
    /// `Preset.Encyclopedia` — root tpl resolution (`LootGenerator.cs:396-407`).
    pub encyclopedia: Option<String>,
}

/// Every config value the generator reads, resolved for one location: by the C# caller on the
/// override arm (`LocationLootGenerator.BuildConfigView`), by
/// `location_loot_generator::resolve_loot_config_view` off the resident `spt-location` stem
/// otherwise.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LootConfigView {
    pub container_randomisation_enabled: bool,
    /// `Maps.ContainsKey(locationId)`.
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
    /// `MultiplierForLocation(StaticLootMultiplier, locationId)`.
    pub static_loot_multiplier: f64,
    /// `MultiplierForLocation(LooseLootMultiplier, locationId)`.
    pub loose_loot_multiplier: f64,
    /// `EquipmentLootSettings`.
    pub mod_spawn_chance_percent: HashMap<String, f64>,
    /// `LooseLootBlacklist[locationId]`, or empty when the map has no entry.
    pub loose_loot_blacklist: HashSet<String>,
}

/// The three `SeasonalEventService` answers the generator reads. `christmasContainerIds` left for
/// the views side in Phase 4 — it is config, not service, state.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonalView {
    pub seasonal_event_active: bool,
    pub christmas_event_enabled: bool,
    pub inactive_seasonal_items: HashSet<String>,
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
    pub epoch: u64,
    pub views_override: Option<Box<LootViewsWire>>,
    pub varying: LootVarying,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicLootRequest {
    pub epoch: u64,
    pub views_override: Option<Box<LootViewsWire>>,
    pub varying: DynamicLootVarying,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicLootVarying {
    #[serde(flatten)]
    pub common: LootVarying,
    /// The raw splice or the LazyLoad-typed loot — per-call in both arms (flip #4 decision).
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

#[derive(Debug)]
pub struct Diagnostic {
    /// The `typeof(T).FullName` category this line logs under — the C# class the emitting Rust
    /// file ports. Set by each file's helper from its `CATEGORY` const.
    pub category: &'static str,
    /// One of [`DEBUG`], [`WARNING`], [`ERROR`], [`SUCCESS`].
    pub level: String,
    pub locale_key: Option<String>,
    /// Object; rendered by `diag::localise`.
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicLootResult {
    pub spawnpoints: Vec<SpawnpointTemplate>,
    pub tracked_counts: BTreeMap<String, i32>,
}

// ---------------------------------------------------------------------------
// Reward-loot envelopes (`Generators/Loot/LootGenerator.cs`)
// ---------------------------------------------------------------------------

/// The distrust fallback for the four reward exports: the C#-built database half plus the
/// `ItemConfig` sets the resident arm reads off the configs root, present iff the caller is
/// ineligible for residency. `presetsByTpl` rides only on sealed-case sends and `presetTpls` only
/// on reward-container sends, mirroring the old per-envelope members.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardViewsWire {
    /// `IndexMap`, not `HashMap`: `LootGenerator.cs:56,244,567` filter `templateTable.Items.Values`
    /// into a pool and then draw an index out of it, so the iteration order is observable.
    pub items_view: IndexMap<String, ItemView>,
    /// `presetHelper.GetDefaultPresets().Values` — order preserved, the weapon/armor preset
    /// draws index into the filtered result. Only random loot reads it; the other envelopes
    /// send `[]`.
    #[serde(default)]
    pub default_presets: Vec<PresetView>,
    /// Per call site: `GetDefaultPresetsByTplKey()` on forced sends, `GetDefaultPresetByTpl()`
    /// on sealed/container sends, unread (`{}`) on random sends.
    #[serde(default)]
    pub default_presets_by_tpl: IndexMap<String, PresetView>,
    /// Sealed only: `presetHelper.GetPresets(tpl)` per weapon tpl — inner order is drawn from.
    pub presets_by_tpl: Option<IndexMap<String, Vec<PresetView>>>,
    /// Container only: `presetHelper.HasPreset(tpl)` as a set.
    pub preset_tpls: Option<HashSet<String>>,
    // The `ItemConfig` sets the resident arm reads off the configs root's `spt-item` stem instead
    // (flip #8). Same wire names and values as the varying half carried, and the same types
    // [`crate::db::models::ItemConfigLift`] carries, so both arms hand the generator one shape.
    /// The blacklist [`get_item_reward_pool`](crate::loot::loot_generator) unions in:
    /// `itemFilterService.GetBlacklistedItems()` (`LootGenerator.cs:425`), which hands back
    /// `itemConfig.Blacklist` itself (`ItemFilterService.cs:38-41`). A different object from
    /// [`RewardLootVarying::global_blacklist`]'s cache, so a mod's runtime additions reach the
    /// sealed filters and not this one — the two are equal on an unmodded server.
    pub config_blacklist: HashSet<String>,
    /// `itemFilterService.GetItemRewardBlacklist()` (`LootGenerator.cs:221`) —
    /// `ItemConfig.RewardItemBlacklist` verbatim. An `IndexSet`, unlike
    /// [`Self::config_blacklist`], because that is the shape the lift carries; the reward pool only
    /// extends a `HashSet` out of it, so the order it keeps is unread.
    pub reward_item_blacklist: IndexSet<String>,
    /// `itemFilterService.GetItemRewardBaseTypeBlacklist()` (`LootGenerator.cs:224`) —
    /// `ItemConfig.RewardItemTypeBlacklist`. Collected into a `Vec<&str>` the base-class walk only
    /// answers a bool off, so an `IndexSet` for the same reason.
    pub reward_base_type_blacklist: IndexSet<String>,
    /// `itemFilterService.GetBossItems()` (`LootGenerator.cs:235`), an `IndexSet` for the same
    /// reason — the reward pool extends a `HashSet` from it and the sealed filter asks it for
    /// membership.
    pub boss_items: IndexSet<String>,
}

/// The per-call half every reward export carries: the service-backed blacklists/sets (Phases-2/4
/// carve-out — a mod can extend them at runtime) and the test seed. The four config-backed sets
/// that used to ride beside them live on [`RewardViewsWire`] now.
///
/// Both sets are `HashSet` because every use is a membership test — neither is ever iterated to
/// make a draw, so a randomised order cannot reach the RNG.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardLootVarying {
    /// The blacklist the sealed-container filters test against: `itemFilterService.IsItemBlacklisted`
    /// (`LootGenerator.cs:822,880`), which reads `ItemBlacklistCache` — a *copy* of
    /// `itemConfig.Blacklist` that `AddItemToBlacklistCache` extends at runtime
    /// (`ItemFilterService.cs:13,94-102`). Filled from `GetItemBlacklistCache()`. Stays varying
    /// where [`RewardViewsWire::config_blacklist`] did not, precisely because it is that mutable
    /// cache and not the config value.
    pub global_blacklist: HashSet<String>,
    /// `seasonalEventService.GetInactiveSeasonalEventItems()` (`LootGenerator.cs:240`) — a
    /// service's own cache, so it rides every send.
    pub inactive_seasonal_items: HashSet<String>,
    /// Test-only, as [`LootVarying::test_seed`].
    pub test_seed: Option<u64>,
}

/// `Models/Common/MinMax.cs` closed over `int`. Neither member is `required` in C#, so a missing
/// one is not a parse error there either.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinMaxI32 {
    pub min: Option<i32>,
    pub max: Option<i32>,
}

/// `Models/Spt/Services/LootRequest.cs`. `UseForcedLoot`/`ForcedLoot` are absent by design: the C#
/// caller branches on them before it reaches this crate, and forced loot arrives on its own
/// envelope ([`CreateForcedLootRequest`]).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LootRequestView {
    pub weapon_preset_count: Option<MinMaxI32>,
    pub armor_preset_count: Option<MinMaxI32>,
    pub item_count: Option<MinMaxI32>,
    pub weapon_crate_count: Option<MinMaxI32>,
    pub item_blacklist: Option<HashSet<String>>,
    pub item_type_whitelist: Option<HashSet<String>>,
    pub item_limits: Option<IndexMap<String, i32>>,
    pub item_stack_limits: Option<IndexMap<String, MinMaxI32>>,
    pub armor_level_whitelist: Option<HashSet<i32>>,
    pub allow_boss_items: Option<bool>,
    pub use_reward_item_blacklist: Option<bool>,
    pub block_seasonal_items_out_of_season: Option<bool>,
}

/// `SealedAirdropContainerSettings` (`Models/Spt/Config/InventoryConfig.cs:54-79`). `FoundInRaid`
/// is absent — the C# caller applies it after the native call.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedContainerSettingsView {
    /// Ordered: `get_weighted_value` draws over it.
    pub weapon_reward_weight: IndexMap<String, f64>,
    pub default_presets_only: bool,
    /// Ordered: iterated with a draw per entry (`LootGenerator.cs:615`).
    pub weapon_mod_reward_limits: IndexMap<String, MinMaxI32>,
    /// Ordered: iterated with a draw per entry (`LootGenerator.cs:520`).
    pub reward_type_limits: IndexMap<String, MinMaxI32>,
    pub ammo_box_whitelist: Vec<String>,
    pub allow_boss_items: bool,
}

/// `RewardDetails` (`Models/Spt/Config/InventoryConfig.cs:36-52`). `FoundInRaid`/`_type` are absent
/// for the same reason as on [`SealedContainerSettingsView`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardDetailsView {
    pub reward_count: i32,
    /// Ordered: the weighted pick walks it.
    pub reward_tpl_pool: Option<IndexMap<String, f64>>,
    pub reward_type_pool: Option<HashSet<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRandomLootRequest {
    pub epoch: u64,
    pub views_override: Option<Box<RewardViewsWire>>,
    pub varying: CreateRandomLootVarying,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRandomLootVarying {
    #[serde(flatten)]
    pub common: RewardLootVarying,
    pub loot_request: LootRequestView,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateForcedLootRequest {
    pub epoch: u64,
    pub views_override: Option<Box<RewardViewsWire>>,
    pub varying: CreateForcedLootVarying,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateForcedLootVarying {
    #[serde(flatten)]
    pub common: RewardLootVarying,
    /// Ordered: `LootGenerator.cs:155` draws a count per entry as it walks this map.
    pub forced_loot: IndexMap<String, MinMaxI32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedWeaponCaseRequest {
    pub epoch: u64,
    pub views_override: Option<Box<RewardViewsWire>>,
    pub varying: SealedWeaponCaseVarying,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedWeaponCaseVarying {
    #[serde(flatten)]
    pub common: RewardLootVarying,
    pub container_settings: SealedContainerSettingsView,
    /// `ragfairLinkedItemService.GetLinkedDbItems(tpl)` per `weaponRewardWeight` key
    /// (`LootGenerator.cs:498`) — the inner `Vec` order is drawn from. Service-backed (the
    /// linked-item cache is mod-extendable at runtime), so it rides every sealed send.
    pub linked_items: IndexMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomLootContainerRequest {
    pub epoch: u64,
    pub views_override: Option<Box<RewardViewsWire>>,
    pub varying: RandomLootContainerVarying,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomLootContainerVarying {
    #[serde(flatten)]
    pub common: RewardLootVarying,
    pub reward_details: RewardDetailsView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardLootResult {
    pub items: Vec<Vec<Item>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every required `LootVarying` member, for splicing into an envelope test literal. `testSeed`
    /// is deliberately absent — its omission is what exercises the missing-field → `None` path.
    const VARYING_JSON: &str = r#"
        "locationId":"bigmap",
        "moneyTpls":["5449016a4bdc2d6f028b456f"],
        "staticAmmoDist":{"Caliber762x39":[{"tpl":"dddddddddddddddddddddddd","relativeProbability":5}]},
        "seasonal":{"seasonalEventActive":false,"christmasEventEnabled":false,
            "inactiveSeasonalItems":[]},
        "lootableItemBlacklist":[],
        "counter":{"maxCounts":{"ffffffffffffffffffffffff":2},"trackedCounts":{}}
    "#;

    /// The four `LootViewsWire` members every override carries, for splicing beside the per-send
    /// statics members.
    const VIEWS_JSON: &str = r#"
        "itemsView":{"aaaaaaaaaaaaaaaaaaaaaaaa":{"parent":"bbbbbbbbbbbbbbbbbbbbbbbb","width":2,"height":1,
            "slots":[{"name":"mod_magazine","required":false,"filter":["cccccccccccccccccccccccc"]}]}},
        "defaultPresets":{"p1":{"items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb"}]}},
        "config":{"containerRandomisationEnabled":true,"locationInRandomisationMaps":true,
            "containerTypesToNotRandomise":["eeeeeeeeeeeeeeeeeeeeeeee"],
            "containerGroupMinSizeMultiplier":0.8,"containerGroupMaxSizeMultiplier":1.2,
            "allowDuplicateItemsInStaticContainers":false,"tplsToStripChildItemsFrom":[],
            "fitLootIntoContainerAttempts":3,"magazineLootHasAmmoChancePercent":25,
            "staticMagazineLootHasAmmoChancePercent":50,"minFillLooseMagazinePercent":30,
            "minFillStaticMagazinePercent":30,"staticLootMultiplier":1.5,"looseLootMultiplier":1.1,
            "modSpawnChancePercent":{"mod_scope":25},"looseLootBlacklist":[]},
        "christmasContainerIds":[]
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
            (r#"{"StackObjectsCount":"1,000"}"#, Some(1000.0)),
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
            r#"{{"epoch":7,"viewsOverride":{{{VIEWS_JSON},
            "staticWeapons":[{{"Id":"w1","Root":"aaaaaaaaaaaaaaaaaaaaaaaa","Items":[]}}],
            "staticContainers":[{{"probability":0.35,"template":{{"Id":"c1","IsContainer":true}}}}],
            "staticForced":[{{"containerId":"c1","itemTpl":"dddddddddddddddddddddddd"}}],
            "staticLootDist":{{"eeeeeeeeeeeeeeeeeeeeeeee":{{
                "itemcountDistribution":[{{"count":1,"relativeProbability":10}}],
                "itemDistribution":[{{"tpl":"dddddddddddddddddddddddd","relativeProbability":10}}]}}}},
            "statics":{{"containersGroups":{{"g1":{{"minContainers":1,"maxContainers":3}}}},
                "containers":{{"c1":{{"groupId":"g1"}}}}}}}},
            "varying":{{{VARYING_JSON}}}}}"#
        );
        let parsed: StaticContainersRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.epoch, 7);
        assert_eq!(parsed.varying.location_id, "bigmap");
        assert_eq!(
            parsed.varying.counter.max_counts["ffffffffffffffffffffffff"],
            2
        );
        let views = parsed.views_override.unwrap();
        assert_eq!(views.config.static_loot_multiplier, 1.5);
        assert_eq!(views.items_view["aaaaaaaaaaaaaaaaaaaaaaaa"].width, Some(2));
        assert_eq!(views.static_weapons.unwrap()[0].id.as_deref(), Some("w1"));
        assert_eq!(views.static_containers.unwrap()[0].probability, Some(0.35));
        assert_eq!(views.static_forced.unwrap()[0].container_id, "c1");
        assert_eq!(
            views.static_loot_dist.as_ref().unwrap()["eeeeeeeeeeeeeeeeeeeeeeee"]
                .item_count_distribution
                .as_ref()
                .unwrap()[0]
                .count,
            Some(1)
        );
        assert_eq!(
            views.statics.unwrap().containers_groups.unwrap()["g1"].max_containers,
            Some(3)
        );
    }

    #[test]
    fn dynamic_loot_request_deserializes() {
        // No viewsOverride: the resident arm's send is just `{epoch, varying}`.
        let json = format!(
            r#"{{"epoch":7,"varying":{{{VARYING_JSON},
            "looseLoot":{{"spawnpointCount":{{"mean":12.5,"std":2}},
                "spawnpointsForced":[{{"locationId":"f1","probability":1,"template":{{"Id":"t1"}}}}],
                "spawnpoints":[{{"locationId":"s1","probability":0.5,"template":{{"Id":"t2"}},
                    "itemDistribution":[{{"composedKey":{{"key":"ck1"}},"relativeProbability":4}}]}}]}}}}}}"#
        );
        let parsed: DynamicLootRequest = serde_json::from_str(&json).unwrap();

        assert!(parsed.views_override.is_none());
        assert_eq!(
            parsed.varying.common.money_tpls,
            vec!["5449016a4bdc2d6f028b456f"]
        );
        let loose = &parsed.varying.loose_loot;
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

    /// Every `RewardViewsWire` member of an override reward send, for splicing into a test
    /// literal's `viewsOverride`.
    const REWARD_VIEWS_JSON: &str = r#"
        "itemsView":{"aaaaaaaaaaaaaaaaaaaaaaaa":{"parent":"bbbbbbbbbbbbbbbbbbbbbbbb","name":"event_container_airdrop",
            "type":"item","armorClass":4,"questItem":false,"width":2,"height":1}},
        "defaultPresets":[{"id":"p1","name":"ak_default","encyclopedia":"bbbbbbbbbbbbbbbbbbbbbbbb",
            "items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb"}]}],
        "defaultPresetsByTpl":{"bbbbbbbbbbbbbbbbbbbbbbbb":{"id":"p1","name":"ak_default",
            "encyclopedia":"bbbbbbbbbbbbbbbbbbbbbbbb","items":[]}},
        "configBlacklist":["cccccccccccccccccccccccc"],
        "rewardItemBlacklist":["dddddddddddddddddddddddd"],
        "rewardBaseTypeBlacklist":["eeeeeeeeeeeeeeeeeeeeeeee"],
        "bossItems":["ffffffffffffffffffffffff"]
    "#;

    /// Every required `RewardLootVarying` member, for splicing into a test literal's `varying`.
    /// `testSeed` is deliberately absent, as in [`VARYING_JSON`].
    const REWARD_VARYING_JSON: &str = r#"
        "globalBlacklist":["cccccccccccccccccccccccc"],
        "inactiveSeasonalItems":["111111111111111111111111"]
    "#;

    #[test]
    fn item_view_reward_fields_deserialize() {
        let parsed: ItemView = serde_json::from_str(
            r#"{"parent":"bbbbbbbbbbbbbbbbbbbbbbbb","name":"weapon_ak","type":"Item",
               "armorClass":6,"questItem":true}"#,
        )
        .unwrap();
        assert_eq!(parsed.name.as_deref(), Some("weapon_ak"));
        assert_eq!(parsed.item_type.as_deref(), Some("Item"));
        assert_eq!(parsed.armor_class, Some(6));
        assert_eq!(parsed.quest_item, Some(true));
    }

    /// `LootGenerator.cs:246` reads `QuestItem.GetValueOrDefault(false)` while `:571` tests
    /// `QuestItem is null`, so an explicit `false` and an absent/null value are not interchangeable.
    #[test]
    fn quest_item_keeps_null_and_false_distinct() {
        let null: ItemView = serde_json::from_str(r#"{"questItem":null}"#).unwrap();
        assert_eq!(null.quest_item, None);

        let absent: ItemView = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.quest_item, None);

        let explicit: ItemView = serde_json::from_str(r#"{"questItem":false}"#).unwrap();
        assert_eq!(explicit.quest_item, Some(false));
    }

    #[test]
    fn preset_view_reward_fields_deserialize() {
        let parsed: PresetView = serde_json::from_str(
            r#"{"id":"p1","name":"ak_default","encyclopedia":"bbbbbbbbbbbbbbbbbbbbbbbb","items":[]}"#,
        )
        .unwrap();
        assert_eq!(parsed.id.as_deref(), Some("p1"));
        assert_eq!(parsed.name.as_deref(), Some("ak_default"));
        assert_eq!(
            parsed.encyclopedia.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
    }

    #[test]
    fn create_random_loot_request_deserializes() {
        let json = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON}}},
            "varying":{{{REWARD_VARYING_JSON},
            "lootRequest":{{"weaponPresetCount":{{"min":1,"max":2}},"armorPresetCount":{{"min":0,"max":1}},
                "itemCount":{{"min":3,"max":5}},"weaponCrateCount":{{"min":0,"max":2}},
                "itemBlacklist":["222222222222222222222222"],"itemTypeWhitelist":["333333333333333333333333"],
                "itemLimits":{{"444444444444444444444444":2}},
                "itemStackLimits":{{"555555555555555555555555":{{"min":1,"max":4}}}},
                "armorLevelWhitelist":[4,5,6],"allowBossItems":false,"useRewardItemBlacklist":true,
                "blockSeasonalItemsOutOfSeason":true}}}}}}"#
        );
        let parsed: CreateRandomLootRequest = serde_json::from_str(&json).unwrap();

        let views = parsed.views_override.as_ref().unwrap();
        assert_eq!(views.default_presets.len(), 1);
        assert_eq!(views.default_presets[0].id.as_deref(), Some("p1"));
        assert!(views.boss_items.contains("ffffffffffffffffffffffff"));
        assert!(
            parsed
                .varying
                .common
                .global_blacklist
                .contains("cccccccccccccccccccccccc")
        );
        assert!(parsed.varying.common.test_seed.is_none());
        assert_eq!(
            views.items_view["aaaaaaaaaaaaaaaaaaaaaaaa"].armor_class,
            Some(4)
        );

        let request = &parsed.varying.loot_request;
        assert_eq!(request.item_count.as_ref().unwrap().max, Some(5));
        assert_eq!(
            request.item_limits.as_ref().unwrap()["444444444444444444444444"],
            2
        );
        assert_eq!(
            request.item_stack_limits.as_ref().unwrap()["555555555555555555555555"].min,
            Some(1)
        );
        assert!(request.armor_level_whitelist.as_ref().unwrap().contains(&6));
        assert_eq!(request.allow_boss_items, Some(false));
        assert_eq!(request.use_reward_item_blacklist, Some(true));
        assert_eq!(request.block_seasonal_items_out_of_season, Some(true));
    }

    #[test]
    fn create_forced_loot_request_deserializes() {
        let json = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON}}},
            "varying":{{{REWARD_VARYING_JSON},
            "forcedLoot":{{"666666666666666666666666":{{"min":1,"max":3}},
                "777777777777777777777777":{{"min":2,"max":2}}}}}}}}"#
        );
        let parsed: CreateForcedLootRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed
                .views_override
                .as_ref()
                .unwrap()
                .default_presets_by_tpl["bbbbbbbbbbbbbbbbbbbbbbbb"]
                .name
                .as_deref(),
            Some("ak_default")
        );
        // The forced-loot map is walked in order, drawing a count per entry.
        assert_eq!(
            parsed.varying.forced_loot.keys().collect::<Vec<_>>(),
            vec!["666666666666666666666666", "777777777777777777777777"]
        );
        assert_eq!(
            parsed.varying.forced_loot["666666666666666666666666"].max,
            Some(3)
        );
    }

    #[test]
    fn sealed_weapon_case_request_deserializes() {
        let json = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON},
            "presetsByTpl":{{"888888888888888888888888":[{{"id":"p1","items":[]}},{{"id":"p2","items":[]}}]}}}},
            "varying":{{{REWARD_VARYING_JSON},
            "containerSettings":{{"weaponRewardWeight":{{"888888888888888888888888":5,"999999999999999999999999":1}},
                "defaultPresetsOnly":false,
                "weaponModRewardLimits":{{"aaaaaaaaaaaaaaaaaaaaaaa1":{{"min":0,"max":2}}}},
                "rewardTypeLimits":{{"aaaaaaaaaaaaaaaaaaaaaaa2":{{"min":1,"max":1}}}},
                "ammoBoxWhitelist":["aaaaaaaaaaaaaaaaaaaaaaa3"],"allowBossItems":false}},
            "linkedItems":{{"888888888888888888888888":["aaaaaaaaaaaaaaaaaaaaaaa4","aaaaaaaaaaaaaaaaaaaaaaa5"]}}}}}}"#
        );
        let parsed: SealedWeaponCaseRequest = serde_json::from_str(&json).unwrap();

        let settings = &parsed.varying.container_settings;
        // Weight order feeds `get_weighted_value`; limit maps are iterated for draws.
        assert_eq!(
            settings.weapon_reward_weight.keys().collect::<Vec<_>>(),
            vec!["888888888888888888888888", "999999999999999999999999"]
        );
        assert_eq!(
            settings.weapon_reward_weight["888888888888888888888888"],
            5.0
        );
        assert!(!settings.default_presets_only);
        assert!(!settings.allow_boss_items);
        assert_eq!(
            settings.weapon_mod_reward_limits["aaaaaaaaaaaaaaaaaaaaaaa1"].max,
            Some(2)
        );
        assert_eq!(
            settings.reward_type_limits["aaaaaaaaaaaaaaaaaaaaaaa2"].min,
            Some(1)
        );
        assert_eq!(
            settings.ammo_box_whitelist,
            vec!["aaaaaaaaaaaaaaaaaaaaaaa3"]
        );

        let views = parsed.views_override.as_ref().unwrap();
        let presets = &views.presets_by_tpl.as_ref().unwrap()["888888888888888888888888"];
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[1].id.as_deref(), Some("p2"));
        assert_eq!(
            parsed.varying.linked_items["888888888888888888888888"],
            vec!["aaaaaaaaaaaaaaaaaaaaaaa4", "aaaaaaaaaaaaaaaaaaaaaaa5"]
        );
    }

    #[test]
    fn random_loot_container_request_deserializes() {
        let json = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON},
            "presetTpls":["bbbbbbbbbbbbbbbbbbbbbbbb"]}},
            "varying":{{{REWARD_VARYING_JSON},
            "rewardDetails":{{"rewardCount":2,
                "rewardTplPool":{{"aaaaaaaaaaaaaaaaaaaaaaa6":3.5,"aaaaaaaaaaaaaaaaaaaaaaa7":1}},
                "rewardTypePool":["aaaaaaaaaaaaaaaaaaaaaaa8"]}},
            "testSeed":42}}}}"#
        );
        let parsed: RandomLootContainerRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.varying.common.test_seed, Some(42));
        assert!(
            parsed
                .views_override
                .as_ref()
                .unwrap()
                .preset_tpls
                .as_ref()
                .unwrap()
                .contains("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(parsed.varying.reward_details.reward_count, 2);
        let pool = parsed
            .varying
            .reward_details
            .reward_tpl_pool
            .as_ref()
            .unwrap();
        assert_eq!(
            pool.keys().collect::<Vec<_>>(),
            vec!["aaaaaaaaaaaaaaaaaaaaaaa6", "aaaaaaaaaaaaaaaaaaaaaaa7"]
        );
        assert_eq!(pool["aaaaaaaaaaaaaaaaaaaaaaa6"], 3.5);
        assert!(
            parsed
                .varying
                .reward_details
                .reward_type_pool
                .as_ref()
                .unwrap()
                .contains("aaaaaaaaaaaaaaaaaaaaaaa8")
        );

        // Both pools are optional in C# (`InventoryConfig.cs:47,50`).
        let json = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON},"presetTpls":[]}},
            "varying":{{{REWARD_VARYING_JSON},"rewardDetails":{{"rewardCount":0}}}}}}"#
        );
        let parsed: RandomLootContainerRequest = serde_json::from_str(&json).unwrap();
        assert!(parsed.varying.reward_details.reward_tpl_pool.is_none());
        assert!(parsed.varying.reward_details.reward_type_pool.is_none());
    }

    #[test]
    fn reward_loot_result_serializes_with_camel_case_keys() {
        let out = serde_json::to_value(RewardLootResult {
            items: vec![vec![Item {
                id: "aaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
                template: "bbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
                ..Default::default()
            }]],
        })
        .unwrap();

        assert_eq!(
            out.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["items"]
        );
        assert_eq!(out["items"][0][0]["_id"], "aaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn results_serialize_with_camel_case_keys() {
        let out = serde_json::to_value(StaticContainersResult {
            spawnpoints: vec![],
            tracked_counts: BTreeMap::from([("tpl".to_owned(), 1)]),
            static_loot_item_count: 4,
            static_container_count: 2,
        })
        .unwrap();

        assert_eq!(out["staticLootItemCount"], 4);
        assert_eq!(out["staticContainerCount"], 2);
        assert_eq!(out["trackedCounts"]["tpl"], 1);
    }
}
