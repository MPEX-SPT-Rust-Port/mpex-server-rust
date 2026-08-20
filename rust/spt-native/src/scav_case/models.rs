//! Wire models for the scav case reward generator.
//!
//! Request/response envelopes only — a new contract between the C# caller and this crate, so they
//! are plain camelCase with no passthrough map, as the reward-loot envelopes in
//! [`crate::loot::models`] are. The DB/EFT types they carry ([`Item`], [`ItemView`],
//! [`PresetView`]) are reused from there rather than redeclared.

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};

use crate::bot::repair_service::MinMax;
use crate::loot::models::{Item, ItemView, PresetView};

/// The scav case request envelope, split as the loot family's
/// ([`crate::loot::models::CreateRandomLootRequest`]): the epoch naming the resident DB the
/// varying half was built against, with the C#-built invariant bundle riding along only as the
/// distrust fallback.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScavCaseRewardsRequest {
    pub epoch: u64,
    pub views_override: Option<Box<ScavCaseViewsWire>>,
    pub varying: ScavCaseVarying,
}

/// The C#-built invariant bundle: the distrust fallback. Every member has a resident twin — the
/// four view members derive off the published roots, the three config-backed ones read the
/// `spt-scavcase`/`spt-item` stems of the configs root.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScavCaseViewsWire {
    /// `hideoutTable.Production.ScavRecipes` (`:54`) — searched for
    /// [`ScavCaseVarying::recipe_id`].
    pub scav_recipes: Vec<ScavRecipeView>,
    /// `IndexMap`, not `HashMap`: `CacheDbItems` (`:89,147`) filters `templateTable.Items.Values`
    /// into a pool that `PickRandomRewards` (`:238`) and `GetRandomAmmo` (`:308`) then draw an index
    /// out of, so the iteration order is observable.
    pub items_view: IndexMap<String, ItemView>,
    /// `ragfairPriceService.GetStaticPriceForItem(tpl)` per item (`:288,380`). Lookup only.
    pub static_prices: HashMap<String, f64>,
    /// `presetHelper.GetDefaultPreset(tpl)` (`:345`). A tpl absent from here is the C# `null` that
    /// warns and skips the reward.
    pub default_presets_by_tpl: IndexMap<String, PresetView>,
    /// The `spt-scavcase` config. Config-backed, so it rides the invariant bundle now; the resident
    /// arm reads [`crate::db::models::ConfigsRoot::scavcase`] instead.
    pub config: ScavCaseConfigView,
    /// `itemFilterService.IsItemRewardBlacklisted` (`:119,174`), which is
    /// `ItemConfig.RewardItemBlacklist` verbatim (`ItemFilterService.cs:33-35`) — distinct from
    /// [`ScavCaseConfigView::reward_item_blacklist`], the scav case config's own list. The resident
    /// arm reads [`crate::db::models::ItemConfigLift::reward_item_blacklist`].
    pub reward_item_blacklist: IndexSet<String>,
    /// `itemFilterService.IsBossItem` (`:124,179`) — `ItemConfig.BossItems` verbatim
    /// (`ItemFilterService.cs:69-71`). `IndexSet`, as every set lifted off a C# `HashSet`
    /// (`db::models`); membership is all the generator asks of it.
    pub boss_items: IndexSet<String>,
}

/// Everything else `ScavCaseRewardGenerator.Generate` (`:49-77`) reads — the per-request and
/// service-backed half, riding every send.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScavCaseVarying {
    pub recipe_id: String,
    /// `seasonalEventService.GetInactiveSeasonalEventItems()` (`:86`) — service state, not config.
    pub inactive_seasonal_items: HashSet<String>,
    /// `itemFilterService.IsItemBlacklisted` (`:112,168`), as a set. Its backing cache is the
    /// config blacklist *plus* whatever `AddItemToBlacklistCache` added at runtime, so it stays
    /// varying where the two config-backed lists above did not.
    pub global_blacklist: HashSet<String>,
    /// Test-only, as [`crate::loot::models::RewardLootVarying::test_seed`].
    pub test_seed: Option<u64>,
}

/// `Models/Eft/Hideout/HideoutProduction.cs:97-110` — the two members the generator reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScavRecipeView {
    pub id: String,
    pub end_products: EndProductsView,
}

