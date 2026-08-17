//! `Generators/ScavCaseRewardGenerator.cs`.

use std::collections::HashMap;

use crate::bot::repair_service::MinMax;
use crate::diag::DiagSink;
use crate::loot::item_helper::{self, AMMO};
use crate::loot::models::{Diagnostic, ItemView, WARNING};
use crate::loot::random_util::{get_array_value, get_chance_100, get_int};
use crate::scav_case::ScavCaseError;
use crate::scav_case::models::{ScavCaseConfigView, ScavCaseRequest, ScavRecipeView};

/// The `typeof(T).FullName` this file's diagnostics log under.
const CATEGORY: &str = "SPTarkov.Server.Core.Generators.ScavCaseRewardGenerator";

/// `Models/Enums/Money.cs:7`.
const ROUBLES: &str = "5449016a4bdc2d6f028b456f";
/// `Models/Enums/Money.cs:8`.
const EUROS: &str = "569668774bdc2da2298b4568";
/// `Models/Enums/Money.cs:9`.
const DOLLARS: &str = "5696686a4bdc2da3298b456a";
/// `Models/Enums/Money.cs:10`.
const GP: &str = "5d235b4d86f7742e017bc88a";

/// `RewardRarity` (`:504-509`) — the keys the config's rarity maps are read by.
const COMMON: &str = "common";
/// See [`COMMON`].
const RARE: &str = "rare";
/// See [`COMMON`].
const SUPERRARE: &str = "superrare";

/// `CacheDbItems`' `DbItemsCache` filter (`:87-143`).
///
/// C# caches the two lists on the generator instance and refills them only when empty; there is no
/// instance to hang them off here, so each request rebuilds them. The filter reads nothing but the
/// request, so the pool is identical either way.
///
/// `TemplateItem` carries its own `Id`, [`ItemView`] does not — hence the tpl in the tuple, which
/// every caller of the pool needs (prices, presets, baseclass tests are all keyed by it).
///
/// Still `pub` rather than `pub(crate)` only because nothing outside the tests calls it yet;
/// `Generate` is the caller that tightens it. [`build_ammo_pool`] already has one in
/// [`get_random_ammo`].
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
pub(crate) fn build_ammo_pool(req: &ScavCaseRequest) -> Vec<(&str, &ItemView)> {
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

/// `Models/Spt/Hideout/ScavCaseRewardCountsAndPrices.cs:17-31`. Every member is a `double?` there
/// and every one is assigned unconditionally at `:403-420`, so they are plain `f64` here — counts
/// included, which is why `:215` casts them back to `int` to draw with.
pub struct RewardCountAndPriceDetails {
    pub min_count: f64,
    pub max_count: f64,
    pub min_price_rub: f64,
    pub max_price_rub: f64,
}

/// `GetScavCaseRewardCountsAndPrices` (`:396-423`) — the recipe's three end-product counts paired
/// with the config's three price ranges, common/rare/superrare in that order.
///
/// # Panics
///
/// If the config is missing a rarity, where the C# dictionary index (`:405`) throws
/// `KeyNotFoundException`.
pub fn get_reward_counts_and_prices(
    scav_case_details: &ScavRecipeView,
    config: &ScavCaseConfigView,
) -> (
    RewardCountAndPriceDetails,
    RewardCountAndPriceDetails,
    RewardCountAndPriceDetails,
) {
    let details = |end_products: &MinMax<i32>, rarity: &str| RewardCountAndPriceDetails {
        min_count: f64::from(end_products.min),
        max_count: f64::from(end_products.max),
        min_price_rub: config.reward_item_value_range_rub[rarity].min,
        max_price_rub: config.reward_item_value_range_rub[rarity].max,
    };
    let end_products = &scav_case_details.end_products;

    (
        details(&end_products.common, COMMON),
        details(&end_products.rare, RARE),
        details(&end_products.superrare, SUPERRARE),
    )
}

/// `GetFilteredItemsByPrice` (`:375-389`) — the reward cache narrowed to one rarity's price band,
/// both ends inclusive.
pub fn get_filtered_items_by_price<'a>(
    db_items: &[(&'a str, &'a ItemView)],
    item_filters: &RewardCountAndPriceDetails,
    static_prices: &HashMap<String, f64>,
) -> Vec<(&'a str, &'a ItemView)> {
    db_items
        .iter()
        .filter(|(tpl, _)| {
            let handbook_price = static_price(static_prices, tpl);

            handbook_price >= item_filters.min_price_rub
                && handbook_price <= item_filters.max_price_rub
        })
        .copied()
        .collect()
}

