using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.RepeatableQuests;
using SPTarkov.Server.Core.Helpers.Quest;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Repeatable;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the mod hook contract across all six classes the native repeatable-quest path replaces: a
/// Harmony patch on a frozen 4.1.2 member of any of them must route generation to the legacy path,
/// because that is the only body the patch can hook. A patch on a <c>Generate</c> dispatcher is the
/// exception, it wraps whichever path runs. Harmony patches are process-wide, so every patch is
/// removed in a finally and the fixture never runs in parallel with others.
///
/// One generator is the vehicle: all four share the same six-type hookable set, so a patch anywhere
/// in it flips every one of them.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RepeatableQuestHookLivenessTests
{
    /// <summary>
    /// The six classes the native path folds in - the order the generators declare them in.
    /// </summary>
    private static readonly Type[] _frozenTypes =
    [
        typeof(EliminationQuestGenerator),
        typeof(CompletionQuestGenerator),
        typeof(ExplorationQuestGenerator),
        typeof(PickupQuestGenerator),
        typeof(RepeatableQuestRewardGenerator),
        typeof(RepeatableQuestHelper),
    ];

    private static readonly MongoId _sessionId = new("6193a720f8ee7e52e4290000");

    private static bool _patchFired;
    private static bool _prefixFired;
    private static bool _postfixFired;

    private ExplorationQuestGenerator _explorationQuestGenerator = default!;
    private QuestConfig _questConfig = default!;

    private RepeatableQuestConfig _repeatableConfig = default!;
    private MongoId _traderId;
    private int _pmcLevel;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _explorationQuestGenerator = di.GetService<ExplorationQuestGenerator>();
        _questConfig = di.GetService<QuestConfig>();

        _repeatableConfig = _questConfig.RepeatableQuests.First(config => config.Side == PlayerGroup.Pmc);
        _traderId = _repeatableConfig.TraderWhitelist.First(whitelist => whitelist.QuestTypes.Contains("Exploration")).TraderId;

        // The midpoint of the second shipped exploration band, so the level tracks the data rather
        // than an edge this fixture invents
        var band = _repeatableConfig.QuestConfig.ExplorationConfig[1].LevelRange;
        _pmcLevel = (band.Min + band.Max) / 2;

        _explorationQuestGenerator.NativeTestSeed = 424242;
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _explorationQuestGenerator.NativeTestSeed = null;
    }

    /// <summary>
    /// Nothing the exploration pass calls, on a generator it never touches - it is here to prove the
    /// hookable set was not narrowed to the running dispatcher's callees.
    /// </summary>
    [Test]
    public void HarmonyPatchOnEliminationQuestGeneratorForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(EliminationQuestGenerator), "GetGenerationData", expectFired: false);
    }

    /// <inheritdoc cref="HarmonyPatchOnEliminationQuestGeneratorForcesTheLegacyPath"/>
    [Test]
    public void HarmonyPatchOnCompletionQuestGeneratorForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(CompletionQuestGenerator), "GetItemsToRetrievePool", expectFired: false);
    }

    [Test]
    public void HarmonyPatchOnExplorationQuestGeneratorFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(typeof(ExplorationQuestGenerator), "TryGetLocationInfo", expectFired: true);
    }

    [Test]
    public void HarmonyPatchOnRepeatableQuestRewardGeneratorFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(
            typeof(RepeatableQuestRewardGenerator),
            nameof(RepeatableQuestRewardGenerator.GenerateReward),
            expectFired: true
        );
    }

    [Test]
    public void HarmonyPatchOnRepeatableQuestHelperFiresAndForcesTheLegacyPath()
    {
        AssertPatchForcesLegacyPath(
            typeof(RepeatableQuestHelper),
            nameof(RepeatableQuestHelper.GenerateRepeatableTemplate),
            expectFired: true
        );
    }

    /// <summary>
    /// The dispatchers are deliberately not in the hookable set: a patch on one wraps whichever path
    /// runs, so it keeps the native body and still sees the call.
    /// </summary>
    [Test]
    public void HarmonyPatchOnGenerateWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.repeatable-quest-hook-liveness.dispatcher");
        var target = AccessTools.Method(typeof(ExplorationQuestGenerator), nameof(ExplorationQuestGenerator.Generate));
        Assert.That(target, Is.Not.Null, "Generate not found");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(RepeatableQuestHookLivenessTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(RepeatableQuestHookLivenessTests), nameof(Postfix))
            );

            var quest = Generate();

            Assert.That(
                _explorationQuestGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Native),
                "a patch on the dispatcher forced legacy"
            );
            Assert.That(quest, Is.Not.Null);
            Assert.That(_prefixFired, Is.True, "prefix on Generate never ran");
            Assert.That(_postfixFired, Is.True, "postfix on Generate never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The hookable set is built by reflection and the dispatchers are excluded by name, so a member
    /// added to any of the six classes under the name <c>Generate</c> - on the helper, say, which has
    /// no dispatcher - would silently fall out of the scan and become unhookable. Recomputing the
    /// frozen surface here and subtracting the four dispatchers by interface-map identity rather than
    /// by name is what makes that a failure: it pins the set's exact contents, not just its shape.
    ///
    /// All four generators carry their own copy of the list, so all four are checked - the copies
    /// diverging is the other way this can rot.
    /// </summary>
    [Test]
    public void EveryGeneratorsHookableSetIsTheFrozenSurfaceMinusTheFourDispatchers()
    {
        var dispatchers = _frozenTypes
            .Where(type => type.IsAssignableTo(typeof(IRepeatableQuestGenerator)))
            .Select(type => type.GetInterfaceMap(typeof(IRepeatableQuestGenerator)).TargetMethods.Single())
            .ToList();

        Assert.That(dispatchers, Has.Count.EqualTo(4), "the four quest generators no longer implement one dispatcher each");

        var expected = FrozenSurface().Except(dispatchers).ToList();
        Assert.That(expected, Is.Not.Empty);

        Assert.Multiple(() =>
        {
            foreach (var type in _frozenTypes.Where(type => type.IsAssignableTo(typeof(IRepeatableQuestGenerator))))
            {
                var members =
                    (List<MethodBase>)type.GetField("_hookableMembers", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

                Assert.That(
                    members,
                    Is.EquivalentTo(expected),
                    $"{type.Name}'s hookable set is not the frozen surface minus the dispatchers"
                );
            }
        });

        // PickupQuestGenerator contributes nothing: its whole legacy body is inline in the dispatcher,
        // so it has no frozen member to hook. Stated here so a future drop is not mistaken for it
        Assert.That(expected.Any(member => member.DeclaringType == typeof(PickupQuestGenerator)), Is.False);
    }

    /// <summary>
    /// The public, protected and protected-internal methods declared across the six classes - the
    /// surface the apicompat gate freezes, recomputed independently of the generators' own scan.
    /// </summary>
    private static List<MethodInfo> FrozenSurface()
    {
        return
        [
            .. _frozenTypes
                .SelectMany(type =>
                    type.GetMethods(
                        BindingFlags.Instance
                            | BindingFlags.Static
                            | BindingFlags.Public
                            | BindingFlags.NonPublic
                            | BindingFlags.DeclaredOnly
                    )
                )
                .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly)),
        ];
    }

    private void AssertPatchForcesLegacyPath(Type declaringType, string methodName, bool expectFired)
    {
        var harmony = new Harmony($"unit-tests.repeatable-quest-hook-liveness.{declaringType.Name}.{methodName}");
        var target = AccessTools.Method(declaringType, methodName);
        Assert.That(target, Is.Not.Null, $"frozen member {declaringType.Name}.{methodName} not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(RepeatableQuestHookLivenessTests), nameof(PatchFired)));

            // Members no pass reaches can never set _patchFired, so this is what proves the patch is
            // actually live rather than a silently failed install
            Assert.That(
                Harmony.GetPatchInfo(target)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {declaringType.Name}.{methodName} was not registered"
            );

            var quest = Generate();

            Assert.That(
                _explorationQuestGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Legacy),
                $"a patch on {declaringType.Name}.{methodName} did not force the legacy path"
            );
            Assert.That(quest, Is.Not.Null, "the legacy path produced no quest");

            if (expectFired)
            {
                Assert.That(_patchFired, Is.True, $"postfix on {declaringType.Name}.{methodName} never ran on the legacy path");
            }
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    private RepeatableQuest? Generate()
    {
        return _explorationQuestGenerator.Generate(_sessionId, _pmcLevel, _traderId, BuildPool(), _repeatableConfig);
    }

    /// <summary>
    /// <c>RepeatableQuestController.GenerateQuestPool</c> (<c>:840-885</c>) for the exploration half;
    /// the elimination targets stay empty, no exploration draw reads them.
    /// </summary>
    private QuestTypePool BuildPool()
    {
        var locations = _repeatableConfig.Locations.Where(location => location.Key != ELocationName.any).ToList();

        return new QuestTypePool
        {
            Types = [.. _repeatableConfig.Types],
            Pool = new QuestPool
            {
                Exploration = new ExplorationPool
                {
                    Locations = locations.ToDictionary(location => location.Key, location => location.Value),
                },
                Elimination = new EliminationPool { Targets = [] },
                Pickup = new ExplorationPool { Locations = [] },
            },
        };
    }

    private static void PatchFired()
    {
        _patchFired = true;
    }

    private static void Prefix()
    {
        _prefixFired = true;
    }

    private static void Postfix()
    {
        _postfixFired = true;
    }
}
