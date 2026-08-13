//! The draw semantics of `Utils/RandomUtil.cs`, ported bug-for-bug.
//!
//! Production draws come from thread entropy, as the C# ones come from a CSPRNG — no sequence
//! parity between unseeded runs. Under a [`TestSeedGuard`] (installed by the `testSeed` request
//! field) every draw instead comes from a seeded xoshiro256** whose sequences are bit-identical
//! to `SeededRandomSource` in `Utils/RandomSource.cs`, pinned by the KAT tests below and by
//! `RandomSourceParityTests.cs`.

use std::cell::RefCell;

use indexmap::IndexMap;
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;

use super::item_helper::LootError;

thread_local! {
    /// The test-only seeded generator; `None` means production entropy.
    static TEST_RNG: RefCell<Option<Xoshiro256StarStar>> = const { RefCell::new(None) };

    /// Where the stream of the last seeded [`TestSeedGuard::install`] on this thread ended, kept for
    /// a following [`TestSeedGuard::resume`] with the same seed to carry on from. Never drawn from
    /// while parked, so an unseeded run cannot reach it.
    static PARKED_RNG: RefCell<Option<(u64, Xoshiro256StarStar)>> = const { RefCell::new(None) };
}

/// Routes every draw on this thread through a seeded xoshiro256** until dropped. Installed by the
/// `testSeed` request field at the FFI entry points; RAII so a panic during generation cannot leak
/// a seeded state onto a pooled thread.
#[must_use = "the seeded override is uninstalled as soon as the guard is dropped"]
pub struct TestSeedGuard {
    /// Whatever occupied the slot before this guard, restored on drop rather than cleared, so a
    /// nested install cannot silently drop its caller back to entropy.
    previous: Option<Xoshiro256StarStar>,
    /// The seed to park this guard's stream under on drop; `None` parks nothing.
    park_under: Option<u64>,
}

impl TestSeedGuard {
    /// A fresh stream from `seed`, parked on drop for a [`resume`](Self::resume) to carry on from.
    pub fn install(seed: u64) -> Self {
        Self::replace(xoshiro_from_u64(seed), Some(seed))
    }

    /// Carries on from the stream a preceding [`install`](Self::install) with the same seed parked,
    /// or starts fresh from `seed` when there is none.
    ///
    /// This is what keeps one location's generation on one stream. C# installs a single
    /// `SeededRandomSource` for the whole of `GenerateLocationLoot` and draws from it in the static
    /// phase before the dynamic one, where the native side is entered once per phase — so the
    /// dynamic entry point has to pick the stream up where the static entry point left it rather
    /// than restart it, or the two phases replay the same draw values.
    ///
    /// The park is consumed, so a second resume with no static run in between starts fresh again.
    pub fn resume(seed: u64) -> Self {
        let parked = PARKED_RNG.with(|slot| {
            let mut slot = slot.borrow_mut();
            match slot.as_ref() {
                Some((parked_seed, _)) if *parked_seed == seed => slot.take().map(|(_, rng)| rng),
                _ => None,
            }
        });

        Self::replace(parked.unwrap_or_else(|| xoshiro_from_u64(seed)), None)
    }

    fn replace(rng: Xoshiro256StarStar, park_under: Option<u64>) -> Self {
        let previous = TEST_RNG.with(|slot| slot.borrow_mut().replace(rng));

        Self {
            previous,
            park_under,
        }
    }
}

impl Drop for TestSeedGuard {
    fn drop(&mut self) {
        let ended =
            TEST_RNG.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), self.previous.take()));

        PARKED_RNG.with(|slot| {
            *slot.borrow_mut() = self.park_under.zip(ended);
        });
    }
}

/// splitmix64; parity twin of `Xoshiro256StarStar.SplitMix64` in `Utils/RandomSource.cs`.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Seed expansion pinned here rather than trusting `SeedableRng::seed_from_u64`, so the C# twin
/// replicates this exact function, not a trait default that could change underneath it.
fn xoshiro_from_u64(seed: u64) -> Xoshiro256StarStar {
    let mut state = seed;
    let mut seed_bytes = [0u8; 32];
    for chunk in seed_bytes.chunks_exact_mut(8) {
        chunk.copy_from_slice(&splitmix64(&mut state).to_le_bytes());
    }

    Xoshiro256StarStar::from_seed(seed_bytes)
}

