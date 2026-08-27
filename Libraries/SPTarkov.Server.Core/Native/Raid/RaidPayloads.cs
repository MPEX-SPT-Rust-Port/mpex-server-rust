using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Spt.Location;

namespace SPTarkov.Server.Core.Native.Raid;

/// <summary>
/// The request/response envelopes of the raid-setup exports, mirroring
/// <c>rust/spt-native/src/raid/models.rs</c> member for member. Conventions are
/// <c>Native/ScavCase/ScavCasePayloads.cs</c>'s: an explicit
/// <see cref="JsonPropertyNameAttribute"/> on every member, members Rust declares as
/// <c>Option&lt;T&gt;</c> nullable and everything else <c>required</c>.
///
/// The one type this family does not mirror is the raid changes themselves: the response's
/// <see cref="GetRaidAdjustmentsResponse.RaidChanges"/> and the request member
/// <see cref="MakeAdjustmentsRequest.RaidChanges"/> are both the real
/// <see cref="Models.Spt.Location.RaidChanges"/>, so a decoded response lands directly on the
/// record the session parks and the HTTP response carries, and the second export is handed the
/// very record the session parked back. Its <see cref="Models.Spt.Location.ExtractChange"/>
/// members are PascalCase on the wire, which the Rust mirror spells out; the four members
/// <c>RaidChangesInWire</c> does not declare cross anyway and Rust ignores them.
/// </summary>
public record GetRaidAdjustmentsRequest
{
    /// <summary>
    ///     <c>GetRaidTimeRequest.Side</c> verbatim - the native side runs the same
    ///     case-insensitive <c>"pmc"</c> test a null fails.
    /// </summary>
    [JsonPropertyName("side")]
    public string? Side { get; set; }

    /// <summary>
    ///     <c>GetRaidTimeRequest.Location</c> verbatim, for the missing-key error text only: the
    ///     settings lookup itself is resolved caller-side into <see cref="MapSettings"/>.
    /// </summary>
    [JsonPropertyName("location")]
    public string? Location { get; set; }

    /// <summary>
    ///     <c>LocationBase.EscapeTimeLimit</c>.
    /// </summary>
    [JsonPropertyName("escapeTimeLimit")]
    public double? EscapeTimeLimit { get; set; }

    /// <summary>
    ///     <c>globalTable.Configuration.Exp.MatchEnd.SurvivedSecondsRequirement</c>.
    /// </summary>
    [JsonPropertyName("survivedSecondsRequirement")]
    public required int SurvivedSecondsRequirement { get; set; }

    /// <summary>
    ///     <c>locationConfig.ScavRaidTimeSettings.Settings.TrainArrivalDelayObservedSeconds</c>.
    /// </summary>
    [JsonPropertyName("trainArrivalDelayObservedSeconds")]
    public required int TrainArrivalDelayObservedSeconds { get; set; }

    [JsonPropertyName("mapSettings")]
    public required MapSettingsState MapSettings { get; set; }

    /// <summary>
    ///     The map's exits, already filtered to <c>PassageRequirement == Train</c>: that skip draws
    ///     nothing and logs nothing, so the filtered list is the whole walk.
    /// </summary>
    [JsonPropertyName("trainExits")]
    public required List<TrainExitWire> TrainExits { get; set; }

    /// <summary>
    ///     Test-only seed for the native RNG stream; null in production.
    /// </summary>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }
}

/// <summary>
/// Three-state projection of <c>locationConfig.ScavRaidTimeSettings.Maps</c> via
/// <c>TryGetValue(location.ToLowerInvariant(), …)</c> - the lowercasing is load-bearing
/// (<c>RaidTimeAdjustmentService.cs:282</c>; shipped <c>base.json</c> Ids are mixed-case).
/// <c>Found=false</c> is the legacy <c>KeyNotFoundException</c> point; found with a null
/// <see cref="Value"/> is the warn-and-defaults branch; found with a value is the settings.
/// </summary>
public record MapSettingsState
{
    [JsonPropertyName("found")]
    public required bool Found { get; set; }

