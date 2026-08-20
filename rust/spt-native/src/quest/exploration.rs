//! `Generators/RepeatableQuests/ExplorationQuestGenerator.cs` — the survive-N-raids repeatable
//! quest.
//!
//! Line references in this file are that generator unless another file is named.

use crate::loot::math_util::map_to_range;
use crate::loot::models::{Diagnostic, ERROR, WARNING};
use crate::loot::mongo_id;
use crate::loot::random_util::{
    draw_random_from_dict, draw_random_from_list, get_chance_100, rand_int,
};
use crate::quest::models::{
    ExitView, ExplorationConfig, ListOrT, QuestConditionCounterCondition, QuestTypePool,
    RepeatableQuest, RepeatableQuestConfig, RepeatableQuestType,
};
use crate::quest::{QuestContext, helper, reward_generator};

/// The `typeof(T).FullName` this file's diagnostics log under.
const CATEGORY: &str = "SPTarkov.Server.Core.Generators.RepeatableQuests.ExplorationQuestGenerator";

/// A `ServerLocalisationService.GetText` line the C# caller replays through its logger.
fn localised(level: &str, locale_key: &str, args: Option<serde_json::Value>) -> Diagnostic {
    Diagnostic {
        category: CATEGORY,
        level: level.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args,
        message: None,
    }
}

/// `protected record LocationInfo` (`:30-35`). `LocationName` is an `ELocationName` in the C#,
/// whose `ToString` is the enum member name — the same string the pool is keyed by here.
struct LocationInfo {
    location_name: String,
    location_target: Vec<String>,
    requires_specific_extract: bool,
    num_of_extracts_required: i32,
}

/// `GetNumberOfExits` (`:163-173`) — one draw, with bounds that depend on the branch.
fn get_number_of_exits(
    exploration_config: &ExplorationConfig,
    requires_specific_extract: bool,
) -> i32 {
    let exit_times_min = if requires_specific_extract {
        exploration_config.minimum_extracts_with_specific_exit
    } else {
        exploration_config.minimum_extracts
    };

    // Different max extract count when specific extract needed
    let exit_times_max = if requires_specific_extract {
        exploration_config.maximum_extracts_with_specific_exit + 1
    } else {
        exploration_config.maximum_extracts + 1
    };

    // `RandInt(low, high)` is `GetInt32(low, high)` (`RandomUtil.cs:263`), exclusive at the top —
    // which is what the `+ 1` above is for, and what `rand_int`'s two argument form already is.
    rand_int(i64::from(exit_times_min), Some(i64::from(exit_times_max))) as i32
}

/// `TryGetLocationInfo` (`:123-155`) — the drawn location, or `None` once the pool is empty, which
/// also drops `Exploration` from the pool's type list (`:133`).
fn try_get_location_info(
    exploration_config: &ExplorationConfig,
    pool: &mut QuestTypePool,
) -> Option<LocationInfo> {
    if pool
        .pool
        .exploration
        .locations
        .as_ref()
        .is_none_or(indexmap::IndexMap::is_empty)
    {
        // there are no more locations left for exploration; delete it as a possible quest type
        pool.types.retain(|quest_type| quest_type != "Exploration");

        return None;
    }

    let locations = pool
        .pool
        .exploration
        .locations
        .as_mut()
        .expect("checked above");

    // If location drawn is factory, it's possible to either get factory4_day and factory4_night use
    // index 0, as the key is factory4_day
    // `DrawRandomFromDict(dict)[0]` (`:140`) — one key, with replacement.
    let location_key = draw_random_from_dict(locations, 1, true)
        .into_iter()
        .next()
        .expect("DrawRandomFromDict drew nothing at ExplorationQuestGenerator:140");

    // Make the location info object
    // The C# indexer at `:143` throws on a key the dict does not carry, which the draw rules out.
    let location_target = locations[&location_key].clone();

    let requires_specific_extract = get_chance_100(exploration_config.specific_exits.chance);

    let num_extracts = get_number_of_exits(exploration_config, requires_specific_extract);

    // Remove the location from the available pool
    locations.shift_remove(&location_key);

    Some(LocationInfo {
        location_name: location_key,
        location_target,
        requires_specific_extract,
        num_of_extracts_required: num_extracts,
    })
}

