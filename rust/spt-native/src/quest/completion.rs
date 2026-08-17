//! `Generators/RepeatableQuests/CompletionQuestGenerator.cs` — the hand-over-N-of-X repeatable
//! quest.
//!
//! Line references in this file are that generator unless another file is named.

use std::collections::HashSet;

use crate::loot::item_helper::{self, AMMO, ARMOR, WEAPON};
use crate::loot::math_util::interp1;
use crate::loot::models::{Diagnostic, ERROR, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{get_array_value, get_double, get_int, rand_int};
use crate::quest::models::{
    CompletionConfig, LevelledItemFilter, ListOrT, QuestCondition, QuestTypePool, RepeatableQuest,
    RepeatableQuestConfig, RepeatableQuestType,
};
use crate::quest::{QuestContext, helper, reward_generator};

/// `MaxRandomNumberAttempts` (`:33`).
const MAX_RANDOM_NUMBER_ATTEMPTS: i32 = 6;

/// `ItemHelper.cs:64-79` — `_dogTagTpls`, read only by `IsDogtag` (`ItemHelper.cs:728-731`), which
/// only this generator calls. It lives here rather than in `item_helper` until a second caller
/// needs it.
///
/// `ItemTpl.BARTER_DOGTAGT` (`ItemTpl.cs:974`) is not one of them, and is left out here too.
const DOG_TAG_TPLS: [&str; 14] = [
    "59f32bb586f774757e1e8442", // BEAR
    "6662e9aca7e0b43baa3d5f74", // BEAR EOD
    "6662e9cda7e0b43baa3d5f76", // BEAR TUE
    "59f32c3b86f77472a31742f0", // USEC
    "6662e9f37fa79a6d83730fa0", // USEC EOD
    "6662ea05f6259762c56f3189", // USEC TUE
    "675dc9d37ae1a8792107ca96", // BEAR prestige 1
    "675dcb0545b1a2d108011b2b", // BEAR prestige 2
    "684180bc51bf8645f7067bc8", // BEAR prestige 3
    "684181208d035f60230f63f9", // BEAR prestige 4
    "6764207f2fa5e32733055c4a", // USEC prestige 1
    "6764202ae307804338014c1a", // USEC prestige 2
    "68418091b5b0c9e4c60f0e7a", // USEC prestige 3
    "684180ee9b6d80d840042e8a", // USEC prestige 4
];

/// A `ServerLocalisationService.GetText` line the C# caller replays through its logger.
fn localised(level: &str, locale_key: &str, args: Option<serde_json::Value>) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args,
        message: None,
    }
}

/// `ItemHelper.IsDogtag` (`ItemHelper.cs:728-731`).
fn is_dogtag(tpl: &str) -> bool {
    DOG_TAG_TPLS.contains(&tpl)
}

/// `GetItemsToRetrievePool` (`:125-155`) — the item table filtered down to tpls the player could
/// reasonably be asked to hand in.
///
/// A `Vec` rather than a set: the C# `HashSet<MongoId>` is built by `ToHashSet` over the item
/// table and never has anything removed, so it enumerates in insertion order, and `:105` turns it
/// back into a list that `:270` indexes. Order is the parity contract, uniqueness is free.
fn get_items_to_retrieve_pool<'a>(
    ctx: &QuestContext<'a>,
    completion_config: &CompletionConfig,
    item_tpl_blacklist: &HashSet<String>,
) -> Vec<&'a str> {
    // Get seasonal items that should not be added to pool as seasonal event is not active
    let seasonal_items = ctx.seasonal_item_tpl_blacklist;

    let required_item_type_blacklist: Vec<&str> = completion_config
        .required_item_type_blacklist
        .iter()
        .map(String::as_str)
        .collect();

    // Check for specific base classes which don't make sense as reward item
    // also check if the price is greater than 0; there are some items whose price can not be found
    ctx.items
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

            // Valid reward items share same logic as items to retrieve
            reward_generator::is_valid_reward_item(
                ctx,
                tpl,
                item_tpl_blacklist,
                &required_item_type_blacklist,
                None,
            )
        })
        .map(|(tpl, _)| tpl.as_str())
        .collect()
}

