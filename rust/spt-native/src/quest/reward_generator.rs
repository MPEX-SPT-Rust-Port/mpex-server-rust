//! `Generators/RepeatableQuests/RepeatableQuestRewardGenerator.cs` — the reward chain every
//! repeatable quest type ends with: experience, money, GP coins, an optional weapon preset, item
//! rewards, trader standing and an optional skill point reward.
//!
//! Line references in this file are that generator unless another file is named.

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::bot::exhaustable_array::ExhaustableArray;
use crate::loot::item_helper::{self, AMMO, ARMORED_EQUIPMENT, DEFAULT_INVALID_BASE_TYPES, WEAPON};
use crate::loot::math_util::interp1;
use crate::loot::models::{DEBUG, Diagnostic, ERROR, Item, Upd, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{
    get_array_value, get_chance_100, get_double, rand_int, round_half_even,
};
use crate::quest::QuestContext;
use crate::quest::models::{RepeatableQuestConfig, Reward, RewardScaling};

/// `Models/Enums/Money.cs:7`.
const ROUBLES: &str = "5449016a4bdc2d6f028b456f";
/// `Models/Enums/Money.cs:8`.
const EUROS: &str = "569668774bdc2da2298b4568";
/// `Models/Enums/Money.cs:10`.
const GP: &str = "5d235b4d86f7742e017bc88a";
/// `Models/Enums/Traders.cs:9`.
const FENCE: &str = "579dc571d53a0658a154fbec";
/// `Models/Enums/Traders.cs:11`.
const PEACEKEEPER: &str = "5935c25fb3acc3127c3d8cd9";

/// `Models/Enums/RewardType.cs` through `JsonStringEnumConverter` — the four members this
/// generator writes.
const EXPERIENCE: &str = "Experience";
const SKILL: &str = "Skill";
const ITEM: &str = "Item";
const TRADER_STANDING: &str = "TraderStanding";

/// A plain interpolated log line the C# caller replays through its logger.
fn plain(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

/// A `ServerLocalisationService.GetText` line — the localised text stays C#-side, so only the key
/// and its arguments cross.
fn localised(level: &str, locale_key: &str, args: serde_json::Value) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args: Some(args),
        message: None,
    }
}

/// `PresetHelper.GetDefaultPresetOrItemPrice` (`PresetHelper.cs:272-282`), resolved per tpl by the
/// C# caller. It always returns a number, so a tpl the map does not know prices at 0.
fn get_default_preset_or_item_price(ctx: &QuestContext<'_>, tpl: &str) -> f64 {
    ctx.default_preset_or_item_prices
        .get(tpl)
        .copied()
        .unwrap_or(0.0)
}

/// `ItemHelper.GetItemAndChildrenPrice` (`ItemHelper.cs:419-422`) — note the `int` accumulator and
/// the `(int)` cast, which truncate every price before summing.
fn get_item_and_children_price(ctx: &QuestContext<'_>, tpls: &[&str]) -> f64 {
    tpls.iter()
        .map(|tpl| {
            item_helper::get_item_price(ctx.handbook_prices, ctx.flea_prices, tpl).unwrap_or(0.0)
                as i64
        })
        .sum::<i64>() as f64
}

/// `HandbookHelper.FromRoubles` (`HandbookHelper.cs:215-225`). `GetTemplatePrice` answers 0 for a
/// tpl the handbook does not carry (`HandbookHelper.cs:113-125`).
fn from_roubles(ctx: &QuestContext<'_>, rouble_currency_count: f64, currency_type_to: &str) -> f64 {
    if currency_type_to == ROUBLES {
        return rouble_currency_count;
    }

    // Get price of currency from handbook
    let price = ctx
        .handbook_prices
        .get(currency_type_to)
        .copied()
        .unwrap_or(0.0);

    if price > 0.0 {
        f64::max(1.0, round_half_even(rouble_currency_count / price))
    } else {
        0.0
    }
}

/// `Models/Spt/Repeatable/QuestRewardValues.cs` — the scaled reward figures one pass works from.
struct QuestRewardValues {
    skill_point_reward: f64,
    skill_reward_chance: f64,
    reward_reputation: f64,
    reward_num_items: i32,
    reward_roubles: f64,
    gp_coin_reward_count: f64,
    reward_xp: f64,
}

