use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use rayon::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::bot::bot_inventory_generator::{generate_inventory, generate_inventory_batch};
use crate::diag::DiagSink;
use crate::logger::{LogLevel, LogRecord, Logger};
use crate::loot::item_helper::LootError;
use crate::loot::location_loot_generator::{generate_dynamic_loot, generate_static_containers};
use crate::loot::loot_generator::{
    create_forced_loot, create_random_loot, get_random_loot_container_loot,
    get_sealed_weapon_case_loot,
};
use crate::quest::{QuestError, generate_repeatable_quest};
use crate::ragfair::models::{DynamicOffersHeader, DynamicOffersResult};
use crate::ragfair::offer_generator::{RagfairError, generate_dynamic_offers};
use crate::runtime::runtime;
use crate::verify;

pub const STATUS_OK: i32 = 0;
pub const STATUS_BAD_ARGS: i32 = 1;
pub const STATUS_PANIC: i32 = 2;
/// Generation failed: the error message, not a result, is in the out-buffer.
pub const STATUS_ERROR: i32 = 3;
/// A slice-less ragfair or repeatable-quest request named a stamp this process has not cached; the
/// caller resends.
pub const STATUS_STALE_SLICE: i32 = 4;

/// How a generator failure crosses the boundary: which status code, and what message buffer.
pub trait FfiFailure {
    fn status(&self) -> i32;
    fn into_message(self) -> String;
}

impl FfiFailure for LootError {
    fn status(&self) -> i32 {
        STATUS_ERROR
    }

    fn into_message(self) -> String {
        self.message
    }
}

impl FfiFailure for RagfairError {
    fn status(&self) -> i32 {
        match self {
            RagfairError::Loot(_) => STATUS_ERROR,
            RagfairError::StaleSlice => STATUS_STALE_SLICE,
        }
    }

    fn into_message(self) -> String {
        match self {
            RagfairError::Loot(error) => error.message,
            RagfairError::StaleSlice => "no cached invariant slice for this stamp".to_string(),
        }
    }
}

impl FfiFailure for QuestError {
    fn status(&self) -> i32 {
        match self {
            QuestError::Failed(_) => STATUS_ERROR,
            QuestError::StaleSlice => STATUS_STALE_SLICE,
        }
    }

    fn into_message(self) -> String {
        match self {
            QuestError::Failed(message) => message,
            QuestError::StaleSlice => "no cached invariant slice for this stamp".to_string(),
        }
    }
}

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
unsafe fn run_generator_with<Request, Response, Error>(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    generate: fn(Request) -> Result<Response, Error>,
    encode: fn(Response) -> Vec<u8>,
) -> i32
where
    Request: DeserializeOwned,
    Error: FfiFailure,
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
            let status = error.status();
            unsafe { write_buffer(error.into_message().into_bytes(), out_ptr, out_len) };

            status
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
    unsafe {
        run_generator(req_ptr, req_len, out_ptr, out_len, |request| {
            create_random_loot(request, &mut DiagSink::Pipeline)
        })
    }
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
        run_generator(req_ptr, req_len, out_ptr, out_len, |request| {
            get_sealed_weapon_case_loot(request, &mut DiagSink::Pipeline)
        })
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

/// One repeatable quest, from the cached invariant slice or the one this request carries.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_repeatable_quest(
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
            generate_repeatable_quest,
            |response| {
                serde_json::to_vec(&response).expect("quest response serialization cannot fail")
            },
        )
    }
}

/// The process-wide log pipeline and its init ref-count. A plain `Mutex<Option>` rather than a
/// `OnceLock` so `spt_logger_close` can take it down (and tests can re-initialise afterwards); the
/// count lives inside the same lock so init and close cannot race each other.
static LOGGER: Mutex<(usize, Option<Logger>)> = Mutex::new((0, None));

fn logger_guard() -> std::sync::MutexGuard<'static, (usize, Option<Logger>)> {
    LOGGER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Native emitters get a small process-local id per thread — the managed thread id never crosses
