use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use rayon::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::bot::bot_inventory_generator::{generate_inventory, generate_inventory_batch};
use crate::loot::item_helper::LootError;
use crate::loot::location_loot_generator::{generate_dynamic_loot, generate_static_containers};
use crate::loot::loot_generator::{
    create_forced_loot, create_random_loot, get_random_loot_container_loot,
    get_sealed_weapon_case_loot,
};
use crate::ragfair::models::{DynamicOffersHeader, DynamicOffersResult};
use crate::ragfair::offer_generator::generate_dynamic_offers;
use crate::runtime::runtime;
use crate::verify;

pub const STATUS_OK: i32 = 0;
pub const STATUS_BAD_ARGS: i32 = 1;
pub const STATUS_PANIC: i32 = 2;
/// Generation failed: the error message, not a result, is in the out-buffer.
pub const STATUS_ERROR: i32 = 3;

#[unsafe(no_mangle)]
pub extern "C" fn spt_native_abi_version() -> u32 {
    crate::ABI_VERSION
}

/// # Safety
/// `dir_ptr` must point to `dir_len` readable bytes of UTF-8; `out_ptr` and `out_len`
/// must be valid for writes. The returned buffer must be released with `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_verify_database(
    dir_ptr: *const u8,
    dir_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if dir_ptr.is_null() || out_ptr.is_null() || out_len.is_null() {
        return STATUS_BAD_ARGS;
    }
    let dir_bytes = unsafe { std::slice::from_raw_parts(dir_ptr, dir_len) };
    let Ok(dir) = std::str::from_utf8(dir_bytes) else {
        return STATUS_BAD_ARGS;
    };
    let dir = PathBuf::from(dir);

    let result = catch_unwind(AssertUnwindSafe(|| {
        let report = runtime().block_on(verify::verify(dir));
        serde_json::to_vec(&report).expect("VerifyReport serialization cannot fail")
    }));

    match result {
        Ok(json) => {
            let boxed = json.into_boxed_slice();
            let len = boxed.len();
            unsafe {
                *out_ptr = Box::into_raw(boxed) as *mut u8;
                *out_len = len;
            }
            STATUS_OK
        }
        Err(_) => STATUS_PANIC,
    }
}

/// Hands `bytes` to the caller, who releases them with `spt_buf_free`.
///
/// # Safety
/// `out_ptr` and `out_len` must be valid for writes.
unsafe fn write_buffer(bytes: Vec<u8>, out_ptr: *mut *mut u8, out_len: *mut usize) {
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    unsafe {
        *out_ptr = Box::into_raw(boxed) as *mut u8;
        *out_len = len;
    }
}

/// The payload encoding tags of the framed ragfair envelope.
///
/// The stage-B tag: no longer written, still accepted by the C# reader.
pub const PAYLOAD_JSON: u8 = 0;
/// What `write_framed_offers` emits from stage C on: `rmp_serde::to_vec_named`, so the maps stay
/// string-keyed under the same wire names the JSON stage used.
pub const PAYLOAD_MSGPACK: u8 = 1;

/// The shared body of the generation exports: JSON request in, status plus either the JSON
/// result or an error message out. Runs on the calling thread.
///
/// # Safety
/// As documented on the exports below.
unsafe fn run_generator<Request, Response>(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    generate: fn(Request) -> Result<Response, LootError>,
) -> i32
where
    Request: DeserializeOwned,
    Response: Serialize,
{
    unsafe {
        run_generator_with(req_ptr, req_len, out_ptr, out_len, generate, |response| {
            serde_json::to_vec(&response).expect("result serialization cannot fail")
        })
    }
}

