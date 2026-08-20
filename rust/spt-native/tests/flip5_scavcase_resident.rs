//! Resident-arm integration for the scav case flip (#5): publish a minimal five-root DB —
//! templates/traders/globals, the hideout root carrying the scav recipes, and the configs root
//! carrying the `spt-scavcase`/`spt-item` stems — then prove a `{epoch}` send generates
//! identically to the same data sent as `viewsOverride`, and that a wrong epoch is a stale error,
//! not a wrong answer.
//!
//! Its own process (integration tests build their own binary), so the process-global store races
//! with nothing; the whole protocol lives in one `#[test]` fn, sequential, to keep it that way.

use spt_native::ffi::{
    STATUS_OK, STATUS_STALE_EPOCH, spt_buf_free, spt_db_publish, spt_generate_scav_case_rewards,
};
use spt_native::loot::item_helper::{AMMO, MONEY, WEAPON};

/// `BaseClasses.ITEM` — the root node every parent chain hangs off.
const ITEM_NODE: &str = "54009119af1c881c07000029";

/// `Money.ROUBLES` — `GetRandomMoney` indexes all four real money tpls straight out of the items
/// view, so the fixture must carry every one of them under the `MONEY` node.
const ROUBLES: &str = "5449016a4bdc2d6f028b456f";
const EUROS: &str = "569668774bdc2da2298b4568";
const DOLLARS: &str = "5696686a4bdc2da3298b456a";
const GP: &str = "5d235b4d86f7742e017bc88a";

// Deliberately non-hex so `strip_mongo_ids` leaves them visible — a draw that diverged between
// the two arms would then fail the byte comparison. (The money tpls above are forced hex, so a
// diverging money draw shows through its stack count instead: every currency/rarity pair below
// carries a distinct fixed count.)
const AMMO_TPL: &str = "scav_ammo_tpl";
const WEAPON_TPL: &str = "scav_weapon_tpl";
const MOD_TPL: &str = "scav_weapon_mod";
const RECIPE_ID: &str = "recipe_ok";

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

/// The two weapon presets' item lists, shared verbatim by the publish's `_items` and the
/// override's `defaultPresetsByTpl` view so both arms clone identical bytes. The default (p1, two
/// items) and the non-default (p2, one item) differ in length, so a derive that resolved the
/// wrong preset is visible in the response shape.
fn preset_p1_items() -> String {
    format!(
        r#"[{{"_id":"root_p1","_tpl":"{WEAPON_TPL}"}},
        {{"_id":"mod_p1","_tpl":"{MOD_TPL}","parentId":"root_p1","slotId":"mod_stock"}}]"#
    )
}

fn preset_p2_items() -> String {
    format!(r#"[{{"_id":"root_p2","_tpl":"{WEAPON_TPL}"}}]"#)
}

/// The scav case config, shared verbatim by the publish's `spt-scavcase` stem and the override
/// bundle's `config` member — byte-equal generation off the two arms is the gate that the resident
/// resolve and the override parse read the same values out of it.
///
/// The chances are 100/100 with both `allowMultiple*` caps on, so every draw arm fires on a fixed
/// schedule rather than by seed luck: each rarity's first pick is money, its second is ammo, and
/// anything after falls through to the price-filtered pool — whose common band (40k-60k) holds
/// exactly the weapon, making the third common pick the default-preset expansion. The recipe's
/// counts (common 3, rare 1, superrare 0) therefore exercise the money, ammo, plain-item, and
/// preset arms in one send.
fn config() -> String {
    // `kind` and `ammoRewardBlacklist` are members the view does not bind — carried here so both
    // arms prove they ride past rather than failing the parse.
    r#"{
        "kind":"spt-scavcase",
        "rewardItemValueRangeRub":{"common":{"min":40000.0,"max":60000.0},
            "rare":{"min":0.0,"max":100000.0},"superrare":{"min":0.0,"max":0.0}},
        "moneyRewards":{"moneyRewardChancePercent":100,
            "rubCount":{"common":{"min":1000,"max":1000},"rare":{"min":1111,"max":1111},
                "superrare":{"min":1,"max":1}},
            "usdCount":{"common":{"min":2000,"max":2000},"rare":{"min":2222,"max":2222},
                "superrare":{"min":1,"max":1}},
            "eurCount":{"common":{"min":3000,"max":3000},"rare":{"min":3333,"max":3333},
                "superrare":{"min":1,"max":1}},
            "gpCount":{"common":{"min":4000,"max":4000},"rare":{"min":4444,"max":4444},
                "superrare":{"min":1,"max":1}}},
        "ammoRewards":{"ammoRewardChancePercent":100,"ammoRewardBlacklist":{},
            "ammoRewardValueRangeRub":{"common":{"min":0.0,"max":80.0}},
            "minStackSize":30},
        "rewardItemParentBlacklist":[],"rewardItemBlacklist":[],
        "allowMultipleMoneyRewardsPerRarity":false,
        "allowMultipleAmmoRewardsPerRarity":false,
        "allowBossItemsAsRewards":true}"#
        .to_owned()
}

