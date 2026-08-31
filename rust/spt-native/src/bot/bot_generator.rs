//! `Generators/Bot/BotGenerator.cs` — the prelude/finish draws the batch path runs natively
//! (spec docs/superpowers/specs/2026-08-29-botgen-finish-design.md). Batch arm only: the
//! per-bot and player-scav arms keep the C# prelude, so these bodies exist twice by design.

use indexmap::IndexMap;

use crate::bot::BotViews;
use crate::bot::bot_inventory_generator::UNHEARD;
use crate::bot::models::{
    AppearanceWire, BodyPartTemplateWire, BotDbSkillsWire, BotGenerationDetailsWire,
    BotHealthResult, BotSkillsResult, BotTypeHealthWire, CurrentMaxWire, PmcConfigWire,
    SkillResult, TemplateVariantWire,
};
use crate::bot::repair_service::MinMax;
use crate::loot::item_helper::LootError;
use crate::loot::models::{Item, Upd};
use crate::loot::random_util::{
    get_array_value, get_double, get_int, get_weighted_value, round_half_even,
};

/// `Models/Enums/GameEditions.cs`. Its `UNHEARD` twin already lives in
/// [`crate::bot::bot_inventory_generator`], so it is imported rather than declared twice.
pub(crate) const EDGE_OF_DARKNESS: &str = "edge_of_darkness";

// `Models/Enums/MemberCategory.cs` numeric values, pinned C#-side by
// `SptNativeBotWireTests.MemberCategoryNumericValuesMatchTheNativeConstants`.
const MEMBER_CATEGORY_DEVELOPER: i32 = 1;
const MEMBER_CATEGORY_UNIQUE_ID: i32 = 2;
const MEMBER_CATEGORY_UNHEARD: i32 = 1024;

/// Everything the prelude draws for one bot, in `GenerateBotPrelude`'s statement order.
pub(crate) struct BotExtras {
    pub settings_experience: i32,
    pub voice: String,
    pub health: BotHealthResult,
    pub skills: BotSkillsResult,
    /// `None` for non-PMC bots — `SetRandomisedGameVersionAndCategory` is PMC-branch only.
    pub game_version: Option<GameVersionDraw>,
    pub appearance: AppearanceDraw,
}

pub(crate) struct GameVersionDraw {
    pub game_version: String,
    pub member_category: i32,
    /// `None` on the nikita branch — the C# quirk leaves `SelectedMemberCategory` untouched.
    pub selected_member_category: Option<i32>,
}

pub(crate) struct AppearanceDraw {
    pub head: String,
    pub feet: String,
    pub body: String,
    pub hands: String,
}

/// The prelude's moved draws in statement order: exp-reward, voice, health, skills,
/// game-version (PMC), appearance. The caller has already drawn the level and picked the
/// variant; the inventory and dogtag draws follow this call.
pub(crate) fn draw_prelude_extras(
    details: &BotGenerationDetailsWire,
    variant: &TemplateVariantWire,
    views: &BotViews,
    is_nikita: bool,
) -> Result<BotExtras, LootError> {
    let settings_experience =
        get_experience_reward_for_kill(&variant.experience_reward, &details.bot_difficulty)?;
    let voice = get_weighted_value(&variant.appearance.voice)?;
    let health = generate_health(&variant.health, details.is_player_scav)?;
    let skills = generate_skills(&variant.skills);
    let game_version = if details.is_pmc {
        Some(set_randomised_game_version_and_category(
            views.pmc_config(),
            is_nikita,
        )?)
    } else {
        None
    };
    // Two C#-vs-Rust edges live behind this call, both only reachable on malformed DB data and
    // both booked as documented divergences rather than defects: C# indexes
    // `templateTable.Customization[bodyTpl]` and throws on a body tpl the customization table does
    // not hold, where the `body_to_fixed_hands` map-miss falls through to the weighted hands draw;
    // and C#'s `chosenBodyTemplate?.Name.Trim()` NREs on a customization entry with a null `_name`,
    // where `bot::views::derive` skips that entry when building the map.
    let appearance = set_bot_appearance(&variant.appearance, views.body_to_fixed_hands())?;

    Ok(BotExtras {
        settings_experience,
        voice,
        health,
        skills,
        game_version,
        appearance,
    })
}

