using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Tables;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the mod hook contract for scav case rewards: a Harmony patch on any frozen 4.1.2 member of
/// <see cref="ScavCaseRewardGenerator"/> must route generation to the legacy path, because that is
/// the only body the patch can hook. <c>Generate</c> is the exception, it is the dispatcher and a
/// patch on it wraps whichever path runs. Harmony patches are process-wide, so every patch is
/// removed in a finally and the fixture never runs in parallel with others.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ScavCaseHookLivenessTests
{
    private static bool _patchFired;
    private static bool _prefixFired;
    private static bool _postfixFired;

    private ScavCaseRewardGenerator _scavCaseRewardGenerator = default!;
    private MongoId _recipeId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _scavCaseRewardGenerator = di.GetService<ScavCaseRewardGenerator>();
        _recipeId = di.GetService<HideoutTable>().Production.ScavRecipes!.First().Id;

        _scavCaseRewardGenerator.NativeTestSeed = 424242;
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _scavCaseRewardGenerator.NativeTestSeed = null;
    }

    [TestCaseSource(nameof(FrozenMembers))]
    public void HarmonyPatchOnAFrozenMemberForcesTheLegacyPath(MethodInfo member)
    {
        var harmony = new Harmony($"unit-tests.scav-case-hook-liveness.{member.Name}");

        try
        {
            harmony.Patch(member, postfix: new HarmonyMethod(typeof(ScavCaseHookLivenessTests), nameof(PatchFired)));

            Assert.That(
                Harmony.GetPatchInfo(member)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {member.Name} was not registered"
            );

            var rewards = Generate();

            Assert.That(
                _scavCaseRewardGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Legacy),
                $"a patch on {member.Name} did not force the legacy path"
            );
            Assert.That(rewards, Is.Not.Empty, "the legacy path produced no rewards");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// Every legacy pass calls CacheDbItems, so this is what proves a patch on the frozen surface is
    /// actually live rather than a silently failed install that only trips the dispatch check.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAFrozenMemberFiresOnTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.scav-case-hook-liveness.CacheDbItems");
        var target = AccessTools.Method(typeof(ScavCaseRewardGenerator), "CacheDbItems");
        Assert.That(target, Is.Not.Null, "frozen member CacheDbItems not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(ScavCaseHookLivenessTests), nameof(PatchFired)));

            Generate();

            Assert.That(_scavCaseRewardGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(_patchFired, Is.True, "postfix on CacheDbItems never ran on the legacy path");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The dispatcher is deliberately not in the hookable set: a patch on it wraps whichever path
    /// runs, so it keeps the native body and still sees the call.
    /// </summary>
    [Test]
    public void HarmonyPatchOnGenerateWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.scav-case-hook-liveness.dispatcher");
        var target = AccessTools.Method(typeof(ScavCaseRewardGenerator), nameof(ScavCaseRewardGenerator.Generate));
        Assert.That(target, Is.Not.Null, "Generate not found");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(ScavCaseHookLivenessTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(ScavCaseHookLivenessTests), nameof(Postfix))
            );

            var rewards = Generate();

            Assert.That(
                _scavCaseRewardGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Native),
                "a patch on the dispatcher forced legacy"
            );
            Assert.That(rewards, Is.Not.Empty);
            Assert.That(_prefixFired, Is.True, "prefix on Generate never ran");
            Assert.That(_postfixFired, Is.True, "postfix on Generate never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The hookable set is built by reflection and the dispatcher is excluded by name, so a member
    /// added under the name <c>Generate</c> would silently fall out of the scan and become
    /// unhookable. Recomputing the frozen surface here pins the set's exact contents, not just its
    /// shape.
    /// </summary>
    [Test]
    public void TheHookableSetIsTheFrozenSurfaceMinusTheDispatcher()
    {
        var members =
            (List<MethodBase>)
                typeof(ScavCaseRewardGenerator).GetField("_hookableMembers", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

        Assert.That(members, Is.EquivalentTo(FrozenSurface()), "the hookable set is not the frozen surface minus the dispatcher");
    }

    private static IEnumerable<TestCaseData> FrozenMembers()
    {
        return FrozenSurface().Select(member => new TestCaseData(member).SetArgDisplayNames(member.Name));
    }

    /// <summary>
    /// The public, protected and protected-internal methods declared on the generator minus the
    /// dispatcher - the surface the apicompat gate freezes, recomputed independently of the
    /// generator's own scan.
    /// </summary>
    private static List<MethodInfo> FrozenSurface()
    {
        return
        [
            .. typeof(ScavCaseRewardGenerator)
                .GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
                .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
                .Where(method => method.Name != nameof(ScavCaseRewardGenerator.Generate)),
        ];
    }

    private List<List<Item>> Generate()
    {
        return _scavCaseRewardGenerator.Generate(_recipeId).ToList();
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