    [JsonPropertyName("value")]
    public ScavRaidTimeLocationSettingsWire? Value { get; set; }
}

/// <summary>
/// The five members of <c>ScavRaidTimeLocationSettings</c> this pass reads. <c>AdjustWaves</c> is
/// absent on purpose: it belongs to <c>MakeAdjustmentsToMap</c>, not to this one.
/// </summary>
public record ScavRaidTimeLocationSettingsWire
{
    [JsonPropertyName("reducedChancePercent")]
    public required double ReducedChancePercent { get; set; }

    /// <summary>
    ///     Insertion order defines the cumulative-weight walk, so this crosses as a JSON object and
    ///     is read back into an <c>IndexMap</c>.
    /// </summary>
    [JsonPropertyName("reductionPercentWeights")]
    public required Dictionary<string, double> ReductionPercentWeights { get; set; }

    [JsonPropertyName("reduceLootByPercent")]
    public required bool ReduceLootByPercent { get; set; }

    [JsonPropertyName("minDynamicLootPercent")]
    public required double MinDynamicLootPercent { get; set; }

    [JsonPropertyName("minStaticLootPercent")]
    public required double MinStaticLootPercent { get; set; }
}

/// <summary>
/// The five members of an <c>Exit</c> the exit-adjustment walk reads.
/// </summary>
public record TrainExitWire
{
    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("minTime")]
    public double? MinTime { get; set; }

    [JsonPropertyName("maxTime")]
    public double? MaxTime { get; set; }

    [JsonPropertyName("count")]
    public int? Count { get; set; }

    [JsonPropertyName("exfiltrationTime")]
    public double? ExfiltrationTime { get; set; }
}

public record GetRaidAdjustmentsResponse
{
    /// <summary>
    ///     Whether the raid time was actually reduced - the caller's cue to park
    ///     <see cref="RaidChanges"/> on the session. False on both default returns (a pmc raid and
    ///     a failed chance roll), which park nothing.
    /// </summary>
    [JsonPropertyName("applied")]
    public required bool Applied { get; set; }

    /// <summary>
    ///     Set on the applied path: the percent the debug line at
    ///     <c>RaidTimeAdjustmentService.cs:260</c> prints, which the clamped loot members do not
    ///     let the caller recover.
    /// </summary>
    [JsonPropertyName("chosenReductionPercent")]
    public int? ChosenReductionPercent { get; set; }

    /// <summary>
    ///     The map's settings key was present but its value was null - the caller's cue to emit the
    ///     <c>RaidTimeAdjustmentService.cs:285</c> warning.
    /// </summary>
    [JsonPropertyName("mapSettingsMissingValue")]
    public required bool MapSettingsMissingValue { get; set; }

    [JsonPropertyName("raidChanges")]
    public required RaidChanges RaidChanges { get; set; }
}

/// <summary>
/// One map-adjustment pass's inputs: the changes <c>GetRaidAdjustments</c> parked, the map members
/// the pass reads and the map's <c>AdjustWaves</c> setting.
/// </summary>
public record MakeAdjustmentsRequest
{
    /// <summary>
    ///     <c>LocationBase.Id</c>, for the missing-key error text only: the settings lookup itself
    ///     is resolved caller-side into <see cref="MapSettings"/>.
    /// </summary>
    [JsonPropertyName("mapId")]
    public string? MapId { get; set; }

    /// <summary>
    ///     The session's parked changes, as they are. A null <c>ExitChanges</c> is deliberately not
    ///     coalesced: the Rust member is a plain <c>Vec</c>, so a null fails the parse and surfaces
    ///     as an <see cref="InvalidOperationException"/> where legacy raised a
    ///     <see cref="NullReferenceException"/> - the booked divergence. It is unreachable anyway:
    ///     every path that parks a <c>RaidChanges</c> writes an empty list.
    /// </summary>
    [JsonPropertyName("raidChanges")]
    public required RaidChanges RaidChanges { get; set; }

    [JsonPropertyName("mapSettings")]
    public required MapSettingsAdjustState MapSettings { get; set; }