/// `GenerateReward` (`:60-220`) — the whole reward list for one quest, or `None` when the trader is
/// not in the config's whitelist (`:118-122`).
///
/// The C# takes the quest type's `BaseQuestConfig`; the only member it reads off it is
/// `PossibleSkillRewards` (`:199`), and the three Rust config records inline that member rather
/// than sharing a base type, so the list itself is the parameter here.
pub fn generate_reward<'a>(
    ctx: &mut QuestContext<'a>,
    pmc_level: i32,
    difficulty: f64,
    trader_id: &str,
    repeatable_config: &RepeatableQuestConfig,
    possible_skill_rewards: &[String],
    reward_tpl_blacklist: Option<&HashSet<String>>,
) -> Option<IndexMap<String, Vec<Reward>>> {
    // Get vars to configure rewards with
    let reward_params =
        get_quest_reward_values(&repeatable_config.reward_scaling, difficulty, pmc_level);

    // Get budget to spend on item rewards (copy of raw roubles given)
    let mut item_reward_budget = reward_params.reward_roubles;

    // `:76-81` keys the dictionary `Success`/`Started`/`Fail`; only `Success` is ever populated, so
    // it is built on its own and the three keys are assembled in that order on the way out.
    let mut success: Vec<Reward> = Vec::new();

    // Start reward index to keep track
    let mut reward_index = -1;

    // Add xp reward
    if reward_params.reward_xp > 0.0 {
        success.push(Reward {
            id: mongo_id::generate(),
            unknown: Some(false),
            game_mode: Some(Vec::new()),
            available_in_game_editions: Some(Vec::new()),
            index: Some(reward_index),
            value: Some(reward_params.reward_xp),
            reward_type: Some(EXPERIENCE.to_owned()),
            ..Default::default()
        });
        reward_index += 1;
    }

    // Add money reward
    success.push(get_money_reward(
        ctx,
        trader_id,
        reward_params.reward_roubles,
        reward_index,
    ));
    reward_index += 1;

    // Add GP coin reward
    success.push(generate_item_reward(
        GP,
        reward_params.gp_coin_reward_count,
        reward_index,
        true,
    ));
    reward_index += 1;

    // Add preset weapon to reward if checks pass
    let trader_whitelist_details = repeatable_config
        .trader_whitelist
        .iter()
        .find(|trader_whitelist| trader_whitelist.trader_id == trader_id);

    let Some(trader_whitelist_details) = trader_whitelist_details else {
        ctx.diagnostics.push(plain(
            ERROR,
            format!("Unable to find trader id: {trader_id} in whitelist"),
        ));

        return None;
    };

    // `&&` short-circuits in both languages, so a trader that cannot give weapons spends no draw
    if trader_whitelist_details.reward_can_be_weapon
        && get_chance_100(trader_whitelist_details.weapon_reward_chance_percent)
        && let Some((chosen_weapon, chosen_weapon_price)) =
            get_random_weapon_preset_within_budget(ctx, item_reward_budget, reward_index)
    {
        success.push(chosen_weapon);

        // Subtract price of preset from item budget so we don't give player too much stuff
        item_reward_budget -= chosen_weapon_price;
        reward_index += 1;
    }

    let mut in_budget_reward_item_pool =
        choose_reward_items_within_budget(ctx, repeatable_config, item_reward_budget, trader_id);

    if let Some(reward_tpl_blacklist) = reward_tpl_blacklist {
        // Filter reward pool of items from blacklist, only use if there's at least 1 item remaining
        let filtered_reward_item_pool: Vec<&str> = in_budget_reward_item_pool
            .iter()
            .copied()
            .filter(|tpl| !reward_tpl_blacklist.contains(*tpl))
            .collect();

        if !filtered_reward_item_pool.is_empty() {
            in_budget_reward_item_pool = filtered_reward_item_pool;
        }
    }

    let config_name = &repeatable_config.name;
    let num_items = reward_params.reward_num_items;
    ctx.diagnostics.push(plain(
        DEBUG,
        format!(
            "Generating: {config_name} quest for: {trader_id} with budget: {item_reward_budget} \
             totalling: {num_items} items"
        ),
    ));

    if !in_budget_reward_item_pool.is_empty() {
        let items_to_reward = get_rewardable_items_from_pool_within_budget(
            ctx,
            &in_budget_reward_item_pool,
            reward_params.reward_num_items,
            item_reward_budget,
            repeatable_config,
        );

        // Add item rewards
        for (tpl, stack_count) in items_to_reward {
            success.push(generate_item_reward(
                tpl,
                f64::from(stack_count),
                reward_index,
                true,
            ));
            reward_index += 1;
        }
    }

    // Add rep reward to rewards array
    if reward_params.reward_reputation > 0.0 {
        success.push(Reward {
            id: mongo_id::generate(),
            unknown: Some(false),
            game_mode: Some(Vec::new()),
            available_in_game_editions: Some(Vec::new()),
            target: Some(trader_id.to_owned()),
            value: Some(reward_params.reward_reputation),
            reward_type: Some(TRADER_STANDING.to_owned()),
            index: Some(reward_index),
            ..Default::default()
        });
        reward_index += 1;

        let reputation = reward_params.reward_reputation;
        ctx.diagnostics.push(plain(
            DEBUG,
            format!("Adding: {reputation} {trader_id} trader reputation reward"),
        ));
    }

    // Chance of adding skill reward
    if get_chance_100(reward_params.skill_reward_chance) {
        let target_skill = get_array_value(possible_skill_rewards);
        success.push(Reward {
            id: mongo_id::generate(),
            unknown: Some(false),
            game_mode: Some(Vec::new()),
            available_in_game_editions: Some(Vec::new()),
            target: Some(target_skill.clone()),
            value: Some(reward_params.skill_point_reward),
            reward_type: Some(SKILL.to_owned()),
            index: Some(reward_index),
            ..Default::default()
        });

        let skill_point_reward = reward_params.skill_point_reward;
        ctx.diagnostics.push(plain(
            DEBUG,
            format!("Adding {skill_point_reward} skill points to {target_skill}"),
        ));
    }

    let mut rewards = IndexMap::with_capacity(3);
    rewards.insert("Success".to_owned(), success);
    rewards.insert("Started".to_owned(), Vec::new());
    rewards.insert("Fail".to_owned(), Vec::new());

    Some(rewards)
}