/// `BotGenerator.GetExperienceRewardForKillByDifficulty`. The missing-difficulty fallback to
/// `normal` DOES draw (and does NOT re-check `-1`). Draw-free cases: `max == -1` (explicit
/// shortcut, returns the literal) and `min == max` (`get_int`'s `max > min` guard matches C#
/// `GetInt` — parity is automatic; `pmcusec` ships `250/250`). The C# Debug log line on the
/// fallback is not ported (log-only divergence, noted in RUST-LEDGER.md).
fn get_experience_reward_for_kill(
    rewards: &IndexMap<String, MinMax<i32>>,
    bot_difficulty: &str,
) -> Result<i32, LootError> {
    let Some(band) = rewards.get(&bot_difficulty.to_lowercase()) else {
        let band = rewards
            .get("normal")
            .ok_or_else(|| LootError::new("no `normal` experience reward band"))?;
        return Ok(get_int(band.min, band.max));
    };
    if band.max == -1 {
        // Quirk: the -1/-1 shortcut returns the literal without drawing.
        return Ok(-1);
    }
    Ok(get_int(band.min, band.max))
}

/// `BotGenerator.GenerateHealth`. Draw order: body-part band selection (skipped for pscav),
/// hydration, energy, temperature, then Head, Chest, Stomach, LeftArm, RightArm, LeftLeg,
/// RightLeg. Part maxima round half-to-even (`Math.Round(double)`).
fn generate_health(
    health: &BotTypeHealthWire,
    player_scav: bool,
) -> Result<BotHealthResult, LootError> {
    if health.body_parts.is_empty() {
        // C#: GetArrayValue throws / GetLowestHpBodyPart returns null then NREs. Error the bot.
        return Err(LootError::new("bot health template has no body parts"));
    }
    let parts = if player_scav {
        get_lowest_hp_body_part(&health.body_parts)
    } else {
        get_array_value(&health.body_parts)
    };

    let level = |band: &MinMax<f64>| CurrentMaxWire {
        current: get_double(band.min, band.max),
        maximum: band.max,
    };
    let part = |band: &MinMax<f64>| CurrentMaxWire {
        current: get_double(band.min, band.max),
        maximum: round_half_even(band.max),
    };

    let hydration = level(&health.hydration);
    let energy = level(&health.energy);
    let temperature = level(&health.temperature);
    let body_parts = IndexMap::from([
        ("Head".to_owned(), part(&parts.head)),
        ("Chest".to_owned(), part(&parts.chest)),
        ("Stomach".to_owned(), part(&parts.stomach)),
        ("LeftArm".to_owned(), part(&parts.left_arm)),
        ("RightArm".to_owned(), part(&parts.right_arm)),
        ("LeftLeg".to_owned(), part(&parts.left_leg)),
        ("RightLeg".to_owned(), part(&parts.right_leg)),
    ]);

    Ok(BotHealthResult {
        hydration,
        energy,
        temperature,
        body_parts,
    })
}

/// `BotGenerator.GetLowestHpBodyPart`. Quirk: Stomach is excluded from the total. C#'s OrderBy
/// is a stable sort, so ties keep the first band in source order — the strict `<` fold matches.
fn get_lowest_hp_body_part(parts: &[BodyPartTemplateWire]) -> &BodyPartTemplateWire {
    let total = |part: &BodyPartTemplateWire| {
        part.head.max
            + part.chest.max
            + part.left_arm.max
            + part.right_arm.max
            + part.left_leg.max
            + part.right_leg.max
    };
    parts
        .iter()
        .fold(None::<(&BodyPartTemplateWire, f64)>, |best, part| {
            let part_total = total(part);
            match best {
                Some((_, best_total)) if best_total <= part_total => best,
                _ => Some((part, part_total)),
            }
        })
        .map(|(part, _)| part)
        .expect("caller checked non-empty")
}

/// `BotGenerator.GenerateSkills` + both randomisers. Null-valued entries skip without a draw;
/// ids pass through as raw key strings (C# parses `SkillTypes` on hydration).
fn generate_skills(skills: &BotDbSkillsWire) -> BotSkillsResult {
    fn randomise(map: &IndexMap<String, Option<MinMax<f64>>>) -> Vec<SkillResult> {
        map.iter()
            .filter_map(|(key, band)| {
                let band = band.as_ref()?;
                Some(SkillResult {
                    id: key.clone(),
                    progress: get_double(band.min, band.max),
                })
            })
            .collect()
    }
    BotSkillsResult {
        common: randomise(&skills.common),
        mastering: skills.mastering.as_ref().map(randomise).unwrap_or_default(),
    }
}

