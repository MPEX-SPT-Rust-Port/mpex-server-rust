//! Wire models for the repeatable-quest generators.
//!
//! The same two families as `loot::models`, `bot::models` and `ragfair::models`:
//!
//! * DB/EFT models mirroring the C# records under `SPTarkov.Server.Core.Models` — member names
//!   pinned to the exact `JsonPropertyName` of the record they replace, nullability mirroring the
//!   C# declaration, absent staying absent on the way out (C# serializes with
//!   `JsonIgnoreCondition.WhenWritingNull`). These cross the FFI in both directions — the
//!   templates come in, a generated quest goes out — so each one carries a
//!   `#[serde(flatten)] extra` map for the members `Tools/Ceciler`'s `[JsonExtensionData]`
//!   property catches on the C# side.
//! * `Models/Spt/Config/QuestConfig.cs` records and the request/response envelopes, which only
//!   ever travel one way and so have no passthrough map. The envelopes are a fresh contract
//!   between the C# caller and this crate, so they are plain camelCase.
//!
//! C# enums that reach the wire as strings stay `String` here unless something branches on them:
//! `QuestTypeEnum` (note `PickUp`, where the pool and the config say `Pickup`), `RewardType` and
//! `PlayerGroup` are echoed verbatim rather than re-declared.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::bot::repair_service::MinMax;
use crate::loot::models::{Diagnostic, Item, ItemView, PresetView, deserialize_string_or_number};

/// Mod-added members captured on the way in and replayed on the way out.
type Extra = serde_json::Map<String, serde_json::Value>;

// ---------------------------------------------------------------------------
// DB/EFT wire models
// ---------------------------------------------------------------------------

/// `Utils/Json/ListOrT.cs` — a member the client writes as either one value or an array of them.
/// Untagged, so the shape it arrived in is the shape it leaves in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ListOrT<T> {
    Item(T),
    List(Vec<T>),
}

