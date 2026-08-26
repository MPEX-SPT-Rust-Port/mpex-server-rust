use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use rayon::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::base_class::{self, BaseClassRequest, BaseClassResponse};
use crate::bot::bot_inventory_generator::{generate_inventory, generate_inventory_batch};
use crate::diag::DiagSink;
use crate::linked_items::{self, LinkedItemsRequest, LinkedItemsResponse};
use crate::logger::{ConsoleMessage, LogLevel, LogRecord, Logger, compile_format, render};
use crate::loot::item_helper::{LootEpochError, LootError};
use crate::loot::location_loot_generator::{generate_dynamic_loot, generate_static_containers};
use crate::loot::loot_generator::{
    create_forced_loot, create_random_loot, get_random_loot_container_loot,
    get_sealed_weapon_case_loot,
};
use crate::quest::{QuestError, generate_repeatable_quest};
use crate::ragfair::models::{DynamicOffersHeader, DynamicOffersResult};
use crate::ragfair::offer_generator::{RagfairError, generate_dynamic_offers};
use crate::runtime::runtime;
use crate::scav_case::{ScavCaseError, generate_scav_case_rewards};
use crate::verify;

pub const STATUS_OK: i32 = 0;
pub const STATUS_BAD_ARGS: i32 = 1;
pub const STATUS_PANIC: i32 = 2;
/// The call failed - generation, or the disk, for the profile exports: the error message, not a
/// result, is in the out-buffer.
pub const STATUS_ERROR: i32 = 3;
/// A request named a resident-DB epoch this process does not hold; the caller republishes and
/// retries.
pub const STATUS_STALE_EPOCH: i32 = 4;

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

impl FfiFailure for LootEpochError {
    fn status(&self) -> i32 {
        match self {
            LootEpochError::Loot(_) => STATUS_ERROR,
            LootEpochError::StaleEpoch => STATUS_STALE_EPOCH,
        }
    }

    fn into_message(self) -> String {
        match self {
            LootEpochError::Loot(error) => error.message,
            LootEpochError::StaleEpoch => {
                "resident DB epoch mismatch; republish and retry".to_string()
            }
        }
    }
}

impl FfiFailure for RagfairError {
    fn status(&self) -> i32 {
        match self {
            RagfairError::Loot(_) => STATUS_ERROR,
            RagfairError::StaleEpoch => STATUS_STALE_EPOCH,
        }
    }

    fn into_message(self) -> String {
        match self {
            RagfairError::Loot(error) => error.message,
            RagfairError::StaleEpoch => {
                "resident DB epoch mismatch; republish and retry".to_string()
            }
        }
    }
}

impl FfiFailure for QuestError {
    fn status(&self) -> i32 {
        match self {
            QuestError::Failed(_) => STATUS_ERROR,
            QuestError::StaleEpoch => STATUS_STALE_EPOCH,
        }
    }

    fn into_message(self) -> String {
        match self {
            QuestError::Failed(message) => message,
            QuestError::StaleEpoch => "resident DB epoch mismatch; republish and retry".to_string(),
        }
    }
}

impl FfiFailure for ScavCaseError {
    fn status(&self) -> i32 {
        match self {
            ScavCaseError::Failed(_) => STATUS_ERROR,
            ScavCaseError::StaleEpoch => STATUS_STALE_EPOCH,
        }
    }

    fn into_message(self) -> String {
        match self {
            ScavCaseError::Failed(message) => message,
            ScavCaseError::StaleEpoch => {
                "resident DB epoch mismatch; republish and retry".to_string()
            }
        }
    }
}

impl FfiFailure for crate::db::StaleEpoch {
    fn status(&self) -> i32 {
        STATUS_STALE_EPOCH
    }

    fn into_message(self) -> String {
        "resident DB epoch mismatch; republish and retry".to_string()
    }
}

impl FfiFailure for crate::db::PublishError {
    fn status(&self) -> i32 {
        STATUS_ERROR
    }

    fn into_message(self) -> String {
        match self {
            crate::db::PublishError::Schema(message) | crate::db::PublishError::Views(message) => {
                message
            }
        }
    }
}

impl FfiFailure for crate::db::load::LoadError {
    fn status(&self) -> i32 {
        match self {
            crate::db::load::LoadError::BadArgs(_) => STATUS_BAD_ARGS,
            _ => STATUS_ERROR,
        }
    }

    fn into_message(self) -> String {
        match self {
            // Both already name their culprit: the schema number, or the path that would not read.
            crate::db::load::LoadError::BadArgs(message)
            | crate::db::load::LoadError::Io(message) => message,
            crate::db::load::LoadError::Publish(error) => error.into_message(),
        }
    }
}

impl FfiFailure for crate::profile::ProfileError {
    fn status(&self) -> i32 {
        match self {
            crate::profile::ProfileError::BadArgs(_) => STATUS_BAD_ARGS,
            crate::profile::ProfileError::Io(_) => STATUS_ERROR,
        }
    }

    fn into_message(self) -> String {
        match self {
            // Both already name their culprit: the id/schema, or the path that failed.
            crate::profile::ProfileError::BadArgs(message)
            | crate::profile::ProfileError::Io(message) => message,
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

/// The text a caught panic carries — `panic!`/`expect` payloads are a `String` or a `&str`.
/// Anything else gets fixed fallback text; the payload type is not worth reporting.
pub(crate) fn panic_message(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "native call panicked with a non-string payload".to_owned())
}

/// The shared body of the bot generation exports: JSON request in, status plus either the JSON
/// result or an error message out. Runs on the calling thread. Generator errors arrive wrapped as
/// [`LootEpochError::Loot`]; an override-less request naming a non-resident epoch is
/// [`LootEpochError::StaleEpoch`] → `STATUS_STALE_EPOCH`.
///
/// # Safety
/// As documented on the exports below.
unsafe fn run_generator<Request, Response>(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    generate: fn(Request) -> Result<Response, LootEpochError>,
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
        // Diagnostics emitted before the failure are already in the log (DiagSink emits live);
        // only the failure message itself needs to cross.
        Ok(Err(error)) => {
            let status = error.status();
            unsafe { write_buffer(error.into_message().into_bytes(), out_ptr, out_len) };

            status
        }
        Err(payload) => {
            unsafe { write_buffer(panic_message(payload).into_bytes(), out_ptr, out_len) };

            STATUS_PANIC
        }
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
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            generate_static_containers,
            |response| serde_json::to_vec(&response).expect("result serialization cannot fail"),
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
    unsafe {
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            generate_dynamic_loot,
            |response| serde_json::to_vec(&response).expect("result serialization cannot fail"),
        )
    }
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
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            |request| create_random_loot(request, &mut DiagSink::Pipeline),
            |response| serde_json::to_vec(&response).expect("result serialization cannot fail"),
        )
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
    unsafe {
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            create_forced_loot,
            |response| serde_json::to_vec(&response).expect("result serialization cannot fail"),
        )
    }
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
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            |request| get_sealed_weapon_case_loot(request, &mut DiagSink::Pipeline),
            |response| serde_json::to_vec(&response).expect("result serialization cannot fail"),
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
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            get_random_loot_container_loot,
            |response| serde_json::to_vec(&response).expect("result serialization cannot fail"),
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

/// One repeatable quest, off the resident DB's derived views or the override this request
/// carries.
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

/// One scav case craft's rewards.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_scav_case_rewards(
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
            |request| generate_scav_case_rewards(request, &mut DiagSink::Pipeline),
            |response| {
                serde_json::to_vec(&response).expect("scav case response serialization cannot fail")
            },
        )
    }
}

/// The success envelope of `spt_build_item_base_class_cache`: the response under `result`, the
/// same shape `ScavCaseResponse` puts on the wire.
#[derive(Serialize)]
struct BaseClassEnvelope {
    result: BaseClassResponse,
}

/// The whole `_itemBaseClassesCache` in one call, off the resident templates root or the override
/// this request carries. An override-less request naming an epoch the process does not hold is
/// `STATUS_STALE_EPOCH`.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_build_item_base_class_cache(
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
            |request: BaseClassRequest| {
                base_class::run(request).map(|result| BaseClassEnvelope { result })
            },
            |envelope| {
                serde_json::to_vec(&envelope)
                    .expect("base class response serialization cannot fail")
            },
        )
    }
}

/// The success envelope of `spt_build_ragfair_linked_item_table`: the response under `result`,
/// the same shape `BaseClassEnvelope` puts on the wire.
#[derive(Serialize)]
struct LinkedItemsEnvelope {
    result: LinkedItemsResponse,
}

/// The whole `linkedItemsCache` in one call, off the resident templates root or the override
/// this request carries. An override-less request naming an epoch the process does not hold is
/// `STATUS_STALE_EPOCH`.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_build_ragfair_linked_item_table(
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
            |request: LinkedItemsRequest| {
                linked_items::run(request).map(|result| LinkedItemsEnvelope { result })
            },
            |envelope| {
                serde_json::to_vec(&envelope)
                    .expect("linked items response serialization cannot fail")
            },
        )
    }
}

/// Installs a publish envelope's roots into the process-global resident DB and answers the new
/// epoch as `{"epoch":N}`. A parse failure (including an unknown root name) is `STATUS_BAD_ARGS`;
/// a schema or view-derivation failure is `STATUS_ERROR` and leaves the previous DB intact.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_db_publish(
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
            crate::db::publish,
            |epoch| format!("{{\"epoch\":{epoch}}}").into_bytes(),
        )
    }
}

