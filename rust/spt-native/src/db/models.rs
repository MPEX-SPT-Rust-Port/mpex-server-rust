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

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bot::durability_limits_helper::BotDurability;
use crate::bot::models::{PmcConfigWire, RandomisedResourceDetails, WalletLootSettingsWire};
use crate::bot::repair_service::{BonusSettings, MinMax};
use crate::loot::models::{
    Item, SpawnpointTemplate, StaticContainer, StaticContainerData, StaticForced, StaticLootDetails,
};
use crate::quest::models::{LevelledItemFilter, RepeatableQuestTemplates, RepeatableTemplates};
use crate::ragfair::models::DynamicConfigWire;
use crate::scav_case::models::ScavCaseConfigView;

/// `{"schema":1,"roots":{...}}` — the envelope `DbPayloadProjection` (C#) writes.
#[derive(Debug, Deserialize, Serialize)]
pub struct PublishRequest {
    pub schema: u32,
    pub roots: PublishRoots,
}

/// Every root optional: an absent root keeps the currently-resident one. Unknown root names are
/// a parse error (`deny_unknown_fields`), surfacing as `STATUS_BAD_ARGS` — C# and Rust ship in
/// lockstep, so a typo should fail loudly, not silently install nothing.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRoots {
    pub templates: Option<TemplatesRoot>,
    pub traders: Option<TradersRoot>,
    pub globals: Option<GlobalsRoot>,
    pub locations: Option<LocationsRoot>,
    pub hideout: Option<HideoutRoot>,
    pub configs: Option<ConfigsRoot>,
}