/// `Models/Eft/Common/Tables/Quest.cs:10-129`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Quest {
    /// SPT addition — human readable quest name.
    #[serde(rename = "QuestName", skip_serializing_if = "Option::is_none")]
    pub quest_name: Option<String>,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "canShowNotificationsInGame")]
    pub can_show_notifications_in_game: bool,
    #[serde(rename = "conditions")]
    pub conditions: QuestConditionTypes,
    #[serde(rename = "description")]
    pub description: String,
    #[serde(rename = "failMessageText", skip_serializing_if = "Option::is_none")]
    pub fail_message_text: Option<String>,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "note", skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(rename = "traderId")]
    pub trader_id: String,
    /// C# declares this `required string`, but `repeatableQuests.json` ships it null on the
    /// Elimination and Exploration templates — `System.Text.Json` writes the null straight into the
    /// non-nullable member, so the faithful mirror is an `Option`.
    #[serde(rename = "location", skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(rename = "image")]
    pub image: String,
    /// `QuestTypeEnum` through `JsonStringEnumConverter`.
    #[serde(rename = "type")]
    pub quest_type: String,
    #[serde(rename = "isKey", skip_serializing_if = "Option::is_none")]
    pub is_key: Option<bool>,
    #[serde(rename = "restartable")]
    pub restartable: bool,
    #[serde(rename = "instantComplete", skip_serializing_if = "Option::is_none")]
    pub instant_complete: Option<bool>,
    #[serde(rename = "secretQuest", skip_serializing_if = "Option::is_none")]
    pub secret_quest: Option<bool>,
    #[serde(rename = "startedMessageText", skip_serializing_if = "Option::is_none")]
    pub started_message_text: Option<String>,
    #[serde(rename = "successMessageText", skip_serializing_if = "Option::is_none")]
    pub success_message_text: Option<String>,
    #[serde(
        rename = "acceptPlayerMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub accept_player_message: Option<String>,
    #[serde(
        rename = "acceptanceAndFinishingSource",
        skip_serializing_if = "Option::is_none"
    )]
    pub acceptance_and_finishing_source: Option<String>,
    #[serde(
        rename = "declinePlayerMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub decline_player_message: Option<String>,
    #[serde(
        rename = "completePlayerMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub complete_player_message: Option<String>,
    #[serde(rename = "templateId", skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    /// Keyed `Started`/`Success`/`Fail`; insertion ordered so the key order survives the trip.
    #[serde(rename = "rewards", skip_serializing_if = "Option::is_none")]
    pub rewards: Option<IndexMap<String, Vec<Reward>>>,
    /// Becomes `AppearStatus` inside the client.
    #[serde(rename = "status", skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    #[serde(rename = "KeyQuest", skip_serializing_if = "Option::is_none")]
    pub key_quest: Option<bool>,
    #[serde(
        rename = "changeQuestMessageText",
        skip_serializing_if = "Option::is_none"
    )]
    pub change_quest_message_text: Option<String>,
    /// `Pmc` or `Scav`.
    #[serde(rename = "side")]
    pub side: String,
    #[serde(rename = "progressSource", skip_serializing_if = "Option::is_none")]
    pub progress_source: Option<String>,
    #[serde(rename = "rankingModes", skip_serializing_if = "Option::is_none")]
    pub ranking_modes: Option<Vec<String>>,
    #[serde(rename = "gameModes", skip_serializing_if = "Option::is_none")]
    pub game_modes: Option<Vec<String>>,
    #[serde(rename = "arenaLocations", skip_serializing_if = "Option::is_none")]
    pub arena_locations: Option<Vec<String>>,
    #[serde(rename = "dialogueId", skip_serializing_if = "Option::is_none")]
    pub dialogue_id: Option<String>,
    /// `QuestStatusEnum`, whose wire form depends on the converters the C# caller registers.
    /// Nothing in this port reads or writes it, so it is echoed as-is.
    #[serde(rename = "sptStatus", skip_serializing_if = "Option::is_none")]
    pub spt_status: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/RepeatableQuests.cs:6-19` — `record RepeatableQuest : Quest`. Rust has
/// no record inheritance, so the base is flattened in and [`Quest::extra`] catches the unknowns for
/// both halves.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepeatableQuest {
    #[serde(flatten)]
    pub quest: Quest,
    #[serde(rename = "changeCost")]
    pub change_cost: Vec<ChangeCost>,
    #[serde(rename = "changeStandingCost", skip_serializing_if = "Option::is_none")]
    pub change_standing_cost: Option<i32>,
    #[serde(
        rename = "sptRepatableGroupName",
        skip_serializing_if = "Option::is_none"
    )]
    pub spt_repatable_group_name: Option<String>,
    #[serde(rename = "questStatus", skip_serializing_if = "Option::is_none")]
    pub quest_status: Option<RepeatableQuestStatus>,
}

/// `Models/Eft/Common/Tables/RepeatableQuests.cs:115-128`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChangeCost {
    /// What item it will take to reset daily.
    #[serde(rename = "templateId")]
    pub template_id: String,
    /// Amount of item needed to reset.
    #[serde(rename = "count", skip_serializing_if = "Option::is_none")]
    pub count: Option<i32>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/RepeatableQuests.cs:36-55`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepeatableQuestStatus {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "uid", skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(rename = "qid")]
    pub qid: String,
    #[serde(rename = "startTime", skip_serializing_if = "Option::is_none")]
    pub start_time: Option<i64>,
    #[serde(rename = "status", skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    /// C# types this `object?`; nothing reads it, so it is echoed as-is.
    #[serde(rename = "statusTimers", skip_serializing_if = "Option::is_none")]
    pub status_timers: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:155-171`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestConditionTypes {
    #[serde(rename = "Started", skip_serializing_if = "Option::is_none")]
    pub started: Option<Vec<QuestCondition>>,
    #[serde(rename = "AvailableForFinish", skip_serializing_if = "Option::is_none")]
    pub available_for_finish: Option<Vec<QuestCondition>>,
    #[serde(rename = "AvailableForStart", skip_serializing_if = "Option::is_none")]
    pub available_for_start: Option<Vec<QuestCondition>>,
    #[serde(rename = "Success", skip_serializing_if = "Option::is_none")]
    pub success: Option<Vec<QuestCondition>>,
    #[serde(rename = "Fail", skip_serializing_if = "Option::is_none")]
    pub fail: Option<Vec<QuestCondition>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:173-322`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestCondition {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "index", skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,
    #[serde(rename = "compareMethod", skip_serializing_if = "Option::is_none")]
    pub compare_method: Option<String>,
    #[serde(rename = "dynamicLocale")]
    pub dynamic_locale: bool,
    #[serde(
        rename = "globalQuestCounterId",
        skip_serializing_if = "Option::is_none"
    )]
    pub global_quest_counter_id: Option<String>,
    #[serde(
        rename = "visibilityConditions",
        skip_serializing_if = "Option::is_none"
    )]
    pub visibility_conditions: Option<Vec<VisibilityCondition>>,
    /// Nullable in the client.
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// A tpl or a plain string (`event_labyrinth_06_mech_place_01`), one or many.
    #[serde(rename = "target", skip_serializing_if = "Option::is_none")]
    pub target: Option<ListOrT<String>>,
    #[serde(
        rename = "value",
        default,
        deserialize_with = "deserialize_string_or_number",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<f64>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub quest_type: Option<String>,
    /// `HashSet<QuestStatusEnum>`, whose wire form depends on the converters the C# caller
    /// registers. Nothing in this port reads or writes it, so it is echoed as-is.
    #[serde(rename = "status", skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    #[serde(rename = "availableAfter", skip_serializing_if = "Option::is_none")]
    pub available_after: Option<i32>,
    #[serde(rename = "dispersion", skip_serializing_if = "Option::is_none")]
    pub dispersion: Option<f64>,
    #[serde(rename = "onlyFoundInRaid", skip_serializing_if = "Option::is_none")]
    pub only_found_in_raid: Option<bool>,
    #[serde(rename = "oneSessionOnly", skip_serializing_if = "Option::is_none")]
    pub one_session_only: Option<bool>,
    #[serde(
        rename = "isResetOnConditionFailed",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_reset_on_condition_failed: Option<bool>,
    #[serde(rename = "isNecessary", skip_serializing_if = "Option::is_none")]
    pub is_necessary: Option<bool>,
    #[serde(
        rename = "doNotResetIfCounterCompleted",
        skip_serializing_if = "Option::is_none"
    )]
    pub do_not_reset_if_counter_completed: Option<bool>,
    #[serde(
        rename = "dogtagLevel",
        default,
        deserialize_with = "deserialize_string_or_i32",
        skip_serializing_if = "Option::is_none"
    )]
    pub dogtag_level: Option<i32>,
    #[serde(rename = "traderId", skip_serializing_if = "Option::is_none")]
    pub trader_id: Option<String>,
    #[serde(rename = "maxDurability", skip_serializing_if = "Option::is_none")]
    pub max_durability: Option<f64>,
    #[serde(rename = "minDurability", skip_serializing_if = "Option::is_none")]
    pub min_durability: Option<f64>,
    #[serde(rename = "counter", skip_serializing_if = "Option::is_none")]
    pub counter: Option<QuestConditionCounter>,
    #[serde(rename = "plantTime", skip_serializing_if = "Option::is_none")]
    pub plant_time: Option<f64>,
    #[serde(rename = "zoneId", skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
    #[serde(rename = "countInRaid", skip_serializing_if = "Option::is_none")]
    pub count_in_raid: Option<bool>,
    #[serde(rename = "completeInSeconds", skip_serializing_if = "Option::is_none")]
    pub complete_in_seconds: Option<f64>,
    #[serde(rename = "isEncoded", skip_serializing_if = "Option::is_none")]
    pub is_encoded: Option<bool>,
    #[serde(rename = "conditionType")]
    pub condition_type: String,
    #[serde(rename = "epicGamesId", skip_serializing_if = "Option::is_none")]
    pub epic_games_id: Option<String>,
    #[serde(rename = "steamGamesId", skip_serializing_if = "Option::is_none")]
    pub steam_games_id: Option<String>,
    /// `HideoutAreas`, echoed as-is for the same reason as [`Self::status`].
    #[serde(rename = "areaType", skip_serializing_if = "Option::is_none")]
    pub area_type: Option<serde_json::Value>,
    #[serde(rename = "baseAccuracy", skip_serializing_if = "Option::is_none")]
    pub base_accuracy: Option<ValueCompare>,
    #[serde(rename = "containsItems", skip_serializing_if = "Option::is_none")]
    pub contains_items: Option<Vec<String>>,
    #[serde(rename = "durability", skip_serializing_if = "Option::is_none")]
    pub durability: Option<ValueCompare>,
    #[serde(rename = "effectiveDistance", skip_serializing_if = "Option::is_none")]
    pub effective_distance: Option<ValueCompare>,
    #[serde(rename = "emptyTacticalSlot", skip_serializing_if = "Option::is_none")]
    pub empty_tactical_slot: Option<ValueCompare>,
    #[serde(rename = "ergonomics", skip_serializing_if = "Option::is_none")]
    pub ergonomics: Option<ValueCompare>,
    #[serde(rename = "height", skip_serializing_if = "Option::is_none")]
    pub height: Option<ValueCompare>,
    #[serde(
        rename = "hasItemFromCategory",
        skip_serializing_if = "Option::is_none"
    )]
    pub has_item_from_category: Option<Vec<String>>,
    #[serde(rename = "magazineCapacity", skip_serializing_if = "Option::is_none")]
    pub magazine_capacity: Option<ValueCompare>,
    #[serde(rename = "muzzleVelocity", skip_serializing_if = "Option::is_none")]
    pub muzzle_velocity: Option<ValueCompare>,
    #[serde(rename = "recoil", skip_serializing_if = "Option::is_none")]
    pub recoil: Option<ValueCompare>,
    #[serde(rename = "weight", skip_serializing_if = "Option::is_none")]
    pub weight: Option<ValueCompare>,
    #[serde(rename = "width", skip_serializing_if = "Option::is_none")]
    pub width: Option<ValueCompare>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:324-331`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestConditionCounter {
    #[serde(rename = "id", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "conditions", skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<QuestConditionCounterCondition>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:333-421`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestConditionCounterCondition {
    #[serde(rename = "id", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "dynamicLocale", skip_serializing_if = "Option::is_none")]
    pub dynamic_locale: Option<bool>,
    #[serde(rename = "target", skip_serializing_if = "Option::is_none")]
    pub target: Option<ListOrT<String>>,
    #[serde(rename = "completeInSeconds", skip_serializing_if = "Option::is_none")]
    pub complete_in_seconds: Option<i32>,
    #[serde(rename = "energy", skip_serializing_if = "Option::is_none")]
    pub energy: Option<ValueCompare>,
    #[serde(rename = "exitName", skip_serializing_if = "Option::is_none")]
    pub exit_name: Option<String>,
    #[serde(rename = "hydration", skip_serializing_if = "Option::is_none")]
    pub hydration: Option<ValueCompare>,
    #[serde(rename = "time", skip_serializing_if = "Option::is_none")]
    pub time: Option<ValueCompare>,
    #[serde(rename = "compareMethod", skip_serializing_if = "Option::is_none")]
    pub compare_method: Option<String>,
    /// C# types this `object?` — the client writes a number here, but nothing reads it back.
    #[serde(rename = "value", skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(rename = "weapon", skip_serializing_if = "Option::is_none")]
    pub weapon: Option<Vec<String>>,
    #[serde(rename = "distance", skip_serializing_if = "Option::is_none")]
    pub distance: Option<CounterConditionDistance>,
    #[serde(rename = "equipmentInclusive", skip_serializing_if = "Option::is_none")]
    pub equipment_inclusive: Option<Vec<Vec<String>>>,
    #[serde(
        rename = "weaponModsInclusive",
        skip_serializing_if = "Option::is_none"
    )]
    pub weapon_mods_inclusive: Option<Vec<Vec<String>>>,
    #[serde(
        rename = "weaponModsExclusive",
        skip_serializing_if = "Option::is_none"
    )]
    pub weapon_mods_exclusive: Option<Vec<Vec<String>>>,
    #[serde(
        rename = "enemyEquipmentInclusive",
        skip_serializing_if = "Option::is_none"
    )]
    pub enemy_equipment_inclusive: Option<Vec<Vec<String>>>,
    #[serde(
        rename = "enemyEquipmentExclusive",
        skip_serializing_if = "Option::is_none"
    )]
    pub enemy_equipment_exclusive: Option<Vec<Vec<String>>>,
    #[serde(rename = "weaponCaliber", skip_serializing_if = "Option::is_none")]
    pub weapon_caliber: Option<Vec<String>>,
    #[serde(rename = "savageRole", skip_serializing_if = "Option::is_none")]
    pub savage_role: Option<Vec<String>>,
    #[serde(rename = "status", skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<String>>,
    #[serde(rename = "bodyPart", skip_serializing_if = "Option::is_none")]
    pub body_part: Option<Vec<String>>,
    #[serde(rename = "daytime", skip_serializing_if = "Option::is_none")]
    pub daytime: Option<DaytimeCounter>,
    #[serde(rename = "conditionType")]
    pub condition_type: String,
    #[serde(rename = "enemyHealthEffects", skip_serializing_if = "Option::is_none")]
    pub enemy_health_effects: Option<Vec<EnemyHealthEffect>>,
    #[serde(rename = "resetOnSessionEnd", skip_serializing_if = "Option::is_none")]
    pub reset_on_session_end: Option<bool>,
    #[serde(
        rename = "bodyPartsWithEffects",
        skip_serializing_if = "Option::is_none"
    )]
    pub body_parts_with_effects: Option<Vec<EnemyHealthEffect>>,
    #[serde(
        rename = "IncludeNotEquippedItems",
        skip_serializing_if = "Option::is_none"
    )]
    pub include_not_equipped_items: Option<bool>,
    #[serde(rename = "equipmentExclusive", skip_serializing_if = "Option::is_none")]
    pub equipment_exclusive: Option<Vec<Vec<String>>>,
    #[serde(rename = "zoneIds", skip_serializing_if = "Option::is_none")]
    pub zones: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:423-430`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnemyHealthEffect {
    #[serde(rename = "bodyParts", skip_serializing_if = "Option::is_none")]
    pub body_parts: Option<Vec<String>>,
    #[serde(rename = "effects", skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:432-439`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValueCompare {
    #[serde(rename = "compareMethod", skip_serializing_if = "Option::is_none")]
    pub compare_method: Option<String>,
    #[serde(rename = "value", skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:441-448` — [`ValueCompare`] with the members the other way
/// round, which is how the C# declares it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CounterConditionDistance {
    #[serde(rename = "value", skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(rename = "compareMethod", skip_serializing_if = "Option::is_none")]
    pub compare_method: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:450-457`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaytimeCounter {
    #[serde(rename = "from", skip_serializing_if = "Option::is_none")]
    pub from: Option<i32>,
    #[serde(rename = "to", skip_serializing_if = "Option::is_none")]
    pub to: Option<i32>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Quest.cs:459-478`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VisibilityCondition {
    #[serde(rename = "id", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "target", skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(rename = "value", skip_serializing_if = "Option::is_none")]
    pub value: Option<i32>,
    #[serde(rename = "dynamicLocale", skip_serializing_if = "Option::is_none")]
    pub dynamic_locale: Option<bool>,
    #[serde(rename = "oneSessionOnly", skip_serializing_if = "Option::is_none")]
    pub one_session_only: Option<bool>,
    #[serde(rename = "conditionType")]
    pub condition_type: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Eft/Common/Tables/Reward.cs:9-77`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reward {
    #[serde(
        rename = "value",
        default,
        deserialize_with = "deserialize_string_or_number",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<f64>,
    #[serde(rename = "id")]
    pub id: String,
    /// `RewardType` through `JsonStringEnumConverter`.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub reward_type: Option<String>,
    #[serde(rename = "index", skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,
    /// Can be more than just a `MongoId`.
    #[serde(rename = "target", skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(rename = "items", skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<Item>>,
    #[serde(rename = "loyaltyLevel", skip_serializing_if = "Option::is_none")]
    pub loyalty_level: Option<i32>,
    /// `StringOrInt` through `StringOrIntConverter` — a trader id or a hideout area id.
    #[serde(rename = "traderId", skip_serializing_if = "Option::is_none")]
    pub trader_id: Option<StringOrInt>,
    #[serde(rename = "isEncoded", skip_serializing_if = "Option::is_none")]
    pub is_encoded: Option<bool>,
    #[serde(rename = "unknown", skip_serializing_if = "Option::is_none")]
    pub unknown: Option<bool>,
    #[serde(rename = "findInRaid", skip_serializing_if = "Option::is_none")]
    pub find_in_raid: Option<bool>,
    #[serde(rename = "gameMode", skip_serializing_if = "Option::is_none")]
    pub game_mode: Option<Vec<String>>,
    /// Game editions whitelisted to get the reward.
    #[serde(
        rename = "availableInGameEditions",
        skip_serializing_if = "Option::is_none"
    )]
    pub available_in_game_editions: Option<Vec<String>>,
    /// Game editions blacklisted from getting the reward.
    #[serde(
        rename = "notAvailableInGameEditions",
        skip_serializing_if = "Option::is_none"
    )]
    pub not_available_in_game_editions: Option<Vec<String>>,
    #[serde(rename = "illustrationConfig", skip_serializing_if = "Option::is_none")]
    pub illustration_config: Option<IllustrationConfig>,
    #[serde(rename = "isHidden", skip_serializing_if = "Option::is_none")]
    pub is_hidden: Option<bool>,
    /// Only found with `NotificationPopup` rewards.
    #[serde(rename = "message", skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Utils/Json/StringOrInt.cs` through its converter — either form round trips as it arrived.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrInt {
    Int(i64),
    Str(String),
}

/// `Models/Eft/Common/Tables/Reward.cs:79-89`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IllustrationConfig {
    #[serde(rename = "image")]
    pub image: String,
    #[serde(rename = "bigImage")]
    pub big_image: String,
    #[serde(rename = "isBigImage")]
    pub is_big_image: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---------------------------------------------------------------------------
// Quest type pool
// ---------------------------------------------------------------------------

/// The four repeatable task types, as the pool and `RepeatableQuestConfig.Types` spell them. Note
/// `Pickup` here against the `PickUp` of `QuestTypeEnum`, which is what the generated quest's
/// `type` member carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatableQuestType {
    Elimination,
    Completion,
    Exploration,
    Pickup,
}

/// `Models/Spt/Repeatable/QuestTypePool.cs:6-13`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestTypePool {
    #[serde(rename = "types")]
    pub types: Vec<String>,
    #[serde(rename = "pool")]
    pub pool: QuestPool,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Spt/Repeatable/QuestTypePool.cs:15-25`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestPool {
    #[serde(rename = "Exploration")]
    pub exploration: ExplorationPool,
    #[serde(rename = "Elimination")]
    pub elimination: EliminationPool,
    #[serde(rename = "Pickup")]
    pub pickup: ExplorationPool,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Spt/Repeatable/QuestTypePool.cs:27-31` — keyed by `ELocationName`, insertion ordered
/// because the generators draw from it by index.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExplorationPool {
    #[serde(rename = "locations", skip_serializing_if = "Option::is_none")]
    pub locations: Option<IndexMap<String, Vec<String>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Spt/Repeatable/QuestTypePool.cs:33-37`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EliminationPool {
    #[serde(rename = "targets", skip_serializing_if = "Option::is_none")]
    pub targets: Option<IndexMap<String, TargetLocation>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Spt/Repeatable/QuestTypePool.cs:39-43`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetLocation {
    #[serde(rename = "locations", skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---------------------------------------------------------------------------
// Config wire models — `Models/Spt/Config/QuestConfig.cs`, deserialize-only
// ---------------------------------------------------------------------------

/// `Models/Spt/Config/QuestConfig.cs:126-237`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatableQuestConfig {
    pub id: String,
    pub name: String,
    /// `PlayerGroup` — `Pmc` or `Scav`; `Random` is declared but unimplemented.
    pub side: String,
    pub types: Vec<String>,
    pub reset_time: i64,
    pub num_quests: i32,
    pub min_player_level: i32,
    pub reward_scaling: RewardScaling,
    /// Keyed by `ELocationName`; insertion ordered, because the location draw indexes it.
    pub locations: IndexMap<String, Vec<String>>,
    pub trader_whitelist: Vec<TraderWhitelist>,
    pub quest_config: RepeatableQuestTypesConfig,
    pub reward_base_type_blacklist: Vec<String>,
    pub reward_blacklist: Vec<String>,
    pub reward_ammo_stack_min_size: i32,
    pub free_changes_available: i32,
    pub free_changes: i32,
    pub keep_daily_quest_type_on_replacement: bool,
    pub standing_change_cost: Vec<f64>,
}

/// `Models/Spt/Config/QuestConfig.cs:239-294`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardScaling {
    pub levels: Vec<f64>,
    pub experience: Vec<f64>,
    pub roubles: Vec<f64>,
    pub gp_coins: Vec<f64>,
    pub items: Vec<f64>,
    pub reputation: Vec<f64>,
    pub reward_spread: f64,
    pub skill_reward_chance: Vec<f64>,
    pub skill_point_reward: Vec<f64>,
}

