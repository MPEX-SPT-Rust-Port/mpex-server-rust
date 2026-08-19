//! `Generators/RepeatableQuests/PickupQuestGenerator.cs` — the fetch-N-items-of-a-type repeatable
//! quest.
//!
//! Line references in this file are that generator unless another file is named.

use crate::loot::random_util::{get_array_value, rand_int};
use crate::quest::models::{
    ListOrT, QuestTypePool, RepeatableQuest, RepeatableQuestConfig, RepeatableQuestType,
};
use crate::quest::{QuestContext, helper, reward_generator};

/// `Generate` (`:22-67`) — dead in production: `RepeatableQuestController` dispatches on the
/// `PickUp` of `QuestTypeEnum`, which never matches the `Pickup` the pool and config spell.
///
/// `questTypePool` is untouched — the location draw the C# would have made is commented out at
/// `:46-47`.
pub fn generate(
    ctx: &mut QuestContext<'_>,
    session_id: &str,
    pmc_level: i32,
    trader_id: &str,
    _quest_type_pool: &mut QuestTypePool,
    repeatable_config: &RepeatableQuestConfig,
) -> Option<RepeatableQuest> {
    // `:30` is a plain assignment that cannot throw — only `Daily_Savage` ships a `Pickup` block,
    // and the other two configs carry the null into `:39`, after `:32` has spent its draws.
    let pickup_config = repeatable_config.quest_config.pickup.as_ref();

    // `:32` — the helper logs its own give-up reasons and returns null
    // (`RepeatableQuestHelper.cs:102-121`). This generator, unlike
    // `ExplorationQuestGenerator.cs:80-84`, never checks for it: the null rides through the `:39`
    // and `:41` draws to the dereference at `:49`.
    let quest = helper::generate_repeatable_template(
        ctx,
        RepeatableQuestType::Pickup,
        trader_id,
        &repeatable_config.side,
        session_id,
    );

    let pickup_config = pickup_config.expect("Pickup config was null at PickupQuestGenerator:39");

    // `GetArrayValue` (`:39`) is `GetRandomElement` (`RandomUtil.cs:487-490`), which throws on a
    // null list and on an empty one (`RandomUtil.cs:158-169`) — `get_array_value` panics on the
    // empty case for free.
    let item_type_to_fetch_with_count = get_array_value(
        pickup_config
            .item_type_to_fetch_with_max_count
            .as_deref()
            .expect("ItemTypeToFetchWithMaxCount was null at PickupQuestGenerator:39"),
    );

    // `MinimumPickupCount.Value` (`:42`) throws on a null; `MaximumPickupCount + 1` (`:43`) does
    // not — a null max lifts through to `RandInt`'s optional `high`, which then draws
    // `[0, minPickupCount)` instead (`RandomUtil.cs:254-264`).
    let minimum_pickup_count = item_type_to_fetch_with_count
        .minimum_pickup_count
        .expect("MinimumPickupCount was null at PickupQuestGenerator:42");
    let item_count_to_fetch = rand_int(
        i64::from(minimum_pickup_count),
        item_type_to_fetch_with_count
            .maximum_pickup_count
            .map(|maximum| i64::from(maximum) + 1),
    );

    // `ItemType` is a `string?` the C# drops into the list as-is; a null one would serialise as a
    // null element, which this port spells as the empty string the way `MongoId`'s default does.
    let item_type = item_type_to_fetch_with_count
        .item_type
        .clone()
        .unwrap_or_default();

    // Choose location - doesn't seem to work for anything other than 'any'
    // var locationKey: string = this.randomUtil.drawRandomFromDict(questTypePool.pool.Pickup.locations)[0];
    // var locationTarget = questTypePool.pool.Pickup.locations[locationKey];

    // `quest.Conditions` (`:49`) is where a null template lands, and `FirstOrDefault` hands back a
    // null the C# then dereferences at `:50`.
    let mut quest = quest.expect("Template was null at PickupQuestGenerator:49");
    let find_condition = quest
        .quest
        .conditions
        .available_for_finish
        .as_mut()
        .expect("AvailableForFinish was null at PickupQuestGenerator:49")
        .iter_mut()
        .find(|condition| condition.condition_type == "FindItem")
        .expect("No FindItem condition at PickupQuestGenerator:50");
    find_condition.target = Some(ListOrT::List(vec![item_type.clone()]));
    find_condition.value = Some(item_count_to_fetch as f64);

    // `:53-57` — the `CounterCreator`'s `Equipment` condition, each hop a C# null dereference.
    // var locationCondition = counterCreatorCondition._props.counter.conditions.find(x => x._parent === "Location");
    // (locationCondition._props as ILocationConditionProps).target = [...locationTarget];
    let equipment_condition = quest
        .quest
        .conditions
        .available_for_finish
        .as_mut()
        .expect("AvailableForFinish was null at PickupQuestGenerator:53")
        .iter_mut()
        .find(|condition| condition.condition_type == "CounterCreator")
        .expect("No CounterCreator condition at PickupQuestGenerator:57")
        .counter
        .as_mut()
        .expect("Counter was null at PickupQuestGenerator:57")
        .conditions
        .as_mut()
        .expect("Conditions were null at PickupQuestGenerator:57")
        .iter_mut()
        .find(|condition| condition.condition_type == "Equipment")
        .expect("No Equipment condition at PickupQuestGenerator:58");
    equipment_condition.equipment_inclusive = Some(vec![vec![item_type]]);

    // Add rewards
    // `:64` passes a difficulty of 1 flat — Pickup has no difficulty scaling.
    quest.quest.rewards = reward_generator::generate_reward(
        ctx,
        pmc_level,
        1.0,
        trader_id,
        repeatable_config,
        &pickup_config.possible_skill_rewards,
        None,
    );

    Some(quest)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::diag::DiagSink;
    use crate::loot::random_util::TestSeedGuard;
    use crate::quest::models::{
        ListOrT, QuestTypePool, QuestVaryingRequest, RepeatableQuestConfig,
        tests::{varying_value, views_override_value},
    };
    use crate::quest::{QuestContext, QuestViews};

    const QUEST_CONFIG_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/configs/quest.json"
    );
    const TEMPLATES_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/repeatableQuests.json"
    );

    /// `Models/Enums/Traders.cs:9` — the only trader `Daily_Savage` whitelists, and the reward
    /// generator gives up on a trader that is not on the config's whitelist.
    const FENCE: &str = "579dc571d53a0658a154fbec";

    fn json(path: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("readable")).expect("JSON")
    }

    /// `Daily_Savage` is the only shipped config carrying a `Pickup` block (`QuestConfig.cs:352`).
    fn savage_config() -> RepeatableQuestConfig {
        serde_json::from_value(json(QUEST_CONFIG_PATH)["repeatableQuests"][2].clone())
            .expect("parses")
    }

    /// The fixture views with the real Pickup template spliced in.
    fn views() -> QuestViews {
        let mut value = views_override_value();
        let templates = json(TEMPLATES_PATH);

        value["repeatableQuestTemplates"]["Pickup"] = templates["templates"]["Pickup"].clone();

        QuestViews::Override(Box::new(
            serde_json::from_value(value).expect("fixture views parse"),
        ))
    }

    /// The fixture varying half with the Pickup scav template id spliced in.
    fn varying() -> QuestVaryingRequest {
        let mut value = varying_value();

        value["repeatableQuestTemplateIds"]["scav"]["Pickup"] =
            serde_json::json!("628f588ebb558574b2260fe5");

        serde_json::from_value(value).expect("fixture varying parses")
    }

    /// Pickup draws nothing from the pool, so any pool does.
    fn pool() -> QuestTypePool {
        serde_json::from_value(serde_json::json!({
            "types": ["Exploration", "Completion"],
            "pool": {
                "Exploration": { "locations": {} },
                "Elimination": { "targets": {} },
                "Pickup": { "locations": {} }
            }
        }))
        .expect("pool parses")
    }

    /// The two draws (`:39` the item type, `:41` its count) land on the `FindItem` condition and
    /// the `CounterCreator`'s `Equipment` condition.
    #[test]
    fn a_seeded_pickup_quest_asks_for_the_drawn_item_type() {
        let views = views();
        let varying = varying();
        let config = savage_config();
        let pickup_config = config.quest_config.pickup.as_ref().expect("Pickup config");
        let entries = pickup_config
            .item_type_to_fetch_with_max_count
            .as_ref()
            .expect("ItemTypeToFetchWithMaxCount");
        let mut drawn = BTreeSet::new();
        let mut counts = BTreeSet::new();

        for seed in 1..=60u64 {
            let mut ctx = QuestContext::new(&views, &varying);
            ctx.diagnostics = DiagSink::capture();
            let mut pool = pool();
            let _guard = TestSeedGuard::install(seed);
            let quest = super::generate(
                &mut ctx,
                "6193a720f8ee7e52e4290000",
                20,
                FENCE,
                &mut pool,
                &config,
            )
            .expect("the shipped Pickup template and config generate");

            let conditions = quest
                .quest
                .conditions
                .available_for_finish
                .as_ref()
                .expect("AvailableForFinish");

            // `:49-51` — the drawn item type and count on the `FindItem` condition
            let find_condition = conditions
                .iter()
                .find(|condition| condition.condition_type == "FindItem")
                .expect("FindItem condition");
            let Some(ListOrT::List(target)) = &find_condition.target else {
                panic!("seed {seed}: FindItem target is not a list");
            };
            assert_eq!(target.len(), 1, "seed {seed}");
            let item_type = target[0].clone();

            let entry = entries
                .iter()
                .find(|entry| entry.item_type.as_deref() == Some(item_type.as_str()))
                .unwrap_or_else(|| panic!("seed {seed}: {item_type} is not a configured type"));
            let count = find_condition.value.expect("FindItem value") as i32;
            let minimum = entry.minimum_pickup_count.expect("minPickupCount");
            let maximum = entry.maximum_pickup_count.expect("maxPickupCount");
            assert!(
                (minimum..=maximum).contains(&count),
                "seed {seed}: {count} outside {minimum}..={maximum}"
            );

            // `:57-61` — the same item type on the `Equipment` counter condition, the template's
            // other two counter conditions left alone
            let counter_conditions = conditions
                .iter()
                .find(|condition| condition.condition_type == "CounterCreator")
                .expect("CounterCreator condition")
                .counter
                .as_ref()
                .expect("counter")
                .conditions
                .as_ref()
                .expect("counter conditions");
            assert_eq!(counter_conditions.len(), 3, "seed {seed}");
            let equipment_condition = counter_conditions
                .iter()
                .find(|condition| condition.condition_type == "Equipment")
                .expect("Equipment condition");
            assert_eq!(
                equipment_condition.equipment_inclusive.as_deref(),
                Some(&[vec![item_type.clone()]][..]),
                "seed {seed}"
            );

            // `:64` — rewards replace the template's empty set
            assert!(quest.quest.rewards.is_some(), "seed {seed}");

            // Pickup never touches the pool
            assert_eq!(pool.types, ["Exploration", "Completion"]);

            drawn.insert(item_type);
            counts.insert((minimum, maximum, count));
        }

        // `:39` draws across the whole list, and `:41`'s `+ 1` puts the upper bound in reach
        assert!(drawn.len() > 1);
        assert!(
            counts
                .iter()
                .any(|(_, maximum, count)| count == maximum && *count > 1)
        );
    }

    /// `:30` reads a config the two Pmc configs do not carry, and `:39` dereferences it — after
    /// `:32` has generated the template, which is why the panic is not raised at `:30`.
    #[test]
    #[should_panic(expected = "PickupQuestGenerator:39")]
    fn a_config_without_a_pickup_block_throws_at_the_first_dereference() {
        let views = views();
        let varying = varying();
        let mut ctx = QuestContext::new(&views, &varying);
        ctx.diagnostics = DiagSink::capture();
        let daily: RepeatableQuestConfig =
            serde_json::from_value(json(QUEST_CONFIG_PATH)["repeatableQuests"][0].clone())
                .expect("parses");

        let _guard = TestSeedGuard::install(1);
        super::generate(
            &mut ctx,
            "6193a720f8ee7e52e4290000",
            20,
            FENCE,
            &mut pool(),
            &daily,
        );
    }
}
