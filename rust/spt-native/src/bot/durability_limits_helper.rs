//! Bot weapon/armor durability rolls, ported from `Helpers/Bot/DurabilityLimitsHelper.cs`.
//!
//! Only four methods are public on the C# helper, and all four are reachable from
//! `BotGeneratorHelper.GenerateExtraPropertiesForItem` — through
//! `GenerateWeaponRepairableProperties` (`BotGeneratorHelper.cs:215-221`) for anything with a
//! `WeapClass`, and `GenerateArmorRepairableProperties` (`:229-245`) for anything with an
//! `ArmorClass`. A mod with neither (and every mod without a `MaxDurability`) reaches none of
//! them, so it draws nothing here. `GenerateArmorRepairableProperties` also short-circuits an
//! `ArmorClass` of 0 to the template's own max durability without a draw.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! Each public entry point below draws exactly the calls listed against it, in this order:
//!
//! 1. [`get_randomized_max_weapon_durability`] → `GenerateMaxWeaponDurability`
//!    (`DurabilityLimitsHelper.cs:135`): one `randomUtil.GetInt(lowestMax, highestMax)`.
//!    Reads `Durability.{Default|Pmc|BotDurabilities[role]}.Weapon.{LowestMax,HighestMax}`.
//! 2. [`get_randomized_max_armor_durability`] → `GenerateMaxPmcArmorDurability` (`:142`): one
//!    `randomUtil.GetDouble(lowestMaxPercent, highestMaxPercent)` — **PMC roles only**. A null
//!    role, or any non-PMC role, returns the template's max durability and draws nothing.
//!    Reads `Durability.Pmc.Armor.{LowestMaxPercent,HighestMaxPercent}`.
//! 3. [`get_randomized_weapon_durability`] → `GenerateWeaponDurability` (`:193`): one
//!    `randomUtil.GetInt(minDelta, maxDelta)`. Reads
//!    `…Weapon.{MinDelta,MaxDelta,MinLimitPercent}` — the limit percent is resolved *after* the
//!    draw and consumes none.
//! 4. [`get_randomized_armor_durability`] → `GenerateArmorDurability` (`:205`): one
//!    `randomUtil.GetInt(minDelta, maxDelta)`. Reads `…Armor.{MinDelta,MaxDelta,MinLimitPercent}`,
//!    likewise resolved after the draw.
//!
//! `GetDurabilityRole` (`:78-114`) and every `Get…FromConfig` getter consume no draws.
//!
//! The C# `logger.Debug("… doesn't exist in bot config durability values, using default fallback")`
//! is dropped: these functions have no diagnostic sink and the fallback itself is ported verbatim.
use indexmap::IndexMap;
use serde::Deserialize;

use crate::loot::item_helper::LootError;
use crate::loot::random_util::{get_double, get_int, round_half_even};

/// `Models/Spt/Config/BotDurability.cs` — `BotConfig.Durability`.
#[derive(Debug, Clone, Deserialize)]
pub struct BotDurability {
    #[serde(rename = "default")]
    pub default: DefaultDurability,
    #[serde(rename = "botDurabilities")]
    pub bot_durabilities: IndexMap<String, DefaultDurability>,
    #[serde(rename = "pmc")]
    pub pmc: PmcDurability,
}

/// `Models/Spt/Config/BotDurability.cs` — also the per-role shape in `BotDurabilities`.
#[derive(Debug, Clone, Deserialize)]
pub struct DefaultDurability {
    #[serde(rename = "armor")]
    pub armor: ArmorDurability,
    #[serde(rename = "weapon")]
    pub weapon: WeaponDurability,
}

/// `Models/Spt/Config/BotDurability.cs`.
#[derive(Debug, Clone, Deserialize)]
pub struct PmcDurability {
    #[serde(rename = "armor")]
    pub armor: PmcDurabilityArmor,
    #[serde(rename = "weapon")]
    pub weapon: WeaponDurability,
}

/// `Models/Spt/Config/BotDurability.cs`. Its own type, not [`ArmorDurability`], because the
/// percent bounds are non-nullable here and nullable there.
#[derive(Debug, Clone, Deserialize)]
pub struct PmcDurabilityArmor {
    #[serde(rename = "lowestMaxPercent")]
    pub lowest_max_percent: i32,
    #[serde(rename = "highestMaxPercent")]
    pub highest_max_percent: i32,
    #[serde(rename = "maxDelta")]
    pub max_delta: i32,
    #[serde(rename = "minDelta")]
    pub min_delta: i32,
    #[serde(rename = "minLimitPercent")]
    pub min_limit_percent: i32,
}