/// `Models/Spt/Config/QuestConfig.cs:296-333`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraderWhitelist {
    pub trader_id: String,
    pub name: String,
    pub quest_types: Vec<String>,
    pub reward_base_whitelist: Vec<String>,
    pub reward_can_be_weapon: bool,
    pub weapon_reward_chance_percent: f64,
}

/// `Models/Spt/Config/QuestConfig.cs:335-360`.
#[derive(Debug, Clone, Deserialize)]
pub struct RepeatableQuestTypesConfig {
    #[serde(rename = "Exploration")]
    pub exploration: Vec<ExplorationConfig>,
    #[serde(rename = "Completion")]
    pub completion: Vec<CompletionConfig>,
    /// Only `Daily_Savage` carries one.
    #[serde(rename = "Pickup", default)]
    pub pickup: Option<Pickup>,
    #[serde(rename = "Elimination")]
    pub elimination: Vec<EliminationConfig>,
}

/// `Models/Spt/Config/QuestConfig.cs:362-399` — `record ExplorationConfig : BaseQuestConfig`, whose
/// one base member is inlined the way the rest of this crate flattens C# record inheritance.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorationConfig {
    pub possible_skill_rewards: Vec<String>,
    pub level_range: MinMax<i32>,
    #[serde(rename = "minExtracts")]
    pub minimum_extracts: i32,
    #[serde(rename = "maxExtracts")]
    pub maximum_extracts: i32,
    #[serde(rename = "minExtractsWithSpecificExit")]
    pub minimum_extracts_with_specific_exit: i32,
    #[serde(rename = "maxExtractsWithSpecificExit")]
    pub maximum_extracts_with_specific_exit: i32,
    pub specific_exits: SpecificExits,
}