/// One raw draw: the seeded override when installed, thread entropy otherwise.
fn next_u64() -> u64 {
    TEST_RNG.with(|slot| match slot.borrow_mut().as_mut() {
        Some(rng) => rng.next_u64(),
        None => rand::rng().next_u64(),
    })
}

/// Uniform in `[0, range)` by bitmask rejection; parity twin of `SeededRandomSource.NextBelow` in
/// `Utils/RandomSource.cs`. The canonical range algorithm both languages share — deliberately not
/// `RandomNumberGenerator.GetInt32`'s internals nor `rand`'s.
fn next_below(range: u64) -> u64 {
    if range <= 1 {
        return 0;
    }
    let mask = u64::MAX >> (range - 1).leading_zeros();
    loop {
        let value = next_u64() & mask;
        if value < range {
            return value;
        }
    }
}

/// Uniform in `[from_inclusive, to_exclusive)`; parity twin of `SeededRandomSource.GetInt32`.
fn get_int32(from_inclusive: i32, to_exclusive: i32) -> i32 {
    let range = (i64::from(to_exclusive) - i64::from(from_inclusive)) as u64;
    (i64::from(from_inclusive) + next_below(range) as i64) as i32
}

/// Uniform `[0, 1)` from 48 random bits with 0 folded to 1 — the shape of
/// `RandomUtil.GetSecureRandomNumber` (`RandomUtil.cs:465-478`); parity twin of
/// `SeededRandomSource.NextDouble48`.
fn next_double48() -> f64 {
    let mut value = next_u64() & 0x0000_FFFF_FFFF_FFFF;
    if value == 0 {
        value = 1;
    }

    value as f64 / 281_474_976_710_656.0
}