/// Per-root canonical digests of the resident DB's typed lift surface,
/// `{"epoch":N,"roots":{…}}` — `{"epoch":0,"roots":{}}` before the first publish. Digests are
/// stable within a toolchain but no wire contract: compare two calls within one process, never
/// across builds or machines. Test support for the load/projection equivalence gate.
///
/// # Safety
/// `out_ptr` and `out_len` must be valid for writes; the buffer must be released with
/// `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_db_resident_digest(out_ptr: *mut *mut u8, out_len: *mut usize) -> i32 {
    if out_ptr.is_null() || out_len.is_null() {
        return STATUS_BAD_ARGS;
    }
    match catch_unwind(AssertUnwindSafe(crate::db::resident_digests_json)) {
        Ok(json) => {
            unsafe { write_buffer(json, out_ptr, out_len) };

            STATUS_OK
        }
        Err(payload) => {
            unsafe { write_buffer(panic_message(payload).into_bytes(), out_ptr, out_len) };

            STATUS_PANIC
        }
    }
}

/// The framed load response: `[u32-LE header length][header JSON][blob 0][blob 1]…`, the blobs in
/// `files[]` order. Framed rather than one JSON document so the file bytes cross as bytes — the
/// eager tree is tens of megabytes of JSON that C# re-parses itself.
fn encode_load_response(response: crate::db::load::LoadResponse) -> Vec<u8> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Header<'a> {
        epoch: Option<u64>,
        verify: &'a Option<crate::verify::VerifyReport>,
        files: Vec<FileEntry<'a>>,
    }

    #[derive(Serialize)]
    struct FileEntry<'a> {
        path: &'a str,
        len: usize,
    }

    let header = serde_json::to_vec(&Header {
        epoch: response.epoch,
        verify: &response.verify,
        files: response
            .files
            .iter()
            .map(|(path, bytes)| FileEntry {
                path,
                len: bytes.len(),
            })
            .collect(),
    })
    .expect("load header serialization cannot fail");

    let blob_bytes: usize = response.files.iter().map(|(_, bytes)| bytes.len()).sum();
    let mut out = Vec::with_capacity(4 + header.len() + blob_bytes);
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    for (_, bytes) in &response.files {
        out.extend_from_slice(bytes);
    }

    out
}

/// `db::load::load` is async and CLR-free; the export is the only place a runtime is needed.
fn db_load_blocking(
    request: crate::db::load::LoadRequest,
) -> Result<crate::db::load::LoadResponse, crate::db::load::LoadError> {
    runtime().block_on(crate::db::load::load(request))
}

/// Fused SPT_Data load (state-ownership Phase 3): one walk hashes, reads, installs the resident
/// roots and hands the eager file bytes back. Request/response contract in `db/load.rs`.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_db_load(
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
            db_load_blocking,
            encode_load_response,
        )
    }
}

/// Every file name in the profiles directory as `{"files":[…]}`, the directory created when
/// missing. Contract in `profile.rs`.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_profile_list(
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
            crate::profile::list,
            crate::profile::encode_list,
        )
    }
}

/// One profile's bytes, framed `[u32-LE header length][header JSON][file bytes]` with the header
/// `{"found":true}` or `{"found":false}` — the bytes cross as bytes, never as an escaped string.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_profile_load(
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
            crate::profile::load,
            crate::profile::encode_load_frame,
        )
    }
}

/// Writes one profile through the temp-then-rename protocol; answers the empty envelope `{}`.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_profile_save(
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
            crate::profile::save,
            |()| b"{}".to_vec(),
        )
    }
}

/// Removes one profile; `{"deleted":true}` when the file was there, `{"deleted":false}` when it
/// was not. A missing file is not an error.
///
/// # Safety
/// See `spt_generate_static_containers`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_profile_delete(
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
            crate::profile::delete,
            |deleted| format!("{{\"deleted\":{deleted}}}").into_bytes(),
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

/// A clone of the running pipeline's console channel, if it has one. Cloned under the lock and
/// used outside it, so a blocking Raw send never holds LOGGER against emitters.
fn console_sender() -> Option<std::sync::mpsc::SyncSender<ConsoleMessage>> {
    logger_guard().1.as_ref().and_then(Logger::console_sender)
}

/// Raw bytes to the terminal: through the console sink's queue when one is running (total order
/// with log lines, blocking — a prompt must not drop), straight to stdout otherwise (before
/// init, after close, no console target configured, or the sink just closed under us).
pub(crate) fn console_write_stdout(bytes: Vec<u8>) {
    let bytes = match console_sender() {
        Some(sender) => match sender.send(ConsoleMessage::Raw(bytes)) {
            Ok(()) => return,
            Err(std::sync::mpsc::SendError(ConsoleMessage::Raw(bytes))) => bytes,
            Err(_) => return,
        },
        None => bytes,
    };
    let mut out = std::io::stdout();
    let _ = out.write_all(&bytes);
    let _ = out.flush();
}

/// Drain barrier before a stdin read: everything queued before this call is on the terminal when
/// it returns. Without a running console sink there is nothing queued to wait for.
fn console_flush() {
    if let Some(sender) = console_sender() {
        let (ack_sender, ack_receiver) = std::sync::mpsc::sync_channel::<()>(1);
        if sender.send(ConsoleMessage::Flush(ack_sender)).is_ok() {
            let _ = ack_receiver.recv();
        }
    }
}

/// The C# callback receiving Rust-originated log lines, so mod-registered ILogHandlers see the
/// full stream and not only what crossed spt_log_emit. Spans are category, message, thread
/// name; scalars are level, tid, unix-millis. Buffers are valid only for the call.
type LogTap =
    unsafe extern "C" fn(*const u8, usize, *const u8, usize, *const u8, usize, i32, i32, i64);

/// Poison-tolerant like `logger_guard` — a panicked taker must not kill logging.
static LOG_TAP: Mutex<Option<LogTap>> = Mutex::new(None);

fn log_tap() -> Option<LogTap> {
    *LOG_TAP
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
    // Rust-originated lines only: C#-originated lines fan out to handlers on the C# side, with
    // their original Exception object.
    // The tap must fire before LOGGER is taken - a handler may call straight back into spt_log_emit.
    if let Some(tap) = log_tap() {
        unsafe {
            tap(
                record.category.as_ptr(),
                record.category.len(),
                record.message.as_ptr(),
                record.message.len(),
                record.thread_name.as_ptr(),
                record.thread_name.len(),
                record.level as i32,
                record.tid,
                record.unix_millis,
            )
        };
    }
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
        crate::console::init_terminal();
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
        Err(payload) => {
            unsafe { write_buffer(panic_message(payload).into_bytes(), out_ptr, out_len) };

            STATUS_PANIC
        }
    }
}

/// Replaces the running pipeline's configuration in place: the C# side's answer to a mod mutating
/// `SptLoggerConfiguration.Loggers` at runtime. Parse failure (`STATUS_ERROR`, message in the
/// out-buffer) leaves the running pipeline untouched, as does calling before `spt_logger_init` has
/// run. The init ref-count is untouched — a reinit is not an init. New sinks open under the
/// pipeline lock, so a same-path target reopens in append mode (`freshened_paths`) rather than
/// cascading the archives a second time; the old sinks flush and join after the swap.
///
/// # Safety
/// `config_ptr` must point to `config_len` readable bytes of UTF-8; `out_ptr` and `out_len` must
/// be valid for writes. A returned buffer is released with `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_logger_reinit(
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
        // Parsing and opening the new sinks under the lock keeps the swap atomic against emits -
        // reinit is rare, so briefly blocking the pipeline is the cheap side of that trade.
        let mut guard = logger_guard();
        if guard.1.is_none() {
            return Err(
                "the log pipeline is not initialised; call spt_logger_init first".to_owned(),
            );
        }
        let new_logger = Logger::from_json(bytes)?;
        let old = guard.1.replace(new_logger);
        drop(guard);
        // Join the old writer threads outside the lock, same rule as `spt_logger_close`.
        if let Some(old) = old {
            old.close();
        }
        Ok(())
    })) {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(error)) => {
            unsafe { write_buffer(error.into_bytes(), out_ptr, out_len) };
            STATUS_ERROR
        }
        Err(payload) => {
            unsafe { write_buffer(panic_message(payload).into_bytes(), out_ptr, out_len) };

            STATUS_PANIC
        }
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

/// Registers — or with a null pointer clears — the process-wide tap receiving Rust-originated
/// log lines. Independent of pipeline init state: a tap set before `spt_logger_init` still
/// receives generator lines.
///
/// # Safety
/// A non-null `tap` must stay callable until cleared or process exit (C# roots the delegate),
/// and must not unwind. The spans it receives are valid only for the duration of each call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_log_set_tap(tap: Option<LogTap>) -> i32 {
    match catch_unwind(AssertUnwindSafe(|| {
        *LOG_TAP
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = tap;
    })) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_PANIC,
    }
}

/// Verbatim byte passthrough from C#'s redirected Console.Out/Error — the raw write sites that
/// live outside the log pipeline. stdout bytes queue behind the console sink (never dropped,
/// ordered against log lines); stderr bytes write directly, because the stderr path is the
/// failure channel of last resort and must not depend on a possibly-broken pipeline.
///
/// # Safety
/// `bytes_ptr` must point to `bytes_len` readable bytes unless `bytes_len` is 0. The bytes are
/// copied before return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_console_write(
    bytes_ptr: *const u8,
    bytes_len: usize,
    to_stderr: i32,
) -> i32 {
    let bytes: &[u8] = if bytes_len == 0 {
        &[]
    } else if bytes_ptr.is_null() {
        return STATUS_BAD_ARGS;
    } else {
        unsafe { std::slice::from_raw_parts(bytes_ptr, bytes_len) }
    };

    match catch_unwind(AssertUnwindSafe(|| {
        if to_stderr != 0 {
            let mut err = std::io::stderr();
            let _ = err.write_all(bytes);
            let _ = err.flush();
        } else {
            console_write_stdout(bytes.to_vec());
        }
    })) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_PANIC,
    }
}

