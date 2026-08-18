using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.Ragfair;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the mod hook contract for the ragfair linked item table: a Harmony patch on any frozen 4.1.2
/// member of <see cref="RagfairLinkedItemService"/> must route the build to the legacy path, because
/// that is the only body the patch can hook. <c>BuildLinkedItemTable</c> is the exception, it is the
/// dispatcher and a patch on it wraps whichever path runs. Harmony patches are process-wide, so
/// every patch is removed in a finally and the fixture never runs in parallel with others.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairLinkedItemHookLivenessTests
{
    private static bool _patchFired;
    private static bool _prefixFired;
    private static bool _postfixFired;

    private MongoId _knownTpl;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        _knownTpl = DI.GetInstance().GetService<TemplateTable>().Items.Keys.First();
    }

    [TestCaseSource(nameof(FrozenMembers))]
    public void HarmonyPatchOnAFrozenMemberForcesTheLegacyPath(MethodInfo member)
    {
        var harmony = new Harmony($"unit-tests.ragfair-linked-item-hook-liveness.{member.Name}");

        try
        {
            harmony.Patch(member, postfix: new HarmonyMethod(typeof(RagfairLinkedItemHookLivenessTests), nameof(PatchFired)));

            Assert.That(
                Harmony.GetPatchInfo(member)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {member.Name} was not registered"
            );

            // The build only runs on a cold cache, so each case needs its own instance
            var service = Build();

            service.GetLinkedItems(_knownTpl);

            Assert.That(
                service.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Legacy),
                $"a patch on {member.Name} did not force the legacy path"
            );
            Assert.That(service.CacheForTests, Is.Not.Empty, "the legacy path built no table");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// Every legacy pass calls GetSlotFilters, so this is what proves a patch on the frozen surface
    /// is actually live rather than a silently failed install that only trips the dispatch check.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAFrozenMemberFiresOnTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.ragfair-linked-item-hook-liveness.GetSlotFilters");
        var target = AccessTools.Method(typeof(RagfairLinkedItemService), "GetSlotFilters");
        Assert.That(target, Is.Not.Null, "frozen member GetSlotFilters not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(RagfairLinkedItemHookLivenessTests), nameof(PatchFired)));

            var service = Build();

            service.GetLinkedItems(_knownTpl);

            Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(_patchFired, Is.True, "postfix on GetSlotFilters never ran on the legacy path");
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
    public void HarmonyPatchOnBuildLinkedItemTableWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.ragfair-linked-item-hook-liveness.dispatcher");
        var target = AccessTools.Method(typeof(RagfairLinkedItemService), "BuildLinkedItemTable");
        Assert.That(target, Is.Not.Null, "BuildLinkedItemTable not found");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(RagfairLinkedItemHookLivenessTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(RagfairLinkedItemHookLivenessTests), nameof(Postfix))
            );

            var service = Build();

            service.GetLinkedItems(_knownTpl);

            Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Native), "a patch on the dispatcher forced legacy");
            Assert.That(service.CacheForTests, Is.Not.Empty);
            Assert.That(_prefixFired, Is.True, "prefix on BuildLinkedItemTable never ran");
            Assert.That(_postfixFired, Is.True, "postfix on BuildLinkedItemTable never ran");
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
    /// <c>BuildLinkedItemTable</c> - and drops a member mods can currently patch.
    /// </summary>
    [Test]
    public void TheHookableSetIsTheFrozenSurfaceMinusTheDispatcher()
    {
        var members =
            (List<MethodBase>)
                typeof(RagfairLinkedItemService)
                    .GetField("_hookableMembers", BindingFlags.Static | BindingFlags.NonPublic)!
                    .GetValue(null)!;

        Assert.That(members, Is.EquivalentTo(FrozenSurface()), "the hookable set is not the frozen surface minus the dispatcher");

        // The 4.1.2 surface by name, so a member added or dropped shows up here and not just as a
        // shape change both sides of the reflection scan agree on
        Assert.That(
            members.Select(member => member.Name),
            Is.EquivalentTo(
                new[]
                {
                    "GetLinkedItems",
                    "GetLinkedDbItems",
                    "GetSlotFilters",
                    "GetChamberFilters",
                    "GetCartridgeFilters",
                    "AddRevolverCylinderAmmoToLinkedItems",
                }
            )
        );
    }

    private static IEnumerable<TestCaseData> FrozenMembers()
    {
        return FrozenSurface().Select(member => new TestCaseData(member).SetArgDisplayNames(member.Name));
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
            .. typeof(RagfairLinkedItemService)
                .GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
                .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
                .Where(method => method.Name != "BuildLinkedItemTable"),
        ];
    }

    /// <summary>
    /// A service built by hand off the container's own services, on the additive constructor the
    /// container picks - a cold cache, so the query below actually reaches the dispatcher.
    /// </summary>
    private static RagfairLinkedItemService Build()
    {
        var constructor = typeof(RagfairLinkedItemService).GetConstructors().MaxBy(candidate => candidate.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => DI.GetInstance().GetService(parameter.ParameterType)).ToArray();

        return (RagfairLinkedItemService)constructor.Invoke(arguments);
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