/// The `configs` root: SPT_Data/configs projected from the live C# singletons, keyed by each
/// config's `kind` (`BaseConfig.Kind`, `[JsonPropertyName("kind")]` — `"spt-ragfair"`, …).
/// Typed stems are lifted only where a family reads them (Tasks 5-10); everything else rides
/// `extra` full-fidelity. An absent stem is `None` — the consuming family fails its resolve
/// loudly, per call; a present-but-malformed stem fails the whole publish parse
/// (STATUS_BAD_ARGS), previous resident DB intact.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ConfigsRoot {
    /// `Models/Spt/Config/ItemConfig.cs`, whose `Kind` is `spt-item` (`ItemConfig.cs:9-10`) — see
    /// [`ItemConfigLift`].
    #[serde(default, rename = "spt-item")]
    pub item: Option<ItemConfigLift>,
    /// `Models/Spt/Config/ScavCaseConfig.cs`, whose `Kind` is `spt-scavcase`
    /// (`ScavCaseConfig.cs:8-9`). Parsed by the scav case family's own view of the config — the
    /// same type the override bundle carries, so both arms read one parse of one shape. The
    /// members it omits (`kind`, `ammoRewards.ammoRewardBlacklist`, the dispatch/residency flags,
    /// anything Ceciler's `[JsonExtensionData]` adds on a Release build) are ignored on arrival.
    #[serde(default, rename = "spt-scavcase")]
    pub scavcase: Option<ScavCaseConfigView>,
    /// `Models/Spt/Config/RagfairConfig.cs`, whose `Kind` is `spt-ragfair`
    /// (`RagfairConfig.cs:8-9`) — see [`RagfairConfigLift`].
    #[serde(default, rename = "spt-ragfair")]
    pub ragfair: Option<RagfairConfigLift>,
    /// `Models/Spt/Config/InventoryConfig.cs`, whose `Kind` is `spt-inventory`
    /// (`InventoryConfig.cs:8-9`) — see [`InventoryConfigLift`].
    #[serde(default, rename = "spt-inventory")]
    pub inventory: Option<InventoryConfigLift>,
    /// `Models/Spt/Config/QuestConfig.cs`, whose `Kind` is `spt-quest` (`QuestConfig.cs:11-12`) —
    /// see [`QuestConfigLift`].
    #[serde(default, rename = "spt-quest")]
    pub quest: Option<QuestConfigLift>,
    /// `Models/Spt/Config/LocationConfig.cs`, whose `Kind` is `spt-location`
    /// (`LocationConfig.cs:9-10`) — see [`LocationConfigLift`].
    #[serde(default, rename = "spt-location")]
    pub location: Option<LocationConfigLift>,
    /// `Models/Spt/Config/SeasonalEventConfig.cs`, whose `Kind` is `spt-seasonalevents`
    /// (`SeasonalEventConfig.cs:11-12`) — see [`SeasonalEventConfigLift`].
    #[serde(default, rename = "spt-seasonalevents")]
    pub seasonalevents: Option<SeasonalEventConfigLift>,
    /// `Models/Spt/Config/BotConfig.cs`, whose `Kind` is `spt-bot` (`BotConfig.cs:10-11`) — see
    /// [`BotConfigLift`].
    #[serde(default, rename = "spt-bot")]
    pub bot: Option<BotConfigLift>,
    /// `Models/Spt/Config/PmcConfig.cs`, whose `Kind` is `spt-pmc` (`PmcConfig.cs:10-11`). Parsed
    /// by the bot family's own narrowed view of the config — the same [`PmcConfigWire`] the
    /// override bundle carries, so both arms read one parse of one shape (the
    /// [`ScavCaseConfigView`] precedent). The members it omits are ignored on arrival.
    #[serde(default, rename = "spt-pmc")]
    pub pmc: Option<PmcConfigWire>,
    /// `Models/Spt/Config/RepairConfig.cs`, whose `Kind` is `spt-repair`
    /// (`RepairConfig.cs:8-9`) — see [`RepairConfigLift`].
    #[serde(default, rename = "spt-repair")]
    pub repair: Option<RepairConfigLift>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/BotConfig.cs` — the nine members bot generation reads that no runtime writer
/// touches.
///
/// `equipment` is deliberately **not** lifted, though `BuildSharedVarying` reads it:
/// `BotInventoryGenerator.ReplayRandomisationClamps` (`:405-432`) writes the nighttime mod-chance
/// clamps back into `Equipment[role].Randomisation[band].EquipmentMods` through the dictionary
/// indexer after *every* native single-bot send, which trips no write barrier and so never moves
/// the mutation stamp. That write is a cross-bot feedback loop (the next bot's C# prelude reads it
/// at `BotEquipmentFilterService.cs:63`), so a resident copy would freeze at the published values
/// and diverge from bot 2 on. It rides the request instead
/// ([`crate::bot::models::SharedBotVaryingWire::equipment`]) and lands in this lift's
/// [`Self::extra`], unread.
///
/// Strictness per member follows the C# `required`: every member below is `required`
/// (`BotConfig.cs:57,63,69,76,83,130,135,140,146`) except `secureContainerAmmoStackCount`, a plain
/// auto-property C# fills with the type's default. Everything else — the four dispatch flags, the
/// brain types, the caps, whatever Ceciler's `[JsonExtensionData]` adds on a Release build — rides
/// [`Self::extra`].
#[derive(Debug, Deserialize, Serialize)]
pub struct BotConfigLift {
    /// `BotConfig.Bosses` — scanned case-insensitively by `BotHelper.IsBotBoss`, so source order
    /// is irrelevant but the `List<string>` shape is mirrored anyway.
    pub bosses: Vec<String>,
    pub durability: BotDurability,
    /// Bot role → item tpl → max count. Keyed lookups plus one `["pmc"]`/`["default"]` fallback
    /// (`BotLootGenerator.cs:876-881`); the inner map is cloned and zeroed per bot, so its order
    /// is the running total's order.
    #[serde(rename = "itemSpawnLimits")]
    pub item_spawn_limits: IndexMap<String, IndexMap<String, f64>>,
    #[serde(rename = "walletLoot")]
    pub wallet_loot: WalletLootSettingsWire,
    /// Bot role → currency → stack size → weight. The innermost map is what `GetWeightedValue`
    /// scans, so every level stays ordered.
    #[serde(rename = "currencyStackSize")]
    pub currency_stack_size: IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
    #[serde(default, rename = "secureContainerAmmoStackCount")]
    pub secure_container_ammo_stack_count: i32,
    #[serde(rename = "disableLootOnBotTypes")]
    pub disable_loot_on_bot_types: HashSet<String>,
    /// `HashSet<MongoId>` in C#; membership only (`BotEquipmentModGenerator.cs:1079,1088`).
    #[serde(rename = "lowProfileGasBlockTpls")]
    pub low_profile_gas_block_tpls: HashSet<String>,
    /// Keyed by the *raw* bot role — `BotGeneratorHelper.cs:63` looks it up verbatim, with no
    /// equipment-role mapping.
    #[serde(rename = "lootItemResourceRandomization")]
    pub loot_item_resource_randomization: IndexMap<String, RandomisedResourceDetails>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/RepairConfig.cs` — `repairKit.weapon` only, the one `BonusSettings` bot
