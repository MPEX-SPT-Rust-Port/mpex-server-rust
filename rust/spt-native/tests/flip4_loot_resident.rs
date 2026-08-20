//! Resident-arm integration: publish a minimal five-root DB with per-map statics, weapon presets
//! and the three config stems the two families read (`spt-item`, `spt-location`,
//! `spt-seasonalevents`), then prove a `{epoch}` send generates identically to the same data sent
//! as `viewsOverride` — for location loot and for the reward family's sealed case — and that a
//! wrong epoch is a stale error, not a wrong answer.
//!
//! Location loot is sent for two maps, because the resident arm resolves the config view per
//! location: `bigmap` has an entry in every map-keyed member of the `spt-location` config,
//! `factory4_day` in none of them, so between them they cover both sides of all three of
//! `BuildConfigView`'s per-location resolutions.
//!
//! Its own process (integration tests build their own binary), so the process-global store races
//! with nothing; the whole protocol lives in one `#[test]` fn, sequential, to keep it that way.

use spt_native::ffi::{
    STATUS_OK, STATUS_STALE_EPOCH, spt_buf_free, spt_db_publish, spt_generate_dynamic_loot,
    spt_generate_static_containers, spt_get_sealed_weapon_case_loot,
};
use spt_native::loot::item_helper::{MONEY, WEAPON};

/// `BaseClasses.ITEM` — the root node the money chain hangs off.
const ITEM_NODE: &str = "54009119af1c881c07000029";
const CONTAINER_TPL: &str = "111111111111111111111111";
const MONEY_TPL: &str = "333333333333333333333333";

/// A guaranteed container that only the `spt-seasonalevents` stem's christmas list keeps out — so
/// a resident arm that failed to read that stem spawns it and fails the byte comparison.
const XMAS_CONTAINER_ID: &str = "xmas1";
/// The loose loot spawn point `bigmap` blacklists and `factory4_day` does not.
const LOOSE_POINT_ID: &str = "f4d_point";

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

/// The `staticContainers` list, shared verbatim by both arms. The christmas container is
/// guaranteed, so only the seasonal filter can keep it out of a result.
fn containers_json() -> String {
    format!(
        "[{},{},{},{}]",
        container("c1", "aaaaaaaaaaaaaaaaaaaaaaa1", 1.0),
        container("r1", "aaaaaaaaaaaaaaaaaaaaaaa2", 0.5),
        container("r2", "aaaaaaaaaaaaaaaaaaaaaaa3", 0.5),
        container(XMAS_CONTAINER_ID, "aaaaaaaaaaaaaaaaaaaaaaa4", 1.0),
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
/// arms replay one stream. The four `ItemConfig` sets are not here — they ride the resident
/// configs root on one arm and [`reward_item_config_json`] on the other.
fn sealed_varying() -> String {
    format!(
        r#""globalBlacklist":[],"inactiveSeasonalItems":[],"testSeed":7,
        "containerSettings":{{"weaponRewardWeight":{{"{WEAPON_TPL}":1}},"defaultPresetsOnly":false,
            "weaponModRewardLimits":{{"{MISC_NODE}":{{"min":1,"max":2}}}},"rewardTypeLimits":{{}},
            "ammoBoxWhitelist":[],"allowBossItems":false}},
        "linkedItems":{{"{WEAPON_TPL}":["{MOD_A_TPL}","{MOD_B_TPL}"]}}"#
    )
}

/// The four `ItemConfig` sets on the override arm, byte-identical to what the publish's
/// `spt-item` stem carries below — all four empty, so no filter fires on either arm.
fn reward_item_config_json() -> &'static str {
    r#""configBlacklist":[],"rewardItemBlacklist":[],
        "rewardBaseTypeBlacklist":[],"bossItems":[]"#
}

/// Every `LootVarying` member of the sends below, `testSeed` included so both arms replay one
/// draw stream. The rest of the config view and the christmas container ids are not here — they
/// ride the resident configs root on one arm and [`config_view_json`] on the other; the two
/// multipliers are, because C# resolves those against the live raid-adjusted config on both arms
/// and the values must match what [`config_view_json`] puts in the override's view.
fn varying(location_id: &str, test_seed: u64) -> String {
    let multiplier = multiplier_for(location_id);

    format!(
        r#""locationId":"{location_id}","moneyTpls":["{MONEY_TPL}"],"staticAmmoDist":{{}},
        "staticLootMultiplier":{multiplier},"looseLootMultiplier":{multiplier},
        "seasonal":{{"seasonalEventActive":false,"christmasEventEnabled":false,
            "inactiveSeasonalItems":[]}},
        "lootableItemBlacklist":[],"counter":{{"maxCounts":{{}},"trackedCounts":{{}}}},
        "testSeed":{test_seed}"#
    )
}

/// What `MultiplierForLocation` answers for each fixture location, C#-side: `bigmap` has its own
/// entry in the config, `factory4_day` falls back to `"default"`. Both arms are handed this same
/// resolved number, so it is the one config value the byte comparison cannot police — the live-data
/// C# gates (`LootParityTests`, `LootResidentDbTests`) cover the resolution itself.
fn multiplier_for(location_id: &str) -> &'static str {
    if location_id == "bigmap" { "1" } else { "2" }
}

