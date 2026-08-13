//! Wire models for the bot generation family.
//!
//! Same two families as `loot::models`: DB/EFT models mirroring the C# records (wire names pinned
//! to the `JsonPropertyName`, or the member name verbatim where the record carries none, and a
//! `#[serde(flatten)] extra` map so mod-added fields survive the trip), and request/response
//! envelopes, which are a fresh contract between the C# caller and this crate and so are plain
//! camelCase.
//!
//! Blocks typed `serde_json::Value` here are deliberate: each grows a real type in the task that
//! first reads it, the way the loot port grew its models.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loot::models::{Diagnostic, ItemView};

/// Mod-added fields captured on the way in and replayed on the way out.
type Extra = serde_json::Map<String, serde_json::Value>;

// ---------------------------------------------------------------------------
// DB/EFT wire models
// ---------------------------------------------------------------------------

/// The three `BotType` blocks the inventory generator reads (`Models/Eft/Common/Tables/BotType.cs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotTemplateWire {
    #[serde(rename = "inventory")]
    pub inventory: BotTypeInventoryWire,
    /// `BotType.Chances` — typed by the task that reads it.
    #[serde(rename = "chances")]
    pub chances: serde_json::Value,
    /// `BotType.Generation` — typed by the task that reads it.
    #[serde(rename = "generation")]
    pub generation: serde_json::Value,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `BotTypeInventory` (`BotType.cs:228-240`). `IndexMap` throughout: every one of these maps is
/// enumerated to build a weighted pool, so the iteration order reaches the RNG.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BotTypeInventoryWire {
    /// `Dictionary<EquipmentSlots, Dictionary<MongoId, double>>`. The key is the `EquipmentSlots`
    /// member name (`Headwear`, `TacticalVest`, …), kept as a string here.
    #[serde(rename = "equipment", default)]
    pub equipment: IndexMap<String, IndexMap<String, f64>>,
    /// Carries no `JsonPropertyName`, and nothing sets a naming policy, so the wire name is the
    /// C# member name verbatim — PascalCase, unlike its three siblings.
    #[serde(rename = "Ammo", default)]
    pub ammo: IndexMap<String, IndexMap<String, f64>>,
    #[serde(rename = "items", default)]
    pub items: ItemPoolsWire,
    /// `GlobalMods` = `Dictionary<MongoId, Dictionary<string, HashSet<MongoId>>>`
    /// (`GlobalTablesUsings.cs`). The inner set is a `Vec` here: it is drawn from by index, and a
    /// `HashSet` deserialized from a JSON array keeps that array's order in C# too.
    #[serde(rename = "mods", default)]
    pub mods: IndexMap<String, IndexMap<String, Vec<String>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `ItemPools` (`BotType.cs:242-253`) — no `JsonPropertyName` on any member, so the wire names are
/// the property names verbatim.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemPoolsWire {
    #[serde(rename = "Backpack", default)]
    pub backpack: IndexMap<String, f64>,
    #[serde(rename = "Pockets", default)]
    pub pockets: IndexMap<String, f64>,
    #[serde(rename = "SecuredContainer", default)]
    pub secured_container: IndexMap<String, f64>,
    #[serde(rename = "SpecialLoot", default)]
    pub special_loot: IndexMap<String, f64>,
    #[serde(rename = "TacticalVest", default)]
    pub tactical_vest: IndexMap<String, f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Spt/Config/BotConfig.cs:465-472` — `BotConfig.LootItemResourceRandomization[botRole]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RandomisedResourceDetails {
    #[serde(rename = "food")]
    pub food: Option<RandomisedResourceValues>,
    #[serde(rename = "meds")]
    pub meds: Option<RandomisedResourceValues>,
}

/// `Models/Spt/Config/BotConfig.cs:474-487`. Both members are non-nullable `float`s in C#, so a key
/// missing from the config lands on 0 rather than disabling randomisation — `#[serde(default)]`
/// reproduces that.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RandomisedResourceValues {
    #[serde(rename = "resourcePercent", default)]
    pub resource_percent: f64,
    #[serde(rename = "chanceMaxResourcePercent", default)]
    pub chance_max_resource_percent: f64,
}

