//! Resident-arm integration: publish a minimal four-root DB with per-map statics and weapon
//! presets, then prove a `{epoch}` send generates identically to the same data sent as
//! `viewsOverride` — for location loot and for the reward family's sealed case — and that a
//! wrong epoch is a stale error, not a wrong answer.
//!
//! Its own process (integration tests build their own binary), so the process-global store races
//! with nothing; the whole protocol lives in one `#[test]` fn, sequential, to keep it that way.

use spt_native::ffi::{
    STATUS_OK, STATUS_STALE_EPOCH, spt_buf_free, spt_db_publish, spt_generate_static_containers,
    spt_get_sealed_weapon_case_loot,
};
use spt_native::loot::item_helper::{MONEY, WEAPON};

/// `BaseClasses.ITEM` — the root node the money chain hangs off.
const ITEM_NODE: &str = "54009119af1c881c07000029";
const CONTAINER_TPL: &str = "111111111111111111111111";
const MONEY_TPL: &str = "333333333333333333333333";

// The sealed-case tpls are deliberately non-hex so `strip_mongo_ids` leaves them visible — a
// draw that diverged between the two arms would then fail the byte comparison.
const MISC_NODE: &str = "misc_node";
const WEAPON_TPL: &str = "weapon_tpl";
const MOD_A_TPL: &str = "weapon_mod_a";
const MOD_B_TPL: &str = "weapon_mod_b";

type Export = unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32;

/// Calls an export and takes ownership of whatever it wrote — a result, or an error message.
fn call(export: Export, request: &[u8]) -> (i32, Vec<u8>) {
    let mut out_ptr: *mut u8 = std::ptr::null_mut();
    let mut out_len: usize = 0;
    let status = unsafe { export(request.as_ptr(), request.len(), &mut out_ptr, &mut out_len) };
    if out_ptr.is_null() {
        return (status, Vec::new());
    }
    let out = unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec();
    unsafe { spt_buf_free(out_ptr, out_len) };

    (status, out)
}

/// Strips MongoIds (24 hex chars) — item ids are minted from the process-wide MongoId counter,
/// not the seeded RNG, so they legitimately differ between two seeded runs.
fn strip_mongo_ids(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut run = String::new();
    for c in json.chars() {
        if c.is_ascii_hexdigit() {
            run.push(c);
            continue;
        }
        if run.len() == 24 {
            out.push_str("<id>");
        } else {
            out.push_str(&run);
        }
        run.clear();
        out.push(c);
    }
    if run.len() == 24 {
        out.push_str("<id>");
    } else {
        out.push_str(&run);
    }

    out
}

/// A container spawn point with fixed data ids, shared verbatim by the resident root and the
/// override send so the two arms read identical bytes.
fn container(id: &str, root: &str, probability: f64) -> String {
    format!(
        r#"{{"probability":{probability},"template":{{"Id":"{id}","IsContainer":true,
        "Root":"{root}","Items":[{{"_id":"{root}","_tpl":"{CONTAINER_TPL}"}}]}}}}"#
    )
}

/// The `staticLoot`/`staticLootDist` value, shared verbatim by both arms.
fn loot_dist() -> String {
    format!(
        r#"{{"{CONTAINER_TPL}":{{
            "itemcountDistribution":[{{"count":2,"relativeProbability":1}}],
            "itemDistribution":[{{"tpl":"{MONEY_TPL}","relativeProbability":1}}]}}}}"#
    )
}

/// The `staticContainers` list, shared verbatim by both arms.
fn containers_json() -> String {
    format!(
        "[{},{},{}]",
        container("c1", "aaaaaaaaaaaaaaaaaaaaaaa1", 1.0),
        container("r1", "aaaaaaaaaaaaaaaaaaaaaaa2", 0.5),
        container("r2", "aaaaaaaaaaaaaaaaaaaaaaa3", 0.5),
    )
}

/// The `statics` value (container groups), shared verbatim by both arms.
fn statics_json() -> &'static str {
    r#"{"containersGroups":{"g1":{"minContainers":1,"maxContainers":2}},
        "containers":{"r1":{"groupId":"g1"},"r2":{"groupId":"g1"}}}"#
}

/// The two weapon presets' item lists, shared verbatim by the publish's `_items` and the
/// override's `presetsByTpl` views so both arms draw from identical bytes. The default (p1, two
/// items) and the alternative (p2, one item) differ in length, so which preset a draw picked is
/// visible in the response shape.
fn preset_p1_items() -> String {
    format!(
        r#"[{{"_id":"root_p1","_tpl":"{WEAPON_TPL}"}},
        {{"_id":"mod_p1","_tpl":"{MOD_A_TPL}","parentId":"root_p1","slotId":"mod_stock"}}]"#
    )
}

fn preset_p2_items() -> String {
    format!(r#"[{{"_id":"root_p2","_tpl":"{WEAPON_TPL}"}}]"#)
}

/// The override's `presetsByTpl`: `GetPresets(tpl)` for the one weapon, in globals map order —
/// the same order the resident derive keeps.
fn preset_views_json() -> String {
    format!(
        r#"{{"{WEAPON_TPL}":[
        {{"items":{p1},"id":"p1","name":"weapon_default","encyclopedia":"{WEAPON_TPL}"}},
        {{"items":{p2},"id":"p2","name":"weapon_alt"}}]}}"#,
        p1 = preset_p1_items(),
        p2 = preset_p2_items(),
    )
}