/// The seed the static sends replay. Every static send `install`s a fresh stream from it, so the
/// order they run in cannot shift one arm's draws relative to the other's.
const STATIC_SEED: u64 = 42;

/// The seed the dynamic pair replays — deliberately *not* [`STATIC_SEED`]. A dynamic send
/// `resume`s the stream a preceding static send of the same seed parked (that is what keeps one
/// raid on one stream), so on 42 the first of the two dynamic sends would carry on from the last
/// static send and the second would start fresh: two different streams for the same request.
const DYNAMIC_SEED: u64 = 99;

/// The `spt-location` stem the publish carries: `bigmap` has an entry in both map-keyed members the
/// resident arm resolves per location, `factory4_day` in neither. Every scalar is a value the
/// generator reads, so a lift that dropped one shows up as a diverging draw. The two multiplier maps
/// are deliberately absent: the lift no longer declares them (they are adjusted per raid and ride
/// the request instead), and a publish carrying them would only prove they are ignored.
fn location_config_stem() -> String {
    format!(
        r#"{{"kind":"spt-location",
        "containerRandomisationSettings":{{"enabled":true,"maps":{{"bigmap":true}},
            "containerTypesToNotRandomise":[],"containerGroupMinSizeMultiplier":1,
            "containerGroupMaxSizeMultiplier":1}},
        "allowDuplicateItemsInStaticContainers":true,"tplsToStripChildItemsFrom":[],
        "fitLootIntoContainerAttempts":3,"magazineLootHasAmmoChancePercent":0,
        "staticMagazineLootHasAmmoChancePercent":0,"minFillLooseMagazinePercent":0,
        "minFillStaticMagazinePercent":0,
        "equipmentLootSettings":{{"modSpawnChancePercent":{{}}}},
        "looseLootBlacklist":{{"bigmap":["{LOOSE_POINT_ID}"]}}}}"#
    )
}

/// `LocationLootGenerator.BuildConfigView(locationId)` as the C# projects it, for the override arm.
/// The two locations differ in the two members the resident resolution has to get right — `bigmap`
/// is in `maps` and has its own blacklist, `factory4_day` is in neither — plus the multiplier, which
/// is C#-resolved on both arms and so must equal what [`varying`] sends.
fn config_view_json(location_id: &str) -> String {
    let multiplier = multiplier_for(location_id);
    let (in_maps, blacklist) = match location_id {
        "bigmap" => ("true", format!(r#"["{LOOSE_POINT_ID}"]"#)),
        _ => ("false", "[]".to_owned()),
    };

    format!(
        r#"{{"containerRandomisationEnabled":true,"locationInRandomisationMaps":{in_maps},
        "containerTypesToNotRandomise":[],"containerGroupMinSizeMultiplier":1,
        "containerGroupMaxSizeMultiplier":1,"allowDuplicateItemsInStaticContainers":true,
        "tplsToStripChildItemsFrom":[],"fitLootIntoContainerAttempts":3,
        "magazineLootHasAmmoChancePercent":0,"staticMagazineLootHasAmmoChancePercent":0,
        "minFillLooseMagazinePercent":0,"minFillStaticMagazinePercent":0,
        "staticLootMultiplier":{multiplier},"looseLootMultiplier":{multiplier},
        "modSpawnChancePercent":{{}},"looseLootBlacklist":{blacklist}}}"#
    )
}