/// The six `EquipmentFilters` chance percentages `BotGeneratorHelper.GenerateExtraPropertiesForItem`
/// reads (`Models/Spt/Config/BotConfig.cs:287-318`). All nullable in C#; each call site supplies its
/// own literal fallback, so the `Option`s are carried through rather than defaulted here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EquipmentFilters {
    #[serde(rename = "faceShieldIsActiveChancePercent")]
    pub face_shield_is_active_chance_percent: Option<f64>,
    #[serde(rename = "lightIsActiveDayChancePercent")]
    pub light_is_active_day_chance_percent: Option<f64>,
    #[serde(rename = "lightIsActiveNightChancePercent")]
    pub light_is_active_night_chance_percent: Option<f64>,
    #[serde(rename = "laserIsActiveChancePercent")]
    pub laser_is_active_chance_percent: Option<f64>,
    #[serde(rename = "nvgIsActiveChanceDayPercent")]
    pub nvg_is_active_chance_day_percent: Option<f64>,
    #[serde(rename = "nvgIsActiveChanceNightPercent")]
    pub nvg_is_active_chance_night_percent: Option<f64>,
}

/// `Models/Spt/Bots/ChooseRandomCompatibleModResult.cs`. Every member is nullable there and the
/// four `IsItemIncompatibleWithCurrentItems` exits each set a different subset, so the `Option`s
/// are load-bearing — `found` and `slotBlocked` are absent, not false, on the final compatible
/// return.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChooseRandomCompatibleModResult {
    #[serde(rename = "incompatible", skip_serializing_if = "Option::is_none")]
    pub incompatible: Option<bool>,
    #[serde(rename = "found", skip_serializing_if = "Option::is_none")]
    pub found: Option<bool>,
    #[serde(rename = "chosenTpl", skip_serializing_if = "Option::is_none")]
    pub chosen_template: Option<String>,
    #[serde(rename = "reason", skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(rename = "slotBlocked", skip_serializing_if = "Option::is_none")]
    pub slot_blocked: Option<bool>,
}

// ---------------------------------------------------------------------------
// Request / response envelopes
// ---------------------------------------------------------------------------

/// `BotInventoryContainerService.ContainerDetails` (`BotInventoryContainerService.cs:415-451`),
/// serialized so the C# side can rebuild the service's per-bot cache after a native call.
///
/// `ContainerDbItem` and `ContainerInventoryItem` ride as ids: the C# rebuild resolves the first
/// through `itemHelper.GetItem` and the second out of the inventory it was just handed, exactly as
/// `AddEmptyContainerToBot` does. `ContainerFull` is not carried — it is initialised `false` and
/// nothing in the codebase ever assigns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDetailsWire {
    /// `ContainerDetails.ContainerDbItem.Id`.
    pub container_tpl: String,
    /// `ContainerDetails.ContainerInventoryItem.Id`.
    pub container_item_id: String,
    /// `ContainerDetails.ContainerGridDetails`, in the container template's grid order.
    pub grids: Vec<ContainerMapDetailsWire>,
}

/// `BotInventoryContainerService.ContainerMapDetails` (`BotInventoryContainerService.cs:453-457`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerMapDetailsWire {
    /// `int[CellsV, CellsH]` as rows of columns, `1` = occupied. Dimensions are implied, matching
    /// the `[Vec<Vec<u8>>]` grids `loot::container_extensions` already packs into.
    pub grid_map: Vec<Vec<u8>>,
    pub grid_full: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBotInventoryRequest {
    pub bot_id: String,
    /// Test-only: when present, every draw comes from a seeded xoshiro256** for the duration of
    /// the call (see `loot::random_util::TestSeedGuard`). Never set on the production path.
    #[serde(default)]
    pub test_seed: Option<u64>,
    pub details: BotGenerationDetailsWire,
    pub template: BotTemplateWire,
    /// Hoisted live state — `PlayerProfile.Info.Level`.
    pub generating_player_level: i32,
    /// Hoisted live state — `RaidTime`/`WeatherHelper.IsNightTime`.
    pub is_night_time: bool,
    /// `BotConfig.Equipment[role]`; typed fields grow per-task.
    pub equipment_config: serde_json::Map<String, serde_json::Value>,
    pub item_spawn_limits: IndexMap<String, IndexMap<String, f64>>,
    pub wallet_loot: serde_json::Value,
    pub currency_stack_size: serde_json::Value,
    pub secure_container_ammo_stack_count: serde_json::Value,
    pub disable_loot_on_bot_types: Vec<String>,
    pub low_profile_gas_block_tpls: Vec<String>,
    pub loot_item_resource_randomization: IndexMap<String, RandomisedResourceDetails>,
    pub pmc_config: serde_json::Value,
    pub repair_kit_weapon: serde_json::Value,
    /// `GetBotEquipmentBlacklist(role, level)` result.
    pub equipment_blacklist: serde_json::Value,
    pub sight_whitelist: IndexMap<String, Vec<String>>,
    /// The 13 resolved `BotLootCacheService` pools.
    pub loot_pools: IndexMap<String, serde_json::Value>,
    /// `GlobalTable.ItemPresets`.
    pub item_presets: IndexMap<String, serde_json::Value>,
    pub default_presets_by_tpl: IndexMap<String, serde_json::Value>,
    pub presets_by_id: IndexMap<String, serde_json::Value>,
    /// `ItemFilterService.GetBlacklistedItems()`.
    pub config_blacklist: Vec<String>,
    pub handbook_prices: IndexMap<String, f64>,
    /// The `TemplateItem` slice, flattened by the C# caller exactly as the loot envelopes take it.
    pub items: IndexMap<String, ItemView>,
}

