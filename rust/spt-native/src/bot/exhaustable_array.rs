//! Draw-without-replacement over a pool, ported from `Utils/Collections/ExhaustableArray.cs`.
//!
//! Crate-internal, so nothing outside the tests below reaches it until the mod-generation tasks
//! land — hence the blanket allow. Drop it once they do. Not an `expect`: the lint is per-target
//! and the tests here already use every item, so an `expect` would go unfulfilled under `cfg(test)`
//! and fail `clippy --all-targets`.
#![allow(
    dead_code,
    reason = "consumed by the bot mod-generation tasks that follow"
)]

use crate::loot::random_util::get_int;

/// A pool each draw permanently removes from, mirroring `ExhaustableArray<T>`.
///
/// The C# holds a `LinkedList<T>` and draws with `ElementAt(index)` + `Remove(element)`; a `Vec<T>`
/// with `remove(index)` is the same walk-to-index-then-unlink, so the surviving order — and with it
/// every following index draw — matches. The one place the two can part is a pool holding equal
/// values: `LinkedList.Remove` unlinks the *first* node equal to the drawn one, not the node at
/// `index`, so C# on `[a, b, a]` drawing index 2 leaves `[b, a]` where this leaves `[a, b]`. Every
/// ported call site fills the pool from a set of distinct template ids, so the two never diverge in
/// practice.
///
/// The C# `cloner.Clone` on the way out is not ported: the element is removed from the pool, so the
/// value handed back is already the caller's own and nothing else can observe a mutation of it.
pub struct ExhaustableArray<T> {
    pool: Vec<T>,
}

impl<T> ExhaustableArray<T> {
    pub fn new(item_pool: Vec<T>) -> Self {
        Self { pool: item_pool }
    }

    /// A random element, removed from the pool; `None` once exhausted.
    ///
    /// The empty check comes first, as in the C#, so an exhausted pool consumes no draw — call-site
    /// stream parity depends on it.
    pub fn get_random_value(&mut self) -> Option<T> {
        if self.pool.is_empty() {
            return None;
        }

        // Inclusive at both ends, so the last element is reachable — as `GetInt(0, Count - 1)` is.
        let index = get_int(0, self.pool.len() as i32 - 1) as usize;

        Some(self.pool.remove(index))
    }

    /// The head of the pool, removed from it; `None` once exhausted. Consumes no draw either way.
    pub fn get_first_value(&mut self) -> Option<T> {
        if self.pool.is_empty() {
            return None;
        }

        Some(self.pool.remove(0))
    }

    pub fn has_values(&self) -> bool {
        !self.pool.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::loot::random_util::TestSeedGuard;

    const SEED: u64 = 42;
    const POOL: [i32; 5] = [10, 20, 30, 40, 50];
    /// Pinned from the first run; a permutation of [`POOL`], since every draw removes.
    const DRAWN_UNDER_SEED_42: [i32; 5] = [20, 30, 10, 40, 50];

    fn pool() -> ExhaustableArray<i32> {
        ExhaustableArray::new(POOL.to_vec())
    }

    #[test]
    fn seeded_draws_are_pinned() {
        let _guard = TestSeedGuard::install(SEED);
        let mut array = pool();
        let drawn: Vec<i32> = (0..POOL.len())
            .filter_map(|_| array.get_random_value())
            .collect();

        assert_eq!(drawn, DRAWN_UNDER_SEED_42);
    }

    #[test]
    fn every_element_is_drawn_exactly_once_before_the_pool_exhausts() {
        let _guard = TestSeedGuard::install(SEED);
        let mut array = pool();
        let drawn: Vec<i32> = std::iter::from_fn(|| array.get_random_value()).collect();

        assert_eq!(drawn.len(), POOL.len(), "a draw was repeated or skipped");
        assert_eq!(
            drawn.iter().copied().collect::<HashSet<i32>>(),
            POOL.into_iter().collect::<HashSet<i32>>()
        );
        assert_eq!(array.get_random_value(), None, "the pool refilled itself");
    }

    #[test]
    fn has_values_flips_as_the_pool_empties() {
        let _guard = TestSeedGuard::install(SEED);
        let mut array = pool();

        for remaining in (1..=POOL.len()).rev() {
            assert!(array.has_values(), "{remaining} left but reported empty");
            array.get_random_value().expect("a value is left");
        }

        assert!(!array.has_values());
    }

    #[test]
    fn an_exhausted_pool_consumes_no_draw() {
        // The C# returns `default` before touching the RNG, so draws either side of an exhausted
        // pool are one uninterrupted stream. Call-site parity depends on it.
        let baseline: Vec<i32> = {
            let _guard = TestSeedGuard::install(SEED);
            (0..3).map(|_| get_int(1, 10)).collect()
        };

        let _guard = TestSeedGuard::install(SEED);
        let mut empty: ExhaustableArray<i32> = ExhaustableArray::new(Vec::new());
        for _ in 0..3 {
            assert_eq!(empty.get_random_value(), None);
            assert_eq!(empty.get_first_value(), None);
        }
        let after: Vec<i32> = (0..3).map(|_| get_int(1, 10)).collect();

        assert_eq!(after, baseline);
        assert!(!empty.has_values());
    }

    #[test]
    fn get_first_value_walks_the_pool_in_order_without_drawing() {
        let baseline: Vec<i32> = {
            let _guard = TestSeedGuard::install(SEED);
            (0..3).map(|_| get_int(1, 10)).collect()
        };

        let _guard = TestSeedGuard::install(SEED);
        let mut array = pool();
        let taken: Vec<i32> = std::iter::from_fn(|| array.get_first_value()).collect();
        let after: Vec<i32> = (0..3).map(|_| get_int(1, 10)).collect();

        assert_eq!(taken, POOL);
        assert_eq!(after, baseline, "`GetFirstValue` consumed a draw");
    }

    #[test]
    fn a_single_element_pool_draws_that_element() {
        // `GetInt(0, 0)` returns 0 without consuming, so the lone element comes back and the pool
        // empties.
        let _guard = TestSeedGuard::install(SEED);
        let mut array = ExhaustableArray::new(vec![7]);

        assert_eq!(array.get_random_value(), Some(7));
        assert_eq!(array.get_random_value(), None);
    }
}
