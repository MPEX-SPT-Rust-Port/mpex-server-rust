//! Weighted draws over a pool of keys, mirroring `Utils/Collections/ProbabilityObjectArray.cs`.

use crate::loot::random_util;

/// One weighted entry, mirroring the C# `ProbabilityObject<K, V>`.
#[derive(Debug, Clone)]
pub struct ProbabilityObject<K, V> {
    pub key: K,
    pub relative_probability: f64,
    pub data: Option<V>,
}

/// The pool itself. The C# subclasses `List<ProbabilityObject<K, V>>`; here the vec is a field.
#[derive(Debug, Clone)]
pub struct ProbabilityObjectArray<K: Clone + PartialEq, V> {
    items: Vec<ProbabilityObject<K, V>>,
}

impl<K: Clone + PartialEq, V> Default for ProbabilityObjectArray<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + PartialEq, V> ProbabilityObjectArray<K, V> {
    /// An empty pool.
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Appends an entry, mirroring `List.Add`.
    pub fn add(&mut self, po: ProbabilityObject<K, V>) {
        self.items.push(po);
    }

    /// Entry count.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the pool holds no entries.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The data of the first entry with `key`, mirroring `Data` (`ProbabilityObjectArray.cs:94-98`).
    pub fn data(&self, key: &K) -> Option<&V> {
        self.items.iter().find(|po| po.key == *key)?.data.as_ref()
    }

    /// Draws `item_count_to_draw` keys with replacement, mirroring `Draw`
    /// (`ProbabilityObjectArray.cs:145-173`).
    ///
    /// An all-zero pool normalizes by a sum of 0 and every cumulative probability comes out NaN, so
    /// no index ever matches and the result is empty. Ported as-is — see the module tests.
    pub fn draw(&self, item_count_to_draw: usize) -> Vec<K> {
        if self.items.is_empty() {
            // Nothing in pool
            return Vec::new();
        }

        // `CumulativeProbability`: cumulative sum, then scaled by `1 / sum` (not divided by `sum`,
        // which would round differently and, at sum 0, give NaN by a different route).
        let sum: f64 = self.items.iter().map(|po| po.relative_probability).sum();
        let factor = 1.0 / sum;
        let mut running = 0.0;
        let cumulative_probabilities: Vec<f64> = self
            .items
            .iter()
            .map(|po| {
                running += po.relative_probability;
                running * factor
            })
            .collect();

        let mut results = Vec::with_capacity(item_count_to_draw);
        for _ in 0..item_count_to_draw {
            // `Random.Shared.NextDouble()` is a 53-bit uniform over [0, 1); `next_double53` is its
            // parity twin.
            let rand = random_util::next_double53();

            // `FindIndex`, so a NaN cumulative sum matches nothing and the draw is skipped.
            let Some(random_index) = cumulative_probabilities
                .iter()
                .position(|probability| *probability >= rand)
            else {
                continue;
            };

            results.push(self.items[random_index].key.clone());
        }

        results
    }

