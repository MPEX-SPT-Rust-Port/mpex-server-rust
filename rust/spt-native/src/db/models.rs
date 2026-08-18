//! Wire models of the resident database (spec § The epoch protocol, as amended 2026-08-18).
//!
//! Task-1 shape rule: every root is a `#[serde(flatten)]` superset map. Typed fields are lifted
//! out of `extra` only when Rust-side derivation reads them (`ragfair::views::derive` today) —
//! the flatten map is what keeps the root full-fidelity regardless. Wire names are pinned to the
//! C# `JsonPropertyName` of the record each type mirrors (`Models/Spt/Tables/TemplateTable.cs`,
//! `TradersTable.cs`, `GlobalTable.cs` and the member types they reach). Every lifted container
//! carries `#[serde(default)]`: a partial or junk root (the store tests publish `{"a":1}`)
//! deserializes with empty containers and derivation stays total over it.

use indexmap::{IndexMap, IndexSet};
use serde::Deserialize;
use serde_json::Value;

use crate::loot::models::Item;

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
}

/// `Models/Spt/Tables/TemplateTable.cs` — only the members the ragfair view derivation reads
/// are typed; everything else rides in `extra`.
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

/// `Models/Spt/Tables/GlobalTable.cs:10-26` — only `ItemPresets` is typed.
#[derive(Debug, Default, Deserialize)]
pub struct GlobalsRoot {
    /// `GlobalTable.ItemPresets` (`GlobalTable.cs:24-25`), keyed by preset id — that key domain
    /// (the map's keys, not each preset's `_id`) is what `PresetHelper.IsPreset`/`GetPreset`
    /// answer from.
    #[serde(rename = "ItemPresets", default)]
    pub item_presets: IndexMap<String, Preset>,
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
    /// `[JsonConverter(StringToNumberFactoryConverter)] int?` in C# — but a published root has
    /// already been through the C# record, which re-serializes the value as a plain number.
    #[serde(rename = "armorClass")]
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
