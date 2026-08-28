using System.Text.Json.Serialization;

namespace SPTarkov.Server.Core.Native.Achievements;

/// <summary>
/// The profile projection <c>spt_get_achievement_statistics</c> reads, mirroring
/// <c>rust/spt-native/src/achievements.rs</c> member for member. Conventions are
/// <c>Native/Raid/RaidPayloads.cs</c>'s: an explicit <see cref="JsonPropertyNameAttribute"/> on
/// every member, and everything Rust declares as non-<c>Option</c> <c>required</c>.
///
/// The <c>ProfileHelper.GetProfiles()</c> call and the <c>AchievementProfileIdBlacklist</c> filter
/// stay C#-side; what crosses is what is left of them.
/// </summary>
public record AchievementStatisticsRequest
{
    /// <summary>
    ///     The achievement table's ids, in table order - which is the order the response is keyed
    ///     in. Unfiltered: an empty id crosses as <c>""</c> and the native side skips it, which is
    ///     legacy's <c>IsNullOrEmpty</c> filter moved to the far side of the wire. A null would be
    ///     rejected outright, which is why the builder projects through <c>MongoId.ToString()</c>.
    /// </summary>
    [JsonPropertyName("achievementIds")]
    public required List<string> AchievementIds { get; set; }

    /// <summary>
    ///     The denominator: every non-blacklisted profile, the ones with no achievements dictionary
    ///     included - those count here and ship no set.
    /// </summary>
    [JsonPropertyName("profileCount")]
    public required int ProfileCount { get; set; }

    /// <summary>
    ///     One key set per profile that has an achievements dictionary, so a set count below
    ///     <see cref="ProfileCount"/> is the ordinary shape rather than a projection bug.
    /// </summary>
    [JsonPropertyName("completedSets")]
    public required List<List<string>> CompletedSets { get; set; }
}

/// <summary>
/// One <c>CompletedAchievementsResponse</c>'s worth of percentages.
/// </summary>
public record AchievementStatisticsResponse
{
    /// <summary>
    ///     Percentage per achievement id, in achievement order. The order is observable JSON on the
    ///     way to the client, and <c>System.Text.Json</c> fills a <see cref="Dictionary{TKey,TValue}"/>
    ///     in the order it read the wire - so the decoded map re-serializes in the order the native
    ///     side wrote it.
    /// </summary>
    [JsonPropertyName("elements")]
    public required Dictionary<string, int> Elements { get; set; }
}
