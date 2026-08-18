//! `Generators/Bot/BotLevelGenerator.cs` — the level/exp draw, run natively on the batch path.
//!
//! Only `GenerateBotLevel` + `ChooseBotLevel` (`BotLevelGenerator.cs:21-58`) move.
//! `GetRelativePmcBotLevelRange` (`:67-101`) stays C#-side as hoisted wave state
//! (`levelGeneration.levelMin/levelMax` on the shared views), and the non-PMC early return
//! (`:23-26`) lives at the batch call site so this function never runs without a range.

use crate::loot::random_util::{get_biased_random_number, get_int};

/// `BotLevelGenerator.GenerateBotLevel` (`BotLevelGenerator.cs:21-45`) for one PMC bot.
///
/// Returns `(level, exp)`. Quirks carried over as-built:
/// - `ChooseBotLevel(min, max, 1, 1.15)` (`:32`, `:55-58`) — the C# `(int)` cast truncates toward
///   zero, and an inverted range makes `GetBiasedRandomNumber` return `-1`, which the clamp turns
///   into 0.
/// - `expTable.Take(level).Sum(...)` (`:39`) — i32 arithmetic, like the C#.
/// - The fractional draw is skipped at and above `maxLevelIndex` (`:42`), and
///   `GetInt(0, exp - 1)` returns 0 without drawing when `exp <= 1` (`RandomUtil.cs:48`).
pub fn generate_bot_level(level_min: i32, level_max: i32, exp_table: &[i32]) -> (i32, i32) {
    let level =
        get_biased_random_number(f64::from(level_min), f64::from(level_max), 1.0, 1.15) as i32;

    let max_level_index = exp_table.len() as i32 - 1;
    let level = level.clamp(0, max_level_index + 1);

    let base_exp: i32 = exp_table[..level as usize].iter().sum();

    let fractional_exp = if level < max_level_index {
        get_int(0, exp_table[level as usize] - 1)
    } else {
        0
    };

    (level, base_exp + fractional_exp)
}

#[cfg(test)]
mod tests {
    use super::generate_bot_level;
    use crate::loot::random_util::TestSeedGuard;

    // min == max short-circuits the level draw in get_biased_random_number (RandomUtil.cs:388-391),
    // so the only draw left is the fractional exp: level 2 < max_level_index 3 draws
    // get_int(0, exp_table[2] - 1) = [0, 29].
    #[test]
    fn a_degenerate_range_pins_the_level_and_bounds_the_fractional_exp() {
        let _guard = TestSeedGuard::install(7);
        let exp_table = [10, 20, 30, 40];
        let (level, exp) = generate_bot_level(2, 2, &exp_table);
        assert_eq!(level, 2);
        let base = 10 + 20;
        assert!(exp >= base && exp <= base + 29, "exp {exp}");
    }

    // Level pinned at the table ceiling: level == len == max_level_index + 1, so Take(level) sums
    // the whole table and the fractional draw is skipped (BotLevelGenerator.cs:42) — fully
    // deterministic, zero RNG consumed.
    #[test]
    fn at_the_table_ceiling_the_whole_table_sums_and_no_fractional_draws() {
        let exp_table = [10, 20, 30];
        let (level, exp) = generate_bot_level(3, 3, &exp_table);
        assert_eq!((level, exp), (3, 60));
    }

    // At exactly max_level_index the fractional is also skipped (`level < maxLevelIndex` is false).
    // Seeded because the assertion is about a draw *not* happening: under seed 7 the draw a `<=`
    // here would make — get_int(0, exp_table[2] - 1) — is non-zero, so the mutation reads 30 + 26.
    #[test]
    fn at_max_level_index_no_fractional_draws() {
        let _guard = TestSeedGuard::install(7);
        let exp_table = [10, 20, 30];
        let (level, exp) = generate_bot_level(2, 2, &exp_table);
        assert_eq!((level, exp), (2, 30));
    }

    // max < min: GetBiasedRandomNumber returns -1 (RandomUtil.cs:376-380), (int) cast keeps -1,
    // Math.Clamp lands on 0, base is 0, and the fractional draws from expTable[0].
    #[test]
    fn an_inverted_range_lands_on_level_zero() {
        let _guard = TestSeedGuard::install(1);
        let exp_table = [100, 200];
        let (level, exp) = generate_bot_level(10, 5, &exp_table);
        assert_eq!(level, 0);
        assert!((0..=99).contains(&exp), "exp {exp}");
    }

    // GetInt(0, exp - 1) with exp <= 1 returns min without drawing (RandomUtil.cs:48 `max > min`).
    #[test]
    fn a_one_exp_level_draws_zero_fractional() {
        let exp_table = [1, 1, 1];
        let (level, exp) = generate_bot_level(1, 1, &exp_table);
        assert_eq!((level, exp), (1, 1));
    }

    // Same seed, same output; different seed may differ — the draw is on the seeded stream.
    #[test]
    fn the_draw_is_seed_stable() {
        let first = {
            let _guard = TestSeedGuard::install(42);
            generate_bot_level(5, 30, &[10; 79])
        };
        let second = {
            let _guard = TestSeedGuard::install(42);
            generate_bot_level(5, 30, &[10; 79])
        };
        assert_eq!(first, second);
        assert!(first.0 >= 5 && first.0 <= 30);
    }
}