/// `GetItemsWithinBudget` (`:165-180`) — the pool trimmed to what the budget affords, and the
/// budget itself.
fn get_items_within_budget<'a>(
    ctx: &QuestContext<'_>,
    pmc_level: i32,
    levels_config: &[f64],
    roubles_config: &[f64],
    items_to_retrieve_pool: Vec<&'a str>,
) -> (Vec<&'a str>, f64) {
    // Be fair, don't value the items be more expensive than the reward
    let multiplier = get_double(0.5, 1.0);
    let roubles_budget =
        (interp1(f64::from(pmc_level), levels_config, roubles_config) * multiplier).floor();

    // Make sure there is always a 5000 rouble budget available for selection
    let roubles_budget = roubles_budget.max(5000.0);

    (
        items_to_retrieve_pool
            .into_iter()
            // `double? < double` (`:179`) — a tpl neither price table knows compares false and is
            // dropped, where `:299` would otherwise throw on the same null.
            .filter(|tpl| {
                item_helper::get_item_price(ctx.handbook_prices, ctx.flea_prices, tpl)
                    .is_some_and(|price| price < roubles_budget)
            })
            .collect(),
        roubles_budget,
    )
}

/// The level-filtered union of a `[{minPlayerLevel, itemIds}]` list (`:199`/`:233`).
fn levelled_item_ids(filters: &[LevelledItemFilter], pmc_level: i32) -> HashSet<&str> {
    filters
        .iter()
        .filter(|filter| filter.min_player_level.is_some_and(|min| min <= pmc_level))
        .flat_map(|filter| filter.item_ids.iter().map(String::as_str))
        .collect()
}

/// `GetWhitelistedItemSelection` (`:188-214`).
fn get_whitelisted_item_selection<'a>(
    ctx: &QuestContext<'_>,
    item_selection: Vec<&'a str>,
    pmc_level: i32,
) -> Vec<&'a str> {
    let item_whitelist = ctx.completion_items_whitelist;

    // Whitelist doesn't exist or is empty, return original
    if item_whitelist.is_empty() {
        return item_selection;
    }

    // Filter and concatenate items according to current player level
    let item_ids_whitelisted = levelled_item_ids(item_whitelist, pmc_level);
    // `:368` asks `IsOfBaseclass` once per whitelisted id, answered from `ItemBaseClassService`'s
    // precomputed ancestor sets. The cache probe below is that same shape: one lookup of the item's
    // ancestor set, same answers as the walk it replaced (see `ItemBaseClassCache`).
    let whitelisted_base_classes: Vec<&str> = item_ids_whitelisted.iter().copied().collect();

    item_selection
        .into_iter()
        .filter(|tpl| {
            // Whitelist can contain item tpls and item base type ids
            ctx.base_classes
                .is_of_baseclasses(tpl, &whitelisted_base_classes)
                || item_ids_whitelisted.contains(tpl)
        })
        .collect()
}

/// `GetBlacklistedItemSelection` (`:222-246`).
fn get_blacklisted_item_selection<'a>(
    ctx: &QuestContext<'_>,
    item_selection: Vec<&'a str>,
    pmc_level: i32,
) -> Vec<&'a str> {
    let item_blacklist = ctx.completion_items_blacklist;

    // Blacklist doesn't exist or is empty, return original
    if item_blacklist.is_empty() {
        return item_selection;
    }

    // Filter and concatenate the arrays according to current player level
    let item_ids_blacklisted = levelled_item_ids(item_blacklist, pmc_level);
    // One cache probe per candidate item, as on the whitelist above.
    let blacklisted_base_classes: Vec<&str> = item_ids_blacklisted.iter().copied().collect();

    item_selection
        .into_iter()
        .filter(|tpl| {
            // Ported bug, not a typo: `:241` joins the two tests with `||` where the whitelist's
            // mirror image would need `&&`. An item is never its own base class, so a tpl listed
            // directly passes the left operand, and a tpl matching only a listed base class passes
            // the right one — the blacklist drops nothing unless a tpl is listed *and* descends
            // from something else in the same list.
            !ctx.base_classes
                .is_of_baseclasses(tpl, &blacklisted_base_classes)
                || !item_ids_blacklisted.contains(tpl)
        })
        .collect()
}

