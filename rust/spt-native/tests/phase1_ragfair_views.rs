//! Phase 1 ragfair flip — equivalence of the natively-derived ragfair views against the
//! C#-built invariant-slice views over the full real database.
//!
//! Run by hand, after `RagfairViewsEquivalenceTests` has written both fixture files:
//!   cargo test --release --test phase1_ragfair_views -- --ignored --nocapture

use std::path::PathBuf;

use serde_json::Value;
use spt_native::db::models::PublishRequest;
use spt_native::ragfair::views;

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

#[test]
#[ignore = "phase 1 ragfair flip — needs the fixture files from RagfairViewsEquivalenceTests"]
fn derived_views_match_the_csharp_built_slice() {
    let roots_path = fixture_path("SPT_PHASE1_RAGFAIR_ROOTS", "spt-phase1-ragfair-roots.json");
    let views_path = fixture_path(
        "SPT_PHASE1_RAGFAIR_VIEWS",
        "spt-phase1-ragfair-views-expected.json",
    );

    let roots_bytes = std::fs::read(&roots_path).unwrap_or_else(|error| {
        panic!(
            "run RagfairViewsEquivalenceTests first — {}: {error}",
            roots_path.display()
        )
    });
    let expected_bytes = std::fs::read(&views_path).unwrap_or_else(|error| {
        panic!(
            "run RagfairViewsEquivalenceTests first — {}: {error}",
            views_path.display()
        )
    });

    let request: PublishRequest =
        serde_json::from_slice(&roots_bytes).expect("roots envelope parses into the typed roots");
    let expected: Value = serde_json::from_slice(&expected_bytes).expect("expected views parse");

    let templates = request
        .roots
        .templates
        .expect("envelope has a templates root");
    let traders = request.roots.traders.expect("envelope has a traders root");
    let globals = request.roots.globals.expect("envelope has a globals root");

    let derived = views::derive(&templates, &traders, &globals).expect("derive succeeds");

    let actual = [
        ("items", serde_json::to_value(&derived.items).unwrap()),
        (
            "itemPresets",
            serde_json::to_value(&derived.item_presets).unwrap(),
        ),
        (
            "defaultPresets",
            serde_json::to_value(&derived.default_presets).unwrap(),
        ),
        (
            "defaultPresetsByTpl",
            serde_json::to_value(&derived.default_presets_by_tpl).unwrap(),
        ),
        (
            "presetsByTpl",
            serde_json::to_value(&derived.presets_by_tpl).unwrap(),
        ),
        (
            "fleaPrices",
            serde_json::to_value(&derived.flea_prices).unwrap(),
        ),
        (
            "handbookPrices",
            serde_json::to_value(&derived.handbook_prices).unwrap(),
        ),
        (
            "highestTraderPrices",
            serde_json::to_value(&derived.highest_trader_prices).unwrap(),
        ),
    ];

    assert_eq!(
        expected
            .as_object()
            .expect("expected file is an object")
            .len(),
        actual.len(),
        "expected file carries exactly the eight views"
    );

    for (name, actual_value) in &actual {
        let expected_value = expected
            .get(name)
            .unwrap_or_else(|| panic!("expected file lacks '{name}'"));
        assert_json_equivalent(expected_value, actual_value, name);
        println!("{name}: equivalent ✓");
    }

    // Insertion order is contract for the order-bearing maps (RagfairPayloads.cs:94-96,117-122)
    for name in ["fleaPrices", "items", "itemPresets", "presetsByTpl"] {
        let expected_keys: Vec<&String> = expected[name].as_object().unwrap().keys().collect();
        let actual_value = &actual
            .iter()
            .find(|(actual_name, _)| *actual_name == name)
            .unwrap()
            .1;
        let actual_keys: Vec<&String> = actual_value.as_object().unwrap().keys().collect();
        assert_eq!(expected_keys, actual_keys, "{name}: key order");
        println!("{name}: key order preserved ✓ ({} keys)", actual_keys.len());
    }
}