/// Every `SealedWeaponCaseVarying` member, shared verbatim by both sealed arms: a single-entry
/// weapon weight (no draw), a mod reward count with a real range (draws), and a seed so both
/// arms replay one stream.
fn sealed_varying() -> String {
    format!(
        r#""globalBlacklist":[],"configBlacklist":[],"rewardItemBlacklist":[],
        "rewardBaseTypeBlacklist":[],"bossItems":[],"inactiveSeasonalItems":[],"testSeed":7,
        "containerSettings":{{"weaponRewardWeight":{{"{WEAPON_TPL}":1}},"defaultPresetsOnly":false,
            "weaponModRewardLimits":{{"{MISC_NODE}":{{"min":1,"max":2}}}},"rewardTypeLimits":{{}},
            "ammoBoxWhitelist":[],"allowBossItems":false}},
        "linkedItems":{{"{WEAPON_TPL}":["{MOD_A_TPL}","{MOD_B_TPL}"]}}"#
    )
}

/// Every `LootVarying` member of the sends below, `testSeed` included so both arms replay one
/// draw stream.
fn varying() -> String {
    format!(
        r#""locationId":"bigmap","moneyTpls":["{MONEY_TPL}"],"staticAmmoDist":{{}},
        "config":{{"containerRandomisationEnabled":true,"locationInRandomisationMaps":true,
            "containerTypesToNotRandomise":[],"containerGroupMinSizeMultiplier":1,
            "containerGroupMaxSizeMultiplier":1,"allowDuplicateItemsInStaticContainers":true,
            "tplsToStripChildItemsFrom":[],"fitLootIntoContainerAttempts":3,
            "magazineLootHasAmmoChancePercent":0,"staticMagazineLootHasAmmoChancePercent":0,
            "minFillLooseMagazinePercent":0,"minFillStaticMagazinePercent":0,
            "staticLootMultiplier":1,"looseLootMultiplier":1,"modSpawnChancePercent":{{}},
            "looseLootBlacklist":[]}},
        "seasonal":{{"seasonalEventActive":false,"christmasEventEnabled":false,
            "inactiveSeasonalItems":[],"christmasContainerIds":[]}},
        "lootableItemBlacklist":[],"counter":{{"maxCounts":{{}},"trackedCounts":{{}}}},
        "testSeed":42"#
    )
}