/// `Models/Spt/Bots/BotGenerationDetails.cs`, narrowed to what the generator reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGenerationDetailsWire {
    pub role: String,
    pub role_lowercase: String,
    pub side: String,
    pub bot_level: i32,
    pub is_pmc: bool,
    pub is_player_scav: bool,
    pub game_version: String,
    pub location: Option<String>,
    pub bot_difficulty: String,
    pub clear_bot_container_cache_after_generation: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotInventoryResult {
    /// `BotBaseInventory` shape, built by the orchestrator task.
    pub inventory: serde_json::Value,
    pub diagnostics: Vec<Diagnostic>,
    /// Slot → grid state, from `bot_generator_helper::ContainerGrids::into_wire`.
    pub container_grids: IndexMap<String, ContainerDetailsWire>,
    /// Equipment-slot → clamped chance.
    pub randomisation_clamps: IndexMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every required [`GenerateBotInventoryRequest`] member. `testSeed` is deliberately absent —
    /// its omission is what exercises the missing-field → `None` path.
    const REQUEST_JSON: &str = r#"{
        "botId":"bbbbbbbbbbbbbbbbbbbbbbbb",
        "details":{"role":"assault","roleLowercase":"assault","side":"Savage","botLevel":12,
            "isPmc":false,"isPlayerScav":false,"gameVersion":"standard","location":"bigmap",
            "botDifficulty":"normal","clearBotContainerCacheAfterGeneration":true},
        "template":{
            "inventory":{
                "equipment":{"Headwear":{"aaaaaaaaaaaaaaaaaaaaaaa1":3.5,"aaaaaaaaaaaaaaaaaaaaaaa2":1}},
                "Ammo":{"Caliber762x39":{"aaaaaaaaaaaaaaaaaaaaaaa3":5}},
                "items":{"Backpack":{"aaaaaaaaaaaaaaaaaaaaaaa4":2},"Pockets":{},
                    "SecuredContainer":{},"SpecialLoot":{},"TacticalVest":{}},
                "mods":{"aaaaaaaaaaaaaaaaaaaaaaa5":{"mod_magazine":["aaaaaaaaaaaaaaaaaaaaaaa6"]}},
                "modAddedInventoryField":"kept"},
            "chances":{"equipment":{"Headwear":75}},
            "generation":{"items":{"backpackLoot":{"weights":{"1":1}}}},
            "modAddedTemplateField":7},
        "generatingPlayerLevel":30,
        "isNightTime":true,
        "equipmentConfig":{"weaponModLimits":{"scopeLimit":2}},
        "itemSpawnLimits":{"assault":{"aaaaaaaaaaaaaaaaaaaaaaa7":1}},
        "walletLoot":{"chancePercent":10},
        "currencyStackSize":{"RUB":{"min":1,"max":2}},
        "secureContainerAmmoStackCount":{"min":5,"max":10},
        "disableLootOnBotTypes":["bosstest"],
        "lowProfileGasBlockTpls":["aaaaaaaaaaaaaaaaaaaaaaa8"],
        "lootItemResourceRandomization":{"assault":{"food":{"chanceMaxResourcePercent":60}}},
        "pmcConfig":{"forceHealingItemsIntoSecure":true},
        "repairKitWeapon":{"maxUsePercent":20},
        "equipmentBlacklist":{"equipment":{"Headwear":["aaaaaaaaaaaaaaaaaaaaaaa9"]}},
        "sightWhitelist":{"55818ad54bdc2ddc698b4569":["55818add4bdc2d5b648b456f"]},
        "lootPools":{"backpackLoot":{"aaaaaaaaaaaaaaaaaaaaaab1":4}},
        "itemPresets":{"p1":{"_id":"p1","_items":[]}},
        "defaultPresetsByTpl":{"aaaaaaaaaaaaaaaaaaaaaab2":{"_id":"p2"}},
        "presetsById":{"p2":{"_id":"p2"}},
        "configBlacklist":["aaaaaaaaaaaaaaaaaaaaaab3"],
        "handbookPrices":{"aaaaaaaaaaaaaaaaaaaaaab4":12500.5},
        "items":{"aaaaaaaaaaaaaaaaaaaaaab5":{"parent":"aaaaaaaaaaaaaaaaaaaaaab6","width":2,"height":1}}
    }"#;

    #[test]
    fn generate_bot_inventory_request_deserializes() {
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(REQUEST_JSON).unwrap();

        assert_eq!(parsed.bot_id, "bbbbbbbbbbbbbbbbbbbbbbbb");
        // Absent `testSeed` is the production path.
        assert_eq!(parsed.test_seed, None);

        assert_eq!(parsed.details.role, "assault");
        assert_eq!(parsed.details.role_lowercase, "assault");
        assert_eq!(parsed.details.side, "Savage");
        assert_eq!(parsed.details.bot_level, 12);
        assert!(!parsed.details.is_pmc);
        assert!(!parsed.details.is_player_scav);
        assert_eq!(parsed.details.game_version, "standard");
        assert_eq!(parsed.details.location.as_deref(), Some("bigmap"));
        assert_eq!(parsed.details.bot_difficulty, "normal");
        assert!(parsed.details.clear_bot_container_cache_after_generation);

        let inventory = &parsed.template.inventory;
        assert_eq!(
            inventory.equipment["Headwear"].keys().collect::<Vec<_>>(),
            vec!["aaaaaaaaaaaaaaaaaaaaaaa1", "aaaaaaaaaaaaaaaaaaaaaaa2"]
        );
        assert_eq!(
            inventory.equipment["Headwear"]["aaaaaaaaaaaaaaaaaaaaaaa1"],
            3.5
        );
        assert_eq!(
            inventory.ammo["Caliber762x39"]["aaaaaaaaaaaaaaaaaaaaaaa3"],
            5.0
        );
        assert_eq!(inventory.items.backpack["aaaaaaaaaaaaaaaaaaaaaaa4"], 2.0);
        assert!(inventory.items.tactical_vest.is_empty());
        assert_eq!(
            inventory.mods["aaaaaaaaaaaaaaaaaaaaaaa5"]["mod_magazine"],
            vec!["aaaaaaaaaaaaaaaaaaaaaaa6"]
        );
        assert_eq!(parsed.template.chances["equipment"]["Headwear"], 75);
        assert_eq!(
            parsed.template.generation["items"]["backpackLoot"]["weights"]["1"],
            1
        );

        assert_eq!(parsed.generating_player_level, 30);
        assert!(parsed.is_night_time);
        assert_eq!(parsed.equipment_config["weaponModLimits"]["scopeLimit"], 2);
        assert_eq!(
            parsed.item_spawn_limits["assault"]["aaaaaaaaaaaaaaaaaaaaaaa7"],
            1.0
        );
        assert_eq!(parsed.wallet_loot["chancePercent"], 10);
        assert_eq!(parsed.currency_stack_size["RUB"]["max"], 2);
        assert_eq!(parsed.secure_container_ammo_stack_count["min"], 5);
        assert_eq!(parsed.disable_loot_on_bot_types, vec!["bosstest"]);
        assert_eq!(
            parsed.low_profile_gas_block_tpls,
            vec!["aaaaaaaaaaaaaaaaaaaaaaa8"]
        );
        let food = parsed.loot_item_resource_randomization["assault"]
            .food
            .as_ref()
            .unwrap();
        assert_eq!(food.chance_max_resource_percent, 60.0);
        // Absent in the payload; C#'s non-nullable float lands on 0, not on "no randomisation".
        assert_eq!(food.resource_percent, 0.0);
        assert!(
            parsed.loot_item_resource_randomization["assault"]
                .meds
                .is_none()
        );
        assert_eq!(parsed.pmc_config["forceHealingItemsIntoSecure"], true);
        assert_eq!(parsed.repair_kit_weapon["maxUsePercent"], 20);
        assert_eq!(
            parsed.equipment_blacklist["equipment"]["Headwear"][0],
            "aaaaaaaaaaaaaaaaaaaaaaa9"
        );
        assert_eq!(
            parsed.sight_whitelist["55818ad54bdc2ddc698b4569"],
            vec!["55818add4bdc2d5b648b456f"]
        );
        assert_eq!(
            parsed.loot_pools["backpackLoot"]["aaaaaaaaaaaaaaaaaaaaaab1"],
            4
        );
        assert_eq!(parsed.item_presets["p1"]["_id"], "p1");
        assert_eq!(
            parsed.default_presets_by_tpl["aaaaaaaaaaaaaaaaaaaaaab2"]["_id"],
            "p2"
        );
        assert_eq!(parsed.presets_by_id["p2"]["_id"], "p2");
        assert_eq!(parsed.config_blacklist, vec!["aaaaaaaaaaaaaaaaaaaaaab3"]);
        assert_eq!(parsed.handbook_prices["aaaaaaaaaaaaaaaaaaaaaab4"], 12500.5);
        assert_eq!(parsed.items["aaaaaaaaaaaaaaaaaaaaaab5"].width, Some(2));
    }

    #[test]
    fn test_seed_is_read_when_present() {
        let json = REQUEST_JSON.replace(
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","#,
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","testSeed":42,"#,
        );
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.test_seed, Some(42));

        // Explicit null is the same as absent.
        let json = REQUEST_JSON.replace(
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","#,
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","testSeed":null,"#,
        );
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.test_seed, None);
    }

    #[test]
    fn mod_added_template_fields_survive_the_round_trip() {
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(REQUEST_JSON).unwrap();
        let out = serde_json::to_value(&parsed.template).unwrap();

        assert_eq!(out["modAddedTemplateField"], 7);
        assert_eq!(out["inventory"]["modAddedInventoryField"], "kept");
        // Exact wire casing: `Ammo` is PascalCase in the database, the rest are camelCase.
        let inventory = out["inventory"].as_object().unwrap();
        assert!(inventory.contains_key("Ammo"));
        assert!(inventory.contains_key("equipment"));
        assert!(inventory.contains_key("items"));
        assert!(inventory.contains_key("mods"));
        // `ItemPools` members carry no `JsonPropertyName`, so they ride out PascalCase.
        let pools = out["inventory"]["items"].as_object().unwrap();
        assert!(pools.contains_key("SecuredContainer"));
        assert!(pools.contains_key("SpecialLoot"));
        assert!(pools.contains_key("TacticalVest"));
    }

    #[test]
    fn bot_inventory_result_serializes_with_camel_case_keys() {
        let out = serde_json::to_value(BotInventoryResult {
            inventory: serde_json::json!({"items":[],"equipment":"aaaaaaaaaaaaaaaaaaaaaaaa"}),
            diagnostics: vec![Diagnostic {
                level: crate::loot::models::DEBUG.to_owned(),
                locale_key: Some("bot-missing_item".to_owned()),
                args: Some(serde_json::json!({"tpl":"x"})),
                message: None,
            }],
            container_grids: IndexMap::from([(
                "TacticalVest".to_owned(),
                ContainerDetailsWire {
                    container_tpl: "aaaaaaaaaaaaaaaaaaaaaac1".to_owned(),
                    container_item_id: "aaaaaaaaaaaaaaaaaaaaaac2".to_owned(),
                    grids: vec![ContainerMapDetailsWire {
                        grid_map: vec![vec![0, 1], vec![0, 0]],
                        grid_full: false,
                    }],
                },
            )]),
            randomisation_clamps: IndexMap::from([("Headwear".to_owned(), 62.5)]),
        })
        .unwrap();

        assert_eq!(
            out.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec![
                "inventory",
                "diagnostics",
                "containerGrids",
                "randomisationClamps"
            ]
        );
        assert_eq!(out["inventory"]["equipment"], "aaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(out["diagnostics"][0]["localeKey"], "bot-missing_item");
        assert_eq!(out["diagnostics"][0]["args"]["tpl"], "x");
        let grids = &out["containerGrids"]["TacticalVest"];
        assert_eq!(grids["containerTpl"], "aaaaaaaaaaaaaaaaaaaaaac1");
        assert_eq!(grids["containerItemId"], "aaaaaaaaaaaaaaaaaaaaaac2");
        assert_eq!(grids["grids"][0]["gridMap"][0][1], 1);
        assert_eq!(grids["grids"][0]["gridFull"], false);
        assert_eq!(out["randomisationClamps"]["Headwear"], 62.5);
    }
}