/// `GetQuestRewardValues` (`:222-245`). The C# object initializer runs its assignments in the order
/// they are written (`:237-243`), so the draws come out reputation, item count, roubles, GP coins,
/// experience — **not** the source order of the methods themselves.
fn get_quest_reward_values(
    reward_scaling: &RewardScaling,
    effective_difficulty: f64,
    pmc_level: i32,
) -> QuestRewardValues {
    // difficulty could go from 0.2 ... -> for lowest difficulty receive 0.2*nominal reward
    let levels_config = &reward_scaling.levels;
    let roubles_config = &reward_scaling.roubles;
    let gp_coin_config = &reward_scaling.gp_coins;
    let xp_config = &reward_scaling.experience;
    let items_config = &reward_scaling.items;
    let reward_spread_config = reward_scaling.reward_spread;
    let skill_reward_chance_config = &reward_scaling.skill_reward_chance;
    let skill_point_reward_config = &reward_scaling.skill_point_reward;
    let reputation_config = &reward_scaling.reputation;

    let pmc_level = f64::from(pmc_level);

    QuestRewardValues {
        skill_point_reward: interp1(pmc_level, levels_config, skill_point_reward_config),
        skill_reward_chance: interp1(pmc_level, levels_config, skill_reward_chance_config),
        reward_reputation: get_reward_rep(
            effective_difficulty,
            pmc_level,
            levels_config,
            reputation_config,
            reward_spread_config,
        ),
        reward_num_items: get_reward_num_items(pmc_level, levels_config, items_config),
        reward_roubles: get_reward_roubles(
            effective_difficulty,
            pmc_level,
            levels_config,
            roubles_config,
            reward_spread_config,
        ),
        gp_coin_reward_count: get_gp_coin_reward_count(
            effective_difficulty,
            pmc_level,
            levels_config,
            gp_coin_config,
            reward_spread_config,
        ),
        reward_xp: get_reward_xp(
            effective_difficulty,
            pmc_level,
            levels_config,
            xp_config,
            reward_spread_config,
        ),
    }
}

/// `GetRewardXp` (`:247-259`).
fn get_reward_xp(
    effective_difficulty: f64,
    pmc_level: f64,
    levels_config: &[f64],
    xp_config: &[f64],
    reward_spread_config: f64,
) -> f64 {
    let interpolated_xp = interp1(pmc_level, levels_config, xp_config);
    let random_spread = get_double(1.0 - reward_spread_config, 1.0 + reward_spread_config);

    (effective_difficulty * interpolated_xp * random_spread).floor()
}

/// `GetGpCoinRewardCount` (`:261-273`).
fn get_gp_coin_reward_count(
    effective_difficulty: f64,
    pmc_level: f64,
    levels_config: &[f64],
    gp_coin_config: &[f64],
    reward_spread_config: f64,
) -> f64 {
    let interpolated_gp_coins = interp1(pmc_level, levels_config, gp_coin_config);
    let random_spread = get_double(1.0 - reward_spread_config, 1.0 + reward_spread_config);

    (effective_difficulty * interpolated_gp_coins * random_spread).ceil()
}

/// `GetRewardRep` (`:275-289`).
fn get_reward_rep(
    effective_difficulty: f64,
    pmc_level: f64,
    levels_config: &[f64],
    reputation_config: &[f64],
    reward_spread_config: f64,
) -> f64 {
    let difficulty_mod = 100.0 * effective_difficulty;
    let interpolated_rep = interp1(pmc_level, levels_config, reputation_config);
    let random_spread = get_double(1.0 - reward_spread_config, 1.0 + reward_spread_config);
    let multiplier = difficulty_mod * interpolated_rep * random_spread;

    round_half_even(multiplier) / 100.0
}