/// `Models/Spt/Config/QuestConfig.cs:401-414`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecificExits {
    pub chance: f64,
    pub passage_requirement_whitelist: Vec<String>,
}

/// `Models/Spt/Config/QuestConfig.cs:416-471`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionConfig {
    pub possible_skill_rewards: Vec<String>,
    pub level_range: MinMax<i32>,
    pub requested_item_count: MinMax<i32>,
    pub unique_item_count: MinMax<i32>,
    pub requested_bullet_count: MinMax<i32>,
    pub use_whitelist: bool,
    pub use_blacklist: bool,
    #[serde(rename = "requiredItemsAreFiR")]
    pub required_items_are_fir: bool,
    pub required_item_min_durability_min_max: MinMax<i32>,
    pub required_item_type_blacklist: Vec<String>,
}

/// `Models/Spt/Config/QuestConfig.cs:473-482`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pickup {
    pub possible_skill_rewards: Vec<String>,
    #[serde(rename = "ItemTypeToFetchWithMaxCount")]
    pub item_type_to_fetch_with_max_count: Option<Vec<PickupTypeWithMaxCount>>,
    /// Carries no `JsonPropertyName` in the C#, so the member name is the wire name.
    #[serde(rename = "ItemTypesToFetch")]
    pub item_types_to_fetch: Option<Vec<String>>,
    pub max_item_fetch_count: Option<i32>,
}

