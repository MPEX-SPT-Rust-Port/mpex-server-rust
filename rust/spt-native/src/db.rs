//! The process-global resident database (spec § The epoch protocol, as amended 2026-08-18).
//!
//! One `RwLock<Option<Arc<ResidentDb>>>`: readers clone the `Arc` at entry and never hold the
//! lock across a generation; a publish builds the merged replacement and swaps it under the
//! write lock. Epoch starts at 1 on the first successful publish and increments on every
//! publish, full or partial. A failed publish leaves the previous resident DB fully intact.

pub mod models;

use std::sync::{Arc, RwLock};

use crate::db::models::{GlobalsRoot, PublishRequest, TemplatesRoot, TradersRoot};

pub struct ResidentDb {
    pub epoch: u64,
    pub templates: Option<Arc<TemplatesRoot>>,
    pub traders: Option<Arc<TradersRoot>>,
    pub globals: Option<Arc<GlobalsRoot>>,
}

#[derive(Debug)]
pub enum PublishError {
    Schema(String),
}

static DB: RwLock<Option<Arc<ResidentDb>>> = RwLock::new(None);

/// The resident DB right now, or `None` before the first publish. Arc clone under a read lock,
/// released before returning — callers never block a publish beyond the clone itself.
pub fn current() -> Option<Arc<ResidentDb>> {
    DB.read().unwrap().clone()
}

/// Install `request`'s roots over the currently-resident ones and bump the epoch.
// ponytail: whole merge under the write lock (publish is startup + stamp-bump cadence); split
// the derive out of the lock if reader stalls ever measure
pub fn publish(request: PublishRequest) -> Result<u64, PublishError> {
    if request.schema != 1 {
        return Err(PublishError::Schema(format!(
            "unsupported publish schema {}",
            request.schema
        )));
    }

    let mut slot = DB.write().unwrap();
    let previous = slot.as_ref();
    let merged = ResidentDb {
        epoch: previous.map_or(1, |db| db.epoch + 1),
        templates: request
            .roots
            .templates
            .map(Arc::new)
            .or_else(|| previous.and_then(|db| db.templates.clone())),
        traders: request
            .roots
            .traders
            .map(Arc::new)
            .or_else(|| previous.and_then(|db| db.traders.clone())),
        globals: request
            .roots
            .globals
            .map(Arc::new)
            .or_else(|| previous.and_then(|db| db.globals.clone())),
    };
    let epoch = merged.epoch;
    *slot = Some(Arc::new(merged));

    Ok(epoch)
}

#[cfg(test)]
pub fn clear() {
    *DB.write().unwrap() = None;
}

#[cfg(test)]
pub mod tests {
    /// Serializes every test that touches the process-global store — the same discipline the
    /// slice caches used (`ragfair/slice_cache.rs`).
    pub static DB_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

#[cfg(test)]
mod store_tests {
    use super::*;

    fn request(json: &str) -> models::PublishRequest {
        serde_json::from_str(json).expect("test request parses")
    }

    #[test]
    fn epoch_starts_at_one_and_increments() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        assert!(current().is_none());

        let first = publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let second = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(current().unwrap().epoch, 2);
    }

    #[test]
    fn partial_publish_keeps_absent_roots() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        let after = current().unwrap();

        // traders/globals survive by Arc identity; templates was replaced
        assert!(Arc::ptr_eq(
            before.traders.as_ref().unwrap(),
            after.traders.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.globals.as_ref().unwrap(),
            after.globals.as_ref().unwrap()
        ));
        assert!(!Arc::ptr_eq(
            before.templates.as_ref().unwrap(),
            after.templates.as_ref().unwrap()
        ));
    }

    #[test]
    fn wrong_schema_is_an_error_and_leaves_store_untouched() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        let error = publish(request(r#"{"schema":2,"roots":{}}"#)).unwrap_err();
        assert!(matches!(error, PublishError::Schema(_)));
        assert!(current().is_none());
    }

    #[test]
    fn unknown_root_name_fails_the_parse() {
        // deny_unknown_fields on PublishRoots: a typo'd root is a deserialize error,
        // which ffi.rs maps to STATUS_BAD_ARGS
        let result: Result<models::PublishRequest, _> =
            serde_json::from_str(r#"{"schema":1,"roots":{"tempaltes":{}}}"#);
        assert!(result.is_err());
    }
}