/// `GetRewardNumItems` (`:291-296`). `RandInt`'s upper bound is exclusive, so the `+ 1` is what
/// makes the interpolated count reachable.
fn get_reward_num_items(pmc_level: f64, levels_config: &[f64], items_config: &[f64]) -> i32 {
    let interpolated_num_items = interp1(pmc_level, levels_config, items_config);

    rand_int(1, Some(round_half_even(interpolated_num_items) as i64 + 1)) as i32
}

/// `GetRewardRoubles` (`:298-310`).
fn get_reward_roubles(
    effective_difficulty: f64,
    pmc_level: f64,
    levels_config: &[f64],
    roubles_config: &[f64],
    reward_spread_config: f64,
) -> f64 {
    let interpolated_roubles = interp1(pmc_level, levels_config, roubles_config);
    let random_spread = get_double(1.0 - reward_spread_config, 1.0 + reward_spread_config);

    (effective_difficulty * interpolated_roubles * random_spread).floor()
}

/// `GetRewardableItemsFromPoolWithinBudget` (`:320-396`) — the items, with their stack sizes, that
/// fit inside `item_reward_budget`.
///
/// **Quirk 4, ported verbatim:** `:392` breaks unconditionally at the end of the first full pass, so
/// this returns at most one item however large `max_item_count` is. The only path that re-iterates
/// is the ammo reject at `:348`, whose `i--` the `for`'s own `i++` immediately undoes — meaning `i`
/// never leaves 0 and the loop condition is just `maxItemCount > 0`, which is what the guard and
/// `loop` below spell.
fn get_rewardable_items_from_pool_within_budget<'a>(
    ctx: &mut QuestContext<'a>,
    item_pool: &[&'a str],
    max_item_count: i32,
    item_reward_budget: f64,
    repeatable_config: &RepeatableQuestConfig,
) -> Vec<(&'a str, i32)> {
    let items = ctx.items;
    let mut items_to_return: Vec<(&'a str, i32)> = Vec::new();
    let mut exhaustible_item_pool = ExhaustableArray::new(item_pool.to_vec());

    if max_item_count <= 0 {
        return items_to_return;
    }

    loop {
        // Default stack size to 1
        let mut reward_item_stack_count = 1;

        // Get a random item
        let chosen_item_from_pool = exhaustible_item_pool.get_random_value();
        let (Some(chosen_item_from_pool), true) =
            (chosen_item_from_pool, exhaustible_item_pool.has_values())
        else {
            break;
        };

        // Handle edge case - ammo
        if item_helper::is_of_baseclass(items, chosen_item_from_pool, AMMO) {
            // Don't reward ammo that stacks to less than what's allowed in config. A template
            // without a `StackMaxSize` takes the lifted-comparison-against-null arm of `:346`,
            // which is false.
            let stack_max_size = items
                .get(chosen_item_from_pool)
                .and_then(|template| template.stack_max_size);

            if stack_max_size.is_some_and(|stack_max_size| {
                stack_max_size < repeatable_config.reward_ammo_stack_min_size
            }) {
                continue;
            }

            // Choose the smallest value between budget, fitting size and stack max
            reward_item_stack_count = calculate_ammo_stack_size_that_fits_budget(
                ctx,
                chosen_item_from_pool,
                item_reward_budget,
                max_item_count,
            );
        }

        // 25% chance to double, triple or quadruple reward stack
        // (Only occurs when item is stackable and not weapon, armor or ammo)
        if can_increase_reward_item_stack_size(ctx, chosen_item_from_pool, 70000, 25) {
            reward_item_stack_count =
                get_randomised_reward_item_stack_size_by_price(ctx, chosen_item_from_pool);
        }

        items_to_return.push((chosen_item_from_pool, reward_item_stack_count));

        let item_cost = get_default_preset_or_item_price(ctx, chosen_item_from_pool);
        let calculated_item_reward_budget =
            item_reward_budget - f64::from(reward_item_stack_count) * item_cost;
        let paid = f64::from(reward_item_stack_count) * item_cost;
        ctx.diagnostics.push(plain(
            DEBUG,
            format!("Added item: {chosen_item_from_pool} with price: {paid}"),
        ));

        // If we still have budget narrow down possible items
        if calculated_item_reward_budget > 0.0 {
            // Filter possible reward items to only items with a price below the remaining budget.
            // The rebuilt pool is dead — `:392` breaks before it can be drawn from — but it is what
            // `:382` reports on, so it is built here too.
            exhaustible_item_pool = ExhaustableArray::new(filter_reward_pool_within_budget(
                ctx,
                item_pool,
                calculated_item_reward_budget,
                0.0,
            ));

            if !exhaustible_item_pool.has_values() {
                ctx.diagnostics.push(plain(
                    DEBUG,
                    format!(
                        "Reward pool empty with: {calculated_item_reward_budget} roubles of budget remaining"
                    ),
                ));
            }
        }

        // No budget for more items, end loop
        break;
    }

    items_to_return
}

/// `CalculateAmmoStackSizeThatFitsBudget` (`:406-421`) — how many cartridges the budget buys, at
/// least 1 and never above the template's stack max or 100.
fn calculate_ammo_stack_size_that_fits_budget(
    ctx: &QuestContext<'_>,
    item_selected: &str,
    roubles_budget: f64,
    reward_num_items: i32,
) -> i32 {
    // Calculate budget per reward item
    let stack_rouble_budget = roubles_budget / f64::from(reward_num_items);

    let single_cartridge_price = ctx
        .handbook_prices
        .get(item_selected)
        .copied()
        .unwrap_or(0.0);

    // Get a stack size of ammo that fits rouble budget
    let stack_size_that_fits_budget = round_half_even(stack_rouble_budget / single_cartridge_price);

    // Get itemDbs max stack size for ammo - don't go above 100 (some mods mess around with stack
    // sizes). `:417` reads `StackMaxSize.Value`, which throws on a template that has none — as
    // would the `Math.Clamp(x, 1, 0)` below it — so an ammo template without one is a broken
    // database in both languages.
    let stack_max_count = ctx
        .items
        .get(item_selected)
        .and_then(|template| template.stack_max_size)
        .expect("ammo template carries a stackMaxSize")
        .min(100);

    // Ensure stack size is at least 1 + is no larger than the max possible stack size
    stack_size_that_fits_budget.clamp(1.0, f64::from(stack_max_count)) as i32
}

/// `CanIncreaseRewardItemStackSize` (`:423-431`). The three eligibility tests short-circuit the
/// chance roll, so an ineligible item spends no draw.
fn can_increase_reward_item_stack_size(
    ctx: &QuestContext<'_>,
    tpl: &str,
    max_rouble_price_to_stack: i32,
    random_chance_to_pass: i32,
) -> bool {
    let is_eligible_for_stack_size_increase = get_default_preset_or_item_price(ctx, tpl)
        < f64::from(max_rouble_price_to_stack)
        && !item_helper::is_of_baseclasses(ctx.items, tpl, &[WEAPON, ARMORED_EQUIPMENT, AMMO])
        && !item_helper::item_requires_soft_inserts(ctx.items, tpl);

    is_eligible_for_stack_size_increase && get_chance_100(f64::from(random_chance_to_pass))
}

/// `GetRandomisedRewardItemStackSizeByPrice` (`:438-458`).
///
/// **Quirk 6, ported verbatim:** the 3000-10000 band offers `[2, 3]` where the bands either side of
/// it offer `[2, 3, 4]` (`:443-448`).
fn get_randomised_reward_item_stack_size_by_price(ctx: &QuestContext<'_>, tpl: &str) -> i32 {
    let reward_item_price = get_default_preset_or_item_price(ctx, tpl);

    // Define price tiers and corresponding stack size options
    const PRICE_TIERS: [(f64, &[i32]); 3] = [
        (3000.0, &[2, 3, 4]),
        (10000.0, &[2, 3]),
        (i32::MAX as f64, &[2, 3, 4]), // Default for prices 10001+ RUB
    ];

    // Find the appropriate price tier and return a random stack size from its options
    let tier = PRICE_TIERS
        .iter()
        .find(|(tier_price, _)| reward_item_price < *tier_price);

    let Some((_, stack_sizes)) = tier else {
        return 4; // Default to 4 if no tier matches
    };

    *get_array_value(stack_sizes)
}

/// `ChooseRewardItemsWithinBudget` (`:467-488`) — every rewardable tpl priced inside the budget,
/// falling back to the whole affordable pool when the price window comes back empty.
fn choose_reward_items_within_budget<'a>(
    ctx: &mut QuestContext<'a>,
    repeatable_config: &RepeatableQuestConfig,
    roubles_budget: f64,
    trader_id: &str,
) -> Vec<&'a str> {
    // First filter for type and baseclass to avoid lookup in handbook for non-available items
    let rewardable_item_pool = get_rewardable_items(ctx, repeatable_config, trader_id);
    let min_price = f64::min(25000.0, 0.5 * roubles_budget);

    let mut rewardable_item_pool_within_budget =
        filter_reward_pool_within_budget(ctx, &rewardable_item_pool, roubles_budget, min_price);

    if rewardable_item_pool_within_budget.is_empty() {
        ctx.diagnostics.push(localised(
            WARNING,
            "repeatable-no_reward_item_found_in_price_range",
            serde_json::json!({ "minPrice": min_price, "roublesBudget": roubles_budget }),
        ));

        // In case we don't find any items in the price range. `GetItemPrice` returns `double?` and
        // `:484` compares it with `<`, so a tpl neither table prices fails the filter.
        rewardable_item_pool_within_budget = rewardable_item_pool
            .into_iter()
            .filter(|tpl| {
                item_helper::get_item_price(ctx.handbook_prices, ctx.flea_prices, tpl)
                    .is_some_and(|price| price < roubles_budget)
            })
            .collect();
    }

    rewardable_item_pool_within_budget
}

