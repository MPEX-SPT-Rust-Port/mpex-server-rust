//! `RaidTimeAdjustmentService.GetRaidAdjustments` (`:201-273`) with the two halves it calls,
//! `GetMapSettings` (`:280-290`) and `GetExitAdjustments` (`:298-374`).

use indexmap::IndexMap;

use crate::loot::random_util::{get_chance_100, get_weighted_value, reduce_value_by_percent};
use crate::raid::RaidError;
use crate::raid::models::{
    ExtractChangeWire, GetRaidAdjustmentsRequest, GetRaidAdjustmentsResponse, RaidChangesWire,
    ScavRaidTimeLocationSettingsWire,
};

/// A randomised adjustment to the raid, off the map settings the caller resolved.
///
/// The C# also writes the result into `ProfileActivityRaidData.RaidAdjustments` (`:270`) on the
/// applied path only; that write is the caller's, keyed off [`GetRaidAdjustmentsResponse::applied`].
///
/// # Errors
///
/// [`RaidError::Failed`] on a location absent from `scavRaidTimeSettings.maps` (Quirk 11) or on a
/// weight key that is not an integer — the two legacy throw points.
pub fn get_adjustments(
    request: &GetRaidAdjustmentsRequest,
) -> Result<GetRaidAdjustmentsResponse, RaidError> {
    let base_escape_time_minutes = request.escape_time_limit;
    let survived_seconds_requirement = f64::from(request.survived_seconds_requirement);

    // `:207-216` — prep result object to return.
    let mut result = RaidChangesWire {
        dynamic_loot_percent: Some(100.0),
        static_loot_percent: Some(100.0),
        simulated_raid_start_seconds: Some(0.0),
        raid_time_minutes: base_escape_time_minutes,
        new_survive_time_seconds: Some(survived_seconds_requirement),
        original_survival_time_seconds: Some(survived_seconds_requirement),
        exit_changes: Vec::new(),
    };

    // Quirk 7 (`:219-222`): a pmc raid sends the default result back — before the map-settings
    // resolve, before any draw, and without the session write. The C# test is
    // `string.Equals(request.Side, "pmc", StringComparison.OrdinalIgnoreCase)`, which a null Side
    // fails; against an all-ASCII literal that is `eq_ignore_ascii_case`.
    if request
        .side
        .as_deref()
        .is_some_and(|side| side.eq_ignore_ascii_case("pmc"))
    {
        return Ok(GetRaidAdjustmentsResponse {
            applied: false,
            chosen_reduction_percent: None,
            map_settings_missing_value: false,
            raid_changes: result,
        });
    }

    // We're scav, adjust values (`:225`, `GetMapSettings`).
    //
    // Quirk 11 (`:282`): `Maps[key]` throws `KeyNotFoundException` on a missing key — booked here
    // as an error naming the map. The shipped `"default"` entry is never consulted.
    if !request.map_settings.found {
        return Err(RaidError::new(format!(
            "Unable to find scav raid time settings for map: {}",
            request.location.as_deref().unwrap_or_default()
        )));
    }

    // Quirk 11 (`:283-287`): only a JSON-null *value* warns and falls back to a default-constructed
    // `ScavRaidTimeLocationSettings`, whose 0% chance makes the roll below a silent no-op.
    let defaults = default_map_settings();
    let map_settings_missing_value = request.map_settings.value.is_none();
    let map_settings = request.map_settings.value.as_ref().unwrap_or(&defaults);

    // Quirk 5 (`:228`): chance of reducing raid time for scav, not guaranteed — and `GetChance100`
    // draws an integer 1-99 and compares `<=`, so a 95% chance fires ≈95/99 of the time. The draw
    // is spent either way.
    if !get_chance_100(map_settings.reduced_chance_percent) {
        // Quirk 7 (`:230-232`): send default, again without the session write.
        return Ok(GetRaidAdjustmentsResponse {
            applied: false,
            chosen_reduction_percent: None,
            map_settings_missing_value,
            raid_changes: result,
        });
    }

    // Quirk 6 (`:235`): get the weighted percent to reduce the raid time by. The twin carries the
    // three draw paths (single entry: none; weights summing to the entry count: one int; else one
    // double) and the negative-weight skip, whose mid-draw warning is dropped and booked.
    //
    // Its only failure is the `InvalidOperationException("No item was picked.")` an empty weight map
    // falls through to (`WeightedRandomHelper.cs:106`) — carried across as its message.
    let chosen_key = get_weighted_value(&map_settings.reduction_percent_weights)
        .map_err(|error| RaidError::new(error.message))?;

    // Booked divergence: `int.Parse` throws a `FormatException` on a non-numeric key; here the
    // message crosses instead.
    let chosen_raid_reduction_percent: i32 = chosen_key.parse().map_err(|_| {
        RaidError::new(format!(
            "Scav raid time reduction percent weight key is not an integer: {chosen_key}"
        ))
    })?;
    let raid_time_remaining_percent = 100 - chosen_raid_reduction_percent;

    // Quirk 16 (`:239`): the reduction base is `EscapeTimeLimit ?? 1d` — a null escape time feeds
    // 1.0 into `ReduceValueByPercent` rather than propagating the null.
    let new_raid_time_minutes = reduce_value_by_percent(
        base_escape_time_minutes.unwrap_or(1.0),
        f64::from(chosen_raid_reduction_percent),
    )
    .floor();

    // `:242-244`: the start time *is* null-propagating, so the two members disagree on a null base.
    let simulated_raid_start_time_minutes =
        base_escape_time_minutes.map(|base| base - new_raid_time_minutes);
    result.simulated_raid_start_seconds =
        simulated_raid_start_time_minutes.map(|minutes| minutes * 60.0);
    result.raid_time_minutes = Some(new_raid_time_minutes);

    // Quirk 15 (`:247-250`): the `?? 0` sits *outside* the subtraction, so a null escape time nulls
    // the whole inner term and the max runs on 0 — not on the original survive time.
    result.new_survive_time_seconds = Some(
        result
            .original_survival_time_seconds
            .zip(base_escape_time_minutes)
            .map(|(original, base)| original - (base - new_raid_time_minutes) * 60.0)
            .unwrap_or(0.0)
            .max(0.0),
    );

    // `:252-256`
    if map_settings.reduce_loot_by_percent {
        result.dynamic_loot_percent =
            Some(f64::from(raid_time_remaining_percent).max(map_settings.min_dynamic_loot_percent));
        result.static_loot_percent =
            Some(f64::from(raid_time_remaining_percent).max(map_settings.min_static_loot_percent));
    }

    // `:263-267`: the `Count != 0` guard on the `AddRange` is a no-op against an empty list.
    result.exit_changes = get_exit_adjustments(request, new_raid_time_minutes);

    // `:260`'s debug line is the applier's, re-emitted off `chosen_reduction_percent`.
    Ok(GetRaidAdjustmentsResponse {
        applied: true,
        chosen_reduction_percent: Some(chosen_raid_reduction_percent),
        map_settings_missing_value,
        raid_changes: result,
    })
}