/// generation passes to [`crate::bot::repair_service::add_buff`]
/// (`BotWeaponGenerator.cs:173`). `RepairKit` and `RepairKit.Weapon` are both `required`
/// (`RepairConfig.cs:29-30,84-85`), so both are strict here; the armor/vest/headwear kits and every
/// other member ride the two `extra` maps.
#[derive(Debug, Deserialize, Serialize)]
pub struct RepairConfigLift {
    #[serde(rename = "repairKit")]
    pub repair_kit: RepairKitLift,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `RepairConfig.cs:78-92` `RepairKit`, narrowed to `weapon`.
#[derive(Debug, Deserialize, Serialize)]
pub struct RepairKitLift {
    pub weapon: BonusSettings,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/ItemConfig.cs` — the four sets the reward families read. Shared: the scav
/// case and repeatable-quest families read `rewardItemBlacklist`/`bossItems`, the ragfair family
/// reads `blacklist`.
///
/// **Deliberately soft.** All four members are `required` in C#
/// (`ItemConfig.cs:16,28,34,40`), so the rule the other lifts follow — mirror the C# `required`, as
/// [`RagfairConfigLift::dynamic`] does — would make every one of them strict. They carry
/// `#[serde(default)]` anyway because `spt-item` is the one stem five families share (scav case,
/// repeatable quest, ragfair, bot, reward loot), and their fixtures publish it *partially* on
/// purpose: `scav_case/mod.rs:391` and `quest/mod.rs:295` publish a bare `{"bossItems": [...]}` to
/// prove that the absence of the **sibling** stem is what fails, and the bot family's filler
/// (`bot/mod.rs:483`) publishes `{"kind": …, "blacklist": [...]}`, the one member flip #6's cases
/// read. Strict members would turn each of those into a publish failure and make the fixtures
/// assert the wrong thing.
///
/// The consequence to know: a *present but partial* `spt-item` stem yields empty sets, so a family
/// reading it silently stops filtering rather than failing loudly. Unreachable from the shipped
/// projection — C# `required` members always serialize, so every stem the server publishes carries
/// all four — but a hand-built or mod-rewritten stem could fall into it.
///
/// Two other lifts stay soft-despite-`required`: [`InventoryConfigLift::custom_money_tpls`] (an
/// absent member is a valid empty set — its doc has the reasoning) and the whole `spt-pmc` stem,
/// which parses as the override wire's soft [`PmcConfigWire`](crate::bot::models::PmcConfigWire)
/// (its doc has the trade). `phase4_configs_root.rs` pins every soft member's wire name against the
/// projected dump, since a drifted name on a soft member parses fine and silently reads empty.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ItemConfigLift {
    /// `ItemConfig.Blacklist` (`ItemConfig.cs:14-15`) — what `ItemFilterService.GetBlacklistedItems`
    /// returns verbatim (`ItemFilterService.cs:51-54`). A `HashSet` rather than an `IndexSet`
    /// because its only reader is `loot::item_helper::is_valid_item`, whose blacklist parameter is
    /// shared with the loot family: membership only, so order cannot be observed.
    #[serde(default)]
    pub blacklist: HashSet<String>,
    #[serde(default, rename = "rewardItemBlacklist")]
    pub reward_item_blacklist: IndexSet<String>,
    #[serde(default, rename = "rewardItemTypeBlacklist")]
    pub reward_item_type_blacklist: IndexSet<String>,
    #[serde(default, rename = "bossItems")]
    pub boss_items: IndexSet<String>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/RagfairConfig.cs` — `dynamic` only (`RagfairConfig.cs:29-30`), the same
/// [`DynamicConfigWire`] the override bundle carries, so both arms read one parse of one shape.
/// Deliberately strict: no `#[serde(default)]` on `dynamic`, so a `spt-ragfair` stem that arrives
/// without it — or with a malformed one — fails the whole publish rather than handing the offer
/// path a config it would have to invent values for. Every other `RagfairConfig` member (and
/// whatever Ceciler's `[JsonExtensionData]` adds on a Release build) rides [`Self::extra`].
#[derive(Debug, Deserialize, Serialize)]
pub struct RagfairConfigLift {
    pub dynamic: DynamicConfigWire,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/InventoryConfig.cs` — `customMoneyTpls` only
/// (`InventoryConfig.cs:20-21`), the mod-added currencies `PaymentHelper.IsMoneyTpl` unions onto
/// the four `Money` constants (`PaymentHelper.cs:19-33`). `#[serde(default)]`, unlike
/// [`RagfairConfigLift::dynamic`]: no stock configuration carries a custom currency, and an empty
/// set is exactly what the ragfair path did before the lift, so an absent member is not a failure.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct InventoryConfigLift {
    /// An `IndexSet` because the C# member is a `List<MongoId>` and this is membership-only; a
    /// duplicate entry would have been a duplicate `HashSet.Add` on the C# side too.
    #[serde(default, rename = "customMoneyTpls")]
    pub custom_money_tpls: IndexSet<String>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// Hideout root: `production.scavRecipes` only (flip #5) — locations-root
/// partial-projection precedent. Wire names pin to the C# `JsonPropertyName`s
/// (HideoutTable.cs / HideoutProduction.cs): note capitalized Common/Rare/Superrare.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct HideoutRoot {
    #[serde(default)]
    pub production: HideoutProductionRoot,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct HideoutProductionRoot {
    #[serde(default, rename = "scavRecipes")]
    pub scav_recipes: Vec<DbScavRecipe>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct DbScavRecipe {
    #[serde(default, rename = "_id")]
    pub id: String,
    #[serde(default, rename = "endProducts")]
    pub end_products: Option<DbEndProducts>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct DbEndProducts {
    #[serde(default, rename = "Common")]
    pub common: Option<MinMax<i32>>,
    #[serde(default, rename = "Rare")]
    pub rare: Option<MinMax<i32>>,
    #[serde(default, rename = "Superrare")]
    pub superrare: Option<MinMax<i32>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/TemplateTable.cs` — only the members the ragfair view derivation and the
/// repeatable-quest flip read are typed; everything else rides in `extra`.
#[derive(Debug, Default, Deserialize, Serialize)]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/Tables/RepeatableQuests.cs:21-34` `RepeatableQuestDatabase` — only the
/// members the repeatable-quest generators read (`Templates`, `Data`) are typed; `rewards` and
/// `samples` ride in `extra`.
#[derive(Debug, Deserialize, Serialize)]
pub struct RepeatableQuestsWire {
    pub templates: Option<RepeatableTemplates>,
    pub data: Option<RepeatableQuestsData>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `RepeatableQuests.cs:138-141` `Options`, the `Data` member's type.
#[derive(Debug, Deserialize, Serialize)]
pub struct RepeatableQuestsData {
    #[serde(rename = "Completion")]
    pub completion: Option<CompletionFilter>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `RepeatableQuests.cs:144-150` `CompletionFilter`. C# declares both lists nullable; absent and
/// null collapse to empty here, the same branch the C# takes for both (`quest::models` documents
/// the same collapse on its slice members).
#[derive(Debug, Deserialize, Serialize)]
pub struct CompletionFilter {
    #[serde(rename = "itemsWhitelist", default)]
    pub items_whitelist: Vec<LevelledItemFilter>,
    #[serde(rename = "itemsBlacklist", default)]
    pub items_blacklist: Vec<LevelledItemFilter>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/TradersTable.cs:6` — the root *is* `Dictionary<MongoId, Trader>`, so the
/// flatten map replaces a named `extra` wholesale: every key is a trader id. [`TraderEntry`]
/// keeps the parse total over values no C# `Trader` could have loaded (the store tests publish
/// `{"b":2}`); those ride through as raw JSON, exactly as full-fidelity as before the lift.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct TradersRoot {
    #[serde(flatten)]
    pub traders: IndexMap<String, TraderEntry>,
}

/// One value of the traders dictionary root. Untagged: any JSON object parses as [`Trader`]
/// (whose members are all optional), anything else falls through to raw JSON.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum TraderEntry {
    Trader(Box<Trader>),
    /// Not trader-shaped. `ragfair::views::derive` skips it — the C# `TradersTable` could never
    /// have deserialized it in the first place.
    Other(Value),
}

/// `Models/Eft/Common/Tables/Trader.cs:9-28` — only `base` is typed.
#[derive(Debug, Deserialize, Serialize)]
pub struct Trader {
    /// `required` in C#; `Option` here so the parse stays total. A base-less trader prices like
    /// one with no loyalty levels — unobservable, C# throws at database load before it could
    /// ever publish one.
    #[serde(rename = "base")]
    pub base: Option<TraderBase>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Trader.cs:30-180` `TraderBase` — only `loyaltyLevels` is typed.
#[derive(Debug, Deserialize, Serialize)]
pub struct TraderBase {
    #[serde(rename = "loyaltyLevels")]
    pub loyalty_levels: Option<Vec<TraderLoyaltyLevel>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Trader.cs:182-…` `TraderLoyaltyLevel` — only `buy_price_coef` is read.
#[derive(Debug, Deserialize, Serialize)]
pub struct TraderLoyaltyLevel {
    #[serde(rename = "buy_price_coef")]
    pub buy_price_coef: Option<f64>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/GlobalTable.cs:10-26` — only `ItemPresets` and the `config` lift are typed;
/// everything else rides [`Self::extra`].
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct GlobalsRoot {
    /// `GlobalTable.ItemPresets` (`GlobalTable.cs:24-25`), keyed by preset id — that key domain
    /// (the map's keys, not each preset's `_id`) is what `PresetHelper.IsPreset`/`GetPreset`
    /// answer from.
    #[serde(rename = "ItemPresets", default)]
    pub item_presets: IndexMap<String, Preset>,
    /// `GlobalTable.Configuration` (`GlobalTable.cs:12-13`) — see [`GlobalsConfigLift`].
    #[serde(default, rename = "config")]
    pub config: GlobalsConfigLift,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// Globals `config` lift: `exp.level.exp_table` only (flip #6) — everything else
/// rides `extra`. Wire names pin to GlobalTable.cs (`config`/`exp`/`level`/`exp_table`,
/// entries `{"exp": n}` — GlobalTable.cs:12, :299, :1166, :1290, :1311).
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct GlobalsConfigLift {
    #[serde(default)]
    pub exp: GlobalsExpLift,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct GlobalsExpLift {
    #[serde(default)]
    pub level: GlobalsExpLevelLift,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct GlobalsExpLevelLift {
    #[serde(default, rename = "exp_table")]
    pub exp_table: Vec<ExpTableEntry>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ExpTableEntry {
    #[serde(default)]
    pub exp: i32,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/GlobalTable.cs:4397-4422` `Preset`.
#[derive(Debug, Deserialize, Serialize)]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/QuestConfig.cs` — the two maps `RepeatableQuestHelper` reads off the config
/// rather than the database. Both C# members are `required`
/// (`QuestConfig.cs:44-48`, `:67-71`), so a stem missing one could not have come from a live
/// `QuestConfig`: strict like [`RagfairConfigLift::dynamic`], rather than defaulting to an empty
/// map the helper would then fail every lookup against. `repeatableQuests`, the dispatch flags and
/// whatever Ceciler's `[JsonExtensionData]` adds on a Release build ride [`Self::extra`].
#[derive(Debug, Deserialize, Serialize)]
pub struct QuestConfigLift {
    /// `QuestConfig.RepeatableQuestTemplates`, whose wire name is `repeatableQuestTemplateIds` —
    /// the template **ids** by player group (`RepeatableQuestHelper.cs:187-197`), not the quest
    /// templates the views carry. Named for the wire, as the request member it replaces was.
    #[serde(rename = "repeatableQuestTemplateIds")]
    pub repeatable_quest_template_ids: RepeatableQuestTemplates,
    /// `QuestConfig.LocationIdMap` — `GetQuestLocationByMapId` (`RepeatableQuestHelper.cs:204`).
    #[serde(rename = "locationIdMap")]
    pub location_id_map: IndexMap<String, String>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/LocationConfig.cs` — the *source* members `BuildConfigView`
/// (`LocationLootGenerator.cs:459-482`) reads, raw and un-resolved: the two per-location
/// resolutions left to the resident arm are done per call by
/// `loot::location_loot_generator::resolve_loot_config_view` instead, so the map-keyed members are
/// lifted whole.
///
/// `staticLootMultiplier`/`looseLootMultiplier` are deliberately **not** lifted, though
/// `BuildConfigView` reads them: `RaidTimeAdjustmentService.AdjustLootMultipliers` scales those two
/// dictionaries in place through the indexer for a shortened scav raid, which trips no write
/// barrier and so never moves the mutation stamp. They ride the request instead
/// (`LootVarying::static_loot_multiplier`) and land in this root's [`Self::extra`], unread.
///
/// Strictness per member follows the C# `required`: the two maps and the two settings objects are
/// `required` (`LocationConfig.cs:101-102,134-135,146-147,158-159`) and so are strict here; the
/// loose scalars are plain auto-properties, which C# fills with the type's default, and default
/// here too. Every other member (the dispatch flags, the waves, whatever Ceciler's
/// `[JsonExtensionData]` adds on a Release build) rides [`Self::extra`].
#[derive(Debug, Deserialize, Serialize)]
pub struct LocationConfigLift {
    #[serde(rename = "containerRandomisationSettings")]
    pub container_randomisation_settings: ContainerRandomisationSettingsLift,
    #[serde(default, rename = "allowDuplicateItemsInStaticContainers")]
    pub allow_duplicate_items_in_static_containers: bool,
    /// `HashSet<MongoId>` in C#; membership only on both sides of the flip, so the order is unread.
    #[serde(rename = "tplsToStripChildItemsFrom")]
    pub tpls_to_strip_child_items_from: HashSet<String>,
    #[serde(default, rename = "fitLootIntoContainerAttempts")]
    pub fit_loot_into_container_attempts: i32,
    /// `int` in C#, `double` on the view — the widening the C# member assignment does implicitly.
    #[serde(default, rename = "magazineLootHasAmmoChancePercent")]
    pub magazine_loot_has_ammo_chance_percent: i32,
    /// See [`Self::magazine_loot_has_ammo_chance_percent`].
    #[serde(default, rename = "staticMagazineLootHasAmmoChancePercent")]
    pub static_magazine_loot_has_ammo_chance_percent: i32,
    /// See [`Self::magazine_loot_has_ammo_chance_percent`].
    #[serde(default, rename = "minFillLooseMagazinePercent")]
    pub min_fill_loose_magazine_percent: i32,
    /// See [`Self::magazine_loot_has_ammo_chance_percent`].
    #[serde(default, rename = "minFillStaticMagazinePercent")]
    pub min_fill_static_magazine_percent: i32,
    #[serde(rename = "equipmentLootSettings")]
    pub equipment_loot_settings: EquipmentLootSettingsLift,
    /// Keyed by map id; the value is that map's blacklisted loose-loot spawn point ids. A map with
    /// no entry blacklists nothing (`LocationLootGenerator.cs:480`).
    #[serde(rename = "looseLootBlacklist")]
    pub loose_loot_blacklist: HashMap<String, HashSet<String>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `LocationConfig.cs:228-250` `ContainerRandomisationSettings`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ContainerRandomisationSettingsLift {
    #[serde(default)]
    pub enabled: bool,
    /// The maps container randomisation is allowed on. Only `ContainsKey` is ever asked
    /// (`LocationLootGenerator.cs:466`), so the values ride along unread and the order is unread.
    pub maps: HashMap<String, bool>,
    #[serde(rename = "containerTypesToNotRandomise")]
    pub container_types_to_not_randomise: HashSet<String>,
    #[serde(default, rename = "containerGroupMinSizeMultiplier")]
    pub container_group_min_size_multiplier: f64,
    #[serde(default, rename = "containerGroupMaxSizeMultiplier")]
    pub container_group_max_size_multiplier: f64,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `LocationConfig.cs:192-199` `EquipmentLootSettings`.
#[derive(Debug, Deserialize, Serialize)]
pub struct EquipmentLootSettingsLift {
    /// Keyed by slot name; keyed lookups only, so the order is unread.
    #[serde(rename = "modSpawnChancePercent")]
    pub mod_spawn_chance_percent: HashMap<String, f64>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Config/SeasonalEventConfig.cs` — `christmasContainerIds` only
/// (`SeasonalEventConfig.cs:56-57`), the one member of that config the loot family reads off the
/// config rather than through `SeasonalEventService`. `required` in C#, so strict here; the whole
/// rest of the config rides [`Self::extra`].
#[derive(Debug, Deserialize, Serialize)]
pub struct SeasonalEventConfigLift {
    /// Spawn point ids, not tpls. Membership only — the christmas-container filter in
    /// `loot::location_loot_generator::generate_static_containers` is the sole reader.
    #[serde(rename = "christmasContainerIds")]
    pub christmas_container_ids: HashSet<String>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Spt/Tables/LocationTable.cs` — the root is the serialized table: one key per map
/// (`bigmap`, `factory4_day`, …) plus the UI-linkage `base` (`LocationTable.cs:117-118`), so the
/// flatten map replaces a named `extra` wholesale, the same shape as [`TradersRoot`].
/// `GetDictionary()` re-keys by C# property name at read time; the wire keys here are the
/// `JsonPropertyName`s `GetLocation` falls back to.
#[derive(Debug, Default, Deserialize, Serialize)]
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
#[derive(Debug, Deserialize, Serialize)]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `StaticContainerDetails` (`Location.cs:106-116`). All three `Option`: the C# members are
/// nullable `IEnumerable`s and the generator logs a map-specific error per missing list.
#[derive(Debug, Deserialize, Serialize)]
pub struct StaticContainerDetailsWire {
    #[serde(rename = "staticWeapons")]
    pub static_weapons: Option<Vec<SpawnpointTemplate>>,
    #[serde(rename = "staticContainers")]
    pub static_containers: Option<Vec<StaticContainerData>>,
    #[serde(rename = "staticForced")]
    pub static_forced: Option<Vec<StaticForced>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/LocationBase.cs:8` `LocationBase` — only the two members
/// `BuildBossSpawnsByLocation` reads (`RepeatableQuestNativeRequestBuilder.cs:195-206`).
#[derive(Debug, Deserialize, Serialize)]
pub struct LocationBaseView {
    /// `required string` in C# (`LocationBase.cs:181-182`); `Option` so the parse stays total —
    /// the builder's `Base?.Id is not { }` skip is the branch a missing id takes.
    #[serde(rename = "Id")]
    pub id: Option<String>,
    #[serde(rename = "BossLocationSpawn", default)]
    pub boss_location_spawn: Vec<BossSpawnView>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `LocationBase.cs:501` `BossLocationSpawn` — `BossName` is the only member the elimination
/// projection reads off a spawn.
#[derive(Debug, Deserialize, Serialize)]
pub struct BossSpawnView {
    #[serde(rename = "BossName")]
    pub boss_name: Option<String>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `LocationBase.cs:806-883` `Exit`, restricted to the four members `ToExitView` reads
/// (`RepeatableQuestNativeRequestBuilder.cs:238-247`). `AllExtracts` entries are the derived
/// `AllExtractsExit : Exit`; its `SptName` addition rides in `extra`.
#[derive(Debug, Deserialize, Serialize)]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/Tables/HandbookBase.cs:6-13` — `Categories` is unread by the ragfair
/// derivation and rides in `extra`.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct HandbookBase {
    #[serde(rename = "Items", default)]
    pub items: Vec<HandbookItem>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `HandbookBase.cs:35-46` `HandbookItem`.
#[derive(Debug, Deserialize, Serialize)]
pub struct HandbookItem {
    #[serde(rename = "Id", default)]
    pub id: String,
    #[serde(rename = "Price")]
    pub price: Option<f64>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `Models/Eft/Common/Tables/TemplateItem.cs:12-38` — the members
/// `PayloadProjection.BuildItemsView` reads, plus the flatten superset.
#[derive(Debug, Deserialize, Serialize)]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs` `TemplateItemProperties`, restricted to what `BuildItemsView` reads.
/// C# `HashSet<MongoId>`/`HashSet<string>` members are `IndexSet` here: a .NET `HashSet` built
/// by deserializing a JSON array keeps that array's order and drops later duplicates, which is
/// exactly `IndexSet`'s first-wins insertion order.
#[derive(Debug, Deserialize, Serialize)]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1640-1658` `Grid`.
#[derive(Debug, Deserialize, Serialize)]
pub struct Grid {
    #[serde(rename = "_name")]
    pub name: Option<String>,
    #[serde(rename = "_props")]
    pub properties: Option<GridProperties>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1660-1683` `GridProperties`.
#[derive(Debug, Deserialize, Serialize)]
pub struct GridProperties {
    #[serde(rename = "filters")]
    pub filters: Option<Vec<GridFilter>>,
    #[serde(rename = "cellsH")]
    pub cells_h: Option<i32>,
    #[serde(rename = "cellsV")]
    pub cells_v: Option<i32>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1685-1696` `GridFilter`.
#[derive(Debug, Deserialize, Serialize)]
pub struct GridFilter {
    #[serde(rename = "Filter")]
    pub filter: Option<IndexSet<String>>,
    #[serde(rename = "ExcludedFilter")]
    pub excluded_filter: Option<IndexSet<String>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1698-1740` `Slot` — `Cartridges` and `Chambers` are `Slot` lists too.
#[derive(Debug, Deserialize, Serialize)]
pub struct Slot {
    #[serde(rename = "_name")]
    pub name: Option<String>,
    #[serde(rename = "_props")]
    pub properties: Option<SlotProperties>,
    #[serde(rename = "_max_count")]
    pub max_count: Option<f64>,
    #[serde(rename = "_required")]
    pub required: Option<bool>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1742-1749` `SlotProperties`.
#[derive(Debug, Deserialize, Serialize)]
pub struct SlotProperties {
    #[serde(rename = "filters")]
    pub filters: Option<Vec<SlotFilter>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1751-1767` `SlotFilter` — shared by `Slot` and `StackSlot` properties.
#[derive(Debug, Deserialize, Serialize)]
pub struct SlotFilter {
    #[serde(rename = "Plate")]
    pub plate: Option<String>,
    #[serde(rename = "Filter")]
    pub filter: Option<IndexSet<String>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1769-1791` `StackSlot`.
#[derive(Debug, Deserialize, Serialize)]
pub struct StackSlot {
    #[serde(rename = "_max_count")]
    pub max_count: Option<f64>,
    #[serde(rename = "_props")]
    pub properties: Option<StackSlotProperties>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: IndexMap<String, Value>,
}

/// `TemplateItem.cs:1793-1797` `StackSlotProperties`.
#[derive(Debug, Deserialize, Serialize)]
pub struct StackSlotProperties {
    #[serde(rename = "filters")]
    pub filters: Option<Vec<SlotFilter>>,
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
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

    /// The four stems the reward and ragfair families read come out of the flatten map as typed
    /// values, every other kind still rides `extra`, and the members the views omit — `kind`, the
    /// dispatch flags, `ammoRewardBlacklist`, every `RagfairConfig`/`InventoryConfig` member but
    /// one, whatever Ceciler's `[JsonExtensionData]` adds on a Release build — are ignored rather
    /// than failing the parse.
    #[test]
    fn configs_root_lifts_the_typed_stems_and_keeps_the_rest() {
        let root: ConfigsRoot = serde_json::from_str(
            &(r#"{
                "spt-item": {"kind": "spt-item", "blacklist": ["b1"],
                    "rewardItemBlacklist": ["r1"], "rewardItemTypeBlacklist": ["t1"],
                    "bossItems": ["boss1"], "handbookPriceOverride": {},
                    "somethingCecilerAdded": 7},
                "spt-scavcase": {"kind": "spt-scavcase",
                    "rewardItemValueRangeRub": {"common": {"min": 0.0, "max": 1.0}},
                    "moneyRewards": {"moneyRewardChancePercent": 5,
                        "rubCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                            "superrare": {"min": 1, "max": 1}},
                        "usdCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                            "superrare": {"min": 1, "max": 1}},
                        "eurCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                            "superrare": {"min": 1, "max": 1}},
                        "gpCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                            "superrare": {"min": 1, "max": 1}}},
                    "ammoRewards": {"ammoRewardChancePercent": 5, "ammoRewardBlacklist": {},
                        "ammoRewardValueRangeRub": {}, "minStackSize": 30},
                    "rewardItemParentBlacklist": [], "rewardItemBlacklist": ["config_r1"],
                    "allowMultipleMoneyRewardsPerRarity": false,
                    "allowMultipleAmmoRewardsPerRarity": true,
                    "allowBossItemsAsRewards": false,
                    "forceLegacyScavCaseGeneration": false},
                "spt-core": {"kind": "spt-core"},
                "spt-quest": {"kind": "spt-quest",
                    "repeatableQuestTemplateIds": {"pmc": {"Elimination": "pmc_elim"},
                        "scav": {"Elimination": "scav_elim"}},
                    "locationIdMap": {"bigmap": "55f2d3fd4bdc2d5f408b4567"},
                    "repeatableQuests": []},
                "spt-inventory": {"kind": "spt-inventory", "customMoneyTpls": ["custom_money"],
                    "randomLootContainers": {}},
                "spt-ragfair": {"kind": "spt-ragfair", "runIntervalSeconds": 450,
                    "dynamic": "#
                .to_owned()
                + crate::ragfair::models::tests::DYNAMIC_JSON
                + r#"}
            }"#),
        )
        .unwrap();

        let item = root.item.as_ref().unwrap();
        assert!(item.blacklist.contains("b1"));
        assert!(item.reward_item_blacklist.contains("r1"));
        assert!(item.reward_item_type_blacklist.contains("t1"));
        assert!(item.boss_items.contains("boss1"));
        // Unlifted ItemConfig members ride the stem's own extra
        assert!(item.extra.contains_key("handbookPriceOverride"));
        assert!(item.extra.contains_key("somethingCecilerAdded"));

        let scavcase = root.scavcase.as_ref().unwrap();
        assert_eq!(scavcase.money_rewards.money_reward_chance_percent, 5);
        assert!(scavcase.reward_item_blacklist.contains("config_r1"));
        assert!(scavcase.allow_multiple_ammo_rewards_per_rarity);

        let ragfair = root.ragfair.as_ref().unwrap();
        assert!(ragfair.dynamic.use_trader_price_for_offers_if_higher);
        assert_eq!(ragfair.dynamic.end_time_seconds.max, 36000);
        // Unlifted RagfairConfig members ride the stem's own extra
        assert_eq!(ragfair.extra["runIntervalSeconds"], 450);

        assert!(
            root.inventory
                .as_ref()
                .unwrap()
                .custom_money_tpls
                .contains("custom_money")
        );

        let quest = root.quest.as_ref().unwrap();
        assert_eq!(
            quest.repeatable_quest_template_ids.pmc["Elimination"],
            "pmc_elim"
        );
        assert_eq!(
            quest.repeatable_quest_template_ids.scav["Elimination"],
            "scav_elim"
        );
        assert_eq!(quest.location_id_map["bigmap"], "55f2d3fd4bdc2d5f408b4567");
        // Unlifted QuestConfig members ride the stem's own extra
        assert!(quest.extra.contains_key("repeatableQuests"));

        // Every other kind is still an untyped key of the flatten map, and the five lifted ones are
        // no longer in it
        assert!(root.extra.contains_key("spt-core"));
        assert!(!root.extra.contains_key("spt-item"));
        assert!(!root.extra.contains_key("spt-scavcase"));
        assert!(!root.extra.contains_key("spt-ragfair"));
        assert!(!root.extra.contains_key("spt-inventory"));
        assert!(!root.extra.contains_key("spt-quest"));
    }

    /// A configs root with none of the lifted stems parses: absent is `None`, which the consuming
    /// family's resolve rejects per call, naming the stem — or, for `spt-inventory`, reads as the
    /// empty custom-money set the ragfair path had before the lift.
    #[test]
    fn configs_root_without_the_lifted_stems_parses_with_none() {
        let root: ConfigsRoot =
            serde_json::from_str(r#"{"spt-core": {"kind": "spt-core"}}"#).unwrap();

        assert!(root.item.is_none());
        assert!(root.scavcase.is_none());
        assert!(root.ragfair.is_none());
        assert!(root.inventory.is_none());
        assert!(root.quest.is_none());
    }

    /// The other half of the strictness rule: a stem that *is* there but does not parse fails the
    /// whole publish (`STATUS_BAD_ARGS`, previous resident DB intact) rather than collapsing to the
    /// `serde(default)` `None` an absent one gets. `#[serde(default)]` only covers an absent key.
    #[test]
    fn a_malformed_lifted_stem_is_a_parse_error_not_a_silent_none() {
        // `rewardItemValueRangeRub` is a map, not a number
        assert!(
            serde_json::from_str::<ConfigsRoot>(
                r#"{"spt-scavcase": {"rewardItemValueRangeRub": 5}}"#
            )
            .is_err()
        );
        // `bossItems` is a set of strings, not an object
        assert!(
            serde_json::from_str::<ConfigsRoot>(r#"{"spt-item": {"bossItems": {"a": 1}}}"#)
                .is_err()
        );
        // `dynamic` carries no `serde(default)`, so a `spt-ragfair` stem without it fails rather
        // than handing the offer path an invented config
        assert!(
            serde_json::from_str::<ConfigsRoot>(r#"{"spt-ragfair": {"kind": "spt-ragfair"}}"#)
                .is_err()
        );
        // `customMoneyTpls` is soft when absent, not when malformed
        assert!(
            serde_json::from_str::<ConfigsRoot>(r#"{"spt-inventory": {"customMoneyTpls": 5}}"#)
                .is_err()
        );
        // Neither `QuestConfigLift` member carries a `serde(default)`, so a `spt-quest` stem
        // without one fails rather than handing the helper an empty map to miss every lookup in
        assert!(
            serde_json::from_str::<ConfigsRoot>(r#"{"spt-quest": {"kind": "spt-quest"}}"#).is_err()
        );
        assert!(
            serde_json::from_str::<ConfigsRoot>(
                r#"{"spt-quest": {"repeatableQuestTemplateIds": {"pmc": {}, "scav": {}},
                    "locationIdMap": []}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn publish_roots_accepts_locations_as_a_root_name() {
        let request: PublishRequest =
            serde_json::from_str(r#"{"schema":1,"roots":{"locations":{"factory4_day":{}}}}"#)
                .unwrap();
        assert!(request.roots.locations.is_some());
    }
}