/// `Models/Eft/Hideout/HideoutProduction.cs:112-122`. C# declares all three `MinMax<int>?` and then
/// dereferences them unconditionally (`:403-420`), so a missing one is an NRE there and a parse
/// error here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndProductsView {
    pub common: MinMax<i32>,
    pub rare: MinMax<i32>,
    pub superrare: MinMax<i32>,
}

/// `Models/Spt/Config/ScavCaseConfig.cs`. `AmmoRewards.AmmoRewardBlacklist` is absent — nothing in
/// the generator reads it. Also the resident `spt-scavcase` stem
/// ([`crate::db::models::ConfigsRoot::scavcase`]): no `deny_unknown_fields`, so the unread members
/// (`kind`, the dispatch flags, a Release build's `[JsonExtensionData]` additions) ride past.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScavCaseConfigView {
    /// Keyed by `RewardRarity` (`:504-509`): `common`/`rare`/`superrare`. Lookup only.
    pub reward_item_value_range_rub: HashMap<String, MinMax<f64>>,
    pub money_rewards: MoneyRewardsView,
    pub ammo_rewards: AmmoRewardsView,
    pub reward_item_parent_blacklist: HashSet<String>,
    /// The config's own list (`:111,168`) — distinct from
    /// [`ScavCaseViewsWire::reward_item_blacklist`], which is `ItemFilterService`'s.
    pub reward_item_blacklist: HashSet<String>,
    pub allow_multiple_money_rewards_per_rarity: bool,
    pub allow_multiple_ammo_rewards_per_rarity: bool,
    pub allow_boss_items_as_rewards: bool,
}

/// `Models/Spt/Config/ScavCaseConfig.cs:36-52`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyRewardsView {
    pub money_reward_chance_percent: i32,
    pub rub_count: MoneyLevelsView,
    pub usd_count: MoneyLevelsView,
    pub eur_count: MoneyLevelsView,
    pub gp_count: MoneyLevelsView,
}

/// `Models/Spt/Config/ScavCaseConfig.cs:54-64` — the rarity levels C# reaches by JSON property name
/// (`GetByJsonProperty<MinMax<int>>(rarity)`, `:472-495`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyLevelsView {
    pub common: MinMax<i32>,
    pub rare: MinMax<i32>,
    pub superrare: MinMax<i32>,
}

/// `Models/Spt/Config/ScavCaseConfig.cs:66-79`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmmoRewardsView {
    pub ammo_reward_chance_percent: i32,
    /// Keyed by rarity, as [`ScavCaseConfigView::reward_item_value_range_rub`]. `GetRandomAmmo`
    /// (`:290`) misses for an unknown rarity rather than throwing.
    pub ammo_reward_value_range_rub: HashMap<String, MinMax<f64>>,
    pub min_stack_size: i32,
}

