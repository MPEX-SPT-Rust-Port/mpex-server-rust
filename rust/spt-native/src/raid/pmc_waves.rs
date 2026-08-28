//! `PmcWaveGenerator.ApplyWaveChangesToMap` (`PWG:51-64`), the raid-setup family's last pass.
//!
//! Citation convention: a `PWG:N` is a line of `Generators/PmcWaveGenerator.cs` — spelled out
//! because the module's siblings cite `RaidTimeAdjustmentService.cs` bare.
//!
//! Like the raid-start passes this one hands back a *delta*: which boss-wave indices the removal
//! drops. The mutation is the applier's.

use crate::raid::models::{ApplyPmcWavesRequest, ApplyPmcWavesResponse};

/// The removal half of `PmcWaveGenerator.ApplyWaveChangesToMap`. The append never crosses:
/// C# appends its own config objects so the by-reference aliasing into the cloned location
/// survives (spec D3). No RNG, no seed.
pub fn apply_pmc_wave_changes(request: &ApplyPmcWavesRequest) -> ApplyPmcWavesResponse {
    // `PWG:54-56`: all three halves of the guard — the config flag, the map's own wave list, and
    // that list being non-empty. Any one of them off leaves `BossLocationSpawn` alone.
    let apply = request.remove_existing_pmc_waves && request.waves_found && request.wave_count > 0;

    // `PWG:58-59`: a `HashSet<string>` on the default (ordinal) comparer, so the two names match
    // case-SENSITIVELY, and `Contains(null)` is false — a null `BossName` is kept.
    let remove_indices = if apply {
        request
            .boss_names
            .iter()
            .enumerate()
            .filter(|(_, name)| matches!(name.as_deref(), Some("pmcUSEC") | Some("pmcBEAR")))
            .map(|(index, _)| index)
            .collect()
    } else {
        Vec::new()
    };

    ApplyPmcWavesResponse {
        apply,
        remove_indices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_the_two_pmc_boss_names_by_index() {
        let response = apply_pmc_wave_changes(&ApplyPmcWavesRequest {
            remove_existing_pmc_waves: true,
            waves_found: true,
            wave_count: 2,
            boss_names: vec![
                Some("bossBully".into()),
                Some("pmcUSEC".into()),
                None,
                Some("pmcBEAR".into()),
                Some("pmcusec".into()), // case-sensitive: kept
            ],
        });

        assert!(response.apply);
        assert_eq!(response.remove_indices, vec![1, 3]);
    }

    #[test]
    fn any_failed_gate_means_no_apply_and_no_indices() {
        for (remove, found, count) in [(false, true, 2), (true, false, 2), (true, true, 0)] {
            let response = apply_pmc_wave_changes(&ApplyPmcWavesRequest {
                remove_existing_pmc_waves: remove,
                waves_found: found,
                wave_count: count,
                boss_names: vec![Some("pmcUSEC".into())],
            });

            assert!(!response.apply);
            assert!(response.remove_indices.is_empty());
        }
    }
}
