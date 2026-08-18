//! Phase 1 quest flip — equivalence of the natively-derived quest views against the
//! C#-built views-override slice over the full real database.
//!
//! Run by hand, after `QuestViewsEquivalenceTests` has written both fixture files:
//!   cargo test --release --test phase1_quest_views -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::{Value, json};
use spt_native::db::models::PublishRequest;
use spt_native::quest::models::{ExitView, LevelledItemFilter};
use spt_native::quest::views;
use spt_native::ragfair::views as ragfair_views;

fn fixture_path(env_var: &str, file_name: &str) -> PathBuf {
    std::env::var_os(env_var)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(file_name))
}

/// C#'s System.Text.Json and serde_json disagree on number form (100 vs 100.0) and on
/// null-vs-absent (WhenWritingNull omits members Rust may serialize as null). Compare
/// semantically and report the JSON path of the first divergence.
fn assert_json_equivalent(expected: &Value, actual: &Value, path: &str) {
    match (expected, actual) {
        (Value::Number(expected), Value::Number(actual)) => {
            let (expected, actual) = (
                expected.as_f64().expect("expected number fits f64"),
                actual.as_f64().expect("actual number fits f64"),
            );
            assert!(expected == actual, "{path}: {expected} != {actual}");
        }
        (Value::Object(expected), Value::Object(actual)) => {
            let keys: std::collections::BTreeSet<_> =
                expected.keys().chain(actual.keys()).collect();
            for key in keys {
                let null = Value::Null;
                let expected_value = expected.get(key).unwrap_or(&null);
                let actual_value = actual.get(key).unwrap_or(&null);
                assert_json_equivalent(expected_value, actual_value, &format!("{path}.{key}"));
            }
            // Key ORDER of the order-bearing maps is asserted separately at the top level,
            // per field, where the field is order-bearing.
        }
        (Value::Array(expected), Value::Array(actual)) => {
            assert_eq!(expected.len(), actual.len(), "{path}: array length");
            for (index, (expected_value, actual_value)) in expected.iter().zip(actual).enumerate() {
                assert_json_equivalent(expected_value, actual_value, &format!("{path}[{index}]"));
            }
        }
        (expected, actual) => assert_eq!(expected, actual, "{path}"),
    }
}

/// `LevelledItemFilter` deliberately carries no `Serialize` (`item_ids` is a membership-only
/// `HashSet`), so the wire JSON is built by hand — `itemIds` sorted, because neither side's
/// hash-set order is semantic (C# holds a `HashSet<MongoId>` too). [`sort_item_ids`] applies the
/// same canonicalization to the expected side.
fn filters_to_value(filters: &[LevelledItemFilter]) -> Value {
    Value::Array(
        filters
            .iter()
            .map(|filter| {
                let mut item_ids: Vec<&String> = filter.item_ids.iter().collect();
                item_ids.sort();
                json!({"minPlayerLevel": filter.min_player_level, "itemIds": item_ids})
            })
            .collect(),
    )
}

/// Sort the `itemIds` arrays of the expected completion filters in place — the set-typed twin of
/// [`filters_to_value`]'s sort, so the comparison is order-free for exactly that one field.
fn sort_item_ids(filters: &mut Value) {
    for filter in filters
        .as_array_mut()
        .expect("completion filters are an array")
    {
        if let Some(Value::Array(ids)) = filter.get_mut("itemIds") {
            ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        }
    }
}

/// `ExitView` carries no `Serialize` either; list order inside each location is load-bearing
/// (the exit is drawn by index) and crosses as-is.
fn extracts_to_value(extracts: &IndexMap<String, Vec<ExitView>>) -> Value {
    Value::Object(
        extracts
            .iter()
            .map(|(location, exits)| {
                let exits = exits
                    .iter()
                    .map(|exit| {
                        json!({
                            "name": exit.name,
                            "side": exit.side,
                            "chance": exit.chance,
                            "passageRequirement": exit.passage_requirement,
                        })
                    })
                    .collect();
                (location.clone(), Value::Array(exits))
            })
            .collect(),
    )
}

