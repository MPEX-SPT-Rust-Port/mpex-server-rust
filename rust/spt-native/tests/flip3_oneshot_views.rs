//! Flip #3 — equivalence of the resident-derived walk inputs against the C#-built override
//! payloads, compared through the walk outputs, over the full real database.
//!
//! Run by hand, after `OneShotViewsEquivalenceTests` has written the fixture files:
//!   cargo test --release --test flip3_oneshot_views -- --ignored --nocapture

use indexmap::IndexMap;
use spt_native::db::models::PublishRequest;
use spt_native::loot::models::ItemView;
use spt_native::{base_class, linked_items};

fn fixture(file_name: &str) -> Vec<u8> {
    let path = std::env::temp_dir().join(file_name);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "run OneShotViewsEquivalenceTests first — {}: {error}",
            path.display()
        )
    })
}

/// The shared `{"itemsView": …}` shape of both C# override payloads.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverridePayload {
    items_view: IndexMap<String, ItemView>,
}

#[test]
#[ignore = "needs the C#-written fixtures; run OneShotViewsEquivalenceTests first"]
fn resident_walk_outputs_match_the_override_payloads() {
    let envelope: PublishRequest =
        serde_json::from_slice(&fixture("spt-flip3-roots.json")).expect("publish envelope parses");
    let templates = envelope
        .roots
        .templates
        .expect("envelope carries the templates root");

    let base_override: OverridePayload =
        serde_json::from_slice(&fixture("spt-flip3-baseclass-override.json"))
            .expect("base class override parses");
    let expected = base_class::build(&base_override.items_view);
    let actual = base_class::build(&base_class::items_view_for_base_class(&templates.items));
    assert_eq!(
        actual.item_base_classes, expected.item_base_classes,
        "base class chains diverge between resident and override inputs"
    );
    assert_eq!(
        actual.root_node_ids, expected.root_node_ids,
        "root node ids diverge (order included) between resident and override inputs"
    );
    let base_class_chains = expected.item_base_classes.len();

    let linked_override: OverridePayload =
        serde_json::from_slice(&fixture("spt-flip3-linkeditems-override.json"))
            .expect("linked items override parses");
    let expected = linked_items::build(&linked_override.items_view);
    let actual = linked_items::build(&linked_items::items_view_for_linked_items(&templates.items));
    assert_eq!(
        actual.linked_items, expected.linked_items,
        "linked item sets diverge between resident and override inputs"
    );

    println!(
        "flip #3 handshake green over the full real database: {} base-class chains, {} linked-item sets",
        base_class_chains,
        expected.linked_items.len()
    );
}
