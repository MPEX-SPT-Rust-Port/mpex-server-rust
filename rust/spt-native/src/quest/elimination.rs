//! `Generators/RepeatableQuests/EliminationQuestGenerator.cs` — the kill-N-of-X repeatable quest.
//!
//! Line references in this file are that generator unless another file is named.

use indexmap::IndexMap;

use crate::loot::math_util::map_to_range;
use crate::loot::models::{Diagnostic, ERROR, ItemView, WARNING};
use crate::loot::mongo_id;
use crate::loot::probability_object_array::{ProbabilityObject, ProbabilityObjectArray};
use crate::loot::random_util::{
    draw_random_from_list, get_array_value, get_chance_100, get_secure_random_number, rand_int,
};
use crate::quest::QuestContext;
use crate::quest::models::{
    BossInfo, CounterConditionDistance, DaytimeCounter, EliminationConfig, ListOrT,
    ProbabilityObjectWire, QuestConditionCounterCondition, QuestTypePool, RepeatableQuest,
    RepeatableQuestConfig, RepeatableQuestType, TargetLocation,
};
use crate::quest::{helper, reward_generator};

/// The `typeof(T).FullName` this file's diagnostics log under.
const CATEGORY: &str = "SPTarkov.Server.Core.Generators.RepeatableQuests.EliminationQuestGenerator";

/// `Models/Enums/Traders.cs:9`.
const FENCE: &str = "579dc571d53a0658a154fbec";

/// `Models/Common/MongoId.cs:334` — `default(MongoId)`, whose `ToString` is `string.Empty`.
const MONGO_ID_EMPTY: &str = "";

/// The melee weapon category `:675` refuses to pair a distance requirement with.
const MELEE_CATEGORY: &str = "5b5f7a0886f77409407a7f96";

/// `Constants/BodyPartContants.cs:3-16` through the `_bodyPartsToClient` map (`:40-46`) — body
/// parts as the client wants them, keyed by the name the config draws.
///
/// A slice rather than a map: four entries, looked up by a linear scan that costs less than the
/// hashing would.
const BODY_PARTS_TO_CLIENT: [(&str, &[&str]); 4] = [
    ("Arms", &["LeftArm", "RightArm"]),
    ("Legs", &["LeftLeg", "RightLeg"]),
    ("Head", &["Head"]),
    ("Chest", &["Chest", "Stomach"]),
];

/// `MaxDistDifficulty is defined by 2, this could be a tuning parameter if we don't like the reward
/// generation` (`:51`).
const MAX_DIST_DIFFICULTY: i32 = 2;

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

/// A plain interpolated log line (`:381`).
fn plain(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        category: CATEGORY,
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

/// `ItemHelper.GetItemTplsOfBaseType` (`ItemHelper.cs:1554-1556`) — every tpl whose `_parent` is
/// `desired_base_type`, in table order.
fn get_item_tpls_of_base_type<'a>(
    items: &'a IndexMap<String, ItemView>,
    desired_base_type: &str,
) -> Vec<&'a str> {
    items
        .iter()
        .filter(|(_, item)| item.parent.as_deref() == Some(desired_base_type))
        .map(|(tpl, _)| tpl.as_str())
        .collect()
}

/// `new ProbabilityObjectArray<K, V>(cloner, source)` (`:283-289`) — the config entries deep cloned
/// into a drawing pool.
fn probability_pool<V: Clone>(
    source: &[ProbabilityObjectWire<V>],
) -> ProbabilityObjectArray<String, V> {
    let mut pool = ProbabilityObjectArray::new();
    for entry in source {
        pool.add(ProbabilityObject {
            key: entry.key.clone(),
            relative_probability: entry.relative_probability,
            data: entry.data.clone(),
        });
    }

    pool
}

/// `protected record EliminationQuestGenerationData` (`:53-60`). The two config members are
/// borrowed rather than cloned — the C# record holds the same references.
struct EliminationQuestGenerationData<'a> {
    elimination_config: &'a EliminationConfig,
    locations_config: &'a IndexMap<String, Vec<String>>,
    targets_config: ProbabilityObjectArray<String, BossInfo>,
    body_parts_config: ProbabilityObjectArray<String, Vec<String>>,
    weapon_category_requirement_config: ProbabilityObjectArray<String, Vec<String>>,
    weapon_requirement_config: ProbabilityObjectArray<String, Vec<String>>,
}

/// `GetGenerationData` (`:271-299`).
fn get_generation_data<'a>(
    ctx: &mut QuestContext<'_>,
    repeatable_config: &'a RepeatableQuestConfig,
    pmc_level: i32,
) -> Option<EliminationQuestGenerationData<'a>> {
    let Some(elimination_config) =
        helper::get_elimination_config_by_pmc_level(pmc_level, repeatable_config)
    else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-elimination-config-not-found",
            None,
        ));

        return None;
    };

    let locations_config = &repeatable_config.locations;

    let targets_config = probability_pool(&elimination_config.targets);
    let body_parts_config = probability_pool(&elimination_config.body_parts);
    let weapon_category_requirement_config =
        probability_pool(&elimination_config.weapon_category_requirements);
    let weapon_requirement_config = probability_pool(&elimination_config.weapon_requirements);

    Some(EliminationQuestGenerationData {
        elimination_config,
        locations_config,
        targets_config,
        body_parts_config,
        weapon_category_requirement_config,
        weapon_requirement_config,
    })
}

