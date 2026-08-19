//! Wire models of the resident database (spec § The epoch protocol).
//!
//! Task-1 shape rule: every root is a `#[serde(flatten)]` superset map. Typed fields are lifted
//! out of `extra` only when Rust-side derivation reads them (`ragfair::views::derive` today, the
//! repeatable-quest flip's inputs lifted ahead of it) —
//! the flatten map is what keeps the root full-fidelity regardless. Wire names are pinned to the
//! C# `JsonPropertyName` of the record each type mirrors (`Models/Spt/Tables/TemplateTable.cs`,
//! `TradersTable.cs`, `GlobalTable.cs` and the member types they reach). Every lifted container
//! carries `#[serde(default)]`: a partial or junk root (the store tests publish `{"a":1}`)
//! deserializes with empty containers and derivation stays total over it.

use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use serde::Deserialize;
use serde_json::Value;

use crate::bot::repair_service::MinMax;
use crate::loot::models::{
    Item, SpawnpointTemplate, StaticContainer, StaticContainerData, StaticForced, StaticLootDetails,
};
use crate::quest::models::{LevelledItemFilter, RepeatableTemplates};

/// `{"schema":1,"roots":{...}}` — the envelope `DbPayloadProjection` (C#) writes.
#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    pub schema: u32,
    pub roots: PublishRoots,
}

/// Every root optional: an absent root keeps the currently-resident one. Unknown root names are
/// a parse error (`deny_unknown_fields`), surfacing as `STATUS_BAD_ARGS` — C# and Rust ship in
/// lockstep, so a typo should fail loudly, not silently install nothing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRoots {
    pub templates: Option<TemplatesRoot>,
    pub traders: Option<TradersRoot>,
    pub globals: Option<GlobalsRoot>,
    pub locations: Option<LocationsRoot>,
    pub hideout: Option<HideoutRoot>,
}