/// `run_generator` with the response encoding open: the ragfair export frames its response
/// instead of emitting one JSON document.
///
/// # Safety
/// As documented on the exports below.
unsafe fn run_generator_with<Request, Response>(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    generate: fn(Request) -> Result<Response, LootError>,
    encode: fn(Response) -> Vec<u8>,
) -> i32
where
    Request: DeserializeOwned,
{
    if req_ptr.is_null() || out_ptr.is_null() || out_len.is_null() {
        return STATUS_BAD_ARGS;
    }
    let request_bytes = unsafe { std::slice::from_raw_parts(req_ptr, req_len) };
    let request: Request = match serde_json::from_slice(request_bytes) {
        Ok(request) => request,
        Err(error) => {
            unsafe { write_buffer(error.to_string().into_bytes(), out_ptr, out_len) };

            return STATUS_BAD_ARGS;
        }
    };

    let result = catch_unwind(AssertUnwindSafe(|| generate(request).map(encode)));

    match result {
        Ok(Ok(json)) => {
            unsafe { write_buffer(json, out_ptr, out_len) };

            STATUS_OK
        }
        // Only the message survives: diagnostics gathered before the failure are dropped.
        Ok(Err(error)) => {
            unsafe { write_buffer(error.message.into_bytes(), out_ptr, out_len) };

            STATUS_ERROR
        }
        Err(_) => STATUS_PANIC,
    }
}

/// # Safety
/// `req_ptr` must point to `req_len` readable bytes of JSON; `out_ptr` and `out_len` must be valid
/// for writes. Any buffer handed back — a result or an error message — must be released with
/// `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_static_containers(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        run_generator(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            generate_static_containers,
        )
    }
}

/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_dynamic_loot(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe { run_generator(req_ptr, req_len, out_ptr, out_len, generate_dynamic_loot) }
}

/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_create_random_loot(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe { run_generator(req_ptr, req_len, out_ptr, out_len, create_random_loot) }
}

/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_create_forced_loot(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe { run_generator(req_ptr, req_len, out_ptr, out_len, create_forced_loot) }
}

/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_get_sealed_weapon_case_loot(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        run_generator(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            get_sealed_weapon_case_loot,
        )
    }
}

/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_get_random_loot_container_loot(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        run_generator(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            get_random_loot_container_loot,
        )
    }
}

/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_bot_inventory(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe { run_generator(req_ptr, req_len, out_ptr, out_len, generate_inventory) }
}

/// One wave of bots in one call - the shared views ride once instead of once per bot.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_bot_inventory_batch(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe { run_generator(req_ptr, req_len, out_ptr, out_len, generate_inventory_batch) }
}

/// The framed ragfair response: encoding tag, length-prefixed header, then one length-prefixed
/// payload per offer, serialized across rayon. Stage C changes only the payloads' encoding.
fn write_framed_offers(result: DynamicOffersResult) -> Vec<u8> {
    let header = rmp_serde::to_vec_named(&DynamicOffersHeader {
        rejected_can_sell_templates: result.rejected_can_sell_templates,
        diagnostics: result.diagnostics,
    })
    .expect("header serialization cannot fail");
    let payloads: Vec<Vec<u8>> = result
        .offers
        .par_iter()
        .map(|offer| rmp_serde::to_vec_named(offer).expect("offer serialization cannot fail"))
        .collect();

    let body: usize = payloads.iter().map(|payload| 4 + payload.len()).sum();
    let mut out = Vec::with_capacity(1 + 4 + header.len() + 4 + body);
    out.push(PAYLOAD_MSGPACK);
    out.extend_from_slice(
        &u32::try_from(header.len())
            .expect("header fits u32")
            .to_le_bytes(),
    );
    out.extend_from_slice(&header);
    out.extend_from_slice(
        &u32::try_from(payloads.len())
            .expect("count fits u32")
            .to_le_bytes(),
    );
    for payload in &payloads {
        out.extend_from_slice(
            &u32::try_from(payload.len())
                .expect("offer fits u32")
                .to_le_bytes(),
        );
        out.extend_from_slice(payload);
    }
    out
}

/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_dynamic_offers(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            generate_dynamic_offers,
            write_framed_offers,
        )
    }
}

