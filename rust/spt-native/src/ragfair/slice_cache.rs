//! The one process-lifetime cache in the crate: the parsed ragfair invariant slice, keyed by the
//! caller's `DatabaseMutationStamp` value. Single sequential caller (the C# generator singleton),
//! so the mutex is uncontended; `Arc` lets a hit generate without holding the lock.

use std::sync::{Arc, Mutex};

use crate::ragfair::models::{InvariantSlice, PreparedSlice};

static CACHE: Mutex<Option<(i64, Arc<PreparedSlice>)>> = Mutex::new(None);

/// Store `slice` under `stamp`, replacing whatever was cached, and hand back the prepared form
/// so a full send generates from the same allocation it cached.
pub fn store(stamp: i64, slice: InvariantSlice) -> Arc<PreparedSlice> {
    let prepared = Arc::new(PreparedSlice::from(slice));
    *CACHE.lock().unwrap() = Some((stamp, Arc::clone(&prepared)));

    prepared
}

/// The cached slice, only if it was stored under exactly `stamp`.
pub fn fetch(stamp: i64) -> Option<Arc<PreparedSlice>> {
    CACHE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|(stored, prepared)| (*stored == stamp).then(|| Arc::clone(prepared)))
}

#[cfg(test)]
pub fn clear() {
    *CACHE.lock().unwrap() = None;
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Serialises the tests that touch the one static slot; ffi.rs's cache tests import it too.
    pub static CACHE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The models fixture parsed whole. What the slot holds is opaque to the cache, so the cheapest
    /// valid slice is the one the wire tests already pin.
    fn minimal_slice() -> InvariantSlice {
        use crate::ragfair::models::tests::{DYNAMIC_JSON, INVARIANT_TAIL};

        serde_json::from_str(&format!("{{\"dynamic\":{DYNAMIC_JSON},{INVARIANT_TAIL}}}")).unwrap()
    }

    #[test]
    fn fetch_misses_when_empty_or_mismatched() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear();
        assert!(fetch(1).is_none());
        store(1, minimal_slice());
        assert!(fetch(2).is_none());
    }

    #[test]
    fn fetch_hits_on_the_stored_stamp() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        store(7, minimal_slice());
        assert!(fetch(7).is_some());
    }
}
