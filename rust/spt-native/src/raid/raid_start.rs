//! `LocationLifecycleService.AdjustExtracts` (`LLS:251-275`) and
//! `LocationLifecycleService.AdjustBotHostilitySettings` (`LLS:281-363`), the raid-start half of
//! the family.
//!
//! Citation convention: an `LLS:N` is a line of `Services/InRaid/LocationLifecycleService.cs` —
//! spelled out here because the module's siblings cite `RaidTimeAdjustmentService.cs` bare.
//!
//! Both passes hand back *deltas*: which location entry each config role matched and which ops to
//! run on it, and which extracts to append. The mutations — and every warning — are the applier's,
//! so legacy's live-object aliasing (Quirks 8 and 9) stays C#'s own.

use crate::raid::RaidError;
use crate::raid::models::{
    AdjustExtractsRequest, AdjustExtractsResponse, AdjustHostilityRequest, AdjustHostilityResponse,
    HostilityEntryWire,
};

/// `LLS:75-76` — the side constants `IsSide` is called with here.
const SAVAGE: &str = "savage";
const SCAV: &str = "scav";

/// `IsSide` (`LLS:84-87`): `string.Equals(playerSide, sideCheck, OrdinalIgnoreCase)`, which a null
/// side fails. The side checked against is an ASCII constant, so ordinal-ignore-case is
/// `eq_ignore_ascii_case` — the port's standing convention for the comparison.
fn is_side(side: Option<&str>, side_check: &str) -> bool {
    side.is_some_and(|side| side.eq_ignore_ascii_case(side_check))
}

/// Which of the config's hostility changes apply to which location entry, one entry per config
/// role in config insertion order — the applier walks the list once, so legacy's warn/apply
/// interleaving inside its single loop (`LLS:283-362`) survives the split.
///
/// # Errors
///
/// [`RaidError::Failed`] on Quirk 10: a matched location entry whose `AlwaysEnemies` is null, with
/// enemy types to add to it. Legacy NREs there; the native arm reports it and applies **nothing**,
/// where legacy would have kept the earlier roles' mutations — the same half-applied asymmetry the
/// family's other error points carry, unobservable because the throw abandons the cloned map.
pub fn adjust_bot_hostility_settings(
    request: &AdjustHostilityRequest,
) -> Result<AdjustHostilityResponse, RaidError> {
    let mut entries = Vec::with_capacity(request.hostility_settings.len());

    // `LLS:283-285`: one pass over the config map, in its own insertion order.
    for (role, config) in &request.hostility_settings {
        // `LLS:286-288`: `FirstOrDefault` on an `OrdinalIgnoreCase` `BotRole` match — a null
        // `BotRole` never equals a non-null key, and a null `AdditionalHostilitySettings`
        // (`location_settings: None`) no-ops the whole probe.
        let matched = request.location_settings.as_deref().and_then(|settings| {
            settings.iter().enumerate().find(|(_, entry)| {
                entry
                    .bot_role
                    .as_deref()
                    .is_some_and(|bot_role| bot_role.eq_ignore_ascii_case(role))
            })
        });

        let mut entry = HostilityEntryWire {
            role: role.clone(),
            matched_index: matched.map(|(index, _)| index),
            add_always_enemies: Vec::new(),
            run_chanced_enemies_loop: false,
            set_always_friends: None,
            bear_enemy_chance: None,
            usec_enemy_chance: None,
            savage_enemy_chance: None,
            savage_player_behaviour: None,
        };

        // `LLS:291-296`: no matching bot in config, skip. The warning is the applier's — it holds
        // the live `KeyValuePair` the message interpolates (Quirk 12).
        let Some((_, location)) = matched else {
            entries.push(entry);
            continue;
        };

        // `LLS:299-305`: add new permanent enemies if they don't already exist.
        if let Some(enemy_types) = &config.additional_enemy_types {
            // Quirk 10 (`LLS:303`): `AlwaysEnemies` is a nullable `HashSet`, so a null one NREs on
            // the first `Add` — an *empty* list never reaches the add and is fine.
            if !enemy_types.is_empty() && location.always_enemies_is_null {
                return Err(RaidError::new(format!(
                    "Bot: {role} has no AlwaysEnemies list on the location to add enemy types to"
                )));
            }

            entry.add_always_enemies = enemy_types.clone();
        }

        // Quirk 8 (`LLS:308-326`): a non-null `ChancedEnemies` — **empty included** — clears the
        // location list and then refills it with the probe-as-you-fill loop. The loop itself stays
        // in the applier, verbatim, because its merge branch writes through a live reference.
        entry.run_chanced_enemies_loop = config.has_chanced_enemies;

        // `LLS:330-336`: non-null `AdditionalFriendlyTypes` resets `AlwaysFriends` before the
        // fill, so an empty list is a clear — `Some(vec![])`, never `None`.
        entry.set_always_friends = config.additional_friendly_types.clone();

        // `LLS:340-361`: the four scalar copies. Legacy guards each with its own null check, which
        // the `Option` carries across as-is — a `None` is the write that legacy skips.
        entry.bear_enemy_chance = config.bear_enemy_chance;
        entry.usec_enemy_chance = config.usec_enemy_chance;
        entry.savage_enemy_chance = config.savage_enemy_chance;
        entry.savage_player_behaviour = config.savage_player_behaviour.clone();

        entries.push(entry);
    }

    Ok(AdjustHostilityResponse { entries })
}