/// `FilterRewardPoolWithinBudget` (`:497-506`) — strictly inside both bounds.
fn filter_reward_pool_within_budget<'a>(
    ctx: &QuestContext<'_>,
    reward_items: &[&'a str],
    roubles_budget: f64,
    min_price: f64,
) -> Vec<&'a str> {
    reward_items
        .iter()
        .copied()
        .filter(|tpl| {
            let item_price = get_default_preset_or_item_price(ctx, tpl);

            item_price < roubles_budget && item_price > min_price
        })
        .collect()
}

/// `GetRandomWeaponPresetWithinBudget` (`:514-545`) — the reward and the preset's price, or `None`
/// once every default weapon preset has been drawn and priced out of budget.
///
/// The `while HasValues()` spin (`:519-542`) terminates because each draw removes.
fn get_random_weapon_preset_within_budget(
    ctx: &mut QuestContext<'_>,
    roubles_budget: f64,
    reward_index: i32,
) -> Option<(Reward, f64)> {
    // Add a random default preset weapon as reward
    let default_weapon_presets = ctx.default_weapon_presets;
    let mut default_preset_pool = ExhaustableArray::new(default_weapon_presets.iter().collect());

    while default_preset_pool.has_values() {
        let Some(random_preset) = default_preset_pool.get_random_value() else {
            continue;
        };

        // Gather all tpls so we can get prices of them
        let tpls: Vec<&str> = random_preset
            .items
            .iter()
            .map(|item| item.template.as_str())
            .collect();

        // Does preset items fit our budget
        let preset_price = get_item_and_children_price(ctx, &tpls);
        if preset_price <= roubles_budget {
            let first_tpl = tpls.first().copied().unwrap_or_default();
            ctx.diagnostics.push(plain(
                DEBUG,
                format!("Added weapon: {first_tpl}with price: {preset_price}"),
            ));

            // `:535` clones the preset before mutating its root's `Upd`
            let mut chosen_preset_items = random_preset.items.clone();
            // `Encyclopedia` is never null here — `GetDefaultWeaponPresets` filters on it
            // (`PresetHelper.cs:100`) — so `:538`'s `.Value` cannot throw
            let encyclopedia = random_preset.encyclopedia.as_deref().unwrap_or_default();

            return generate_preset_reward(
                ctx,
                encyclopedia,
                1.0,
                reward_index,
                &mut chosen_preset_items,
                true,
            )
            .map(|reward| (reward, preset_price));
        }
    }

    None
}