/// `GetBotTypeToEliminate` (`:307-326`) — the filtered targets config and the bot type drawn from
/// it, or `None` once nothing but bosses is left, which also drops `Elimination` from the pool's
/// type list (`:324`).
fn get_bot_type_to_eliminate(
    generation_data: &EliminationQuestGenerationData<'_>,
    quest_type_pool: &mut QuestTypePool,
) -> Option<(String, ProbabilityObjectArray<String, BossInfo>)> {
    let target_pool = &quest_type_pool.pool.elimination;

    // `targetPool.Targets.ContainsKey` (`:314`) — the C# dereferences a null `Targets` here and
    // only null-checks it back at `:129`, so a null pool throws before that check can fire.
    let targets = target_pool
        .targets
        .as_ref()
        .expect("QuestTypePool.Pool.Elimination.Targets was null at EliminationQuestGenerator:314");
    let targets_config = generation_data
        .targets_config
        .filter(|entry| targets.contains_key(&entry.key));

    // `!targetsConfig.All(x => x.Data?.IsBoss ?? false)` (`:316`), spelled as "nothing that isn't a
    // boss survives a filter" so no new member is needed on the shared pool type. An empty config
    // makes `All` vacuously true, but the `Count != 0` in front of it already rules that out.
    let all_bosses = targets_config
        .filter(|entry| {
            !entry
                .data
                .as_ref()
                .and_then(|info| info.is_boss)
                .unwrap_or(false)
        })
        .is_empty();

    if !targets_config.is_empty() && !all_bosses {
        // `Draw()[0]` (`:318`) — `Draw` defaults to one key, and an empty result would throw here.
        let drawn = targets_config.draw(1);

        return Some((drawn[0].clone(), targets_config));
    }

    // There are no more targets left for elimination; delete it as a possible quest type
    // also if only bosses are left we need to leave otherwise it's a guaranteed boss elimination
    // -> then it would not be a quest with low probability anymore
    quest_type_pool
        .types
        .retain(|quest_type| quest_type != "Elimination");

    None
}

/// `TryGetLocationKey` (`:337-402`). Returns the chosen key, and mutates the pool's location list
/// for `bot_type_to_eliminate` (`:391`) — dropping the target wholesale once that list empties
/// (`:398`).
fn try_get_location_key(
    ctx: &mut QuestContext<'_>,
    generation_data: &EliminationQuestGenerationData<'_>,
    target_pool: &mut IndexMap<String, TargetLocation>,
    bot_type_to_eliminate: &str,
    locations: &[String],
) -> Option<String> {
    let use_specific_location = get_chance_100(f64::from(
        generation_data.elimination_config.specific_location_chance,
    ));

    if !use_specific_location {
        // We're not using a specific location, and the locations contain any.
        if locations.iter().any(|location| location == "any") {
            return Some("any".to_owned());
        }

        // We're not using a specific location and locations didn't contain any.
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-elimination-any-not-found",
            None,
        ));

        return None;
    }

    let mut locations = locations.to_vec();

    // Don't filter when there's less than 2 options
    if locations.len() > 1 {
        // Specific location
        locations.retain(|location| location != "any");
        if locations.is_empty() {
            // Never should reach this if everything works out
            ctx.diagnostics.push(localised(
                ERROR,
                "quest-repeatable_elimination_generation_failed_please_report",
                None,
            ));

            return None;
        }
    }

    // Get name of location we want elimination to occur on
    // `DrawRandomFromList(locations).First()` (`:376`) — one draw, with replacement.
    let location_key = draw_random_from_list(&locations, 1, true)
        .into_iter()
        .next()
        .expect("DrawRandomFromList drew nothing at EliminationQuestGenerator:376");

    // Get a pool of locations the chosen bot type can be eliminated on
    let Some(possible_location_pool) = target_pool.get_mut(bot_type_to_eliminate) else {
        ctx.diagnostics.push(plain(
            WARNING,
            format!("Bot to kill: {bot_type_to_eliminate} not found in elimination dict"),
        ));

        return None;
    };

    // Filter locations bot can be killed on to just those not chosen by key
    possible_location_pool.locations = possible_location_pool.locations.as_ref().map(|pool| {
        pool.iter()
            .filter(|location| **location != location_key)
            .cloned()
            .collect()
    });

    // None left after filtering
    if possible_location_pool
        .locations
        .as_ref()
        .is_none_or(Vec::is_empty)
    {
        // TODO: Why do any of this?!
        // Remove chosen bot to eliminate from pool
        target_pool.shift_remove(bot_type_to_eliminate);
    }

    Some(location_key)
}