    /// <summary>
    ///     The exit names, in the order of the list the builder materialised out of
    ///     <c>LocationBase.Exits</c> - the list every <see cref="ExitUpdateWire.Index"/> indexes.
    /// </summary>
    [JsonPropertyName("exits")]
    public required List<string?> Exits { get; set; }

    [JsonPropertyName("waves")]
    public required List<WaveTimesWire> Waves { get; set; }

    [JsonPropertyName("bossSpawns")]
    public required List<BossSpawnWire> BossSpawns { get; set; }
}

/// <summary>
/// <see cref="MapSettingsState"/>'s three states, projected down to the one member this pass reads.
/// <c>Found=false</c> is the legacy <c>KeyNotFoundException</c> point; found with a null
/// <see cref="Value"/> is the warn-and-defaults branch, whose default <c>AdjustWaves</c> is false.
/// </summary>
public record MapSettingsAdjustState
{
    [JsonPropertyName("found")]
    public required bool Found { get; set; }

    /// <summary>
    ///     <c>ScavRaidTimeLocationSettings.AdjustWaves</c>.
    /// </summary>
    [JsonPropertyName("value")]
    public bool? Value { get; set; }
}

/// <summary>
/// The two members of a <c>Wave</c> the pass reads and writes.
/// </summary>
public record WaveTimesWire
{
    [JsonPropertyName("timeMin")]
    public int? TimeMin { get; set; }

    [JsonPropertyName("timeMax")]
    public int? TimeMax { get; set; }
}

/// <summary>
/// The two members of a <c>BossLocationSpawn</c> the pass reads.
/// </summary>
public record BossSpawnWire
{
    [JsonPropertyName("bossName")]
    public string? BossName { get; set; }

    [JsonPropertyName("time")]
    public double? Time { get; set; }
}

public record MakeAdjustmentsResponse
{
    /// <summary>
    ///     Applied unconditionally, null included - the assignment legacy makes without a guard.
    /// </summary>
    [JsonPropertyName("escapeTimeLimit")]
    public double? EscapeTimeLimit { get; set; }

    [JsonPropertyName("exitUpdates")]
    public required List<ExitUpdateWire> ExitUpdates { get; set; }

    /// <summary>
    ///     An exit change named an exit the map does not have, which returns out of the whole
    ///     legacy method: the applier lands the updates emitted so far, logs
    ///     <see cref="AbortedExitName"/> and stops. Authoritative on its own - the name is log
    ///     payload, and a null one is a legitimate name to have failed to match.
    /// </summary>
    [JsonPropertyName("aborted")]
    public required bool Aborted { get; set; }

    [JsonPropertyName("abortedExitName")]
    public string? AbortedExitName { get; set; }

    /// <summary>
    ///     The map's settings key was present but its value was null - the caller's cue to emit the
    ///     warning. Never set on an aborted run: the abort precedes the resolve.
    /// </summary>
    [JsonPropertyName("mapSettingsMissingValue")]
    public required bool MapSettingsMissingValue { get; set; }

    /// <summary>
    ///     Null when the map's <c>AdjustWaves</c> is off, or when the run aborted.
    /// </summary>
    [JsonPropertyName("waveAdjustments")]
    public WaveAdjustmentsWire? WaveAdjustments { get; set; }
}

/// <summary>
/// One exit's changed members. A null member means the change carried none, so the live exit keeps
/// its own value - it never means "null it".
/// </summary>
public record ExitUpdateWire
{
    /// <summary>
    ///     Into the builder-materialised exit list, not into <c>LocationBase.Exits</c> re-enumerated.
    /// </summary>
    [JsonPropertyName("index")]
    public required int Index { get; set; }

    [JsonPropertyName("chance")]
    public double? Chance { get; set; }

    [JsonPropertyName("minTime")]
    public double? MinTime { get; set; }

    [JsonPropertyName("maxTime")]
    public double? MaxTime { get; set; }
}

public record WaveAdjustmentsWire
{
    /// <summary>
    ///     The request <c>waves</c> entries that survive, in map order.
    /// </summary>
    [JsonPropertyName("waveKeepIndices")]
    public required List<int> WaveKeepIndices { get; set; }