/// `GetLocationExitsForSide` (`:181-186`) — a map the slice carries no extracts for is the C#
/// null, which `:256` reports.
fn get_location_exits_for_side<'a>(
    ctx: &QuestContext<'a>,
    location_key: &str,
    player_group: &str,
) -> Option<Vec<&'a ExitView>> {
    // `:183` lowercases the key before the lookup, which is why the slice is keyed that way.
    let map_extracts = ctx.extracts_by_location.get(&location_key.to_lowercase())?;

    Some(
        map_extracts
            .iter()
            .filter(|exit| exit.side.as_deref() == Some(player_group))
            .collect(),
    )
}

/// `TryGenerateAvailableForFinish` (`:194-236`).
fn try_generate_available_for_finish(
    ctx: &mut QuestContext<'_>,
    quest: &mut RepeatableQuest,
    location_info: &LocationInfo,
) -> bool {
    // This should never be hit, this is here to shut the compiler up.
    // `AvailableForFinish?[0]` (`:197`) only guards the null list — an empty one throws there, and
    // panics here.
    if !quest
        .quest
        .conditions
        .available_for_finish
        .as_ref()
        .is_some_and(|conditions| conditions[0].counter.is_some())
    {
        ctx.diagnostics.push(Diagnostic {
            category: CATEGORY,
            level: ERROR.to_owned(),
            locale_key: None,
            args: None,
            message: Some("Counter is null, something has gone terribly wrong".to_owned()),
        });

        return false;
    }

    // Lookup the location. `GetQuestLocationByMapId` logs its own miss
    // (`RepeatableQuestHelper.cs:206-210`) before `:208` logs a second line for the same failure.
    let Some(location) = helper::get_quest_location_by_map_id(ctx, &location_info.location_name)
    else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-unable_to_find_location_id_for_location_name",
            Some(serde_json::json!(location_info.location_name)),
        ));

        return false;
    };
    let location = location.to_owned();

    let exit_status_condition = QuestConditionCounterCondition {
        id: Some(mongo_id::generate()),
        dynamic_locale: Some(true),
        status: Some(vec!["Survived".to_owned()]),
        condition_type: "ExitStatus".to_owned(),
        ..Default::default()
    };

    let location_condition = QuestConditionCounterCondition {
        id: Some(mongo_id::generate()),
        dynamic_locale: Some(true),
        target: Some(ListOrT::List(location_info.location_target.clone())),
        condition_type: "Location".to_owned(),
        ..Default::default()
    };

    let available_for_finish = &mut quest
        .quest
        .conditions
        .available_for_finish
        .as_mut()
        .expect("checked above")[0];
    let counter = available_for_finish
        .counter
        .as_mut()
        .expect("checked above");
    counter.id = Some(mongo_id::generate());
    counter.conditions = Some(vec![exit_status_condition, location_condition]);
    available_for_finish.value = Some(f64::from(location_info.num_of_extracts_required));
    available_for_finish.id = mongo_id::generate();

    quest.quest.location = Some(location);

    true
}

/// `GenerateQuestConditionCounter` (`:293-302`).
fn generate_quest_condition_counter(exit: &ExitView) -> QuestConditionCounterCondition {
    QuestConditionCounterCondition {
        id: Some(mongo_id::generate()),
        dynamic_locale: Some(true),
        exit_name: exit.name.clone(),
        condition_type: "ExitName".to_owned(),
        ..Default::default()
    }
}