/// `GeneratePresetReward` (`:556-590`) — a preset reward, structured as the client wants it.
///
/// **Quirk 5, ported verbatim:** `:558` mints an id that `:587` overwrites with the root item's own
/// id, making it dead. It is minted anyway (outside the RNG stream, so nothing downstream shifts).
///
/// Deviation: a preset with no root item is the C# `NullReferenceException` at `:587`, reached one
/// line after the warning at `:578`. Unwinding is undefined behaviour behind the FFI, so this
/// reports the same warning and gives up on the weapon reward instead.
fn generate_preset_reward(
    ctx: &mut QuestContext<'_>,
    tpl: &str,
    count: f64,
    index: i32,
    preset: &mut [Item],
    found_in_raid: bool,
) -> Option<Reward> {
    let id = mongo_id::generate();
    let mut quest_reward_item = Reward {
        id: mongo_id::generate(),
        unknown: Some(false),
        game_mode: Some(Vec::new()),
        available_in_game_editions: Some(Vec::new()),
        index: Some(index),
        target: Some(id),
        value: Some(count),
        is_encoded: Some(false),
        find_in_raid: Some(found_in_raid),
        reward_type: Some(ITEM.to_owned()),
        items: Some(Vec::new()),
        ..Default::default()
    };

    // Get presets root item
    let root_item = preset.iter().position(|item| item.template == tpl);
    let Some(root_item) = root_item else {
        ctx.diagnostics.push(plain(
            WARNING,
            format!("Root item of preset: {tpl} not found"),
        ));

        return None;
    };

    if let Some(upd) = preset[root_item].upd.as_mut() {
        // `SpawnedInSession` is untyped on `Upd`, so it rides in the passthrough map the way
        // `item_helper::set_found_in_raid` writes it
        upd.extra.insert(
            "SpawnedInSession".to_owned(),
            serde_json::Value::Bool(found_in_raid),
        );
    }

    // C# hands `ReparentItemAndChildren` the root *inside* `preset`, whose id the remap leaves
    // alone (`ItemHelper.cs:1695`); the clone taken here is that same untouched root.
    let root_item = preset[root_item].clone();

    quest_reward_item.items = Some(item_helper::reparent_item_and_children(&root_item, preset));
    quest_reward_item.target = Some(root_item.id); // Target property and root items id must match

    Some(quest_reward_item)
}