    /// <summary>
    ///     Final absolute times per kept wave, in keep order - so parallel to
    ///     <see cref="WaveKeepIndices"/>, with the double subtraction already applied.
    /// </summary>
    [JsonPropertyName("waveTimes")]
    public required List<WaveTimesWire> WaveTimes { get; set; }

    /// <summary>
    ///     The request <c>bossSpawns</c> entries that survive, in map order.
    /// </summary>
    [JsonPropertyName("bossKeepIndices")]
    public required List<int> BossKeepIndices { get; set; }

    [JsonPropertyName("bossTimeUpdates")]
    public required List<BossTimeUpdateWire> BossTimeUpdates { get; set; }

    /// <summary>
    ///     The offset that was subtracted, for the debug line alone; null when no PMC spawn
    ///     survived, which is what silences that line.
    /// </summary>
    [JsonPropertyName("pmcStartSeconds")]
    public double? PmcStartSeconds { get; set; }

    [JsonPropertyName("removedWaveCount")]
    public required int RemovedWaveCount { get; set; }

    [JsonPropertyName("removedBossCount")]
    public required int RemovedBossCount { get; set; }
}

/// <summary>
/// One boss spawn's new time.
/// </summary>
public record BossTimeUpdateWire
{
    /// <summary>
    ///     Into the request's <c>bossSpawns</c> - the <em>original</em> spawn list, not the kept
    ///     one, so the write lands on the very object legacy wrote (which on a map that took the
    ///     custom-PMC splice belongs to the live <c>PmcConfig</c>). The absolute time leans on each
    ///     instance appearing at one index: legacy's read-modify-write compounds on an instance a
    ///     mod spliced in twice, where this write is last-write-wins - the booked aliasing
    ///     divergence in the roadmap ledger.
    /// </summary>
    [JsonPropertyName("index")]
    public required int Index { get; set; }

    [JsonPropertyName("time")]
    public required double Time { get; set; }
}

/// <summary>
/// One raid-start hostility pass's inputs: the config's changes and the map's own settings, both
/// projected down to what the matching and op selection read.
/// </summary>
public record AdjustHostilityRequest
{
    /// <summary>
    ///     <c>pmcConfig.HostilitySettings</c>, in its own insertion order - the order the legacy
    ///     <c>foreach</c> walks it in, which the native side reads back into an <c>IndexMap</c>.
    /// </summary>
    [JsonPropertyName("hostilitySettings")]
    public required Dictionary<string, HostilityConfigWire> HostilitySettings { get; set; }

    /// <summary>
    ///     The map's <c>AdditionalHostilitySettings</c>, in the order of the list the builder
    ///     materialised out of it - the list every <see cref="HostilityEntryWire.MatchedIndex"/>
    ///     indexes. Null mirrors a null member: every role then reports unmatched, which is what
    ///     legacy's <c>?.FirstOrDefault</c> does.
    /// </summary>
    [JsonPropertyName("locationSettings")]
    public List<LocationHostilityWire>? LocationSettings { get; set; }
}

/// <summary>
/// The two things the pass reads off one <c>AdditionalHostilitySettings</c> entry: the role it
/// matches on, and whether the entry can take an enemy type at all.
/// </summary>
public record LocationHostilityWire
{
    [JsonPropertyName("botRole")]
    public string? BotRole { get; set; }

    /// <summary>
    ///     <c>AlwaysEnemies</c> is a nullable <c>HashSet</c>, so a null one NREs on the first add -
    ///     the native side reports that instead of the applier reaching it.
    /// </summary>
    [JsonPropertyName("alwaysEnemiesIsNull")]
    public required bool AlwaysEnemiesIsNull { get; set; }
}

/// <summary>
/// One config role's changes.
/// </summary>
public record HostilityConfigWire
{
    [JsonPropertyName("additionalEnemyTypes")]
    public List<string>? AdditionalEnemyTypes { get; set; }

    /// <summary>
    ///     <c>ChancedEnemies is not null</c> - a pure null check, so a non-null <em>empty</em> list
    ///     is still true and still has the applier clear the location's list. The list itself never
    ///     crosses: the applier reads it off the live config, because the loop's merge branch writes
    ///     through a live reference.
    /// </summary>
    [JsonPropertyName("hasChancedEnemies")]
    public required bool HasChancedEnemies { get; set; }