/// `GenerateBodyParts` (`:410-441`) — the client-facing body parts and the difficulty they carry,
/// which the C# hands back through a `ref` parameter.
fn generate_body_parts(generation_data: &EliminationQuestGenerationData<'_>) -> (Vec<String>, f64) {
    // if we add a bodyPart condition, we draw randomly one or two parts
    // each bodyPart of the BODYPARTS ProbabilityObjectArray includes the string(s)
    // which need to be presented to the client in ProbabilityObjectArray.data
    // e.g. we draw "Arms" from the probability array but must present ["LeftArm", "RightArm"] to the client
    let mut body_parts_to_client = Vec::new();

    // Quirk 2 (`:418`): `RandInt(1, 3)`'s upper bound is exclusive, so this is one or two parts and
    // never the three the "one or two" comment above it implies. The `RandInt` is spent before
    // `DrawAndRemove` draws, so both the count and the order are load-bearing.
    let body_parts = generation_data
        .body_parts_config
        .draw_and_remove(rand_int(1, Some(3)) as usize, None);

    let mut probability = 0f64;

    for body_part in &body_parts {
        // more than one part lead to an "OR" condition hence more parts reduce the difficulty
        // `?? 0d` (`:425`) — a key the pool does not carry contributes nothing.
        probability += generation_data
            .body_parts_config
            .probability(body_part)
            .unwrap_or(0.0);

        // Add multiple body parts needed for key
        if let Some((_, body_part_list_to_client)) = BODY_PARTS_TO_CLIENT
            .iter()
            .find(|(key, _)| *key == body_part.as_str())
        {
            body_parts_to_client.extend(
                body_part_list_to_client
                    .iter()
                    .map(|part| (*part).to_owned()),
            );
            continue;
        }

        // Add singular body-part, e.g. head
        body_parts_to_client.push(body_part.clone());
    }

    (body_parts_to_client, 1.0 / probability)
}

/// `IsDistanceRequirementAllowed` (`:452-502`).
fn is_distance_requirement_allowed(
    ctx: &QuestContext<'_>,
    generation_data: &EliminationQuestGenerationData<'_>,
    bot_type_to_eliminate: &str,
    location_key: &str,
    targets_config: &ProbabilityObjectArray<String, BossInfo>,
) -> bool {
    // This location is can be chosen for a distance requirement
    let whitelisted = !generation_data
        .elimination_config
        .dist_location_blacklist
        .iter()
        .any(|blacklisted| blacklisted == location_key);

    // We're not whitelisted, exit early to avoid doing a roll for no reason
    if !whitelisted {
        return false;
    }

    // Are we allowed a distance condition by chance?
    let is_allowed_by_chance =
        get_chance_100(generation_data.elimination_config.distance_probability);

    // Not allowed by chance, return early.
    // We now just assume we rolled this condition and don't take it into account anymore.
    if !is_allowed_by_chance {
        return false;
    }

    // We're not a boss, return true if this location is whitelisted
    if !targets_config
        .data(&bot_type_to_eliminate.to_owned())
        .and_then(|info| info.is_boss)
        .unwrap_or(false)
    {
        return whitelisted;
    }

    // Get all boss spawn information, filter for the current boss to spawn on map, then remove
    // blacklisted locations (`:485-497`). The slice carries `BossName` per location id already, and
    // the ids are compared ordinally against `DistLocationBlacklist`, never lowercased.
    let allowed_spawns_any = ctx.boss_spawns_by_location.iter().any(|(id, boss_names)| {
        boss_names
            .iter()
            .any(|boss_name| boss_name == bot_type_to_eliminate)
            && !generation_data
                .elimination_config
                .dist_location_blacklist
                .iter()
                .any(|blacklisted| blacklisted == id)
    });

    // if the boss spawns on non-blacklisted locations and the current location is allowed,
    // we can generate a distance kill requirement
    whitelisted && allowed_spawns_any
}

/// `GenerateDistanceRequirement` (`:509-524`).
fn generate_distance_requirement(
    generation_data: &EliminationQuestGenerationData<'_>,
) -> (i32, i32) {
    let config = generation_data.elimination_config;

    // Random distance with lower values more likely; simple distribution for starters...
    // Two draws (`:514`); C# evaluates the left operand of the subtraction first.
    let first = get_secure_random_number();
    let second = get_secure_random_number();
    let distance = ((first - second).abs() * (1.0 + config.max_distance - config.min_distance)
        + config.min_distance)
        .floor() as i32;

    let distance = (f64::from(distance) / 5.0).ceil() as i32 * 5;

    let distance_difficulty =
        (f64::from(MAX_DIST_DIFFICULTY * distance) / config.max_distance) as i32;

    (distance, distance_difficulty)
}