/// the boundary for lines Rust originates, but worker threads still stay distinguishable.
static NEXT_EMIT_TID: AtomicI32 = AtomicI32::new(1);
thread_local! {
    static EMIT_TID: i32 = NEXT_EMIT_TID.fetch_add(1, Ordering::Relaxed);
}

/// The native-side entry to the same pipeline `spt_log_emit` feeds: filters, level gate, format,
/// sinks. Uninitialised pipeline is a silent no-op, matching the export's contract.
pub(crate) fn emit_pipeline(category: &str, level: LogLevel, message: &str) {
    let unix_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let thread = std::thread::current();
    let record = LogRecord {
        category,
        message,
        exception: "",
        thread_name: thread.name().unwrap_or(""),
        level,
        tid: EMIT_TID.with(|tid| *tid),
        unix_millis,
    };
    if let Some(logger) = logger_guard().1.as_ref() {
        logger.emit(&record);
    }
}

/// Initialises the log pipeline from the raw bytes of `sptLogger.json`. Ref-counted: a call while
/// already initialised keeps the running pipeline, ignores the new config, bumps the count and
/// returns `STATUS_OK`. It takes as many `spt_logger_close` calls as there were successful inits to
/// tear the pipeline down - the prepatcher's nested `Program.Main` disposes its own container while
/// the outer host keeps logging. On `STATUS_ERROR` the parse-error text is in the out-buffer, the
/// count is untouched, the pipeline stays uninitialised and every later emit is a silent no-op - a
/// broken log config must not stop the server.
///
/// # Safety
/// `config_ptr` must point to `config_len` readable bytes of UTF-8; `out_ptr` and `out_len` must
/// be valid for writes. A returned buffer is released with `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_logger_init(
    config_ptr: *const u8,
    config_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if out_ptr.is_null() || out_len.is_null() {
        return STATUS_BAD_ARGS;
    }

    // Zeroed before the `config_ptr` guard: a bad-args return still leaves the caller's out-params
    // written, never stale.
    unsafe {
        *out_ptr = std::ptr::null_mut();
        *out_len = 0;
    }

    if config_ptr.is_null() {
        return STATUS_BAD_ARGS;
    }

    let bytes = unsafe { std::slice::from_raw_parts(config_ptr, config_len) };

    match catch_unwind(AssertUnwindSafe(|| {
        let mut guard = logger_guard();
        if guard.1.is_some() {
            guard.0 += 1;
            return Ok(());
        }
        Logger::from_json(bytes).map(|logger| {
            *guard = (1, Some(logger));
        })
    })) {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(error)) => {
            unsafe { write_buffer(error.into_bytes(), out_ptr, out_len) };
            STATUS_ERROR
        }
        Err(_) => STATUS_PANIC,
    }
}

/// Queues one log message through the pipeline: filters, level gate, per-target formatting, and
/// the console/file writer threads. `STATUS_OK` no-op when the pipeline is uninitialised - init
/// failure was already reported once, per-line noise would drown it.
///
/// # Safety
/// Each pointer must point to its length in readable UTF-8 bytes, unless the length is 0 - an
/// empty `ReadOnlySpan<byte>` marshals as a null pointer, which must not be rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_log_emit(
    category_ptr: *const u8,
    category_len: usize,
    message_ptr: *const u8,
    message_len: usize,
    exception_ptr: *const u8,
    exception_len: usize,
    thread_name_ptr: *const u8,
    thread_name_len: usize,
    level: i32,
    tid: i32,
    unix_millis: i64,
) -> i32 {
    fn as_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
        if len == 0 {
            return Some("");
        }
        if ptr.is_null() {
            return None;
        }
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }).ok()
    }

    let (Some(category), Some(message), Some(exception), Some(thread_name)) = (
        as_str(category_ptr, category_len),
        as_str(message_ptr, message_len),
        as_str(exception_ptr, exception_len),
        as_str(thread_name_ptr, thread_name_len),
    ) else {
        return STATUS_BAD_ARGS;
    };

    let Some(level) = LogLevel::from_i32(level) else {
        return STATUS_BAD_ARGS;
    };

    let record = LogRecord {
        category,
        message,
        exception,
        thread_name,
        level,
        tid,
        unix_millis,
    };

    match catch_unwind(AssertUnwindSafe(|| {
        if let Some(logger) = logger_guard().1.as_ref() {
            logger.emit(&record);
        }
    })) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_PANIC,
    }
}

