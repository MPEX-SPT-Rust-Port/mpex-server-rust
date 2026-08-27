using NUnit.Framework;
using SPTarkov.Server.Core.Controllers;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Profile;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;

namespace UnitTests.Tests.Controllers;

/// <summary>
/// Golden parity gate on the achievement statistics port: the legacy C# loop and the spt-native
/// pass must produce the same <see cref="CompletedAchievementsResponse.Elements"/> - the same
/// percentages in the same key order, which is observable JSON on the way to the client.
///
/// State this fixture mutates, all of it restored in <see cref="OneTimeTearDown"/>:
/// <see cref="CoreConfig.ForceLegacyAchievementStatistics"/> on the shared config singleton - the
/// path selector - the <c>AchievementProfileIdBlacklist</c> set, and the achievement table and
/// profile store it seeds. Every profile another fixture left in <see cref="SaveServer"/> is
/// blacklisted for the duration, because an unknown denominator would leave every percentage here
/// unpinnable; the seeded set is then exactly eight counted profiles plus one blacklisted.
///
/// Nothing here reads resident database state - the export names no epoch and the whole projection
/// crosses in the request - so the achievement rows this fixture appends need no mutation stamp
/// bump.
/// </summary>
[TestFixture]
[NonParallelizable]
public class AchievementParityTests
{
    /// <summary>
    /// Held by two of the eight counted profiles: 2/8 = 25%.
    /// </summary>
    private static readonly MongoId _sharedAchievement = new();

    /// <summary>
    /// Held by one counted profile each: 1/8 = 12.5%, the banker's-rounding pin.
    /// </summary>
    private static readonly MongoId _firstOnlyAchievement = new();

    private static readonly MongoId _secondOnlyAchievement = new();

    /// <summary>
    /// Held only by the blacklisted profile, so it stays at 0% - the numerator half of the
    /// blacklist check.
    /// </summary>
    private static readonly MongoId _blacklistedOnlyAchievement = new();

    private readonly MongoId _sessionId = new();

    /// <summary>
    /// The achievement rows appended to the live table, removed by reference in the teardown.
    /// </summary>
    private readonly List<Achievement> _injectedAchievements = [];

    private readonly List<MongoId> _seededProfiles = [];

    private AchievementController _controller = default!;
    private CoreConfig _coreConfig = default!;
    private TemplateTable _templateTable = default!;
    private SaveServer _saveServer = default!;
    private List<string> _originalBlacklist = [];
    private bool _originalForceLegacy;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _controller = di.GetService<AchievementController>();
        _coreConfig = di.GetService<CoreConfig>();
        _templateTable = di.GetService<TemplateTable>();
        _saveServer = di.GetService<SaveServer>();

        _originalForceLegacy = _coreConfig.ForceLegacyAchievementStatistics;

        // Whatever another fixture left in the profile store would move the denominator, and the
        // fixture order is not ours to pick - so those profiles are blacklisted for the duration
        // through the very mechanism this fixture pins, and the set is put back whole afterwards
        var blacklist = _coreConfig.Features.AchievementProfileIdBlacklist!;
        _originalBlacklist = [.. blacklist];
        foreach (var profileId in _saveServer.GetProfiles().Keys)
        {
            blacklist.Add(profileId);
        }

        _injectedAchievements.AddRange([
            TestAchievement(_sharedAchievement),
            TestAchievement(_firstOnlyAchievement),
            // The empty id both arms skip - legacy's IsNullOrEmpty filter, and the native
            // side's own empty-string skip on the other side of the wire
            TestAchievement(MongoId.Empty()),
            TestAchievement(_secondOnlyAchievement),
            TestAchievement(_blacklistedOnlyAchievement),
        ]);
        _templateTable.Achievements.AddRange(_injectedAchievements);

        SeedProfile(new Dictionary<MongoId, long> { { _sharedAchievement, 1 }, { _firstOnlyAchievement, 1 } });
        SeedProfile(new Dictionary<MongoId, long> { { _sharedAchievement, 1 }, { _secondOnlyAchievement, 1 } });

        // The profile with no achievements dictionary at all: legacy skips it, and it ships no set
        // natively - but it counts in the denominator on both arms
        SeedProfile(null);

        for (var i = 0; i < 5; i++)
        {
            SeedProfile([]);
        }

        // Completes everything and is blacklisted: it must move neither numerator nor denominator
        blacklist.Add(
            SeedProfile(
                new Dictionary<MongoId, long>
                {
                    { _sharedAchievement, 1 },
                    { _firstOnlyAchievement, 1 },
                    { _secondOnlyAchievement, 1 },
                    { _blacklistedOnlyAchievement, 1 },
                }
            )
        );
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        // The setup may have thrown before it got this far, and putting nothing back is better
        // than an NRE that hides why
        if (_coreConfig is null)
        {
            return;
        }

        _coreConfig.ForceLegacyAchievementStatistics = _originalForceLegacy;

        var blacklist = _coreConfig.Features.AchievementProfileIdBlacklist;
        if (blacklist is not null)
        {
            blacklist.Clear();
            foreach (var entry in _originalBlacklist)
            {
                blacklist.Add(entry);
            }
        }

        foreach (var achievement in _injectedAchievements)
        {
            _templateTable.Achievements.Remove(achievement);
        }