/// `GenerateWeaponCategoryRequirement` (`:532-561`).
fn generate_weapon_category_requirement(
    generation_data: &mut EliminationQuestGenerationData<'_>,
    distance: Option<i32>,
) -> Option<String> {
    match distance {
        // Filter out close range weapons from far distance requirement
        Some(distance) if distance > 50 => {
            const WEAPON_TYPE_BLACKLIST: [&str; 2] = ["Shotgun", "Pistol"];

            // Filter out close range weapons from long distance requirement
            generation_data
                .weapon_category_requirement_config
                .remove_all(|category| WEAPON_TYPE_BLACKLIST.contains(&category.key.as_str()));
        }
        // Filter out long range weapons from close distance requirement
        Some(distance) if distance < 20 => {
            const WEAPON_TYPE_BLACKLIST: [&str; 2] = ["MarksmanRifle", "DMR"];

            // Filter out far range weapons from close distance requirement
            generation_data
                .weapon_category_requirement_config
                .remove_all(|category| WEAPON_TYPE_BLACKLIST.contains(&category.key.as_str()));
        }
        // A null distance matches neither relational pattern, so nothing is filtered.
        _ => {}
    }

    // Pick a weighted weapon category
    let weapon_requirement = generation_data
        .weapon_category_requirement_config
        .draw_and_remove(1, None);

    // Get the hideout id value stored in the .data array
    // `weaponRequirement[0]` (`:560`) throws on an emptied pool, as does the `[0]` on the data.
    generation_data
        .weapon_category_requirement_config
        .data(&weapon_requirement[0])
        .map(|category| category[0].clone())
}

/// `GenerateSpecificWeaponRequirement` (`:568-582`).
fn generate_specific_weapon_requirement(
    ctx: &mut QuestContext<'_>,
    generation_data: &EliminationQuestGenerationData<'_>,
) -> String {
    let weapon_requirement = generation_data
        .weapon_requirement_config
        .draw_and_remove(1, None);
    let specific_allowed_weapon_category = generation_data
        .weapon_requirement_config
        .data(&weapon_requirement[0]);

    // `specificAllowedWeaponCategory?[0] is null` (`:573`) — an absent entry, and a `[0]` that
    // throws on an entry present but empty.
    let Some(category) = specific_allowed_weapon_category.map(|category| category[0].clone())
    else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-elimination-specific-weapon-null",
            None,
        ));

        return MONGO_ID_EMPTY.to_owned();
    };

    let allowed_weapons = get_item_tpls_of_base_type(ctx.items, &category);

    // `GetArrayValue` spends a draw only when the list holds more than one element
    // (`RandomUtil.cs:174` through `GetInt`), and throws on an empty one.
    (*get_array_value(&allowed_weapons)).to_owned()
}

/// `GetEliminationKillCount` (`:591-608`) — three branches over three sets of bounds, each its own
/// draw. Never merged: the bounds differ, so the drawn value would too.
fn get_elimination_kill_count(
    target_key: &str,
    targets_config: &ProbabilityObjectArray<String, BossInfo>,
    elimination_config: &EliminationConfig,
) -> i32 {
    let data = targets_config.data(&target_key.to_owned());

    if data.and_then(|info| info.is_boss).unwrap_or(false) {
        return rand_int(
            i64::from(elimination_config.min_boss_kills),
            Some(i64::from(elimination_config.max_boss_kills) + 1),
        ) as i32;
    }

    if data.and_then(|info| info.is_pmc).unwrap_or(false) {
        return rand_int(
            i64::from(elimination_config.min_pmc_kills),
            Some(i64::from(elimination_config.max_pmc_kills) + 1),
        ) as i32;
    }

    rand_int(
        i64::from(elimination_config.min_kills),
        Some(i64::from(elimination_config.max_kills) + 1),
    ) as i32
}

/// `DifficultyWeighing` (`:610-613`). `dist` and `kill` are `int` in the C#, so both call-site
/// quotients (`:210`/`:211`) truncate before they arrive here.
fn difficulty_weighing(
    target: f64,
    body_part: f64,
    dist: i32,
    kill: i32,
    weapon_requirement: i32,
) -> f64 {
    (target.sqrt() + body_part + f64::from(dist) + f64::from(weapon_requirement)).sqrt()
        * f64::from(kill)
}

/// `GenerateEliminationLocation` (`:622-631`).
fn generate_elimination_location(location: &[String]) -> QuestConditionCounterCondition {
    QuestConditionCounterCondition {
        id: Some(mongo_id::generate()),
        dynamic_locale: Some(true),
        target: Some(ListOrT::List(location.to_vec())),
        condition_type: "Location".to_owned(),
        ..Default::default()
    }
}

