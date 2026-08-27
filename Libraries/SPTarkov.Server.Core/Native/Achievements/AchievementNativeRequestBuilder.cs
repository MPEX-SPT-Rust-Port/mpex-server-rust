using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Profile;

namespace SPTarkov.Server.Core.Native.Achievements;

/// <summary>
/// Assembles the achievement statistics request out of the achievement table and the profiles the
/// controller already filtered - everything <c>AchievementController.GetAchievementStatics</c>
/// would have walked for itself - and sends it.
///
/// It owns no frozen member set: this port moves no hookable member. The whole legacy body is
/// inline in <c>GetAchievementStatics</c>, which is the dispatcher, so a patch there wraps
/// whichever arm runs.
/// </summary>
[Injectable]
public class AchievementNativeRequestBuilder
{
    /// <summary>
    ///     One statistics pass's inputs: the achievement table's ids in table order, the count of
    ///     profiles that survived the blacklist, and one completed-id set per profile that has an
    ///     achievements dictionary at all.
    /// </summary>
    /// <param name="achievements">The achievement table, in table order</param>
    /// <param name="profiles">The non-blacklisted profiles, already filtered</param>
    public AchievementStatisticsRequest BuildStatisticsRequest(IEnumerable<Achievement> achievements, ICollection<SptProfile> profiles)
    {
        return new AchievementStatisticsRequest
        {
            // Ids cross unfiltered - the empty-id skip is native-side, matching legacy's Where.
            // Achievement.Id is a required MongoId, a non-nullable struct, so ToString() is total:
            // it yields string.Empty for the empty id, which is exactly what legacy's
            // IsNullOrEmpty test over the implicit MongoId-to-string operator saw. A null would be
            // rejected by the native Vec<String> instead of skipped
            AchievementIds = achievements.Select(achievement => achievement.Id.ToString()).ToList(),
            ProfileCount = profiles.Count,
            CompletedSets = profiles
                .Where(profile => profile.CharacterData?.PmcData?.Achievements is not null)
                .Select(profile => profile.CharacterData!.PmcData!.Achievements!.Keys.Select(key => key.ToString()).ToList())
                .ToList(),
        };
    }

    /// <summary>
    ///     Counts one achievement table's completion percentages natively.
    /// </summary>
    /// <exception cref="InvalidOperationException">The pass failed, or the native side misbehaved.</exception>
    public AchievementStatisticsResponse SendStatistics(AchievementStatisticsRequest request)
    {
        return SptNative.GetAchievementStatistics(request);
    }
}