/// `ragfairPriceService.GetStaticPriceForItem` (`:288,380`). The `double?` it returns is never
/// actually null: `HandbookHelper.GetTemplatePrice` (`Helpers/Profile/HandbookHelper.cs:106-125`)
/// answers 0 for a template with no handbook entry, so a priceless item clears a floor of 0 rather
/// than failing the comparison.
fn static_price(static_prices: &HashMap<String, f64>, tpl: &str) -> f64 {
    static_prices.get(tpl).copied().unwrap_or(0.0)
}

/// `PickRandomRewards` (`:209-243`) — the rewards for one rarity, money and ammo mixed in by chance.
///
/// Quirk 1, the draw order this whole function exists to preserve: the money chance is drawn on
/// *every* iteration (`:218`), before `!reward_was_money` can short-circuit it, and the ammo chance
/// is drawn on every iteration the money branch did not take (`:227`) — capped or not. Each flag is
/// set only when its `allow_multiple_*` config is false (`:222-225`, `:231-234`), so an unset flag
/// lets the branch fire again.
///
/// # Errors
///
/// Where the C# throws: an empty `items` pool (`:238`, `InvalidOperationException` out of
/// `RandomUtil.GetRandomElement`), or the empty ammo pool of [`get_random_ammo`].
pub fn pick_random_rewards<'a>(
    req: &'a ScavCaseRequest,
    items: &[(&'a str, &'a ItemView)],
    item_filters: &RewardCountAndPriceDetails,
    rarity: &str,
    diagnostics: &mut DiagSink,
) -> Result<Vec<(&'a str, &'a ItemView)>, ScavCaseError> {
    let mut result = Vec::new();

    let mut reward_was_money = false;
    let mut reward_was_ammo = false;
    // `:215` — the `(int)` casts off the `double?` counts.
    let random_count = get_int(item_filters.min_count as i32, item_filters.max_count as i32);
    for _ in 0..random_count {
        if reward_should_be_money(req) && !reward_was_money {
            // Only allow one reward to be money
            result.push(get_random_money(req));
            if !req.config.allow_multiple_money_rewards_per_rarity {
                reward_was_money = true;
            }
        } else if reward_should_be_ammo(req) && !reward_was_ammo {
            // Only allow one reward to be ammo
            result.push(get_random_ammo(req, rarity, diagnostics)?);
            if !req.config.allow_multiple_ammo_rewards_per_rarity {
                reward_was_ammo = true;
            }
        } else {
            // `:238`. `GetArrayValue` takes its `IList` path here, which throws this exact message
            // on an empty pool (`RandomUtil.cs:165`) before drawing anything.
            if items.is_empty() {
                return Err(ScavCaseError::new("Sequence contains no elements."));
            }

            result.push(*get_array_value(items));
        }
    }

    Ok(result)
}

/// `RewardShouldBeMoney` (`:249-252`).
fn reward_should_be_money(req: &ScavCaseRequest) -> bool {
    get_chance_100(f64::from(
        req.config.money_rewards.money_reward_chance_percent,
    ))
}

/// `RewardShouldBeAmmo` (`:258-261`).
fn reward_should_be_ammo(req: &ScavCaseRequest) -> bool {
    get_chance_100(f64::from(
        req.config.ammo_rewards.ammo_reward_chance_percent,
    ))
}