/// `GenerateEliminationCondition` (`:642-694`).
///
/// `targeted_body_parts` is not optional here: the only call site (`:252`) hands over a list that
/// was initialised at `:155` and is never null, so the `is not null` guard at `:669` always passes
/// and an empty body-part list is written out as an empty array.
fn generate_elimination_condition(
    target: &str,
    targeted_body_parts: &[String],
    distance: Option<f64>,
    allowed_weapon: Option<&str>,
    allowed_weapon_category: Option<&str>,
) -> QuestConditionCounterCondition {
    let mut kill_condition_props = QuestConditionCounterCondition {
        id: Some(mongo_id::generate()),
        dynamic_locale: Some(true),
        target: Some(ListOrT::Item(target.to_owned())), // e,g, "AnyPmc"
        value: Some(serde_json::json!(1)),
        reset_on_session_end: Some(false),
        enemy_health_effects: Some(Vec::new()),
        daytime: Some(DaytimeCounter {
            from: Some(0),
            to: Some(0),
            ..Default::default()
        }),
        condition_type: "Kills".to_owned(),
        ..Default::default()
    };

    if target.starts_with("boss") {
        kill_condition_props.target = Some(ListOrT::Item("Savage".to_owned()));
        kill_condition_props.savage_role = Some(vec![target.to_owned()]);
    }

    // Has specific body part hit condition
    kill_condition_props.body_part = Some(targeted_body_parts.to_vec());

    // Don't allow distance + melee requirement
    if let Some(distance) = distance
        && allowed_weapon_category != Some(MELEE_CATEGORY)
    {
        kill_condition_props.distance = Some(CounterConditionDistance {
            value: Some(distance),
            compare_method: Some(">=".to_owned()),
            ..Default::default()
        });
    }

    // Has specific weapon requirement
    if let Some(allowed_weapon) = allowed_weapon {
        kill_condition_props.weapon = Some(vec![allowed_weapon.to_owned()]);
    }

    // Quirk 1: the weapon *category* requirement never reaches the client. `:687-691` reads
    //
    //     if (allowedWeaponCategory?.Length > 0)
    //     {
    //         // TODO - fix - does weaponCategories exist?
    //         // killConditionProps.weaponCategories = [allowedWeaponCategory];
    //     }
    //
    // — an empty body around a commented-out assignment. The category is still rolled (`:180`),
    // still drawn (`:557`), still suppresses the specific-weapon requirement (`:194`) and still
    // feeds the difficulty term (`:212`), so every one of those draws is load-bearing for RNG
    // parity even though the result is discarded right here. Ported as the same dead end; the
    // `allowed_weapon_category` parameter is read by the distance guard above and nothing else.

    kill_condition_props
}