/// `Models/Spt/Config/QuestConfig.cs:484-494`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickupTypeWithMaxCount {
    pub item_type: Option<String>,
    #[serde(rename = "maxPickupCount")]
    pub maximum_pickup_count: Option<i32>,
    #[serde(rename = "minPickupCount")]
    pub minimum_pickup_count: Option<i32>,
}

/// `Models/Spt/Config/QuestConfig.cs:496-611`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EliminationConfig {
    pub possible_skill_rewards: Vec<String>,
    pub level_range: MinMax<i32>,
    pub targets: Vec<ProbabilityObjectWire<BossInfo>>,
    pub body_part_chance: i32,
    pub body_parts: Vec<ProbabilityObjectWire<Vec<String>>>,
    pub specific_location_chance: i32,
    pub dist_location_blacklist: Vec<String>,
    #[serde(rename = "distProb")]
    pub distance_probability: f64,
    #[serde(rename = "maxDist")]
    pub max_distance: f64,
    #[serde(rename = "minDist")]
    pub min_distance: f64,
    pub max_kills: i32,
    pub min_kills: i32,
    pub max_boss_kills: i32,
    pub min_boss_kills: i32,
    pub max_pmc_kills: i32,
    pub min_pmc_kills: i32,
    pub weapon_requirement_chance: i32,
    pub weapon_category_requirement_chance: i32,
    pub weapon_category_requirements: Vec<ProbabilityObjectWire<Vec<String>>>,
    pub weapon_requirements: Vec<ProbabilityObjectWire<Vec<String>>>,
}

/// `Utils/Collections/ProbabilityObjectArray.cs:247-275` as it appears in the config. Always keyed
/// by a string here, so only the data type is generic; the drawing pool it feeds is
/// [`crate::loot::probability_object_array::ProbabilityObjectArray`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbabilityObjectWire<V> {
    pub key: String,
    pub relative_probability: f64,
    pub data: Option<V>,
}

/// `Models/Spt/Config/QuestConfig.cs:622-635`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BossInfo {
    pub is_boss: Option<bool>,
    pub is_pmc: Option<bool>,
}

// ---------------------------------------------------------------------------
// Request / response envelopes
// ---------------------------------------------------------------------------

/// The whole request, the ragfair ABI-13 envelope reused: the stamp the caller's slice was (or
/// would be) projected at, the slice itself on a full send, and the varying half every call
/// carries. [`crate::quest::slice_cache`] answers a slice-less send from what it stored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestNativeRequest {
    /// The caller's `DatabaseMutationStamp.Current`, a C# `long`.
    pub invariant_stamp: i64,
    /// Absent on a slice-less send, where the native side reuses the slice it stored under
    /// [`Self::invariant_stamp`].
    pub invariant: Option<QuestInvariantSlice>,
    pub varying: QuestVaryingRequest,
}