/// Flushes the console queue, then reads one line from stdin — C#'s Console.ReadLine, with the
/// prompt guaranteed visible first. EOF answers STATUS_OK with a null buffer (C#'s null); note an
/// empty line may also surface as a null buffer, which every current caller ignores anyway. On a
/// read error the error text is in the buffer with STATUS_ERROR.
///
/// # Safety
/// `out_ptr` and `out_len` must be valid for writes. A returned buffer is released with
/// `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_console_read_line(out_ptr: *mut *mut u8, out_len: *mut usize) -> i32 {
    if out_ptr.is_null() || out_len.is_null() {
        return STATUS_BAD_ARGS;
    }

    unsafe {
        *out_ptr = std::ptr::null_mut();
        *out_len = 0;
    }

    match catch_unwind(AssertUnwindSafe(|| {
        console_flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => Ok(Option::None),
            Ok(_) => Ok(Some(crate::console::strip_line_ending(line))),
            Err(error) => Err(error.to_string()),
        }
    })) {
        Ok(Ok(Option::None)) => STATUS_OK,
        Ok(Ok(Some(line))) => {
            if !line.is_empty() {
                unsafe { write_buffer(line.into_bytes(), out_ptr, out_len) };
            }
            STATUS_OK
        }
        Ok(Err(error)) => {
            unsafe { write_buffer(error.into_bytes(), out_ptr, out_len) };
            STATUS_ERROR
        }
        Err(payload) => {
            unsafe { write_buffer(panic_message(payload).into_bytes(), out_ptr, out_len) };
            STATUS_PANIC
        }
    }
}

/// C#'s `Console.Title`. Windows sets it through the console API; elsewhere an OSC 0 escape is
/// queued behind the console sink when stdout is a terminal, and silently skipped when not.
///
/// # Safety
/// `title_ptr` must point to `title_len` readable bytes of UTF-8 unless `title_len` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_console_set_title(title_ptr: *const u8, title_len: usize) -> i32 {
    let title = if title_len == 0 {
        ""
    } else if title_ptr.is_null() {
        return STATUS_BAD_ARGS;
    } else {
        match std::str::from_utf8(unsafe { std::slice::from_raw_parts(title_ptr, title_len) }) {
            Ok(title) => title,
            Err(_) => return STATUS_BAD_ARGS,
        }
    };

    match catch_unwind(AssertUnwindSafe(|| {
        if let Some(bytes) = crate::console::set_title(title) {
            console_write_stdout(bytes);
        }
    })) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_PANIC,
    }
}

/// C#'s `Console.Clear()`, tty-gated like its `IsOutputRedirected` guard was: a clear escape
/// queued behind the console sink, or nothing when stdout is not a terminal.
///
/// # Safety
/// No pointer arguments; marked unsafe only for symmetry with the export family.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_console_clear() -> i32 {
    match catch_unwind(AssertUnwindSafe(|| {
        if let Some(bytes) = crate::console::clear() {
            console_write_stdout(bytes);
        }
    })) {
        Ok(()) => STATUS_OK,
        Err(_) => STATUS_PANIC,
    }
}

/// The `IsLogEnabled` gate against the *applied* configuration. NOT a STATUS_* export — the
/// return is tri-state: 1 the level is admitted by some target, 0 it is not, -1 there is no
/// running pipeline (or the level is not a LogLevel), in which case the C# side falls back to
/// its own configuration object so handler-only fan-out keeps working.
#[unsafe(no_mangle)]
pub extern "C" fn spt_log_enabled(level: i32) -> i32 {
    let Some(level) = LogLevel::from_i32(level) else {
        return -1;
    };

    match catch_unwind(AssertUnwindSafe(|| {
        logger_guard()
            .1
            .as_ref()
            .map(|logger| logger.enabled(level))
    })) {
        Ok(Some(true)) => 1,
        Ok(Some(false)) => 0,
        Ok(None) | Err(_) => -1,
    }
}

