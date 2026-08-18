//! The process-global resident database (spec § The epoch protocol, as amended 2026-08-18).
//!
//! One `RwLock<Option<Arc<ResidentDb>>>`: readers clone the `Arc` at entry and never hold the
//! lock across a generation; a publish builds the merged replacement and swaps it under the
//! write lock. Epoch starts at 1 on the first successful publish and increments on every
//! publish, full or partial. A failed publish leaves the previous resident DB fully intact.

pub mod models;

use std::sync::{Arc, RwLock};

use crate::db::models::{GlobalsRoot, PublishRequest, TemplatesRoot, TradersRoot};
use crate::ragfair::views::RagfairDbViews;

pub struct ResidentDb {
    pub epoch: u64,
    pub templates: Option<Arc<TemplatesRoot>>,
    pub traders: Option<Arc<TradersRoot>>,
    pub globals: Option<Arc<GlobalsRoot>>,
    /// `Some` whenever all three source roots are resident — re-derived on every such publish,
    /// `None` otherwise.
    pub ragfair_views: Option<Arc<RagfairDbViews>>,
}

#[derive(Debug)]
pub enum PublishError {
    Schema(String),
    /// A ragfair view derivation failure. Aborts the publish before the swap — the previous
    /// resident DB stays fully intact.
    Views(String),
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
    let epoch = previous.map_or(1, |db| db.epoch + 1);
    let templates = request
        .roots
        .templates
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.templates.clone()));
    let traders = request
        .roots
        .traders
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.traders.clone()));
    let globals = request
        .roots
        .globals
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.globals.clone()));

    // Derived before the swap: a derivation error aborts the publish and leaves the previous
    // resident DB fully intact.
    let ragfair_views = match (&templates, &traders, &globals) {
        (Some(templates), Some(traders), Some(globals)) => Some(Arc::new(
            crate::ragfair::views::derive(templates, traders, globals)
                .map_err(PublishError::Views)?,
        )),
        _ => None,
    };

    *slot = Some(Arc::new(ResidentDb {
        epoch,
        templates,
        traders,
        globals,
        ragfair_views,
    }));

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

        // Populated store: a schema-2 publish leaves epoch and roots untouched too
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();
        let error = publish(request(r#"{"schema":2,"roots":{"templates":{"a":9}}}"#)).unwrap_err();
        assert!(matches!(error, PublishError::Schema(_)));
        let after = current().unwrap();
        assert_eq!(after.epoch, before.epoch);
        assert!(Arc::ptr_eq(
            before.templates.as_ref().unwrap(),
            after.templates.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.traders.as_ref().unwrap(),
            after.traders.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.globals.as_ref().unwrap(),
            after.globals.as_ref().unwrap()
        ));
    }

    #[test]
    fn full_publish_derives_ragfair_views_and_a_republish_rederives() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();

        // Not all three roots resident yet: no views
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":1}}}"#)).unwrap();
        assert!(current().unwrap().ragfair_views.is_none());

        // All three resident (junk roots parse as empty typed containers): views derive
        publish(request(
            r#"{"schema":1,"roots":{"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let first = current().unwrap();
        let first_views = first.ragfair_views.as_ref().expect("views derived");

        // A templates-only republish re-derives even though traders/globals are unchanged
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        let second = current().unwrap();
        let second_views = second.ragfair_views.as_ref().expect("views re-derived");
        assert!(!Arc::ptr_eq(first_views, second_views));
    }

    #[test]
    fn derivation_error_aborts_the_publish_before_the_swap() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();

        // A globals root that parses but fails derivation: an items-less preset is the C#
        // PresetController.Initialize NullReferenceException, surfaced as PublishError::Views
        let error = publish(request(
            r#"{"schema":1,"roots":{"globals":{"ItemPresets":{"bad":{"_id":"bad","_items":[]}}}}}"#,
        ))
        .unwrap_err();
        assert!(matches!(error, PublishError::Views(_)));

        let after = current().unwrap();
        assert_eq!(after.epoch, before.epoch);
        assert!(Arc::ptr_eq(
            before.globals.as_ref().unwrap(),
            after.globals.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.ragfair_views.as_ref().unwrap(),
            after.ragfair_views.as_ref().unwrap()
        ));
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