/// `TryGenerateSpecificExtractRequirement` (`:246-285`).
fn try_generate_specific_extract_requirement(
    ctx: &mut QuestContext<'_>,
    quest: &mut RepeatableQuest,
    repeatable_config: &RepeatableQuestConfig,
    exploration_config: &ExplorationConfig,
    location_info: &LocationInfo,
) -> bool {
    // Fetch extracts for the requested side
    let Some(map_exits) =
        get_location_exits_for_side(ctx, &location_info.location_name, &repeatable_config.side)
    else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-unable_to_find_exits_for_location",
            Some(serde_json::json!(location_info.location_name)),
        ));

        return false;
    };

    // Only get exits that have a greater than 0% chance to spawn
    // `exit.Chance > 0` on a `double?` (`:263`) — a null chance compares false and is dropped.
    let exit_pool = map_exits
        .into_iter()
        .filter(|exit| exit.chance.is_some_and(|chance| chance > 0.0));

    // Exclude exits with a requirement to leave (e.g. car extracts)
    let possible_exits: Vec<&ExitView> = exit_pool
        .filter(|exit| {
            exploration_config
                .specific_exits
                .passage_requirement_whitelist
                .contains(&exit.passage_requirement)
        })
        .collect();

    if possible_exits.is_empty() {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-unable_choose_exit_pool_empty",
            Some(serde_json::json!(location_info.location_name)),
        ));

        return false;
    }

    // Choose one of the exits we filtered above
    // `DrawRandomFromList(list)[0]` (`:278`) — one exit, with replacement.
    let chosen_exit = draw_random_from_list(&possible_exits, 1, true)
        .into_iter()
        .next()
        .expect("DrawRandomFromList drew nothing at ExplorationQuestGenerator:278");

    // Create a quest condition to leave raid via chosen exit
    let exit_condition = generate_quest_condition_counter(chosen_exit);
    quest
        .quest
        .conditions
        .available_for_finish
        .as_mut()
        .expect("AvailableForFinish was null at ExplorationQuestGenerator:282")[0]
        .counter
        .as_mut()
        .expect("Counter was null at ExplorationQuestGenerator:282")
        .conditions
        .as_mut()
        .expect("Conditions were null at ExplorationQuestGenerator:282")
        .push(exit_condition);

    true
}