/// Which of the map's extracts a scav player's exit list gains. Infallible: every branch of
/// `AdjustExtracts` either returns early or filters a list.
pub fn adjust_extracts(request: &AdjustExtractsRequest) -> AdjustExtractsResponse {
    // `LLS:253-257`: a non-scav player leaves the exits alone — before the map lookup, so an
    // unknown map never warns on the pmc side.
    if !is_side(request.player_side.as_deref(), SAVAGE) {
        return AdjustExtractsResponse {
            warn_unknown_map: false,
            append_extract_indices: Vec::new(),
        };
    }

    // `LLS:260-266`: no extract data for the map — the applier emits the warning, which names the
    // location the request does not carry.
    if !request.map_found {
        return AdjustExtractsResponse {
            warn_unknown_map: true,
            append_extract_indices: Vec::new(),
        };
    }

    // `LLS:269-274` (Quirk 9): find only scav extracts and overwrite existing exits with them. An
    // empty result is the `.Any()` false branch — no append at all, which an empty index list
    // says. The `Union` itself stays in the applier: its operand is a deferred sequence of live
    // `AllExtractsExit` instances, and its record-equality dedup is C#'s.
    AdjustExtractsResponse {
        warn_unknown_map: false,
        append_extract_indices: request
            .extract_sides
            .iter()
            .enumerate()
            .filter(|(_, side)| is_side(side.as_deref(), SCAV))
            .map(|(index, _)| index)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;
    use crate::raid::models::{HostilityConfigWire, LocationHostilityWire};

    /// A config entry that changes nothing; each test turns on only the members it is about.
    fn config() -> HostilityConfigWire {
        HostilityConfigWire {
            additional_enemy_types: None,
            has_chanced_enemies: false,
            additional_friendly_types: None,
            bear_enemy_chance: None,
            usec_enemy_chance: None,
            savage_enemy_chance: None,
            savage_player_behaviour: None,
        }
    }

    fn location(bot_role: &str) -> LocationHostilityWire {
        LocationHostilityWire {
            bot_role: Some(bot_role.to_owned()),
            always_enemies_is_null: false,
        }
    }

    fn request(
        hostility_settings: Vec<(&str, HostilityConfigWire)>,
        location_settings: Option<Vec<LocationHostilityWire>>,
    ) -> AdjustHostilityRequest {
        AdjustHostilityRequest {
            hostility_settings: hostility_settings
                .into_iter()
                .map(|(role, config)| (role.to_owned(), config))
                .collect::<IndexMap<String, HostilityConfigWire>>(),
            location_settings,
        }
    }

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn an_unmatched_role_is_reported_not_matched() {
        let mut unmatched = config();
        unmatched.additional_enemy_types = Some(owned(&["sptBear"]));
        unmatched.has_chanced_enemies = true;
        unmatched.additional_friendly_types = Some(owned(&["sptUsec"]));
        unmatched.bear_enemy_chance = Some(50.0);

        let response = adjust_bot_hostility_settings(&request(
            vec![("assault", unmatched)],
            Some(vec![location("bossTagilla")]),
        ))
        .expect("an unmatched role is a skip, not a failure");

        assert_eq!(response.entries.len(), 1);
        let entry = &response.entries[0];
        assert_eq!(entry.role, "assault");
        assert_eq!(entry.matched_index, None);
        // `LLS:295` `continue`s past every op, so the entry carries none of the config's changes.
        assert!(entry.add_always_enemies.is_empty());
        assert!(!entry.run_chanced_enemies_loop);
        assert_eq!(entry.set_always_friends, None);
        assert_eq!(entry.bear_enemy_chance, None);
    }

    #[test]
    fn entries_come_back_one_per_config_role_in_config_order() {
        // Matched and unmatched interleaved: the single ordered list is what preserves legacy's
        // warn/apply interleaving, which grouping the warnings first would reorder.
        let response = adjust_bot_hostility_settings(&request(
            vec![
                ("pmcBot", config()),
                ("assault", config()),
                ("bossBully", config()),
            ],
            Some(vec![location("assault")]),
        ))
        .expect("nothing here can fail");

        let roles: Vec<&str> = response
            .entries
            .iter()
            .map(|entry| entry.role.as_str())
            .collect();
        assert_eq!(roles, vec!["pmcBot", "assault", "bossBully"]);

        let matches: Vec<Option<usize>> = response
            .entries
            .iter()
            .map(|entry| entry.matched_index)
            .collect();
        assert_eq!(matches, vec![None, Some(0), None]);
    }

    #[test]
    fn a_matched_role_with_non_null_chanced_enemies_gets_the_loop_flag() {
        // Quirk 8: `hasChancedEnemies` is a pure null check — a non-null but EMPTY config list
        // still enters the branch, and the clear at `LLS:310` still has to run.
        let mut chanced = config();
        chanced.has_chanced_enemies = true;

        let response = adjust_bot_hostility_settings(&request(
            vec![("assault", chanced), ("bossBully", config())],
            Some(vec![location("assault"), location("bossBully")]),
        ))
        .expect("nothing here can fail");

        assert!(response.entries[0].run_chanced_enemies_loop);
        // A null `ChancedEnemies` leaves the location list untouched.
        assert!(!response.entries[1].run_chanced_enemies_loop);
    }

    #[test]
    fn a_null_always_enemies_with_additional_types_is_an_error() {
        // Quirk 10 (`LLS:303`): the first `AlwaysEnemies.Add` on a null set NREs in legacy.
        let mut adds_enemies = config();
        adds_enemies.additional_enemy_types = Some(owned(&["sptBear"]));

        let null_always_enemies = LocationHostilityWire {
            bot_role: Some("assault".to_owned()),
            always_enemies_is_null: true,
        };

        let Err(RaidError::Failed(message)) = adjust_bot_hostility_settings(&request(
            vec![("assault", adds_enemies)],
            Some(vec![null_always_enemies]),
        )) else {
            panic!("a null AlwaysEnemies with types to add is the legacy NRE point");
        };
        assert!(message.contains("assault"), "{message}");

        // The boundary: an empty list never reaches the `Add`, so legacy never throws there.
        let mut adds_nothing = config();
        adds_nothing.additional_enemy_types = Some(Vec::new());

        let response = adjust_bot_hostility_settings(&request(
            vec![("assault", adds_nothing)],
            Some(vec![LocationHostilityWire {
                bot_role: Some("assault".to_owned()),
                always_enemies_is_null: true,
            }]),
        ))
        .expect("an empty AdditionalEnemyTypes never touches AlwaysEnemies");
        assert!(response.entries[0].add_always_enemies.is_empty());
    }

    #[test]
    fn a_none_location_settings_reports_every_role_unmatched() {
        // A null `AdditionalHostilitySettings`: legacy's `?.FirstOrDefault` yields null per role,
        // so every role warns and skips.
        let response = adjust_bot_hostility_settings(&request(
            vec![("assault", config()), ("bossBully", config())],
            None,
        ))
        .expect("nothing here can fail");

        assert_eq!(response.entries.len(), 2);
        assert!(
            response
                .entries
                .iter()
                .all(|entry| entry.matched_index.is_none())
        );
    }

    #[test]
    fn matching_is_case_insensitive_on_bot_role() {
        let response = adjust_bot_hostility_settings(&request(
            vec![("assaultGroup", config())],
            Some(vec![location("ASSAULTGROUP")]),
        ))
        .expect("nothing here can fail");

        assert_eq!(response.entries[0].matched_index, Some(0));
    }

    #[test]
    fn scalars_copy_only_when_present() {
        let mut scalars = config();
        scalars.bear_enemy_chance = Some(50.0);
        scalars.savage_enemy_chance = Some(3.0);
        scalars.savage_player_behaviour = Some("AlwaysEnemy".to_owned());

        let response = adjust_bot_hostility_settings(&request(
            vec![("assault", scalars)],
            Some(vec![location("assault")]),
        ))
        .expect("nothing here can fail");

        let entry = &response.entries[0];
        assert_eq!(entry.bear_enemy_chance, Some(50.0));
        assert_eq!(entry.savage_enemy_chance, Some(3.0));
        assert_eq!(
            entry.savage_player_behaviour.as_deref(),
            Some("AlwaysEnemy")
        );
        // The unset one stays absent: `LLS:346-349` writes nothing on a null.
        assert_eq!(entry.usec_enemy_chance, None);
    }

    #[test]
    fn an_empty_friendly_types_list_still_sets_always_friends() {
        // `LLS:330-336`: the reset at `:332` is inside the non-null branch, so an empty list is a
        // clear — `Some(vec![])`, which `None` (the null config) must not collapse into.
        let mut clears_friends = config();
        clears_friends.additional_friendly_types = Some(Vec::new());

        let response = adjust_bot_hostility_settings(&request(
            vec![("assault", clears_friends), ("bossBully", config())],
            Some(vec![location("assault"), location("bossBully")]),
        ))
        .expect("nothing here can fail");

        assert_eq!(response.entries[0].set_always_friends, Some(Vec::new()));
        assert_eq!(response.entries[1].set_always_friends, None);
    }

    fn extracts_request(
        player_side: &str,
        map_found: bool,
        sides: &[Option<&str>],
    ) -> AdjustExtractsRequest {
        AdjustExtractsRequest {
            player_side: Some(player_side.to_owned()),
            map_found,
            extract_sides: sides
                .iter()
                .map(|side| side.map(std::borrow::ToOwned::to_owned))
                .collect(),
        }
    }

    #[test]
    fn a_non_savage_side_appends_nothing() {
        // `LLS:253-257`, ignore-case against `"savage"`: a pmc-side raid returns before the map
        // lookup, so the unknown map below never warns either.
        let response = adjust_extracts(&extracts_request("Usec", false, &[Some("scav")]));

        assert!(!response.warn_unknown_map);
        assert!(response.append_extract_indices.is_empty());
    }

    #[test]
    fn an_unknown_map_warns_and_appends_nothing() {
        let response = adjust_extracts(&extracts_request("SAVAGE", false, &[Some("scav")]));

        assert!(response.warn_unknown_map);
        assert!(response.append_extract_indices.is_empty());
    }

    #[test]
    fn scav_side_extracts_are_selected_by_index_ignore_case() {
        let response = adjust_extracts(&extracts_request(
            "savage",
            true,
            &[Some("Pmc"), Some("SCAV"), None, Some("scav")],
        ));

        assert!(!response.warn_unknown_map);
        // Indices into the materialized `AllExtracts` list, and a null `Side` never matches.
        assert_eq!(response.append_extract_indices, vec![1, 3]);
    }

    #[test]
    fn no_scav_extracts_means_no_append() {
        // The `.Any()` false branch (`LLS:270`): the exits are left exactly as they were.
        let response = adjust_extracts(&extracts_request("savage", true, &[Some("Pmc"), None]));

        assert!(!response.warn_unknown_map);
        assert!(response.append_extract_indices.is_empty());
    }
}