/// `GenerateItemReward` (`:600-627`) — a plain item reward, structured as the client wants it.
fn generate_item_reward(tpl: &str, count: f64, index: i32, found_in_raid: bool) -> Reward {
    let id = mongo_id::generate();
    let root_item = Item {
        id: id.clone(),
        template: tpl.to_owned(),
        upd: Some(Upd {
            stack_objects_count: Some(count),
            // `SpawnedInSession` is untyped on `Upd`, so it rides in the passthrough map
            extra: [(
                "SpawnedInSession".to_owned(),
                serde_json::Value::Bool(found_in_raid),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }),
        ..Default::default()
    };

    Reward {
        id: mongo_id::generate(),
        unknown: Some(false),
        game_mode: Some(Vec::new()),
        available_in_game_editions: Some(Vec::new()),
        index: Some(index),
        target: Some(id),
        value: Some(count),
        is_encoded: Some(false),
        find_in_raid: Some(found_in_raid),
        reward_type: Some(ITEM.to_owned()),
        items: Some(vec![root_item]),
        ..Default::default()
    }
}

/// `GetMoneyReward` (`:629-640`) — euros for Peacekeeper and Fence, roubles for everyone else.
fn get_money_reward(
    ctx: &QuestContext<'_>,
    trader_id: &str,
    reward_roubles: f64,
    reward_index: i32,
) -> Reward {
    // Determine currency based on trader
    // PK and Fence use Euros, everyone else is Roubles
    let currency = if trader_id == PEACEKEEPER || trader_id == FENCE {
        EUROS
    } else {
        ROUBLES
    };

    // Convert reward amount to Euros if necessary
    let reward_amount_to_give_player = if currency == EUROS {
        from_roubles(ctx, reward_roubles, EUROS)
    } else {
        reward_roubles
    };

    // Get chosen currency + amount and return
    generate_item_reward(currency, reward_amount_to_give_player, reward_index, false)
}

/// `GetRewardableItems` (`:652-684`) — the whole item table filtered down to tpls that make sense
/// as a reward from this trader.
pub fn get_rewardable_items<'a>(
    ctx: &QuestContext<'a>,
    repeatable_quest_config: &RepeatableQuestConfig,
    trader_id: &str,
) -> Vec<&'a str> {
    let items = ctx.items;

    // Get an array of seasonal items that should not be shown right now as seasonal event is not
    // active
    let seasonal_items = ctx.seasonal_item_tpl_blacklist;

    // `:674` repeats this lookup per item; it reads nothing the loop writes, so it is hoisted
    let trader_whitelist = repeatable_quest_config
        .trader_whitelist
        .iter()
        .find(|trader| trader.trader_id == trader_id);
    let reward_base_whitelist: Option<Vec<&str>> = trader_whitelist.map(|trader| {
        trader
            .reward_base_whitelist
            .iter()
            .map(String::as_str)
            .collect()
    });
    let reward_blacklist: HashSet<String> = repeatable_quest_config
        .reward_blacklist
        .iter()
        .cloned()
        .collect();
    let reward_base_type_blacklist: Vec<&str> = repeatable_quest_config
        .reward_base_type_blacklist
        .iter()
        .map(String::as_str)
        .collect();

    // Check for specific base classes which don't make sense as reward item
    // also check if the price is greater than 0; there are some items whose price can not be found
    // those are not in the game yet (e.g. AGS grenade launcher)
    items
        .iter()
        .filter(|(tpl, item_template)| {
            // Base "Item" item has no parent, ignore it
            if item_template
                .parent
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                return false;
            }

            if seasonal_items.contains(tpl.as_str()) {
                return false;
            }

            is_valid_reward_item(
                ctx,
                tpl,
                &reward_blacklist,
                &reward_base_type_blacklist,
                reward_base_whitelist.as_deref(),
            )
        })
        .map(|(tpl, _)| tpl.as_str())
        .collect()
}