/// `GenerateCondition` (`:352-384`) — one `HandoverItem` condition.
fn generate_condition(
    ctx: &QuestContext<'_>,
    item_tpl: &str,
    value: f64,
    completion_config: &CompletionConfig,
) -> QuestCondition {
    let mut only_found_in_raid = completion_config.required_items_are_fir;
    let min_durability = if ctx
        .base_classes
        .is_of_baseclasses(item_tpl, &[WEAPON, ARMOR])
    {
        // `GetArrayValue` over a two element array (`:356`) — always a draw, never the range
        // between the two bounds.
        *get_array_value(&[
            completion_config.required_item_min_durability_min_max.min,
            completion_config.required_item_min_durability_min_max.max,
        ])
    } else {
        0
    };

    // Dog tags MUST NOT be FiR for them to work
    if is_dogtag(item_tpl) || ctx.base_classes.is_of_baseclass(item_tpl, AMMO) {
        only_found_in_raid = false;
    }

    QuestCondition {
        id: mongo_id::generate(),
        index: Some(0),
        parent_id: Some(String::new()),
        dynamic_locale: true,
        visibility_conditions: Some(Vec::new()),
        target: Some(ListOrT::List(vec![item_tpl.to_owned()])),
        value: Some(value),
        min_durability: Some(f64::from(min_durability)),
        max_durability: Some(100.0),
        dogtag_level: Some(0),
        only_found_in_raid: Some(only_found_in_raid),
        is_encoded: Some(false),
        condition_type: "HandoverItem".to_owned(),
        ..Default::default()
    }
}

/// `GenerateAvailableForFinish` (`:256-340`) — the hand-in conditions, appended to `quest`, and
/// the tpls they ask for.
fn generate_available_for_finish(
    ctx: &mut QuestContext<'_>,
    quest: &mut RepeatableQuest,
    completion_config: &CompletionConfig,
    mut item_selection: Vec<&str>,
    mut roubles_budget: f64,
) -> Vec<String> {
    let handbook_prices = ctx.handbook_prices;
    let flea_prices = ctx.flea_prices;

    // Store the indexes of items we are asking player to supply
    let distinct_items_to_retrieve_count = get_int(
        completion_config.unique_item_count.min,
        completion_config.unique_item_count.max,
    );
    let mut chosen_requirement_items_tpls: Vec<String> = Vec::new();
    let mut used_item_indexes: HashSet<i64> = HashSet::new();

    for _ in 0..distinct_items_to_retrieve_count {
        let mut chosen_item_index = rand_int(item_selection.len() as i64, None);
        let mut found = false;

        for _ in 0..MAX_RANDOM_NUMBER_ATTEMPTS {
            if used_item_indexes.contains(&chosen_item_index) {
                chosen_item_index = rand_int(item_selection.len() as i64, None);
            } else {
                found = true;
                break;
            }
        }

        if !found {
            ctx.diagnostics.push(localised(
                ERROR,
                "repeatable-no_reward_item_found_in_price_range",
                Some(serde_json::json!({ "minPrice": 0, "roublesBudget": roubles_budget })),
            ));

            return chosen_requirement_items_tpls;
        }

        // Store index of item we've already chosen for later checking. The indexes outlive the
        // list they point into — `:325` rebuilds `itemSelection` without clearing this set — so a
        // later pass can reject an index that now names a different tpl. Ported as-is.
        used_item_indexes.insert(chosen_item_index);

        let tpl_chosen = item_selection[chosen_item_index as usize];
        // `GetItemPrice(tplChosen)!.Value` (`:299`) — `:179` already dropped every priceless tpl,
        // so the C#'s null-forgiving deref cannot fire.
        let item_price = item_helper::get_item_price(handbook_prices, flea_prices, tpl_chosen)
            .expect("priced by CompletionQuestGenerator:179");
        let min_value = completion_config.requested_item_count.min;
        let max_value = completion_config.requested_item_count.max;

        let mut value = min_value;

        // Get the value range within budget
        let x = (roubles_budget / item_price).floor() as i32;
        let max_value = max_value.min(x);
        if max_value > min_value
        // If it doesn't blow the budget we have for the request, draw a random amount of the
        // selected Item type to be requested
        {
            value = rand_int(i64::from(min_value), Some(i64::from(max_value) + 1)) as i32;
        }

        roubles_budget -= f64::from(value) * item_price;

        // Push a CompletionCondition with the item and the amount of the item into quest
        chosen_requirement_items_tpls.push(tpl_chosen.to_owned());
        let condition = generate_condition(ctx, tpl_chosen, f64::from(value), completion_config);
        quest
            .quest
            .conditions
            .available_for_finish
            .as_mut()
            .expect("AvailableForFinish was null at CompletionQuestGenerator:319")
            .push(condition);

        // Is there budget left for more items
        if roubles_budget > 0.0 {
            // Reduce item pool to fit budget
            item_selection.retain(|tpl| {
                item_helper::get_item_price(handbook_prices, flea_prices, tpl)
                    .is_some_and(|price| price < roubles_budget)
            });

            if item_selection.is_empty() {
                // Nothing fits new budget, exit
                break;
            }

            continue;
        }

        break;
    }

    chosen_requirement_items_tpls
}