/// The `itemsView` + `defaultPresets` + config-backed members every location-loot override
/// carries, resolved for `location_id`.
fn loot_views_json(location_id: &str) -> String {
    format!(
        r#""itemsView":{{
            "{ITEM_NODE}":{{}},
            "{MONEY}":{{"parent":"{ITEM_NODE}"}},
            "{CONTAINER_TPL}":{{"parent":"{ITEM_NODE}","width":1,"height":1,
                "gridCellsH":2,"gridCellsV":2}},
            "{MONEY_TPL}":{{"parent":"{MONEY}","width":1,"height":1,
                "stackMaxSize":500000,"stackMinRandom":100,"stackMaxRandom":200}}
        }},
        "defaultPresets":{{}},
        "config":{config},
        "christmasContainerIds":["{XMAS_CONTAINER_ID}"]"#,
        config = config_view_json(location_id),
    )
}

/// The loose loot `factory4_day`'s dynamic send generates from: one guaranteed point, whose
/// template id is the one `bigmap` — and only `bigmap` — blacklists.
fn loose_loot_json() -> String {
    format!(
        r#"{{"spawnpointCount":{{"mean":1,"std":0}},"spawnpointsForced":[],
        "spawnpoints":[{{"locationId":"f4d_1","probability":1,
            "template":{{"Id":"{LOOSE_POINT_ID}","Root":"aaaaaaaaaaaaaaaaaaaaaab1",
                "Items":[{{"_id":"aaaaaaaaaaaaaaaaaaaaaab1","_tpl":"{MONEY_TPL}",
                    "composedKey":"ck1"}}]}},
            "itemDistribution":[{{"composedKey":{{"key":"ck1"}},"relativeProbability":1}}]}}]}}"#
    )
}