/// `ItemFilterService.GetItemRewardBlacklist()`/`GetBossItems()`, which are `ItemConfig`'s own two
/// sets verbatim — shared by the publish's `spt-item` stem and the override bundle. Both hold a tpl
/// the fixture's items view does not, so neither arm's reward pool changes.
const REWARD_BLACKLISTED: &str = "scav_reward_blacklisted";
const BOSS_ITEM: &str = "scav_boss_item";

/// Every `ScavCaseVarying` member, shared verbatim by both sends, `testSeed` included so both
/// arms replay one draw stream. Config-backed members moved out of here to [`config`] and the
/// `spt-item` stem; what is left is the per-call id, the service-state sets and the seed.
fn varying() -> String {
    format!(
        r#""recipeId":"{RECIPE_ID}","inactiveSeasonalItems":[],"globalBlacklist":[],"testSeed":42"#
    )
}

#[test]
fn a_resident_send_matches_the_override_send_and_a_wrong_epoch_is_stale() {
    // (1) A five-root publish: templates whose derived items view is exactly the override's
    // below, empty traders (the ragfair derive is total over them), globals carrying the weapon's
    // two presets, a hideout root with two scav recipes — one complete, one missing its
    // `Rare` band, which the derivation must drop rather than error on — and a configs root
    // carrying the two stems the family's config-backed inputs come out of, `kind` members and
    // all, as the C# projection writes them.
    let publish = format!(
        r#"{{"schema":1,"roots":{{
            "templates":{{"items":{{
                "{ITEM_NODE}":{{"_type":"Node","_parent":"","_props":{{}}}},
                "{MONEY}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{AMMO}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{WEAPON}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{ROUBLES}":{{"_type":"Item","_parent":"{MONEY}","_props":{{"StackMaxSize":500000}}}},
                "{EUROS}":{{"_type":"Item","_parent":"{MONEY}","_props":{{"StackMaxSize":500000}}}},
                "{DOLLARS}":{{"_type":"Item","_parent":"{MONEY}","_props":{{"StackMaxSize":500000}}}},
                "{GP}":{{"_type":"Item","_parent":"{MONEY}","_props":{{"StackMaxSize":500000}}}},
                "{AMMO_TPL}":{{"_type":"Item","_parent":"{AMMO}","_props":{{"StackMaxSize":60}}}},
                "{WEAPON_TPL}":{{"_type":"Item","_parent":"{WEAPON}","_props":{{}}}}
            }},"handbook":{{"Items":[
                {{"Id":"{AMMO_TPL}","Price":50}},
                {{"Id":"{WEAPON_TPL}","Price":50000}}
            ]}},"prices":{{}}}},
            "traders":{{}},
            "globals":{{"ItemPresets":{{
                "p2":{{"_id":"p2","_name":"weapon_alt","_items":{p2_items}}},
                "p1":{{"_id":"p1","_name":"weapon_default","_encyclopedia":"{WEAPON_TPL}",
                    "_items":{p1_items}}}
            }}}},
            "hideout":{{"production":{{"scavRecipes":[
                {{"_id":"{RECIPE_ID}","endProducts":{{"Common":{{"min":3,"max":3}},
                    "Rare":{{"min":1,"max":1}},"Superrare":{{"min":0,"max":0}}}}}},
                {{"_id":"recipe_missing_rare","endProducts":{{"Common":{{"min":1,"max":1}},
                    "Superrare":{{"min":0,"max":0}}}}}}
            ]}}}},
            "configs":{{
                "spt-scavcase":{config},
                "spt-item":{{"kind":"spt-item","blacklist":[],
                    "rewardItemBlacklist":["{REWARD_BLACKLISTED}"],
                    "rewardItemTypeBlacklist":[],"bossItems":["{BOSS_ITEM}"],
                    "handbookPriceOverride":{{}}}},
                "spt-core":{{"kind":"spt-core"}}
            }}
        }}}}"#,
        p1_items = preset_p1_items(),
        p2_items = preset_p2_items(),
        config = config(),
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
    let (status, resident) = call(spt_generate_scav_case_rewards, resident_request.as_bytes());
    assert_eq!(
        status,
        STATUS_OK,
        "resident send failed: {}",
        String::from_utf8_lossy(&resident)
    );

    // (4) The same varying with the equivalent viewsOverride at epoch 0. Unlike flip #4's sealed
    // send, the reward pool here iterates the whole items view, so the override must mirror the
    // resident derive in full: every items-table key in table order (types included — the pool
    // filters read them), the handbook prices with the 0.0 handbook-miss default written out, the
    // recipe in view form (lowercase bands, the incomplete recipe already dropped), and the
    // default-preset map the preset cache resolves — p1, the `_encyclopedia` default, not p2.
    let override_request = format!(
        r#"{{"epoch":0,"viewsOverride":{{
            "scavRecipes":[{{"id":"{RECIPE_ID}","endProducts":{{"common":{{"min":3,"max":3}},
                "rare":{{"min":1,"max":1}},"superrare":{{"min":0,"max":0}}}}}}],
            "itemsView":{{
                "{ITEM_NODE}":{{"type":"Node"}},
                "{MONEY}":{{"parent":"{ITEM_NODE}","type":"Node"}},
                "{AMMO}":{{"parent":"{ITEM_NODE}","type":"Node"}},
                "{WEAPON}":{{"parent":"{ITEM_NODE}","type":"Node"}},
                "{ROUBLES}":{{"parent":"{MONEY}","type":"Item","stackMaxSize":500000}},
                "{EUROS}":{{"parent":"{MONEY}","type":"Item","stackMaxSize":500000}},
                "{DOLLARS}":{{"parent":"{MONEY}","type":"Item","stackMaxSize":500000}},
                "{GP}":{{"parent":"{MONEY}","type":"Item","stackMaxSize":500000}},
                "{AMMO_TPL}":{{"parent":"{AMMO}","type":"Item","stackMaxSize":60}},
                "{WEAPON_TPL}":{{"parent":"{WEAPON}","type":"Item"}}
            }},
            "staticPrices":{{
                "{ITEM_NODE}":0.0,"{MONEY}":0.0,"{AMMO}":0.0,"{WEAPON}":0.0,
                "{ROUBLES}":0.0,"{EUROS}":0.0,"{DOLLARS}":0.0,"{GP}":0.0,
                "{AMMO_TPL}":50.0,"{WEAPON_TPL}":50000.0
            }},
            "defaultPresetsByTpl":{{"{WEAPON_TPL}":{{"items":{p1_items},"id":"p1",
                "name":"weapon_default","encyclopedia":"{WEAPON_TPL}"}}}},
            "config":{config},
            "rewardItemBlacklist":["{REWARD_BLACKLISTED}"],"bossItems":["{BOSS_ITEM}"]
        }},"varying":{{{varying}}}}}"#,
        p1_items = preset_p1_items(),
        config = config(),
        varying = varying(),
    );
    let (status, override_send) = call(spt_generate_scav_case_rewards, override_request.as_bytes());
    assert_eq!(
        status,
        STATUS_OK,
        "override send failed: {}",
        String::from_utf8_lossy(&override_send)
    );

    // Two responses that skipped every reward would compare equal vacuously — the preset arm's
    // weapon must be in there (quirk 5 silently drops it if the default-preset map missed).
    let resident = String::from_utf8(resident).unwrap();
    assert!(
        resident.contains(WEAPON_TPL),
        "no weapon preset expanded: {resident}"
    );

    // (5) The flip's promise: identical generation off either arm, minted ids aside.
    assert_eq!(
        strip_mongo_ids(&resident),
        strip_mongo_ids(&String::from_utf8(override_send).unwrap())
    );

    // (6) An epoch the store does not hold is stale, never a wrong answer.
    let stale_request = format!(r#"{{"epoch":{},"varying":{{{}}}}}"#, epoch + 1, varying());
    let (status, out) = call(spt_generate_scav_case_rewards, stale_request.as_bytes());
    assert_eq!(status, STATUS_STALE_EPOCH);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "resident DB epoch mismatch; republish and retry"
    );
}
