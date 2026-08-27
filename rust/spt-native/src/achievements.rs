//! `Controllers/AchievementController.cs` — `GetAchievementStatics`'s counting loop. Its sibling
//! `GetAchievements` is a bare field return and stays C#-side.

use std::any::Any;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loot::random_util::round_half_even;

/// What a statistics pass can fail with: the message of a C#-sanctioned throw carried back to the
/// caller instead of unwinding (the [`crate::raid::RaidError`] shape — this port carries no epoch
/// either).
#[derive(Debug)]
pub enum AchievementError {
    Failed(String),
}

impl AchievementError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// The profile projection C# builds: the achievement table's ids in table order, the count of
/// non-blacklisted profiles, and one key set per profile that has an achievements dictionary. The
/// `ProfileHelper.GetProfiles()` call and the `AchievementProfileIdBlacklist` filter stay C#-side.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AchievementStatisticsRequest {
    pub achievement_ids: Vec<String>,
    /// The denominator: profiles with no achievements dictionary count here and ship no set.
    pub profile_count: i32,
    pub completed_sets: Vec<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AchievementStatisticsResponse {
    pub elements: IndexMap<String, i32>,
}

/// The module boundary: one `CompletedAchievementsResponse`'s worth of percentages. Draws nothing,
/// so no seed guard; the `catch_unwind` keeps a port bug off `STATUS_PANIC`.
///
/// # Errors
///
/// [`AchievementError::Failed`] on a duplicate achievement id — legacy's `stats.Add`
/// `ArgumentException` — or a panic message.
pub fn get_achievement_statistics(
    request: AchievementStatisticsRequest,
) -> Result<AchievementStatisticsResponse, AchievementError> {
    catch_unwind(AssertUnwindSafe(|| get_statistics(&request)))
        .unwrap_or_else(|payload| Err(panic_message(payload)))
}

/// `GetAchievementStatics`'s loop (`AchievementController.cs:38-67`). The response map's order is
/// the achievement iteration order — the legacy dictionary serializes to the client in insertion
/// order and that order is observable JSON. `(int)Math.Round(double)` is banker's rounding, hence
/// `round_half_even`. No RNG, no seed.
fn get_statistics(
    request: &AchievementStatisticsRequest,
) -> Result<AchievementStatisticsResponse, AchievementError> {
    let sets: Vec<HashSet<&str>> = request
        .completed_sets
        .iter()
        .map(|set| set.iter().map(String::as_str).collect())
        .collect();

    let mut elements = IndexMap::new();
    for id in &request.achievement_ids {
        // `Where(achievementId => !string.IsNullOrEmpty(achievementId))` (`:41`)
        if id.is_empty() {
            continue;
        }
        // Booked divergence: legacy's `stats.Add` (`:66`) throws `ArgumentException` here.
        if elements.contains_key(id.as_str()) {
            return Err(AchievementError::new(format!(
                "duplicate achievement id: {id}"
            )));
        }
        let have = sets.iter().filter(|set| set.contains(id.as_str())).count();
        let percentage = if request.profile_count > 0 {
            round_half_even(have as f64 / f64::from(request.profile_count) * 100.0) as i32
        } else {
            0
        };
        elements.insert(id.clone(), percentage);
    }

    Ok(AchievementStatisticsResponse { elements })
}

/// The text a caught panic carries — `panic!`/`expect` payloads are a `String` or a `&str`.
fn panic_message(payload: Box<dyn Any + Send>) -> AchievementError {
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "achievement statistics panicked".to_owned());

    AchievementError::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_are_banker_rounded_in_achievement_order() {
        let response = get_achievement_statistics(AchievementStatisticsRequest {
            achievement_ids: vec!["b".into(), "a".into(), "".into(), "c".into()],
            profile_count: 8,
            completed_sets: vec![
                vec!["a".into(), "b".into()],
                vec!["b".into()],
                vec!["a".into()],
            ],
        })
        .unwrap();
        // "" skipped; order is b, a, c; 2/8*100=25, 2/8*100=25, 0
        assert_eq!(
            response
                .elements
                .iter()
                .map(|(k, v)| (k.as_str(), *v))
                .collect::<Vec<_>>(),
            vec![("b", 25), ("a", 25), ("c", 0)]
        );
    }

    #[test]
    fn half_percentages_round_to_even() {
        // 1 of 8 profiles = 12.5 → 12 (ToEven), where round-half-up would say 13
        let response = get_achievement_statistics(AchievementStatisticsRequest {
            achievement_ids: vec!["a".into()],
            profile_count: 8,
            completed_sets: vec![vec!["a".into()]],
        })
        .unwrap();
        assert_eq!(response.elements["a"], 12);
    }

    #[test]
    fn zero_profiles_yield_zero_percent_entries() {
        let response = get_achievement_statistics(AchievementStatisticsRequest {
            achievement_ids: vec!["a".into()],
            profile_count: 0,
            completed_sets: vec![],
        })
        .unwrap();
        assert_eq!(response.elements["a"], 0);
    }

    #[test]
    fn a_duplicate_achievement_id_is_an_error() {
        let result = get_achievement_statistics(AchievementStatisticsRequest {
            achievement_ids: vec!["a".into(), "a".into()],
            profile_count: 1,
            completed_sets: vec![],
        });
        assert!(result.is_err());
    }
}
