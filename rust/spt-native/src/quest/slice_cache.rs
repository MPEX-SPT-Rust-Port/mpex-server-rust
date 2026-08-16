//! The repeatable-quest twin of [`crate::ragfair::slice_cache`]: the parsed quest invariant slice,
//! keyed by the caller's `DatabaseMutationStamp` value, in its own process-lifetime slot. A
//! separate static from ragfair's — the two families project different slices and move their
//! stamps independently.
//!
//! The slot key is the stamp alone, with no producer identity, so every caller that stores under a
//! stamp must have projected from the same live database; a second generator instance re-keying the
//! slot costs a stale-miss retry, never a wrong slice.

use std::sync::{Arc, Mutex};

use crate::quest::models::QuestInvariantSlice;

static CACHE: Mutex<Option<(i64, Arc<QuestInvariantSlice>)>> = Mutex::new(None);

/// Store `slice` under `stamp`, replacing whatever was cached, and hand it back so a full send
/// generates from the same allocation it cached.
pub fn store(stamp: i64, slice: QuestInvariantSlice) -> Arc<QuestInvariantSlice> {
    let slice = Arc::new(slice);
    *CACHE.lock().unwrap() = Some((stamp, Arc::clone(&slice)));

    slice
}

/// The slice one request generates from: the one it carried, cached under `stamp` on the way
/// through, or the cached slice when it carried none.
///
/// `None` is the stale case — a slice-less request naming a stamp this process has not stored,
/// which the caller answers by resending the slice (`STATUS_STALE_SLICE`). A miss is never a wrong
/// answer, only a retry.
pub fn take_or_stale(
    stamp: i64,
    invariant: Option<QuestInvariantSlice>,
) -> Option<Arc<QuestInvariantSlice>> {
    match invariant {
        Some(slice) => Some(store(stamp, slice)),
        None => fetch(stamp),
    }
}

/// The cached slice, only if it was stored under exactly `stamp`.
fn fetch(stamp: i64) -> Option<Arc<QuestInvariantSlice>> {
    CACHE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|(stored, slice)| (*stored == stamp).then(|| Arc::clone(slice)))
}

#[cfg(test)]
pub fn clear() {
    *CACHE.lock().unwrap() = None;
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::quest::models::tests::slice;

    /// Serialises the tests that touch the one static slot; the FFI export's cache tests import it
    /// the way ragfair's do.
    pub static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_slice_less_request_hits_the_slice_stored_under_its_stamp() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear();

        store(5, slice());

        assert!(take_or_stale(5, None).is_some());
    }

    #[test]
    fn a_slice_less_request_naming_an_uncached_stamp_is_stale() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear();

        store(5, slice());

        assert!(take_or_stale(6, None).is_none());
    }

    #[test]
    fn a_request_carrying_a_slice_replaces_the_cached_one() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap();
        clear();

        store(5, slice());
        take_or_stale(6, Some(slice()));

        assert!(take_or_stale(6, None).is_some());
        // Replaced, not added: the stamp it displaced is stale again
        assert!(take_or_stale(5, None).is_none());
    }
}
