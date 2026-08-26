//! Phase 3 fused load over the real shipped `SPT_Data` tree.
//!
//! Its own process (integration tests build their own binary), so the process-global store races
//! with nothing and the first publish is epoch 1. `verify: false` — the source tree carries no
//! `checks.dat`; that file is generated into the build output.
//!
//! This is also the loud gate for the flip's new constraint: the five resident-root file sets
//! (templates, traders, globals, locations, hideout) must be comment-free JSON, where the pure-C#
//! importer tolerated JSONC. A comment anywhere in them fails this test by name.
//!
//! Requires `scripts/decompress-assets.sh` to have unpacked `looseLoot.json`.

use std::collections::BTreeMap;

use spt_native::db;
use spt_native::db::load::{LoadRequest, load};

const SPT_DATA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Libraries/SPTarkov.Server.Assets/SPT_Data"
);

#[tokio::test]
async fn the_shipped_tree_installs_five_roots_and_hands_back_the_eager_files() {
    let response = load(LoadRequest {
        schema: 1,
        dir: SPT_DATA.to_string(),
        verify: false,
        handbook_price_override: None,
    })
    .await
    .unwrap_or_else(|error| {
        panic!("{SPT_DATA} failed to load ({error:?}) — run scripts/decompress-assets.sh")
    });

    assert!(response.verify.is_none());
    assert_eq!(response.epoch, Some(1), "the first publish of this process");

    let resident = db::current().expect("a resident DB");
    let templates = resident.templates.as_ref().expect("templates");
    let traders = resident.traders.as_ref().expect("traders");
    assert!(resident.globals.is_some());
    let locations = resident.locations.as_ref().expect("locations");
    let hideout = resident.hideout.as_ref().expect("hideout");
    assert!(resident.ragfair_views.is_some());
    assert!(resident.quest_views.is_some());
    assert!(resident.bot_views.is_some());

    assert!(
        templates.items.len() > 3000,
        "items view is {}",
        templates.items.len()
    );
    assert_eq!(traders.traders.len(), 12);
    assert_eq!(
        locations.locations.len(),
        19,
        "map dirs, never locations/base.json"
    );
    assert!(!hideout.production.scav_recipes.is_empty());

    let files: BTreeMap<String, Vec<u8>> = response.files.into_iter().collect();

    // Return-only families: no resident root claims them, C# still needs the bytes.
    for eager in [
        "database/bots/types/assault.json",
        "database/locales/menu/en.json",
        "database/locations/factory4_day/staticAmmo.json",
        "database/server.json",
    ] {
        assert!(files.contains_key(eager), "{eager} should ship to C#");
    }

    // Lazy (C# re-reads the disk path per access), assembly-only, and importer-skipped files
    // must never ride the handoff.
    let excluded: Vec<&str> = files
        .keys()
        .map(String::as_str)
        .filter(|key| {
            key.ends_with("/looseLoot.json")
                || key.ends_with("/staticLoot.json")
                || key.ends_with("/staticContainers.json")
                || key.starts_with("database/locales/global/")
                || key.starts_with("database/locales/server/")
                || key.starts_with("database/locales/web/")
                || key.ends_with("/bearsuits.json")
                || key.ends_with("/usecsuits.json")
                || key.ends_with("/archivedQuests.json")
        })
        .collect();
    assert!(excluded.is_empty(), "these must not ship: {excluded:?}");

    // The handoff is byte-exact against disk (BOM stripped, nothing else touched).
    let prices = std::fs::read(format!("{SPT_DATA}/database/templates/prices.json")).unwrap();
    let prices = prices
        .strip_prefix(&[0xEF, 0xBB, 0xBF][..])
        .unwrap_or(&prices);
    assert_eq!(files["database/templates/prices.json"], prices);

    // Cross-parse determinism, total over the shipped tree: a second load of the same bytes must
    // digest identically on all five gated roots. This is the automatic sweep for the rule at
    // db::hash_value — anything reachable from a gated root must be IndexSet/IndexMap, never
    // HashSet/HashMap — so a per-instance-order container under a gated root fails loudly here
    // instead of surfacing as an intermittently red equivalence gate pointing at the wrong layer.
    // In this test (same process, same store) because loads install globally and the epoch-1
    // assertion above owns first place.
    let first: serde_json::Value = serde_json::from_slice(&db::resident_digests_json()).unwrap();
    load(LoadRequest {
        schema: 1,
        dir: SPT_DATA.to_string(),
        verify: false,
        handbook_price_override: None,
    })
    .await
    .expect("the second load of the same tree");
    let second: serde_json::Value = serde_json::from_slice(&db::resident_digests_json()).unwrap();
    for root in ["templates", "traders", "globals", "locations", "hideout"] {
        assert_eq!(
            first["roots"][root], second["roots"][root],
            "{root}: two parses of the same bytes must digest equal"
        );
    }
}