/// Uniform `[0, 1)` from 53 random bits — the shape of `Random.Shared.NextDouble()`; parity twin
/// of `SeededRandomSource.NextDouble53`. `ProbabilityObjectArray` draws with this, not the 48-bit
/// helper, because its C# original does.
pub fn next_double53() -> f64 {
    (next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
}

/// A random integer in `min..=max`, inclusive at both ends. `max <= min` yields `min`, matching
/// `RandomUtil.GetInt` (`RandomUtil.cs:35-50`) — including its fold of `int.MaxValue` down to an
/// exclusive bound of `int.MaxValue - 1`.
pub fn get_int(min: i32, max: i32) -> i32 {
    let (max, exclusive) = if max == i32::MAX {
        (i32::MAX - 1, true)
    } else {
        (max, false)
    };

    if max > min {
        get_int32(min, if exclusive { max } else { max + 1 })
    } else {
        min
    }
}

/// A random float in `[min, max)`, matching `RandomUtil.GetDouble` (`RandomUtil.cs:77-81`).
pub fn get_double(min: f64, max: f64) -> f64 {
    // Same shape as the C#, so an inverted range walks below `min` here too instead of panicking.
    min + next_double48() * (max - min)
}

/// Whether an event with `chance_percent` (0-100) fires, matching `RandomUtil.GetChance100`
/// (`RandomUtil.cs:145-150`).
///
/// The C# rolls `GetInt(1, 100, exclusive: true)` — an *integer* in 1-99 — so anything under 1%
/// never fires and anything at or above 99% always does. Ported as-is; the loot tables rely on it.
pub fn get_chance_100(chance_percent: f64) -> bool {
    // The C# `Math.Clamp(chance, 0, 100)` is a no-op against a 1-99 roll, so it is not ported.
    f64::from(get_int(1, 99)) <= chance_percent
}

/// A normally distributed draw via the Box-Muller transform, matching
/// `RandomUtil.GetNormallyDistributedRandomNumber` (`RandomUtil.cs:215-246`).
///
/// Negative draws are rerolled. The C# checks `attempt > 100` *after* drawing and recurses with
/// `attempt + 1`, so 102 draws are made before it gives up and returns a flat
/// `get_double(0.01, mean * 2)` instead. This loops where the C# recurses — same count, same
/// fallback.
pub fn get_normally_distributed_random_number(mean: f64, sigma: f64) -> f64 {
    let mut attempt = 0;

    loop {
        // `next_double48` already folds 0 to 1, as the C# helper does, so neither loop can spin;
        // they are kept for bug-for-bug shape parity with the C#.
        let mut u = 0.0;
        while u == 0.0 {
            u = next_double48();
        }

        let mut v = 0.0;
        while v == 0.0 {
            v = next_double48();
        }

        let w = (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos();
        let value_drawn = mean + w * sigma;

        if value_drawn < 0.0 {
            if attempt > 100 {
                return get_double(0.01, mean * 2.0);
            }

            attempt += 1;
            continue;
        }

        return value_drawn;
    }
}

/// A random element of `list`, matching `RandomUtil.GetRandomElement` (`RandomUtil.cs:159-175`).
///
/// # Panics
///
/// If `list` is empty, as the C# throws on an empty collection.
pub fn get_array_value<T>(list: &[T]) -> &T {
    &list[get_int(0, list.len() as i32 - 1) as usize]
}

/// A key drawn in proportion to its weight, matching `WeightedRandomHelper.GetWeightedValue`
/// through `WeightedRandom` (`WeightedRandomHelper.cs:23-108`). Insertion order is the C#
/// `Dictionary` enumeration order the original scans, so the map has to stay ordered.
///
/// Three paths, each consuming different RNG — call-site parity depends on which one runs:
/// a single entry returns without drawing at all, weights summing to the entry count take one
/// `get_int`, and everything else takes one `get_double`.
///
/// The C#'s `logger.Error` calls for empty or mismatched inputs are not ported: keys and weights
/// come from one map here so they cannot disagree, and no ported call site passes an empty one.
/// Its `logger.Warning("Weight at index: {i} is negative...")` is dropped too — the negative weight
/// is still skipped with the same bug-for-bug effect below, only the log line is missing, so a mod
/// shipping negative weights gets the same items without the diagnostic it would see on 4.1.2.
///
/// # Errors
///
/// Where the C# throws: an empty map (its uniform shortcut indexes out of bounds), or a scan that
/// falls off the end ("No item was picked.").
pub fn get_weighted_value(values: &IndexMap<String, f64>) -> Result<String, LootError> {
    if values.len() == 1
        && let Some(key) = values.keys().next()
    {
        return Ok(key.clone());
    }

    let mut cumulative_weights = vec![0.0; values.len()];
    let mut sum_of_weights = 0.0;
    for (index, weight) in values.values().enumerate() {
        // Bug-for-bug: a skipped weight leaves its slot at 0 rather than at the running sum, so a
        // zeroed slot can still win the `>= random_number` scan below when the sum is 0.
        if *weight < 0.0 {
            continue;
        }

        sum_of_weights += weight;
        cumulative_weights[index] = sum_of_weights;
    }

    #[expect(
        clippy::float_cmp,
        reason = "the C# compares exactly; weights averaging 1.0 is the intended trigger"
    )]
    if sum_of_weights == values.len() as f64 {
        // Weights are all the same, early exit
        let random_index = get_int(0, values.len() as i32 - 1);
        return values
            .keys()
            .nth(random_index as usize)
            .cloned()
            .ok_or_else(|| LootError::new("No item was picked."));
    }

    // Getting the random number in a range of [0...sum(weights)]
    let random_number = sum_of_weights * get_double(0.0, 1.0);

    // Picking the random item based on its weight.
    for (index, key) in values.keys().enumerate() {
        if cumulative_weights[index] >= random_number {
            return Ok(key.clone());
        }
    }

    Err(LootError::new("No item was picked."))
}