/// `GetRandomMoney` (`:266-276`).
///
/// Quirk 2: the pool is built in the fixed order `[ROUBLES, EUROS, DOLLARS, GP]` (`:270-273`) and
/// then index-drawn, so that order is part of the stream. All four are looked up before the draw,
/// as the C# adds all four before calling `GetArrayValue`.
///
/// # Panics
///
/// If a money template is missing from the items view, where the C# `templateTable.Items[...]`
/// index throws `KeyNotFoundException`.
fn get_random_money(req: &ScavCaseRequest) -> (&str, &ItemView) {
    let items = &req.items_view;
    let money = [
        (ROUBLES, &items[ROUBLES]),
        (EUROS, &items[EUROS]),
        (DOLLARS, &items[DOLLARS]),
        (GP, &items[GP]),
    ];

    *get_array_value(&money)
}

/// `GetRandomAmmo` (`:283-309`) — the ammo cache narrowed to the rarity's price band, index-drawn.
///
/// Quirk 3: an empty filtered pool is only warned about (`:301-305`); the C# then hands the empty
/// sequence to `GetArrayValue` anyway, which `ToList()`s it, short-circuits `GetInt(0, -1)` to 0
/// without drawing and throws indexing it (`:308`). Ported as the warning plus that failure, with
/// the stream untouched either way.
///
/// # Errors
///
/// Where the C# throws: no ammo inside the rarity's price band.
fn get_random_ammo<'a>(
    req: &'a ScavCaseRequest,
    rarity: &str,
    diagnostics: &mut DiagSink,
) -> Result<(&'a str, &'a ItemView), ScavCaseError> {
    let ammo_reward_value_range_rub = &req.config.ammo_rewards.ammo_reward_value_range_rub;
    // C# filters its `DbAmmoItemsCache` (`:285`); see [`build_ammo_pool`] for why this rebuilds it.
    let possible_ammo_pool: Vec<(&str, &ItemView)> = build_ammo_pool(req)
        .into_iter()
        .filter(|(tpl, _)| {
            // Is ammo handbook price between desired range (`:288-296`). A rarity the config does
            // not list misses the `TryGetValue` and fails every ammo, rather than throwing.
            let handbook_price = static_price(&req.static_prices, tpl);

            ammo_reward_value_range_rub
                .get(rarity)
                .is_some_and(|matching_ammo_reward_for_rarity| {
                    handbook_price >= matching_ammo_reward_for_rarity.min
                        && handbook_price <= matching_ammo_reward_for_rarity.max
                })
        })
        .collect();

    if possible_ammo_pool.is_empty() {
        // Filtered pool is empty
        diagnostics.push(Diagnostic {
            category: CATEGORY,
            level: WARNING.to_owned(),
            locale_key: Some("scavcase-no_cartridges_found_matching_price".to_owned()),
            args: None,
            message: None,
        });

        return Err(ScavCaseError::new(format!(
            "No cartridges found matching the price range for rarity: {rarity}"
        )));
    }

    // Get a random ammo and return it
    Ok(*get_array_value(&possible_ammo_pool))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::diag::DiagSink;
    use crate::loot::item_helper::{AMMO, MONEY};
    use crate::loot::models::{ItemView, WARNING};
    use crate::loot::random_util::{TestSeedGuard, get_chance_100, get_int};
    use crate::scav_case::generator::{
        DOLLARS, EUROS, GP, ROUBLES, RewardCountAndPriceDetails, build_ammo_pool,
        build_reward_pool, get_filtered_items_by_price, get_reward_counts_and_prices,
        pick_random_rewards,
    };
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

    // ---- Reward picking (`:209-309`) and the two inputs it is handed (`:375-423`) ----

    /// Four reward templates, inserted into `itemsView` in an order that is deliberately not their
    /// sorted order, so an index draw over the pool tells real map order apart from lexicographic
    /// tpl order.
    const PICK_D: &str = "d1d1d1d1d1d1d1d1d1d1d1d1";
    const PICK_B: &str = "b2b2b2b2b2b2b2b2b2b2b2b2";
    const PICK_A: &str = "a3a3a3a3a3a3a3a3a3a3a3a3";
    const PICK_C: &str = "c4c4c4c4c4c4c4c4c4c4c4c4";
    /// Ammo, likewise unsorted; only the last two are inside the common ammo price range.
    const AMMO_DEAR: &str = "e3e3e3e3e3e3e3e3e3e3e3e3";
    const AMMO_MID: &str = "e2e2e2e2e2e2e2e2e2e2e2e2";
    const AMMO_CHEAP: &str = "e1e1e1e1e1e1e1e1e1e1e1e1";

    /// The seed every picking test runs under. In the three-reward run below it draws money index 2
    /// and pool indices 0 and 3 — none of which coincide with sorted order, so the assertions
    /// discriminate.
    const SEED: u64 = 42;

    /// The two chances are all the picking tests vary; everything else, `allow_multiple_*` included,
    /// is fixed. Prices are chosen so the common reward range (100-1000)
    /// selects exactly the four `PICK_*` templates: the ammo sits outside it and the money
    /// templates are absent from `staticPrices` altogether, which is the C# handbook miss (price 0).
    fn pick_request_json(money_chance: i32, ammo_chance: i32) -> Value {
        json!({
            "recipeId": "aaaaaaaaaaaaaaaaaaaaaaaa",
            "scavRecipes": [{"id": "aaaaaaaaaaaaaaaaaaaaaaaa", "endProducts": {
                "common": {"min": 3, "max": 3}, "rare": {"min": 1, "max": 2},
                "superrare": {"min": 0, "max": 0}}}],
            "config": {
                "rewardItemValueRangeRub": {
                    "common": {"min": 100.0, "max": 1000.0},
                    "rare": {"min": 1000.0, "max": 5000.0},
                    "superrare": {"min": 5000.0, "max": 50000.0}},
                "moneyRewards": {"moneyRewardChancePercent": money_chance,
                    "rubCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}},
                    "usdCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}},
                    "eurCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}},
                    "gpCount": {"common": {"min": 1, "max": 2}, "rare": {"min": 1, "max": 2},
                        "superrare": {"min": 1, "max": 2}}},
                "ammoRewards": {"ammoRewardChancePercent": ammo_chance,
                    "ammoRewardValueRangeRub": {"common": {"min": 0.0, "max": 80.0}},
                    "minStackSize": 30},
                "rewardItemParentBlacklist": [],
                "rewardItemBlacklist": [],
                "allowMultipleMoneyRewardsPerRarity": false,
                "allowMultipleAmmoRewardsPerRarity": false,
                "allowBossItemsAsRewards": true
            },
            "itemsView": {
                ITEM_NODE: {"parent": null, "type": "Node"},
                AMMO: {"parent": ITEM_NODE, "type": "Node"},
                MONEY: {"parent": ITEM_NODE, "type": "Node"},
                MISC_NODE: {"parent": ITEM_NODE, "type": "Node"},
                PICK_D: {"parent": MISC_NODE, "type": "Item"},
                PICK_B: {"parent": MISC_NODE, "type": "Item"},
                PICK_A: {"parent": MISC_NODE, "type": "Item"},
                PICK_C: {"parent": MISC_NODE, "type": "Item"},
                AMMO_DEAR: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                AMMO_MID: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                AMMO_CHEAP: {"parent": AMMO, "type": "Item", "stackMaxSize": 60},
                ROUBLES: {"parent": MONEY, "type": "Item", "stackMaxSize": 500000},
                EUROS: {"parent": MONEY, "type": "Item", "stackMaxSize": 500000},
                DOLLARS: {"parent": MONEY, "type": "Item", "stackMaxSize": 500000},
                GP: {"parent": MONEY, "type": "Item", "stackMaxSize": 500000}
            },
            "staticPrices": {PICK_D: 100.0, PICK_B: 500.0, PICK_A: 1000.0, PICK_C: 750.0,
                AMMO_DEAR: 5000.0, AMMO_MID: 50.0, AMMO_CHEAP: 10.0},
            "defaultPresetsByTpl": {},
            "inactiveSeasonalItems": [],
            "globalBlacklist": [],
            "rewardItemBlacklist": [],
            "bossItems": []
        })
    }

    fn pick_request(money_chance: i32, ammo_chance: i32) -> ScavCaseRequest {
        serde_json::from_value(pick_request_json(money_chance, ammo_chance)).unwrap()
    }

    /// The common-rarity pool the C# hands `PickRandomRewards`: the reward cache, price-filtered.
    fn common_pool(req: &ScavCaseRequest) -> Vec<(&str, &ItemView)> {
        let (common, _, _) = get_reward_counts_and_prices(&req.scav_recipes[0], &req.config);

        get_filtered_items_by_price(&build_reward_pool(req), &common, &req.static_prices)
    }

    /// Exactly one reward, over a pool the test hands in ready-filtered — `GetInt(1, 1)` returns
    /// without drawing, so the stream starts on the first chance roll.
    fn one_reward() -> RewardCountAndPriceDetails {
        RewardCountAndPriceDetails {
            min_count: 1.0,
            max_count: 1.0,
            min_price_rub: 0.0,
            max_price_rub: 0.0,
        }
    }

    /// A price band alone; `GetFilteredItemsByPrice` reads neither count.
    fn price_band(min_price_rub: f64, max_price_rub: f64) -> RewardCountAndPriceDetails {
        RewardCountAndPriceDetails {
            min_count: 0.0,
            max_count: 0.0,
            min_price_rub,
            max_price_rub,
        }
    }

    #[test]
    fn reward_counts_and_prices_pair_the_end_products_with_the_config_ranges() {
        let req = pick_request(0, 0);
        let (common, rare, superrare) =
            get_reward_counts_and_prices(&req.scav_recipes[0], &req.config);

        assert_eq!((common.min_count, common.max_count), (3.0, 3.0));
        assert_eq!(
            (common.min_price_rub, common.max_price_rub),
            (100.0, 1000.0)
        );
        assert_eq!((rare.min_count, rare.max_count), (1.0, 2.0));
        assert_eq!((rare.min_price_rub, rare.max_price_rub), (1000.0, 5000.0));
        assert_eq!((superrare.min_count, superrare.max_count), (0.0, 0.0));
        assert_eq!(
            (superrare.min_price_rub, superrare.max_price_rub),
            (5000.0, 50000.0)
        );
    }

    /// `:381` — `>= Min && <= Max`, both ends inclusive. `PICK_D` sits exactly on the floor and
    /// `PICK_A` exactly on the ceiling, so a `>`/`<` either side drops one of them.
    #[test]
    fn filtered_items_by_price_is_inclusive_at_both_ends() {
        let req = pick_request(0, 0);
        let pool = build_reward_pool(&req);

        let inclusive =
            get_filtered_items_by_price(&pool, &price_band(100.0, 1000.0), &req.static_prices);
        let exclusive =
            get_filtered_items_by_price(&pool, &price_band(101.0, 999.0), &req.static_prices);

        // Insertion order, not sorted order — the pool is filtered out of `itemsView`.
        assert_eq!(tpls(&inclusive), vec![PICK_D, PICK_B, PICK_A, PICK_C]);
        assert_eq!(tpls(&exclusive), vec![PICK_B, PICK_C]);
    }

    /// A template with no `staticPrices` entry is not skipped: `GetStaticPriceForItem` answers 0 for
    /// a handbook miss (`HandbookHelper.cs:106-125`), so it passes a floor of 0.
    #[test]
    fn filtered_items_by_price_treats_a_missing_price_as_zero() {
        let req = pick_request(0, 0);
        let free = get_filtered_items_by_price(
            &build_reward_pool(&req),
            &price_band(0.0, 0.0),
            &req.static_prices,
        );

        assert!(!req.static_prices.contains_key(ROUBLES));
        assert_eq!(tpls(&free), vec![ROUBLES, EUROS, DOLLARS, GP]);
    }

    /// Quirk 1 (`:218`): the money chance is drawn *every* iteration, before `!rewardWasMoney` can
    /// short-circuit it, and the ammo chance is drawn on every iteration the money branch did not
    /// take. Three iterations at 100% money / 0% ammo therefore spend eight draws — chance, money
    /// index, chance, chance, pool index, chance, chance, pool index — on top of the `GetInt(3, 3)`
    /// count, which returns without drawing.
    #[test]
    fn a_capped_money_reward_still_costs_the_stream_its_chance_draw() {
        let req = pick_request(100, 0);
        let pool = common_pool(&req);
        let (common, _, _) = get_reward_counts_and_prices(&req.scav_recipes[0], &req.config);
        let mut diagnostics = DiagSink::capture();

        let picked = {
            let _guard = TestSeedGuard::install(SEED);
            pick_random_rewards(&req, &pool, &common, "common", &mut diagnostics).unwrap()
        };

        // One money reward only, and the two that follow come from the pool in `itemsView` order.
        assert_eq!(tpls(&picked), vec![DOLLARS, PICK_D, PICK_C]);

        // What the same seed would have produced had the capped iterations skipped their money
        // chance draw — the mistake quirk 1 guards against. Different, so the assertion above is
        // not passing by luck.
        let skipped_the_capped_draws = {
            let _guard = TestSeedGuard::install(SEED);
            assert!(get_chance_100(100.0));
            let money = get_int(0, 3);
            assert!(!get_chance_100(0.0));
            let first = get_int(0, 3);
            assert!(!get_chance_100(0.0));
            let second = get_int(0, 3);
            vec![
                [ROUBLES, EUROS, DOLLARS, GP][money as usize].to_owned(),
                tpls(&pool)[first as usize].clone(),
                tpls(&pool)[second as usize].clone(),
            ]
        };
        assert_ne!(tpls(&picked), skipped_the_capped_draws);
    }

    /// Quirk 2 (`:270-273`): the money pool is `[ROUBLES, EUROS, DOLLARS, GP]` and the draw is an
    /// index into it. This seed draws index 2 — `DOLLARS` under that order, `EUROS` under sorted
    /// tpl order.
    #[test]
    fn the_money_pool_is_roubles_euros_dollars_gp() {
        let req = pick_request(100, 0);
        let pool = common_pool(&req);
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked =
            pick_random_rewards(&req, &pool, &one_reward(), "common", &mut diagnostics).unwrap();

        assert_eq!(tpls(&picked), vec![DOLLARS]);
    }

    /// The ammo branch price-filters the ammo cache against `ammoRewardValueRangeRub[rarity]`
    /// (`:288-297`) and draws an index out of what survives, in `itemsView` order.
    #[test]
    fn the_ammo_branch_draws_from_the_price_filtered_ammo_cache() {
        let req = pick_request(0, 100);
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked =
            pick_random_rewards(&req, &[], &one_reward(), "common", &mut diagnostics).unwrap();

        // 5000 rub is outside the 0-80 range, so `AMMO_DEAR` cannot come out. What survives is
        // `[AMMO_MID, AMMO_CHEAP]` in `itemsView` order and the draw takes index 1 — `AMMO_CHEAP`
        // under that order, `AMMO_MID` under sorted tpl order.
        assert_eq!(tpls(&picked), vec![AMMO_CHEAP]);
        assert!(diagnostics.captured().is_empty());
    }

    /// Quirk 3 (`:301-308`): a rarity absent from `ammoRewardValueRangeRub` fails the `TryGetValue`
    /// for every ammo, so the filtered pool is empty — C# warns and then indexes it anyway, which
    /// throws. The warning is emitted and the throw becomes the error the caller propagates.
    #[test]
    fn an_ammo_rarity_without_a_price_range_warns_and_then_fails() {
        let req = pick_request(0, 100);
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked = pick_random_rewards(&req, &[], &one_reward(), "rare", &mut diagnostics);

        assert!(picked.is_err());
        assert!(
            !req.config
                .ammo_rewards
                .ammo_reward_value_range_rub
                .contains_key("rare")
        );
        assert_eq!(diagnostics.captured().len(), 1);
        assert_eq!(diagnostics.captured()[0].level, WARNING);
        assert_eq!(
            diagnostics.captured()[0].locale_key.as_deref(),
            Some("scavcase-no_cartridges_found_matching_price")
        );
    }

    /// `:238` — `GetArrayValue` over the reward pool, which is a `List`: an empty one throws
    /// `InvalidOperationException` before any draw.
    #[test]
    fn an_empty_reward_pool_fails_the_pick() {
        let req = pick_request(0, 0);
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);

        assert!(pick_random_rewards(&req, &[], &one_reward(), "common", &mut diagnostics).is_err());
    }
}