/// Hideout root: `production.scavRecipes` only (flip #5) — locations-root
/// partial-projection precedent. Wire names pin to the C# `JsonPropertyName`s
/// (HideoutTable.cs / HideoutProduction.cs): note capitalized Common/Rare/Superrare.
#[derive(Debug, Default, Deserialize)]
pub struct HideoutRoot {
    #[serde(default)]
    pub production: HideoutProductionRoot,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct HideoutProductionRoot {
    #[serde(default, rename = "scavRecipes")]
    pub scav_recipes: Vec<DbScavRecipe>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DbScavRecipe {
    #[serde(default, rename = "_id")]
    pub id: String,
    #[serde(default, rename = "endProducts")]
    pub end_products: Option<DbEndProducts>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DbEndProducts {
    #[serde(default, rename = "Common")]
    pub common: Option<MinMax<i32>>,
    #[serde(default, rename = "Rare")]
    pub rare: Option<MinMax<i32>>,
    #[serde(default, rename = "Superrare")]
    pub superrare: Option<MinMax<i32>>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/TemplateTable.cs` — only the members the ragfair view derivation and the
/// repeatable-quest flip read are typed; everything else rides in `extra`.
#[derive(Debug, Default, Deserialize)]
pub struct TemplatesRoot {
    /// `TemplateTable.Items` (`TemplateTable.cs:16-17`).
    #[serde(default)]
    pub items: IndexMap<String, TemplateItem>,
    /// `TemplateTable.Handbook` (`TemplateTable.cs:28-29`).
    #[serde(default)]
    pub handbook: HandbookBase,
    /// `TemplateTable.Prices` (`TemplateTable.cs:46-47`) — source order is contract, it is what
    /// `GetFleaPricesAsArray` draws an index into.
    #[serde(default)]
    pub prices: IndexMap<String, f64>,
    /// `TemplateTable.RepeatableQuests` (`TemplateTable.cs:25-26`) — what the repeatable-quest
    /// flip reads.
    #[serde(rename = "repeatableQuests")]
    pub repeatable_quests: Option<RepeatableQuestsWire>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/Tables/RepeatableQuests.cs:21-34` `RepeatableQuestDatabase` — only the
/// members the repeatable-quest generators read (`Templates`, `Data`) are typed; `rewards` and
/// `samples` ride in `extra`.
#[derive(Debug, Deserialize)]
pub struct RepeatableQuestsWire {
    pub templates: Option<RepeatableTemplates>,
    pub data: Option<RepeatableQuestsData>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `RepeatableQuests.cs:138-141` `Options`, the `Data` member's type.
#[derive(Debug, Deserialize)]
pub struct RepeatableQuestsData {
    #[serde(rename = "Completion")]
    pub completion: Option<CompletionFilter>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `RepeatableQuests.cs:144-150` `CompletionFilter`. C# declares both lists nullable; absent and
/// null collapse to empty here, the same branch the C# takes for both (`quest::models` documents
/// the same collapse on its slice members).
#[derive(Debug, Deserialize)]
pub struct CompletionFilter {
    #[serde(rename = "itemsWhitelist", default)]
    pub items_whitelist: Vec<LevelledItemFilter>,
    #[serde(rename = "itemsBlacklist", default)]
    pub items_blacklist: Vec<LevelledItemFilter>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/TradersTable.cs:6` — the root *is* `Dictionary<MongoId, Trader>`, so the
/// flatten map replaces a named `extra` wholesale: every key is a trader id. [`TraderEntry`]
/// keeps the parse total over values no C# `Trader` could have loaded (the store tests publish
/// `{"b":2}`); those ride through as raw JSON, exactly as full-fidelity as before the lift.
#[derive(Debug, Default, Deserialize)]
pub struct TradersRoot {
    #[serde(flatten)]
    pub traders: IndexMap<String, TraderEntry>,
}

/// One value of the traders dictionary root. Untagged: any JSON object parses as [`Trader`]
/// (whose members are all optional), anything else falls through to raw JSON.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TraderEntry {
    Trader(Box<Trader>),
    /// Not trader-shaped. `ragfair::views::derive` skips it — the C# `TradersTable` could never
    /// have deserialized it in the first place.
    Other(Value),
}

/// `Models/Eft/Common/Tables/Trader.cs:9-28` — only `base` is typed.
#[derive(Debug, Deserialize)]
pub struct Trader {
    /// `required` in C#; `Option` here so the parse stays total. A base-less trader prices like
    /// one with no loyalty levels — unobservable, C# throws at database load before it could
    /// ever publish one.
    #[serde(rename = "base")]
    pub base: Option<TraderBase>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Trader.cs:30-180` `TraderBase` — only `loyaltyLevels` is typed.
#[derive(Debug, Deserialize)]
pub struct TraderBase {
    #[serde(rename = "loyaltyLevels")]
    pub loyalty_levels: Option<Vec<TraderLoyaltyLevel>>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Trader.cs:182-…` `TraderLoyaltyLevel` — only `buy_price_coef` is read.
#[derive(Debug, Deserialize)]
pub struct TraderLoyaltyLevel {
    #[serde(rename = "buy_price_coef")]
    pub buy_price_coef: Option<f64>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/GlobalTable.cs:10-26` — only `ItemPresets` and the `config` lift are typed;
/// everything else rides [`Self::extra`].
#[derive(Debug, Default, Deserialize)]
pub struct GlobalsRoot {
    /// `GlobalTable.ItemPresets` (`GlobalTable.cs:24-25`), keyed by preset id — that key domain
    /// (the map's keys, not each preset's `_id`) is what `PresetHelper.IsPreset`/`GetPreset`
    /// answer from.
    #[serde(rename = "ItemPresets", default)]
    pub item_presets: IndexMap<String, Preset>,
    /// `GlobalTable.Configuration` (`GlobalTable.cs:12-13`) — see [`GlobalsConfigLift`].
    #[serde(default, rename = "config")]
    pub config: GlobalsConfigLift,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// Globals `config` lift: `exp.level.exp_table` only (flip #6) — everything else
/// rides `extra`. Wire names pin to GlobalTable.cs (`config`/`exp`/`level`/`exp_table`,
/// entries `{"exp": n}` — GlobalTable.cs:12, :299, :1166, :1290, :1311).
#[derive(Debug, Default, Deserialize)]
pub struct GlobalsConfigLift {
    #[serde(default)]
    pub exp: GlobalsExpLift,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct GlobalsExpLift {
    #[serde(default)]
    pub level: GlobalsExpLevelLift,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct GlobalsExpLevelLift {
    #[serde(default, rename = "exp_table")]
    pub exp_table: Vec<ExpTableEntry>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ExpTableEntry {
    #[serde(default)]
    pub exp: i32,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/GlobalTable.cs:4397-4422` `Preset`.
#[derive(Debug, Deserialize)]
pub struct Preset {
    #[serde(rename = "_id", default)]
    pub id: String,
    #[serde(rename = "_name")]
    pub name: Option<String>,
    /// C# declares `List<Item>` non-nullable but not `required` — an items-less preset is the
    /// `NullReferenceException` `PresetController.Initialize` (`PresetController.cs:33-34`)
    /// would have thrown; `ragfair::views::derive` turns it into a publish-aborting error.
    #[serde(rename = "_items", default)]
    pub items: Vec<Item>,
    /// Only default presets carry `_encyclopedia`.
    #[serde(rename = "_encyclopedia")]
    pub encyclopedia: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/LocationTable.cs` — the root is the serialized table: one key per map
/// (`bigmap`, `factory4_day`, …) plus the UI-linkage `base` (`LocationTable.cs:117-118`), so the
/// flatten map replaces a named `extra` wholesale, the same shape as [`TradersRoot`].
/// `GetDictionary()` re-keys by C# property name at read time; the wire keys here are the
/// `JsonPropertyName`s `GetLocation` falls back to.
#[derive(Debug, Default, Deserialize)]
pub struct LocationsRoot {
    #[serde(flatten)]
    pub locations: IndexMap<String, LocationEntry>,
}

/// One value of the locations dictionary root — `Models/Eft/Common/Location.cs`. Typed members:
/// the two the repeatable-quest projection reads, plus the three statics the loot flip reads
/// (published as each `LazyLoad.Value`, transformers applied — flip #4). `looseLoot` is
/// deliberately NOT lifted (549 MiB resident; the per-call splice is retained until Phase 3) and
/// `staticAmmo` stays a public-API parameter — both would ride in `extra` if published, but the
/// `DbPayloadProjection` (C#) publish deliberately omits them; only test fixtures land them there.
#[derive(Debug, Deserialize)]
pub struct LocationEntry {
    /// `Location.Base` (`Location.cs:12-13`). `Option` keeps the parse total — the table's
    /// UI-linkage `base` key parses as an entry with no `base` member of its own.
    pub base: Option<LocationBaseView>,
    /// `Location.AllExtracts` (`Location.cs:45-46`) — a member of `Location`, not `LocationBase`;
    /// what `BuildExtractsByLocation` projects (`RepeatableQuestNativeRequestBuilder.cs:231`).
    #[serde(rename = "allExtracts", default)]
    pub all_extracts: Vec<ExitSourceView>,
    /// `Location.StaticLoot` (`Location.cs:24-25`).
    #[serde(rename = "staticLoot")]
    pub static_loot: Option<HashMap<String, StaticLootDetails>>,
    /// `Location.StaticContainers` (`Location.cs:30-31`).
    #[serde(rename = "staticContainers")]
    pub static_containers: Option<StaticContainerDetailsWire>,
    /// `Location.Statics` (`Location.cs:39-40`).
    #[serde(rename = "statics")]
    pub statics: Option<StaticContainer>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `StaticContainerDetails` (`Location.cs:106-116`). All three `Option`: the C# members are
/// nullable `IEnumerable`s and the generator logs a map-specific error per missing list.
#[derive(Debug, Deserialize)]
pub struct StaticContainerDetailsWire {
    #[serde(rename = "staticWeapons")]
    pub static_weapons: Option<Vec<SpawnpointTemplate>>,
    #[serde(rename = "staticContainers")]
    pub static_containers: Option<Vec<StaticContainerData>>,
    #[serde(rename = "staticForced")]
    pub static_forced: Option<Vec<StaticForced>>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/LocationBase.cs:8` `LocationBase` — only the two members
/// `BuildBossSpawnsByLocation` reads (`RepeatableQuestNativeRequestBuilder.cs:195-206`).
#[derive(Debug, Deserialize)]
pub struct LocationBaseView {
    /// `required string` in C# (`LocationBase.cs:181-182`); `Option` so the parse stays total —
    /// the builder's `Base?.Id is not { }` skip is the branch a missing id takes.
    #[serde(rename = "Id")]
    pub id: Option<String>,
    #[serde(rename = "BossLocationSpawn", default)]
    pub boss_location_spawn: Vec<BossSpawnView>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `LocationBase.cs:501` `BossLocationSpawn` — `BossName` is the only member the elimination
/// projection reads off a spawn.
#[derive(Debug, Deserialize)]
pub struct BossSpawnView {
    #[serde(rename = "BossName")]
    pub boss_name: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `LocationBase.cs:806-883` `Exit`, restricted to the four members `ToExitView` reads
/// (`RepeatableQuestNativeRequestBuilder.cs:238-247`). `AllExtracts` entries are the derived
/// `AllExtractsExit : Exit`; its `SptName` addition rides in `extra`.
#[derive(Debug, Deserialize)]
pub struct ExitSourceView {
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "Side")]
    pub side: Option<String>,
    #[serde(rename = "Chance")]
    pub chance: Option<f64>,
    /// `RequirementState` through `JsonStringEnumConverter`, non-nullable in C# — always a string
    /// on a published root; `Option` keeps the parse total.
    #[serde(rename = "PassageRequirement")]
    pub passage_requirement: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/Tables/HandbookBase.cs:6-13` — `Categories` is unread by the ragfair
/// derivation and rides in `extra`.
#[derive(Debug, Default, Deserialize)]
pub struct HandbookBase {
    #[serde(rename = "Items", default)]
    pub items: Vec<HandbookItem>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `HandbookBase.cs:35-46` `HandbookItem`.
#[derive(Debug, Deserialize)]
pub struct HandbookItem {
    #[serde(rename = "Id", default)]
    pub id: String,
    #[serde(rename = "Price")]
    pub price: Option<f64>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/Tables/TemplateItem.cs:12-38` — the members
/// `PayloadProjection.BuildItemsView` reads, plus the flatten superset.
#[derive(Debug, Deserialize)]
pub struct TemplateItem {
    #[serde(rename = "_name")]
    pub name: Option<String>,
    /// Non-nullable `MongoId` in C#: absent deserializes as the empty id, which is what the
    /// `IsEmpty` check in `BuildItemsView` tests.
    #[serde(rename = "_parent", default)]
    pub parent: String,
    #[serde(rename = "_type")]
    pub item_type: Option<String>,
    #[serde(rename = "_props")]
    pub properties: Option<TemplateItemProperties>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs` `TemplateItemProperties`, restricted to what `BuildItemsView` reads.
/// C# `HashSet<MongoId>`/`HashSet<string>` members are `IndexSet` here: a .NET `HashSet` built
/// by deserializing a JSON array keeps that array's order and drops later duplicates, which is
/// exactly `IndexSet`'s first-wins insertion order.
#[derive(Debug, Deserialize)]
pub struct TemplateItemProperties {
    #[serde(rename = "Width")]
    pub width: Option<i32>,
    #[serde(rename = "Height")]
    pub height: Option<i32>,
    #[serde(rename = "StackMaxSize")]
    pub stack_max_size: Option<i32>,
    #[serde(rename = "StackMinRandom")]
    pub stack_min_random: Option<i32>,
    #[serde(rename = "StackMaxRandom")]
    pub stack_max_random: Option<i32>,
    #[serde(rename = "ExtraSizeUp")]
    pub extra_size_up: Option<i32>,
    #[serde(rename = "ExtraSizeDown")]
    pub extra_size_down: Option<i32>,
    #[serde(rename = "ExtraSizeLeft")]
    pub extra_size_left: Option<i32>,
    #[serde(rename = "ExtraSizeRight")]
    pub extra_size_right: Option<i32>,
    #[serde(rename = "ExtraSizeForceAdd")]
    pub extra_size_force_add: Option<bool>,
    #[serde(rename = "Grids")]
    pub grids: Option<Vec<Grid>>,
    #[serde(rename = "Slots")]
    pub slots: Option<Vec<Slot>>,
    #[serde(rename = "StackSlots")]
    pub stack_slots: Option<Vec<StackSlot>>,
    #[serde(rename = "Cartridges")]
    pub cartridges: Option<Vec<Slot>>,
    #[serde(rename = "Chambers")]
    pub chambers: Option<Vec<Slot>>,
    #[serde(rename = "ConflictingItems")]
    pub conflicting_items: Option<IndexSet<String>>,
    #[serde(rename = "Caliber")]
    pub caliber: Option<String>,
    #[serde(rename = "ammoCaliber")]
    pub ammo_caliber: Option<String>,
    #[serde(rename = "defAmmo")]
    pub def_ammo: Option<String>,
    /// `[JsonConverter(StringToNumberFactoryConverter)] int?` in C# (`TemplateItem.cs:666-669`).
    /// A root published by C# carries a plain number, but the fused load (`db::load`) splices the
    /// shipped `items.json` in raw, where most templates write it as a string — see
    /// [`deserialize_string_or_number`].
    #[serde(
        rename = "armorClass",
        default,
        deserialize_with = "deserialize_string_or_number"
    )]
    pub armor_class: Option<i32>,
    #[serde(rename = "QuestItem")]
    pub quest_item: Option<bool>,
    /// The `ReloadMode` member *name*, normalized at parse time — see
    /// [`deserialize_reload_mode`].
    #[serde(
        rename = "ReloadMode",
        default,
        deserialize_with = "deserialize_reload_mode"
    )]
    pub reload_mode: Option<String>,
    /// Same C# enum as [`Self::reload_mode`] (`TemplateItem.cs:568-569`).
    #[serde(
        rename = "ReloadMagType",
        default,
        deserialize_with = "deserialize_reload_mode"
    )]
    pub reload_mag_type: Option<String>,
    #[serde(rename = "isChamberLoad")]
    pub is_chamber_load: Option<bool>,
    #[serde(rename = "defMagType")]
    pub def_mag_type: Option<String>,
    #[serde(rename = "LinkedWeapon")]
    pub linked_weapon: Option<String>,
    #[serde(rename = "MaxDurability")]
    pub max_durability: Option<f64>,
    #[serde(rename = "weapClass")]
    pub weap_class: Option<String>,
    #[serde(rename = "HasHinge")]
    pub has_hinge: Option<bool>,
    #[serde(rename = "Foldable")]
    pub foldable: Option<bool>,
    #[serde(rename = "FoldedSlot")]
    pub folded_slot: Option<String>,
    #[serde(rename = "SizeReduceRight")]
    pub size_reduce_right: Option<i32>,
    #[serde(rename = "weapFireType")]
    pub weap_fire_type: Option<IndexSet<String>>,
    #[serde(rename = "MaxHpResource")]
    pub max_hp_resource: Option<i32>,
    #[serde(rename = "MaxResource")]
    pub max_resource: Option<i32>,
    #[serde(rename = "foodUseTime")]
    pub food_use_time: Option<f64>,
    #[serde(rename = "FaceShieldComponent")]
    pub face_shield_component: Option<bool>,
    #[serde(rename = "BlocksEarpiece")]
    pub blocks_earpiece: Option<bool>,
    #[serde(rename = "BlocksEyewear")]
    pub blocks_eyewear: Option<bool>,
    #[serde(rename = "BlocksFaceCover")]
    pub blocks_face_cover: Option<bool>,
    #[serde(rename = "BlocksHeadwear")]
    pub blocks_headwear: Option<bool>,
    #[serde(rename = "BlocksFolding")]
    pub blocks_folding: Option<bool>,
    #[serde(rename = "BlocksCollapsible")]
    pub blocks_collapsible: Option<bool>,
    /// Wire name `blockLeftStance` — the C# prop is `BlockLeftStance`, not `Blocks…`
    /// (`TemplateItem.cs:766-767`).
    #[serde(rename = "blockLeftStance")]
    pub block_left_stance: Option<bool>,
    #[serde(rename = "BlocksArmorVest")]
    pub blocks_armor_vest: Option<bool>,
    #[serde(rename = "Durability")]
    pub durability: Option<f64>,
    #[serde(rename = "MaximumNumberOfUsage")]
    pub maximum_number_of_usage: Option<i32>,
    /// `int?` in C#; `f64` because that is what `ItemView.max_repair_resource` carries.
    #[serde(rename = "MaxRepairResource")]
    pub max_repair_resource: Option<f64>,
    #[serde(rename = "CanSellOnRagfair")]
    pub can_sell_on_ragfair: Option<bool>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1640-1658` `Grid`.
#[derive(Debug, Deserialize)]
pub struct Grid {
    #[serde(rename = "_name")]
    pub name: Option<String>,
    #[serde(rename = "_props")]
    pub properties: Option<GridProperties>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1660-1683` `GridProperties`.
#[derive(Debug, Deserialize)]
pub struct GridProperties {
    #[serde(rename = "filters")]
    pub filters: Option<Vec<GridFilter>>,
    #[serde(rename = "cellsH")]
    pub cells_h: Option<i32>,
    #[serde(rename = "cellsV")]
    pub cells_v: Option<i32>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1685-1696` `GridFilter`.
#[derive(Debug, Deserialize)]
pub struct GridFilter {
    #[serde(rename = "Filter")]
    pub filter: Option<IndexSet<String>>,
    #[serde(rename = "ExcludedFilter")]
    pub excluded_filter: Option<IndexSet<String>>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1698-1740` `Slot` — `Cartridges` and `Chambers` are `Slot` lists too.
#[derive(Debug, Deserialize)]
pub struct Slot {
    #[serde(rename = "_name")]
    pub name: Option<String>,
    #[serde(rename = "_props")]
    pub properties: Option<SlotProperties>,
    #[serde(rename = "_max_count")]
    pub max_count: Option<f64>,
    #[serde(rename = "_required")]
    pub required: Option<bool>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1742-1749` `SlotProperties`.
#[derive(Debug, Deserialize)]
pub struct SlotProperties {
    #[serde(rename = "filters")]
    pub filters: Option<Vec<SlotFilter>>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1751-1767` `SlotFilter` — shared by `Slot` and `StackSlot` properties.
#[derive(Debug, Deserialize)]
pub struct SlotFilter {
    #[serde(rename = "Plate")]
    pub plate: Option<String>,
    #[serde(rename = "Filter")]
    pub filter: Option<IndexSet<String>>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1769-1791` `StackSlot`.
#[derive(Debug, Deserialize)]
pub struct StackSlot {
    #[serde(rename = "_max_count")]
    pub max_count: Option<f64>,
    #[serde(rename = "_props")]
    pub properties: Option<StackSlotProperties>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1793-1797` `StackSlotProperties`.
#[derive(Debug, Deserialize)]
pub struct StackSlotProperties {
    #[serde(rename = "filters")]
    pub filters: Option<Vec<SlotFilter>>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// `StringToNumberFactoryConverter` (`Utils/Json/Converters/StringToNumberFactoryConverter.cs`):
/// a string parses to the number, with empty/whitespace, `__REPLACEME__` and any parse failure
/// all collapsing to the C# `default` (null here); a number or null passes through. The shipped
/// `items.json` writes `armorClass` as a string on most templates and a number on the rest, and
/// the fused load splices those raw bytes without the C# round-trip that used to normalize them.
fn deserialize_string_or_number<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        Some(Value::Number(number)) => {
            Ok(number.as_i64().and_then(|value| i32::try_from(value).ok()))
        }
        Some(Value::String(text)) => Ok(text.trim().parse().ok()),
        _ => Ok(None),
    }
}

/// `Models/Enums/ReloadMode.cs` in declaration order — index = numeric enum value.
const RELOAD_MODE_MEMBERS: [&str; 4] = [
    "ExternalMagazine",
    "InternalMagazine",
    "OnlyBarrel",
    "ExternalMagazineWithInternalReloadSupport",
];

/// `EftEnumConverter` *writes* enums as numbers, so a published root carries `"ReloadMode": 0`;
/// its `Read` accepts a number or a case-insensitive name (`Enum.Parse(..., ignoreCase: true)`,
/// numeric strings included). What the view later needs is `ReloadMode?.ToString()` — the member
/// name for defined values, the raw number for undefined ones (C# `Enum.Parse` accepts any
/// integer and `ToString` prints an undefined value as its number). Normalize to that string at
/// parse time. A name with no member is what C# throws on, so it fails the deserialize here.
fn deserialize_reload_mode<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    fn member_or_number(value: i64) -> String {
        usize::try_from(value)
            .ok()
            .and_then(|index| RELOAD_MODE_MEMBERS.get(index))
            .map_or_else(|| value.to_string(), |member| (*member).to_string())
    }

    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let value = number.as_i64().ok_or_else(|| {
                serde::de::Error::custom(format!("ReloadMode {number} is not an integer"))
            })?;
            Ok(Some(member_or_number(value)))
        }
        Some(Value::String(text)) => {
            if let Ok(value) = text.parse::<i64>() {
                return Ok(Some(member_or_number(value)));
            }
            RELOAD_MODE_MEMBERS
                .iter()
                .find(|member| member.eq_ignore_ascii_case(&text))
                .map(|member| Some((*member).to_string()))
                .ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "ReloadMode '{text}' has no member — C# Enum.Parse throws"
                    ))
                })
        }
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected a ReloadMode name or number, found {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `repeatableQuests` block — the exact wire a published `templates` root carries
    /// under that key, the same fixture `quest::models`' round-trip tests parse.
    const REPEATABLE_QUESTS_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/repeatableQuests.json"
    );

    #[test]
    fn locations_root_lifts_base_and_extracts_and_keeps_the_rest() {
        // Wire names pinned to Location.cs ("base", "allExtracts") and LocationBase.cs
        // ("Id", "BossLocationSpawn", "BossName", "Name"/"Side"/"Chance"/"PassageRequirement")
        let root: LocationsRoot = serde_json::from_str(
            r#"{
                "factory4_day": {
                    "base": {
                        "Id": "55f2d3fd4bdc2d5f408b4567",
                        "BossLocationSpawn": [
                            {"BossName": "bossTagilla", "BossChance": 25.0}
                        ],
                        "OpenZones": "ZoneFactory"
                    },
                    "allExtracts": [
                        {
                            "Name": "Gate 3",
                            "Side": "Pmc",
                            "Chance": 100.0,
                            "PassageRequirement": "TransferItem",
                            "ExfiltrationTime": 8.0
                        }
                    ],
                    "looseLoot": {"spawnpointCount": 1}
                }
            }"#,
        )
        .unwrap();

        let entry = &root.locations["factory4_day"];
        let base = entry.base.as_ref().unwrap();
        assert_eq!(base.id.as_deref(), Some("55f2d3fd4bdc2d5f408b4567"));
        assert_eq!(
            base.boss_location_spawn[0].boss_name.as_deref(),
            Some("bossTagilla")
        );
        let exit = &entry.all_extracts[0];
        assert_eq!(exit.name.as_deref(), Some("Gate 3"));
        assert_eq!(exit.side.as_deref(), Some("Pmc"));
        assert_eq!(exit.chance, Some(100.0));
        assert_eq!(exit.passage_requirement.as_deref(), Some("TransferItem"));
        // Unlifted members ride in extra at every level
        assert!(entry.extra.contains_key("looseLoot"));
        assert!(base.extra.contains_key("OpenZones"));
        assert!(base.boss_location_spawn[0].extra.contains_key("BossChance"));
        assert!(exit.extra.contains_key("ExfiltrationTime"));
    }

    #[test]
    fn location_entry_lifts_the_statics_and_keeps_loose_loot_in_extra() {
        let root: LocationsRoot = serde_json::from_str(
            r#"{"bigmap":{
                "base": {"Id": "bigmap"},
                "allExtracts": [],
                "looseLoot": {"spawnpointCount": {"mean": 1.0}},
                "staticLoot": {"tpl1": {"itemcountDistribution": [], "itemDistribution": []}},
                "staticContainers": {"staticWeapons": [], "staticContainers": [], "staticForced": []},
                "statics": {"containersGroups": {}, "containers": {}},
                "staticAmmo": {}
            }}"#,
        )
        .unwrap();

        let entry = &root.locations["bigmap"];
        assert!(entry.static_loot.as_ref().unwrap().contains_key("tpl1"));
        assert!(
            entry
                .static_containers
                .as_ref()
                .unwrap()
                .static_weapons
                .is_some()
        );
        assert!(entry.statics.is_some());
        // looseLoot is deliberately NOT lifted (flip #4 decision: per-call splice retained) —
        // it still rides extra, as does staticAmmo (public-API parameter, stays varying).
        assert!(entry.extra.contains_key("looseLoot"));
        assert!(entry.extra.contains_key("staticAmmo"));
    }

    #[test]
    fn templates_root_lifts_the_shipped_repeatable_quests_block() {
        let block =
            std::fs::read_to_string(REPEATABLE_QUESTS_PATH).expect("SPT_Data file readable");
        let root: TemplatesRoot =
            serde_json::from_str(&format!(r#"{{"repeatableQuests":{block}}}"#)).unwrap();

        let repeatable = root.repeatable_quests.as_ref().unwrap();
        let templates = repeatable.templates.as_ref().unwrap();
        assert!(templates.elimination.is_some());
        let completion = repeatable
            .data
            .as_ref()
            .unwrap()
            .completion
            .as_ref()
            .unwrap();
        assert_eq!(completion.items_whitelist[0].min_player_level, Some(1));
        assert!(!completion.items_blacklist.is_empty());
        // The unlifted RepeatableQuestDatabase members ride in extra
        assert!(repeatable.extra.contains_key("rewards"));
        assert!(repeatable.extra.contains_key("samples"));
    }

    #[test]
    fn armor_class_reads_the_shipped_string_form_as_well_as_a_number() {
        // The C# publish normalizes through StringToNumberFactoryConverter; the fused load splices
        // the shipped items.json raw, where most templates write armorClass as a string.
        let root: TemplatesRoot = serde_json::from_str(
            r#"{"items":{
                "string":{"_props":{"armorClass":"5"}},
                "number":{"_props":{"armorClass":6}},
                "blank":{"_props":{"armorClass":""}},
                "placeholder":{"_props":{"armorClass":"__REPLACEME__"}},
                "null":{"_props":{"armorClass":null}},
                "absent":{"_props":{}}
            }}"#,
        )
        .unwrap();

        let armor_class = |id: &str| root.items[id].properties.as_ref().unwrap().armor_class;
        assert_eq!(armor_class("string"), Some(5));
        assert_eq!(armor_class("number"), Some(6));
        // Every C# `default` path collapses to null
        assert_eq!(armor_class("blank"), None);
        assert_eq!(armor_class("placeholder"), None);
        assert_eq!(armor_class("null"), None);
        assert_eq!(armor_class("absent"), None);
    }

    #[test]
    fn publish_roots_accepts_locations_as_a_root_name() {
        let request: PublishRequest =
            serde_json::from_str(r#"{"schema":1,"roots":{"locations":{"factory4_day":{}}}}"#)
                .unwrap();
        assert!(request.roots.locations.is_some());
    }
}