#[test]
fn a_resident_send_matches_the_override_send_and_a_wrong_epoch_is_stale() {
    // (1) A five-root publish: templates whose derived items view is exactly the override's
    // below, empty traders/globals (the ragfair and quest derives are total over them), a
    // locations root carrying two maps' statics, and a configs root carrying the `spt-item` stem
    // (the four sets the reward family used to be handed per send) plus the `spt-location` and
    // `spt-seasonalevents` stems the location-loot family resolves its config view out of.
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
            "locations":{{
                "bigmap":{{"base":{{"Id":"bigmap"}},"allExtracts":[],
                    "staticLoot":{loot},
                    "staticContainers":{{"staticWeapons":[],"staticContainers":{containers},
                        "staticForced":[]}},
                    "statics":{statics}}},
                "factory4_day":{{"base":{{"Id":"factory4_day"}},"allExtracts":[],
                    "staticLoot":{loot},
                    "staticContainers":{{"staticWeapons":[],"staticContainers":{containers},
                        "staticForced":[]}},
                    "statics":{statics}}}}},
            "configs":{{"spt-item":{{"kind":"spt-item","blacklist":[],"rewardItemBlacklist":[],
                "rewardItemTypeBlacklist":[],"bossItems":[]}},
                "spt-location":{location_config},
                "spt-seasonalevents":{{"kind":"spt-seasonalevents",
                    "christmasContainerIds":["{XMAS_CONTAINER_ID}"]}}}}
        }}}}"#,
        loot = loot_dist(),
        containers = containers_json(),
        statics = statics_json(),
        location_config = location_config_stem(),
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

    // (3) The resident send, once per location: no viewsOverride, just the published epoch. Two
    // locations because the config view is resolved per location on this arm — `bigmap` hits its
    // own entry in all three map-keyed members, `factory4_day` hits none of them and takes the
    // `"default"` multiplier, the not-randomised branch and an empty blacklist.
    for location_id in ["bigmap", "factory4_day"] {
        let resident_request = format!(
            r#"{{"epoch":{epoch},"varying":{{{}}}}}"#,
            varying(location_id, STATIC_SEED)
        );
        let (status, resident) = call(spt_generate_static_containers, resident_request.as_bytes());
        assert_eq!(
            status,
            STATUS_OK,
            "resident send for {location_id} failed: {}",
            String::from_utf8_lossy(&resident)
        );

        // (4) The same varying with the equivalent viewsOverride at epoch 0: the itemsView members
        // mirror what the publish's templates derive (location loot reads none of the members the
        // full derive adds beyond these), the statics are the same bytes the locations root
        // carries, and `config` is what `BuildConfigView(locationId)` would have projected.
        let override_request = format!(
            r#"{{"epoch":0,"viewsOverride":{{{views},
                "staticWeapons":[],"staticContainers":{containers},"staticForced":[],
                "staticLootDist":{loot},"statics":{statics}
            }},"varying":{{{varying}}}}}"#,
            views = loot_views_json(location_id),
            loot = loot_dist(),
            containers = containers_json(),
            statics = statics_json(),
            varying = varying(location_id, STATIC_SEED)
        );
        let (status, override_send) =
            call(spt_generate_static_containers, override_request.as_bytes());
        assert_eq!(
            status,
            STATUS_OK,
            "override send for {location_id} failed: {}",
            String::from_utf8_lossy(&override_send)
        );

        // The christmas container is guaranteed and both arms must have dropped it — otherwise the
        // seasonal stem went unread on the resident arm and the comparison below is vacuous.
        let resident = String::from_utf8(resident).unwrap();
        assert!(
            !resident.contains(XMAS_CONTAINER_ID),
            "the christmas container survived: {resident}"
        );

        // The flip's promise: identical generation off either arm, minted ids aside.
        assert_eq!(
            strip_mongo_ids(&resident),
            strip_mongo_ids(&String::from_utf8(override_send).unwrap()),
            "resident and override diverged on {location_id}"
        );
    }

    // (4b) The blacklist resolution is only observable on the dynamic path: `factory4_day` has no
    // `looseLootBlacklist` entry, so the one point it generates from must survive — a resident arm
    // that fell back to another map's list (or to the whole map) would drop it.
    let dynamic_varying = format!(
        r#"{}, "looseLoot":{}"#,
        varying("factory4_day", DYNAMIC_SEED),
        loose_loot_json()
    );
    let resident_dynamic = format!(r#"{{"epoch":{epoch},"varying":{{{dynamic_varying}}}}}"#);
    let (status, resident_dynamic) = call(spt_generate_dynamic_loot, resident_dynamic.as_bytes());
    assert_eq!(
        status,
        STATUS_OK,
        "resident dynamic send failed: {}",
        String::from_utf8_lossy(&resident_dynamic)
    );
    let override_dynamic = format!(
        r#"{{"epoch":0,"viewsOverride":{{{views}}},"varying":{{{dynamic_varying}}}}}"#,
        views = loot_views_json("factory4_day"),
    );
    let (status, override_dynamic) = call(spt_generate_dynamic_loot, override_dynamic.as_bytes());
    assert_eq!(
        status,
        STATUS_OK,
        "override dynamic send failed: {}",
        String::from_utf8_lossy(&override_dynamic)
    );

    let resident_dynamic = String::from_utf8(resident_dynamic).unwrap();
    assert!(
        resident_dynamic.contains(LOOSE_POINT_ID),
        "the point bigmap blacklists was dropped for factory4_day: {resident_dynamic}"
    );
    assert_eq!(
        strip_mongo_ids(&resident_dynamic),
        strip_mongo_ids(&String::from_utf8(override_dynamic).unwrap())
    );

    // (5) An epoch the store does not hold is stale, never a wrong answer.
    let stale_request = format!(
        r#"{{"epoch":{},"varying":{{{}}}}}"#,
        epoch + 1000,
        varying("bigmap", STATIC_SEED)
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

    // This override `itemsView` (3 entries) is a strict subset of the resident derived view (~9);
    // byte-equality holds only because `rewardTypeLimits: {}` keeps the items-view-iterating
    // reward pool cold — widen it in this fixture and the two arms iterate different pools.
    let override_sealed_request = format!(
        r#"{{"epoch":0,"viewsOverride":{{
            "itemsView":{{
                "{WEAPON_TPL}":{{"parent":"{WEAPON}"}},
                "{MOD_A_TPL}":{{"parent":"{MISC_NODE}"}},
                "{MOD_B_TPL}":{{"parent":"{MISC_NODE}"}}
            }},
            "defaultPresets":[],"defaultPresetsByTpl":{{}},
            "presetsByTpl":{presets},
            {item_config}
        }},"varying":{{{varying}}}}}"#,
        presets = preset_views_json(),
        item_config = reward_item_config_json(),
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