/// `IsValidRewardItem` (`:695-738`) — whether `tpl` may be handed to the player as a reward, or
/// asked of them as a `Completion` hand-in (`CompletionQuestGenerator.cs:147`).
pub fn is_valid_reward_item(
    ctx: &QuestContext<'_>,
    tpl: &str,
    item_tpl_blacklist: &HashSet<String>,
    item_type_blacklist: &[&str],
    item_base_whitelist: Option<&[&str]>,
) -> bool {
    // Return early if not valid item to give as reward
    if !item_helper::is_valid_item(
        ctx.items,
        ctx.item_blacklist,
        ctx.handbook_prices,
        ctx.flea_prices,
        tpl,
        &DEFAULT_INVALID_BASE_TYPES,
    ) {
        return false;
    }

    // Check item is not blacklisted. `:709-714` tests `IsItemBlacklisted` twice; the repeat is
    // dropped here because it cannot answer differently.
    if ctx.item_blacklist.contains(tpl)
        || ctx.reward_item_blacklist.contains(tpl)
        || item_tpl_blacklist.contains(tpl)
    {
        return false;
    }

    // Item has blacklisted base types
    if item_helper::is_of_baseclasses(ctx.items, tpl, item_type_blacklist) {
        return false;
    }

    // Skip boss items
    if ctx.boss_items.contains(tpl) {
        return false;
    }

    // Trader has specific item base types they can give as rewards to player
    if item_base_whitelist
        .is_some_and(|whitelist| !item_helper::is_of_baseclasses(ctx.items, tpl, whitelist))
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::loot::random_util::TestSeedGuard;
    use crate::quest::QuestContext;
    use crate::quest::helper::PRAPOR;
    use crate::quest::models::{QuestInvariantSlice, RepeatableQuestConfig, tests::slice_value};

    const SEED: u64 = 42;

    const QUEST_CONFIG_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/configs/quest.json"
    );

    fn daily_config() -> RepeatableQuestConfig {
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(QUEST_CONFIG_PATH).expect("readable"))
                .expect("JSON");

        serde_json::from_value(config["repeatableQuests"][0].clone()).expect("parses")
    }

    /// The fixture slice with one price per tier band of `:443-448`.
    fn priced_slice() -> QuestInvariantSlice {
        let mut value = slice_value();
        value["defaultPresetOrItemPrices"] =
            serde_json::json!({ "cheap": 1000.0, "mid": 5000.0, "dear": 20000.0 });

        serde_json::from_value(value).expect("fixture slice parses")
    }

    fn stack_sizes_drawn(ctx: &QuestContext<'_>, tpl: &str) -> BTreeSet<i32> {
        (0..200)
            .map(|_| get_randomised_reward_item_stack_size_by_price(ctx, tpl))
            .collect()
    }

    /// Quirk 6 (`:443-448`): the 3000-10000 band offers `[2, 3]` and never 4, unlike the bands
    /// either side of it.
    #[test]
    fn the_stack_size_tier_table_skips_four_in_the_middle_band() {
        let slice = priced_slice();
        let ctx = QuestContext::from_slice(&slice);
        let _guard = TestSeedGuard::install(SEED);

        assert_eq!(
            stack_sizes_drawn(&ctx, "cheap"),
            BTreeSet::from([2, 3, 4]),
            "under 3000 RUB"
        );
        assert_eq!(
            stack_sizes_drawn(&ctx, "mid"),
            BTreeSet::from([2, 3]),
            "3000-10000 RUB"
        );
        assert_eq!(
            stack_sizes_drawn(&ctx, "dear"),
            BTreeSet::from([2, 3, 4]),
            "10001+ RUB"
        );
    }

    /// The reward list always opens with the experience reward (`:87-103`) and the money reward
    /// (`:106`), in that order, under the three keys the client expects (`:76-81`).
    #[test]
    fn a_seeded_reward_leads_with_experience_then_money() {
        let slice = crate::quest::models::tests::slice();
        let mut ctx = QuestContext::from_slice(&slice);
        let config = daily_config();
        let skill_rewards = &config.quest_config.elimination[0].possible_skill_rewards;

        let _guard = TestSeedGuard::install(SEED);
        let rewards = generate_reward(&mut ctx, 20, 1.0, PRAPOR, &config, skill_rewards, None)
            .expect("prapor is in the daily whitelist");

        assert_eq!(
            rewards.keys().collect::<Vec<_>>(),
            ["Success", "Started", "Fail"]
        );
        assert!(rewards["Started"].is_empty());
        assert!(rewards["Fail"].is_empty());

        let success = &rewards["Success"];
        assert_eq!(success[0].reward_type.as_deref(), Some("Experience"));
        assert!(success[0].value.expect("xp value") > 0.0);
        // `:84` starts the index at -1, so the first reward carries -1
        assert_eq!(success[0].index, Some(-1));

        assert_eq!(success[1].reward_type.as_deref(), Some("Item"));
        assert_eq!(success[1].index, Some(0));
        assert_eq!(
            success[1].items.as_ref().expect("money items")[0].template,
            ROUBLES
        );
    }
}