/// `Generate` (`:74-269`) — a randomised Elimination quest, or `None` on any of the give-up paths.
///
/// Pool mutations land on `quest_type_pool`: `:324` drops the quest type, `:391` narrows the
/// target's location list and `:398` drops the target.
pub fn generate(
    ctx: &mut QuestContext<'_>,
    session_id: &str,
    pmc_level: i32,
    trader_id: &str,
    quest_type_pool: &mut QuestTypePool,
    repeatable_config: &RepeatableQuestConfig,
) -> Option<RepeatableQuest> {
    let Some(mut generation_data) = get_generation_data(ctx, repeatable_config, pmc_level) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-eliminationQuestGenerationData-is-null",
            None,
        ));

        return None;
    };

    // the difficulty of the quest varies in difficulty depending on the condition
    // possible conditions are
    // - amount of npcs to kill
    // - type of npc to kill (scav, boss, pmc)
    // - with hit to what body part they should be killed
    // - from what distance they should be killed
    // a random combination of listed conditions can be required
    // possible conditions elements and their relative probability can be defined in QuestConfig.js
    // We use ProbabilityObjectArray to draw by relative probability. e.g. for targets:
    // "targets": {
    //    "Savage": 7,
    //    "AnyPmc": 2,
    //    "bossBully": 0.5
    // }
    // higher is more likely. We define the difficulty to be the inverse of the relative probability.

    // We want to generate a reward which is scaled by the difficulty of this mission. To get an
    // upper bound with which we scale the actual difficulty we calculate the minimum and maximum
    // difficulty (max being the sum of max of each condition type times the number of kills we have
    // to perform):

    // The minimum difficulty is the difficulty for the most probable (= easiest target) with no additional conditions
    // These three read the *unfiltered* targets/body-parts configs, unlike `target_difficulty`.
    let min_difficulty = 1.0 / generation_data.targets_config.max_probability(); // min difficulty is the lowest amount of scavs without any constraints

    // Target on bodyPart max. difficulty is that of the least probable element
    let max_target_difficulty = 1.0 / generation_data.targets_config.min_probability();
    let max_body_parts_difficulty = f64::from(generation_data.elimination_config.min_kills)
        / generation_data.body_parts_config.min_probability();

    let max_kill_difficulty = generation_data.elimination_config.max_kills;

    // Get a random bot type to eliminate
    let Some((bot_type_to_eliminate, targets_config)) =
        get_bot_type_to_eliminate(&generation_data, quest_type_pool)
    else {
        ctx.diagnostics
            .push(localised(WARNING, "repeatable-no-bot-types-remain", None));

        return None;
    };

    // `1 / Probability(key)` is a lifted division (`:127`): null in, null out, and the `.Value` at
    // `:208` is what throws.
    let target_difficulty = targets_config
        .probability(&bot_type_to_eliminate)
        .map(|probability| 1.0 / probability);

    if quest_type_pool.pool.elimination.targets.is_none() {
        ctx.diagnostics
            .push(localised(ERROR, "repeatable-unable-targets-are-null", None));

        return None;
    }

    // Try and get a target location pool for this bot type
    let target_pool_targets = quest_type_pool
        .pool
        .elimination
        .targets
        .as_mut()
        .expect("checked above");
    let Some(target_location_pool) = target_pool_targets.get(&bot_type_to_eliminate) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-unable-get-target-pool",
            Some(serde_json::json!(bot_type_to_eliminate)),
        ));

        return None;
    };

    // `targetLocationPool.Locations` is `List<string>?` and `:145` passes it straight in, so a null
    // one throws inside `TryGetLocationKey`.
    let locations = target_location_pool
        .locations
        .clone()
        .expect("EliminationPool target locations were null at EliminationQuestGenerator:145");

    // Try and get a location key for this quest
    let Some(location_key) = try_get_location_key(
        ctx,
        &generation_data,
        target_pool_targets,
        &bot_type_to_eliminate,
        &locations,
    ) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-unable-get-location-key",
            Some(serde_json::json!(bot_type_to_eliminate)),
        ));

        return None;
    };

    // Generate a body part, make sure we ref the body part difficulty so it can be adjusted
    let mut body_parts_to_client: Vec<String> = Vec::new();
    let mut body_part_difficulty = 0f64;
    let generate_body_parts_roll = get_chance_100(f64::from(
        generation_data.elimination_config.body_part_chance,
    ));
    if generate_body_parts_roll {
        // draw the target body part and calculate the difficulty factor
        let (parts, difficulty) = generate_body_parts(&generation_data);
        body_parts_to_client.extend(parts);
        body_part_difficulty = difficulty;
    }

    // Draw a distance condition
    let is_distance_requirement_allowed = is_distance_requirement_allowed(
        ctx,
        &generation_data,
        &bot_type_to_eliminate,
        &location_key,
        &targets_config,
    );

    let mut distance: Option<i32> = None;
    let mut distance_difficulty = 0;

    // Generate a distance requirement
    if is_distance_requirement_allowed {
        let (dist, dist_diff) = generate_distance_requirement(&generation_data);
        distance = Some(dist);
        distance_difficulty = dist_diff;
    }

    let mut allowed_weapons_category: Option<String> = None;

    let generate_weapon_category_requirement_roll = get_chance_100(f64::from(
        generation_data
            .elimination_config
            .weapon_category_requirement_chance,
    ));

    // Generate a weapon category requirement
    if generate_weapon_category_requirement_roll {
        allowed_weapons_category =
            generate_weapon_category_requirement(&mut generation_data, distance);
    }

    // Only allow a specific weapon requirement if a weapon category was not chosen
    let mut allowed_weapon: Option<String> = None;

    let generate_weapon_requirement_roll = get_chance_100(f64::from(
        generation_data.elimination_config.weapon_requirement_chance,
    ));

    // Generate a weapon requirement
    if !generate_weapon_category_requirement_roll && generate_weapon_requirement_roll {
        // The C# assigns a non-nullable `MongoId` into a `MongoId?`, so even the error path's
        // `MongoId.Empty()` reads as "a weapon was required" at `:212` and `:681`.
        allowed_weapon = Some(generate_specific_weapon_requirement(ctx, &generation_data));
    }

    // Draw how many npm kills are required
    let desired_kill_count = get_elimination_kill_count(
        &bot_type_to_eliminate,
        &targets_config,
        generation_data.elimination_config,
    );

    let kill_difficulty = desired_kill_count;

    // not perfectly happy here; we give difficulty = 1 to the quest reward generation when we have the most difficult mission
    // e.g. killing reshala 5 times from a distance of 200m with a headshot.
    let max_difficulty = difficulty_weighing(1.0, 1.0, 1, 1, 1);
    let cur_difficulty = difficulty_weighing(
        target_difficulty.expect("Probability was null at EliminationQuestGenerator:208")
            / max_target_difficulty,
        body_part_difficulty / max_body_parts_difficulty,
        // Both of these are `int / int` in the C# and truncate towards zero before the call, so a
        // kill count below `MaxKills` zeroes the whole weighing.
        distance_difficulty / MAX_DIST_DIFFICULTY,
        kill_difficulty / max_kill_difficulty,
        i32::from(allowed_weapons_category.is_some() || allowed_weapon.is_some()),
    );

    // Aforementioned issue makes it a bit crazy since now all easier quests give significantly lower rewards than Completion / Exploration
    // I therefore moved the mapping a bit up (from 0.2...1 to 0.5...2) so that normal difficulty still gives good reward and having the
    // crazy maximum difficulty will lead to a higher difficulty reward gain factor than 1
    let difficulty = map_to_range(cur_difficulty, min_difficulty, max_difficulty, 0.5, 2.0);

    let Some(mut quest) = helper::generate_repeatable_template(
        ctx,
        RepeatableQuestType::Elimination,
        trader_id,
        &repeatable_config.side,
        session_id,
    ) else {
        ctx.diagnostics.push(localised(
            ERROR,
            "repeatable-quest_generation_failed_no_template",
            Some(serde_json::json!("elimination")),
        ));

        return None;
    };

    // ASSUMPTION: All fence quests are for scavs
    if trader_id == FENCE {
        quest.quest.side = "Scav".to_owned();
    }

    let available_for_finish_condition = &mut quest
        .quest
        .conditions
        .available_for_finish
        .as_mut()
        .expect("AvailableForFinish was null at EliminationQuestGenerator:240")[0];
    let counter = available_for_finish_condition
        .counter
        .as_mut()
        .expect("Counter was null at EliminationQuestGenerator:241");
    counter.id = Some(mongo_id::generate());
    counter.conditions = Some(Vec::new());
    let counter_conditions = counter.conditions.as_mut().expect("just assigned");

    // Only add specific location condition if specific map selected
    if location_key != "any" {
        // `Enum.Parse<ELocationName>` (`:247`) throws on an unknown key and the indexer at `:248`
        // throws on a key the config carries no locations for — both are panics here.
        let location = generation_data
            .locations_config
            .get(&location_key)
            .unwrap_or_else(|| {
                panic!(
                    "no locations configured for {location_key} at EliminationQuestGenerator:248"
                )
            });
        counter_conditions.push(generate_elimination_location(location));
    }

    counter_conditions.push(generate_elimination_condition(
        &bot_type_to_eliminate,
        &body_parts_to_client,
        distance.map(f64::from),
        allowed_weapon.as_deref(),
        allowed_weapons_category.as_deref(),
    ));
    available_for_finish_condition.value = Some(f64::from(desired_kill_count));
    available_for_finish_condition.id = mongo_id::generate();

    // Get the quest location, default to any if none exist
    quest.quest.location = Some(
        helper::get_quest_location_by_map_id(ctx, &location_key)
            .unwrap_or("any")
            .to_owned(),
    );

    quest.quest.rewards = reward_generator::generate_reward(
        ctx,
        pmc_level,
        f64::min(difficulty, 1.0),
        trader_id,
        repeatable_config,
        &generation_data.elimination_config.possible_skill_rewards,
        None,
    );

    Some(quest)
}

