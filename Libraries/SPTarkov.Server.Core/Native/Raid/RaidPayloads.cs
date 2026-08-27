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
/// The one type this family does not mirror is the result itself: the response's
/// <see cref="GetRaidAdjustmentsResponse.RaidChanges"/> is the real
/// <see cref="Models.Spt.Location.RaidChanges"/>, so a decoded response lands directly on the
/// record the session parks and the HTTP response carries. Its
/// <see cref="Models.Spt.Location.ExtractChange"/> members are PascalCase on the wire, which the
/// Rust mirror spells out.
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
