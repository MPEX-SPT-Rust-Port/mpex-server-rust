using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Services.Items;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the mod hook contract for the item base class cache: a Harmony patch on any frozen 4.1.2
/// member of <see cref="ItemBaseClassService"/> must route the hydrate to the legacy path, because
/// that is the only body the patch can hook. <c>HydrateItemBaseClassCache</c> is the exception, it
/// is the dispatcher and a patch on it wraps whichever path runs. Harmony patches are process-wide,
/// so every patch is removed in a finally and the fixture never runs in parallel with others.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ItemBaseClassHookLivenessTests
{
    private static bool _patchFired;
    private static bool _prefixFired;
    private static bool _postfixFired;

    private ItemBaseClassService _itemBaseClassService = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        _itemBaseClassService = DI.GetInstance().GetService<ItemBaseClassService>();
    }

    [TestCaseSource(nameof(FrozenMembers))]
    public void HarmonyPatchOnAFrozenMemberForcesTheLegacyPath(MethodInfo member)
    {
        var harmony = new Harmony($"unit-tests.item-base-class-hook-liveness.{member.Name}");

        try
        {
            harmony.Patch(member, postfix: new HarmonyMethod(typeof(ItemBaseClassHookLivenessTests), nameof(PatchFired)));

            Assert.That(
                Harmony.GetPatchInfo(member)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {member.Name} was not registered"
            );

            _itemBaseClassService.HydrateItemBaseClassCache();

            Assert.That(
                _itemBaseClassService.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Legacy),
                $"a patch on {member.Name} did not force the legacy path"
            );
            Assert.That(_itemBaseClassService.CacheForTests, Is.Not.Empty, "the legacy path built no cache");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// Every legacy pass calls AddItemToCache, so this is what proves a patch on the frozen surface
    /// is actually live rather than a silently failed install that only trips the dispatch check.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAFrozenMemberFiresOnTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.item-base-class-hook-liveness.AddItemToCache");
        var target = AccessTools.Method(typeof(ItemBaseClassService), nameof(ItemBaseClassService.AddItemToCache));
        Assert.That(target, Is.Not.Null, "frozen member AddItemToCache not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(ItemBaseClassHookLivenessTests), nameof(PatchFired)));

            _itemBaseClassService.HydrateItemBaseClassCache();

            Assert.That(_itemBaseClassService.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(_patchFired, Is.True, "postfix on AddItemToCache never ran on the legacy path");
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
    public void HarmonyPatchOnHydrateWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.item-base-class-hook-liveness.dispatcher");
        var target = AccessTools.Method(typeof(ItemBaseClassService), nameof(ItemBaseClassService.HydrateItemBaseClassCache));
        Assert.That(target, Is.Not.Null, "HydrateItemBaseClassCache not found");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(ItemBaseClassHookLivenessTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(ItemBaseClassHookLivenessTests), nameof(Postfix))
            );

            _itemBaseClassService.HydrateItemBaseClassCache();

            Assert.That(
                _itemBaseClassService.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Native),
                "a patch on the dispatcher forced legacy"
            );
            Assert.That(_itemBaseClassService.CacheForTests, Is.Not.Empty);
            Assert.That(_prefixFired, Is.True, "prefix on HydrateItemBaseClassCache never ran");
            Assert.That(_postfixFired, Is.True, "postfix on HydrateItemBaseClassCache never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The hookable set is built by reflection, and this recomputes the frozen surface independently
    /// to pin the set's exact contents rather than just its shape: it fails if the service's own scan
    /// is ever narrowed - a tightened binding-flag set, another name excluded beside
    /// <c>HydrateItemBaseClassCache</c> - and drops a member mods can currently patch.
    /// </summary>
    [Test]
    public void TheHookableSetIsTheFrozenSurfaceMinusTheDispatcher()
    {
        var members =
            (List<MethodBase>)
                typeof(ItemBaseClassService).GetField("_hookableMembers", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

        Assert.That(members, Is.EquivalentTo(FrozenSurface()), "the hookable set is not the frozen surface minus the dispatcher");
    }

    private static IEnumerable<TestCaseData> FrozenMembers()
    {
        // ItemHasBaseClass is overloaded, so the display name carries the parameter types too
        return FrozenSurface()
            .Select(member =>
                new TestCaseData(member).SetArgDisplayNames(
                    $"{member.Name}({string.Join(", ", member.GetParameters().Select(parameter => parameter.ParameterType.Name))})"
                )
            );
    }

    /// <summary>
    /// The public, protected and protected-internal methods declared on the service minus the
    /// dispatcher - the surface the apicompat gate freezes, recomputed independently of the service's
    /// own scan.
    /// </summary>
    private static List<MethodInfo> FrozenSurface()
    {
        return
        [
            .. typeof(ItemBaseClassService)
                .GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
                .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
                .Where(method => method.Name != nameof(ItemBaseClassService.HydrateItemBaseClassCache)),
        ];
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