    [JsonPropertyName("additionalFriendlyTypes")]
    public List<string>? AdditionalFriendlyTypes { get; set; }

    [JsonPropertyName("bearEnemyChance")]
    public double? BearEnemyChance { get; set; }

    [JsonPropertyName("usecEnemyChance")]
    public double? UsecEnemyChance { get; set; }

    [JsonPropertyName("savageEnemyChance")]
    public double? SavageEnemyChance { get; set; }

    [JsonPropertyName("savagePlayerBehaviour")]
    public string? SavagePlayerBehaviour { get; set; }
}

public record AdjustHostilityResponse
{
    /// <summary>
    ///     One entry per config role, in config insertion order. The applier walks this single list,
    ///     which is what preserves legacy's warn/apply interleaving inside its one loop.
    /// </summary>
    [JsonPropertyName("entries")]
    public required List<HostilityEntryWire> Entries { get; set; }
}

/// <summary>
/// One config role's ops, and which location entry to run them on.
/// </summary>
public record HostilityEntryWire
{
    [JsonPropertyName("role")]
    public required string Role { get; set; }

    /// <summary>
    ///     Into the builder-materialised <c>AdditionalHostilitySettings</c> list. Null is the
    ///     unmatched role: the applier warns and skips.
    /// </summary>
    [JsonPropertyName("matchedIndex")]
    public int? MatchedIndex { get; set; }

    [JsonPropertyName("addAlwaysEnemies")]
    public required List<string> AddAlwaysEnemies { get; set; }

    /// <summary>
    ///     Run the legacy clear-and-refill loop from the live config's list for this role.
    /// </summary>
    [JsonPropertyName("runChancedEnemiesLoop")]
    public required bool RunChancedEnemiesLoop { get; set; }

    /// <summary>
    ///     Non-null - <em>empty included</em> - resets <c>AlwaysFriends</c> before the fill, so an
    ///     empty list is a clear. Null is the whole branch legacy skips.
    /// </summary>
    [JsonPropertyName("setAlwaysFriends")]
    public List<string>? SetAlwaysFriends { get; set; }

    [JsonPropertyName("bearEnemyChance")]
    public double? BearEnemyChance { get; set; }

    [JsonPropertyName("usecEnemyChance")]
    public double? UsecEnemyChance { get; set; }

    [JsonPropertyName("savageEnemyChance")]
    public double? SavageEnemyChance { get; set; }

    [JsonPropertyName("savagePlayerBehaviour")]
    public string? SavagePlayerBehaviour { get; set; }
}

/// <summary>
/// One extract pass's inputs. The map name is absent on purpose: nothing native reads it, and the
/// warning that names it is the applier's.
/// </summary>
public record AdjustExtractsRequest
{
    [JsonPropertyName("playerSide")]
    public string? PlayerSide { get; set; }

    /// <summary>
    ///     Whether the map has extract data at all - the location lookup and its <c>AllExtracts</c>
    ///     resolved caller-side, both of which legacy folds into one null check.
    /// </summary>
    [JsonPropertyName("mapFound")]
    public required bool MapFound { get; set; }

    /// <summary>
    ///     Each extract's <c>Side</c>, in the order of the list the builder materialised out of
    ///     <c>AllExtracts</c> - the list every appended index indexes.
    /// </summary>
    [JsonPropertyName("extractSides")]
    public required List<string?> ExtractSides { get; set; }
}

public record AdjustExtractsResponse
{
    /// <summary>
    ///     The map has no extract data - the applier's cue to emit the warning and stop.
    /// </summary>
    [JsonPropertyName("warnUnknownMap")]
    public required bool WarnUnknownMap { get; set; }

    /// <summary>
    ///     Empty means no append at all, which covers both the non-scav early return and a map with
    ///     no scav extracts.
    /// </summary>
    [JsonPropertyName("appendExtractIndices")]
    public required List<int> AppendExtractIndices { get; set; }
}