/// `BotGenerator.SetRandomisedGameVersionAndCategory`. The nikita branch draws nothing and —
/// quirk — leaves `SelectedMemberCategory` unset. EDGE_OF_DARKNESS/UNHEARD skip the account-type
/// draw.
fn set_randomised_game_version_and_category(
    pmc_config: &PmcConfigWire,
    is_nikita: bool,
) -> Result<GameVersionDraw, LootError> {
    if is_nikita {
        return Ok(GameVersionDraw {
            game_version: UNHEARD.to_owned(),
            member_category: MEMBER_CATEGORY_DEVELOPER,
            selected_member_category: None,
        });
    }
    let game_version = get_weighted_value(&pmc_config.game_version_weight)?;
    let member_category = match game_version.as_str() {
        EDGE_OF_DARKNESS => MEMBER_CATEGORY_UNIQUE_ID,
        UNHEARD => MEMBER_CATEGORY_UNHEARD,
        _ => get_weighted_value(&pmc_config.account_type_weight)?
            .parse()
            .map_err(|_| LootError::new("accountTypeWeight key is not numeric"))?,
    };
    Ok(GameVersionDraw {
        game_version,
        member_category,
        selected_member_category: Some(member_category),
    })
}

/// `BotGenerator.SetBotAppearance`. Draw order is the C# statement order: Head, Feet, Body,
/// then Hands — fixed hands (IsNotRandom body) skip the hands draw. Known divergence
/// (documented in RUST-ROADMAP.md): a drawn body tpl absent from `templates.customization`
/// throws in C# and falls through to a hands draw here.
fn set_bot_appearance(
    appearance: &AppearanceWire,
    body_to_fixed_hands: &IndexMap<String, String>,
) -> Result<AppearanceDraw, LootError> {
    let head = get_weighted_value(&appearance.head)?;
    let feet = get_weighted_value(&appearance.feet)?;
    let body = get_weighted_value(&appearance.body)?;
    let hands = match body_to_fixed_hands.get(&body) {
        Some(fixed) => fixed.clone(),
        None => get_weighted_value(&appearance.hands)?,
    };
    Ok(AppearanceDraw {
        head,
        feet,
        body,
        hands,
    })
}