/// Renders one line with the pipeline's token expansion — the body of C#'s
/// `BaseLogHandler.FormatMessage`, minus the exception append, which stays on the C# side (it is
/// a plain concat and the Exception object never crosses the boundary). Stateless: works with or
/// without a running pipeline.
///
/// # Safety
/// Each pointer must point to its length in readable UTF-8 bytes, unless the length is 0.
/// `out_ptr`/`out_len` must be valid for writes; a returned buffer is released with
/// `spt_buf_free`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn spt_log_format(
    format_ptr: *const u8,
    format_len: usize,
    message_ptr: *const u8,
    message_len: usize,
    logger_ptr: *const u8,
    logger_len: usize,
    thread_name_ptr: *const u8,
    thread_name_len: usize,
    level: i32,
    tid: i32,
    unix_millis: i64,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if out_ptr.is_null() || out_len.is_null() {
        return STATUS_BAD_ARGS;
    }

    unsafe {
        *out_ptr = std::ptr::null_mut();
        *out_len = 0;
    }

    fn as_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
        if len == 0 {
            return Some("");
        }
        if ptr.is_null() {
            return None;
        }
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }).ok()
    }

    let (Some(format), Some(message), Some(logger), Some(thread_name)) = (
        as_str(format_ptr, format_len),
        as_str(message_ptr, message_len),
        as_str(logger_ptr, logger_len),
        as_str(thread_name_ptr, thread_name_len),
    ) else {
        return STATUS_BAD_ARGS;
    };

    let Some(level) = LogLevel::from_i32(level) else {
        return STATUS_BAD_ARGS;
    };

    let record = LogRecord {
        category: logger,
        message,
        exception: "",
        thread_name,
        level,
        tid,
        unix_millis,
    };

    match catch_unwind(AssertUnwindSafe(|| {
        render(&compile_format(format), &record).into_bytes()
    })) {
        Ok(bytes) => {
            unsafe { write_buffer(bytes, out_ptr, out_len) };
            STATUS_OK
        }
        Err(payload) => {
            unsafe { write_buffer(panic_message(payload).into_bytes(), out_ptr, out_len) };
            STATUS_PANIC
        }
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
        Err(payload) => {
            unsafe { write_buffer(panic_message(payload).into_bytes(), out_ptr, out_len) };

            STATUS_PANIC
        }
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
            33,
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
    /// `VIEWS_JSON`, which cannot interpolate a const.
    const CONTAINER_TPL: &str = "111111111111111111111111";

    /// Every required `LootVarying` member, spliced into the request literals below.
    const VARYING_JSON: &str = r#"
        "locationId":"bigmap",
        "moneyTpls":[],"staticAmmoDist":{},
        "staticLootMultiplier":1,"looseLootMultiplier":1,
        "seasonal":{"seasonalEventActive":false,"christmasEventEnabled":false,
            "inactiveSeasonalItems":[]},
        "lootableItemBlacklist":[],"counter":{"maxCounts":{},"trackedCounts":{}}
    "#;

    /// The four `LootViewsWire` members every override send carries.
    const VIEWS_JSON: &str = r#"
        "itemsView":{"111111111111111111111111":{"width":1,"height":1,"gridCellsH":2,"gridCellsV":2}},
        "defaultPresets":{},
        "config":{"containerRandomisationEnabled":false,"locationInRandomisationMaps":false,
            "containerTypesToNotRandomise":[],"containerGroupMinSizeMultiplier":1,
            "containerGroupMaxSizeMultiplier":1,"allowDuplicateItemsInStaticContainers":true,
            "tplsToStripChildItemsFrom":[],"fitLootIntoContainerAttempts":3,
            "magazineLootHasAmmoChancePercent":0,"staticMagazineLootHasAmmoChancePercent":0,
            "minFillLooseMagazinePercent":0,"minFillStaticMagazinePercent":0,
            "staticLootMultiplier":1,"looseLootMultiplier":1,"modSpawnChancePercent":{},
            "looseLootBlacklist":[]},
        "christmasContainerIds":[]
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
            r#"{{"epoch":0,"viewsOverride":{{{VIEWS_JSON},"staticWeapons":[],"staticContainers":[],
            "staticForced":[],"staticLootDist":{{}}}},"varying":{{{VARYING_JSON}}}}}"#
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
            r#"{{"epoch":0,"viewsOverride":{{{VIEWS_JSON}}},"varying":{{{VARYING_JSON},
            "looseLoot":{{"spawnpointCount":{{"mean":0,"std":0}},
            "spawnpointsForced":[],"spawnpoints":[]}}}}}}"#
        );

        let (status, out) = call_generate(spt_generate_dynamic_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["spawnpoints"], serde_json::json!([]));
        assert_eq!(result["trackedCounts"], serde_json::json!({}));
    }

    #[test]
    fn a_stale_loot_epoch_returns_status_4() {
        // No viewsOverride and an epoch the store can never hold (u64::MAX is unreachable —
        // epochs count up from 1). The varying block is the fixture's, so the JSON fully parses
        // and the epoch check in resolve_views is what answers.
        let request = format!(
            r#"{{"epoch":18446744073709551615,"varying":{{{VARYING_JSON},
            "looseLoot":{{"spawnpointCount":{{"mean":0,"std":0}},
            "spawnpointsForced":[],"spawnpoints":[]}}}}}}"#
        );

        let (status, out) = call_generate(spt_generate_dynamic_loot, request.as_bytes());

        assert_eq!(status, STATUS_STALE_EPOCH);
        assert!(String::from_utf8(out).unwrap().contains("epoch mismatch"));
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
            r#"{{"epoch":0,"viewsOverride":{{{VIEWS_JSON},
            "staticWeapons":[],"staticForced":[],"staticLootDist":{{}},
            "staticContainers":[{{"probability":1,"template":{{"Id":"c1","IsContainer":true,
                "Root":"aaaaaaaaaaaaaaaaaaaaaaaa",
                "Items":[{{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"{CONTAINER_TPL}"}}]}}}}]}},
            "varying":{{{VARYING_JSON}}}}}"#
        );

        let (status, out) = call_generate(spt_generate_static_containers, request.as_bytes());

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("Container: {CONTAINER_TPL} is missing from staticLoot.json")
        );
    }

    #[test]
    fn a_panicking_generator_returns_status_panic_and_the_message() {
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        // No generator family has a reachable panic (C# crash-equivalents were ported as
        // `LootError`), so the envelope is driven directly with a panicking generate fn. The
        // literal payload exercises the `&str` downcast; the quest roundtrip test's `.expect`
        // covers the `String` one.
        let status = unsafe {
            run_generator(
                b"{}".as_ptr(),
                2,
                &mut out_ptr,
                &mut out_len,
                |_: serde_json::Value| -> Result<serde_json::Value, LootEpochError> {
                    panic!("kaboom: it went sideways")
                },
            )
        };

        assert_eq!(status, STATUS_PANIC);
        assert!(!out_ptr.is_null(), "the panic message buffer is missing");
        let message = unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec();
        unsafe { spt_buf_free(out_ptr, out_len) };
        assert_eq!(
            String::from_utf8(message).unwrap(),
            "kaboom: it went sideways"
        );
    }

    #[test]
    fn a_non_string_panic_payload_crosses_as_the_fallback_text() {
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let status = unsafe {
            run_generator(
                b"{}".as_ptr(),
                2,
                &mut out_ptr,
                &mut out_len,
                |_: serde_json::Value| -> Result<serde_json::Value, LootEpochError> {
                    std::panic::panic_any(42)
                },
            )
        };

        assert_eq!(status, STATUS_PANIC);
        assert!(!out_ptr.is_null(), "the panic message buffer is missing");
        let message = unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec();
        unsafe { spt_buf_free(out_ptr, out_len) };
        assert_eq!(
            String::from_utf8(message).unwrap(),
            "native call panicked with a non-string payload"
        );
    }

    /// The `RewardViewsWire` members every override reward send carries; sealed adds
    /// `presetsByTpl` and container adds `presetTpls` beside these.
    const REWARD_VIEWS_JSON: &str = r#"
        "itemsView":{},"defaultPresets":[],"defaultPresetsByTpl":{},
        "configBlacklist":[],"rewardItemBlacklist":[],
        "rewardBaseTypeBlacklist":[],"bossItems":[]
    "#;

    /// Every required `RewardLootVarying` member, spliced beside each request's per-export
    /// varying members below.
    const REWARD_VARYING_JSON: &str = r#""globalBlacklist":[],"inactiveSeasonalItems":[]"#;

    #[test]
    fn random_loot_roundtrips_result_json() {
        // Every count zero and an empty type whitelist: nothing to draw, so no fixture is needed.
        let request = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON}}},
            "varying":{{{REWARD_VARYING_JSON},
            "lootRequest":{{"weaponPresetCount":{{"min":0,"max":0}},
            "armorPresetCount":{{"min":0,"max":0}},"itemCount":{{"min":0,"max":0}},
            "weaponCrateCount":{{"min":0,"max":0}},"itemBlacklist":[],"itemTypeWhitelist":[],
            "itemLimits":{{}},"itemStackLimits":{{}},"armorLevelWhitelist":[]}}}}}}"#
        );

        let (status, out) = call_generate(spt_create_random_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["items"], serde_json::json!([]));
    }

    #[test]
    fn forced_loot_roundtrips_result_json() {
        let request = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON}}},
            "varying":{{{REWARD_VARYING_JSON},"forcedLoot":{{}}}}}}"#
        );

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
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON},"presetsByTpl":{{}}}},
            "varying":{{{REWARD_VARYING_JSON},"containerSettings":{{
            "weaponRewardWeight":{{"888888888888888888888888":1}},"defaultPresetsOnly":false,
            "weaponModRewardLimits":{{}},"rewardTypeLimits":{{}},"ammoBoxWhitelist":[],
            "allowBossItems":false}},"linkedItems":{{}}}}}}"#
        );

        let (status, out) = call_generate(spt_get_sealed_weapon_case_loot, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["items"], serde_json::json!([]));
    }

    #[test]
    fn random_loot_container_roundtrips_result_json() {
        let request = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON},"presetTpls":[]}},
            "varying":{{{REWARD_VARYING_JSON},"rewardDetails":{{"rewardCount":0}}}}}}"#
        );

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
        let request = format!(
            r#"{{"epoch":0,"viewsOverride":{{{REWARD_VIEWS_JSON}}},
            "varying":{{{REWARD_VARYING_JSON},"lootRequest":{{}}}}}}"#
        );

        let (status, out) = call_generate(spt_create_random_loot, request.as_bytes());

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "LootRequest.ItemLimits is null"
        );
    }

    /// Every required `GenerateBotInventoryRequest` member, pared down from the orchestrator's own
    /// fixture: an empty items view (as an override, so the resident store is never consulted), an
    /// empty `Pockets` pool (present, or `:516` derefs a null), and zero loot counts, so the bot
    /// generates nothing but the six inventory roots. `equipmentChances` is left open because the
    /// failure case below is the missing `FirstPrimaryWeapon` key.
    fn bot_request(equipment_chances: &str) -> String {
        format!(
            r#"{{
            "epoch":0,
            "viewsOverride":{{"items":{{}},"itemPresets":{{}},"defaultPresetsByTpl":{{}},
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
            "configBlacklist":[]}},
            "bot":{{
                "botId":"bbbbbbbbbbbbbbbbbbbbbbbb",
                "details":{{"role":"assault","roleLowercase":"assault","side":"Savage","botLevel":15,
                    "isPmc":false,"isPlayerScav":false,"gameVersion":"standard","location":"bigmap",
                    "botDifficulty":"normal","clearBotContainerCacheAfterGeneration":false}}}},
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
            "lootPools":{{}},
            "shared":{{
            "generatingPlayerLevel":20,
            "isNightTime":false,
            "equipment":{{}}
            }}
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

    #[test]
    fn a_stale_bot_epoch_returns_status_4() {
        // No viewsOverride and an epoch the store can never hold (u64::MAX is unreachable —
        // epochs count up from 1). The rest of the request is the fixture's, so the JSON fully
        // parses and the epoch check in resolve_bot_views is what answers.
        let mut request: serde_json::Value =
            serde_json::from_str(&bot_request(r#"{"FirstPrimaryWeapon":0}"#)).unwrap();
        request["epoch"] = serde_json::json!(u64::MAX);
        request.as_object_mut().unwrap().remove("viewsOverride");

        let (status, out) = call_generate(
            spt_generate_bot_inventory,
            &serde_json::to_vec(&request).unwrap(),
        );

        assert_eq!(status, STATUS_STALE_EPOCH);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "resident DB epoch mismatch; republish and retry"
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

    /// Every member of the views override, braces included — the three config-backed members
    /// (Task 6) alongside the database views. The two price tables always know `SELLABLE_TPL`, which
    /// only matters when the items view carries it.
    fn ragfair_views_override(items: &str, offer_item_count: &str) -> String {
        let dynamic = ragfair_dynamic(offer_item_count);
        format!(
            r#"{{"dynamic":{dynamic},"configBlacklist":[],"customMoneyTpls":[],
            "itemPresets":{{}},"defaultPresets":[],"defaultPresetsByTpl":{{}},
            "presetsByTpl":{{}},
            "fleaPrices":{{"{SELLABLE_TPL}":25000}},"handbookPrices":{{"{SELLABLE_TPL}":20000}},
            "highestTraderPrices":{{"{SELLABLE_TPL}":12000}},"items":{items}}}"#
        )
    }

    /// The varying half: the service state Decision 10 keeps off the resident DB. Starts with
    /// `timestamp` so the expired-pass test can splice `expiredOffers` in front of it.
    fn ragfair_varying() -> String {
        r#""varying":{"timestamp":1700000000,"offerCounterStart":0,
            "seasonalEventActive":false,"seasonalItemTplBlacklist":[],
            "pmcNamesUsec":["Deagle"],"pmcNamesBear":["Kirill"]}"#
            .to_owned()
    }

    /// Every required `GenerateDynamicOffersRequest` member, views override included.
    fn ragfair_request_with(items: &str, offer_item_count: &str) -> String {
        let varying = ragfair_varying();
        let views_override = ragfair_views_override(items, offer_item_count);
        format!(r#"{{"epoch":0,{varying},"viewsOverride":{views_override}}}"#)
    }

    /// The minimal override-less request naming `epoch` — the resident-DB half of the protocol.
    fn ragfair_request_at_epoch(epoch: u64) -> String {
        let varying = ragfair_varying();
        format!(r#"{{"epoch":{epoch},{varying}}}"#)
    }

    /// The `configs` root an override-less ragfair request needs resident: the two stems the family
    /// requires, each in the shape `DbPayloadProjection` writes (`kind` included, so the parse has
    /// to ignore it).
    fn ragfair_configs_root() -> String {
        let dynamic = ragfair_dynamic(r#"{"default":{"min":2,"max":5}}"#);
        format!(
            r#""configs":{{"spt-ragfair":{{"kind":"spt-ragfair","dynamic":{dynamic}}},
            "spt-item":{{"kind":"spt-item","blacklist":[]}}}}"#
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
    fn ragfair_epoch_protocol_gates_override_less_requests() {
        let _guard = db_lock();
        crate::db::clear();

        // No publish yet: an override-less request has no resident DB to generate from
        let (status, _) = call_generate(
            spt_generate_dynamic_offers,
            ragfair_request_at_epoch(1).as_bytes(),
        );
        assert_eq!(status, STATUS_STALE_EPOCH);

        // Publish the mini roots (junk roots parse as empty typed containers, so the empty
        // ragfair views derive) plus the configs root the override-less arm reads its config half
        // off, and name the returned epoch: the request generates
        let configs = ragfair_configs_root();
        let (status, out) = call_generate(
            spt_db_publish,
            format!(
                r#"{{"schema":1,"roots":{{"templates":{{"a":1}},"traders":{{"b":2}},
                "globals":{{"c":3}},{configs}}}}}"#
            )
            .as_bytes(),
        );
        assert_eq!(status, STATUS_OK);
        let epoch = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["epoch"]
            .as_u64()
            .unwrap();

        let (status, _) = call_generate(
            spt_generate_dynamic_offers,
            ragfair_request_at_epoch(epoch).as_bytes(),
        );
        assert_eq!(status, STATUS_OK);

        // A mismatched epoch is a stale miss carrying the republish-and-retry message
        let (status, out) = call_generate(
            spt_generate_dynamic_offers,
            ragfair_request_at_epoch(epoch + 1).as_bytes(),
        );
        assert_eq!(status, STATUS_STALE_EPOCH);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "resident DB epoch mismatch; republish and retry"
        );

        // The distrust fallback: a views override at epoch 0 never reads the resident DB
        let (status, _) = call_generate(spt_generate_dynamic_offers, ragfair_request().as_bytes());
        assert_eq!(status, STATUS_OK);

        // Last, because it moves the resident epoch: a configs root that *is* resident but carries
        // no stem this family reads is a per-call failure naming the stem, not the stale-epoch
        // answer a missing root gets — a republish would not fix a stem the publish never carried
        let (status, out) = call_generate(
            spt_db_publish,
            br#"{"schema":1,"roots":{"configs":{"spt-core":{"kind":"spt-core"}}}}"#,
        );
        assert_eq!(status, STATUS_OK);
        let stemless = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["epoch"]
            .as_u64()
            .unwrap();
        let (status, out) = call_generate(
            spt_generate_dynamic_offers,
            ragfair_request_at_epoch(stemless).as_bytes(),
        );
        assert_eq!(status, STATUS_ERROR);
        assert!(
            String::from_utf8(out).unwrap().contains("spt-ragfair"),
            "the stem-missing error must name the stem"
        );
    }

    /// The `configs` member of a publish envelope carrying the two stems the repeatable-quest
    /// family reads, with the same values `views_override_value()` sends on the override arm — so
    /// a resident generation and an override one see byte-identical config-backed inputs.
    fn quest_configs_root() -> String {
        let views = crate::quest::models::tests::views_override_value();
        let quest_stem = serde_json::json!({
            "kind": "spt-quest",
            "repeatableQuestTemplateIds": views["repeatableQuestTemplateIds"],
            "locationIdMap": views["locationIdMap"],
        });
        let item_stem = serde_json::json!({
            "kind": "spt-item",
            "rewardItemBlacklist": views["rewardItemBlacklist"],
            "bossItems": views["bossItems"],
        });

        format!(r#""configs":{{"spt-quest":{quest_stem},"spt-item":{item_stem}}}"#)
    }

    /// A repeatable-quest request at `epoch`, with the views override sent or omitted, off the
    /// fixtures the model tests already pin against the shipped database.
    fn quest_request(epoch: u64, include_override: bool, seed: Option<u64>) -> Vec<u8> {
        let mut varying = crate::quest::models::tests::varying_value();
        if let Some(seed) = seed {
            varying["seed"] = serde_json::json!(seed);
        }
        let mut request = serde_json::json!({"epoch": epoch, "varying": varying});
        if include_override {
            request["viewsOverride"] = crate::quest::models::tests::views_override_value();
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
    fn quest_epoch_protocol_gates_override_less_requests() {
        let _guard = db_lock();
        crate::db::clear();

        // No publish yet: an override-less request has no resident DB to generate from
        let (status, _) = call_generate(
            spt_generate_repeatable_quest,
            &quest_request(1, false, None),
        );
        assert_eq!(status, STATUS_STALE_EPOCH);

        // Publish the mini roots (junk roots parse as empty typed containers, so the empty quest
        // views derive) plus the configs root the override-less arm reads its config-backed
        // members off, and name the returned epoch: the request generates
        let configs = quest_configs_root();
        let (status, out) = call_generate(
            spt_db_publish,
            format!(
                r#"{{"schema":1,"roots":{{"templates":{{"a":1}},"traders":{{"b":2}},
                "globals":{{"c":3}},"locations":{{"factory4_day":{{}}}},{configs}}}}}"#
            )
            .as_bytes(),
        );
        assert_eq!(status, STATUS_OK);
        let epoch = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["epoch"]
            .as_u64()
            .unwrap();

        let (status, _) = call_generate(
            spt_generate_repeatable_quest,
            &quest_request(epoch, false, None),
        );
        assert_eq!(status, STATUS_OK);

        // A mismatched epoch is a stale miss carrying the republish-and-retry message
        let (status, out) = call_generate(
            spt_generate_repeatable_quest,
            &quest_request(epoch + 1, false, None),
        );
        assert_eq!(status, STATUS_STALE_EPOCH);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "resident DB epoch mismatch; republish and retry"
        );

        // The distrust fallback: a views override at epoch 0 never reads the resident DB
        let (status, _) =
            call_generate(spt_generate_repeatable_quest, &quest_request(0, true, None));
        assert_eq!(status, STATUS_OK);

        // Last, because it moves the resident epoch: a configs root that *is* resident but carries
        // no stem this family reads is a per-call failure naming the stem, not the stale-epoch
        // answer a missing root gets — a republish would not fix a stem the publish never carried
        let (status, out) = call_generate(
            spt_db_publish,
            br#"{"schema":1,"roots":{"configs":{"spt-core":{"kind":"spt-core"}}}}"#,
        );
        assert_eq!(status, STATUS_OK);
        let stemless = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["epoch"]
            .as_u64()
            .unwrap();
        let (status, out) = call_generate(
            spt_generate_repeatable_quest,
            &quest_request(stemless, false, None),
        );
        assert_eq!(status, STATUS_ERROR);
        assert!(
            String::from_utf8(out).unwrap().contains("spt-quest"),
            "the stem-missing error must name the stem"
        );
    }

    #[test]
    fn a_quest_generator_throw_returns_status_error_and_the_message() {
        // The Daily config ships no `Pickup` block, and `PickupQuestGenerator:39` dereferences it
        // unconditionally — the C#-sanctioned throw, ported as a panic. Rides the override path,
        // so the resident DB is never read.
        let mut request: serde_json::Value =
            serde_json::from_slice(&quest_request(0, true, None)).unwrap();
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

    /// Publish a resident DB rich enough to draw a real elimination quest from: the shipped
    /// repeatable-quest templates plus the one priced weapon item the wire fixtures pin, as
    /// roots. Returns the epoch override-less requests must name.
    fn publish_quest_roots() -> u64 {
        let templates_block = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/database/templates/repeatableQuests.json"
        ))
        .expect("SPT_Data file readable");
        let request = format!(
            r#"{{"schema":1,"roots":{{
                "templates":{{
                    "items":{{
                        "5422acb9af1c889c16000029":{{"_name":"weapon","_type":"Node","_parent":"","_props":{{}}}},
                        "bbbbbbbbbbbbbbbbbbbbbbbb":{{"_name":"item","_type":"Item",
                            "_parent":"5422acb9af1c889c16000029","_props":{{"StackMaxSize":1}}}}
                    }},
                    "handbook":{{"Items":[{{"Id":"bbbbbbbbbbbbbbbbbbbbbbbb","ParentId":"cat","Price":20000.0}}]}},
                    "prices":{{"bbbbbbbbbbbbbbbbbbbbbbbb":25000.0}},
                    "repeatableQuests":{templates_block}
                }},
                "traders":{{}},
                "globals":{{"ItemPresets":{{"preset1":{{"_id":"preset1","_name":"default",
                    "_items":[{{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb"}}],
                    "_encyclopedia":"bbbbbbbbbbbbbbbbbbbbbbbb"}}}}}},
                "locations":{{}},
                {configs}
            }}}}"#,
            configs = quest_configs_root()
        );

        let (status, out) = call_generate(spt_db_publish, request.as_bytes());
        assert_eq!(status, STATUS_OK);
        serde_json::from_slice::<serde_json::Value>(&out).unwrap()["epoch"]
            .as_u64()
            .expect("publish answers an epoch")
    }

    #[test]
    fn a_seeded_quest_request_answers_the_same_bytes_twice() {
        let _guard = db_lock();
        let epoch = publish_quest_roots();
        let request = quest_request(epoch, false, Some(42));
        // Since flip #7 the template ids and the location map ride the resident configs root, not
        // the request, so the masking's "ids the caller already knew" set has to cover the root
        // too — otherwise a regression that swapped the drawn template id would be blanked as a
        // mint on both sides and never show.
        let known = |request: &[u8]| [request, quest_configs_root().as_bytes()].concat();
        let known_request = known(&request);

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
        // The ids a draw can move — the trader the quest is minted for, the template it was cloned
        // from, and the tpls it hands over — are all known, so the masking leaves them in the
        // compared bytes.
        let masked = mask_minted_ids(&first, &known_request);
        assert!(
            masked.contains("54cb50c76803fa8b248b4571"),
            "the request's own ids must survive the masking"
        );
        assert!(
            masked.contains("616052ea3054fc0e2c24ce6e"),
            "the configs root's template id must survive the masking"
        );
        assert_eq!(masked, mask_minted_ids(&second, &known_request));

        // …and the masking has teeth: another seed draws a different quest.
        let other_request = quest_request(epoch, false, Some(7));
        let (status, other) = call_generate(spt_generate_repeatable_quest, &other_request);
        assert_eq!(status, STATUS_OK);
        assert_ne!(masked, mask_minted_ids(&other, &known(&other_request)));
    }

    /// A scav case craft off the generator's own synthetic table, seeded so the reward list is the
    /// one its end-to-end KAT pins — an epoch-0 override send.
    fn scav_case_request() -> Vec<u8> {
        let mut flat = crate::scav_case::generator::tests::container_request_json();
        flat["testSeed"] = serde_json::json!(42);

        serde_json::to_vec(&crate::scav_case::generator::tests::envelope(flat))
            .expect("request serializes")
    }

    #[test]
    fn scav_case_rewards_roundtrip_result_json() {
        let (status, out) = call_generate(spt_generate_scav_case_rewards, &scav_case_request());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let rewards = result["result"].as_array().unwrap();
        assert!(!rewards.is_empty(), "the fixture craft draws rewards");
        // Each entry is one reward item plus its children, so every group has a root with a tpl.
        assert!(
            rewards
                .iter()
                .all(|group| group[0]["_tpl"].is_string() && group[0]["_id"].is_string())
        );
    }

    #[test]
    fn unparseable_scav_case_request_returns_bad_args_with_the_parse_error() {
        let (status, out) = call_generate(spt_generate_scav_case_rewards, b"{\"recipeId\":");

        assert_eq!(status, STATUS_BAD_ARGS);
        let message = String::from_utf8(out).unwrap();
        assert!(
            message.contains("EOF while parsing"),
            "expected the serde error, got: {message}"
        );
    }

    #[test]
    fn a_scav_case_failure_returns_status_error_and_the_message() {
        // `FirstOrDefault` (`:54`) answers null for a recipe id the table does not hold, which the
        // C# then NREs dereferencing — reported here as an error naming the recipe.
        let mut request: serde_json::Value = serde_json::from_slice(&scav_case_request()).unwrap();
        request["varying"]["recipeId"] = serde_json::json!("ffffffffffffffffffffffff");

        let (status, out) = call_generate(
            spt_generate_scav_case_rewards,
            &serde_json::to_vec(&request).unwrap(),
        );

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "No scav case recipe found with id: ffffffffffffffffffffffff"
        );
    }

    #[test]
    fn a_stale_scav_case_epoch_returns_status_4() {
        // No viewsOverride and an epoch the store can never hold (u64::MAX is unreachable —
        // epochs count up from 1). The varying block is the fixture's, so the JSON fully parses
        // and the epoch check in resolve_scav_case_views is what answers.
        let mut request: serde_json::Value = serde_json::from_slice(&scav_case_request()).unwrap();
        request["epoch"] = serde_json::json!(u64::MAX);
        request.as_object_mut().unwrap().remove("viewsOverride");

        let (status, out) = call_generate(
            spt_generate_scav_case_rewards,
            &serde_json::to_vec(&request).unwrap(),
        );

        assert_eq!(status, STATUS_STALE_EPOCH);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "resident DB epoch mismatch; republish and retry"
        );
    }

    /// `child` is an Item whose chain climbs through the `node` root to a `root` the view never
    /// holds — the three-template shape `base_class`'s own tests pin, here across the boundary.
    const BASE_CLASS_REQUEST: &[u8] = br#"{"epoch":0,"viewsOverride":{"itemsView":{
        "child":{"parent":"node","type":"Item"},
        "node":{"parent":"root","type":"Node"}
    }}}"#;

    #[test]
    fn build_item_base_class_cache_export_round_trips() {
        let (status, out) = call_generate(spt_build_item_base_class_cache, BASE_CLASS_REQUEST);

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        // Only the Item-type template is cached; the `node` root must not leak into the map.
        let cache = result["result"]["itemBaseClasses"].as_object().unwrap();
        assert_eq!(cache.len(), 1);
        // A `HashSet` crosses as an array, so the chain is order-independent.
        let chain = result["result"]["itemBaseClasses"]["child"]
            .as_array()
            .unwrap();
        assert_eq!(chain.len(), 2);
        assert!(chain.contains(&serde_json::json!("node")));
        assert!(chain.contains(&serde_json::json!("root")));
        assert_eq!(result["result"]["rootNodeIds"], serde_json::json!(["node"]));
    }

    #[test]
    fn build_item_base_class_cache_rejects_malformed_json() {
        let (status, out) = call_generate(spt_build_item_base_class_cache, b"not json");

        assert_eq!(status, STATUS_BAD_ARGS);
        let message = String::from_utf8(out).unwrap();
        assert!(
            message.contains("expected ident"),
            "expected the serde error, got: {message}"
        );
    }

    #[test]
    fn build_item_base_class_cache_epoch_protocol_gates_override_less_requests() {
        let _guard = db_lock();
        crate::db::clear();

        // No publish yet: an override-less request has no resident DB to build from
        let (status, _) = call_generate(spt_build_item_base_class_cache, br#"{"epoch":1}"#);
        assert_eq!(status, STATUS_STALE_EPOCH);

        // Publish a templates-only mini root and name the returned epoch: the request builds
        let (status, out) = call_generate(
            spt_db_publish,
            br#"{"schema":1,"roots":{"templates":{"items":{
                "child":{"_parent":"node","_type":"Item"},
                "node":{"_parent":"root","_type":"Node"}
            }}}}"#,
        );
        assert_eq!(status, STATUS_OK);
        let epoch = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["epoch"]
            .as_u64()
            .unwrap();

        let (status, out) = call_generate(
            spt_build_item_base_class_cache,
            format!(r#"{{"epoch":{epoch}}}"#).as_bytes(),
        );
        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let chain = result["result"]["itemBaseClasses"]["child"]
            .as_array()
            .unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(result["result"]["rootNodeIds"], serde_json::json!(["node"]));

        // A mismatched epoch is a stale miss carrying the republish-and-retry message
        let (status, out) = call_generate(
            spt_build_item_base_class_cache,
            format!(r#"{{"epoch":{}}}"#, epoch + 1).as_bytes(),
        );
        assert_eq!(status, STATUS_STALE_EPOCH);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "resident DB epoch mismatch; republish and retry"
        );
    }

    /// One weapon linking one stock — enough to see both directions cross the boundary.
    const LINKED_ITEMS_REQUEST: &[u8] = br#"{"epoch":0,"viewsOverride":{"itemsView":{
        "weapon":{"parent":"p","slots":[{"name":"mod_stock","filter":["stockA"]}]},
        "stockA":{"parent":"p"}
    }}}"#;

    #[test]
    fn build_ragfair_linked_item_table_export_round_trips() {
        let (status, out) =
            call_generate(spt_build_ragfair_linked_item_table, LINKED_ITEMS_REQUEST);

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let linked = result["result"]["linkedItems"].as_object().unwrap();
        assert_eq!(linked.len(), 2);
        // A `HashSet` crosses as an array.
        let weapon = linked["weapon"].as_array().unwrap();
        assert_eq!(weapon.len(), 1);
        assert!(weapon.contains(&serde_json::json!("stockA")));
        let stock = linked["stockA"].as_array().unwrap();
        assert!(stock.contains(&serde_json::json!("weapon")));
    }

    #[test]
    fn build_ragfair_linked_item_table_rejects_malformed_json() {
        let (status, out) = call_generate(spt_build_ragfair_linked_item_table, b"not json");

        assert_eq!(status, STATUS_BAD_ARGS);
        let message = String::from_utf8(out).unwrap();
        assert!(
            message.contains("expected ident"),
            "expected the serde error, got: {message}"
        );
    }

    #[test]
    fn build_ragfair_linked_item_table_epoch_protocol_gates_override_less_requests() {
        let _guard = db_lock();
        crate::db::clear();

        let (status, _) = call_generate(spt_build_ragfair_linked_item_table, br#"{"epoch":1}"#);
        assert_eq!(status, STATUS_STALE_EPOCH);

        let (status, out) = call_generate(
            spt_db_publish,
            br#"{"schema":1,"roots":{"templates":{"items":{
                "weapon":{"_parent":"p","_props":{"Slots":[{"_name":"mod_stock","_props":{"filters":[{"Filter":["stockA"]}]}}]}},
                "stockA":{"_parent":"p"}
            }}}}"#,
        );
        assert_eq!(status, STATUS_OK);
        let epoch = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["epoch"]
            .as_u64()
            .unwrap();

        let (status, out) = call_generate(
            spt_build_ragfair_linked_item_table,
            format!(r#"{{"epoch":{epoch}}}"#).as_bytes(),
        );
        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let weapon = result["result"]["linkedItems"]["weapon"]
            .as_array()
            .unwrap();
        assert!(weapon.contains(&serde_json::json!("stockA")));
        assert!(
            result["result"]["linkedItems"]["stockA"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("weapon"))
        );

        let (status, out) = call_generate(
            spt_build_ragfair_linked_item_table,
            format!(r#"{{"epoch":{}}}"#, epoch + 1).as_bytes(),
        );
        assert_eq!(status, STATUS_STALE_EPOCH);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "resident DB epoch mismatch; republish and retry"
        );
    }

    /// Every publish writes the one process-global resident DB, so these run under its test lock.
    fn db_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::db::tests::DB_TEST_LOCK.lock().unwrap()
    }

    #[test]
    fn db_publish_bumps_the_epoch_per_publish() {
        let _guard = db_lock();
        crate::db::clear();

        // All three mini roots resident: the publish also derives the ragfair views.
        let (status, out) = call_generate(
            spt_db_publish,
            br#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        );
        assert_eq!(status, STATUS_OK);
        let body: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(body, serde_json::json!({"epoch": 1}));

        let (status, out) = call_generate(
            spt_db_publish,
            br#"{"schema":1,"roots":{"templates":{"a":9}}}"#,
        );
        assert_eq!(status, STATUS_OK);
        let body: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(body, serde_json::json!({"epoch": 2}));
    }

    #[test]
    fn db_publish_rejects_an_unknown_root_as_bad_args() {
        let _guard = db_lock();
        let (status, out) =
            call_generate(spt_db_publish, br#"{"schema":1,"roots":{"tempaltes":{}}}"#);

        assert_eq!(status, STATUS_BAD_ARGS);
        let message = String::from_utf8(out).unwrap();
        assert!(
            message.contains("unknown field"),
            "expected the serde error, got: {message}"
        );
    }

    #[test]
    fn db_publish_reports_a_schema_error_as_status_error() {
        let _guard = db_lock();
        let (status, out) = call_generate(spt_db_publish, br#"{"schema":2,"roots":{}}"#);

        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "unsupported publish schema 2"
        );
    }

    #[test]
    fn spt_db_load_round_trips_a_mini_tree() {
        let _guard = db_lock();
        crate::db::clear();

        let dir = crate::db::load::tests::mini_tree();
        let request = format!(
            r#"{{"schema":1,"dir":{},"verify":false}}"#,
            serde_json::to_string(dir.path().to_str().unwrap()).unwrap()
        );

        let (status, out) = call_generate(spt_db_load, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let header_len = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&out[4..4 + header_len]).unwrap();
        assert_eq!(header["epoch"], 1);
        assert_eq!(header["verify"], serde_json::Value::Null);

        // The index really addresses the blobs: walk them in files order and land on the end.
        let mut blobs = std::collections::BTreeMap::new();
        let mut at = 4 + header_len;
        for entry in header["files"].as_array().unwrap() {
            let len = entry["len"].as_u64().unwrap() as usize;
            blobs.insert(
                entry["path"].as_str().unwrap().to_owned(),
                &out[at..at + len],
            );
            at += len;
        }
        assert_eq!(at, out.len(), "the blobs fill the frame exactly");

        assert_eq!(blobs.len(), 11, "the mini tree's eager set");
        assert_eq!(
            blobs["database/templates/items.json"],
            fs::read(dir.path().join("database/templates/items.json"))
                .unwrap()
                .as_slice()
        );
        assert!(!blobs.contains_key("database/locations/bigmap/looseLoot.json"));
    }

    #[test]
    fn spt_db_load_rejects_a_bad_schema_with_bad_args() {
        // Rejected before any walk, so it needs no tree and never reaches the store.
        let (status, out) = call_generate(spt_db_load, br#"{"schema":2,"dir":".","verify":false}"#);

        assert_eq!(status, STATUS_BAD_ARGS);
        assert_eq!(String::from_utf8(out).unwrap(), "unsupported load schema 2");
    }

    #[test]
    fn spt_db_load_null_args_is_bad_args_without_a_buffer() {
        let mut out_len: usize = 0;

        let status =
            unsafe { spt_db_load(std::ptr::null(), 0, std::ptr::null_mut(), &mut out_len) };

        assert_eq!(status, STATUS_BAD_ARGS);
        assert_eq!(out_len, 0, "nothing may be written when out_ptr is null");
    }

    #[test]
    fn db_resident_digest_answers_the_empty_report_when_nothing_is_resident() {
        let _guard = db_lock();
        crate::db::clear();

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let status = unsafe { spt_db_resident_digest(&mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_OK);
        let body = unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec();
        unsafe { spt_buf_free(out_ptr, out_len) };
        assert_eq!(body, br#"{"epoch":0,"roots":{}}"#.to_vec());

        assert_eq!(
            unsafe { spt_db_resident_digest(std::ptr::null_mut(), &mut out_len) },
            STATUS_BAD_ARGS
        );
        assert_eq!(
            out_len,
            body.len(),
            "nothing may be written when out_ptr is null"
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

    static TAP_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    unsafe extern "C" fn roundtrip_tap(
        _category_ptr: *const u8,
        _category_len: usize,
        message_ptr: *const u8,
        message_len: usize,
        _tname_ptr: *const u8,
        _tname_len: usize,
        level: i32,
        _tid: i32,
        _millis: i64,
    ) {
        let message =
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(message_ptr, message_len) })
                .unwrap_or("<bad utf8>")
                .to_owned();
        TAP_LINES.lock().unwrap().push(format!("{level}:{message}"));
    }

    #[test]
    fn logger_exports_roundtrip() {
        /// The messages this test emits; everything else in the file belongs to a generator.
        const MINE: [&str; 10] = [
            "hello",
            "nullspans",
            "still up",
            "after teardown",
            "before init",
            "Unable to find an item with tpl of: 54009119af1c881c07000029 in Db",
            "plain line",
            "moved",
            "survives reinit failure",
            "not tapped",
        ];

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

        // The IsLogEnabled gate has no pipeline to ask yet.
        assert_eq!(spt_log_enabled(2), -1);

        // Reinit before init: an error naming the missing init, pipeline stays down.
        let status =
            unsafe { spt_logger_reinit(config.as_ptr(), config.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_ERROR);
        assert!(!out_ptr.is_null());
        unsafe { spt_buf_free(out_ptr, out_len) };

        // Same for a generator diagnostic: a run before the host initialised logging drops its
        // lines rather than panicking on the empty pipeline.
        crate::diag::DiagSink::Pipeline.push(crate::loot::models::Diagnostic {
            category: "SPTarkov.Server.Core.Generators.Loot.LootGenerator",
            level: crate::loot::models::WARNING.to_owned(),
            locale_key: None,
            args: None,
            message: Some("before init".to_owned()),
        });

        // Real init; the second init keeps the running pipeline and takes a reference.
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let status =
            unsafe { spt_logger_init(config.as_ptr(), config.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_OK);
        let status =
            unsafe { spt_logger_init(config.as_ptr(), config.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_OK);

        // The gate answers from the applied config: Information on, Debug off, junk level unknown.
        assert_eq!(spt_log_enabled(2), 1);
        assert_eq!(spt_log_enabled(1), 0);
        assert_eq!(spt_log_enabled(99), -1);

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

        // Reinit with unparseable JSON: error reported, the running pipeline is untouched.
        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let status = unsafe { spt_logger_reinit(b"nope".as_ptr(), 4, &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_ERROR);
        assert!(!out_ptr.is_null());
        unsafe { spt_buf_free(out_ptr, out_len) };
        assert_eq!(
            emit("Cat", "survives reinit failure", "", "main"),
            STATUS_OK
        );

        // Reinit to a second directory: later lines land there, not in the first file.
        let moved_dir = TempDir::new().unwrap();
        let moved_config = format!(
            r#"{{ "loggers": [ {{ "type": "File", "logLevel": "Information",
                "format": "%message%", "filePath": {path:?}, "filePattern": "spt.log",
                "maxFileSizeMB": 10, "maxRollingFiles": 10, "filters": [] }} ] }}"#,
            path = moved_dir.path().display().to_string(),
        );
        let status = unsafe {
            spt_logger_reinit(
                moved_config.as_ptr(),
                moved_config.len(),
                &mut out_ptr,
                &mut out_len,
            )
        };
        assert_eq!(status, STATUS_OK);
        assert_eq!(emit("Cat", "moved", "", "main"), STATUS_OK);

        // Reinit back to the first directory: a path this process already freshened reopens in
        // append mode - no second cascade, so no spt.1.log appears beside the live file.
        let status =
            unsafe { spt_logger_reinit(config.as_ptr(), config.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_OK);

        // The tap receives Rust-originated lines only - this line crosses spt_log_emit inside the
        // armed window, so it proves the export does not tap rather than merely arriving too early.
        assert_eq!(unsafe { spt_log_set_tap(Some(roundtrip_tap)) }, STATUS_OK);
        assert_eq!(emit("Cat", "not tapped", "", "main"), STATUS_OK);

        // Locale table + live diagnostic emission share the same run: bad JSON first.
        let status = unsafe { spt_locales_set(b"nope".as_ptr(), 4, &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_ERROR);
        assert!(!out_ptr.is_null());
        unsafe { spt_buf_free(out_ptr, out_len) };

        let locales =
            br#"{ "roundtrip-test-key": "Unable to find an item with tpl of: %s in Db" }"#;
        let status =
            unsafe { spt_locales_set(locales.as_ptr(), locales.len(), &mut out_ptr, &mut out_len) };
        assert_eq!(status, STATUS_OK);

        let mut sink = crate::diag::DiagSink::Pipeline;
        sink.push(crate::loot::models::Diagnostic {
            category: "SPTarkov.Server.Core.Helpers.Items.ItemHelper",
            level: crate::loot::models::ERROR.to_owned(),
            locale_key: Some("roundtrip-test-key".to_owned()),
            args: Some(serde_json::json!("54009119af1c881c07000029")),
            message: None,
        });
        sink.push(crate::loot::models::Diagnostic {
            category: "SPTarkov.Server.Core.Generators.Loot.LootGenerator",
            level: crate::loot::models::WARNING.to_owned(),
            locale_key: None,
            args: None,
            message: Some("plain line".to_owned()),
        });

        // Parallel tests' generator diagnostics share the process-global tap; filter to ours.
        {
            let tapped = TAP_LINES.lock().unwrap();
            assert!(tapped.contains(
                &"4:Unable to find an item with tpl of: 54009119af1c881c07000029 in Db".to_owned()
            ));
            assert!(tapped.contains(&"3:plain line".to_owned()));
            assert!(
                !tapped.iter().any(|line| line.ends_with(":not tapped")),
                "spt_log_emit lines fan out C#-side and must not reach the tap"
            );
        }

        // A cleared tap receives nothing further.
        assert_eq!(unsafe { spt_log_set_tap(None) }, STATUS_OK);
        sink.push(crate::loot::models::Diagnostic {
            category: "SPTarkov.Server.Core.Generators.Loot.LootGenerator",
            level: crate::loot::models::WARNING.to_owned(),
            locale_key: None,
            args: None,
            message: Some("untapped line".to_owned()),
        });
        assert!(
            !TAP_LINES
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.ends_with(":untapped line")),
            "a cleared tap must receive nothing"
        );

        // The first close drops the second init's reference - the nested `Program.Main` disposing
        // its container must not take the outer host's logging down with it.
        assert_eq!(unsafe { spt_logger_close() }, STATUS_OK);
        assert_eq!(emit("Cat", "still up", "", "main"), STATUS_OK);

        // The matching close flushes and tears down; further closes are no-ops.
        assert_eq!(unsafe { spt_logger_close() }, STATUS_OK);
        assert_eq!(unsafe { spt_logger_close() }, STATUS_OK);
        assert_eq!(emit("Cat", "after teardown", "", "main"), STATUS_OK);
        assert_eq!(spt_log_enabled(2), -1);

        // Generators in other tests emit their diagnostics through the same process-global
        // pipeline, so only this test's own lines can be asserted on.
        // Same shared-pipeline caveat as the first file below: other tests' generator diagnostics
        // land here too for as long as the reinit pointed the pipeline at this directory.
        let moved_contents = fs::read_to_string(moved_dir.path().join("spt.log")).unwrap();
        let moved_mine: Vec<&str> = moved_contents
            .lines()
            .filter(|line| MINE.contains(line))
            .collect();
        assert_eq!(moved_mine, ["moved"]);
        assert!(
            !dir.path().join("spt.1.log").exists(),
            "reinit to an already-freshened path must append, not cascade"
        );
        let contents = fs::read_to_string(dir.path().join("spt.log")).unwrap();
        let mine: Vec<&str> = contents
            .lines()
            .filter(|line| MINE.contains(line))
            .collect();
        // "before init" and "after teardown" never reach the file: no pipeline, nothing written.
        assert_eq!(
            mine,
            [
                "hello",
                "nullspans",
                "survives reinit failure",
                "not tapped",
                "Unable to find an item with tpl of: 54009119af1c881c07000029 in Db",
                "plain line",
                "still up",
            ]
        );
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

    /// spt_log_format is stateless — no pipeline init needed.
    #[test]
    fn log_format_renders_with_the_pipeline_token_expansion() {
        fn format(format: &str, message: &str) -> (i32, Option<String>) {
            let logger = "SPTarkov.Server.Core.Utils.App";
            let thread_name = "main";
            let mut out_ptr: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let status = unsafe {
                spt_log_format(
                    format.as_ptr(),
                    format.len(),
                    message.as_ptr(),
                    message.len(),
                    logger.as_ptr(),
                    logger.len(),
                    thread_name.as_ptr(),
                    thread_name.len(),
                    2,                 // Information
                    7,                 // tid
                    1_786_900_205_123, // 2026-08-16 17:10:05.123 UTC
                    &mut out_ptr,
                    &mut out_len,
                )
            };
            if status != STATUS_OK || out_ptr.is_null() {
                return (status, None);
            }
            let text =
                String::from_utf8(unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec())
                    .unwrap();
            unsafe { spt_buf_free(out_ptr, out_len) };
            (status, Some(text))
        }

        let (status, line) = format("[%date% %time%][%level%][%loggerShort%] %message%", "hi");
        assert_eq!(status, STATUS_OK);
        assert_eq!(
            line.unwrap(),
            "[2026-08-16 17:10:05.123][Information][App] hi"
        );

        // A brace-bearing format renders literally — the CompositeFormat throw is gone, an
        // accepted divergence.
        let (status, line) = format("{%message%}", "x");
        assert_eq!(status, STATUS_OK);
        assert_eq!(line.unwrap(), "{x}");

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let status = unsafe {
            spt_log_format(
                std::ptr::null(),
                3, // null with non-zero len
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                2,
                7,
                0,
                &mut out_ptr,
                &mut out_len,
            )
        };
        assert_eq!(status, STATUS_BAD_ARGS);
    }

    #[test]
    fn console_exports_validate_and_survive_without_a_pipeline() {
        // Null bytes with a non-zero length are rejected; everything else is best-effort OK
        // (the writes land on the test harness's captured stdout, which is fine).
        assert_eq!(
            unsafe { spt_console_write(std::ptr::null(), 5, 0) },
            STATUS_BAD_ARGS
        );
        assert_eq!(
            unsafe { spt_console_write(std::ptr::null(), 0, 0) },
            STATUS_OK
        );
        let bytes = b"ffi console test stdout\n";
        assert_eq!(
            unsafe { spt_console_write(bytes.as_ptr(), bytes.len(), 0) },
            STATUS_OK
        );
        assert_eq!(
            unsafe { spt_console_write(bytes.as_ptr(), bytes.len(), 1) },
            STATUS_OK
        );

        let title = "t";
        assert_eq!(
            unsafe { spt_console_set_title(title.as_ptr(), title.len()) },
            STATUS_OK
        );
        assert_eq!(
            unsafe { spt_console_set_title(std::ptr::null(), 1) },
            STATUS_BAD_ARGS
        );
        assert_eq!(unsafe { spt_console_clear() }, STATUS_OK);
    }

    // The profile exports below work a directory named in every request and touch no
    // process-global state, so — unlike the db exports — none of these take `db_lock`.

    /// A valid MongoId: the only id shape the exports accept.
    const PROFILE_ID: &str = "6889d9d1f8ee8ab88c0b8e11";

    /// The tempdir path as a JSON string literal, ready to splice into an envelope.
    fn profile_dir(dir: &TempDir) -> String {
        serde_json::to_string(dir.path().to_str().unwrap()).unwrap()
    }

    fn profile_exports() -> [Export; 4] {
        [
            spt_profile_list,
            spt_profile_load,
            spt_profile_save,
            spt_profile_delete,
        ]
    }

    #[test]
    fn profile_save_then_load_round_trips_bytes() {
        let dir = TempDir::new().unwrap();
        // Spliced raw into the envelope, not as a JSON string: whatever `RawValue` swallows here
        // is what has to land on disk, indentation and CRLF included.
        let profile = "{\n  \"a\": 1,\r\n\t\"b\":[ ]\n}";
        let request = format!(
            r#"{{"schema":1,"dir":{},"id":"{PROFILE_ID}","profile":{profile}}}"#,
            profile_dir(&dir)
        );

        let (status, out) = call_generate(spt_profile_save, request.as_bytes());
        assert_eq!(status, STATUS_OK);
        assert_eq!(String::from_utf8(out).unwrap(), "{}");

        let request = format!(
            r#"{{"schema":1,"dir":{},"id":"{PROFILE_ID}"}}"#,
            profile_dir(&dir)
        );
        let (status, out) = call_generate(spt_profile_load, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let header_len = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&out[4..4 + header_len]).unwrap();
        assert_eq!(header, serde_json::json!({"found": true}));
        assert_eq!(&out[4 + header_len..], profile.as_bytes());
    }

    #[test]
    fn profile_load_missing_reports_found_false() {
        let dir = TempDir::new().unwrap();
        let request = format!(
            r#"{{"schema":1,"dir":{},"id":"{PROFILE_ID}"}}"#,
            profile_dir(&dir)
        );

        let (status, out) = call_generate(spt_profile_load, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let header_len = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&out[4..4 + header_len]).unwrap();
        assert_eq!(header, serde_json::json!({"found": false}));
        assert_eq!(out.len(), 4 + header_len, "a miss carries no blob");
    }

    #[test]
    fn profile_list_names_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.json"), b"{}").unwrap();
        fs::create_dir(dir.path().join("backups")).unwrap();
        let request = format!(r#"{{"schema":1,"dir":{}}}"#, profile_dir(&dir));

        let (status, out) = call_generate(spt_profile_list, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let body: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(body, serde_json::json!({"files": ["a.json"]}));
    }

    #[test]
    fn profile_delete_reports_deleted() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(format!("{PROFILE_ID}.json")), b"{}").unwrap();
        let request = format!(
            r#"{{"schema":1,"dir":{},"id":"{PROFILE_ID}"}}"#,
            profile_dir(&dir)
        );

        let (status, out) = call_generate(spt_profile_delete, request.as_bytes());
        assert_eq!(status, STATUS_OK);
        assert_eq!(String::from_utf8(out).unwrap(), r#"{"deleted":true}"#);

        let (status, out) = call_generate(spt_profile_delete, request.as_bytes());
        assert_eq!(status, STATUS_OK);
        assert_eq!(String::from_utf8(out).unwrap(), r#"{"deleted":false}"#);
    }

    #[test]
    fn profile_exports_reject_junk_envelopes_as_bad_args() {
        for export in profile_exports() {
            let (status, out) = call_generate(export, b"not json");

            assert_eq!(status, STATUS_BAD_ARGS);
            assert!(!out.is_empty(), "the serde error message is missing");
        }
    }

    #[test]
    fn profile_exports_reject_traversal_ids() {
        let dir = TempDir::new().unwrap();
        let id = "../../etc/passwd";
        let addressed = format!(r#"{{"schema":1,"dir":{},"id":{id:?}}}"#, profile_dir(&dir));
        let saved = format!(
            r#"{{"schema":1,"dir":{},"id":{id:?},"profile":{{}}}}"#,
            profile_dir(&dir)
        );

        for (export, request) in [
            (spt_profile_load as Export, &addressed),
            (spt_profile_save as Export, &saved),
            (spt_profile_delete as Export, &addressed),
        ] {
            let (status, out) = call_generate(export, request.as_bytes());

            assert_eq!(status, STATUS_BAD_ARGS);
            let message = String::from_utf8(out).unwrap();
            assert!(
                message.contains(id),
                "the message must name it, got: {message}"
            );
        }
    }

    #[test]
    fn profile_wrong_schema_is_bad_args() {
        let dir = TempDir::new().unwrap();
        let request = format!(r#"{{"schema":2,"dir":{}}}"#, profile_dir(&dir));

        let (status, out) = call_generate(spt_profile_list, request.as_bytes());

        assert_eq!(status, STATUS_BAD_ARGS);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "unsupported schema 2, expected 1"
        );
    }

    /// The other half of `ProfileError`'s status mapping: a real disk failure is STATUS_ERROR, not
    /// a caller's bad request.
    #[test]
    fn profile_io_failure_is_status_error() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(format!("{PROFILE_ID}.json"))).unwrap();
        let request = format!(
            r#"{{"schema":1,"dir":{},"id":"{PROFILE_ID}","profile":{{}}}}"#,
            profile_dir(&dir)
        );

        let (status, out) = call_generate(spt_profile_save, request.as_bytes());

        assert_eq!(status, STATUS_ERROR);
        assert!(!out.is_empty(), "the io error message is missing");
    }

    #[test]
    fn profile_null_args_are_bad_args_without_a_buffer() {
        for export in profile_exports() {
            let mut out_len: usize = 0;

            let status = unsafe { export(std::ptr::null(), 0, std::ptr::null_mut(), &mut out_len) };

            assert_eq!(status, STATUS_BAD_ARGS);
            assert_eq!(out_len, 0, "nothing may be written when out_ptr is null");
        }
    }
}