/// `new ScavRaidTimeLocationSettings()` (`LocationConfig.cs:267-304`) — every member its C#
/// auto-property default, which is what the null-value branch (`:286`) hands back. `AdjustWaves`
/// has no wire member here; it belongs to `MakeAdjustmentsToMap`, not this pass.
fn default_map_settings() -> ScavRaidTimeLocationSettingsWire {
    ScavRaidTimeLocationSettingsWire {
        reduced_chance_percent: 0.0,
        reduction_percent_weights: IndexMap::new(),
        reduce_loot_by_percent: false,
        min_dynamic_loot_percent: 0.0,
        min_static_loot_percent: 0.0,
    }
}

/// `GetExitAdjustments` (`:298-374`) — exit times adjusted for a scav entering part-way through.
///
/// The `PassageRequirement != Train` skip (`:304-307`) is pre-applied caller-side: it draws nothing
/// and logs nothing, so the filtered list is the whole walk.
fn get_exit_adjustments(
    request: &GetRaidAdjustmentsRequest,
    new_raid_time_minutes: f64,
) -> Vec<ExtractChangeWire> {
    let mut result = Vec::new();

    for exit in &request.train_exits {
        // `:310-316`: prepare train adjustment object.
        let mut exit_change = ExtractChangeWire {
            name: exit.name.clone(),
            min_time: None,
            max_time: None,
            chance: None,
        };

        // `:319-322`: at what minute the player is simulated to join, and the seconds elapsed by
        // then. Recomputed per exit as the C# does, and null-propagating.
        let reduction_seconds = request
            .escape_time_limit
            .map(|base| (base - new_raid_time_minutes) * 60.0);

        let train_arrival_delay_seconds = f64::from(request.train_arrival_delay_observed_seconds);

        // Quirk 14 (`:341-344`): `earliestPossibleDepartureMinutes` sums three nullable members, so
        // one missing member nulls the estimate and the lifted `<` below is false — that exit takes
        // the reduce branch rather than the disable one.
        let earliest_possible_departure_minutes =
            match (exit.min_time, exit.count, exit.exfiltration_time) {
                (Some(min_time), Some(count), Some(exfiltration_time)) => Some(
                    (min_time + f64::from(count) + exfiltration_time + train_arrival_delay_seconds)
                        / 60.0,
                ),
                _ => None,
            };
        let most_possible_time_remaining_after_departure = request
            .escape_time_limit
            .zip(earliest_possible_departure_minutes)
            .map(|(base, earliest)| base - earliest);

        // `:345`: if the raid starts after the last moment the train can leave, assume it has gone.
        if most_possible_time_remaining_after_departure
            .is_some_and(|remaining| new_raid_time_minutes < remaining)
        {
            // Quirk 14 (`:347`): the disable branch sets `Chance = 0` and leaves both times null.
            // Its debug line (`:351`) is dropped and booked.
            exit_change.chance = Some(0.0);

            result.push(exit_change);

            continue;
        }

        // Quirk 14 (`:362-363`): `??` binds looser than `-`, so the *whole* subtraction collapses
        // to 0 when either side is null, and only then does the max clamp it. Negative values seem
        // to make the extract turn red in game.
        exit_change.min_time = Some(
            exit.min_time
                .zip(reduction_seconds)
                .map(|(min_time, reduction)| min_time - reduction)
                .unwrap_or(0.0)
                .max(0.0),
        );
        exit_change.max_time = Some(
            exit.max_time
                .zip(reduction_seconds)
                .map(|(max_time, reduction)| max_time - reduction)
                .unwrap_or(0.0)
                .max(0.0),
        );

        result.push(exit_change);
    }

    result
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;
    use crate::loot::random_util::{TestSeedGuard, get_double, get_int};
    use crate::raid::RaidError;
    use crate::raid::models::{
        GetRaidAdjustmentsRequest, MapSettingsState, ScavRaidTimeLocationSettingsWire,
        TrainExitWire,
    };

    /// Arbitrary. Every fixture below is seed-independent by construction — a 100% chance always
    /// passes `GetChance100`'s 1-99 roll and a 0% one never does — so the seed only has to be
    /// stable, not lucky.
    const SEED: u64 = 20_260_826;

    /// The value `get_int(1, 99)` yields on a fresh `seed` stream once `preamble` has replayed the
    /// draws a call is expected to spend. Comparing it against the first draw *after* the call is
    /// how the draw-count tests below pin "exactly these draws, no others" — the replay reproduces
    /// each logical draw's rejection sampling too, which a raw u64 count would not.
    fn draw_after(seed: u64, preamble: impl FnOnce()) -> i32 {
        let _guard = TestSeedGuard::install(seed);
        preamble();

        get_int(1, 99)
    }

    fn settings(
        reduced_chance_percent: f64,
        weights: &[(&str, f64)],
    ) -> ScavRaidTimeLocationSettingsWire {
        ScavRaidTimeLocationSettingsWire {
            reduced_chance_percent,
            reduction_percent_weights: weights
                .iter()
                .map(|(key, weight)| ((*key).to_owned(), *weight))
                .collect::<IndexMap<String, f64>>(),
            reduce_loot_by_percent: false,
            min_dynamic_loot_percent: 0.0,
            min_static_loot_percent: 0.0,
        }
    }

    /// A scav-side request against a 60-minute map, its settings key present.
    fn request(value: Option<ScavRaidTimeLocationSettingsWire>) -> GetRaidAdjustmentsRequest {
        GetRaidAdjustmentsRequest {
            side: Some("Savage".to_owned()),
            location: Some("bigmap".to_owned()),
            escape_time_limit: Some(60.0),
            survived_seconds_requirement: 1_000,
            train_arrival_delay_observed_seconds: 88,
            map_settings: MapSettingsState { found: true, value },
            train_exits: Vec::new(),
            test_seed: None,
        }
    }

    #[test]
    fn a_pmc_side_request_returns_the_default_result_unapplied() {
        // Quirk 7: the pmc-side return precedes every draw *and* the session write.
        let untouched = draw_after(SEED, || {});

        let mut request = request(Some(settings(100.0, &[("20", 1.0)])));
        request.side = Some("PMC".to_owned());

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("the pmc path never fails");

        assert!(!response.applied);
        assert_eq!(response.chosen_reduction_percent, None);
        assert!(!response.map_settings_missing_value);

        let changes = &response.raid_changes;
        assert_eq!(changes.raid_time_minutes, Some(60.0));
        assert_eq!(changes.simulated_raid_start_seconds, Some(0.0));
        assert_eq!(changes.dynamic_loot_percent, Some(100.0));
        assert_eq!(changes.static_loot_percent, Some(100.0));
        assert_eq!(changes.new_survive_time_seconds, Some(1_000.0));
        assert_eq!(changes.original_survival_time_seconds, Some(1_000.0));
        assert!(changes.exit_changes.is_empty());

        // Not one draw spent: the next value off the stream is still its first.
        assert_eq!(get_int(1, 99), untouched);
    }

    #[test]
    fn a_failed_chance_roll_returns_the_default_result_unapplied() {
        let after_the_chance_roll = draw_after(SEED, || {
            get_int(1, 99);
        });

        let request = request(Some(settings(0.0, &[("20", 1.0)])));

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("a failed roll is not an error");

        // Quirk 7: the chance-failed path returns the same untouched default result.
        assert!(!response.applied);
        assert_eq!(response.chosen_reduction_percent, None);
        assert_eq!(response.raid_changes.raid_time_minutes, Some(60.0));
        assert_eq!(
            response.raid_changes.new_survive_time_seconds,
            Some(1_000.0)
        );

        // Quirk 5: the roll is drawn even at 0%, and it is the only draw spent.
        assert_eq!(get_int(1, 99), after_the_chance_roll);
    }

    #[test]
    fn a_missing_map_settings_key_is_an_error() {
        // Quirk 11: `Maps[key]` throws `KeyNotFoundException` on a missing key.
        let mut request = request(None);
        request.map_settings.found = false;

        let _guard = TestSeedGuard::install(SEED);
        let Err(RaidError::Failed(message)) = get_adjustments(&request) else {
            panic!("a map absent from scavRaidTimeSettings.maps is the legacy throw point");
        };

        assert!(message.contains("bigmap"), "{message}");
    }

    #[test]
    fn a_null_map_settings_value_warns_and_defaults() {
        // Quirk 11: only a JSON-null *value* takes the warn+defaults branch.
        let request = request(None);

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("a null value falls back to the defaults");

        assert!(response.map_settings_missing_value);
        // The defaults' 0% chance can never pass the 1-99 roll, so the run stays unapplied.
        assert!(!response.applied);
        assert_eq!(response.raid_changes.raid_time_minutes, Some(60.0));
    }

    #[test]
    fn a_single_entry_weight_map_spends_no_draw() {
        // Quirk 6: one entry short-circuits `GetWeightedValue` before any draw.
        let after_the_chance_roll = draw_after(SEED, || {
            get_int(1, 99);
        });

        let request = request(Some(settings(100.0, &[("20", 1.0)])));

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("a 100% chance always passes");

        assert!(response.applied);
        assert_eq!(response.chosen_reduction_percent, Some(20));
        assert_eq!(get_int(1, 99), after_the_chance_roll);
    }

    #[test]
    fn a_multi_entry_weight_map_spends_one_draw() {
        // Quirk 6: weights summing to 6 over 2 entries miss the `sum == count` early exit, so the
        // walk takes the `GetDouble(0, 1)` path — one draw, not one per entry.
        let after_the_double_draw = draw_after(SEED, || {
            get_int(1, 99);
            get_double(0.0, 1.0);
        });

        let request = request(Some(settings(100.0, &[("20", 5.0), ("40", 1.0)])));

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("a 100% chance always passes");

        assert!(response.applied);
        assert!(matches!(response.chosen_reduction_percent, Some(20 | 40)));
        assert_eq!(get_int(1, 99), after_the_double_draw);
    }

    #[test]
    fn the_reduction_math_matches_the_csharp_formulas() {
        {
            let mut request = request(Some(settings(100.0, &[("20", 1.0)])));
            let map_settings = request
                .map_settings
                .value
                .as_mut()
                .expect("the fixture carries settings");
            map_settings.reduce_loot_by_percent = true;
            map_settings.min_dynamic_loot_percent = 50.0;
            map_settings.min_static_loot_percent = 90.0;

            let _guard = TestSeedGuard::install(SEED);
            let response = get_adjustments(&request).expect("a 100% chance always passes");

            assert!(response.applied);
            assert_eq!(response.chosen_reduction_percent, Some(20));

            let changes = &response.raid_changes;
            // Quirk 16: floor(reduce_value_by_percent(60, 20)) == floor(48) == 48.
            assert_eq!(changes.raid_time_minutes, Some(48.0));
            assert_eq!(changes.simulated_raid_start_seconds, Some(720.0));
            // Quirk 15: max(1000 - (60 - 48) * 60, 0) == 280.
            assert_eq!(changes.new_survive_time_seconds, Some(280.0));
            assert_eq!(changes.original_survival_time_seconds, Some(1_000.0));
            // 100 - 20 = 80, floored per member: the dynamic floor is below it, the static above.
            assert_eq!(changes.dynamic_loot_percent, Some(80.0));
            assert_eq!(changes.static_loot_percent, Some(90.0));
        }

        {
            // Without `reduceLootByPercent` the default 100s stand, floors and all.
            let request = request(Some(settings(100.0, &[("20", 1.0)])));

            let _guard = TestSeedGuard::install(SEED);
            let response = get_adjustments(&request).expect("a 100% chance always passes");

            assert!(response.applied);
            assert_eq!(response.raid_changes.dynamic_loot_percent, Some(100.0));
            assert_eq!(response.raid_changes.static_loot_percent, Some(100.0));
        }
    }

    #[test]
    fn a_none_escape_time_feeds_one_into_the_reduction() {
        // Quirk 16: the reduction base is `EscapeTimeLimit ?? 1d`.
        let mut request = request(Some(settings(100.0, &[("20", 1.0)])));
        request.escape_time_limit = None;

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("a 100% chance always passes");

        // floor(reduce_value_by_percent(1.0, 20)) == floor(0.8) == 0.
        assert_eq!(response.raid_changes.raid_time_minutes, Some(0.0));
        // The start time is null-propagating, so the null base carries through.
        assert_eq!(response.raid_changes.simulated_raid_start_seconds, None);
        // Quirk 15: the `?? 0` sits outside the subtraction, so the whole term collapses to 0
        // rather than leaving the original survive time in place.
        assert_eq!(response.raid_changes.new_survive_time_seconds, Some(0.0));
    }

    #[test]
    fn a_non_numeric_weight_key_is_an_error() {
        // Booked divergence: `int.Parse` throws a `FormatException` on a non-numeric key.
        let request = request(Some(settings(100.0, &[("twenty", 1.0)])));

        let _guard = TestSeedGuard::install(SEED);
        let Err(RaidError::Failed(message)) = get_adjustments(&request) else {
            panic!("a non-numeric weight key is the legacy FormatException point");
        };

        assert!(message.contains("twenty"), "{message}");
    }

    #[test]
    fn train_exit_adjustments_match_the_csharp_walk() {
        let mut request = request(Some(settings(100.0, &[("20", 1.0)])));
        request.train_exits = vec![
            // (1 + 1 + 1 + 88) / 60 = 1.52 minutes, leaving 58.48 of the 60 — the 48-minute raid
            // starts after the train's earliest departure, so the extract is disabled.
            TrainExitWire {
                name: Some("EarlyTrain".to_owned()),
                min_time: Some(1.0),
                max_time: Some(2.0),
                count: Some(1),
                exfiltration_time: Some(1.0),
            },
            // (800 + 60 + 5 + 88) / 60 = 15.88, leaving 44.12 — below the 48-minute raid, so the
            // times are reduced by the 720 elapsed seconds instead.
            TrainExitWire {
                name: Some("LateTrain".to_owned()),
                min_time: Some(800.0),
                max_time: Some(900.0),
                count: Some(60),
                exfiltration_time: Some(5.0),
            },
            // A null member nulls the whole departure estimate, and the lifted `<` against null is
            // false — so this one reduces too, and its null MinTime collapses the subtraction.
            TrainExitWire {
                name: Some("UnknownTrain".to_owned()),
                min_time: None,
                max_time: Some(900.0),
                count: Some(60),
                exfiltration_time: Some(5.0),
            },
        ];

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("a 100% chance always passes");

        let changes = &response.raid_changes.exit_changes;
        assert_eq!(changes.len(), 3);

        // Quirk 14: the disable branch sets `Chance = 0` and leaves both times null.
        assert_eq!(changes[0].name.as_deref(), Some("EarlyTrain"));
        assert_eq!(changes[0].chance, Some(0.0));
        assert_eq!(changes[0].min_time, None);
        assert_eq!(changes[0].max_time, None);

        assert_eq!(changes[1].name.as_deref(), Some("LateTrain"));
        assert_eq!(changes[1].chance, None);
        assert_eq!(changes[1].min_time, Some(80.0));
        assert_eq!(changes[1].max_time, Some(180.0));

        // The null MinTime zeroes its own member only — the max time still reduces normally, so
        // this is the null-estimate branch and not a blanket zeroing of the whole exit. Which side
        // of the `??` the null came from is pinned by the test below, not by this one.
        assert_eq!(changes[2].name.as_deref(), Some("UnknownTrain"));
        assert_eq!(changes[2].chance, None);
        assert_eq!(changes[2].min_time, Some(0.0));
        assert_eq!(changes[2].max_time, Some(180.0));
    }

    #[test]
    fn a_none_escape_time_zeroes_the_train_times() {
        // Quirk 14, the case that pins the `??` binding: a null escape time nulls the *reduction*,
        // so the whole `MinTime - reductionSeconds` collapses to 0 before the max. Binding the `??`
        // to the reduction alone instead would leave the untouched 800/900 standing.
        let mut request = request(Some(settings(100.0, &[("20", 1.0)])));
        request.escape_time_limit = None;
        request.train_exits = vec![TrainExitWire {
            name: Some("LateTrain".to_owned()),
            min_time: Some(800.0),
            max_time: Some(900.0),
            count: Some(60),
            exfiltration_time: Some(5.0),
        }];

        let _guard = TestSeedGuard::install(SEED);
        let response = get_adjustments(&request).expect("a 100% chance always passes");

        let changes = &response.raid_changes.exit_changes;
        assert_eq!(changes.len(), 1);
        // The null escape time also nulls `mostPossibleTimeRemainingAfterDeparture`, so the lifted
        // `<` is false and the exit reduces rather than disabling.
        assert_eq!(changes[0].chance, None);
        assert_eq!(changes[0].min_time, Some(0.0));
        assert_eq!(changes[0].max_time, Some(0.0));
    }
}
