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