/// # Safety
/// `ptr` and `len` must come from a successful `spt_*` call that handed back a buffer, and be
/// freed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_buf_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn call_verify(dir: &str) -> (i32, Option<serde_json::Value>) {
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let status =
            unsafe { spt_verify_database(dir.as_ptr(), dir.len(), &mut out_ptr, &mut out_len) };
        if status != STATUS_OK {
            return (status, None);
        }
        let json = unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec();
        unsafe { spt_buf_free(out_ptr, out_len) };
        (status, Some(serde_json::from_slice(&json).unwrap()))
    }

    #[test]
    fn abi_version_export_matches_crate_const() {
        assert_eq!(spt_native_abi_version(), crate::ABI_VERSION);
        assert_eq!(
            crate::ABI_VERSION,
            11,
            "bump SptNative.ExpectedAbiVersion too"
        );
    }

    #[test]
    fn verify_roundtrips_report_json() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("database")).unwrap();
        let (status, report) = call_verify(dir.path().to_str().unwrap());
        assert_eq!(status, STATUS_OK);
        let report = report.unwrap();
        assert_eq!(report["ok"], false);
        assert_eq!(report["failures"][0]["path"], "checks.dat");
    }

    #[test]
    fn null_arguments_return_bad_args() {
        let status = unsafe {
            spt_verify_database(
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, STATUS_BAD_ARGS);
    }

    #[test]
    fn invalid_utf8_returns_bad_args() {
        let bad = [0xFFu8, 0xFE];
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let status =
            unsafe { spt_verify_database(bad.as_ptr(), bad.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_BAD_ARGS);
    }

    #[test]
    fn freeing_null_is_a_no_op() {
        unsafe { spt_buf_free(std::ptr::null_mut(), 0) };
    }

    /// The container tpl the error-path request below spawns; matches the `itemsView` key in
    /// `COMMON_JSON`, which cannot interpolate a const.
    const CONTAINER_TPL: &str = "111111111111111111111111";

    /// Every required `LootCommon` member, spliced into the request literals below.
    const COMMON_JSON: &str = r#"
        "locationId":"bigmap",
        "itemsView":{"111111111111111111111111":{"width":1,"height":1,"gridCellsH":2,"gridCellsV":2}},
        "defaultPresets":{},"moneyTpls":[],"staticAmmoDist":{},
        "config":{"containerRandomisationEnabled":false,"locationInRandomisationMaps":false,
            "containerTypesToNotRandomise":[],"containerGroupMinSizeMultiplier":1,
            "containerGroupMaxSizeMultiplier":1,"allowDuplicateItemsInStaticContainers":true,
            "tplsToStripChildItemsFrom":[],"fitLootIntoContainerAttempts":3,
            "magazineLootHasAmmoChancePercent":0,"staticMagazineLootHasAmmoChancePercent":0,
            "minFillLooseMagazinePercent":0,"minFillStaticMagazinePercent":0,
            "staticLootMultiplier":1,"looseLootMultiplier":1,"modSpawnChancePercent":{},
            "looseLootBlacklist":[]},
        "seasonal":{"seasonalEventActive":false,"christmasEventEnabled":false,
            "inactiveSeasonalItems":[],"christmasContainerIds":[]},
        "lootableItemBlacklist":[],"counter":{"maxCounts":{},"trackedCounts":{}}
    "#;

    type Export = unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32;

    /// Calls an export and takes ownership of whatever it wrote — a result, or an error message.
    fn call_generate(export: Export, request: &[u8]) -> (i32, Vec<u8>) {
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

    /// An empty map: no weapons, no containers, nothing to draw from.
    fn empty_static_request() -> String {
        format!(
            r#"{{{COMMON_JSON},"staticWeapons":[],"staticContainers":[],"staticForced":[],
            "staticLootDist":{{}}}}"#
        )
    }

    #[test]
    fn static_containers_roundtrips_result_json() {
        let (status, out) = call_generate(
            spt_generate_static_containers,
            empty_static_request().as_bytes(),
        );

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["spawnpoints"], serde_json::json!([]));
        assert_eq!(result["staticContainerCount"], 0);
        assert_eq!(result["staticLootItemCount"], 0);
    }

    #[test]
    fn dynamic_loot_roundtrips_result_json() {
        let request = format!(
            r#"{{{COMMON_JSON},"looseLoot":{{"spawnpointCount":{{"mean":0,"std":0}},
            "spawnpointsForced":[],"spawnpoints":[]}}}}"#
        );

        let (status, out) = call_generate(spt_generate_dynamic_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["spawnpoints"], serde_json::json!([]));
        assert_eq!(result["trackedCounts"], serde_json::json!({}));
    }

    #[test]
    fn invalid_utf8_request_returns_bad_args() {
        let (status, _) = call_generate(spt_generate_static_containers, &[0xFF, 0xFE]);
        assert_eq!(status, STATUS_BAD_ARGS);

        let (status, _) = call_generate(spt_generate_dynamic_loot, &[0xFF, 0xFE]);
        assert_eq!(status, STATUS_BAD_ARGS);
    }

    #[test]
    fn unparseable_request_returns_bad_args_with_the_parse_error() {
        let (status, out) = call_generate(spt_generate_static_containers, b"{\"locationId\":");

        assert_eq!(status, STATUS_BAD_ARGS);
        let message = String::from_utf8(out).unwrap();
        assert!(
            message.contains("EOF while parsing"),
            "expected the serde error, got: {message}"
        );
    }

    #[test]
    fn a_generation_failure_returns_status_error_and_the_message() {
        // The container draws items, but `staticLootDist` has no entry for its tpl — the C#
        // `staticLootDist[containerTypeId]` throws a KeyNotFoundException here.
        let request = format!(
            r#"{{{COMMON_JSON},"staticWeapons":[],"staticForced":[],"staticLootDist":{{}},
            "staticContainers":[{{"probability":1,"template":{{"Id":"c1","IsContainer":true,
                "Root":"aaaaaaaaaaaaaaaaaaaaaaaa",
                "Items":[{{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"{CONTAINER_TPL}"}}]}}}}]}}"#
        );

        let (status, out) = call_generate(spt_generate_static_containers, request.as_bytes());

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("Container: {CONTAINER_TPL} is missing from staticLoot.json")
        );
    }

    /// Every required `RewardLootDb` member, spliced into the reward-loot request literals below.
    /// The weapon tpl in `weaponRewardWeight` below is deliberately absent from `itemsView`.
    const REWARD_DB_JSON: &str = r#"
        "itemsView":{},"defaultPresets":[],"defaultPresetsByTpl":{},
        "globalBlacklist":[],"configBlacklist":[],
        "rewardItemBlacklist":[],"rewardBaseTypeBlacklist":[],
        "bossItems":[],"inactiveSeasonalItems":[]
    "#;

    #[test]
    fn random_loot_roundtrips_result_json() {
        // Every count zero and an empty type whitelist: nothing to draw, so no fixture is needed.
        let request = format!(
            r#"{{{REWARD_DB_JSON},"lootRequest":{{"weaponPresetCount":{{"min":0,"max":0}},
            "armorPresetCount":{{"min":0,"max":0}},"itemCount":{{"min":0,"max":0}},
            "weaponCrateCount":{{"min":0,"max":0}},"itemBlacklist":[],"itemTypeWhitelist":[],
            "itemLimits":{{}},"itemStackLimits":{{}},"armorLevelWhitelist":[]}}}}"#
        );

        let (status, out) = call_generate(spt_create_random_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["items"], serde_json::json!([]));
    }

    #[test]
    fn forced_loot_roundtrips_result_json() {
        let request = format!(r#"{{{REWARD_DB_JSON},"forcedLoot":{{}}}}"#);

        let (status, out) = call_generate(spt_create_forced_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["items"], serde_json::json!([]));
        assert_eq!(result["diagnostics"], serde_json::json!([]));
    }

    #[test]
    fn sealed_weapon_case_roundtrips_result_json() {
        // The drawn weapon is not in `itemsView`, so the generator takes its diagnostic early-out —
        // a result, not an error, which is what the transport has to carry.
        let request = format!(
            r#"{{{REWARD_DB_JSON},"containerSettings":{{
            "weaponRewardWeight":{{"888888888888888888888888":1}},"defaultPresetsOnly":false,
            "weaponModRewardLimits":{{}},"rewardTypeLimits":{{}},"ammoBoxWhitelist":[],
            "allowBossItems":false}},"presetsByTpl":{{}},"linkedItems":{{}}}}"#
        );

        let (status, out) = call_generate(spt_get_sealed_weapon_case_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["items"], serde_json::json!([]));
        assert_eq!(
            result["diagnostics"][0]["localeKey"],
            "loot-non_item_picked_as_sealed_weapon_crate_reward"
        );
    }

    #[test]
    fn random_loot_container_roundtrips_result_json() {
        let request =
            format!(r#"{{{REWARD_DB_JSON},"rewardDetails":{{"rewardCount":0}},"presetTpls":[]}}"#);

        let (status, out) = call_generate(spt_get_random_loot_container_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["items"], serde_json::json!([]));
    }

    #[test]
    fn unparseable_reward_loot_request_returns_bad_args_with_the_parse_error() {
        let (status, out) = call_generate(spt_create_random_loot, b"{\"itemsView\":");

        assert_eq!(status, STATUS_BAD_ARGS);
        let message = String::from_utf8(out).unwrap();
        assert!(
            message.contains("EOF while parsing"),
            "expected the serde error, got: {message}"
        );
    }

    #[test]
    fn a_reward_loot_failure_returns_status_error_and_the_message() {
        // An empty `lootRequest` parses — every member is optional — and then fails on the first
        // null the C# would have thrown on.
        let request = format!(r#"{{{REWARD_DB_JSON},"lootRequest":{{}}}}"#);

        let (status, out) = call_generate(spt_create_random_loot, request.as_bytes());

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "LootRequest.ItemLimits is null"
        );
    }

    /// Every required `GenerateBotInventoryRequest` member, pared down from the orchestrator's own
    /// fixture: no items view, an empty `Pockets` pool (present, or `:516` derefs a null), and zero
    /// loot counts, so the bot generates nothing but the six inventory roots. `equipmentChances` is
    /// left open because the failure case below is the missing `FirstPrimaryWeapon` key.
    fn bot_request(equipment_chances: &str) -> String {
        format!(
            r#"{{
            "botId":"bbbbbbbbbbbbbbbbbbbbbbbb",
            "details":{{"role":"assault","roleLowercase":"assault","side":"Savage","botLevel":15,
                "isPmc":false,"isPlayerScav":false,"gameVersion":"standard","location":"bigmap",
                "botDifficulty":"normal","clearBotContainerCacheAfterGeneration":false}},
            "template":{{
                "inventory":{{"equipment":{{"Pockets":{{}},"Holster":{{}}}},"Ammo":{{}},
                    "items":{{"Backpack":{{}},"Pockets":{{}},"SecuredContainer":{{}},
                        "SpecialLoot":{{}},"TacticalVest":{{}}}},
                    "mods":{{}}}},
                "chances":{{"equipment":{equipment_chances},"weaponMods":{{}},
                    "equipmentMods":{{}}}},
                "generation":{{"items":{{"grenades":{{"weights":{{"0":1}}}},
                    "healing":{{"weights":{{"0":1}}}},"drugs":{{"weights":{{"0":1}}}},
                    "food":{{"weights":{{"0":1}}}},"drink":{{"weights":{{"0":1}}}},
                    "currency":{{"weights":{{"0":1}}}},"stims":{{"weights":{{"0":1}}}},
                    "backpackLoot":{{"weights":{{"0":1}}}},"pocketLoot":{{"weights":{{"0":1}}}},
                    "vestLoot":{{"weights":{{"0":1}}}},"specialItems":{{"weights":{{"0":1}}}},
                    "magazines":{{"weights":{{"0":1}}}}}}}}}},
            "generatingPlayerLevel":20,
            "isNightTime":false,
            "equipment":{{}},
            "bosses":[],
            "durability":{{
                "default":{{"armor":{{"maxDelta":10,"minDelta":0,"minLimitPercent":15}},
                    "weapon":{{"lowestMax":60,"highestMax":100,"maxDelta":10,"minDelta":0,
                        "minLimitPercent":15}}}},
                "botDurabilities":{{}},
                "pmc":{{"armor":{{"lowestMaxPercent":90,"highestMaxPercent":100,"maxDelta":10,
                        "minDelta":0,"minLimitPercent":15}},
                    "weapon":{{"lowestMax":95,"highestMax":100,"maxDelta":5,"minDelta":0,
                        "minLimitPercent":15}}}}}},
            "itemSpawnLimits":{{}},
            "walletLoot":{{"chancePercent":0}},
            "currencyStackSize":{{}},
            "secureContainerAmmoStackCount":0,
            "disableLootOnBotTypes":[],
            "lowProfileGasBlockTpls":[],
            "lootItemResourceRandomization":{{}},
            "pmcConfig":{{}},
            "repairKitWeapon":{{"rarityWeight":{{}},"bonusTypeWeight":{{}},"Common":{{}},
                "Rare":{{}}}},
            "equipmentBlacklist":{{}},
            "lootPools":{{}},
            "itemPresets":{{}},
            "defaultPresetsByTpl":{{}},
            "configBlacklist":[],
            "handbookPrices":{{}},
            "items":{{}}
        }}"#
        )
    }

    #[test]
    fn bot_inventory_roundtrips_result_json() {
        let request = bot_request(r#"{"FirstPrimaryWeapon":0}"#);

        let (status, out) = call_generate(spt_generate_bot_inventory, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        // The six `GenerateInventoryBase` roots and nothing else: every pool is empty.
        assert_eq!(result["inventory"]["items"].as_array().unwrap().len(), 6);
        assert_eq!(
            result["inventory"]["items"][0]["_id"],
            result["inventory"]["equipment"]
        );
        assert_eq!(result["randomisationClamps"], serde_json::json!({}));
    }

    #[test]
    fn unparseable_bot_request_returns_bad_args_with_the_parse_error() {
        let (status, out) = call_generate(spt_generate_bot_inventory, b"{\"botId\":");

        assert_eq!(status, STATUS_BAD_ARGS);
        let message = String::from_utf8(out).unwrap();
        assert!(
            message.contains("EOF while parsing"),
            "expected the serde error, got: {message}"
        );
    }

    #[test]
    fn a_bot_generation_failure_returns_status_error_and_the_message() {
        // `GetDesiredWeaponsForBot` indexes `chances.equipment["FirstPrimaryWeapon"]`
        // unconditionally, so a chances map without it throws where the C# dictionary would.
        let request = bot_request("{}");

        let (status, out) = call_generate(spt_generate_bot_inventory, request.as_bytes());

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "The given key 'FirstPrimaryWeapon' was not present in the dictionary."
        );
    }

    /// `RagfairConfig.Dynamic` pared to the members the wire type names, every chance at zero so no
    /// draw the offer path makes can change the outcome. `offerItemCount` is left open: the failure
    /// case below is a config with neither an entry for the item's parent nor a `"default"`.
    fn ragfair_dynamic(offer_item_count: &str) -> String {
        format!(
            r#"{{"useTraderPriceForOffersIfHigher":true,
            "barter":{{"chancePercent":0,"itemCountMin":1,"itemCountMax":3,
                "priceRangeVariancePercent":15,"minRoubleCostToBecomeBarter":15000,
                "makeSingleStackOnly":false,"itemTplBlacklist":[],"itemTypeBlacklist":[]}},
            "pack":{{"chancePercent":0,"itemCountMin":2,"itemCountMax":10,"itemTypeWhitelist":[]}},
            "offerAdjustment":{{"adjustPriceWhenBelowHandbookPrice":true,
                "maxPriceDifferenceBelowHandbookPercent":70,"handbookPriceMultiplier":1.5,
                "priceThresholdRub":6000}},
            "offerItemCount":{offer_item_count},
            "priceRanges":{{"default":{{"min":0.8,"max":1.2}},"preset":{{"min":0.9,"max":1.4}},
                "pack":{{"min":0.5,"max":0.8}}}},
            "showDefaultPresetsOnly":false,"ignoreQualityPriceVarianceBlacklist":[],
            "endTimeSeconds":{{"min":1000,"max":36000}},"condition":{{}},
            "stackablePercent":{{"min":10,"max":100}},"nonStackableCount":{{"min":1,"max":2}},
            "rating":{{"min":0.2,"max":0.95}},
            "armor":{{"removeRemovablePlateChance":0,"plateSlotIdToRemovePool":[]}},
            "offerCurrencyChancePercent":{{"5449016a4bdc2d6f028b456f":100}},
            "showAsSingleStack":[],"removeSeasonalItemsWhenNotInEvent":true,
            "blacklist":{{"damagedAmmoPacks":true,"custom":[],"enableBsgList":true,
                "enableQuestList":true,"traderItems":false,
                "armorPlate":{{"maxProtectionLevel":3,"ignoreSlots":[]}},
                "enableCustomItemCategoryList":false,"customItemCategoryList":[]}},
            "unreasonableModPrices":{{}},
            "generateBaseFleaPrices":{{"useHandbookPrice":true,"priceMultiplier":1.1,
                "preventPriceBeingBelowTraderBuyPrice":true,"itemTplMultiplierOverride":{{}},
                "itemTypeMultiplierOverride":{{}},"useHideoutCraftMultiplier":false,
                "hideoutCraftMultiplier":1,"generatePresetPriceByChildren":true}}}}"#
        )
    }

    /// Every required `GenerateDynamicOffersRequest` member. The two price tables always know
    /// `SELLABLE_TPL`, which only matters when the items view carries it.
    fn ragfair_request_with(items: &str, offer_item_count: &str) -> String {
        let dynamic = ragfair_dynamic(offer_item_count);
        format!(
            r#"{{"timestamp":1700000000,"offerCounterStart":0,"dynamic":{dynamic},
            "itemPresets":{{}},"defaultPresets":[],"defaultPresetsByTpl":{{}},"presetsByTpl":{{}},
            "fleaPrices":{{"{SELLABLE_TPL}":25000}},"handbookPrices":{{"{SELLABLE_TPL}":20000}},
            "highestTraderPrices":{{"{SELLABLE_TPL}":12000}},"configBlacklist":[],
            "seasonalEventActive":false,"seasonalItemTplBlacklist":[],
            "pmcNamesUsec":["Deagle"],"pmcNamesBear":["Kirill"],"items":{items}}}"#
        )
    }

    /// The one tpl the offer path would accept, were it in the items view.
    const SELLABLE_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";

    fn ragfair_request() -> String {
        ragfair_request_with("{}", r#"{"default":{"min":2,"max":5}}"#)
    }

    /// One sellable item and an `offerItemCount` with neither its parent nor a `"default"` entry.
    fn ragfair_request_missing_default() -> String {
        ragfair_request_with(
            &format!(
                r#"{{"{SELLABLE_TPL}":{{"parent":"cccccccccccccccccccccccc","type":"Item",
                "stackMaxSize":1,"canSellOnRagfair":true}}}}"#
            ),
            "{}",
        )
    }

    /// Decodes one frame payload by the envelope's encoding tag. The writer never emits msgpack
    /// bin/ext, so every payload lands in a `Value` cleanly.
    fn decode(encoding: u8, payload: &[u8]) -> serde_json::Value {
        match encoding {
            PAYLOAD_JSON => serde_json::from_slice(payload).unwrap(),
            PAYLOAD_MSGPACK => rmp_serde::from_slice(payload).unwrap(),
            other => panic!("unknown payload encoding {other}"),
        }
    }

    /// Splits a framed ragfair response: (encoding, header, offer payloads).
    fn parse_framed(out: &[u8]) -> (u8, serde_json::Value, Vec<Vec<u8>>) {
        let encoding = out[0];
        let mut at = 1;
        let read_len = |buf: &[u8], at: usize| {
            u32::from_le_bytes(buf[at..at + 4].try_into().unwrap()) as usize
        };
        let header_len = read_len(out, at);
        at += 4;
        let header = decode(encoding, &out[at..at + header_len]);
        at += header_len;
        let count = read_len(out, at);
        at += 4;
        let mut payloads = Vec::with_capacity(count);
        for _ in 0..count {
            let len = read_len(out, at);
            at += 4;
            payloads.push(out[at..at + len].to_vec());
            at += len;
        }
        assert_eq!(at, out.len(), "trailing bytes after the last frame");
        (encoding, header, payloads)
    }

    #[test]
    fn a_minimal_dynamic_offers_request_returns_an_empty_offer_list() {
        // Empty items view and empty presets: the assort walk yields nothing, so no draws happen.
        let (status, out) =
            call_generate(spt_generate_dynamic_offers, ragfair_request().as_bytes());

        assert_eq!(status, STATUS_OK);
        let (encoding, header, payloads) = parse_framed(&out);
        assert_eq!(encoding, PAYLOAD_MSGPACK);
        assert!(payloads.is_empty());
        assert_eq!(header["rejectedCanSellTemplates"], serde_json::json!([]));
    }

    #[test]
    fn unparseable_dynamic_offers_request_returns_bad_args_with_the_parse_error() {
        let (status, out) = call_generate(spt_generate_dynamic_offers, b"{\"timestamp\":");

        assert_eq!(status, STATUS_BAD_ARGS);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("EOF while parsing")
        );
    }

    #[test]
    fn a_dynamic_offers_failure_returns_status_error_and_the_message() {
        // `offerItemCount` without a "default" entry is the unguarded C# dictionary miss: it
        // dereferences the null `MinMax` `GetValueOrDefault` hands back, so the message is the
        // null-reference one, not one naming the key.
        let (status, out) = call_generate(
            spt_generate_dynamic_offers,
            ragfair_request_missing_default().as_bytes(),
        );

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Object reference not set to an instance of an object."
        );
    }

    /// Thirty expired single-item entries, unseeded — the parallel walk. One offer per expired
    /// entry, `intId` sequential from `offerCounterStart`, offers in assort order: the merge
    /// contract stage A must preserve.
    #[test]
    fn an_unseeded_expired_pass_keeps_assort_order_and_sequential_int_ids() {
        let expired: Vec<String> = (0..30)
            .map(|i| format!(r#"[{{"_id":"{i:024x}","_tpl":"{SELLABLE_TPL}"}}]"#))
            .collect();
        let request = ragfair_request_with(
            &format!(
                r#"{{"{SELLABLE_TPL}":{{"parent":"cccccccccccccccccccccccc","type":"Item",
                "stackMaxSize":1,"canSellOnRagfair":true}}}}"#
            ),
            r#"{"default":{"min":2,"max":5}}"#,
        )
        // splice the expired entries and a non-zero counter start into the request JSON
        .replacen(
            r#"{"timestamp":"#,
            &format!(r#"{{"expiredOffers":[{}],"timestamp":"#, expired.join(",")),
            1,
        )
        .replacen(r#""offerCounterStart":0"#, r#""offerCounterStart":7"#, 1);

        let (status, out) = call_generate(spt_generate_dynamic_offers, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let (encoding, _, payloads) = parse_framed(&out);
        assert_eq!(encoding, PAYLOAD_MSGPACK);
        assert_eq!(payloads.len(), 30);
        for (i, payload) in payloads.iter().enumerate() {
            let offer = decode(encoding, payload);
            assert_eq!(offer["intId"], serde_json::json!(7 + i as i64));
            assert_eq!(offer["root"], serde_json::json!(format!("{i:024x}")));
        }

        // The C# reader keys off the JSON wire names — a serde rename regression must fail here.
        let offer: serde_json::Value = rmp_serde::from_slice(&payloads[0]).unwrap();
        for key in ["_id", "intId", "user", "root", "items", "requirements"] {
            assert!(
                offer.get(key).is_some(),
                "offer payload lost wire key {key}"
            );
        }
    }

    #[test]
    fn null_generation_arguments_return_bad_args() {
        let status = unsafe {
            spt_generate_static_containers(
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, STATUS_BAD_ARGS);

        let status = unsafe {
            spt_generate_dynamic_loot(
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, STATUS_BAD_ARGS);
    }

    #[test]
    fn a_null_out_pointer_returns_bad_args_without_writing() {
        // The request itself is fine, so only the out-pointer guard can reject it — and it has to,
        // since the failure paths write a message buffer through that pointer.
        let request = empty_static_request();
        let mut out_len: usize = 0;

        let status = unsafe {
            spt_generate_static_containers(
                request.as_ptr(),
                request.len(),
                std::ptr::null_mut(),
                &mut out_len,
            )
        };

        assert_eq!(status, STATUS_BAD_ARGS);
        assert_eq!(out_len, 0, "nothing may be written when out_ptr is null");
    }
}
