//! Regression guard for the Completion whitelist filter's base-class lookups.
//!
//! `CompletionQuestGenerator.GetWhitelistedItemSelection` (`:365-371`) tests every candidate in a
//! 137-entry whitelist against every item in the pool. C# affords that shape because
//! `ItemBaseClassService` answers each `IsOfBaseclass` from a precomputed ancestor set in O(1);
//! [`is_of_baseclass`] walks the parent chain live, so the same shape costs a full walk per
//! candidate and the filter dominates the whole quest call.
//!
//! This measures the three formulations against the real shipped item table and pins the invariant
//! the generator has to exploit: one walk that tests every candidate at each link beats a walk per
//! candidate. The arms must also agree on the kept set - the cheap formulations are only usable
//! because they are behaviour-identical.
//!
//! Requires `scripts/decompress-assets.sh` to have unpacked `items.json`.

use std::collections::HashSet;
use std::time::Instant;

use indexmap::IndexMap;
use spt_native::loot::item_helper::{is_of_baseclass, is_of_baseclasses};
use spt_native::loot::models::ItemView;

const ITEMS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/items.json"
);
const TEMPLATES_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/repeatableQuests.json"
);

/// The level the benchmark fixture generates at - the midpoint of Completion's second shipped band.
const PMC_LEVEL: i64 = 20;

/// Headroom over the measured ratio, which sits around 7x on an idle box. Loose enough that a
/// contended CI run does not fail it, tight enough that reintroducing the per-candidate walk does.
const MIN_SPEEDUP: f64 = 2.0;

fn json(path: &str) -> serde_json::Value {
    let contents = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("{path} unreadable ({error}) - run scripts/decompress-assets.sh")
    });

    serde_json::from_str(&contents).expect("JSON")
}

/// `PayloadProjection.BuildItemsView`, cut to the one member the base-class walk reads.
fn items_view() -> IndexMap<String, ItemView> {
    let raw = json(ITEMS_PATH);
    let mut out = serde_json::Map::new();

    for (id, template) in raw.as_object().expect("items object") {
        out.insert(
            id.clone(),
            serde_json::json!({ "parent": template["_parent"] }),
        );
    }

    serde_json::from_value(serde_json::Value::Object(out)).expect("items view parses")
}

/// The Completion whitelist as `levelled_item_ids` unions it for a [`PMC_LEVEL`] PMC.
fn whitelist_ids() -> Vec<String> {
    json(TEMPLATES_PATH)["data"]["Completion"]["itemsWhitelist"]
        .as_array()
        .expect("whitelist")
        .iter()
        .filter(|entry| {
            entry["minPlayerLevel"]
                .as_i64()
                .is_some_and(|min| min <= PMC_LEVEL)
        })
        .flat_map(|entry| {
            entry["itemIds"]
                .as_array()
                .expect("itemIds")
                .iter()
                .map(|id| id.as_str().expect("tpl").to_owned())
        })
        .collect()
}

/// [`is_of_baseclasses`] with the candidates in a set: its `base_class_tpls.contains` is a linear
/// scan of the slice at every link, which a 137-entry whitelist makes the inner cost.
fn is_of_baseclasses_set(
    items_view: &IndexMap<String, ItemView>,
    tpl: &str,
    base_class_tpls: &HashSet<&str>,
) -> bool {
    let mut current = items_view.get(tpl);

    while let Some(item) = current {
        let parent = match item.parent.as_deref() {
            Some(parent) if !parent.is_empty() => parent,
            _ => return false,
        };

        if base_class_tpls.contains(parent) {
            return true;
        }

        current = items_view.get(parent);
    }

    false
}

#[test]
fn one_walk_per_item_beats_one_walk_per_whitelist_candidate() {
    let items = items_view();
    let whitelist = whitelist_ids();
    let whitelist_refs: Vec<&str> = whitelist.iter().map(String::as_str).collect();
    let whitelist_set: HashSet<&str> = whitelist_refs.iter().copied().collect();

    // Upper bound on the selection reaching the filter: every template in the table. The real pool
    // is the ~4,120 priced, parented `Item` templates that survive the validity and budget filters.
    let selection: Vec<&str> = items.keys().map(String::as_str).collect();

    // --- as ported: `.Any()` restarts the parent walk for every whitelisted candidate ---
    let start = Instant::now();
    let kept_per_candidate = selection
        .iter()
        .filter(|tpl| {
            whitelist_refs
                .iter()
                .any(|whitelisted| is_of_baseclass(&items, tpl, whitelisted))
                || whitelist_set.contains(**tpl)
        })
        .count();
    let per_candidate = start.elapsed();

    // --- one walk, every candidate tested at each link, linear scan of the slice ---
    let start = Instant::now();
    let kept_single_walk = selection
        .iter()
        .filter(|tpl| {
            is_of_baseclasses(&items, tpl, &whitelist_refs) || whitelist_set.contains(**tpl)
        })
        .count();
    let single_walk = start.elapsed();

    // --- one walk, candidates in a set ---
    let start = Instant::now();
    let kept_single_walk_set = selection
        .iter()
        .filter(|tpl| {
            is_of_baseclasses_set(&items, tpl, &whitelist_set) || whitelist_set.contains(**tpl)
        })
        .count();
    let single_walk_set = start.elapsed();

    println!("templates              : {}", items.len());
    println!("whitelist candidates   : {}", whitelist_refs.len());
    println!("selection              : {}", selection.len());
    println!("walk per candidate     : {per_candidate:?}  kept {kept_per_candidate}");
    println!("one walk, slice scan   : {single_walk:?}  kept {kept_single_walk}");
    println!("one walk, set lookup   : {single_walk_set:?}  kept {kept_single_walk_set}");

    assert_eq!(
        kept_per_candidate, kept_single_walk,
        "the single walk must keep the set the ported shape keeps"
    );
    assert_eq!(
        kept_per_candidate, kept_single_walk_set,
        "the set lookup must keep the set the ported shape keeps"
    );

    let speedup = per_candidate.as_secs_f64() / single_walk.as_secs_f64();
    assert!(
        speedup >= MIN_SPEEDUP,
        "one walk should beat a walk per candidate by at least {MIN_SPEEDUP}x, measured {speedup:.1}x"
    );
}