/// What `Generate` (`:76`) hands back: each inner `Vec` is one reward item plus its children.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScavCaseResponse {
    pub result: Vec<Vec<Item>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One recipe, two items, seed set — the minimal envelope the generator needs, in the
    /// resident-DB `{epoch, viewsOverride, varying}` split.
    const REQUEST_JSON: &str = r#"{
        "epoch": 0,
        "viewsOverride": {
            "scavRecipes":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaa",
                "endProducts":{"common":{"min":2,"max":4},"rare":{"min":1,"max":2},
                    "superrare":{"min":0,"max":1}}}],
            "itemsView":{
                "111111111111111111111111":{"parent":"bbbbbbbbbbbbbbbbbbbbbbbb","type":"Item",
                    "name":"patron_762x39","stackMaxSize":60,"questItem":false},
                "222222222222222222222222":{"parent":"dddddddddddddddddddddddd","type":"Item",
                    "name":"weapon_ak","questItem":false}},
            "staticPrices":{"111111111111111111111111":42.5,
                "222222222222222222222222":31000.0},
            "defaultPresetsByTpl":{"222222222222222222222222":{"id":"p1","name":"ak_default",
                "items":[{"_id":"eeeeeeeeeeeeeeeeeeeeeeee","_tpl":"222222222222222222222222"}]}},
            "config":{
                "rewardItemValueRangeRub":{"common":{"min":0.0,"max":25000.5},
                    "rare":{"min":25000.5,"max":100000.0},
                    "superrare":{"min":100000.0,"max":1000000.0}},
                "moneyRewards":{"moneyRewardChancePercent":20,
                    "rubCount":{"common":{"min":3000,"max":10000},"rare":{"min":10000,"max":25000},
                        "superrare":{"min":25000,"max":50000}},
                    "usdCount":{"common":{"min":30,"max":100},"rare":{"min":100,"max":250},
                        "superrare":{"min":250,"max":500}},
                    "eurCount":{"common":{"min":25,"max":90},"rare":{"min":90,"max":225},
                        "superrare":{"min":225,"max":450}},
                    "gpCount":{"common":{"min":1,"max":2},"rare":{"min":2,"max":3},
                        "superrare":{"min":3,"max":4}}},
                "ammoRewards":{"ammoRewardChancePercent":15,
                    "ammoRewardValueRangeRub":{"common":{"min":0.0,"max":80.0}},"minStackSize":30},
                "rewardItemParentBlacklist":["bbbbbbbbbbbbbbbbbbbbbbbb"],
                "rewardItemBlacklist":["cccccccccccccccccccccccc"],
                "allowMultipleMoneyRewardsPerRarity":false,
                "allowMultipleAmmoRewardsPerRarity":true,
                "allowBossItemsAsRewards":false},
            "rewardItemBlacklist":["555555555555555555555555"],
            "bossItems":["666666666666666666666666"]},
        "varying": {
            "recipeId":"aaaaaaaaaaaaaaaaaaaaaaaa",
            "inactiveSeasonalItems":["333333333333333333333333"],
            "globalBlacklist":["444444444444444444444444"],
            "testSeed":42}
    }"#;

    /// The smallest config the view accepts — every member present, since none carries a serde
    /// default.
    const MINIMAL_CONFIG_JSON: &str = r#"{
        "rewardItemValueRangeRub":{},
        "moneyRewards":{"moneyRewardChancePercent":0,
            "rubCount":{"common":{"min":1,"max":1},"rare":{"min":1,"max":1},
                "superrare":{"min":1,"max":1}},
            "usdCount":{"common":{"min":1,"max":1},"rare":{"min":1,"max":1},
                "superrare":{"min":1,"max":1}},
            "eurCount":{"common":{"min":1,"max":1},"rare":{"min":1,"max":1},
                "superrare":{"min":1,"max":1}},
            "gpCount":{"common":{"min":1,"max":1},"rare":{"min":1,"max":1},
                "superrare":{"min":1,"max":1}}},
        "ammoRewards":{"ammoRewardChancePercent":0,"ammoRewardValueRangeRub":{},
            "minStackSize":30},
        "rewardItemParentBlacklist":[],
        "rewardItemBlacklist":[],
        "allowMultipleMoneyRewardsPerRarity":false,
        "allowMultipleAmmoRewardsPerRarity":false,
        "allowBossItemsAsRewards":false}"#;

    #[test]
    fn a_resident_request_with_epoch_and_varying_only_deserializes() {
        let req: ScavCaseRewardsRequest = serde_json::from_str(
            r#"{"epoch": 3, "varying": {"recipeId": "6662e9aca7e0b43baa3d5f9c",
                 "inactiveSeasonalItems": [], "globalBlacklist": [], "testSeed": 7}}"#,
        )
        .unwrap();

        assert_eq!(req.epoch, 3);
        assert!(req.views_override.is_none());
        assert_eq!(req.varying.test_seed, Some(7));
    }

    #[test]
    fn an_override_request_with_epoch_zero_deserializes() {
        let req: ScavCaseRewardsRequest = serde_json::from_str(&format!(
            r#"{{"epoch": 0,
                "viewsOverride": {{"scavRecipes": [], "itemsView": {{}}, "staticPrices": {{}},
                    "defaultPresetsByTpl": {{}}, "config": {MINIMAL_CONFIG_JSON},
                    "rewardItemBlacklist": [], "bossItems": []}},
                "varying": {{"recipeId": "6662e9aca7e0b43baa3d5f9c",
                    "inactiveSeasonalItems": [], "globalBlacklist": []}}}}"#
        ))
        .unwrap();

        assert!(req.views_override.is_some());
    }

    #[test]
    fn scav_case_request_deserializes() {
        let parsed: ScavCaseRewardsRequest = serde_json::from_str(REQUEST_JSON).unwrap();

        assert_eq!(parsed.epoch, 0);
        assert_eq!(parsed.varying.recipe_id, "aaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(parsed.varying.test_seed, Some(42));

        let views = parsed.views_override.as_ref().unwrap();
        assert_eq!(views.scav_recipes.len(), 1);
        let recipe = &views.scav_recipes[0];
        assert_eq!(recipe.id, "aaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(recipe.end_products.common.min, 2);
        assert_eq!(recipe.end_products.common.max, 4);
        assert_eq!(recipe.end_products.rare.max, 2);
        assert_eq!(recipe.end_products.superrare.min, 0);

        let config = &views.config;
        assert_eq!(config.reward_item_value_range_rub["common"].max, 25000.5);
        assert_eq!(
            config.reward_item_value_range_rub["superrare"].min,
            100000.0
        );
        assert_eq!(config.money_rewards.money_reward_chance_percent, 20);
        assert_eq!(config.money_rewards.rub_count.common.min, 3000);
        assert_eq!(config.money_rewards.usd_count.rare.max, 250);
        assert_eq!(config.money_rewards.eur_count.superrare.min, 225);
        assert_eq!(config.money_rewards.gp_count.common.max, 2);
        assert_eq!(config.ammo_rewards.ammo_reward_chance_percent, 15);
        assert_eq!(
            config.ammo_rewards.ammo_reward_value_range_rub["common"].max,
            80.0
        );
        assert_eq!(config.ammo_rewards.min_stack_size, 30);
        assert!(
            config
                .reward_item_parent_blacklist
                .contains("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert!(
            config
                .reward_item_blacklist
                .contains("cccccccccccccccccccccccc")
        );
        assert!(!config.allow_multiple_money_rewards_per_rarity);
        assert!(config.allow_multiple_ammo_rewards_per_rarity);
        assert!(!config.allow_boss_items_as_rewards);

        // Ordered: the reward pool is filtered out of this map and then drawn from by index.
        assert_eq!(
            views.items_view.keys().collect::<Vec<_>>(),
            vec!["111111111111111111111111", "222222222222222222222222"]
        );
        assert_eq!(
            views.items_view["111111111111111111111111"].stack_max_size,
            Some(60)
        );
        assert_eq!(views.static_prices["111111111111111111111111"], 42.5);
        assert_eq!(
            views.default_presets_by_tpl["222222222222222222222222"]
                .name
                .as_deref(),
            Some("ak_default")
        );
        assert!(
            parsed
                .varying
                .inactive_seasonal_items
                .contains("333333333333333333333333")
        );
        assert!(
            parsed
                .varying
                .global_blacklist
                .contains("444444444444444444444444")
        );
        // The two config-backed sets ride the views bundle now, and stay distinct from the config's
        // own `rewardItemBlacklist` asserted above.
        assert!(
            views
                .reward_item_blacklist
                .contains("555555555555555555555555")
        );
        assert!(views.boss_items.contains("666666666666666666666666"));
    }

    /// `testSeed` is the only optional varying member — its omission must not fail the parse.
    #[test]
    fn scav_case_request_without_seed_deserializes() {
        let json = REQUEST_JSON.replace(",\n            \"testSeed\":42", "");
        let parsed: ScavCaseRewardsRequest = serde_json::from_str(&json).unwrap();

        assert!(parsed.varying.test_seed.is_none());
    }

    #[test]
    fn scav_case_response_serializes_with_camel_case_keys() {
        let out = serde_json::to_value(ScavCaseResponse {
            result: vec![vec![Item {
                id: "aaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
                template: "bbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
                ..Default::default()
            }]],
        })
        .unwrap();

        assert_eq!(
            out.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["result"]
        );
        assert_eq!(out["result"][0][0]["_id"], "aaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(out["result"][0][0]["_tpl"], "bbbbbbbbbbbbbbbbbbbbbbbb");
    }
}