/// Drops one `spt_logger_init` reference; on the last one, flushes every sink and joins the writer
/// threads. Closing more often than init was called is an idempotent `STATUS_OK` no-op, and a later
/// `spt_logger_init` re-initialises from zero.
///
/// # Safety
/// No pointer arguments; marked unsafe only for symmetry with the export family.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_logger_close() -> i32 {
    match catch_unwind(AssertUnwindSafe(|| {
        // The guard drops at the end of this block: the writer-thread join below must not hold the
        // lock, or a concurrent emit blocks until the join finishes.
        let taken = {
            let mut guard = logger_guard();
            match guard.0 {
                0 => None,
                1 => {
                    guard.0 = 0;
                    guard.1.take()
                }
                count => {
                    guard.0 = count - 1;
                    None
                }
            }
        };
        if let Some(logger) = taken {
            logger.close();
        }
    })) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_PANIC,
    }
}

/// Stores the resolved server-locale table generator diagnostics render against. Overwrites any
/// previous table — the prepatch host pushing twice is harmless. On `STATUS_ERROR` the parse-error
/// text is in the out-buffer and the previously stored table (if any) is untouched; generator
/// lines then fall back to their locale keys, which must not stop the server.
///
/// # Safety
/// `json_ptr` must point to `json_len` readable bytes of UTF-8; `out_ptr` and `out_len` must be
/// valid for writes. A returned buffer is released with `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_locales_set(
    json_ptr: *const u8,
    json_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if out_ptr.is_null() || out_len.is_null() {
        return STATUS_BAD_ARGS;
    }

    // Zeroed before the `json_ptr` guard: a bad-args return still leaves the caller's out-params
    // written, never stale.
    unsafe {
        *out_ptr = std::ptr::null_mut();
        *out_len = 0;
    }

    if json_ptr.is_null() {
        return STATUS_BAD_ARGS;
    }

    let bytes = unsafe { std::slice::from_raw_parts(json_ptr, json_len) };

    match catch_unwind(AssertUnwindSafe(|| {
        serde_json::from_slice::<std::collections::HashMap<String, String>>(bytes)
            .map(crate::diag::set_locales)
            .map_err(|error| format!("locale table did not parse: {error}"))
    })) {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(error)) => {
            unsafe { write_buffer(error.into_bytes(), out_ptr, out_len) };
            STATUS_ERROR
        }
        Err(_) => STATUS_PANIC,
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
            16,
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
            "weaponModEquipmentBlacklist":{{}},
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

    /// Every required `InvariantSlice` member, braces included. The two price tables always know
    /// `SELLABLE_TPL`, which only matters when the items view carries it.
    fn ragfair_invariant(items: &str, offer_item_count: &str) -> String {
        let dynamic = ragfair_dynamic(offer_item_count);
        format!(
            r#"{{"dynamic":{dynamic},
            "itemPresets":{{}},"defaultPresets":[],"defaultPresetsByTpl":{{}},"presetsByTpl":{{}},
            "fleaPrices":{{"{SELLABLE_TPL}":25000}},"handbookPrices":{{"{SELLABLE_TPL}":20000}},
            "highestTraderPrices":{{"{SELLABLE_TPL}":12000}},"configBlacklist":[],
            "seasonalEventActive":false,"seasonalItemTplBlacklist":[],
            "pmcNamesUsec":["Deagle"],"pmcNamesBear":["Kirill"],"items":{items}}}"#
        )
    }

    /// The varying half, the same for every fixture here.
    const RAGFAIR_VARYING: &str = r#""varying":{"timestamp":1700000000,"offerCounterStart":0}"#;

    /// Every required `GenerateDynamicOffersRequest` member, slice included.
    fn ragfair_request_with(items: &str, offer_item_count: &str) -> String {
        let invariant = ragfair_invariant(items, offer_item_count);
        format!(r#"{{"invariantStamp":0,{RAGFAIR_VARYING},"invariant":{invariant}}}"#)
    }

    /// The minimal request at an arbitrary stamp, with the slice sent or omitted — the two halves
    /// of the cache gate.
    fn ragfair_request_with_stamp(stamp: i64, include_slice: bool) -> String {
        if !include_slice {
            return format!(r#"{{"invariantStamp":{stamp},{RAGFAIR_VARYING}}}"#);
        }
        let invariant = ragfair_invariant("{}", r#"{"default":{"min":2,"max":5}}"#);

        format!(r#"{{"invariantStamp":{stamp},{RAGFAIR_VARYING},"invariant":{invariant}}}"#)
    }

    /// The one tpl the offer path would accept, were it in the items view.
    const SELLABLE_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";

    /// Every dynamic-offers export call writes the one static slice slot, so they run one at a time.
    fn cache_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::ragfair::slice_cache::tests::CACHE_TEST_LOCK
            .lock()
            .unwrap()
    }

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
        // Every full send stores its slice, so this shares the one static slot with the cache test.
        let _guard = cache_lock();
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
        let _guard = cache_lock();
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
        let _guard = cache_lock();
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
    fn ragfair_slice_less_request_hits_the_cache_or_reports_stale() {
        let _guard = cache_lock();
        // full send stores the slice under stamp 41
        let full = ragfair_request_with_stamp(41, true);
        let (status, _) = call_generate(spt_generate_dynamic_offers, full.as_bytes());
        assert_eq!(status, STATUS_OK);

        // slice-less send with the same stamp generates from the cache
        let hit = ragfair_request_with_stamp(41, false);
        let (status, _) = call_generate(spt_generate_dynamic_offers, hit.as_bytes());
        assert_eq!(status, STATUS_OK);

        // slice-less send with a different stamp is a stale-slice miss
        let miss = ragfair_request_with_stamp(42, false);
        let (status, _) = call_generate(spt_generate_dynamic_offers, miss.as_bytes());
        assert_eq!(status, STATUS_STALE_SLICE);
    }

    /// Every repeatable-quest call writes the quest family's own static slice slot — a different
    /// slot from ragfair's, with its own lock.
    fn quest_cache_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::quest::slice_cache::tests::CACHE_TEST_LOCK
            .lock()
            .unwrap()
    }

    /// A repeatable-quest request at `stamp`, with the slice sent or omitted, off the fixtures the
    /// model tests already pin against the shipped database.
    fn quest_request(stamp: i64, include_slice: bool, seed: Option<u64>) -> Vec<u8> {
        let mut varying = crate::quest::models::tests::varying_value();
        if let Some(seed) = seed {
            varying["seed"] = serde_json::json!(seed);
        }
        let mut request = serde_json::json!({"invariantStamp": stamp, "varying": varying});
        if include_slice {
            request["invariant"] = crate::quest::models::tests::slice_value();
        }

        serde_json::to_vec(&request).expect("request serializes")
    }

    /// Every 24-character hex run in `bytes` — the mongo ids crossing the boundary, whichever way.
    fn mongo_ids(bytes: &[u8]) -> Vec<String> {
        let text = std::str::from_utf8(bytes).expect("the payload is UTF-8");
        let mut ids = Vec::new();
        let mut run = String::new();
        // A trailing separator so a run ending at the last byte is closed like every other.
        for character in text.chars().chain(std::iter::once(' ')) {
            if character.is_ascii_hexdigit() {
                run.push(character);
                continue;
            }
            if run.len() == 24 {
                ids.push(run.clone());
            }
            run.clear();
        }

        ids
    }

    /// Blanks only the ids the generators *minted*: the response ids the request did not carry.
    /// Those come off a process counter and a clock, outside the seeded stream, so no seed pins
    /// them. Every id the request already knew — the trader, the template, the item tpls a reward
    /// draw picks — stays visible, which is where a seed regression has to show. Ids appearing as
    /// a `"_tpl"` value count as known even when the request never carried them (a drawn reward's
    /// template comes from the database, never the mint), so a same-price different-item
    /// regression stays visible too.
    fn mask_minted_ids(out: &[u8], request: &[u8]) -> String {
        let mut known: std::collections::HashSet<String> = mongo_ids(request).into_iter().collect();
        let text = std::str::from_utf8(out).expect("the response is UTF-8");
        for (idx, _) in text.match_indices("\"_tpl\":\"") {
            let value = &text[idx + 8..];
            if value.len() >= 24 && value.as_bytes()[24] == b'"' {
                known.insert(value[..24].to_string());
            }
        }
        let mut masked = String::from_utf8(out.to_vec()).expect("the response is UTF-8");
        for id in mongo_ids(out) {
            if !known.contains(&id) {
                masked = masked.replace(&id, "<minted>");
            }
        }

        masked
    }

    #[test]
    fn a_slice_less_quest_request_hits_the_cache_or_reports_stale() {
        let _guard = quest_cache_lock();
        // full send stores the slice under stamp 41
        let (status, _) = call_generate(
            spt_generate_repeatable_quest,
            &quest_request(41, true, None),
        );
        assert_eq!(status, STATUS_OK);

        // slice-less send with the same stamp generates from the cache
        let (status, _) = call_generate(
            spt_generate_repeatable_quest,
            &quest_request(41, false, None),
        );
        assert_eq!(status, STATUS_OK);

        // slice-less send with a different stamp is a stale-slice miss
        let (status, out) = call_generate(
            spt_generate_repeatable_quest,
            &quest_request(42, false, None),
        );
        assert_eq!(status, STATUS_STALE_SLICE);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "no cached invariant slice for this stamp"
        );
    }

    #[test]
    fn a_quest_generator_throw_returns_status_error_and_the_message() {
        let _guard = quest_cache_lock();
        // The Daily config ships no `Pickup` block, and `PickupQuestGenerator:39` dereferences it
        // unconditionally — the C#-sanctioned throw, ported as a panic.
        let mut request: serde_json::Value =
            serde_json::from_slice(&quest_request(9, true, None)).unwrap();
        request["varying"]["questType"] = serde_json::json!("Pickup");

        let (status, out) = call_generate(
            spt_generate_repeatable_quest,
            &serde_json::to_vec(&request).unwrap(),
        );

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Pickup config was null at PickupQuestGenerator:39"
        );
    }

    #[test]
    fn a_seeded_quest_request_answers_the_same_bytes_twice() {
        let _guard = quest_cache_lock();
        let request = quest_request(7, true, Some(42));

        let (first_status, first) = call_generate(spt_generate_repeatable_quest, &request);
        let (second_status, second) = call_generate(spt_generate_repeatable_quest, &request);

        assert_eq!(first_status, STATUS_OK);
        assert_eq!(second_status, STATUS_OK);
        let response: serde_json::Value = serde_json::from_slice(&first).unwrap();
        assert!(
            response["quest"].is_object(),
            "the fixture request generates a quest"
        );
        assert!(response["pool"]["types"].is_array());
        // The ids a draw can move — the trader the quest is minted for, and the tpls it hands over
        // — come off the request, so the masking leaves them in the compared bytes.
        assert!(
            mask_minted_ids(&first, &request).contains("54cb50c76803fa8b248b4571"),
            "the request's own ids must survive the masking"
        );
        assert_eq!(
            mask_minted_ids(&first, &request),
            mask_minted_ids(&second, &request)
        );

        // …and the masking has teeth: another seed draws a different quest.
        let other_request = quest_request(7, true, Some(7));
        let (status, other) = call_generate(spt_generate_repeatable_quest, &other_request);
        assert_eq!(status, STATUS_OK);
        assert_ne!(
            mask_minted_ids(&first, &request),
            mask_minted_ids(&other, &other_request)
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

    #[test]
    fn logger_exports_roundtrip() {
        /// The messages this test emits; everything else in the file belongs to a generator.
        const MINE: [&str; 4] = ["hello", "nullspans", "still up", "after teardown"];

        let dir = TempDir::new().unwrap();
        let config = format!(
            r#"{{ "loggers": [ {{ "type": "File", "logLevel": "Information",
                "format": "%message%", "filePath": {path:?}, "filePattern": "spt.log",
                "maxFileSizeMB": 10, "maxRollingFiles": 10, "filters": [] }} ] }}"#,
            path = dir.path().display().to_string(),
        );

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        // Bad JSON: error status, message in the buffer, pipeline stays uninitialised.
        let status = unsafe { spt_logger_init(b"nope".as_ptr(), 4, &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_ERROR);
        assert!(!out_ptr.is_null());
        unsafe { spt_buf_free(out_ptr, out_len) };

        // Emit before init is an OK no-op.
        let status = emit("Cat", "dropped", "", "main");
        assert_eq!(status, STATUS_OK);

        // Real init; the second init keeps the running pipeline and takes a reference.
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let status =
            unsafe { spt_logger_init(config.as_ptr(), config.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_OK);
        let status =
            unsafe { spt_logger_init(config.as_ptr(), config.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_OK);

        assert_eq!(emit("Cat", "hello", "", "main"), STATUS_OK);

        // An empty `ReadOnlySpan<byte>` marshals as a null pointer with a zero length - the empty
        // spans must arrive as an empty string, not as a bad argument.
        let status = unsafe {
            spt_log_emit(
                "Cat".as_ptr(),
                3,
                "nullspans".as_ptr(),
                9,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                2, // Information
                1,
                0,
            )
        };
        assert_eq!(status, STATUS_OK);

        // An invalid UTF-8 message is rejected.
        let bad = [0xFFu8, 0xFE];
        let status = unsafe {
            spt_log_emit(
                "Cat".as_ptr(),
                3,
                bad.as_ptr(),
                bad.len(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                2, // Information
                1,
                0,
            )
        };
        assert_eq!(status, STATUS_BAD_ARGS);

        // A bad level is rejected.
        let status = unsafe {
            spt_log_emit(
                "Cat".as_ptr(),
                3,
                "x".as_ptr(),
                1,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                42,
                1,
                0,
            )
        };
        assert_eq!(status, STATUS_BAD_ARGS);

        // The first close drops the second init's reference - the nested `Program.Main` disposing
        // its container must not take the outer host's logging down with it.
        assert_eq!(unsafe { spt_logger_close() }, STATUS_OK);
        assert_eq!(emit("Cat", "still up", "", "main"), STATUS_OK);

        // The matching close flushes and tears down; further closes are no-ops.
        assert_eq!(unsafe { spt_logger_close() }, STATUS_OK);
        assert_eq!(unsafe { spt_logger_close() }, STATUS_OK);
        assert_eq!(emit("Cat", "after teardown", "", "main"), STATUS_OK);

        // Generators in other tests emit their diagnostics through the same process-global
        // pipeline, so only this test's own lines can be asserted on.
        let contents = fs::read_to_string(dir.path().join("spt.log")).unwrap();
        let mine: Vec<&str> = contents
            .lines()
            .filter(|line| MINE.contains(line))
            .collect();
        assert_eq!(mine, ["hello", "nullspans", "still up"]);
    }

    fn emit(category: &str, message: &str, exception: &str, tname: &str) -> i32 {
        unsafe {
            spt_log_emit(
                category.as_ptr(),
                category.len(),
                message.as_ptr(),
                message.len(),
                exception.as_ptr(),
                exception.len(),
                tname.as_ptr(),
                tname.len(),
                2, // Information
                1,
                0,
            )
        }
    }
}