/// `BotGenerator.AddDogtagToBot` + `GetDogtagTplByGameVersionAndSide`. Appends to the built
/// inventory; the C# `GenerateInventoryId` rewrite covers the appended item like any other. An
/// unknown side or a missing default band NREs in C# — error the bot instead (unreachable
/// shipped).
pub(crate) fn add_dogtag_to_bot(
    items: &mut Vec<Item>,
    equipment_id: &str,
    side: &str,
    game_version: &str,
    dogtag_settings: &IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
) -> Result<(), LootError> {
    let side_weights = dogtag_settings
        .get(&side.to_lowercase())
        .ok_or_else(|| LootError::new("no dogtag settings for side"))?;
    let possible = side_weights
        .get(game_version)
        .or_else(|| side_weights.get("default"))
        .ok_or_else(|| LootError::new("no dogtag settings for game version or default"))?;
    let template = get_weighted_value(possible)?;

    items.push(Item {
        id: crate::loot::mongo_id::generate(),
        template,
        parent_id: Some(equipment_id.to_owned()),
        slot_id: Some("Dogtag".to_owned()),
        location: None,
        desc: None,
        // `Upd` types no `SpawnedInSession` member — it rides the passthrough map, the crate's
        // established pattern (`loot_generator.rs`, `reward_generator.rs`).
        upd: Some(Upd {
            extra: [("SpawnedInSession".to_owned(), serde_json::Value::Bool(true))]
                .into_iter()
                .collect(),
            ..Default::default()
        }),
        extra: Default::default(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::loot::random_util::TestSeedGuard;

    const SEED: u64 = 0xB07_6E4;

    fn health(value: serde_json::Value) -> BotTypeHealthWire {
        serde_json::from_value(value).expect("health fixture parses")
    }

    /// One `BodyParts` band. `head` and `stomach` are theirs alone and `others` is the max the
    /// remaining five share, which is what lets the callers move the with/without-Stomach totals
    /// and tell bands apart.
    ///
    /// Every part gets its own `min`, one further below its max than the last, so the seven
    /// `get_double(min, max)` calls are seven *different* affine maps of the stream. A shared
    /// `min == max` would collapse them all to the constant `max` and leave the draw order
    /// completely unpinned — the seven draws would be interchangeable.
    fn band(others: f64, head: f64, stomach: f64) -> serde_json::Value {
        let part = |max: f64, span: f64| json!({"min": max - span, "max": max});
        json!({
            "Head": part(head, 1.0), "Chest": part(others, 2.0), "Stomach": part(stomach, 3.0),
            "LeftArm": part(others, 4.0), "RightArm": part(others, 5.0),
            "LeftLeg": part(others, 6.0), "RightLeg": part(others, 7.0),
        })
    }

    /// The three level bands, each drawn with one `get_double`. Distinct ranges so a reordering of
    /// the three would be visible in the pinned values.
    fn levels() -> serde_json::Value {
        json!({
            "Hydration": {"min": 10.0, "max": 35.5},
            "Energy": {"min": 100.0, "max": 200.0},
            "Temperature": {"min": 36.0, "max": 40.0},
        })
    }

    // -- 1. `generate_health`: reproducible at a seed, level maxima raw, part maxima ties-to-even.

    #[test]
    fn health_is_reproducible_and_rounds_part_maxima_half_to_even() {
        // Head 35.5 and Stomach 36.5 are the two `Math.Round` ties: half-up would give 36.0/37.0,
        // banker's rounding gives 36.0/36.0.
        let fixture = health(json!({
            "BodyParts": [band(20.0, 35.5, 36.5)],
            "Hydration": levels()["Hydration"], "Energy": levels()["Energy"],
            "Temperature": levels()["Temperature"],
        }));

        let first = {
            let _guard = TestSeedGuard::install(SEED);
            generate_health(&fixture, false).expect("the fixture has a body part band")
        };

        // The draw order, spelled out: the band selection, then Hydration, Energy, Temperature,
        // then Head, Chest, Stomach, LeftArm, RightArm, LeftLeg, RightLeg — the C# statement and
        // initializer order. Ten `get_double`s off one stream have to reproduce the call's ten
        // `current` values in exactly that sequence. (`band` gives every part its own range, so
        // each position maps the stream differently and a permuted port lands elsewhere; the
        // single-band selection short-circuits through `get_int(0, 0)` without drawing, which the
        // pscav test below is what pins.)
        let control = {
            let _guard = TestSeedGuard::install(SEED);
            let parts = get_array_value(&fixture.body_parts);
            [
                &fixture.hydration,
                &fixture.energy,
                &fixture.temperature,
                &parts.head,
                &parts.chest,
                &parts.stomach,
                &parts.left_arm,
                &parts.right_arm,
                &parts.left_leg,
                &parts.right_leg,
            ]
            .map(|band| get_double(band.min, band.max))
        };
        let drawn = [
            first.hydration.current,
            first.energy.current,
            first.temperature.current,
            first.body_parts["Head"].current,
            first.body_parts["Chest"].current,
            first.body_parts["Stomach"].current,
            first.body_parts["LeftArm"].current,
            first.body_parts["RightArm"].current,
            first.body_parts["LeftLeg"].current,
            first.body_parts["RightLeg"].current,
        ];
        assert_eq!(drawn, control);
        // The ten are genuinely distinct, so no two positions could be swapped unnoticed.
        for (index, value) in drawn.iter().enumerate() {
            assert!(
                !drawn[..index].contains(value),
                "two draws collided at {index}: {drawn:?}"
            );
        }

        // Level maxima are the raw band max — no rounding at all, so 35.5 survives.
        assert_eq!(first.hydration.maximum, 35.5);
        assert_eq!(first.energy.maximum, 200.0);
        assert_eq!(first.temperature.maximum, 40.0);
        // Part maxima round half to even: 35.5 -> 36.0 and 36.5 -> 36.0 (a half-up port gives 37).
        assert_eq!(first.body_parts["Head"].maximum, 36.0);
        assert_eq!(first.body_parts["Stomach"].maximum, 36.0);
        assert_eq!(first.body_parts["Chest"].maximum, 20.0);

        // Every current sits inside its band, and the seven parts are in C# initializer order.
        assert!((10.0..35.5).contains(&first.hydration.current));
        assert!((100.0..200.0).contains(&first.energy.current));
        assert!((36.0..40.0).contains(&first.temperature.current));
        assert_eq!(
            first.body_parts.keys().collect::<Vec<_>>(),
            [
                "Head", "Chest", "Stomach", "LeftArm", "RightArm", "LeftLeg", "RightLeg"
            ]
        );

        // An empty band list errors rather than panicking out of `get_array_value`.
        let empty = health(json!({
            "BodyParts": [], "Hydration": levels()["Hydration"], "Energy": levels()["Energy"],
            "Temperature": levels()["Temperature"],
        }));
        assert!(generate_health(&empty, false).is_err());
        assert!(generate_health(&empty, true).is_err());
    }

    // -- 2. The player-scav branch: lowest total EXCLUDING Stomach, and no selection draw.

    #[test]
    fn the_pscav_branch_excludes_stomach_from_the_total_and_draws_no_selection() {
        // Three bands, identified by their Head max. Totals, six parts (Stomach excluded):
        //   band 0: 10*5 + 11    = 61  (with stomach: 62)
        //   band 1:  8*5 + 12    = 52  (with stomach: 152 — the biggest of the three)
        //   band 2: 12*5 + 13    = 73  (with stomach: 73)
        // Lowest excluding Stomach is band 1; lowest *including* it is band 0. Picking band 1 is
        // the quirk; a port that summed Stomach too would pick band 0.
        let fixture = health(json!({
            "BodyParts": [band(10.0, 11.0, 1.0), band(8.0, 12.0, 100.0), band(12.0, 13.0, 0.0)],
            "Hydration": levels()["Hydration"], "Energy": levels()["Energy"],
            "Temperature": levels()["Temperature"],
        }));

        let pscav = {
            let _guard = TestSeedGuard::install(SEED);
            generate_health(&fixture, true).expect("the fixture has body part bands")
        };
        assert_eq!(
            pscav.body_parts["Head"].maximum, 12.0,
            "the lowest total excluding Stomach is band 1"
        );

        // Draw-free: the band selection is the first thing `generate_health` would draw, so the
        // hydration `current` has to equal the *very first* `get_double` off a fresh stream at the
        // same seed. A selection draw would have shifted it.
        let control = {
            let _guard = TestSeedGuard::install(SEED);
            get_double(10.0, 35.5)
        };
        assert_eq!(pscav.hydration.current, control);

        // And the fixture really is multi-band: the non-pscav sibling *does* spend a `get_int` on
        // `get_array_value`, so its hydration lands elsewhere in the stream. (With a single band
        // `get_int(0, 0)` short-circuits and this assert would fail — which is why the quirk needs
        // three bands to be pinned at all.)
        let rolled = {
            let _guard = TestSeedGuard::install(SEED);
            generate_health(&fixture, false).expect("the fixture has body part bands")
        };
        assert_ne!(rolled.hydration.current, control);
    }

    /// Ties keep the first band in source order, matching C#'s stable `OrderBy`.
    #[test]
    fn a_tied_lowest_total_keeps_the_first_band() {
        let parts: Vec<BodyPartTemplateWire> =
            serde_json::from_value(json!([band(10.0, 5.0, 0.0), band(10.0, 5.0, 99.0)]))
                .expect("band fixtures parse");

        // Identical six-part totals: the first band wins, so its Stomach comes through.
        assert_eq!(get_lowest_hp_body_part(&parts).stomach.max, 0.0);
    }

    // -- 3. `generate_skills`: a null-valued entry is skipped and consumes no draw.

    #[test]
    fn a_null_skill_entry_is_skipped_without_consuming_a_draw() {
        let skills = |value: serde_json::Value| -> BotDbSkillsWire {
            serde_json::from_value(value).expect("skills fixture parses")
        };
        let with_null = skills(json!({
            "Common": {"BotReload": {"min": 1.0, "max": 2.0}, "Sniper": null,
                       "Endurance": {"min": 10.0, "max": 20.0}},
            "Mastering": {"Assault": null, "Pistol": {"min": 3.0, "max": 4.0}},
        }));
        let without_null = skills(json!({
            "Common": {"BotReload": {"min": 1.0, "max": 2.0},
                       "Endurance": {"min": 10.0, "max": 20.0}},
            "Mastering": {"Pistol": {"min": 3.0, "max": 4.0}},
        }));

        let draw = |fixture: &BotDbSkillsWire| {
            let _guard = TestSeedGuard::install(SEED);
            generate_skills(fixture)
        };
        let drawn = draw(&with_null);
        let control = draw(&without_null);

        // Skipped, not emitted as a zero — and the ids are the raw template keys.
        assert_eq!(
            drawn.common.iter().map(|s| &s.id).collect::<Vec<_>>(),
            ["BotReload", "Endurance"]
        );
        assert_eq!(
            drawn.mastering.iter().map(|s| &s.id).collect::<Vec<_>>(),
            ["Pistol"]
        );
        // Draw-free: deleting the null entries outright replays exactly the same stream, so every
        // progress matches. A `filter_map` that drew before discarding would shift `Endurance`.
        assert_eq!(
            serde_json::to_value(&drawn).expect("skills serialize"),
            serde_json::to_value(&control).expect("skills serialize"),
        );

        // An absent `Mastering` is an empty list, not a failure.
        let no_mastering = skills(json!({"Common": {}}));
        let _guard = TestSeedGuard::install(SEED);
        assert!(generate_skills(&no_mastering).mastering.is_empty());
    }

    // -- 4. `get_experience_reward_for_kill`: two draw-free shortcuts, one drawing fallback.

    #[test]
    fn the_experience_reward_shortcuts_consume_no_draw_and_the_fallback_does() {
        let rewards = |value: serde_json::Value| -> IndexMap<String, MinMax<i32>> {
            serde_json::from_value(value).expect("experience fixture parses")
        };
        // What a fresh stream yields first, for the draw-free asserts to compare against.
        let untouched = {
            let _guard = TestSeedGuard::install(SEED);
            get_double(0.0, 1.0)
        };

        // `max == -1`: the literal, no draw. Shipped as `-1/-1` on several roles.
        let minus_one = rewards(json!({"easy": {"min": -1, "max": -1}}));
        let (value, next) = {
            let _guard = TestSeedGuard::install(SEED);
            (
                get_experience_reward_for_kill(&minus_one, "easy").expect("the band is present"),
                get_double(0.0, 1.0),
            )
        };
        assert_eq!(value, -1);
        assert_eq!(next, untouched, "the -1 shortcut must not draw");

        // `min == max`: `get_int`'s `max > min` guard returns `min` without drawing, matching C#
        // `GetInt`. `pmcusec` ships 250/250.
        let flat = rewards(json!({"normal": {"min": 250, "max": 250}}));
        let (value, next) = {
            let _guard = TestSeedGuard::install(SEED);
            (
                get_experience_reward_for_kill(&flat, "normal").expect("the band is present"),
                get_double(0.0, 1.0),
            )
        };
        assert_eq!(value, 250);
        assert_eq!(next, untouched, "an empty range must not draw");

        // The difficulty key is lowercased before the lookup (C# `ToLowerInvariant`).
        let _guard = TestSeedGuard::install(SEED);
        assert_eq!(
            get_experience_reward_for_kill(&flat, "NORMAL").expect("the band is present"),
            250
        );
        drop(_guard);

        // A missing difficulty falls back to `normal` AND draws — and does not re-check `-1`, so a
        // `-1/-1` normal band would come back as a real draw over `[-1, -1]`... which is `-1`
        // anyway; the observable half is that a real band draws.
        let fallback = rewards(json!({
            "easy": {"min": 1, "max": 2}, "normal": {"min": 1000, "max": 2000},
        }));
        let drawn = {
            let _guard = TestSeedGuard::install(SEED);
            get_experience_reward_for_kill(&fallback, "impossible").expect("normal is present")
        };
        let control = {
            let _guard = TestSeedGuard::install(SEED);
            get_int(1000, 2000)
        };
        assert_eq!(drawn, control, "the fallback draws off the `normal` band");

        // No `normal` to fall back to is an error, not a panic.
        let no_normal = rewards(json!({"easy": {"min": 1, "max": 2}}));
        assert!(get_experience_reward_for_kill(&no_normal, "impossible").is_err());
    }

    // -- 5. `set_randomised_game_version_and_category`.

    fn pmc_config(value: serde_json::Value) -> PmcConfigWire {
        serde_json::from_value(value).expect("pmc config fixture parses")
    }

    #[test]
    fn the_nikita_and_fixed_category_game_versions_draw_nothing() {
        let untouched = {
            let _guard = TestSeedGuard::install(SEED);
            get_double(0.0, 1.0)
        };
        // Two-entry maps: either would draw if it were reached.
        let config = pmc_config(json!({
            "gameVersionWeight": {"standard": 1.0, "unheard_edition": 3.0},
            "accountTypeWeight": {"0": 1.0, "256": 3.0},
        }));

        // nikita: UNHEARD / Developer, `SelectedMemberCategory` left unset — the C# quirk — and
        // not a single draw, even though both weight maps above would spend one.
        let (draw, next) = {
            let _guard = TestSeedGuard::install(SEED);
            (
                set_randomised_game_version_and_category(&config, true).expect("nikita draws"),
                get_double(0.0, 1.0),
            )
        };
        assert_eq!(draw.game_version, UNHEARD);
        assert_eq!(draw.member_category, MEMBER_CATEGORY_DEVELOPER);
        assert_eq!(draw.selected_member_category, None);
        assert_eq!(next, untouched, "the nikita branch must not draw");

        // A single-entry `gameVersionWeight` takes `get_weighted_value`'s no-draw shortcut, and
        // both fixed editions skip the account-type draw: the whole call spends nothing.
        for (edition, category) in [
            (EDGE_OF_DARKNESS, MEMBER_CATEGORY_UNIQUE_ID),
            (UNHEARD, MEMBER_CATEGORY_UNHEARD),
        ] {
            let config = pmc_config(json!({
                "gameVersionWeight": {edition: 1.0},
                "accountTypeWeight": {"0": 1.0, "256": 3.0},
            }));
            let (draw, next) = {
                let _guard = TestSeedGuard::install(SEED);
                (
                    set_randomised_game_version_and_category(&config, false)
                        .expect("the single-entry map resolves"),
                    get_double(0.0, 1.0),
                )
            };
            assert_eq!(draw.game_version, edition);
            assert_eq!(draw.member_category, category);
            assert_eq!(draw.selected_member_category, Some(category));
            assert_eq!(next, untouched, "{edition} must skip the account-type draw");
        }
    }

    #[test]
    fn a_plain_game_version_takes_the_account_type_draw_and_parses_its_key() {
        let config = pmc_config(json!({
            "gameVersionWeight": {"standard": 1.0},
            "accountTypeWeight": {"0": 1.0, "256": 3.0},
        }));

        let (draw, spent) = {
            let _guard = TestSeedGuard::install(SEED);
            (
                set_randomised_game_version_and_category(&config, false).expect("standard draws"),
                get_double(0.0, 1.0),
            )
        };
        let untouched = {
            let _guard = TestSeedGuard::install(SEED);
            get_double(0.0, 1.0)
        };

        assert_eq!(draw.game_version, "standard");
        // Pinned at SEED: the weighted account-type draw lands on the "256" key.
        assert_eq!(draw.member_category, 256);
        assert_eq!(draw.selected_member_category, Some(256));
        assert_ne!(spent, untouched, "the account-type draw was consumed");

        // `EftEnumConverter` writes the enum keys as numeric strings; anything else is an error
        // rather than a silent 0.
        let bad = pmc_config(json!({
            "gameVersionWeight": {"standard": 1.0},
            "accountTypeWeight": {"UniqueId": 1.0},
        }));
        let _guard = TestSeedGuard::install(SEED);
        assert!(set_randomised_game_version_and_category(&bad, false).is_err());
    }

    // -- 6. `set_bot_appearance`: draw order, and a fixed-hands body skipping the hands draw.

    fn appearance() -> AppearanceWire {
        // Every map has two entries whose weights sum to 4 rather than 2, so none of them takes
        // `get_weighted_value`'s single-entry or uniform shortcut: each spends one `get_double`.
        // The skew alternates between slots so the four picks cannot all agree — if they did, a
        // permuted draw order would produce the same four values and the order assert below would
        // be vacuous.
        // Their `a`-thresholds are 0.1, 0.9, 0.5 and 0.3 of the stream's uniform — four different
        // cut points, so the same four draws taken in a different order land differently. Without
        // that the order assert below would be vacuous.
        serde_json::from_value(json!({
            "head": {"head_a": 1.0, "head_b": 9.0},
            "feet": {"feet_a": 9.0, "feet_b": 1.0},
            "body": {"body_a": 5.0, "body_b": 5.0},
            "hands": {"hands_a": 3.0, "hands_b": 7.0},
            "voice": {"voice_a": 1.0, "voice_b": 3.0},
        }))
        .expect("appearance fixture parses")
    }

    /// The appearance tests draw from their own seed: [`SEED`] happens to put the first two
    /// uniforms on the same side of both the head and the feet cut points, which would make the
    /// transposition asserts below vacuous.
    const APPEARANCE_SEED: u64 = 5;

    #[test]
    fn the_appearance_draw_order_is_head_feet_body_hands() {
        let no_fixed = IndexMap::new();
        let drawn = {
            let _guard = TestSeedGuard::install(APPEARANCE_SEED);
            set_bot_appearance(&appearance(), &no_fixed).expect("the maps are drawable")
        };

        // The order claim, spelled out: four weighted draws off one stream, taken in the order the
        // slots are named, have to reproduce the call exactly.
        let in_order = |order: [usize; 4]| {
            let _guard = TestSeedGuard::install(APPEARANCE_SEED);
            let map = appearance();
            let weights = [&map.head, &map.feet, &map.body, &map.hands];
            let mut picked = ["".to_owned(), "".to_owned(), "".to_owned(), "".to_owned()];
            for slot in order {
                picked[slot] = get_weighted_value(weights[slot]).expect("the map is drawable");
            }
            picked
        };
        let expected = in_order([0, 1, 2, 3]);
        assert_eq!(
            [drawn.head, drawn.feet, drawn.body, drawn.hands],
            expected,
            "the draw order is Head, Feet, Body, Hands"
        );

        // ...and it is *that* order specifically: every adjacent transposition of it produces a
        // different four-tuple, so the assert above cannot be satisfied by a permuted port.
        for swapped in [[1, 0, 2, 3], [0, 2, 1, 3], [0, 1, 3, 2]] {
            assert_ne!(in_order(swapped), expected, "{swapped:?}");
        }

        // Pinned at SEED.
        assert_eq!(expected, ["head_b", "feet_a", "body_b", "hands_a"]);
    }

    #[test]
    fn a_fixed_hands_body_skips_the_hands_draw() {
        let no_fixed = IndexMap::new();
        // The body pinned above, mapped to hands the weighted map does not even contain.
        let fixed: IndexMap<String, String> =
            [("body_b".to_owned(), "fixed_hands".to_owned())].into();

        let (drawn, next_value) = {
            let _guard = TestSeedGuard::install(APPEARANCE_SEED);
            let drawn = set_bot_appearance(&appearance(), &fixed).expect("the maps are drawable");
            // Whatever the *next* weighted hands draw would yield, off the position the call left.
            let next = get_weighted_value(&appearance().hands).expect("the hands map is drawable");
            (drawn, next)
        };
        let free = {
            let _guard = TestSeedGuard::install(APPEARANCE_SEED);
            set_bot_appearance(&appearance(), &no_fixed).expect("the maps are drawable")
        };

        assert_eq!(drawn.body, "body_b");
        assert_eq!(drawn.hands, "fixed_hands");
        // Stream position: the fixed call stopped exactly where the free call was about to draw its
        // hands, so the follow-up draw reproduces the free call's hands. Had the fixed path drawn
        // and thrown the result away, this would be the draw *after* it.
        assert_eq!(next_value, free.hands);
        // Head/feet/body are unaffected by the fixed-hands lookup.
        assert_eq!((drawn.head, drawn.feet), (free.head, free.feet));

        // A body the map does not hold falls through to the weighted draw — the documented
        // divergence from C#'s throwing `templateTable.Customization[...]` indexer.
        let other: IndexMap<String, String> =
            [("body_a".to_owned(), "fixed_hands".to_owned())].into();
        let missed = {
            let _guard = TestSeedGuard::install(APPEARANCE_SEED);
            set_bot_appearance(&appearance(), &other).expect("the maps are drawable")
        };
        assert_eq!(missed.hands, free.hands);
    }

    // -- 7. `add_dogtag_to_bot`.

    fn dogtags(
        value: serde_json::Value,
    ) -> IndexMap<String, IndexMap<String, IndexMap<String, f64>>> {
        serde_json::from_value(value).expect("dogtag settings parse")
    }

    #[test]
    fn the_dogtag_falls_back_to_default_and_lowercases_the_side() {
        let settings = dogtags(json!({
            "bear": {"unheard_edition": {"bear_unheard": 1.0}, "default": {"bear_default": 1.0}},
            "usec": {"default": {"usec_default": 1.0}},
        }));
        let template_of = |side: &str, version: &str| {
            let mut items = Vec::new();
            add_dogtag_to_bot(&mut items, "equip_id", side, version, &settings)
                .expect("the side and a default band are present");
            items
        };

        // The exact game version wins where it exists...
        assert_eq!(template_of("Bear", UNHEARD)[0].template, "bear_unheard");
        // ...and an unknown one falls back to `default`. The side lookup is lowercased, so the
        // C# `Bear`/`Usec` casing resolves.
        let items = template_of("Bear", "standard");
        assert_eq!(items[0].template, "bear_default");
        assert_eq!(template_of("Usec", "standard")[0].template, "usec_default");

        // The pushed item, field for field.
        let item = &items[0];
        assert_eq!(item.parent_id.as_deref(), Some("equip_id"));
        assert_eq!(item.slot_id.as_deref(), Some("Dogtag"));
        assert_eq!(item.id.len(), 24, "a MongoId: {}", item.id);
        assert_eq!(
            item.upd.as_ref().expect("the dogtag carries an Upd").extra["SpawnedInSession"],
            json!(true)
        );

        // Appends rather than replaces.
        let mut items = vec![Item::default()];
        add_dogtag_to_bot(&mut items, "equip_id", "Bear", "standard", &settings)
            .expect("the side is present");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].template, "bear_default");

        // An unknown side, and a side with neither the version nor a `default`, both error.
        let mut items = Vec::new();
        assert!(
            add_dogtag_to_bot(&mut items, "equip_id", "savage", "standard", &settings).is_err()
        );
        let no_default = dogtags(json!({"bear": {"unheard_edition": {"bear_unheard": 1.0}}}));
        assert!(
            add_dogtag_to_bot(&mut items, "equip_id", "Bear", "standard", &no_default).is_err()
        );
        assert!(items.is_empty(), "a failed draw pushes nothing");
    }
}
