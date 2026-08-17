//! `Generators/ScavCaseRewardGenerator.cs`.

use crate::loot::item_helper::{self, AMMO};
use crate::loot::models::ItemView;
use crate::scav_case::models::ScavCaseRequest;

/// `CacheDbItems`' `DbItemsCache` filter (`:87-143`).
///
/// C# caches the two lists on the generator instance and refills them only when empty; there is no
/// instance to hang them off here, so each request rebuilds them. The filter reads nothing but the
/// request, so the pool is identical either way.
///
/// `TemplateItem` carries its own `Id`, [`ItemView`] does not — hence the tpl in the tuple, which
/// every caller of the pool needs (prices, presets, baseclass tests are all keyed by it).
pub fn build_reward_pool(req: &ScavCaseRequest) -> Vec<(&str, &ItemView)> {
    let parent_blacklist: Vec<&str> = req
        .config
        .reward_item_parent_blacklist
        .iter()
        .map(String::as_str)
        .collect();

    req.items_view
        .iter()
        .filter(|(tpl, item)| {
            // Base "Item" item has no parent, ignore it (`:93`). `TemplateItem.Parent` is a
            // non-nullable `MongoId` whose empty value the projection writes as null
            // (`PayloadProjection.cs:40`), so null and "" both mean `MongoId.Empty()`.
            if item.parent.as_deref().unwrap_or_default().is_empty() {
                return false;
            }

            // `:98`. An exact, case-sensitive compare, as the C# `==` is — unlike the loot
            // generator's `_type` tests, which are `OrdinalIgnoreCase` there too.
            if item.item_type.as_deref() == Some("Node") {
                return false;
            }

            // `:103`
            if item.quest_item.unwrap_or(false) {
                return false;
            }

            // Skip item if item id is on blacklist (`:109-116`). `RewardItemBlacklist` is the
            // config's own list; `global_blacklist` is `ItemFilterService.IsItemBlacklisted`.
            if item.item_type.as_deref() != Some("Item")
                || req.config.reward_item_blacklist.contains(tpl.as_str())
                || req.global_blacklist.contains(tpl.as_str())
            {
                return false;
            }

            // Globally reward-blacklisted (`:119`) — `IsItemRewardBlacklisted`, a different list to
            // the two above.
            if req.reward_item_blacklist.contains(tpl.as_str()) {
                return false;
            }

            // `:124`
            if !req.config.allow_boss_items_as_rewards && req.boss_items.contains(tpl.as_str()) {
                return false;
            }

            // Skip item if parent id is blacklisted (`:130`).
            if item_helper::is_of_baseclasses(&req.items_view, tpl, &parent_blacklist) {
                return false;
            }

            // `:135`
            if req.inactive_seasonal_items.contains(tpl.as_str()) {
                return false;
            }

            true
        })
        .map(|(tpl, item)| (tpl.as_str(), item))
        .collect()
}