/// `Generate` (`:49-113`) — a randomised Exploration quest, or `None` on any of the give-up paths.
///
/// Pool mutations land on `quest_type_pool`: `:133` drops the quest type, `:152` the drawn
/// location.
pub fn generate(
    ctx: &mut QuestContext<'_>,
    session_id: &str,
    pmc_level: i32,
    trader_id: &str,
    quest_type_pool: &mut QuestTypePool,
    repeatable_config: &RepeatableQuestConfig,
) -> Option<RepeatableQuest> {
    let Some(exploration_config) =
        helper::get_exploration_config_by_pmc_level(pmc_level, repeatable_config)
    else {
        ctx.diagnostics.push(localised(
            WARNING,
            "repeatable-exploration_config_no_template",
            Some(serde_json::json!({ "pmcLevel": pmc_level })),
        ));

        return None;
    };

    // Try and get a location to generate for
    let Some(location_info) = try_get_location_info(exploration_config, quest_type_pool) else {
        ctx.diagnostics.push(localised(
            WARNING,
            "repeatable-no_location_found_for_exploration_quest_generation",
            None,
        ));

        return None;
    };

    // Generate the quest template
    let Some(mut quest) = helper::generate_repeatable_template(
        ctx,
        RepeatableQuestType::Exploration,
        trader_id,
        &repeatable_config.side,
        session_id,
    ) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-quest_generation_failed_no_template",
            Some(serde_json::json!("exploration")),
        ));

        return None;
    };

    // Generate the available for finish exit condition
    if !try_generate_available_for_finish(ctx, &mut quest, &location_info) {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-available_for_finish_condition_failed_to_generate",
            Some(serde_json::json!(location_info.location_name)),
        ));

        return None;
    }

    // If we require a specific extract requirement, generate it
    if location_info.requires_specific_extract
        && !try_generate_specific_extract_requirement(
            ctx,
            &mut quest,
            repeatable_config,
            exploration_config,
            &location_info,
        )
    {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-specific_extract_condition_failed_to_generate",
            Some(serde_json::json!(location_info.location_name)),
        ));

        return None;
    }

    // Difficulty for exploration goes from 1 extract to maxExtracts
    // Difficulty for reward goes from 0.2...1 -> map
    let difficulty = map_to_range(
        f64::from(location_info.num_of_extracts_required),
        1.0,
        f64::from(exploration_config.maximum_extracts),
        0.2,
        1.0,
    );
    quest.quest.rewards = reward_generator::generate_reward(
        ctx,
        pmc_level,
        difficulty,
        trader_id,
        repeatable_config,
        &exploration_config.possible_skill_rewards,
        None,
    );

    Some(quest)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::diag::DiagSink;
    use crate::loot::random_util::TestSeedGuard;
    use crate::quest::helper::PRAPOR;
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

    /// The two extracts that survive all three filters at `:185`/`:263`/`:266`.
    const DRAWABLE_EXITS: [&str; 2] = ["Dorms V-Ex", "Crossroads"];

    fn json(path: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("readable")).expect("JSON")
    }

    fn daily_config() -> RepeatableQuestConfig {
        serde_json::from_value(json(QUEST_CONFIG_PATH)["repeatableQuests"][0].clone())
            .expect("parses")
    }

    /// A mixed-case pool key: `:183` lowercases it before the extract lookup, while `:204` looks
    /// the location id up under the key as drawn.
    const LOCATION: &str = "Interchange";
    const LOCATION_ID: &str = "5714dbc024597771384a510d";

    /// The fixture views with the real Exploration template spliced in and five extracts on
    /// `interchange`: two drawable ones, plus one rejected per filter — wrong side (`:185`), zero
    /// spawn chance (`:263`) and a passage requirement outside the whitelist (`:266`).
    fn views() -> QuestViews {
        let mut value = views_override_value();
        let templates = json(TEMPLATES_PATH);

        value["repeatableQuestTemplates"]["Exploration"] =
            templates["templates"]["Exploration"].clone();
        value["repeatableQuestTemplateIds"]["pmc"]["Exploration"] =
            serde_json::json!("616041eb031af660100c9967");
        value["locationIdMap"][LOCATION] = serde_json::json!(LOCATION_ID);
        value["extractsByLocation"] = serde_json::json!({
            LOCATION.to_lowercase(): [
                { "name": DRAWABLE_EXITS[0], "side": "Pmc", "chance": 100.0,
                  "passageRequirement": "TransferItem" },
                { "name": "Scav Land", "side": "Scav", "chance": 100.0,
                  "passageRequirement": "None" },
                { "name": DRAWABLE_EXITS[1], "side": "Pmc", "chance": 100.0,
                  "passageRequirement": "None" },
                { "name": "Car Extract", "side": "Pmc", "chance": 0.0,
                  "passageRequirement": "None" },
                { "name": "Locked Gate", "side": "Pmc", "chance": 100.0,
                  "passageRequirement": "Requirement" },
            ]
        });

        QuestViews::Override(Box::new(
            serde_json::from_value(value).expect("fixture views parse"),
        ))
    }

    /// The fixture varying half. The Exploration pmc template id and the mixed-case location
    /// mapping ride the views override since flip #7.
    fn varying() -> QuestVaryingRequest {
        serde_json::from_value(varying_value()).expect("fixture varying parses")
    }

    /// One location, as `quest.json`'s `locations` map spells it — key and target both raw.
    fn pool() -> QuestTypePool {
        serde_json::from_value(serde_json::json!({
            "types": ["Exploration", "Completion"],
            "pool": {
                "Exploration": { "locations": { LOCATION: [LOCATION] } },
                "Elimination": { "targets": {} },
                "Pickup": { "locations": {} }
            }
        }))
        .expect("pool parses")
    }

    /// The `specificExits.chance` roll at `:145` is 15% for the level 16-40 band, so both branches
    /// show up over enough seeds, and the two carry different extract-count bounds (`:165-170`).
    #[test]
    fn a_seeded_exploration_quest_asks_for_extracts_on_the_drawn_map() {
        let views = views();
        let varying = varying();
        let config = daily_config();
        let mut plain_counts = BTreeSet::new();
        let mut specific_counts = BTreeSet::new();

        for seed in 1..=120u64 {
            let mut ctx = QuestContext::new(&views, &varying);
            ctx.diagnostics = DiagSink::capture();
            let mut pool = pool();
            let _guard = TestSeedGuard::install(seed);
            let quest = super::generate(
                &mut ctx,
                "6193a720f8ee7e52e4290000",
                20,
                PRAPOR,
                &mut pool,
                &config,
            )
            .expect("a mapped location with drawable exits generates");

            // `:233` — the location id the map key resolves to, looked up un-lowercased (`:204`)
            assert_eq!(quest.quest.location.as_deref(), Some(LOCATION_ID));

            // `:152` — the drawn location leaves the pool, but the type stays a candidate
            assert!(
                pool.pool
                    .exploration
                    .locations
                    .as_ref()
                    .expect("locations")
                    .is_empty()
            );
            assert_eq!(pool.types, ["Exploration", "Completion"]);

            let condition = &quest
                .quest
                .conditions
                .available_for_finish
                .as_ref()
                .expect("AvailableForFinish")[0];
            let counter_conditions = condition
                .counter
                .as_ref()
                .expect("counter")
                .conditions
                .as_ref()
                .expect("counter conditions");

            // `:229` overwrites the template's two conditions with a fresh pair
            assert_eq!(counter_conditions[0].condition_type, "ExitStatus");
            assert_eq!(
                counter_conditions[0].status.as_deref(),
                Some(&["Survived".to_owned()][..])
            );
            assert_eq!(counter_conditions[1].condition_type, "Location");
            assert!(
                matches!(&counter_conditions[1].target, Some(ListOrT::List(target)) if target == &[LOCATION.to_owned()])
            );

            let extracts = condition.value.expect("extract count") as i32;
            assert!(counter_conditions.len() <= 3, "seed {seed}");
            match counter_conditions.get(2) {
                // `:282` appends the specific-extract condition, and only the two exits that pass
                // every filter can be drawn (`:278`)
                Some(exit) => {
                    assert_eq!(exit.condition_type, "ExitName", "seed {seed}");
                    let name = exit.exit_name.as_deref().expect("exit name");
                    assert!(DRAWABLE_EXITS.contains(&name), "seed {seed}: {name} drawn");
                    specific_counts.insert(extracts);
                }
                None => {
                    plain_counts.insert(extracts);
                }
            }
        }

        // `:165-170` — `RandInt(2, 7 + 1)` without a specific exit, `RandInt(1, 3 + 1)` with one.
        // The `+ 1` is what puts the upper bound in reach.
        assert_eq!(plain_counts, BTreeSet::from([2, 3, 4, 5, 6, 7]));
        assert_eq!(specific_counts, BTreeSet::from([1, 2, 3]));
    }

    /// `:130-136` — an exhausted location pool drops `Exploration` as a candidate type and gives up
    /// before any draw is spent.
    #[test]
    fn an_empty_location_pool_drops_the_quest_type() {
        let views = views();
        let varying = varying();
        let mut ctx = QuestContext::new(&views, &varying);
        ctx.diagnostics = DiagSink::capture();
        let mut pool = pool();
        pool.pool
            .exploration
            .locations
            .as_mut()
            .expect("locations")
            .clear();

        let _guard = TestSeedGuard::install(1);
        assert!(
            super::generate(
                &mut ctx,
                "6193a720f8ee7e52e4290000",
                20,
                PRAPOR,
                &mut pool,
                &daily_config(),
            )
            .is_none()
        );
        assert_eq!(pool.types, ["Completion"]);
        assert_eq!(
            ctx.diagnostics
                .captured()
                .iter()
                .map(|diagnostic| diagnostic.locale_key.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["repeatable-no_location_found_for_exploration_quest_generation"]
        );
    }
}