#[cfg(test)]
mod tests {
    use crate::diag::DiagSink;
    use crate::loot::random_util::TestSeedGuard;
    use crate::quest::QuestContext;
    use crate::quest::helper::PRAPOR;
    use crate::quest::models::{ListOrT, QuestTypePool, RepeatableQuestConfig, tests::slice};

    const QUEST_CONFIG_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/configs/quest.json"
    );

    /// The shipped Daily config with the two rolls this test wants pinned forced to certainties:
    /// `bodyPartChance` to 100 so `:157` always takes the body-part branch, and
    /// `specificLocationChance` to 100 so `:345` always takes the filtering branch that mutates the
    /// pool. Every other roll is left as shipped.
    fn daily_config() -> RepeatableQuestConfig {
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(QUEST_CONFIG_PATH).expect("readable"))
                .expect("JSON");
        let mut daily = config["repeatableQuests"][0].clone();

        for band in daily["questConfig"]["Elimination"]
            .as_array_mut()
            .expect("elimination bands")
        {
            band["bodyPartChance"] = serde_json::json!(100);
            band["specificLocationChance"] = serde_json::json!(100);
        }

        serde_json::from_value(daily).expect("parses")
    }

    /// One savage target over `any` plus `bigmap`, which is the fixture slice's only mapped
    /// location key.
    fn pool() -> QuestTypePool {
        serde_json::from_value(serde_json::json!({
            "types": ["Elimination", "Completion"],
            "pool": {
                "Exploration": { "locations": {} },
                "Elimination": { "targets": { "Savage": { "locations": ["any", "bigmap"] } } },
                "Pickup": { "locations": {} }
            }
        }))
        .expect("pool parses")
    }

    /// The four `_bodyPartsToClient` groups (`:40-46`) — a drawn part expands to one of these.
    const BODY_PART_GROUPS: [&[&str]; 4] = [
        &["Head"],
        &["Chest", "Stomach"],
        &["LeftArm", "RightArm"],
        &["LeftLeg", "RightLeg"],
    ];

    #[test]
    fn a_seeded_elimination_quest_draws_one_or_two_body_parts_and_empties_the_target_pool() {
        let slice = slice();
        let mut ctx = QuestContext::from_slice(&slice);
        ctx.diagnostics = DiagSink::capture();
        let config = daily_config();
        let mut pool = pool();

        let _guard = TestSeedGuard::install(42);
        let quest = super::generate(
            &mut ctx,
            "6193a720f8ee7e52e4290000",
            20,
            PRAPOR,
            &mut pool,
            &config,
        )
        .expect("a savage target over a mapped location generates");

        // `:258` — `bigmap` is the only key the fixture's `locationIdMap` carries
        assert_eq!(
            quest.quest.location.as_deref(),
            Some("55f2d3fd4bdc2d5f408b4567")
        );

        let condition = &quest
            .quest
            .conditions
            .available_for_finish
            .as_ref()
            .unwrap()[0];
        let counter_conditions = condition
            .counter
            .as_ref()
            .unwrap()
            .conditions
            .as_ref()
            .unwrap();

        // `:248` location sub-condition, then `:252` the kill sub-condition
        assert_eq!(counter_conditions.len(), 2);
        assert_eq!(counter_conditions[0].condition_type, "Location");
        let kill = &counter_conditions[1];
        assert_eq!(kill.condition_type, "Kills");
        assert!(matches!(&kill.target, Some(ListOrT::Item(target)) if target == "Savage"));

        // `:418` draws `RandInt(1, 3)` parts — one or two, never three (quirk 2) — and each expands
        // to its `_bodyPartsToClient` group
        let body_parts = kill.body_part.as_ref().expect("body part condition");
        let groups: Vec<&[&str]> = BODY_PART_GROUPS
            .iter()
            .copied()
            .filter(|group| {
                group
                    .iter()
                    .all(|part| body_parts.contains(&(*part).to_string()))
            })
            .collect();
        assert!(
            (1..=2).contains(&groups.len()),
            "expected one or two body part groups, got {body_parts:?}"
        );
        assert_eq!(
            body_parts.len(),
            groups.iter().map(|group| group.len()).sum::<usize>()
        );

        // `:607` — level 20 lands in the Daily band asking `RandInt(5, 15 + 1)` kills
        let kills = condition.value.expect("kill count");
        assert!((5.0..=15.0).contains(&kills), "{kills} kills");

        // `:391` narrows the *pool's* location list — the one that still holds `any`, not the
        // `any`-stripped local copy `:364` drew from — so one non-`any` location is not enough to
        // empty it and the target survives this pass.
        let targets = pool.pool.elimination.targets.as_ref().unwrap();
        assert_eq!(
            targets["Savage"].locations.as_deref(),
            Some(&["any".to_owned()][..])
        );

        // Quirk 3 (`:394-399`): a second pass draws the `any` that is left, the filter empties the
        // list and the bot type is dropped from the pool wholesale.
        let _guard = TestSeedGuard::resume(42);
        super::generate(
            &mut ctx,
            "6193a720f8ee7e52e4290000",
            20,
            PRAPOR,
            &mut pool,
            &config,
        )
        .expect("the `any` location still generates");
        assert!(
            !pool
                .pool
                .elimination
                .targets
                .as_ref()
                .unwrap()
                .contains_key("Savage")
        );

        // `:324` only fires when no target remains at draw time, so `Elimination` is still a type
        assert_eq!(pool.types, ["Elimination", "Completion"]);
    }

    /// Quirk 1 (`:687-691`): a rolled weapon *category* is drawn, suppresses the specific-weapon
    /// requirement at `:194` and feeds the difficulty term at `:212` — and then never reaches the
    /// client, because the only line that would write it is commented out. Both chances are forced
    /// on here, so a quest that carried the category would have to show one of them.
    #[test]
    fn a_rolled_weapon_category_suppresses_the_weapon_requirement_and_then_vanishes() {
        let mut daily: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(QUEST_CONFIG_PATH).expect("readable"))
                .map(|config: serde_json::Value| config["repeatableQuests"][0].clone())
                .expect("JSON");
        for band in daily["questConfig"]["Elimination"]
            .as_array_mut()
            .expect("elimination bands")
        {
            band["weaponCategoryRequirementChance"] = serde_json::json!(100);
            band["weaponRequirementChance"] = serde_json::json!(100);
        }
        let config: RepeatableQuestConfig = serde_json::from_value(daily).expect("parses");

        let slice = slice();
        let mut ctx = QuestContext::from_slice(&slice);
        ctx.diagnostics = DiagSink::capture();
        let mut pool = pool();

        let _guard = TestSeedGuard::install(7);
        let quest = super::generate(
            &mut ctx,
            "6193a720f8ee7e52e4290000",
            20,
            PRAPOR,
            &mut pool,
            &config,
        )
        .expect("generates");

        let condition = &quest
            .quest
            .conditions
            .available_for_finish
            .as_ref()
            .unwrap()[0];
        let counter_conditions = condition
            .counter
            .as_ref()
            .unwrap()
            .conditions
            .as_ref()
            .unwrap();
        let kill = counter_conditions.last().expect("kill condition");

        // `:194` — the category won the roll, so no specific weapon was ever drawn
        assert_eq!(kill.weapon, None);
        // `:687-691` — and the category itself is written nowhere
        let written = serde_json::to_value(kill).expect("serializes");
        for key in written.as_object().expect("object").keys() {
            assert!(!key.to_lowercase().contains("category"), "{key} leaked");
        }
    }
}