/// `Models/Spt/Config/BotDurability.cs`. `LowestMaxPercent`/`HighestMaxPercent` are declared here
/// too but nothing reads them off this type — only the PMC slice's are used.
#[derive(Debug, Clone, Deserialize)]
pub struct ArmorDurability {
    #[serde(rename = "maxDelta")]
    pub max_delta: i32,
    #[serde(rename = "minDelta")]
    pub min_delta: i32,
    #[serde(rename = "minLimitPercent")]
    pub min_limit_percent: i32,
    #[serde(rename = "lowestMaxPercent")]
    pub lowest_max_percent: Option<i32>,
    #[serde(rename = "highestMaxPercent")]
    pub highest_max_percent: Option<i32>,
}

/// `Models/Spt/Config/BotDurability.cs`. `MinLimitPercent` is a `double` here and an `int` on
/// [`ArmorDurability`]; both are mirrored as declared.
#[derive(Debug, Clone, Deserialize)]
pub struct WeaponDurability {
    #[serde(rename = "lowestMax")]
    pub lowest_max: i32,
    #[serde(rename = "highestMax")]
    pub highest_max: i32,
    #[serde(rename = "maxDelta")]
    pub max_delta: i32,
    #[serde(rename = "minDelta")]
    pub min_delta: i32,
    #[serde(rename = "minLimitPercent")]
    pub min_limit_percent: f64,
}

/// `BotHelper._pmcTypeIds` (`Helpers/Bot/BotHelper.cs:16-22`) — the four `Sides` constants,
/// lowercased.
const PMC_TYPE_IDS: [&str; 4] = ["usec", "bear", "pmcbear", "pmcusec"];

/// `BotHelper.IsBotPmc` (`BotHelper.cs:48-51`). The C# lowercases with `ToLowerInvariant`; every
/// role id in the database is ASCII, so an ASCII fold matches it.
pub fn is_bot_pmc(bot_role: Option<&str>) -> bool {
    let role = bot_role.unwrap_or_default().to_ascii_lowercase();

    PMC_TYPE_IDS.contains(&role.as_str())
}

/// `BotHelper.IsBotBoss` (`BotHelper.cs:53-56`) — note the follower exclusion, which is why
/// `followerBigPipe` and `followerBirdEye` sit in `BotConfig.Bosses` without being bosses here.
/// The C# compares with `CurrentCultureIgnoreCase`; role ids are ASCII, so an ASCII fold matches.
pub fn is_bot_boss(bot_role: &str, bosses: &[String]) -> bool {
    !is_bot_follower(bot_role)
        && bosses
            .iter()
            .any(|boss| boss.eq_ignore_ascii_case(bot_role))
}

/// `BotHelper.IsBotFollower` (`BotHelper.cs:58-61`).
pub fn is_bot_follower(bot_role: &str) -> bool {
    bot_role.to_ascii_lowercase().starts_with("follower")
}

/// `BotHelper.IsBotZombie` (`BotHelper.cs:63-66`).
pub fn is_bot_zombie(bot_role: &str) -> bool {
    bot_role.to_ascii_lowercase().starts_with("infected")
}

/// `DurabilityLimitsHelper.GetDurabilityRole` (`:78-114`). Consumes no draws.
///
/// Borrows from `bot_role` in the pass-through case, so the returned role outlives nothing the
/// caller does not already hold.
fn get_durability_role<'a>(
    bot_role: Option<&'a str>,
    bosses: &[String],
    durability: &BotDurability,
) -> &'a str {
    let Some(bot_role) = bot_role else {
        return "default";
    };

    if is_bot_pmc(Some(bot_role)) {
        return "pmc";
    }

    if is_bot_boss(bot_role, bosses) {
        return "boss";
    }

    if is_bot_follower(bot_role) {
        return "follower";
    }

    if is_bot_zombie(bot_role) {
        return "zombie";
    }

    if durability.bot_durabilities.contains_key(bot_role) {
        return bot_role;
    }

    "default"
}

