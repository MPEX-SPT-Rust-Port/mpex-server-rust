//! `Generators/ScavCaseRewardGenerator.cs`.

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::bot::repair_service::MinMax;
use crate::diag::DiagSink;
use crate::loot::item_helper::{self, AMMO, AMMO_BOX, MONEY, WEAPON};
use crate::loot::models::{Diagnostic, Item, ItemView, PresetView, Upd, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{get_array_value, get_chance_100, get_int};
use crate::scav_case::models::{
    MoneyLevelsView, ScavCaseConfigView, ScavCaseResponse, ScavCaseVarying, ScavRecipeView,
};
use crate::scav_case::{ScavCaseError, ScavCaseViews, StaticPrices};

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

/// What used to be the flattened request, resolved to borrows: the per-request view mapping out
/// of [`ScavCaseViews`] plus the varying block's config and service-backed sets — so the ported
/// bodies below keep reading `req.*` verbatim, as the loot family's `RewardDbRefs` does.
pub(crate) struct ScavCaseRefs<'a> {
    recipe_id: &'a str,
    scav_recipes: &'a [ScavRecipeView],
    config: &'a ScavCaseConfigView,
    items_view: &'a IndexMap<String, ItemView>,
    static_prices: StaticPrices<'a>,
    default_presets_by_tpl: &'a IndexMap<String, PresetView>,
    inactive_seasonal_items: &'a HashSet<String>,
    global_blacklist: &'a HashSet<String>,
    reward_item_blacklist: &'a HashSet<String>,
    boss_items: &'a HashSet<String>,
}

impl<'a> ScavCaseRefs<'a> {
    /// Binds the varying block to the resolved views' borrows.
    pub(crate) fn new(varying: &'a ScavCaseVarying, views: &'a ScavCaseViews) -> Self {
        Self {
            recipe_id: &varying.recipe_id,
            scav_recipes: views.scav_recipes(),
            config: &varying.config,
            items_view: views.items_view(),
            static_prices: views.static_prices(),
            default_presets_by_tpl: views.default_presets_by_tpl(),
            inactive_seasonal_items: &varying.inactive_seasonal_items,
            global_blacklist: &varying.global_blacklist,
            reward_item_blacklist: &varying.reward_item_blacklist,
            boss_items: &varying.boss_items,
        }
    }
}