/// Rounds half to even, matching the default C# `Math.Round(double)`. Ported call sites must use
/// this and never `f64::round`, which rounds halves away from zero.
pub fn round_half_even(value: f64) -> f64 {
    value.round_ties_even()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn get_int_returns_min_when_max_is_not_above_min() {
        assert_eq!(get_int(5, 5), 5);
        assert_eq!(get_int(5, 3), 5);
        assert_eq!(get_int(-2, -7), -2);
    }

    #[test]
    fn get_int_is_inclusive_at_both_ends() {
        let drawn: HashSet<i32> = (0..1000).map(|_| get_int(1, 3)).collect();
        assert_eq!(drawn, HashSet::from([1, 2, 3]));
    }

    #[test]
    fn get_double_stays_within_the_requested_range() {
        for _ in 0..1000 {
            let value = get_double(2.0, 5.0);
            assert!((2.0..5.0).contains(&value), "{value} is outside [2, 5)");
        }
    }

    #[test]
    fn get_chance_100_never_fires_below_one_percent() {
        // The C# rolls an integer 1-99, so any chance under 1% is unreachable. Bug-compatible.
        for _ in 0..1000 {
            assert!(!get_chance_100(0.0));
            assert!(!get_chance_100(0.5));
        }
    }

    #[test]
    fn get_chance_100_always_fires_at_one_hundred_percent() {
        for _ in 0..1000 {
            assert!(get_chance_100(100.0));
        }
    }

    #[test]
    fn get_chance_100_always_fires_at_ninety_nine_percent() {
        // The roll tops out at 99, so 99% is already a certainty. This is the assertion that
        // discriminates the ported 1-99 roll from an innocent-looking `get_int(1, 100)`: under
        // 1-100 a roll of 100 loses here ~1% of the time, which 1000 trials catch. The 100.0 case
        // above cannot tell the two apart, since `roll <= 100` holds either way.
        for _ in 0..1000 {
            assert!(get_chance_100(99.0));
        }
    }

    #[test]
    fn get_normally_distributed_random_number_centres_on_the_mean() {
        let samples: Vec<f64> = (0..10_000)
            .map(|_| get_normally_distributed_random_number(100.0, 10.0))
            .collect();
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;

        assert!((99.0..101.0).contains(&mean), "sample mean {mean} drifted");
        assert!(samples.iter().all(|value| *value >= 0.0), "drew a negative");
    }

    #[test]
    fn get_normally_distributed_random_number_never_returns_negatives() {
        // sigma this far past the mean means nearly every draw is rejected, exercising the reroll
        // loop and its `get_double(0.01, mean * 2)` fallback.
        for _ in 0..100 {
            let value = get_normally_distributed_random_number(1.0, 1000.0);
            assert!(value >= 0.0, "{value} is negative");
        }
    }

    #[test]
    fn get_normally_distributed_random_number_falls_back_once_the_rerolls_run_out() {
        // A mean 1000 sigma below zero can never draw non-negative, so the reroll cap is always
        // reached and the flat `get_double(0.01, mean * 2)` fallback is what comes back. Proves the
        // loop terminates, and pins the C# quirk that the fallback ignores its own sign check.
        for _ in 0..10 {
            let value = get_normally_distributed_random_number(-1000.0, 1.0);
            assert!(
                (-2000.0..=0.01).contains(&value),
                "{value} is not a `get_double(0.01, -2000)` draw"
            );
        }
    }

    #[test]
    fn get_array_value_returns_an_element_of_the_list() {
        let list = ["a", "b", "c"];
        let drawn: HashSet<&str> = (0..1000).map(|_| *get_array_value(&list)).collect();
        assert_eq!(drawn, HashSet::from(["a", "b", "c"]));
    }

    #[test]
    fn round_half_even_rounds_ties_to_even() {
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(-0.5), 0.0);
        assert_eq!(round_half_even(2.4), 2.0);
    }

    #[test]
    fn a_seed_guard_makes_every_draw_repeat_bit_for_bit() {
        let ints_a: Vec<i32>;
        let doubles_a: Vec<u64>;
        {
            let _guard = TestSeedGuard::install(42);
            ints_a = (0..100).map(|_| get_int(1, 1000)).collect();
            doubles_a = (0..100).map(|_| get_double(0.0, 1.0).to_bits()).collect();
        }

        let _guard = TestSeedGuard::install(42);
        let ints_b: Vec<i32> = (0..100).map(|_| get_int(1, 1000)).collect();
        let doubles_b: Vec<u64> = (0..100).map(|_| get_double(0.0, 1.0).to_bits()).collect();

        assert_eq!(ints_a, ints_b);
        assert_eq!(doubles_a, doubles_b);
    }

    #[test]
    fn different_seeds_diverge() {
        let a: Vec<i32> = {
            let _guard = TestSeedGuard::install(1);
            (0..20).map(|_| get_int(0, i32::MAX - 2)).collect()
        };
        let b: Vec<i32> = {
            let _guard = TestSeedGuard::install(2);
            (0..20).map(|_| get_int(0, i32::MAX - 2)).collect()
        };

        assert_ne!(a, b);
    }

    #[test]
    fn a_resumed_guard_carries_on_where_the_installed_one_stopped() {
        // The whole point of `resume`: two calls, one stream. C# hands both loot phases the same
        // `SeededRandomSource`, so the dynamic phase must see the draws that follow the static
        // phase's, not the same ones over again.
        let continuous: Vec<u64> = {
            let _guard = TestSeedGuard::install(9);
            (0..6).map(|_| next_u64()).collect()
        };

        let mut split: Vec<u64> = Vec::new();
        {
            let _guard = TestSeedGuard::install(9);
            split.extend((0..3).map(|_| next_u64()));
        }
        {
            let _guard = TestSeedGuard::resume(9);
            split.extend((0..3).map(|_| next_u64()));
        }

        assert_eq!(continuous, split);
    }

    #[test]
    fn a_resume_starts_fresh_without_a_parked_stream_of_its_own_seed() {
        let fresh: Vec<u64> = {
            let _guard = TestSeedGuard::install(9);
            (0..3).map(|_| next_u64()).collect()
        };

        // Parked under 8, so the resume of 9 cannot take it.
        {
            let _guard = TestSeedGuard::install(8);
            next_u64();
        }
        let after_other_seed: Vec<u64> = {
            let _guard = TestSeedGuard::resume(9);
            (0..3).map(|_| next_u64()).collect()
        };

        // Nothing parked at all, and the park is single-use: a second resume starts over.
        {
            let _guard = TestSeedGuard::install(9);
            next_u64();
        }
        let first_resume: Vec<u64> = {
            let _guard = TestSeedGuard::resume(9);
            (0..3).map(|_| next_u64()).collect()
        };
        let second_resume: Vec<u64> = {
            let _guard = TestSeedGuard::resume(9);
            (0..3).map(|_| next_u64()).collect()
        };

        assert_eq!(after_other_seed, fresh);
        assert_ne!(first_resume, fresh, "the parked stream was not picked up");
        assert_eq!(second_resume, fresh);
    }

    #[test]
    fn dropping_the_guard_restores_entropy() {
        {
            let _guard = TestSeedGuard::install(42);
        }

        TEST_RNG.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn next_double53_stays_in_the_unit_interval() {
        for _ in 0..1000 {
            let value = next_double53();
            assert!((0.0..1.0).contains(&value), "{value} escaped [0, 1)");
        }
    }

    /// Generates the cross-language KAT constants. Run:
    /// `cargo test -p spt-native --lib print_kat_vectors -- --ignored --nocapture`
    /// and paste each printed line into the KAT constants below and into
    /// `Testing/UnitTests/Tests/Utils/RandomSourceParityTests.cs`.
    #[test]
    #[ignore = "generator for the pinned KAT constants, not an assertion"]
    fn print_kat_vectors() {
        fn hex(values: &[u64]) -> String {
            let items: Vec<String> = values.iter().map(|v| format!("0x{v:016X}")).collect();
            items.join(", ")
        }

        let raw: Vec<u64> = {
            let _g = TestSeedGuard::install(42);
            (0..4).map(|_| next_u64()).collect()
        };
        let d48: Vec<u64> = {
            let _g = TestSeedGuard::install(42);
            (0..3).map(|_| next_double48().to_bits()).collect()
        };
        let d53: Vec<u64> = {
            let _g = TestSeedGuard::install(42);
            (0..3).map(|_| next_double53().to_bits()).collect()
        };
        let fill5: Vec<u8> = {
            let _g = TestSeedGuard::install(42);
            next_u64().to_le_bytes()[..5].to_vec()
        };
        let ints: Vec<i32> = {
            let _g = TestSeedGuard::install(42);
            (0..5).map(|_| get_int(1, 10)).collect()
        };
        let doubles: Vec<u64> = {
            let _g = TestSeedGuard::install(42);
            (0..3).map(|_| get_double(0.0, 100.0).to_bits()).collect()
        };
        let chances: Vec<bool> = {
            let _g = TestSeedGuard::install(42);
            (0..5).map(|_| get_chance_100(50.0)).collect()
        };

        println!("RAW_U64: [{}]", hex(&raw));
        println!("NEXT_DOUBLE48_BITS: [{}]", hex(&d48));
        println!("NEXT_DOUBLE53_BITS: [{}]", hex(&d53));
        println!("FILL5: {fill5:#04X?}");
        println!("GET_INT_1_10: {ints:?}");
        println!("GET_DOUBLE_0_100_BITS: [{}]", hex(&doubles));
        let weighted: (Vec<String>, Vec<String>, Vec<String>) = {
            let _g = TestSeedGuard::install(42);
            (
                draw(&kat_mixed_map(), 5),
                draw(&kat_single_map(), 1),
                draw(&kat_uniform_map(), 3),
            )
        };

        println!("GET_CHANCE100_50: {chances:?}");
        println!("WEIGHTED_MIXED_5: {:?}", weighted.0);
        println!("WEIGHTED_SINGLE: {:?}", weighted.1);
        println!("WEIGHTED_UNIFORM_3: {:?}", weighted.2);
    }

    /// The KAT map keys, shared with the C# twin where they have to parse as `MongoId`s — hence
    /// 24 hex characters rather than "a"/"b"/"c"/"d".
    fn kat_keys() -> [&'static str; 4] {
        [
            "aaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbb",
            "cccccccccccccccccccccccc",
            "dddddddddddddddddddddddd",
        ]
    }

    /// `{a:5, b:0, c:1, d:1}` — sum 7 against 4 entries, so the general cumulative-scan path.
    fn kat_mixed_map() -> IndexMap<String, f64> {
        kat_keys()
            .into_iter()
            .map(str::to_owned)
            .zip([5.0, 0.0, 1.0, 1.0])
            .collect()
    }

    /// All 1.0 — sum equals the entry count, so the uniform `get_int` shortcut.
    fn kat_uniform_map() -> IndexMap<String, f64> {
        kat_keys()
            .into_iter()
            .map(str::to_owned)
            .zip([1.0; 4])
            .collect()
    }

    fn kat_single_map() -> IndexMap<String, f64> {
        IndexMap::from([(kat_keys()[1].to_owned(), 3.0)])
    }

    fn draw(map: &IndexMap<String, f64>, count: usize) -> Vec<String> {
        (0..count)
            .map(|_| get_weighted_value(map).expect("a key is always picked"))
            .collect()
    }

    // ---- Cross-language KAT pins. Twin fixture: RandomSourceParityTests.cs (C#). ----
    // Regenerate with `print_kat_vectors` only if a derivation changes deliberately — and then
    // update the C# twin in the same commit.

    const KAT_SEED: u64 = 42;
    const KAT_RAW_U64: [u64; 4] = [
        0x1578_0B2E_0C2E_C716,
        0x6104_D986_6D11_3A7E,
        0xAE17_5332_39E4_99A1,
        0xECB8_AD47_03B3_60A1,
    ];
    const KAT_NEXT_DOUBLE48_BITS: [u64; 3] = [
        0x3FA6_5C18_5D8E_2C00,
        0x3FEB_30CD_A227_4FC0,
        0x3FD4_CC8E_7926_6840,
    ];
    const KAT_NEXT_DOUBLE53_BITS: [u64; 3] = [
        0x3FB5_780B_2E0C_2EC0,
        0x3FD8_4136_619B_444E,
        0x3FE5_C2EA_6647_3C93,
    ];
    const KAT_FILL5: [u8; 5] = [0x16, 0xC7, 0x2E, 0x0C, 0x2E];
    const KAT_GET_INT_1_10: [i32; 5] = [7, 2, 2, 5, 9];
    const KAT_GET_DOUBLE_0_100_BITS: [u64; 3] = [
        0x4011_77F3_0917_1260,
        0x4055_3E20_A6AE_B64E,
        0x4040_3FCF_4EA6_0172,
    ];
    const KAT_GET_CHANCE100_50: [bool; 5] = [true, true, true, false, false];
    /// Five general-path draws from `kat_mixed_map`, then the single-entry map (no draw), then
    /// three uniform-shortcut draws from `kat_uniform_map` — one continuous stream.
    const KAT_WEIGHTED_MIXED_5: [&str; 5] = [
        "aaaaaaaaaaaaaaaaaaaaaaaa",
        "cccccccccccccccccccccccc",
        "aaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaa",
        "dddddddddddddddddddddddd",
    ];
    const KAT_WEIGHTED_SINGLE: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const KAT_WEIGHTED_UNIFORM_3: [&str; 3] = [
        "aaaaaaaaaaaaaaaaaaaaaaaa",
        "cccccccccccccccccccccccc",
        "dddddddddddddddddddddddd",
    ];

    #[test]
    fn kat_raw_u64_sequence_is_pinned() {
        let _g = TestSeedGuard::install(KAT_SEED);
        let raw: Vec<u64> = (0..4).map(|_| next_u64()).collect();
        assert_eq!(raw, KAT_RAW_U64);
    }

    #[test]
    fn kat_double_derivations_are_pinned() {
        {
            let _g = TestSeedGuard::install(KAT_SEED);
            let bits: Vec<u64> = (0..3).map(|_| next_double48().to_bits()).collect();
            assert_eq!(bits, KAT_NEXT_DOUBLE48_BITS);
        }
        let _g = TestSeedGuard::install(KAT_SEED);
        let bits: Vec<u64> = (0..3).map(|_| next_double53().to_bits()).collect();
        assert_eq!(bits, KAT_NEXT_DOUBLE53_BITS);
    }

    #[test]
    fn kat_fill_bytes_are_pinned() {
        let _g = TestSeedGuard::install(KAT_SEED);
        assert_eq!(next_u64().to_le_bytes()[..5], KAT_FILL5);
    }

    #[test]
    fn kat_public_draws_are_pinned() {
        {
            let _g = TestSeedGuard::install(KAT_SEED);
            let ints: Vec<i32> = (0..5).map(|_| get_int(1, 10)).collect();
            assert_eq!(ints, KAT_GET_INT_1_10);
        }
        {
            let _g = TestSeedGuard::install(KAT_SEED);
            let bits: Vec<u64> = (0..3).map(|_| get_double(0.0, 100.0).to_bits()).collect();
            assert_eq!(bits, KAT_GET_DOUBLE_0_100_BITS);
        }
        let _g = TestSeedGuard::install(KAT_SEED);
        let chances: Vec<bool> = (0..5).map(|_| get_chance_100(50.0)).collect();
        assert_eq!(chances, KAT_GET_CHANCE100_50);
    }

    #[test]
    fn kat_weighted_values_are_pinned() {
        let _g = TestSeedGuard::install(KAT_SEED);
        let mixed = draw(&kat_mixed_map(), 5);
        let single = draw(&kat_single_map(), 1);
        let uniform = draw(&kat_uniform_map(), 3);

        assert_eq!(mixed, KAT_WEIGHTED_MIXED_5);
        assert_eq!(single, [KAT_WEIGHTED_SINGLE]);
        assert_eq!(uniform, KAT_WEIGHTED_UNIFORM_3);
    }

    #[test]
    fn a_single_entry_map_consumes_no_draw() {
        // The C# returns the lone key before touching the RNG (`WeightedRandomHelper.cs:26-29`),
        // so the draws either side of it are one uninterrupted stream. Parity depends on it.
        let _g = TestSeedGuard::install(KAT_SEED);
        let without = draw(&kat_uniform_map(), 3);

        let _g = TestSeedGuard::install(KAT_SEED);
        draw(&kat_single_map(), 1);
        let with = draw(&kat_uniform_map(), 3);

        assert_eq!(with, without);
    }

    #[test]
    fn negative_weights_leave_a_zeroed_slot_that_can_still_win() {
        // Bug-for-bug: every weight skipped leaves `sum_of_weights` at 0, so `random_number` is 0
        // and the first zeroed cumulative slot satisfies `>= 0`. Seed-independent.
        let map: IndexMap<String, f64> = kat_keys()
            .into_iter()
            .map(str::to_owned)
            .zip([-1.0, -2.0, -3.0, -4.0])
            .collect();

        let _g = TestSeedGuard::install(KAT_SEED);
        assert_eq!(get_weighted_value(&map).unwrap(), kat_keys()[0]);
    }
}