#[test]
fn a_resident_send_matches_the_override_send_and_a_wrong_epoch_is_stale() {
    // (1) A four-root publish: templates whose derived items view is exactly the override's
    // below, empty traders/globals (the ragfair and quest derives are total over them), and a
    // locations root carrying bigmap's statics.
    let publish = format!(
        r#"{{"schema":1,"roots":{{
            "templates":{{"items":{{
                "{ITEM_NODE}":{{"_type":"Node","_parent":"","_props":{{}}}},
                "{MONEY}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{WEAPON}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{MISC_NODE}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{CONTAINER_TPL}":{{"_parent":"{ITEM_NODE}","_props":{{"Width":1,"Height":1,
                    "Grids":[{{"_props":{{"cellsH":2,"cellsV":2}}}}]}}}},
                "{MONEY_TPL}":{{"_parent":"{MONEY}","_props":{{"Width":1,"Height":1,
                    "StackMaxSize":500000,"StackMinRandom":100,"StackMaxRandom":200}}}},
                "{WEAPON_TPL}":{{"_parent":"{WEAPON}","_props":{{}}}},
                "{MOD_A_TPL}":{{"_parent":"{MISC_NODE}","_props":{{}}}},
                "{MOD_B_TPL}":{{"_parent":"{MISC_NODE}","_props":{{}}}}
            }},"handbook":{{"Items":[]}},"prices":{{}}}},
            "traders":{{}},
            "globals":{{"ItemPresets":{{
                "p1":{{"_id":"p1","_name":"weapon_default","_encyclopedia":"{WEAPON_TPL}",
                    "_items":{p1_items}}},
                "p2":{{"_id":"p2","_name":"weapon_alt","_items":{p2_items}}}
            }}}},
            "locations":{{"bigmap":{{"base":{{"Id":"bigmap"}},"allExtracts":[],
                "staticLoot":{loot},
                "staticContainers":{{"staticWeapons":[],"staticContainers":{containers},
                    "staticForced":[]}},
                "statics":{statics}}}}}
        }}}}"#,
        loot = loot_dist(),
        containers = containers_json(),
        statics = statics_json(),
        p1_items = preset_p1_items(),
        p2_items = preset_p2_items(),
    );

    // (2) Publish through the FFI and read back the epoch.
    let (status, out) = call(spt_db_publish, publish.as_bytes());
    assert_eq!(
        status,
        STATUS_OK,
        "publish failed: {}",
        String::from_utf8_lossy(&out)
    );
    let response: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let epoch = response["epoch"]
        .as_u64()
        .expect("publish answers the epoch");

    // (3) The resident send: no viewsOverride, just the published epoch.
    let resident_request = format!(r#"{{"epoch":{epoch},"varying":{{{}}}}}"#, varying());
    let (status, resident) = call(spt_generate_static_containers, resident_request.as_bytes());
    assert_eq!(
        status,
        STATUS_OK,
        "resident send failed: {}",
        String::from_utf8_lossy(&resident)
    );

    // (4) The same varying with the equivalent viewsOverride at epoch 0: the itemsView members
    // mirror what the publish's templates derive (location loot reads none of the members the
    // full derive adds beyond these), the statics are the same bytes the locations root carries.
    let override_request = format!(
        r#"{{"epoch":0,"viewsOverride":{{
            "itemsView":{{
                "{ITEM_NODE}":{{}},
                "{MONEY}":{{"parent":"{ITEM_NODE}"}},
                "{CONTAINER_TPL}":{{"parent":"{ITEM_NODE}","width":1,"height":1,
                    "gridCellsH":2,"gridCellsV":2}},
                "{MONEY_TPL}":{{"parent":"{MONEY}","width":1,"height":1,
                    "stackMaxSize":500000,"stackMinRandom":100,"stackMaxRandom":200}}
            }},
            "defaultPresets":{{}},
            "staticWeapons":[],"staticContainers":{containers},"staticForced":[],
            "staticLootDist":{loot},"statics":{statics}
        }},"varying":{{{varying}}}}}"#,
        loot = loot_dist(),
        containers = containers_json(),
        statics = statics_json(),
        varying = varying()
    );
    let (status, override_send) = call(spt_generate_static_containers, override_request.as_bytes());
    assert_eq!(
        status,
        STATUS_OK,
        "override send failed: {}",
        String::from_utf8_lossy(&override_send)
    );

    // The flip's promise: identical generation off either arm, minted ids aside.
    assert_eq!(
        strip_mongo_ids(&String::from_utf8(resident).unwrap()),
        strip_mongo_ids(&String::from_utf8(override_send).unwrap())
    );

    // (5) An epoch the store does not hold is stale, never a wrong answer.
    let stale_request = format!(
        r#"{{"epoch":{},"varying":{{{}}}}}"#,
        epoch + 1000,
        varying()
    );
    let (status, out) = call(spt_generate_static_containers, stale_request.as_bytes());
    assert_eq!(status, STATUS_STALE_EPOCH);
    assert!(String::from_utf8(out).unwrap().contains("epoch mismatch"));

    // (6) The reward family off the same publish, on its most resident-mapped export (sealed):
    // the resident arm reads the derived ragfair views, the override arm the same data as
    // `viewsOverride` — one seed, byte-identical rewards.
    let resident_sealed_request =
        format!(r#"{{"epoch":{epoch},"varying":{{{}}}}}"#, sealed_varying());
    let (status, resident_sealed) = call(
        spt_get_sealed_weapon_case_loot,
        resident_sealed_request.as_bytes(),
    );
    assert_eq!(
        status,
        STATUS_OK,
        "resident sealed send failed: {}",
        String::from_utf8_lossy(&resident_sealed)
    );

    let override_sealed_request = format!(
        r#"{{"epoch":0,"viewsOverride":{{
            "itemsView":{{
                "{WEAPON_TPL}":{{"parent":"{WEAPON}"}},
                "{MOD_A_TPL}":{{"parent":"{MISC_NODE}"}},
                "{MOD_B_TPL}":{{"parent":"{MISC_NODE}"}}
            }},
            "defaultPresets":[],"defaultPresetsByTpl":{{}},
            "presetsByTpl":{presets}
        }},"varying":{{{varying}}}}}"#,
        presets = preset_views_json(),
        varying = sealed_varying(),
    );
    let (status, override_sealed) = call(
        spt_get_sealed_weapon_case_loot,
        override_sealed_request.as_bytes(),
    );
    assert_eq!(
        status,
        STATUS_OK,
        "override sealed send failed: {}",
        String::from_utf8_lossy(&override_sealed)
    );

    // Two empty results would compare equal vacuously — the weapon preset must be in there.
    let resident_sealed = String::from_utf8(resident_sealed).unwrap();
    assert!(
        resident_sealed.contains(WEAPON_TPL),
        "no weapon drawn: {resident_sealed}"
    );

    // The sealed tpls are non-hex, so a diverging draw survives the id strip and fails here.
    assert_eq!(
        strip_mongo_ids(&resident_sealed),
        strip_mongo_ids(&String::from_utf8(override_sealed).unwrap())
    );

    // (7) A reward export answers a stale epoch with status 4 as well.
    let stale_sealed_request = format!(
        r#"{{"epoch":{},"varying":{{{}}}}}"#,
        epoch + 1000,
        sealed_varying()
    );
    let (status, out) = call(
        spt_get_sealed_weapon_case_loot,
        stale_sealed_request.as_bytes(),
    );
    assert_eq!(status, STATUS_STALE_EPOCH);
    assert!(String::from_utf8(out).unwrap().contains("epoch mismatch"));
}