/// The weapon slice the four `Get…WeaponDurabilityFromConfig` getters resolve
/// (`DurabilityLimitsHelper.cs:147-187`, `:213-255`, `:323-343`). They are separate methods in C#
/// that each redo this lookup and each throw the same message; folding them into one lookup is
/// observationally identical because none of them draws.
///
/// The `botRole is null` arm of the C# getters is unreachable here: every caller resolves the role
/// through [`get_durability_role`] first, which never returns null.
fn weapon_config<'a>(
    durability_role: &str,
    durability: &'a BotDurability,
) -> Result<&'a WeaponDurability, LootError> {
    match durability_role {
        "default" => Ok(&durability.default.weapon),
        "pmc" => Ok(&durability.pmc.weapon),
        role => durability
            .bot_durabilities
            .get(role)
            .map(|entry| &entry.weapon)
            .ok_or_else(|| LootError::new(format!("Bot role {role} durability doesn't exist"))),
    }
}

/// The three armor values `GetMinArmorDeltaFromConfig`/`GetMaxArmorDeltaFromConfig`/
/// `GetMinArmorLimitPercentFromConfig` (`DurabilityLimitsHelper.cs:257-321`) read. Copied out
/// rather than borrowed because the PMC slice is a different type from the rest.
struct ArmorLimits {
    min_delta: i32,
    max_delta: i32,
    min_limit_percent: i32,
}

fn armor_config(
    durability_role: &str,
    durability: &BotDurability,
) -> Result<ArmorLimits, LootError> {
    let armor = match durability_role {
        "default" => &durability.default.armor,
        "pmc" => {
            let pmc = &durability.pmc.armor;

            return Ok(ArmorLimits {
                min_delta: pmc.min_delta,
                max_delta: pmc.max_delta,
                min_limit_percent: pmc.min_limit_percent,
            });
        }
        role => durability
            .bot_durabilities
            .get(role)
            .map(|entry| &entry.armor)
            .ok_or_else(|| LootError::new(format!("Bot role {role} durability doesn't exist")))?,
    };

    Ok(ArmorLimits {
        min_delta: armor.min_delta,
        max_delta: armor.max_delta,
        min_limit_percent: armor.min_limit_percent,
    })
}

/// `DurabilityLimitsHelper.GetRandomizedMaxWeaponDurability` (`:23-28`). One `GetInt` draw.
pub fn get_randomized_max_weapon_durability(
    bot_role: Option<&str>,
    bosses: &[String],
    durability: &BotDurability,
) -> Result<f64, LootError> {
    let durability_role = get_durability_role(bot_role, bosses, durability);
    let weapon = weapon_config(durability_role, durability)?;

    // 1. GenerateMaxWeaponDurability :135
    Ok(f64::from(get_int(weapon.lowest_max, weapon.highest_max)))
}

/// `DurabilityLimitsHelper.GetRandomizedMaxArmorDurability` (`:36-58`).
///
/// The C# takes the whole `TemplateItem?` and reads nothing off it but
/// `Properties?.MaxDurability`, so that nullable value is the parameter here. One `GetDouble` draw
/// for PMC roles, none otherwise.
///
/// # Errors
///
/// Where the C# throws `DurabilityHelperException`: the template has no max durability.
pub fn get_randomized_max_armor_durability(
    item_max_durability: Option<f64>,
    bot_role: Option<&str>,
    durability: &BotDurability,
) -> Result<f64, LootError> {
    let Some(item_max_durability) = item_max_durability else {
        return Err(LootError::new(
            "Item max durability amount is null when trying to get max armor durability",
        ));
    };

    if bot_role.is_none() {
        return Ok(item_max_durability);
    }

    if is_bot_pmc(bot_role) {
        // 2. GenerateMaxPmcArmorDurability :142
        let multiplier = get_double(
            f64::from(durability.pmc.armor.lowest_max_percent),
            f64::from(durability.pmc.armor.highest_max_percent),
        );

        return Ok(item_max_durability * (multiplier / 100.0));
    }

    // Everyone else (Boss/follower etc)
    Ok(item_max_durability)
}

/// `DurabilityLimitsHelper.GetRandomizedWeaponDurability` (`:66-71`). One `GetInt` draw.
pub fn get_randomized_weapon_durability(
    bot_role: Option<&str>,
    max_durability: f64,
    bosses: &[String],
    durability: &BotDurability,
) -> Result<f64, LootError> {
    let durability_role = get_durability_role(bot_role, bosses, durability);
    let weapon = weapon_config(durability_role, durability)?;

    // 3. GenerateWeaponDurability :193
    let delta = get_int(weapon.min_delta, weapon.max_delta);
    let result = max_durability - f64::from(delta);
    // `Math.Round` with no digits and no mode: half to even. Mirrors the C# association exactly —
    // `percent / 100 * max`, not `percent * (max / 100)`.
    let durability_value_min_limit =
        round_half_even(weapon.min_limit_percent / 100.0 * max_durability);

    // Don't let weapon durability go below the percent defined in config
    Ok(if result >= durability_value_min_limit {
        result
    } else {
        durability_value_min_limit
    })
}