        foreach (var profileId in _seededProfiles)
        {
            _saveServer.DeleteProfileById(profileId);
        }
    }

    [TearDown]
    public void TearDown()
    {
        _coreConfig.ForceLegacyAchievementStatistics = _originalForceLegacy;
    }

    /// <summary>
    /// The parity gate itself: same percentages, same keys, same order. The order is what the
    /// client sees, so it is asserted as a sequence rather than as a set.
    /// </summary>
    [Test]
    public void BothArmsProduceTheSameStatisticsInTheSameKeyOrder()
    {
        var native = Statistics(forceLegacy: false, LootGenerationPath.Native);
        var legacy = Statistics(forceLegacy: true, LootGenerationPath.Legacy);

        Assert.That(native.Elements, Is.Not.Null, "the native arm returned no elements");
        Assert.That(legacy.Elements, Is.Not.Null, "the legacy arm returned no elements");
        Assert.That(
            native.Elements!.ToList(),
            Is.EqualTo(legacy.Elements!.ToList()),
            "the two arms disagree on the statistics or their order"
        );
    }

    /// <summary>
    /// What the seeded profiles are worth, arm by arm: the blacklist holds on both sides of the
    /// numerator and the denominator, the empty id is skipped, and 12.5 rounds to even.
    /// </summary>
    [Test]
    public void EachArmCountsOnlyTheNonBlacklistedProfiles()
    {
        AssertSeededPercentages(Statistics(forceLegacy: false, LootGenerationPath.Native), "native");
        AssertSeededPercentages(Statistics(forceLegacy: true, LootGenerationPath.Legacy), "legacy");
    }

    /// <summary>
    /// The booked divergence, end to end: legacy's <c>stats.Add</c> throws
    /// <see cref="ArgumentException"/> on a duplicate id, and the native pass - which cannot throw
    /// a C# exception - reports it as an error message that crosses in the failure envelope and
    /// surfaces as <see cref="InvalidOperationException"/>. One test, because it is also the only
    /// exercise the error envelope gets on this export.
    /// </summary>
    [Test]
    public void ADuplicateAchievementIdThrowsOnBothArms()
    {
        var duplicate = TestAchievement(_sharedAchievement);
        _templateTable.Achievements.Add(duplicate);

        try
        {
            _coreConfig.ForceLegacyAchievementStatistics = true;
            Assert.That(() => _controller.GetAchievementStatics(_sessionId), Throws.TypeOf<ArgumentException>());
            Assert.That(_controller.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy), "the throwing legacy call took the wrong path");

            _coreConfig.ForceLegacyAchievementStatistics = false;
            Assert.That(
                () => _controller.GetAchievementStatics(_sessionId),
                Throws
                    .TypeOf<InvalidOperationException>()
                    .With.Message.Contains($"duplicate achievement id: {_sharedAchievement.ToString()}"),
                "the native failure did not carry the native message"
            );
            Assert.That(_controller.LastPathTaken, Is.EqualTo(LootGenerationPath.Native), "the throwing native call took the wrong path");
        }
        finally
        {
            _templateTable.Achievements.Remove(duplicate);
        }
    }

    private void AssertSeededPercentages(CompletedAchievementsResponse response, string arm)
    {
        var elements = response.Elements!;

        // 2 of the 8 counted profiles
        Assert.That(elements[_sharedAchievement.ToString()], Is.EqualTo(25), $"{arm}: the shared achievement");

        // 1 of 8 is 12.5, and (int)Math.Round is banker's rounding - 12, never 13
        Assert.That(elements[_firstOnlyAchievement.ToString()], Is.EqualTo(12), $"{arm}: 12.5 did not round to even");
        Assert.That(elements[_secondOnlyAchievement.ToString()], Is.EqualTo(12), $"{arm}: 12.5 did not round to even");

        // Only the blacklisted profile holds it, so it stays at 0 rather than 1 of 9 - and the
        // three above stay at 25 and 12 rather than 22 and 11, which is the denominator half
        Assert.That(elements[_blacklistedOnlyAchievement.ToString()], Is.EqualTo(0), $"{arm}: the blacklisted profile moved the numerator");

        Assert.That(elements.ContainsKey(string.Empty), Is.False, $"{arm}: the empty achievement id was not skipped");
    }

    /// <summary>
    /// One statistics call on the named arm, with the path it took asserted before its output is
    /// used for anything.
    /// </summary>
    private CompletedAchievementsResponse Statistics(bool forceLegacy, LootGenerationPath expected)
    {
        _coreConfig.ForceLegacyAchievementStatistics = forceLegacy;

        var response = _controller.GetAchievementStatics(_sessionId);

        Assert.That(_controller.LastPathTaken, Is.EqualTo(expected), $"forceLegacy={forceLegacy} took the wrong path");

        return response;
    }

    /// <summary>
    /// One profile in the shared store, remembered so the teardown can take it back out again. A
    /// null dictionary is the profile legacy skips outright.
    /// </summary>
    private MongoId SeedProfile(Dictionary<MongoId, long>? achievements)
    {
        var profileId = new MongoId();

        _saveServer.AddProfile(
            new SptProfile
            {
                // Fully qualified: Models.Eft.Common.Tables has an Info of its own
                ProfileInfo = new SPTarkov.Server.Core.Models.Eft.Profile.Info { ProfileId = profileId },
                CharacterData = new Characters { PmcData = new PmcData { Achievements = achievements } },
            }
        );

        _seededProfiles.Add(profileId);

        return profileId;
    }

    /// <summary>
    /// The thinnest achievement the counting loop will accept: only its id is ever read.
    /// </summary>
    private static Achievement TestAchievement(MongoId id)
    {
        return new Achievement
        {
            Index = 0,
            Id = id,
            ImageUrl = string.Empty,
            Rewards = [],
            Conditions = new AchievementQuestConditionTypes(),
            Rarity = string.Empty,
            Hidden = false,
            ShowConditions = false,
            ProgressBarEnabled = false,
            Side = string.Empty,
        };
    }
}