/// The call-invariant half: the database, config and service projections, which only change when
/// the database does.
///
/// The three projections the ragfair slice already carries — the items view, the handbook price
/// table and `templateTable.Prices` — keep ragfair's member names and shapes
/// ([`crate::ragfair::models::InvariantSlice`]) so the C# builder projects them the same way for
/// both families. The rest are this family's own, each projected down to exactly what the C# call
/// sites read.
///
/// Membership-only projections deserialize straight into a [`HashSet`]; ragfair's `PreparedSlice`
/// conversion step exists to do that once per store, which is what deserializing into the set
/// already does.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestInvariantSlice {
    /// The `TemplateItem` table flattened by the C# caller — walked whole by
    /// `RepeatableQuestRewardGenerator.GetRewardableItems` (`:660`) and
    /// `CompletionQuestGenerator.GetItemsToRetrievePool` (`:132`), and read per tpl by every
    /// `ItemHelper` predicate under them.
    pub items: IndexMap<String, ItemView>,
    /// `HandbookHelper.GetTemplatePrice` for the whole items table: the static arm of
    /// `ItemHelper.GetItemPrice` and the cartridge price at
    /// `RepeatableQuestRewardGenerator.cs:411`. `FromRoubles` (`:636`) reads this same map for the
    /// currency tpl (`HandbookHelper.cs:215-225`), so no separate conversion table crosses.
    pub handbook_prices: IndexMap<String, f64>,
    /// `templateTable.Prices` — what `ItemHelper.GetDynamicItemPrice` answers from, the fallback
    /// arm of `GetItemPrice` when the handbook price is below 1 (`ItemHelper.cs:431-440`).
    pub flea_prices: IndexMap<String, f64>,
    /// `PresetHelper.GetDefaultWeaponPresets().Values.ToList()` — the `ExhaustableArray` pool the
    /// weapon-reward path draws from (`RepeatableQuestRewardGenerator.cs:517`).
    pub default_weapon_presets: Vec<PresetView>,
    /// `PresetHelper.GetDefaultPresetOrItemPrice` resolved per tpl (`:365/:426/:440/:502`). It
    /// walks the preset caches `GetDefaultPreset` fills, so it stays a C#-side loop and crosses as
    /// a map.
    pub default_preset_or_item_prices: IndexMap<String, f64>,
    /// `ItemFilterService.GetItemBlacklistCache()` — what `IsItemBlacklisted` (`:710/:713`)
    /// answers from: the `config/item.json` blacklist plus anything added at runtime.
    pub item_blacklist: HashSet<String>,
    /// `ItemFilterService.GetItemRewardBlacklist()` — `IsItemRewardBlacklisted` (`:711`).
    pub reward_item_blacklist: HashSet<String>,
    /// `ItemFilterService.GetBossItems()` — `IsBossItem` (`:726`).
    pub boss_items: HashSet<String>,
    /// `SeasonalEventService.GetInactiveSeasonalEventItems()` (`:655`,
    /// `CompletionQuestGenerator.cs:128`). Ragfair's slice carries it under the same name.
    pub seasonal_item_tpl_blacklist: HashSet<String>,
    /// `TemplateTable.RepeatableQuests.Templates` — the four templates
    /// `GetClonedQuestTemplateForType` clones (`RepeatableQuestHelper.cs:68-76`).
    pub repeatable_quest_templates: RepeatableTemplates,
    /// `TemplateTable.RepeatableQuests.Data.Completion.ItemsWhitelist`
    /// (`CompletionQuestGenerator.cs:190`). Absent, null and empty all take the same C# branch —
    /// the selection comes back unfiltered — so they collapse into one empty list here.
    #[serde(default)]
    pub completion_items_whitelist: Vec<LevelledItemFilter>,
    /// `...ItemsBlacklist` (`CompletionQuestGenerator.cs:224`), same collapse.
    #[serde(default)]
    pub completion_items_blacklist: Vec<LevelledItemFilter>,
    /// Boss names per location, keyed by `LocationBase.Id`. `BossName` is the only member
    /// `EliminationQuestGenerator.cs:485-501` reads off a `BossLocationSpawn`, and the blacklist it
    /// filters the locations with (`DistLocationBlacklist`) is compared against that same key.
    /// Keys are **never lowercased**, unlike `extracts_by_location` — shipped ids are mixed-case
    /// (`Interchange`, `Sandbox_high`) and the C# comparison is ordinal, so the request builder
    /// must not normalise them.
    pub boss_spawns_by_location: IndexMap<String, Vec<String>>,
    /// `LocationBase.AllExtracts` per location, keyed by the lowercased location key the C# looks
    /// up with (`ExplorationQuestGenerator.cs:183`). List order is load-bearing: the exit is drawn
    /// from the filtered list by index (`:279`).
    pub extracts_by_location: IndexMap<String, Vec<ExitView>>,
    /// `QuestConfig.RepeatableQuestTemplates` — the template **ids** by player group
    /// (`RepeatableQuestHelper.cs:187-197`), not the quest templates above.
    pub repeatable_quest_template_ids: RepeatableQuestTemplates,
    /// `QuestConfig.LocationIdMap` — `GetQuestLocationByMapId` (`RepeatableQuestHelper.cs:204`).
    pub location_id_map: IndexMap<String, String>,
}

/// `Models/Eft/Common/Tables/RepeatableQuests.cs:57-70`. The C# member names are the wire names;
/// a missing or null template is the `null` arm of `GetClonedQuestTemplateForType`.
#[derive(Debug, Deserialize)]
pub struct RepeatableTemplates {
    #[serde(rename = "Elimination")]
    pub elimination: Option<RepeatableQuest>,
    #[serde(rename = "Completion")]
    pub completion: Option<RepeatableQuest>,
    #[serde(rename = "Exploration")]
    pub exploration: Option<RepeatableQuest>,
    #[serde(rename = "Pickup")]
    pub pickup: Option<RepeatableQuest>,
}

/// `Models/Eft/Common/Tables/RepeatableQuests.cs:153-169` — C# declares two identical records,
/// `ItemsWhitelist` and `ItemsBlacklist`; one type serves both.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelledItemFilter {
    /// `int?` in the C#, and a null one never passes the `MinPlayerLevel <= pmcLevel` filter
    /// (`CompletionQuestGenerator.cs:202/238`) — a lifted comparison against null is false.
    pub min_player_level: Option<i32>,
    /// `HashSet<MongoId>?`, flattened by the C# `?? []`.
    #[serde(default)]
    pub item_ids: HashSet<String>,
}

/// The members of `Models/Eft/Common/LocationBase.cs:806-883` `Exit` that
/// `ExplorationQuestGenerator` reads: the side filter (`:185`), the spawn-chance and
/// passage-requirement filters (`:263/:267`) and the name it mints the condition from (`:298`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitView {
    pub name: Option<String>,
    pub side: Option<String>,
    /// `double?`; a null one fails the `> 0` filter the way the C# lifted comparison does.
    pub chance: Option<f64>,
    /// `RequirementState` through `JsonStringEnumConverter`, compared as a string against
    /// `SpecificExits.PassageRequirementWhitelist`. Non-nullable in the C#.
    pub passage_requirement: String,
}

/// `Models/Spt/Config/QuestConfig.cs:75-90` `RepeatableQuestTemplates` — template ids keyed by
/// quest type name, one map per player group.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatableQuestTemplates {
    pub pmc: IndexMap<String, String>,
    pub scav: IndexMap<String, String>,
}

/// The members that change every call — everything not projected off the database. The full
/// request ([`QuestNativeRequest`]) wraps this next to the invariant slice.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestVaryingRequest {
    pub quest_type: RepeatableQuestType,
    pub session_id: String,
    pub pmc_level: i32,
    pub trader_id: String,
    pub quest_type_pool: QuestTypePool,
    pub repeatable_config: RepeatableQuestConfig,
    /// Test-only: draws come from a seeded generator when set. Absent in production.
    pub seed: Option<u64>,
}

/// One generated quest and the pool it was drawn from, which the generators mutate. Diagnostics
/// ride the response the way every other export in this crate replays its log lines
/// (`loot::models::StaticContainersResult`, `ragfair::models::DynamicOffersResult`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestNativeResponse {
    /// `null` when the generator gave up — the C# returns `null` from `GenerateRepeatableQuest`
    /// on the same paths.
    pub quest: Option<RepeatableQuest>,
    pub pool: QuestTypePool,
    pub diagnostics: Vec<Diagnostic>,
}

/// `Utils/Json/Converters/StringToNumberFactoryConverter.cs` for the `int?` members that carry it.
fn deserialize_string_or_i32<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(deserialize_string_or_number(deserializer)?.map(|value| value as i32))
}

