use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::loot::item_helper::LootError;
use crate::loot::location_loot_generator::{generate_dynamic_loot, generate_static_containers};
use crate::loot::loot_generator::{
    create_forced_loot, create_random_loot, get_random_loot_container_loot,
    get_sealed_weapon_case_loot,
};
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

    let result = catch_unwind(AssertUnwindSafe(|| {
        generate(request).map(|response| {
            serde_json::to_vec(&response).expect("result serialization cannot fail")
        })
    }));

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
            4,
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
