using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Profile;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Achievements;

namespace SPTarkov.Server.Core.Controllers;

/// <summary>
/// The achievement completion percentages are counted in <c>rust/spt-native</c> by default;
/// <see cref="AchievementNativeRequestBuilder"/> projects the achievement table and the profiles
/// that survived the blacklist into the native payload. The full C# loop is retained below as the
/// legacy path - it is the frozen mod contract - and runs instead of the native path when a mod
/// substituted the controller, when the frozen constructor built the instance or when
/// <see cref="CoreConfig.ForceLegacyAchievementStatistics"/> is set.
///
/// There is no frozen member set to consult: the whole legacy body is inline in
/// <see cref="GetAchievementStatics"/>, which is the dispatcher, so a Harmony patch there wraps
/// whichever arm runs rather than needing one to decline.
/// </summary>
[Injectable]
public class AchievementController(TemplateTable templateTable, ProfileHelper profileHelper, CoreConfig coreConfig)
{
    private readonly AchievementNativeRequestBuilder? _requestBuilder;

    /// <summary>
    ///     The frozen constructor plus the native request builder. Additive and apicompat-verified.
    /// </summary>
    public AchievementController(
        TemplateTable templateTable,
        ProfileHelper profileHelper,
        CoreConfig coreConfig,
        AchievementNativeRequestBuilder requestBuilder
    )
        : this(templateTable, profileHelper, coreConfig)
    {
        _requestBuilder = requestBuilder;
    }

    /// <summary>
    ///     Which implementation the most recent statistics call ran - the spt-native path or the
    ///     retained C# path. Test seam; also handy in a debugger. Unsynchronized - concurrent
    ///     requests race it - which only the non-parallel fixtures that assert on it may ignore.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     The legacy path runs when the frozen constructor built this instance (it has no native
    ///     seam to dispatch to), when forced by config, or when a mod has substituted the
    ///     controller itself. No patch check: this port moves no hookable member.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_requestBuilder is null || coreConfig.ForceLegacyAchievementStatistics)
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return GetType() != typeof(AchievementController);
    }

    /// <summary>
    ///     Get base achievements
    /// </summary>
    /// <param name="sessionID">Session/player id</param>
    /// <returns></returns>
    public GetAchievementsResponse GetAchievements(MongoId sessionID)
    {
        return new GetAchievementsResponse { Elements = templateTable.Achievements };
    }

    /// <summary>
    ///     Shows % of 'other' players who've completed each achievement
    /// </summary>
    /// <param name="sessionId">Session/Player id</param>
    /// <returns>CompletedAchievementsResponse</returns>
    public CompletedAchievementsResponse GetAchievementStatics(MongoId sessionId)
    {
        if (!UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Native;

            var nonBlacklisted = profileHelper
                .GetProfiles()
                .Where(kvp => !coreConfig.Features.AchievementProfileIdBlacklist.Contains(kvp.Value.ProfileInfo.ProfileId))
                .Select(kvp => kvp.Value)
                .ToList();

            var request = _requestBuilder!.BuildStatisticsRequest(templateTable.Achievements, nonBlacklisted);
            var response = _requestBuilder.SendStatistics(request);

            return new CompletedAchievementsResponse { Elements = response.Elements };
        }

        LastPathTaken = LootGenerationPath.Legacy;

        var stats = new Dictionary<string, int>();
        var profiles = profileHelper
            .GetProfiles()
            .Where(kvp => !coreConfig.Features.AchievementProfileIdBlacklist.Contains(kvp.Value.ProfileInfo.ProfileId))
            .ToDictionary();

        var achievements = templateTable.Achievements;
        foreach (
            var achievementId in achievements
                .Select(achievement => achievement.Id)
                .Where(achievementId => !string.IsNullOrEmpty(achievementId))
        )
        {
            var profilesHaveAchievement = 0;
            foreach (var (_, profile) in profiles)
            {
                if (profile.CharacterData?.PmcData?.Achievements is null)
                {
                    continue;
                }

                if (!profile.CharacterData.PmcData.Achievements.ContainsKey(achievementId))
                {
                    continue;
                }

                profilesHaveAchievement++;
            }

            var percentage = 0;
            if (profiles.Count > 0)
            {
                percentage = (int)Math.Round((double)profilesHaveAchievement / profiles.Count * 100);
            }

            stats.Add(achievementId, percentage);
        }

        return new CompletedAchievementsResponse { Elements = stats };
    }
}
