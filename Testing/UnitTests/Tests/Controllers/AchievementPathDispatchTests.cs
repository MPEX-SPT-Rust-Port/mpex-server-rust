using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Controllers;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Achievements;

namespace UnitTests.Tests.Controllers;

/// <summary>
/// Pins the dual-path dispatch for achievement statistics: native by default, the retained C# loop
/// when <see cref="CoreConfig.ForceLegacyAchievementStatistics"/> is set, when the frozen
/// constructor built the instance (no native seam to dispatch to), or when a mod substituted the
/// controller itself.
///
/// There is deliberately no fourth reason: this port moves no frozen member. The whole legacy body
/// is inline in <c>GetAchievementStatics</c>, which is the dispatcher - so a Harmony patch there
/// wraps whichever arm runs rather than forcing a decline, and the two members the native arm still
/// calls for itself, <c>ProfileHelper.GetProfiles</c> and the blacklist filter, stay in C# on both
/// arms. Both are pinned below.
///
/// Harmony patches are process-wide, so every patch is removed in a finally and the fixture never
/// runs in parallel. The force flag on the shared config singleton is restored per case.
/// </summary>
[TestFixture]
[NonParallelizable]
public class AchievementPathDispatchTests
{
    private static bool _prefixFired;
    private static bool _postfixFired;
    private static bool _patchFired;

    private readonly MongoId _sessionId = new();

    private AchievementController _controller = default!;
    private CoreConfig _coreConfig = default!;
    private bool _originalForceLegacy;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _controller = di.GetService<AchievementController>();
        _coreConfig = di.GetService<CoreConfig>();

        _originalForceLegacy = _coreConfig.ForceLegacyAchievementStatistics;
    }

    [TearDown]
    public void TearDown()
    {
        // The captured value, not false: a tree shipping the flag on would otherwise have it
        // silently flipped for every fixture that runs after this one
        _coreConfig.ForceLegacyAchievementStatistics = _originalForceLegacy;
    }

    /// <summary>
    /// The negative control: a stock controller with no force flag takes the native path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        AssertPath(_controller, LootGenerationPath.Native, "stock");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _coreConfig.ForceLegacyAchievementStatistics = true;

        AssertPath(_controller, LootGenerationPath.Legacy, "force flag");
    }

    /// <summary>
    /// A mod compiled against the frozen contract can construct the controller itself, and the
    /// frozen constructor has no native seam wired - such an instance has to run the C# body it was
    /// built for.
    /// </summary>
    [Test]
    public void TheFrozenConstructorRoutesToTheLegacyPath()
    {
        AssertPath(
            (AchievementController)Construct(typeof(AchievementController), narrowest: true),
            LootGenerationPath.Legacy,
            "frozen constructor"
        );
    }

    /// <summary>
    /// The negative control for the two cases either side of it: hand-building the controller off
    /// the container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltControllerWithStockServicesTakesTheNativePath()
    {
        AssertPath((AchievementController)Construct(typeof(AchievementController)), LootGenerationPath.Native, "hand-built");
    }

    /// <summary>
    /// A mod registering its own controller with a higher TypePriority hands the container a
    /// subclass, whose overrides only the C# path can run.
    /// </summary>
    [Test]
    public void AReplacedControllerRoutesToTheLegacyPath()
    {
        AssertPath(
            (AchievementController)Construct(typeof(TestAchievementControllerSubclass)),
            LootGenerationPath.Legacy,
            "replaced controller"
        );
    }

    /// <summary>
    /// The dispatcher rule: <c>GetAchievementStatics</c> is the entry point, so a patch on it wraps
    /// the native body rather than forcing the family back to C#, and the mod's hooks still see
    /// every call.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnGetAchievementStaticsWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.achievement-path-dispatch.GetAchievementStatics");
        var target = Member(typeof(AchievementController), nameof(AchievementController.GetAchievementStatics));

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(AchievementPathDispatchTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(AchievementPathDispatchTests), nameof(Postfix))
            );

            AssertPath(_controller, LootGenerationPath.Native, "a patch on GetAchievementStatics");

            Assert.That(_prefixFired, Is.True, "prefix on GetAchievementStatics never ran");
            Assert.That(_postfixFired, Is.True, "postfix on GetAchievementStatics never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// Hook liveness for the one member the native arm still calls for itself: the profile fetch
    /// stays in C# on both arms and its result is what the projection is built from, so a patch on
    /// it is live without costing the port its native path.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnGetProfilesFiresOnTheNativeArm()
    {
        var harmony = new Harmony("unit-tests.achievement-path-dispatch.GetProfiles");
        var target = Member(typeof(ProfileHelper), nameof(ProfileHelper.GetProfiles));

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(AchievementPathDispatchTests), nameof(PatchFired)));

            AssertPath(_controller, LootGenerationPath.Native, "a patch on GetProfiles");

            Assert.That(_patchFired, Is.True, "postfix on GetProfiles never ran on the native arm");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    private void AssertPath(AchievementController controller, LootGenerationPath expected, string what)
    {
        controller.GetAchievementStatics(_sessionId);

        Assert.That(controller.LastPathTaken, Is.EqualTo(expected), $"{what}: GetAchievementStatics took the wrong path");
    }

    /// <summary>
    /// One controller built by hand off the container's own services, on either the frozen
    /// constructor or the additive one the container picks.
    /// </summary>
    private static object Construct(Type type, bool narrowest = false)
    {
        var constructors = type.GetConstructors();
        var constructor = narrowest
            ? constructors.MinBy(candidate => candidate.GetParameters().Length)!
            : constructors.MaxBy(candidate => candidate.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => DI.GetInstance().GetService(parameter.ParameterType)).ToArray();

        return constructor.Invoke(arguments);
    }

    private static MethodInfo Member(Type declaringType, string name)
    {
        return declaringType.GetMethod(
                name,
                BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly
            ) ?? throw new InvalidOperationException($"{declaringType.Name}.{name} is not declared any more");
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

    /// <summary>
    /// Stands in for a mod-registered controller: identical behaviour, different type. Chains the
    /// widest base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestAchievementControllerSubclass(
        TemplateTable templateTable,
        ProfileHelper profileHelper,
        CoreConfig coreConfig,
        AchievementNativeRequestBuilder requestBuilder
    ) : AchievementController(templateTable, profileHelper, coreConfig, requestBuilder) { }
}