/// `DurabilityLimitsHelper.GetRandomizedArmorDurability` (`:123-128`). One `GetInt` draw.
///
/// The C# `itemTemplate` parameter is documented "Unused" and is in fact unread, so it is not
/// ported.
pub fn get_randomized_armor_durability(
    bot_role: Option<&str>,
    max_durability: f64,
    bosses: &[String],
    durability: &BotDurability,
) -> Result<f64, LootError> {
    let durability_role = get_durability_role(bot_role, bosses, durability);
    let armor = armor_config(durability_role, durability)?;

    // 4. GenerateArmorDurability :205
    let delta = get_int(armor.min_delta, armor.max_delta);
    let result = max_durability - f64::from(delta);
    let durability_value_min_limit =
        round_half_even(f64::from(armor.min_limit_percent) / 100.0 * max_durability);

    // Don't let armor durability go below the percent defined in config
    Ok(if result >= durability_value_min_limit {
        result
    } else {
        durability_value_min_limit
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loot::random_util::TestSeedGuard;

    const SEED: u64 = 42;

    fn bosses() -> Vec<String> {
        ["bossBully", "followerBigPipe"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn weapon(
        lowest_max: i32,
        highest_max: i32,
        min_delta: i32,
        max_delta: i32,
    ) -> WeaponDurability {
        WeaponDurability {
            lowest_max,
            highest_max,
            max_delta,
            min_delta,
            min_limit_percent: 15.0,
        }
    }

    fn armor(min_delta: i32, max_delta: i32) -> ArmorDurability {
        ArmorDurability {
            max_delta,
            min_delta,
            min_limit_percent: 15,
            lowest_max_percent: None,
            highest_max_percent: None,
        }
    }

    /// The `durability` block of `SPT_Data/configs/bot.json`, trimmed to three per-role entries.
    fn durability() -> BotDurability {
        BotDurability {
            default: DefaultDurability {
                armor: armor(0, 10),
                weapon: weapon(60, 100, 0, 10),
            },
            bot_durabilities: [
                (
                    "boss".to_owned(),
                    DefaultDurability {
                        armor: armor(0, 10),
                        weapon: weapon(80, 100, 0, 10),
                    },
                ),
                (
                    "follower".to_owned(),
                    DefaultDurability {
                        armor: armor(0, 10),
                        weapon: weapon(70, 100, 0, 10),
                    },
                ),
                (
                    "assault".to_owned(),
                    DefaultDurability {
                        armor: armor(0, 10),
                        weapon: weapon(50, 100, 0, 10),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            pmc: PmcDurability {
                armor: PmcDurabilityArmor {
                    lowest_max_percent: 90,
                    highest_max_percent: 100,
                    max_delta: 10,
                    min_delta: 0,
                    min_limit_percent: 15,
                },
                weapon: weapon(95, 100, 0, 5),
            },
        }
    }

    #[test]
    fn durability_role_maps_every_c_sharp_branch() {
        let durability = durability();
        let bosses = bosses();

        assert_eq!(get_durability_role(None, &bosses, &durability), "default");
        assert_eq!(
            get_durability_role(Some("pmcUSEC"), &bosses, &durability),
            "pmc"
        );
        assert_eq!(
            get_durability_role(Some("bossBully"), &bosses, &durability),
            "boss"
        );
        // In `BotConfig.Bosses` but a follower, so the boss branch is skipped.
        assert_eq!(
            get_durability_role(Some("followerBigPipe"), &bosses, &durability),
            "follower"
        );
        assert_eq!(
            get_durability_role(Some("infectedAssault"), &bosses, &durability),
            "zombie"
        );
        assert_eq!(
            get_durability_role(Some("assault"), &bosses, &durability),
            "assault"
        );
        assert_eq!(
            get_durability_role(Some("madeUpRole"), &bosses, &durability),
            "default"
        );
    }

    #[test]
    fn max_weapon_durability_draws_once_per_call() {
        let durability = durability();
        let bosses = bosses();
        let _guard = TestSeedGuard::install(SEED);

        let drawn: Vec<f64> = (0..3)
            .map(|_| get_randomized_max_weapon_durability(None, &bosses, &durability).unwrap())
            .collect();

        assert_eq!(drawn, vec![82.0, 93.0, 93.0]);
    }

    #[test]
    fn max_weapon_durability_uses_the_pmc_slice_for_pmc_roles() {
        let durability = durability();
        let bosses = bosses();
        let _guard = TestSeedGuard::install(SEED);

        let drawn: Vec<f64> = (0..3)
            .map(|_| {
                get_randomized_max_weapon_durability(Some("pmcBEAR"), &bosses, &durability).unwrap()
            })
            .collect();

        assert_eq!(drawn, vec![96.0, 96.0, 99.0]);
    }

    #[test]
    fn an_unmapped_role_is_a_config_error() {
        let mut durability = durability();
        durability.bot_durabilities.shift_remove("boss");
        let bosses = bosses();

        let error = get_randomized_max_weapon_durability(Some("bossBully"), &bosses, &durability)
            .unwrap_err();

        assert_eq!(error.message, "Bot role boss durability doesn't exist");
    }

    #[test]
    fn weapon_durability_draws_once_per_call() {
        let durability = durability();
        let bosses = bosses();
        let _guard = TestSeedGuard::install(SEED);

        let drawn: Vec<f64> = (0..3)
            .map(|_| get_randomized_weapon_durability(None, 100.0, &bosses, &durability).unwrap())
            .collect();

        assert_eq!(drawn, vec![94.0, 99.0, 99.0]);
    }

    #[test]
    fn weapon_durability_is_clamped_to_the_min_limit_percent() {
        let mut durability = durability();
        // Force the delta past the 15% floor of a max durability of 20.
        durability.default.weapon.min_delta = 19;
        durability.default.weapon.max_delta = 19;
        let bosses = bosses();
        let _guard = TestSeedGuard::install(SEED);

        // 20 - 19 = 1, below round(15 / 100 * 20) = 3.
        let clamped = get_randomized_weapon_durability(None, 20.0, &bosses, &durability).unwrap();

        assert_eq!(clamped, 3.0);
    }

    #[test]
    fn max_armor_durability_draws_only_for_pmc_roles() {
        let durability = durability();
        let _guard = TestSeedGuard::install(SEED);

        let drawn: Vec<f64> = (0..3)
            .map(|_| {
                get_randomized_max_armor_durability(Some(50.0), Some("Usec"), &durability).unwrap()
            })
            .collect();

        assert_eq!(
            drawn,
            vec![45.21835690221945, 49.248537143259874, 46.62492570071494]
        );
    }

    #[test]
    fn max_armor_durability_passes_the_template_value_through_without_a_draw() {
        let durability = durability();
        let _guard = TestSeedGuard::install(SEED);

        // Neither call may draw, so the weapon roll that follows must see a pristine stream.
        assert_eq!(
            get_randomized_max_armor_durability(Some(50.0), None, &durability).unwrap(),
            50.0
        );
        assert_eq!(
            get_randomized_max_armor_durability(Some(50.0), Some("assault"), &durability).unwrap(),
            50.0
        );
        assert_eq!(
            get_randomized_max_weapon_durability(None, &bosses(), &durability).unwrap(),
            82.0
        );
    }

    #[test]
    fn max_armor_durability_without_a_template_value_is_an_error() {
        let durability = durability();

        let error =
            get_randomized_max_armor_durability(None, Some("Usec"), &durability).unwrap_err();

        assert_eq!(
            error.message,
            "Item max durability amount is null when trying to get max armor durability"
        );
    }

    #[test]
    fn armor_durability_draws_once_per_call() {
        let durability = durability();
        let bosses = bosses();
        let _guard = TestSeedGuard::install(SEED);

        let drawn: Vec<f64> = (0..3)
            .map(|_| get_randomized_armor_durability(None, 50.0, &bosses, &durability).unwrap())
            .collect();

        assert_eq!(drawn, vec![44.0, 49.0, 49.0]);
    }

    #[test]
    fn armor_durability_uses_the_pmc_slice_for_pmc_roles() {
        let durability = durability();
        let bosses = bosses();
        let _guard = TestSeedGuard::install(SEED);

        let drawn: Vec<f64> = (0..3)
            .map(|_| {
                get_randomized_armor_durability(Some("pmcUSEC"), 50.0, &bosses, &durability)
                    .unwrap()
            })
            .collect();

        assert_eq!(drawn, vec![44.0, 49.0, 49.0]);
    }
}
