//! Wire types for the raid-setup family — a fresh C#↔Rust contract mirrored member-for-member
//! by `Native/Raid/RaidPayloads.cs`. camelCase throughout, except the `RaidChanges`/`ExtractChange`
//! mirrors, which reuse the real C# records' JSON names (`ExtractChange` is PascalCase).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRaidAdjustmentsRequest {
    pub side: Option<String>,
    pub location: Option<String>,
    pub escape_time_limit: Option<f64>,
    pub survived_seconds_requirement: i32, // non-nullable C# int (GlobalTable.cs:1211)
    pub train_arrival_delay_observed_seconds: i32, // non-nullable C# int (LocationConfig.cs:264)
    pub map_settings: MapSettingsState,
    pub train_exits: Vec<TrainExitWire>,
    pub test_seed: Option<u64>,
}

/// Three-state projection of `locationConfig.ScavRaidTimeSettings.Maps` via
/// `TryGetValue(location.ToLowerInvariant(), …)` — the lowercasing is load-bearing (RTAS:282;
/// shipped base.json Ids are mixed-case). found=false → the legacy KeyNotFoundException point;
/// found=true+value=None → the warn+defaults branch (Quirk 11); found=true+value=Some → the
/// settings.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapSettingsState {
    pub found: bool,
    pub value: Option<ScavRaidTimeLocationSettingsWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScavRaidTimeLocationSettingsWire {
    pub reduced_chance_percent: f64, // non-nullable C# double — legacy always consumes the draw
    /// Insertion order defines the cumulative-weight walk (Quirk 6) — IndexMap mandatory.
    pub reduction_percent_weights: IndexMap<String, f64>,
    pub reduce_loot_by_percent: bool,
    pub min_dynamic_loot_percent: f64, // non-nullable C# double
    pub min_static_loot_percent: f64,  // non-nullable C# double
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainExitWire {
    pub name: Option<String>,
    pub min_time: Option<f64>,
    pub max_time: Option<f64>,
    pub count: Option<i32>, // Exit.Count is int? (LocationBase.cs:818)
    pub exfiltration_time: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRaidAdjustmentsResponse {
    pub applied: bool,
    /// Some on the applied path — the RTAS:260 debug line's percent (not derivable from the
    /// clamped loot members); the applier re-emits the line from it.
    pub chosen_reduction_percent: Option<i32>,
    pub map_settings_missing_value: bool,
    pub raid_changes: RaidChangesWire,
}

/// Mirrors `Models/Spt/Location/RaidChanges.cs` — deserialized C#-side as the real record.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaidChangesWire {
    pub dynamic_loot_percent: Option<f64>,
    pub static_loot_percent: Option<f64>,
    pub simulated_raid_start_seconds: Option<f64>,
    pub raid_time_minutes: Option<f64>,
    pub new_survive_time_seconds: Option<f64>,
    pub original_survival_time_seconds: Option<f64>,
    pub exit_changes: Vec<ExtractChangeWire>,
}

/// Mirrors `ExtractChange` — PascalCase wire names, matching its `[JsonPropertyName]`s.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExtractChangeWire {
    pub name: Option<String>,
    pub min_time: Option<f64>,
    pub max_time: Option<f64>,
    pub chance: Option<f64>,
}

// ---- Export 2: spt_make_adjustments_to_map ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MakeAdjustmentsRequest {
    pub map_id: Option<String>, // for the missing-key error text only
    pub raid_changes: RaidChangesInWire,
    /// Three-state like MapSettingsState, via Maps.TryGetValue(map_id.ToLowerInvariant(), …):
    /// found=false → Err; value=None → warn flag + waves disabled; value=Some(b) → AdjustWaves=b.
    pub map_settings: MapSettingsAdjustState,
    pub exits: Vec<Option<String>>, // exit names, from the builder-materialized exit list
    pub waves: Vec<WaveTimesWire>,  // Wave.TimeMin/TimeMax are int?
    pub boss_spawns: Vec<BossSpawnWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RaidChangesInWire {
    pub raid_time_minutes: Option<f64>,
    pub simulated_raid_start_seconds: Option<f64>,
    pub exit_changes: Vec<ExtractChangeInWire>,
}

/// PascalCase inner, mirroring `ExtractChange`'s `[JsonPropertyName]`s.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExtractChangeInWire {
    pub name: Option<String>,
    pub min_time: Option<f64>,
    pub max_time: Option<f64>,
    pub chance: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapSettingsAdjustState {
    pub found: bool,
    pub value: Option<bool>, // AdjustWaves
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveTimesWire {
    pub time_min: Option<i32>,
    pub time_max: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BossSpawnWire {
    pub boss_name: Option<String>,
    pub time: Option<f64>, // BossLocationSpawn.Time is double?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MakeAdjustmentsResponse {
    pub escape_time_limit: Option<f64>, // always applied (Quirk 13)
    pub exit_updates: Vec<ExitUpdateWire>,
    /// Quirk 1: the unmatched-exit return. `aborted` alone is authoritative — the name is log
    /// payload only (ExtractChange.Name is nullable, so Option<String> alone can't encode this).
    pub aborted: bool,
    pub aborted_exit_name: Option<String>,
    /// The RTAS:285 warning — set only when the resolve ran (never on an aborted run).
    pub map_settings_missing_value: bool,
    pub wave_adjustments: Option<WaveAdjustmentsWire>, // None when AdjustWaves is off or aborted
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitUpdateWire {
    pub index: usize, // into the builder-materialized exit list
    pub chance: Option<f64>,
    pub min_time: Option<f64>,
    pub max_time: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveAdjustmentsWire {
    pub wave_keep_indices: Vec<usize>,
    /// Final absolute values per kept wave, in keep order (double subtraction applied — Quirk 2).
    pub wave_times: Vec<WaveTimesWire>,
    pub boss_keep_indices: Vec<usize>,
    pub boss_time_updates: Vec<BossTimeUpdateWire>,
    /// `firstPmcSpawn.Time.GetValueOrDefault(1)` — the offset SUBTRAHEND (RTAS:175), not the
    /// applied offset; None when no pmc spawn survived. The applier emits the RTAS:184 debug
    /// line only when Some.
    pub pmc_start_seconds: Option<f64>,
    pub removed_wave_count: usize,
    pub removed_boss_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BossTimeUpdateWire {
    pub index: usize, // into the request bossSpawns array — the ORIGINAL list, not the kept one
    pub time: f64,
}