/// `CacheDbItems`' `DbAmmoItemsCache` filter (`:145-199`).
///
/// Quirk 9, ported verbatim: this is not the reward filter plus a baseclass test. It never checks
/// `QuestItem` (`ScavCaseRewardGenerator.cs:103`) and never checks `RewardItemParentBlacklist`
/// (`ScavCaseRewardGenerator.cs:130`), so quest-item ammo and ammo under a blacklisted parent are
/// both drawable as ammo rewards while the reward pool rejects them.
pub fn build_ammo_pool(req: &ScavCaseRequest) -> Vec<(&str, &ItemView)> {
    req.items_view
        .iter()
        .filter(|(tpl, item)| {
            // Base "Item" item has no parent, ignore it (`:151`).
            if item.parent.as_deref().unwrap_or_default().is_empty() {
                return false;
            }

            // `:156` — this also stands in for the reward filter's separate "Node" test.
            if item.item_type.as_deref() != Some("Item") {
                return false;
            }

            // Not ammo, skip (`:162`).
            if !item_helper::is_of_baseclass(&req.items_view, tpl, AMMO) {
                return false;
            }

            // Skip item if item id is on blacklist (`:168`).
            if req.config.reward_item_blacklist.contains(tpl.as_str())
                || req.global_blacklist.contains(tpl.as_str())
            {
                return false;
            }

            // Globally reward-blacklisted (`:174`).
            if req.reward_item_blacklist.contains(tpl.as_str()) {
                return false;
            }

            // `:179`
            if !req.config.allow_boss_items_as_rewards && req.boss_items.contains(tpl.as_str()) {
                return false;
            }

            // Skip seasonal items (`:185`).
            if req.inactive_seasonal_items.contains(tpl.as_str()) {
                return false;
            }

            // Skip ammo that doesn't stack as high as value in config (`:191`).
            //
            // Quirk 6, ported verbatim: `StackMaxSize` is `int?` and the lifted `<` is false when it
            // is null, so ammo with no stack size at all clears the floor rather than failing it
            // (`ScavCaseRewardGenerator.cs:191`).
            if item.stack_max_size.is_some_and(|stack_max_size| {
                stack_max_size < req.config.ammo_rewards.min_stack_size
            }) {
                return false;
            }

            true
        })
        .map(|(tpl, item)| (tpl.as_str(), item))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::loot::item_helper::AMMO;
    use crate::loot::models::ItemView;
    use crate::scav_case::generator::{build_ammo_pool, build_reward_pool};
    use crate::scav_case::models::ScavCaseRequest;

    /// The base `Item` node — `_parent` is the empty `MongoId`, which the projection writes as null.
    const ITEM_NODE: &str = "54009119af1c881c07000029";
    /// A non-ammo node, so its children fail the ammo pool's baseclass check.
    const MISC_NODE: &str = "cccccccccccccccccccccccc";
    /// In `config.rewardItemParentBlacklist`; itself a child of [`AMMO`].
    const PARENT_BLACKLISTED_NODE: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";

    const GOOD_ITEM_TPL: &str = "111111111111111111111111";
    const QUEST_AMMO_TPL: &str = "222222222222222222222222";
    const CONFIG_BLACKLIST_TPL: &str = "333333333333333333333333";
    const GLOBAL_BLACKLIST_TPL: &str = "444444444444444444444444";
    const REWARD_BLACKLIST_TPL: &str = "555555555555555555555555";
    const BOSS_AMMO_TPL: &str = "666666666666666666666666";
    const PARENT_BLACKLISTED_TPL: &str = "777777777777777777777777";
    const SEASONAL_AMMO_TPL: &str = "888888888888888888888888";
    /// No `_type` at all: `== "Node"` is false, so only the `!= "Item"` check can drop it.
    const TYPELESS_TPL: &str = "999999999999999999999999";
    const AMMO_GOOD_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaa1";
    const AMMO_NULL_STACK_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaa2";
    const AMMO_LOW_STACK_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaa3";

    /// One template per filter rule, in the order the pools must preserve.
    fn request_json() -> Value {
        json!({
            "recipeId": "aaaaaaaaaaaaaaaaaaaaaaaa",
            "scavRecipes": [{"id": "aaaaaaaaaaaaaaaaaaaaaaaa", "endProducts": {
                "common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                "superrare": {"min": 1, "max": 1}}}],
            "config": {
                "rewardItemValueRangeRub": {"common": {"min": 0.0, "max": 1000.0}},
                "moneyRewards": {"moneyRewardChancePercent": 20,
                    "rubCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}},
                    "usdCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}},
                    "eurCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}},
                    "gpCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}}},
                "ammoRewards": {"ammoRewardChancePercent": 15,
                    "ammoRewardValueRangeRub": {"common": {"min": 0.0, "max": 80.0}},
                    "minStackSize": 30},
                "rewardItemParentBlacklist": [PARENT_BLACKLISTED_NODE],
                "rewardItemBlacklist": [CONFIG_BLACKLIST_TPL],
                "allowMultipleMoneyRewardsPerRarity": false,
                "allowMultipleAmmoRewardsPerRarity": false,
                "allowBossItemsAsRewards": false
            },
            "itemsView": {
                ITEM_NODE: {"parent": null, "type": "Node"},
                AMMO: {"parent": ITEM_NODE, "type": "Node"},
                MISC_NODE: {"parent": ITEM_NODE, "type": "Node"},
                PARENT_BLACKLISTED_NODE: {"parent": AMMO, "type": "Node"},
                GOOD_ITEM_TPL: {"parent": MISC_NODE, "type": "Item"},
                QUEST_AMMO_TPL: {"parent": AMMO, "type": "Item", "questItem": true,
                    "stackMaxSize": 60},
                CONFIG_BLACKLIST_TPL: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                GLOBAL_BLACKLIST_TPL: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                REWARD_BLACKLIST_TPL: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                BOSS_AMMO_TPL: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                PARENT_BLACKLISTED_TPL: {"parent": PARENT_BLACKLISTED_NODE, "type": "Item",
                    "stackMaxSize": 60},
                SEASONAL_AMMO_TPL: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                TYPELESS_TPL: {"parent": AMMO, "stackMaxSize": 60},
                AMMO_GOOD_TPL: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                AMMO_NULL_STACK_TPL: {"parent": AMMO, "type": "Item"},
                AMMO_LOW_STACK_TPL: {"parent": AMMO, "type": "Item", "stackMaxSize": 20}
            },
            "staticPrices": {},
            "defaultPresetsByTpl": {},
            "inactiveSeasonalItems": [SEASONAL_AMMO_TPL],
            "globalBlacklist": [GLOBAL_BLACKLIST_TPL],
            "rewardItemBlacklist": [REWARD_BLACKLIST_TPL],
            "bossItems": [BOSS_AMMO_TPL]
        })
    }

    fn request() -> ScavCaseRequest {
        serde_json::from_value(request_json()).unwrap()
    }

    fn tpls(pool: &[(&str, &ItemView)]) -> Vec<String> {
        pool.iter().map(|(tpl, _)| (*tpl).to_owned()).collect()
    }

    #[test]
    fn reward_pool_keeps_survivors_in_items_view_order() {
        assert_eq!(
            tpls(&build_reward_pool(&request())),
            vec![
                GOOD_ITEM_TPL,
                AMMO_GOOD_TPL,
                AMMO_NULL_STACK_TPL,
                AMMO_LOW_STACK_TPL
            ]
        );
    }

    #[test]
    fn reward_pool_drops_one_template_per_rule() {
        let req = request();
        let pool = tpls(&build_reward_pool(&req));

        for (tpl, rule) in [
            (ITEM_NODE, "parent is the empty MongoId (:93)"),
            (AMMO, "_type == Node (:98)"),
            (QUEST_AMMO_TPL, "QuestItem (:103)"),
            (TYPELESS_TPL, "_type != Item (:110)"),
            (CONFIG_BLACKLIST_TPL, "config RewardItemBlacklist (:111)"),
            (GLOBAL_BLACKLIST_TPL, "IsItemBlacklisted (:112)"),
            (REWARD_BLACKLIST_TPL, "IsItemRewardBlacklisted (:119)"),
            (BOSS_AMMO_TPL, "IsBossItem, boss items disallowed (:124)"),
            (
                PARENT_BLACKLISTED_TPL,
                "RewardItemParentBlacklist baseclass (:130)",
            ),
            (SEASONAL_AMMO_TPL, "inactive seasonal (:135)"),
        ] {
            assert!(!pool.contains(&tpl.to_owned()), "{tpl} kept despite {rule}");
        }
    }

    #[test]
    fn ammo_pool_keeps_survivors_in_items_view_order() {
        assert_eq!(
            tpls(&build_ammo_pool(&request())),
            vec![
                QUEST_AMMO_TPL,
                PARENT_BLACKLISTED_TPL,
                AMMO_GOOD_TPL,
                AMMO_NULL_STACK_TPL
            ]
        );
    }

    #[test]
    fn ammo_pool_drops_one_template_per_rule() {
        let req = request();
        let pool = tpls(&build_ammo_pool(&req));

        for (tpl, rule) in [
            (ITEM_NODE, "parent is the empty MongoId (:151)"),
            (AMMO, "_type != Item (:156)"),
            (TYPELESS_TPL, "_type != Item (:156)"),
            (GOOD_ITEM_TPL, "not of baseclass AMMO (:162)"),
            (CONFIG_BLACKLIST_TPL, "config RewardItemBlacklist (:168)"),
            (GLOBAL_BLACKLIST_TPL, "IsItemBlacklisted (:168)"),
            (REWARD_BLACKLIST_TPL, "IsItemRewardBlacklisted (:174)"),
            (BOSS_AMMO_TPL, "IsBossItem, boss items disallowed (:179)"),
            (SEASONAL_AMMO_TPL, "inactive seasonal (:185)"),
            (AMMO_LOW_STACK_TPL, "StackMaxSize < MinStackSize (:191)"),
        ] {
            assert!(!pool.contains(&tpl.to_owned()), "{tpl} kept despite {rule}");
        }
    }

    /// Quirk 6: `StackMaxSize` is `int?`, and `null < int` is false, so ammo with no stack size
    /// never trips the floor (`:191`).
    #[test]
    fn ammo_pool_keeps_ammo_with_null_stack_max_size() {
        let req = request();

        assert!(req.items_view[AMMO_NULL_STACK_TPL].stack_max_size.is_none());
        assert!(tpls(&build_ammo_pool(&req)).contains(&AMMO_NULL_STACK_TPL.to_owned()));
    }

    /// Quirk 9: the ammo filter is not the reward filter plus a baseclass test — it checks neither
    /// `QuestItem` (`:103`) nor `RewardItemParentBlacklist` (`:130`), so both templates the reward
    /// pool drops for those reasons stay in the ammo pool.
    #[test]
    fn ammo_pool_skips_the_quest_item_and_parent_blacklist_checks() {
        let req = request();
        let reward_pool = tpls(&build_reward_pool(&req));
        let ammo_pool = tpls(&build_ammo_pool(&req));

        for tpl in [QUEST_AMMO_TPL, PARENT_BLACKLISTED_TPL] {
            assert!(!reward_pool.contains(&tpl.to_owned()));
            assert!(ammo_pool.contains(&tpl.to_owned()));
        }
    }

    #[test]
    fn both_pools_keep_boss_items_when_the_config_allows_them() {
        let mut json = request_json();
        json["config"]["allowBossItemsAsRewards"] = json!(true);
        let req: ScavCaseRequest = serde_json::from_value(json).unwrap();

        assert_eq!(
            tpls(&build_reward_pool(&req)),
            vec![
                GOOD_ITEM_TPL,
                BOSS_AMMO_TPL,
                AMMO_GOOD_TPL,
                AMMO_NULL_STACK_TPL,
                AMMO_LOW_STACK_TPL
            ]
        );
        assert_eq!(
            tpls(&build_ammo_pool(&req)),
            vec![
                QUEST_AMMO_TPL,
                BOSS_AMMO_TPL,
                PARENT_BLACKLISTED_TPL,
                AMMO_GOOD_TPL,
                AMMO_NULL_STACK_TPL
            ]
        );
    }
}