#[test]
#[ignore = "phase 1 quest flip — needs the fixture files from QuestViewsEquivalenceTests"]
fn derived_views_match_the_csharp_built_slice() {
    let roots_path = fixture_path("SPT_PHASE1_QUEST_ROOTS", "spt-phase1-quest-roots.json");
    let views_path = fixture_path(
        "SPT_PHASE1_QUEST_VIEWS",
        "spt-phase1-quest-views-expected.json",
    );

    let roots_bytes = std::fs::read(&roots_path).unwrap_or_else(|error| {
        panic!(
            "run QuestViewsEquivalenceTests first — {}: {error}",
            roots_path.display()
        )
    });
    let expected_bytes = std::fs::read(&views_path).unwrap_or_else(|error| {
        panic!(
            "run QuestViewsEquivalenceTests first — {}: {error}",
            views_path.display()
        )
    });

    let request: PublishRequest =
        serde_json::from_slice(&roots_bytes).expect("roots envelope parses into the typed roots");
    let mut expected: Value =
        serde_json::from_slice(&expected_bytes).expect("expected views parse");

    let templates = request
        .roots
        .templates
        .expect("envelope has a templates root");
    let traders = request.roots.traders.expect("envelope has a traders root");
    let globals = request.roots.globals.expect("envelope has a globals root");
    let locations = request
        .roots
        .locations
        .expect("envelope has a locations root");

    // The publish path's chaining (db.rs): ragfair derives first, quest consumes it by Arc
    let ragfair = Arc::new(
        ragfair_views::derive(&templates, &traders, &globals).expect("ragfair derive succeeds"),
    );
    let derived =
        views::derive(&templates, &globals, &locations, &ragfair).expect("quest derive succeeds");

    // The two set-typed fields compare order-free: canonicalize the expected side the way
    // filters_to_value canonicalizes the actual one
    for name in ["completionItemsWhitelist", "completionItemsBlacklist"] {
        sort_item_ids(
            expected
                .get_mut(name)
                .unwrap_or_else(|| panic!("expected file lacks '{name}'")),
        );
    }

    let actual = [
        (
            "items",
            serde_json::to_value(&derived.ragfair.items).unwrap(),
        ),
        (
            "handbookPrices",
            serde_json::to_value(&derived.ragfair.handbook_prices).unwrap(),
        ),
        (
            "fleaPrices",
            serde_json::to_value(&derived.ragfair.flea_prices).unwrap(),
        ),
        (
            "defaultWeaponPresets",
            serde_json::to_value(&derived.default_weapon_presets).unwrap(),
        ),
        (
            "defaultPresetOrItemPrices",
            serde_json::to_value(&derived.default_preset_or_item_prices).unwrap(),
        ),
        (
            // RepeatableTemplates carries no Serialize; its wire names are the C# member names
            "repeatableQuestTemplates",
            json!({
                "Elimination": &derived.repeatable_quest_templates.elimination,
                "Completion": &derived.repeatable_quest_templates.completion,
                "Exploration": &derived.repeatable_quest_templates.exploration,
                "Pickup": &derived.repeatable_quest_templates.pickup,
            }),
        ),
        (
            "completionItemsWhitelist",
            filters_to_value(&derived.completion_items_whitelist),
        ),
        (
            "completionItemsBlacklist",
            filters_to_value(&derived.completion_items_blacklist),
        ),
        (
            "bossSpawnsByLocation",
            serde_json::to_value(&derived.boss_spawns_by_location).unwrap(),
        ),
        (
            "extractsByLocation",
            extracts_to_value(&derived.extracts_by_location),
        ),
    ];

    assert_eq!(
        expected
            .as_object()
            .expect("expected file is an object")
            .len(),
        actual.len(),
        "expected file carries exactly the ten views"
    );

    for (name, actual_value) in &actual {
        let expected_value = expected
            .get(name)
            .unwrap_or_else(|| panic!("expected file lacks '{name}'"));
        assert_json_equivalent(expected_value, actual_value, name);
        println!("{name}: equivalent ✓");
    }

    // Insertion order is contract for the maps the generators walk whole
    // (RepeatableQuestRewardGenerator.GetRewardableItems, CompletionQuestGenerator.
    // GetItemsToRetrievePool). defaultWeaponPresets is an array, order-asserted above; the two
    // location maps are lookup-only and deliberately NOT order-asserted.
    let expected_keys: Vec<&String> = expected["items"].as_object().unwrap().keys().collect();
    let actual_items = &actual
        .iter()
        .find(|(actual_name, _)| *actual_name == "items")
        .unwrap()
        .1;
    let actual_keys: Vec<&String> = actual_items.as_object().unwrap().keys().collect();
    assert_eq!(expected_keys, actual_keys, "items: key order");
    println!("items: key order preserved ✓ ({} keys)", actual_keys.len());
}