/// `Generate` (`:47-117`) — a randomised Completion quest, or `None` on either give-up path.
///
/// `quest_type_pool` is untouched: the C# takes it to satisfy `IRepeatableQuestGenerator` and never
/// reads it.
pub fn generate(
    ctx: &mut QuestContext<'_>,
    session_id: &str,
    pmc_level: i32,
    trader_id: &str,
    _quest_type_pool: &mut QuestTypePool,
    repeatable_config: &RepeatableQuestConfig,
) -> Option<RepeatableQuest> {
    let Some(completion_config) =
        helper::get_completion_config_by_pmc_level(pmc_level, repeatable_config)
    else {
        ctx.diagnostics.push(localised(
            WARNING,
            "repeatable-completion_config_no_template",
            Some(serde_json::json!({ "pmcLevel": pmc_level })),
        ));

        return None;
    };

    let levels_config = &repeatable_config.reward_scaling.levels;
    let roubles_config = &repeatable_config.reward_scaling.roubles;

    let Some(mut quest) = helper::generate_repeatable_template(
        ctx,
        RepeatableQuestType::Completion,
        trader_id,
        &repeatable_config.side,
        session_id,
    ) else {
        ctx.diagnostics.push(Diagnostic {
            level: ERROR.to_owned(),
            locale_key: None,
            args: None,
            message: Some(
                "Quest template null when attempting to create completion operational task."
                    .to_owned(),
            ),
        });

        return None;
    };

    let reward_blacklist: HashSet<String> =
        repeatable_config.reward_blacklist.iter().cloned().collect();

    // Filter the items.json items to items the player must retrieve to complete quest: shouldn't be
    // a quest item or "non-existent"
    let items_to_retrieve_pool =
        get_items_to_retrieve_pool(ctx, completion_config, &reward_blacklist);

    // Filter items within our budget
    let (items_to_retrieve_pool, budget) = get_items_within_budget(
        ctx,
        pmc_level,
        levels_config,
        roubles_config,
        items_to_retrieve_pool,
    );

    // We also have the option to use whitelist and/or blacklist which is defined in
    // repeatableQuests.json as
    // [{"minPlayerLevel": 1, "itemIds": ["id1",...]}, {"minPlayerLevel": 15, "itemIds": ["id3",...]}]
    let items_to_retrieve_pool = if completion_config.use_whitelist {
        get_whitelisted_item_selection(ctx, items_to_retrieve_pool, pmc_level)
    } else {
        items_to_retrieve_pool
    };

    let items_to_retrieve_pool = if completion_config.use_blacklist {
        get_blacklisted_item_selection(ctx, items_to_retrieve_pool, pmc_level)
    } else {
        items_to_retrieve_pool
    };

    // Filtering too harsh
    if items_to_retrieve_pool.is_empty() {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-completion_quest_whitelist_too_small_or_blacklist_too_restrictive",
            None,
        ));

        return None;
    }

    let selected_items = generate_available_for_finish(
        ctx,
        &mut quest,
        completion_config,
        items_to_retrieve_pool,
        budget,
    );

    // `selectedItems.ToHashSet()` (`:113`) — the tpls asked for cannot come back as rewards.
    let reward_tpl_blacklist: HashSet<String> = selected_items.into_iter().collect();

    quest.quest.rewards = reward_generator::generate_reward(
        ctx,
        pmc_level,
        1.0,
        trader_id,
        repeatable_config,
        &completion_config.possible_skill_rewards,
        Some(&reward_tpl_blacklist),
    );

    Some(quest)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Instant;

    use crate::loot::item_helper;
    use crate::loot::random_util::TestSeedGuard;
    use crate::quest::QuestContext;
    use crate::quest::helper::PRAPOR;
    use crate::quest::models::{
        ListOrT, QuestInvariantSlice, RepeatableQuestConfig, tests::slice_value,
    };

    const QUEST_CONFIG_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/configs/quest.json"
    );
    const TEMPLATES_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/repeatableQuests.json"
    );
    const ITEMS_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/items.json"
    );

    /// The midpoint of Completion's second shipped level band — the level `BENCHMARK.md`'s
    /// repeatable quest fixture generates at, where the whitelist unions to 137 candidates.
    const PMC_LEVEL: i32 = 20;

    /// Headroom over a single walk per item. The filter sits at ~1x once it stops restarting the
    /// walk, and at ~7x while it does, so this separates the two without being tight enough for a
    /// contended box to trip.
    const MAX_WALK_RATIO: f64 = 3.0;

    /// The base class every fixture item hangs off — whitelisted by tpl at `:205`, so the whole
    /// pool passes the whitelist through `IsOfBaseclass` rather than by name.
    const PARENT: &str = "cccccccccccccccccccccccc";
    const BLACKLISTED: &str = "eeeeeeeeeeeeeeeeeeeeeeee";

    fn json(path: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("readable")).expect("JSON")
    }

    /// The shipped Daily config with `eeee…` added to the reward blacklist — the set
    /// `GetItemsToRetrievePool` forwards to `IsValidRewardItem` (`:79`/`:149`).
    fn daily_config() -> RepeatableQuestConfig {
        let mut daily = json(QUEST_CONFIG_PATH)["repeatableQuests"][0].clone();
        daily["rewardBlacklist"]
            .as_array_mut()
            .expect("reward blacklist")
            .push(serde_json::json!(BLACKLISTED));

        serde_json::from_value(daily).expect("parses")
    }

    /// The fixture slice with the real Completion template spliced in and a three item pool: two
    /// affordable tpls and the blacklisted one.
    fn slice() -> QuestInvariantSlice {
        let mut value = slice_value();
        let templates = json(TEMPLATES_PATH);

        value["repeatableQuestTemplates"]["Completion"] =
            templates["templates"]["Completion"].clone();
        value["repeatableQuestTemplateIds"]["pmc"]["Completion"] =
            serde_json::json!("61604635c725987e815b1a46");

        let item = serde_json::json!({ "parent": PARENT, "type": "Item", "stackMaxSize": 1 });
        value["items"] = serde_json::json!({
            "aaaaaaaaaaaaaaaaaaaaaaaa": item,
            "bbbbbbbbbbbbbbbbbbbbbbbb": item,
            BLACKLISTED: item,
        });
        value["handbookPrices"] = serde_json::json!({
            "aaaaaaaaaaaaaaaaaaaaaaaa": 1000.0,
            "bbbbbbbbbbbbbbbbbbbbbbbb": 2000.0,
            BLACKLISTED: 1500.0,
        });
        value["fleaPrices"] = value["handbookPrices"].clone();
        value["completionItemsWhitelist"] =
            serde_json::json!([{ "minPlayerLevel": 1, "itemIds": [PARENT] }]);

        serde_json::from_value(value).expect("fixture slice parses")
    }

    /// `:264` draws 1-2 unique tpls for a level 20 PMC and `:312` draws 2-4 of each, and the
    /// blacklisted tpl never survives `:147` to be asked for.
    #[test]
    fn a_seeded_completion_quest_hands_over_unblacklisted_items_within_the_config_band() {
        let slice = slice();
        let config = daily_config();
        let mut generated = 0;
        // Ranges pass under a wrong-bounds draw; the *set* of values seen over the 25 seeds does
        // not. Both `:264` and `:312` are inclusive at their upper end, so both bounds must show up.
        let mut distinct_counts = BTreeSet::new();
        let mut hand_in_counts = BTreeSet::new();

        for seed in 1..=25u64 {
            let mut ctx = QuestContext::from_slice(&slice);
            let _guard = TestSeedGuard::install(seed);
            let Some(quest) = super::generate(
                &mut ctx,
                "6193a720f8ee7e52e4290000",
                20,
                PRAPOR,
                &mut serde_json::from_value(serde_json::json!({
                    "types": ["Completion"],
                    "pool": {
                        "Exploration": { "locations": {} },
                        "Elimination": { "targets": {} },
                        "Pickup": { "locations": {} }
                    }
                }))
                .expect("pool parses"),
                &config,
            ) else {
                continue;
            };
            generated += 1;

            let conditions = quest
                .quest
                .conditions
                .available_for_finish
                .as_ref()
                .expect("AvailableForFinish");

            // `:264` — the level 16-40 band asks `GetInt(1, 2)` distinct tpls
            distinct_counts.insert(conditions.len());

            for condition in conditions {
                assert_eq!(condition.condition_type, "HandoverItem", "seed {seed}");

                let Some(ListOrT::List(target)) = &condition.target else {
                    panic!("seed {seed}: `:375` mints a single element list target");
                };
                assert_eq!(target.len(), 1);
                assert_ne!(target[0], BLACKLISTED, "seed {seed}: blacklisted tpl asked");

                // `:312` — the band's `requestedItemCount` is 2-4, and the budget is never tight
                // enough to clamp `maxValue` below it
                hand_in_counts.insert(condition.value.expect("hand-in count") as i32);

                // `:378-379`
                assert_eq!(condition.max_durability, Some(100.0));
                assert_eq!(condition.dogtag_level, Some(0));
            }
        }

        assert_eq!(generated, 25, "every seed should produce a quest");
        // `GetInt` (`:264`) is inclusive at both ends — an exclusive draw would never reach 2
        assert_eq!(distinct_counts, BTreeSet::from([1, 2]));
        // `RandInt(min, max + 1)` (`:312`) — without the `+ 1` the 4 would never appear
        assert_eq!(hand_in_counts, BTreeSet::from([2, 3, 4]));
    }

    /// The `||` at `:241` — a tpl listed by name survives (an item is never its own base class),
    /// a tpl matching only a listed base class survives, and only a tpl that is both listed *and*
    /// descended from something else in the same list is dropped.
    #[test]
    fn the_blacklisted_item_selection_only_drops_a_tpl_listed_twice_over() {
        const POOL: [&str; 3] = [
            "aaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbb",
            BLACKLISTED,
        ];

        let blacklisted = |item_ids: serde_json::Value| {
            let mut value = slice_value();
            value["items"] = serde_json::json!({
                "aaaaaaaaaaaaaaaaaaaaaaaa": { "parent": PARENT, "type": "Item" },
                "bbbbbbbbbbbbbbbbbbbbbbbb": { "parent": PARENT, "type": "Item" },
                BLACKLISTED: { "parent": PARENT, "type": "Item" },
            });
            value["completionItemsBlacklist"] =
                serde_json::json!([{ "minPlayerLevel": 1, "itemIds": item_ids }]);

            serde_json::from_value(value).expect("fixture slice parses")
        };

        let by_name: QuestInvariantSlice = blacklisted(serde_json::json!([BLACKLISTED]));
        let by_base: QuestInvariantSlice = blacklisted(serde_json::json!([PARENT]));
        let by_both: QuestInvariantSlice = blacklisted(serde_json::json!([BLACKLISTED, PARENT]));

        for (slice, expected) in [
            (&by_name, &POOL[..]),
            (&by_base, &POOL[..]),
            (&by_both, &POOL[..2]),
        ] {
            let ctx = QuestContext::from_slice(slice);
            assert_eq!(
                super::get_blacklisted_item_selection(&ctx, POOL.to_vec(), 20),
                expected
            );
        }
    }

    /// The fixture slice carrying the real shipped item table and the real Completion whitelist.
    /// Only `parent` is projected: it is the one member the base class walk reads, and the filter's
    /// other test is a plain `contains` on the tpl.
    ///
    /// Requires `scripts/decompress-assets.sh` to have unpacked `items.json`.
    fn real_items_slice() -> QuestInvariantSlice {
        let mut value = slice_value();
        let raw = json(ITEMS_PATH);
        let mut items = serde_json::Map::new();

        for (tpl, template) in raw.as_object().expect("items object") {
            items.insert(
                tpl.clone(),
                serde_json::json!({ "parent": template["_parent"] }),
            );
        }

        value["items"] = serde_json::Value::Object(items);
        value["completionItemsWhitelist"] =
            json(TEMPLATES_PATH)["data"]["Completion"]["itemsWhitelist"].clone();

        serde_json::from_value(value).expect("real item slice parses")
    }

    /// `GetWhitelistedItemSelection` (`:365-371`) tests every whitelisted candidate against every
    /// item in the pool. C# affords that shape because `ItemBaseClassService` answers each
    /// `IsOfBaseclass` from an ancestor set precomputed at startup, in O(1);
    /// [`item_helper::is_of_baseclass`] walks the parent chain live, so the ported shape restarts a
    /// full walk per candidate — 137 of them at the level this fixture runs at — and the filter
    /// ends up costing more than the rest of the quest call put together.
    ///
    /// Pinned against a one-walk reference measured on the same box in the same profile rather
    /// than a wall clock bound, because the absolute cost differs by an order of magnitude between
    /// Release and debug and would be flaky either way.
    #[test]
    fn the_whitelist_filter_walks_each_item_chain_once() {
        let slice = real_items_slice();
        let ctx = QuestContext::from_slice(&slice);
        let selection: Vec<&str> = slice.items.keys().map(String::as_str).collect();

        let whitelisted = super::levelled_item_ids(ctx.completion_items_whitelist, PMC_LEVEL);
        let candidates: Vec<&str> = whitelisted.iter().copied().collect();

        // One walk per item, every candidate tested at each link.
        let start = Instant::now();
        let reference: Vec<&str> = selection
            .iter()
            .copied()
            .filter(|tpl| {
                item_helper::is_of_baseclasses(ctx.items, tpl, &candidates)
                    || whitelisted.contains(tpl)
            })
            .collect();
        let reference_elapsed = start.elapsed();

        let start = Instant::now();
        let kept = super::get_whitelisted_item_selection(&ctx, selection.clone(), PMC_LEVEL);
        let elapsed = start.elapsed();

        assert_eq!(
            kept, reference,
            "the filter must keep what a single walk per item keeps"
        );

        let ratio = elapsed.as_secs_f64() / reference_elapsed.as_secs_f64();
        assert!(
            ratio <= MAX_WALK_RATIO,
            "the whitelist filter cost {ratio:.1}x a single walk per item over {} templates \
             ({elapsed:?} against {reference_elapsed:?}), so it is restarting the parent walk per \
             whitelisted candidate; test every candidate in one walk instead",
            selection.len()
        );
    }
}