/// `pub` so `slice_cache`'s tests — and the generator ports after them — build a slice off the
/// same fixture.
#[cfg(test)]
pub mod tests {
    use super::*;

    const TEMPLATES_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/repeatableQuests.json"
    );
    const QUEST_CONFIG_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/configs/quest.json"
    );

    fn database(path: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("SPT_Data file readable"))
            .expect("SPT_Data file is JSON")
    }

    /// The two divergences a C# round trip has too, applied to both sides before comparing:
    ///
    /// * null members are dropped — `JsonUtil`'s options set
    ///   `DefaultIgnoreCondition = WhenWritingNull`, so C# never writes them back either
    ///   (`repeatableQuests.json` ships `"location": null` on two templates);
    /// * numbers are compared as `f64` — a C# `double?` member holding `1` writes `1`, serde
    ///   writes `1.0`, and `serde_json::Value`'s `Eq` distinguishes the two representations.
    fn normalise(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.iter()
                    .filter(|(_, member)| !member.is_null())
                    .map(|(key, member)| (key.clone(), normalise(member)))
                    .collect(),
            ),
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(normalise).collect())
            }
            serde_json::Value::Number(number) => serde_json::json!(number.as_f64()),
            other => other.clone(),
        }
    }

    #[test]
    fn every_repeatable_quest_template_round_trips() {
        let templates = database(TEMPLATES_PATH);
        let templates = templates["templates"]
            .as_object()
            .expect("templates is an object");
        assert_eq!(templates.len(), 4);

        for (name, template) in templates {
            let parsed: RepeatableQuest = serde_json::from_value(template.clone())
                .unwrap_or_else(|error| panic!("{name} template deserializes: {error}"));
            let written = serde_json::to_value(&parsed).expect("serializes");
            assert_eq!(
                normalise(&written),
                normalise(template),
                "{name} template did not survive the round trip"
            );
        }
    }

    #[test]
    fn known_template_members_do_not_fall_into_the_passthrough_map() {
        let templates = database(TEMPLATES_PATH);

        let elimination: RepeatableQuest =
            serde_json::from_value(templates["templates"]["Elimination"].clone()).expect("parses");
        assert!(elimination.quest.extra.is_empty());
        assert!(elimination.change_cost[0].extra.is_empty());
        assert!(elimination.quest_status.as_ref().unwrap().extra.is_empty());

        let conditions = elimination.quest.conditions.available_for_finish.unwrap();
        assert!(conditions[0].extra.is_empty());
        let counter_conditions = conditions[0].counter.as_ref().unwrap();
        assert!(counter_conditions.extra.is_empty());
        assert!(
            counter_conditions.conditions.as_ref().unwrap()[0]
                .extra
                .is_empty()
        );

        // The other three templates carry members the Elimination one does not (`FindItem` and
        // `Equipment` counter conditions, `ExitStatus`/`Location` counters).
        for name in ["Completion", "Exploration", "Pickup"] {
            let quest: RepeatableQuest =
                serde_json::from_value(templates["templates"][name].clone()).expect("parses");
            assert!(quest.quest.extra.is_empty(), "{name}");
            for condition in quest
                .quest
                .conditions
                .available_for_finish
                .into_iter()
                .flatten()
            {
                assert!(condition.extra.is_empty(), "{name}");
                for counter in condition.counter.iter() {
                    for counter_condition in counter.conditions.iter().flatten() {
                        assert!(counter_condition.extra.is_empty(), "{name}");
                    }
                }
            }
        }
    }

    #[test]
    fn mod_added_members_ride_the_passthrough_map() {
        let templates = database(TEMPLATES_PATH);
        let mut template = templates["templates"]["Elimination"].clone();
        template["modAddedQuestMember"] = serde_json::json!(42);

        let parsed: RepeatableQuest = serde_json::from_value(template.clone()).expect("parses");
        assert_eq!(parsed.quest.extra["modAddedQuestMember"], 42);
        assert_eq!(
            normalise(&serde_json::to_value(&parsed).expect("serializes")),
            normalise(&template)
        );
    }

    #[test]
    fn every_repeatable_quest_config_deserializes() {
        let config = database(QUEST_CONFIG_PATH);
        let configs: Vec<RepeatableQuestConfig> =
            serde_json::from_value(config["repeatableQuests"].clone()).expect("parses");

        assert_eq!(configs.len(), 3);
        let daily = &configs[0];
        assert_eq!(daily.name, "Daily");
        assert_eq!(daily.side, "Pmc");
        assert_eq!(daily.types, ["Elimination", "Completion", "Exploration"]);
        assert_eq!(daily.quest_config.elimination[0].targets[0].key, "Savage");
        assert_eq!(
            daily.quest_config.elimination[0].targets[0]
                .data
                .as_ref()
                .unwrap()
                .is_boss,
            Some(false)
        );
        assert_eq!(daily.quest_config.exploration[0].level_range.max, 15);
        assert!(daily.quest_config.pickup.is_none());
        assert!(configs[2].quest_config.pickup.is_some());
    }

    /// One entry per map/table, with the real Elimination template spliced in so the
    /// `repeatableQuestTemplates` keys are pinned against the shipped database.
    pub fn slice_value() -> serde_json::Value {
        let templates = database(TEMPLATES_PATH);

        serde_json::json!({
            "items": {
                "bbbbbbbbbbbbbbbbbbbbbbbb": {
                    "parent": "cccccccccccccccccccccccc", "type": "Item", "stackMaxSize": 1
                }
            },
            "handbookPrices": { "bbbbbbbbbbbbbbbbbbbbbbbb": 20000.0 },
            "fleaPrices": { "bbbbbbbbbbbbbbbbbbbbbbbb": 25000.0 },
            "defaultWeaponPresets": [{
                "items": [{ "_id": "aaaaaaaaaaaaaaaaaaaaaaaa", "_tpl": "bbbbbbbbbbbbbbbbbbbbbbbb" }],
                "id": "preset1", "encyclopedia": "bbbbbbbbbbbbbbbbbbbbbbbb"
            }],
            "defaultPresetOrItemPrices": { "bbbbbbbbbbbbbbbbbbbbbbbb": 21000.0 },
            "itemBlacklist": ["999999999999999999999999"],
            "rewardItemBlacklist": ["888888888888888888888888"],
            "bossItems": ["777777777777777777777777"],
            "seasonalItemTplBlacklist": ["666666666666666666666666"],
            "repeatableQuestTemplates": { "Elimination": templates["templates"]["Elimination"] },
            "completionItemsWhitelist": [
                { "minPlayerLevel": 1, "itemIds": ["bbbbbbbbbbbbbbbbbbbbbbbb"] }
            ],
            "completionItemsBlacklist": [{ "minPlayerLevel": 5 }],
            "bossSpawnsByLocation": { "bigmap": ["bossKilla"], "laboratory": ["bossKilla"] },
            "extractsByLocation": {
                "bigmap": [{
                    "name": "Dorms V-Ex", "side": "Pmc", "chance": 100.0,
                    "passageRequirement": "TransferItem"
                }]
            },
            "repeatableQuestTemplateIds": {
                "pmc": { "Elimination": "616052ea3054fc0e2c24ce6e" },
                "scav": { "Elimination": "62825ef60e88d037dc1eb428" }
            },
            "locationIdMap": { "bigmap": "55f2d3fd4bdc2d5f408b4567" }
        })
    }

    pub fn slice() -> QuestInvariantSlice {
        serde_json::from_value(slice_value()).expect("fixture slice parses")
    }

    #[test]
    fn the_invariant_slice_reads_the_locked_wire_contract() {
        let slice = slice();

        assert_eq!(
            slice.items["bbbbbbbbbbbbbbbbbbbbbbbb"].parent.as_deref(),
            Some("cccccccccccccccccccccccc")
        );
        assert_eq!(slice.handbook_prices["bbbbbbbbbbbbbbbbbbbbbbbb"], 20000.0);
        assert_eq!(slice.flea_prices["bbbbbbbbbbbbbbbbbbbbbbbb"], 25000.0);
        assert_eq!(
            slice.default_weapon_presets[0].encyclopedia.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(
            slice.default_preset_or_item_prices["bbbbbbbbbbbbbbbbbbbbbbbb"],
            21000.0
        );
        assert!(slice.item_blacklist.contains("999999999999999999999999"));
        assert!(
            slice
                .reward_item_blacklist
                .contains("888888888888888888888888")
        );
        assert!(slice.boss_items.contains("777777777777777777777777"));
        assert!(
            slice
                .seasonal_item_tpl_blacklist
                .contains("666666666666666666666666")
        );

        // The template rides in whole, and the three types the fixture omits are the `null` arm
        let elimination = slice
            .repeatable_quest_templates
            .elimination
            .as_ref()
            .expect("elimination template");
        assert_eq!(elimination.quest.id, "68690637c1394a820efc27ca");
        assert!(slice.repeatable_quest_templates.completion.is_none());
        assert!(slice.repeatable_quest_templates.exploration.is_none());
        assert!(slice.repeatable_quest_templates.pickup.is_none());

        assert_eq!(
            slice.completion_items_whitelist[0].min_player_level,
            Some(1)
        );
        assert!(
            slice.completion_items_whitelist[0]
                .item_ids
                .contains("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        // A filter entry with no `itemIds` is the C# `?? []`, not a parse failure
        assert!(slice.completion_items_blacklist[0].item_ids.is_empty());

        assert_eq!(slice.boss_spawns_by_location["bigmap"], ["bossKilla"]);
        let extract = &slice.extracts_by_location["bigmap"][0];
        assert_eq!(extract.name.as_deref(), Some("Dorms V-Ex"));
        assert_eq!(extract.side.as_deref(), Some("Pmc"));
        assert_eq!(extract.chance, Some(100.0));
        assert_eq!(extract.passage_requirement, "TransferItem");

        assert_eq!(
            slice.repeatable_quest_template_ids.pmc["Elimination"],
            "616052ea3054fc0e2c24ce6e"
        );
        assert_eq!(
            slice.repeatable_quest_template_ids.scav["Elimination"],
            "62825ef60e88d037dc1eb428"
        );
        assert_eq!(slice.location_id_map["bigmap"], "55f2d3fd4bdc2d5f408b4567");

        // Every view the generators consult is reachable through the context
        let context = crate::quest::QuestContext::from_slice(&slice);
        assert_eq!(context.items.len(), 1);
        assert_eq!(
            context.location_id_map["bigmap"],
            "55f2d3fd4bdc2d5f408b4567"
        );
    }

    #[test]
    fn the_native_request_carries_the_slice_or_leaves_it_out() {
        let mut request = serde_json::json!({
            "invariantStamp": 12,
            "invariant": slice_value(),
            "varying": varying_value(),
        });

        let parsed: QuestNativeRequest =
            serde_json::from_value(request.clone()).expect("full request parses");
        assert_eq!(parsed.invariant_stamp, 12);
        assert!(parsed.invariant.is_some());
        assert_eq!(parsed.varying.quest_type, RepeatableQuestType::Elimination);

        // A slice-less send drops the member entirely
        request.as_object_mut().unwrap().remove("invariant");
        let parsed: QuestNativeRequest =
            serde_json::from_value(request).expect("slice-less request parses");
        assert!(parsed.invariant.is_none());
    }

    /// The varying half of a request, as the locked contract spells it.
    fn varying_value() -> serde_json::Value {
        let config = database(QUEST_CONFIG_PATH);

        serde_json::json!({
            "questType": "Elimination",
            "sessionId": "6193a720f8ee7e52e4290000",
            "pmcLevel": 20,
            "traderId": "54cb50c76803fa8b248b4571",
            "questTypePool": {
                "types": ["Elimination"],
                "pool": {
                    "Exploration": { "locations": { "bigmap": ["bigmap"] } },
                    "Elimination": { "targets": { "Savage": { "locations": ["any"] } } },
                    "Pickup": { "locations": {} }
                }
            },
            "repeatableConfig": config["repeatableQuests"][0],
        })
    }

    #[test]
    fn the_varying_request_reads_the_locked_wire_contract() {
        let templates = database(TEMPLATES_PATH);
        let parsed: QuestVaryingRequest = serde_json::from_value(varying_value()).expect("parses");
        assert_eq!(parsed.quest_type, RepeatableQuestType::Elimination);
        assert_eq!(parsed.pmc_level, 20);
        assert_eq!(parsed.seed, None);
        let targets = parsed
            .quest_type_pool
            .pool
            .elimination
            .targets
            .as_ref()
            .expect("elimination targets");
        assert_eq!(
            targets["Savage"].locations.as_deref(),
            Some(&["any".to_owned()][..])
        );

        let response = QuestNativeResponse {
            quest: Some(
                serde_json::from_value(templates["templates"]["Elimination"].clone())
                    .expect("parses"),
            ),
            pool: parsed.quest_type_pool,
            diagnostics: Vec::new(),
        };
        let written = serde_json::to_value(&response).expect("serializes");
        assert_eq!(written["quest"]["_id"], "68690637c1394a820efc27ca");
        assert_eq!(written["pool"]["types"][0], "Elimination");
        assert!(written["diagnostics"].as_array().unwrap().is_empty());
    }
}