    /// Draws `item_count_to_draw` keys, removing each pick from the working pool unless it is
    /// whitelisted, mirroring `DrawAndRemove` (`ProbabilityObjectArray.cs:182-238`).
    ///
    /// The removals happen on a local copy, so despite the name the array itself is left whole —
    /// that is the C# behaviour (`ProbabilityObjectArray.cs:190,233`) and `GenerateDynamicLoot`
    /// depends on it, calling `data` for every key it just drew
    /// (`LocationLootGenerator.cs:736-738`). Hence `&self`: removing here would silently empty
    /// every dynamic-loot spawn point.
    pub fn draw_and_remove(
        &self,
        item_count_to_draw: usize,
        never_remove_whitelist: Option<&[K]>,
    ) -> Vec<K> {
        if self.items.is_empty() {
            // Nothing in pool
            return Vec::new();
        }

        let mut available_items: Vec<(K, f64)> = self
            .items
            .iter()
            .map(|po| (po.key.clone(), po.relative_probability))
            .collect();

        // Calculate total weighting of all items combined
        let mut total_weight: f64 = available_items.iter().map(|(_, weight)| *weight).sum();

        let mut drawn_keys = Vec::with_capacity(item_count_to_draw);

        // Loop until we have drawn to desired count or pool is empty
        for _ in 0..item_count_to_draw {
            if available_items.is_empty() {
                break;
            }

            // Get value between 0 and the total weight to act as a target to aim for
            // C# rolls `Random.Shared.NextDouble() * totalWeight` (`ProbabilityObjectArray.cs:202`).
            let mut random_target = random_util::next_double53() * total_weight;

            // Find element related to random target (greedy)
            let mut chosen_index = None;
            for (index, (_, weight)) in available_items.iter().enumerate() {
                // Subtract weight of item from above chosen value
                random_target -= *weight;
                if random_target <= 0.0 {
                    // Item falls within 'slice' of desired target
                    chosen_index = Some(index);
                    break;
                }
            }

            // If index not found choose the last element
            let chosen_index = chosen_index.unwrap_or(available_items.len() - 1);

            let (chosen_key, chosen_weight) = available_items[chosen_index].clone();
            drawn_keys.push(chosen_key.clone());

            // Only remove item if it's not in whitelist
            if !never_remove_whitelist.is_some_and(|whitelist| whitelist.contains(&chosen_key)) {
                // Reduce total weight value by items weight + Remove item from pool
                total_weight -= chosen_weight;
                available_items.remove(chosen_index);
            }
        }

        drawn_keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(entries: &[(&str, f64)]) -> ProbabilityObjectArray<String, String> {
        let mut array = ProbabilityObjectArray::new();
        for (key, weight) in entries {
            array.add(ProbabilityObject {
                key: (*key).to_string(),
                relative_probability: *weight,
                data: Some(format!("data-{key}")),
            });
        }

        array
    }

    fn count(keys: &[String], wanted: &str) -> usize {
        keys.iter().filter(|key| *key == wanted).count()
    }

    #[test]
    fn draw_on_an_empty_pool_returns_nothing() {
        let empty: ProbabilityObjectArray<String, String> = ProbabilityObjectArray::new();

        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert!(empty.draw(5).is_empty());
        assert!(empty.draw_and_remove(5, None).is_empty());
    }

    #[test]
    fn draw_follows_the_relative_probabilities() {
        let array = pool(&[("a", 5.0), ("b", 1.0), ("c", 1.0)]);

        let drawn = array.draw(10_000);

        assert_eq!(drawn.len(), 10_000);
        let a = count(&drawn, "a");
        // 5/7 = 71.4% expected; the band is ~20 standard deviations wide either way, so it cannot
        // flake, but it does catch an unnormalized or off-by-one cumulative walk.
        assert!((6000..=8200).contains(&a), "{a} draws of 'a' out of 10000");
        assert!(count(&drawn, "b") > 0 && count(&drawn, "c") > 0);
    }

    #[test]
    fn zero_weights_make_draw_return_nothing_but_draw_and_remove_pick_the_first() {
        // Deliberately asymmetric, and both halves are load-bearing for the loot tables.
        //
        // `Draw` normalizes by a sum of 0, so every cumulative probability is `0 * inf` = NaN, and
        // `FindIndex(p >= rand)` never matches -> nothing is drawn.
        //
        // `DrawAndRemove` aims at `rand * 0` = 0 instead, and the very first subtraction leaves
        // `0 - 0 <= 0` -> index 0 is picked every time.
        let array = pool(&[("a", 0.0), ("b", 0.0), ("c", 0.0)]);

        assert!(array.draw(5).is_empty());
        assert_eq!(array.draw_and_remove(1, None), vec!["a".to_string()]);
        assert_eq!(
            array.draw_and_remove(3, None),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn draw_and_remove_stops_once_the_pool_empties() {
        let array = pool(&[("a", 1.0), ("b", 2.0), ("c", 3.0)]);

        let mut drawn = array.draw_and_remove(10, None);

        assert_eq!(drawn.len(), 3);
        drawn.sort();
        assert_eq!(drawn, vec!["a", "b", "c"]);
    }

    #[test]
    fn draw_and_remove_never_removes_whitelisted_keys() {
        // The lopsided weights pin the other half of the whitelist branch: `total_weight` must be
        // left alone too, not just the pool. Decrementing it on a whitelisted pick would take it
        // 101 -> 1 -> -99, and since every aim value below 'a's weight of 100 resolves to 'a', 'b'
        // would become unreachable after the first draw. Over 5000 draws 'b' is otherwise certain
        // to come up (miss odds ~1e-21).
        let array = pool(&[("a", 100.0), ("b", 1.0)]);
        let whitelist = vec!["a".to_string()];

        let drawn = array.draw_and_remove(5000, Some(&whitelist));

        // 'a' stays in the pool forever, so the loop runs every iteration instead of stopping once
        // 'b' is gone.
        assert_eq!(drawn.len(), 5000);
        // Drawn once, then removed — never more, never less.
        assert_eq!(
            count(&drawn, "b"),
            1,
            "'b' was drawn the wrong number of times"
        );
    }

    #[test]
    fn draw_and_remove_falls_back_to_the_last_element_when_float_residue_remains() {
        // `total_weight` is summed once and then decremented per removal, so it drifts away from
        // the true pool weight. Here 1 + 1 + 1 + 1e16 rounds to 1e16 + 4, and removing the 1e16
        // leaves `total_weight` at 4.0 over a pool that really weighs 3.0. A quarter of the aim
        // values then land in (3, 4], the subtraction walk never crosses zero, and the
        // `chosen_index == -1 -> last element` fallback is what answers.
        let mut last = 0;
        let mut first = 0;
        let mut middle = 0;

        for _ in 0..10_000 {
            let array = pool(&[("a", 1.0), ("b", 1.0), ("c", 1.0), ("huge", 1e16)]);
            let drawn = array.draw_and_remove(2, None);

            assert_eq!(drawn.len(), 2);
            assert_eq!(drawn[0], "huge");
            match drawn[1].as_str() {
                "a" => first += 1,
                "b" => middle += 1,
                "c" => last += 1,
                other => panic!("drew {other}"),
            }
        }

        // Without the fallback 'c' would be a third of the picks; with it, half.
        assert!((4500..=5500).contains(&last), "'c' drawn {last} times");
        assert!((2000..=3000).contains(&first), "'a' drawn {first} times");
        assert!((2000..=3000).contains(&middle), "'b' drawn {middle} times");
    }

    #[test]
    fn draw_and_remove_leaves_the_array_itself_intact() {
        // The C# removes from a local copy of the pool, never from the array. `GenerateDynamicLoot`
        // (`LocationLootGenerator.cs:736-738`) depends on it: it calls `Data` for every key it just
        // drew, which would come back empty if the entries were gone.
        let array = pool(&[("a", 1.0), ("b", 1.0), ("c", 1.0)]);

        let drawn = array.draw_and_remove(3, None);

        assert_eq!(array.len(), 3);
        for key in &drawn {
            assert_eq!(array.data(key), Some(&format!("data-{key}")));
        }
    }

    #[test]
    fn data_returns_the_first_match() {
        let mut array = pool(&[("a", 1.0)]);
        array.add(ProbabilityObject {
            key: "a".to_string(),
            relative_probability: 1.0,
            data: Some("shadowed".to_string()),
        });

        assert_eq!(array.data(&"a".to_string()), Some(&"data-a".to_string()));
        assert_eq!(array.data(&"missing".to_string()), None);
    }

    #[test]
    fn a_seed_guard_makes_pool_draws_repeat() {
        let array = pool(&[("a", 5.0), ("b", 1.0), ("c", 1.0)]);

        let first = {
            let _guard = crate::loot::random_util::TestSeedGuard::install(7);
            (array.draw(10), array.draw_and_remove(3, None))
        };
        let _guard = crate::loot::random_util::TestSeedGuard::install(7);
        let second = (array.draw(10), array.draw_and_remove(3, None));

        assert_eq!(first, second);
    }

    /// Twin of `print_kat_vectors` in random_util.rs, for the pool draws. Run:
    /// `cargo test -p spt-native --lib print_pool_kat_vectors -- --ignored --nocapture`
    #[test]
    #[ignore = "generator for the pinned KAT constants, not an assertion"]
    fn print_pool_kat_vectors() {
        let array = pool(&[("a", 5.0), ("b", 1.0), ("c", 1.0)]);

        let draw5 = {
            let _g = crate::loot::random_util::TestSeedGuard::install(42);
            array.draw(5)
        };
        let draw_and_remove3 = {
            let _g = crate::loot::random_util::TestSeedGuard::install(42);
            array.draw_and_remove(3, None)
        };

        println!("POOL_DRAW5: {draw5:?}");
        println!("POOL_DRAW_AND_REMOVE3: {draw_and_remove3:?}");
    }

    #[test]
    fn kat_pool_draws_are_pinned() {
        // Twin assertions live in RandomSourceParityTests.cs (C#).
        let array = pool(&[("a", 5.0), ("b", 1.0), ("c", 1.0)]);

        {
            let _g = crate::loot::random_util::TestSeedGuard::install(42);
            assert_eq!(
                array.draw(5),
                vec![
                    "a".to_string(),
                    "a".to_string(),
                    "a".to_string(),
                    "c".to_string(),
                    "c".to_string()
                ]
            );
        }
        let _g = crate::loot::random_util::TestSeedGuard::install(42);
        assert_eq!(
            array.draw_and_remove(3, None),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
}