/// `CacheDbItems`' `DbItemsCache` filter (`:87-143`).
///
/// C# caches the two lists on the generator instance and refills them only when empty; there is no
/// instance to hang them off here, so each request rebuilds them. The filter reads nothing but the
/// request, so the pool is identical either way.
///
/// `TemplateItem` carries its own `Id`, [`ItemView`] does not — hence the tpl in the tuple, which
/// every caller of the pool needs (prices, presets, baseclass tests are all keyed by it).
pub(crate) fn build_reward_pool<'a>(req: &ScavCaseRefs<'a>) -> Vec<(&'a str, &'a ItemView)> {
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
            if item_helper::is_of_baseclasses(req.items_view, tpl, &parent_blacklist) {
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
pub(crate) fn build_ammo_pool<'a>(req: &ScavCaseRefs<'a>) -> Vec<(&'a str, &'a ItemView)> {
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
            if !item_helper::is_of_baseclass(req.items_view, tpl, AMMO) {
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
    static_prices: StaticPrices<'_>,
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
fn static_price(static_prices: StaticPrices<'_>, tpl: &str) -> f64 {
    static_prices.get(tpl).unwrap_or(0.0)
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
pub(crate) fn pick_random_rewards<'a>(
    req: &ScavCaseRefs<'a>,
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
fn reward_should_be_money(req: &ScavCaseRefs<'_>) -> bool {
    get_chance_100(f64::from(
        req.config.money_rewards.money_reward_chance_percent,
    ))
}

/// `RewardShouldBeAmmo` (`:258-261`).
fn reward_should_be_ammo(req: &ScavCaseRefs<'_>) -> bool {
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
fn get_random_money<'a>(req: &ScavCaseRefs<'a>) -> (&'a str, &'a ItemView) {
    let items = req.items_view;
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
    req: &ScavCaseRefs<'a>,
    rarity: &str,
    diagnostics: &mut DiagSink,
) -> Result<(&'a str, &'a ItemView), ScavCaseError> {
    let ammo_reward_value_range_rub = &req.config.ammo_rewards.ammo_reward_value_range_rub;
    // C# filters its `DbAmmoItemsCache` (`:285`); see [`build_ammo_pool`] for why this rebuilds it.
    // Ceiling: that rebuild is a full items-view scan *per ammo draw*, where C# scans once per
    // generator instance. At most 7 rewards on a cold path, so it stays under the request's own
    // transport cost — do not copy this shape onto a hot path without hoisting the pool.
    let possible_ammo_pool: Vec<(&str, &ItemView)> = build_ammo_pool(req)
        .into_iter()
        .filter(|(tpl, _)| {
            // Is ammo handbook price between desired range (`:288-296`). A rarity the config does
            // not list misses the `TryGetValue` and fails every ammo, rather than throwing.
            let handbook_price = static_price(req.static_prices, tpl);

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

/// `RandomiseContainerItemRewards` (`:318-368`) — the picks turned into reward groups: each is the
/// reward item plus whatever children its branch gives it.
///
/// The branch order is the C#'s and the branches are exclusive: ammo box (`:335`), then armor or
/// weapon (`:340-342`), then the `[AMMO, MONEY]` stack path (`:359`).
///
/// # Errors
///
/// Where the C# throws: `AddCartridgesToAmmoBox` on a box naming no cartridge or a cartridge with no
/// stack size (`ItemHelper.cs:1245,1266`).
fn randomise_container_item_rewards(
    req: &ScavCaseRefs<'_>,
    reward_items: &[(&str, &ItemView)],
    rarity: &str,
    diagnostics: &mut DiagSink,
) -> Result<Vec<Vec<Item>>, ScavCaseError> {
    // Each array is an item + children
    let mut result: Vec<Vec<Item>> = Vec::new();
    for (reward_item_tpl, reward_item_db) in reward_items {
        // `:324-332`. `Upd` starts null and only the stack branch below fills it in.
        let mut result_item = vec![Item {
            id: mongo_id::generate(),
            template: (*reward_item_tpl).to_owned(),
            upd: None,
            ..Default::default()
        }];

        if item_helper::is_of_baseclass(req.items_view, reward_item_tpl, AMMO_BOX) {
            // `:335-337`
            item_helper::add_cartridges_to_ammo_box(
                req.items_view,
                &mut result_item,
                reward_item_tpl,
            )
            .map_err(|failure| ScavCaseError::new(failure.message.unwrap_or_default()))?;
        }
        // Armor or weapon = use default preset from globals.json (`:340-342`)
        else if item_helper::armor_item_has_removable_or_soft_insert_slots(
            req.items_view,
            reward_item_tpl,
        ) || item_helper::is_of_baseclass(req.items_view, reward_item_tpl, WEAPON)
        {
            // Quirk 5 (`:345-351`): a tpl with no default preset is warned about — in interpolated
            // text, not a locale key — and skipped, so the reward is dropped outright.
            let Some(preset) = req.default_presets_by_tpl.get(*reward_item_tpl) else {
                diagnostics.push(Diagnostic {
                    category: CATEGORY,
                    level: WARNING.to_owned(),
                    locale_key: None,
                    args: None,
                    message: Some(format!(
                        "No preset for item: {reward_item_tpl} {}, skipping",
                        // `TemplateItem.Name` is nullable and interpolates as the empty string.
                        reward_item_db.name.as_deref().unwrap_or_default()
                    )),
                });

                continue;
            };

            // Ensure preset has unique ids and is cloned so we don't alter the preset data stored
            // in memory (`:354-357`) — the *whole* result item is replaced, minted root included.
            //
            // `RemapRootItemId` is a no-op on anything but the root's id value: `ReplaceIDs` has
            // already re-idded the tree consistently, so all the second pass does is mint the root
            // one more id and repoint its children at it. Ported because the C# does it.
            let mut preset_and_mods = preset.items.clone();
            item_helper::replace_ids(&mut preset_and_mods);
            item_helper::remap_root_item_id(&mut preset_and_mods);

            result_item = preset_and_mods;
        } else if item_helper::is_of_baseclasses(req.items_view, reward_item_tpl, &[AMMO, MONEY]) {
            // `:359-362`. The gate is an ancestor walk but the draw below keys on the direct
            // parent (quirk 4), so an item that only passes here through a grandparent still gets
            // an `Upd` — carrying the else branch's 1.
            result_item[0].upd = Some(Upd {
                stack_objects_count: Some(f64::from(get_random_amount_reward_for_scav_case(
                    req,
                    reward_item_tpl,
                    reward_item_db,
                    rarity,
                ))),
                ..Default::default()
            });
        }

        result.push(result_item);
    }

    Ok(result)
}

/// `GetRandomAmountRewardForScavCase` (`:431-447`).
///
/// Quirk 4: this keys on the item's **direct parent** (`:433`) where its `:359` gate walks the whole
/// ancestor chain, so a grandchild of `AMMO` reaches neither branch and takes the else's 1.
fn get_random_amount_reward_for_scav_case(
    req: &ScavCaseRefs<'_>,
    tpl: &str,
    item_to_calculate: &ItemView,
    rarity: &str,
) -> i32 {
    let parent_id = item_to_calculate.parent.as_deref().unwrap_or_default();

    if parent_id == AMMO {
        get_randomised_ammo_reward_stack_size(req, item_to_calculate)
    } else if parent_id == MONEY {
        get_randomised_money_reward_stack_size(req, tpl, rarity)
    } else {
        1
    }
}

/// `GetRandomisedAmmoRewardStackSize` (`:454-457`).
///
/// A template with no `StackMaxSize` (quirk 6 lets one into the ammo pool) coalesces to a max of 0,
/// which is below the floor — `GetInt` returns the floor without drawing at all
/// (`RandomUtil.cs:48`).
fn get_randomised_ammo_reward_stack_size(
    req: &ScavCaseRefs<'_>,
    item_to_calculate: &ItemView,
) -> i32 {
    get_int(
        req.config.ammo_rewards.min_stack_size,
        item_to_calculate.stack_max_size.unwrap_or(0),
    )
}

/// `GetRandomisedMoneyRewardStackSize` (`:465-501`) — each currency reads its own count map, and
/// `EUROS` reads `EurCount` while `DOLLARS` reads `UsdCount`.
fn get_randomised_money_reward_stack_size(req: &ScavCaseRefs<'_>, tpl: &str, rarity: &str) -> i32 {
    let money_rewards = &req.config.money_rewards;

    let count = if tpl == ROUBLES {
        money_level(&money_rewards.rub_count, rarity)
    } else if tpl == EUROS {
        money_level(&money_rewards.eur_count, rarity)
    } else if tpl == DOLLARS {
        money_level(&money_rewards.usd_count, rarity)
    } else if tpl == GP {
        money_level(&money_rewards.gp_count, rarity)
    } else {
        return 1;
    };

    get_int(count.min, count.max)
}

/// `GetByJsonProperty<MinMax<int>>(rarity)` (`:472`) — the rarity level under its JSON property
/// name.
///
/// # Panics
///
/// On a rarity none of the three members is named after, where the C# reflection lookup answers
/// null and the `.Min` that follows throws `NullReferenceException`. `Generate` only ever passes
/// the three [`COMMON`]/[`RARE`]/[`SUPERRARE`] constants.
fn money_level<'a>(money_levels: &'a MoneyLevelsView, rarity: &str) -> &'a MinMax<i32> {
    match rarity {
        COMMON => &money_levels.common,
        RARE => &money_levels.rare,
        SUPERRARE => &money_levels.superrare,
        unknown => panic!("Money reward counts have no rarity level named: {unknown}"),
    }
}

/// `Generate` (`:49-77`) — the three rarities picked, containerised, and concatenated in
/// common/rare/superrare order.
///
/// The two passes are not interleaved: all three rarities are picked (`:63-67`) before any of them
/// is containerised (`:70-72`), and both spend draws off the one stream.
///
/// Not the module's entry point — [`crate::scav_case::generate_scav_case_rewards`] is, and it is
/// what installs the seed and catches the panics this path is allowed to raise.
///
/// # Errors
///
/// Quirk 8: a recipe id the table does not hold, where the C# `FirstOrDefault` answers null and
/// `:403` throws dereferencing `EndProducts`. Plus whatever [`pick_random_rewards`] and
/// [`randomise_container_item_rewards`] report.
pub(crate) fn generate(
    varying: &ScavCaseVarying,
    views: &ScavCaseViews,
    diagnostics: &mut DiagSink,
) -> Result<ScavCaseResponse, ScavCaseError> {
    let req = &ScavCaseRefs::new(varying, views);
    // `CacheDbItems()` (`:51`); see [`build_reward_pool`] for why it is not cached.
    let db_items_cache = build_reward_pool(req);

    // Get scavcase details from hideout/scavcase.json (`:54`)
    let scav_case_details = req
        .scav_recipes
        .iter()
        .find(|recipe| recipe.id == req.recipe_id)
        .ok_or_else(|| {
            ScavCaseError::new(format!(
                "No scav case recipe found with id: {}",
                req.recipe_id
            ))
        })?;
    let (common_counts, rare_counts, superrare_counts) =
        get_reward_counts_and_prices(scav_case_details, req.config);

    // Get items that fit the price criteria as set by the scavCase config (`:58-60`)
    let common_priced_items =
        get_filtered_items_by_price(&db_items_cache, &common_counts, req.static_prices);
    let rare_priced_items =
        get_filtered_items_by_price(&db_items_cache, &rare_counts, req.static_prices);
    let super_rare_priced_items =
        get_filtered_items_by_price(&db_items_cache, &superrare_counts, req.static_prices);

    // Get randomly picked items from each item collection, the count range of which is defined in
    // hideout/scavcase.json (`:63-67`)
    let randomly_picked_common_rewards = pick_random_rewards(
        req,
        &common_priced_items,
        &common_counts,
        COMMON,
        diagnostics,
    )?;
    let randomly_picked_rare_rewards =
        pick_random_rewards(req, &rare_priced_items, &rare_counts, RARE, diagnostics)?;
    let randomly_picked_super_rare_rewards = pick_random_rewards(
        req,
        &super_rare_priced_items,
        &superrare_counts,
        SUPERRARE,
        diagnostics,
    )?;

    // Add randomised stack sizes to ammo and money rewards (`:70-72`)
    let mut result = randomise_container_item_rewards(
        req,
        &randomly_picked_common_rewards,
        COMMON,
        diagnostics,
    )?;
    result.extend(randomise_container_item_rewards(
        req,
        &randomly_picked_rare_rewards,
        RARE,
        diagnostics,
    )?);
    result.extend(randomise_container_item_rewards(
        req,
        &randomly_picked_super_rare_rewards,
        SUPERRARE,
        diagnostics,
    )?);

    Ok(ScavCaseResponse { result })
}

#[cfg(test)]
pub mod tests {
    use serde_json::{Value, json};

    use crate::diag::DiagSink;
    use crate::loot::item_helper::{self, AMMO, AMMO_BOX, ARMOR, MONEY, WEAPON};
    use crate::loot::models::{Item, ItemView, WARNING};
    use crate::loot::random_util::{TestSeedGuard, get_chance_100, get_int};
    use crate::scav_case::generator::{
        COMMON, DOLLARS, EUROS, GP, ROUBLES, RewardCountAndPriceDetails, SUPERRARE, ScavCaseRefs,
        build_ammo_pool, build_reward_pool, generate, get_filtered_items_by_price,
        get_reward_counts_and_prices, pick_random_rewards, randomise_container_item_rewards,
    };
    use crate::scav_case::models::{ScavCaseRewardsRequest, ScavCaseVarying};
    use crate::scav_case::{ScavCaseError, ScavCaseViews, resolve_scav_case_views};

    /// Splits a flat fixture into the envelope halves: the four view members into `viewsOverride`,
    /// everything else (config, sets, seed) into `varying` — so the tests keep mutating one flat
    /// object, as the loot family's `split_envelope` lets them.
    pub fn envelope(flat: Value) -> Value {
        let Value::Object(mut varying) = flat else {
            panic!("fixture envelope is not an object");
        };
        let mut views = serde_json::Map::new();
        for key in [
            "scavRecipes",
            "itemsView",
            "staticPrices",
            "defaultPresetsByTpl",
        ] {
            if let Some(value) = varying.remove(key) {
                views.insert(key.to_owned(), value);
            }
        }

        json!({ "epoch": 0, "viewsOverride": views, "varying": varying })
    }

    /// A parsed override fixture: the varying half plus its resolved views, held together so
    /// [`Self::refs`] can lend the generator both.
    struct Fixture {
        varying: ScavCaseVarying,
        views: ScavCaseViews,
    }

    impl Fixture {
        fn refs(&self) -> ScavCaseRefs<'_> {
            ScavCaseRefs::new(&self.varying, &self.views)
        }
    }

    fn fixture(flat: Value) -> Fixture {
        let request: ScavCaseRewardsRequest = serde_json::from_value(envelope(flat)).unwrap();
        let views = resolve_scav_case_views(request.epoch, request.views_override)
            .expect("an override request resolves without the store");

        Fixture {
            varying: request.varying,
            views,
        }
    }

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

    fn request() -> Fixture {
        fixture(request_json())
    }

    fn tpls(pool: &[(&str, &ItemView)]) -> Vec<String> {
        pool.iter().map(|(tpl, _)| (*tpl).to_owned()).collect()
    }

    #[test]
    fn reward_pool_keeps_survivors_in_items_view_order() {
        assert_eq!(
            tpls(&build_reward_pool(&request().refs())),
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
        let pool = tpls(&build_reward_pool(&req.refs()));

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
            tpls(&build_ammo_pool(&request().refs())),
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
        let pool = tpls(&build_ammo_pool(&req.refs()));

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
        let req = req.refs();

        assert!(req.items_view[AMMO_NULL_STACK_TPL].stack_max_size.is_none());
        assert!(tpls(&build_ammo_pool(&req)).contains(&AMMO_NULL_STACK_TPL.to_owned()));
    }

    /// Quirk 9: the ammo filter is not the reward filter plus a baseclass test — it checks neither
    /// `QuestItem` (`:103`) nor `RewardItemParentBlacklist` (`:130`), so both templates the reward
    /// pool drops for those reasons stay in the ammo pool.
    #[test]
    fn ammo_pool_skips_the_quest_item_and_parent_blacklist_checks() {
        let req = request();
        let req = req.refs();
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
        let req = fixture(json);
        let req = req.refs();

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

    fn pick_request(money_chance: i32, ammo_chance: i32) -> Fixture {
        fixture(pick_request_json(money_chance, ammo_chance))
    }

    /// [`pick_request`] with both caps lifted. `allowMultipleAmmoRewardsPerRarity` ships **true**
    /// (`SPT_Data/configs/scavcase.json:111`), so this is the production path for ammo.
    fn pick_request_allowing_multiple(money_chance: i32, ammo_chance: i32) -> Fixture {
        let mut json = pick_request_json(money_chance, ammo_chance);
        json["config"]["allowMultipleMoneyRewardsPerRarity"] = json!(true);
        json["config"]["allowMultipleAmmoRewardsPerRarity"] = json!(true);

        fixture(json)
    }

    /// The common-rarity pool the C# hands `PickRandomRewards`: the reward cache, price-filtered.
    fn common_pool<'a>(req: &ScavCaseRefs<'a>) -> Vec<(&'a str, &'a ItemView)> {
        let (common, _, _) = get_reward_counts_and_prices(&req.scav_recipes[0], req.config);

        get_filtered_items_by_price(&build_reward_pool(req), &common, req.static_prices)
    }

    /// A fixed reward count, over a pool the test hands in ready-filtered — `GetInt(n, n)` returns
    /// without drawing, so the stream starts on the first chance roll.
    fn rewards(count: f64) -> RewardCountAndPriceDetails {
        RewardCountAndPriceDetails {
            min_count: count,
            max_count: count,
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
        let req = req.refs();
        let (common, rare, superrare) =
            get_reward_counts_and_prices(&req.scav_recipes[0], req.config);

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
        let req = req.refs();
        let pool = build_reward_pool(&req);

        let inclusive =
            get_filtered_items_by_price(&pool, &price_band(100.0, 1000.0), req.static_prices);
        let exclusive =
            get_filtered_items_by_price(&pool, &price_band(101.0, 999.0), req.static_prices);

        // Insertion order, not sorted order — the pool is filtered out of `itemsView`.
        assert_eq!(tpls(&inclusive), vec![PICK_D, PICK_B, PICK_A, PICK_C]);
        assert_eq!(tpls(&exclusive), vec![PICK_B, PICK_C]);
    }

    /// A template with no `staticPrices` entry is not skipped: `GetStaticPriceForItem` answers 0 for
    /// a handbook miss (`HandbookHelper.cs:106-125`), so it passes a floor of 0.
    #[test]
    fn filtered_items_by_price_treats_a_missing_price_as_zero() {
        let req = pick_request(0, 0);
        let req = req.refs();
        let free = get_filtered_items_by_price(
            &build_reward_pool(&req),
            &price_band(0.0, 0.0),
            req.static_prices,
        );

        assert!(req.static_prices.get(ROUBLES).is_none());
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
        let req = req.refs();
        let pool = common_pool(&req);
        let (common, _, _) = get_reward_counts_and_prices(&req.scav_recipes[0], req.config);
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
        let req = req.refs();
        let pool = common_pool(&req);
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked =
            pick_random_rewards(&req, &pool, &rewards(1.0), "common", &mut diagnostics).unwrap();

        assert_eq!(tpls(&picked), vec![DOLLARS]);
    }

    /// The ammo branch price-filters the ammo cache against `ammoRewardValueRangeRub[rarity]`
    /// (`:288-297`) and draws an index out of what survives, in `itemsView` order.
    #[test]
    fn the_ammo_branch_draws_from_the_price_filtered_ammo_cache() {
        let req = pick_request(0, 100);
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked =
            pick_random_rewards(&req, &[], &rewards(1.0), "common", &mut diagnostics).unwrap();

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
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked = pick_random_rewards(&req, &[], &rewards(1.0), "rare", &mut diagnostics);

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

    /// `:231-234` — the ammo cap is only applied when `AllowMultipleAmmoRewardsPerRarity` is false,
    /// and the shipped config sets it **true** (`SPT_Data/configs/scavcase.json:111`), so two ammo
    /// rewards in one rarity is the production path. Setting the flag unconditionally would send the
    /// second iteration to the pool branch instead, over a differently sized pool.
    #[test]
    fn multiple_ammo_rewards_land_in_one_rarity_when_the_config_allows_them() {
        let req = pick_request_allowing_multiple(0, 100);
        let req = req.refs();
        let pool = common_pool(&req);
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked =
            pick_random_rewards(&req, &pool, &rewards(2.0), "common", &mut diagnostics).unwrap();

        assert!(req.config.allow_multiple_ammo_rewards_per_rarity);
        assert_eq!(tpls(&picked), vec![AMMO_CHEAP, AMMO_CHEAP]);
    }

    /// `:222-225`, the money twin of the test above: no cap, so every iteration takes the money
    /// branch and the pool is never reached.
    #[test]
    fn multiple_money_rewards_land_in_one_rarity_when_the_config_allows_them() {
        let req = pick_request_allowing_multiple(100, 0);
        let req = req.refs();
        let pool = common_pool(&req);
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let picked =
            pick_random_rewards(&req, &pool, &rewards(2.0), "common", &mut diagnostics).unwrap();

        assert!(req.config.allow_multiple_money_rewards_per_rarity);
        assert_eq!(tpls(&picked), vec![DOLLARS, EUROS]);
    }

    /// `:238` — `GetArrayValue` over the reward pool, which is a `List`: an empty one throws
    /// `InvalidOperationException` before any draw.
    #[test]
    fn an_empty_reward_pool_fails_the_pick() {
        let req = pick_request(0, 0);
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);

        assert!(pick_random_rewards(&req, &[], &rewards(1.0), "common", &mut diagnostics).is_err());
    }

    // ---- Containerisation (`:318-368`), stack sizes (`:431-501`) and `Generate` (`:49-77`) ----

    /// A node between [`AMMO`] and [`GRANDCHILD_AMMO_TPL`] — quirk 4's shape: an item the ancestor
    /// walk at `:359` admits and the direct-parent test at `:433` does not.
    const AMMO_SUB_NODE: &str = "aa0000000000000000000000";
    const BOX_TPL: &str = "b00000000000000000000000";
    const CARTRIDGE_TPL: &str = "c00000000000000000000000";
    const AMMO_STACKING_TPL: &str = "a10000000000000000000000";
    const AMMO_NO_STACK_MAX_TPL: &str = "a20000000000000000000000";
    const GRANDCHILD_AMMO_TPL: &str = "a30000000000000000000000";
    const WEAPON_TPL: &str = "d00000000000000000000000";
    const WEAPON_NO_PRESET_TPL: &str = "d10000000000000000000000";
    const MOD_TPL: &str = "d20000000000000000000000";
    const ARMOR_TPL: &str = "e00000000000000000000000";
    const SOFT_INSERT_TPL: &str = "e10000000000000000000000";
    const PLAIN_TPL: &str = "f00000000000000000000000";

    /// The ids the fixture's presets ship with; `ReplaceIDs` + `RemapRootItemId` (`:354-356`) must
    /// leave none of them in the reward.
    const PRESET_WEAPON_ROOT: &str = "aaaa000000000000000000w1";
    const PRESET_WEAPON_MOD: &str = "aaaa000000000000000000w2";
    const PRESET_ARMOR_ROOT: &str = "aaaa000000000000000000a1";
    const PRESET_ARMOR_INSERT: &str = "aaaa000000000000000000a2";

    const RECIPE_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";

    /// One template per `RandomiseContainerItemRewards` branch, plus the two the stack draw keys
    /// off. Money counts are fixed per currency *and* per rarity (rub 1000/1001/1002, usd
    /// 2000/…, eur 3000/…, gp 4000/…) so a stack size names the map it came out of; `GetInt(n, n)`
    /// returns without drawing, keeping the stream free of them.
    ///
    /// Prices sort the templates into three disjoint rarity bands: common 0-100 takes
    /// `AMMO_STACKING_TPL`, `BOX_TPL`, `PLAIN_TPL` and `ROUBLES`; rare 1000-5000 takes the two
    /// weapons; superrare 20000-50000 takes the armor. Everything else is priced at 500, which no
    /// band covers.
    pub fn container_request_json() -> Value {
        json!({
            "recipeId": RECIPE_ID,
            "scavRecipes": [{"id": RECIPE_ID, "endProducts": {
                "common": {"min": 4, "max": 4}, "rare": {"min": 2, "max": 2},
                "superrare": {"min": 1, "max": 1}}}],
            "config": {
                "rewardItemValueRangeRub": {
                    "common": {"min": 0.0, "max": 100.0},
                    "rare": {"min": 1000.0, "max": 5000.0},
                    "superrare": {"min": 20000.0, "max": 50000.0}},
                "moneyRewards": {"moneyRewardChancePercent": 0,
                    "rubCount": {"common": {"min": 1000, "max": 1000},
                        "rare": {"min": 1001, "max": 1001},
                        "superrare": {"min": 1002, "max": 1002}},
                    "usdCount": {"common": {"min": 2000, "max": 2000},
                        "rare": {"min": 2001, "max": 2001},
                        "superrare": {"min": 2002, "max": 2002}},
                    "eurCount": {"common": {"min": 3000, "max": 3000},
                        "rare": {"min": 3001, "max": 3001},
                        "superrare": {"min": 3002, "max": 3002}},
                    "gpCount": {"common": {"min": 4000, "max": 4000},
                        "rare": {"min": 4001, "max": 4001},
                        "superrare": {"min": 4002, "max": 4002}}},
                "ammoRewards": {"ammoRewardChancePercent": 0,
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
                AMMO_BOX: {"parent": ITEM_NODE, "type": "Node"},
                WEAPON: {"parent": ITEM_NODE, "type": "Node"},
                ARMOR: {"parent": ITEM_NODE, "type": "Node"},
                MISC_NODE: {"parent": ITEM_NODE, "type": "Node"},
                AMMO_SUB_NODE: {"parent": AMMO, "type": "Node"},
                AMMO_STACKING_TPL: {"parent": AMMO, "type": "Item", "name": "patron_545",
                    "stackMaxSize": 60},
                BOX_TPL: {"parent": AMMO_BOX, "type": "Item", "name": "ammo_box_545",
                    "stackSlotMaxCount": 60.0, "stackSlotFirstFilterFirst": CARTRIDGE_TPL},
                PLAIN_TPL: {"parent": MISC_NODE, "type": "Item", "name": "bandage"},
                ROUBLES: {"parent": MONEY, "type": "Item", "name": "roubles",
                    "stackMaxSize": 500000},
                WEAPON_TPL: {"parent": WEAPON, "type": "Item", "name": "weapon_ak"},
                WEAPON_NO_PRESET_TPL: {"parent": WEAPON, "type": "Item", "name": "weapon_mp5"},
                ARMOR_TPL: {"parent": ARMOR, "type": "Item", "name": "armor_6b13",
                    "slots": [{"name": "soft_armor_front"}]},
                CARTRIDGE_TPL: {"parent": AMMO, "type": "Item", "name": "patron_9x19",
                    "stackMaxSize": 30},
                AMMO_NO_STACK_MAX_TPL: {"parent": AMMO, "type": "Item", "name": "patron_no_stack"},
                GRANDCHILD_AMMO_TPL: {"parent": AMMO_SUB_NODE, "type": "Item",
                    "name": "patron_grandchild", "stackMaxSize": 60},
                MOD_TPL: {"parent": MISC_NODE, "type": "Item", "name": "mod_stock"},
                SOFT_INSERT_TPL: {"parent": MISC_NODE, "type": "Item", "name": "soft_insert"},
                EUROS: {"parent": MONEY, "type": "Item", "name": "euros", "stackMaxSize": 500000},
                DOLLARS: {"parent": MONEY, "type": "Item", "name": "dollars",
                    "stackMaxSize": 500000},
                GP: {"parent": MONEY, "type": "Item", "name": "gp", "stackMaxSize": 500000}
            },
            "staticPrices": {
                AMMO_STACKING_TPL: 50.0, BOX_TPL: 90.0, PLAIN_TPL: 60.0, ROUBLES: 10.0,
                WEAPON_TPL: 2000.0, WEAPON_NO_PRESET_TPL: 3000.0, ARMOR_TPL: 30000.0,
                CARTRIDGE_TPL: 500.0, AMMO_NO_STACK_MAX_TPL: 500.0, GRANDCHILD_AMMO_TPL: 500.0,
                MOD_TPL: 500.0, SOFT_INSERT_TPL: 500.0,
                EUROS: 500.0, DOLLARS: 500.0, GP: 500.0
            },
            "defaultPresetsByTpl": {
                WEAPON_TPL: {"items": [
                    {"_id": PRESET_WEAPON_ROOT, "_tpl": WEAPON_TPL},
                    {"_id": PRESET_WEAPON_MOD, "_tpl": MOD_TPL, "parentId": PRESET_WEAPON_ROOT,
                        "slotId": "mod_stock"}]},
                ARMOR_TPL: {"items": [
                    {"_id": PRESET_ARMOR_ROOT, "_tpl": ARMOR_TPL},
                    {"_id": PRESET_ARMOR_INSERT, "_tpl": SOFT_INSERT_TPL,
                        "parentId": PRESET_ARMOR_ROOT, "slotId": "soft_armor_front"}]}
            },
            "inactiveSeasonalItems": [],
            "globalBlacklist": [],
            "rewardItemBlacklist": [],
            "bossItems": []
        })
    }

    fn container_request() -> Fixture {
        fixture(container_request_json())
    }

    /// The picks `PickRandomRewards` would have handed on, spelled as tpls.
    fn picks<'a>(req: &ScavCaseRefs<'a>, tpls: &[&str]) -> Vec<(&'a str, &'a ItemView)> {
        tpls.iter()
            .map(|tpl| {
                let (tpl, item) = req.items_view.get_key_value(*tpl).unwrap();

                (tpl.as_str(), item)
            })
            .collect()
    }

    /// What a reward group is, ignoring the minted ids: root tpl, item count, root stack size.
    fn shapes(rewards: &[Vec<Item>]) -> Vec<(String, usize, Option<f64>)> {
        rewards
            .iter()
            .map(|group| {
                (
                    group[0].template.clone(),
                    group.len(),
                    group[0]
                        .upd
                        .as_ref()
                        .and_then(|upd| upd.stack_objects_count),
                )
            })
            .collect()
    }

    /// `:335-337` — an ammo box reward is hydrated with cartridge children, and nothing sets a stack
    /// size on its root.
    #[test]
    fn an_ammo_box_reward_is_filled_with_cartridges() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let rewards = randomise_container_item_rewards(
            &req,
            &picks(&req, &[BOX_TPL]),
            COMMON,
            &mut diagnostics,
        )
        .unwrap();

        // 60 cartridges of a 30-max template: two stacks, the second at location 0 (`:1266-1275`).
        assert_eq!(shapes(&rewards), vec![(BOX_TPL.to_owned(), 3, None)]);
        assert_eq!(rewards[0][1].template, CARTRIDGE_TPL);
        assert_eq!(
            rewards[0][1].parent_id.as_deref(),
            Some(rewards[0][0].id.as_str())
        );
        assert_eq!(rewards[0][2].template, CARTRIDGE_TPL);
        assert!(diagnostics.captured().is_empty());
    }

    /// `:340-357` — an armor with soft-insert slots is replaced *whole* by a clone of its default
    /// preset, re-idded root and all, with the parent/child hierarchy intact.
    #[test]
    fn an_armor_reward_becomes_a_freshly_idded_clone_of_its_preset() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let rewards = randomise_container_item_rewards(
            &req,
            &picks(&req, &[ARMOR_TPL]),
            COMMON,
            &mut diagnostics,
        )
        .unwrap();

        let group = &rewards[0];
        assert_eq!(shapes(&rewards), vec![(ARMOR_TPL.to_owned(), 2, None)]);
        assert_eq!(group[1].template, SOFT_INSERT_TPL);
        assert_eq!(group[1].slot_id.as_deref(), Some("soft_armor_front"));
        // Re-idded, and the child still points at the new root.
        assert!(![PRESET_ARMOR_ROOT, PRESET_ARMOR_INSERT].contains(&group[0].id.as_str()));
        assert!(![PRESET_ARMOR_ROOT, PRESET_ARMOR_INSERT].contains(&group[1].id.as_str()));
        assert_eq!(group[1].parent_id.as_deref(), Some(group[0].id.as_str()));
        assert!(diagnostics.captured().is_empty());
    }

    /// Quirk 5 (`:345-351`): a weapon with no default preset warns and `continue`s, so the reward
    /// vanishes from the result rather than arriving bare.
    #[test]
    fn a_weapon_without_a_preset_is_dropped_with_a_warning() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let rewards = randomise_container_item_rewards(
            &req,
            &picks(&req, &[WEAPON_NO_PRESET_TPL, PLAIN_TPL]),
            COMMON,
            &mut diagnostics,
        )
        .unwrap();

        // Two picks in, one reward out — and the survivor is the one that was not dropped.
        assert_eq!(shapes(&rewards), vec![(PLAIN_TPL.to_owned(), 1, None)]);
        assert_eq!(diagnostics.captured().len(), 1);
        assert_eq!(diagnostics.captured()[0].level, WARNING);
        // Interpolated text, not a locale key (`:348`).
        assert_eq!(diagnostics.captured()[0].locale_key, None);
        assert_eq!(
            diagnostics.captured()[0].message.as_deref(),
            Some(
                format!("No preset for item: {WEAPON_NO_PRESET_TPL} weapon_mp5, skipping").as_str()
            )
        );
    }

    /// Quirk 4 (`:359` vs `:433-446`): the gate is an ancestor walk, the stack draw keys on the
    /// direct parent. An item whose *grandparent* is `AMMO` therefore passes the gate, misses both
    /// branches of `GetRandomAmountRewardForScavCase` — and still has its `Upd` set, to the else
    /// branch's 1.
    #[test]
    fn a_grandchild_of_ammo_passes_the_gate_and_stacks_to_one() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let rewards = randomise_container_item_rewards(
            &req,
            &picks(&req, &[GRANDCHILD_AMMO_TPL]),
            COMMON,
            &mut diagnostics,
        )
        .unwrap();

        assert!(item_helper::is_of_baseclasses(
            req.items_view,
            GRANDCHILD_AMMO_TPL,
            &[AMMO, MONEY]
        ));
        assert_eq!(
            req.items_view[GRANDCHILD_AMMO_TPL].parent.as_deref(),
            Some(AMMO_SUB_NODE)
        );
        assert_eq!(
            shapes(&rewards),
            vec![(GRANDCHILD_AMMO_TPL.to_owned(), 1, Some(1.0))]
        );
    }

    /// `:359-362` gates on `[AMMO, MONEY]`, so anything else keeps the `Upd` of `null` `:330` gave
    /// it.
    #[test]
    fn a_plain_reward_keeps_its_null_upd() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let rewards = randomise_container_item_rewards(
            &req,
            &picks(&req, &[PLAIN_TPL]),
            COMMON,
            &mut diagnostics,
        )
        .unwrap();

        assert_eq!(shapes(&rewards), vec![(PLAIN_TPL.to_owned(), 1, None)]);
    }

    /// `:456` — `GetInt(MinStackSize, StackMaxSize ?? 0)`, drawn off the same stream the picks use.
    #[test]
    fn an_ammo_reward_stacks_between_the_config_floor_and_the_template_ceiling() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let rewards = {
            let _guard = TestSeedGuard::install(SEED);
            randomise_container_item_rewards(
                &req,
                &picks(&req, &[AMMO_STACKING_TPL]),
                COMMON,
                &mut diagnostics,
            )
            .unwrap()
        };

        // The one draw this reward costs, off a stream of its own: swapping the bounds or widening
        // either end changes it.
        let expected = {
            let _guard = TestSeedGuard::install(SEED);
            f64::from(get_int(30, 60))
        };
        assert_eq!(
            shapes(&rewards),
            vec![(AMMO_STACKING_TPL.to_owned(), 1, Some(expected))]
        );
    }

    /// Quirk 6's other half: `StackMaxSize` is `int?`, `:456` coalesces it to 0, and `GetInt`'s
    /// `max > min` guard (`RandomUtil.cs:48`) then returns the floor **without drawing** — so an
    /// ammo template with no stack size at all costs the stream nothing.
    #[test]
    fn ammo_without_a_stack_max_size_takes_the_floor_without_drawing() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();

        let _guard = TestSeedGuard::install(SEED);
        let rewards = randomise_container_item_rewards(
            &req,
            &picks(&req, &[AMMO_NO_STACK_MAX_TPL]),
            COMMON,
            &mut diagnostics,
        )
        .unwrap();

        assert!(
            req.items_view[AMMO_NO_STACK_MAX_TPL]
                .stack_max_size
                .is_none()
        );
        assert_eq!(
            shapes(&rewards),
            vec![(AMMO_NO_STACK_MAX_TPL.to_owned(), 1, Some(30.0))]
        );
        // The stream is still at its first value, so nothing was drawn above.
        assert_eq!(get_int(0, 1_000_000), {
            let _guard = TestSeedGuard::install(SEED);
            get_int(0, 1_000_000)
        });
    }

    /// `:469-500` — each currency reads its own count map, and each map its own rarity level.
    /// `EUROS` reads `EurCount` while `DOLLARS` reads **UsdCount**, which the fixture's disjoint
    /// values pin.
    #[test]
    fn money_stacks_come_from_the_currencys_own_rarity_range() {
        let req = container_request();
        let req = req.refs();
        let mut diagnostics = DiagSink::capture();
        let money = picks(&req, &[ROUBLES, EUROS, DOLLARS, GP]);

        let common =
            randomise_container_item_rewards(&req, &money, COMMON, &mut diagnostics).unwrap();
        let superrare =
            randomise_container_item_rewards(&req, &money, SUPERRARE, &mut diagnostics).unwrap();

        assert_eq!(
            shapes(&common),
            vec![
                (ROUBLES.to_owned(), 1, Some(1000.0)),
                (EUROS.to_owned(), 1, Some(3000.0)),
                (DOLLARS.to_owned(), 1, Some(2000.0)),
                (GP.to_owned(), 1, Some(4000.0)),
            ]
        );
        assert_eq!(
            shapes(&superrare),
            vec![
                (ROUBLES.to_owned(), 1, Some(1002.0)),
                (EUROS.to_owned(), 1, Some(3002.0)),
                (DOLLARS.to_owned(), 1, Some(2002.0)),
                (GP.to_owned(), 1, Some(4002.0)),
            ]
        );
    }

    /// Quirk 8 (`:54-55`): `FirstOrDefault` answers null for a recipe id the table does not hold and
    /// C# then NREs dereferencing `EndProducts`. Reported as an error naming the id instead.
    #[test]
    fn an_unknown_recipe_id_fails_naming_the_recipe() {
        let mut json = container_request_json();
        json["recipeId"] = json!("ffffffffffffffffffffffff");
        let req = fixture(json);
        let mut diagnostics = DiagSink::capture();

        let error = generate(&req.varying, &req.views, &mut diagnostics).unwrap_err();

        let ScavCaseError::Failed(message) = error else {
            panic!("expected the throw's message, got {error:?}");
        };
        assert!(message.contains("ffffffffffffffffffffffff"), "{message}");
    }

    /// The end-to-end KAT: one seeded `Generate` (`:49-77`) over the synthetic table, pinned reward
    /// for reward. This is the anchor later parity work triages against — a drift in draw order,
    /// pool contents, branch order or stack sizes moves it.
    ///
    /// The stream: three draws per reward (money chance, ammo chance, pool index — both chances are
    /// 0%, so every reward comes from the price-filtered pool), all three rarities picked before any
    /// of them is containerised (`:63-72`), then the stack draws.
    #[test]
    fn generate_returns_the_seeded_reward_list() {
        let req = container_request();
        let mut diagnostics = DiagSink::capture();

        let response = {
            let _guard = TestSeedGuard::install(SEED);
            generate(&req.varying, &req.views, &mut diagnostics).unwrap()
        };

        assert_eq!(
            shapes(&response.result),
            vec![
                // Common: four draws over `[AMMO_STACKING, BOX, PLAIN, ROUBLES]`, landing on 1, 3,
                // 2 and 0. The ammo box brought two cartridge stacks; the roubles took the money
                // branch of the stack draw (`:439-441`) as an ordinary *pool* draw, the money
                // chance being 0 here; the ammo's 54 is the only draw containerisation itself
                // spends, which is what pins the pick-then-containerise order of `:63-72`.
                (BOX_TPL.to_owned(), 3, None),
                (ROUBLES.to_owned(), 1, Some(1000.0)),
                (PLAIN_TPL.to_owned(), 1, None),
                (AMMO_STACKING_TPL.to_owned(), 1, Some(54.0)),
                // Rare: two draws over `[WEAPON_TPL, WEAPON_NO_PRESET_TPL]`. One reward survives —
                // the other pick was the preset-less weapon quirk 5 drops.
                (WEAPON_TPL.to_owned(), 2, None),
                // Superrare: a one-item pool, containerised into its armor preset.
                (ARMOR_TPL.to_owned(), 2, None),
            ]
        );
        // Quirk 5's warning, and nothing else.
        assert_eq!(diagnostics.captured().len(), 1);
        assert_eq!(
            diagnostics.captured()[0].message.as_deref(),
            Some(
                format!("No preset for item: {WEAPON_NO_PRESET_TPL} weapon_mp5, skipping").as_str()
            )
        );
    }
}
