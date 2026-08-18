using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Services.Ragfair;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the dual-path dispatch for the ragfair linked item table build: native by default, the
/// retained 4.1.2 C# implementation when <c>RagfairConfig.ForceLegacyRagfairLinkedItemBuild</c> is
/// set, when the frozen 4.1.2 constructor built the instance (no native seam to dispatch to), when a
/// mod substituted the service itself, or when a Harmony patch on a frozen member is live
/// (<see cref="RagfairLinkedItemHookLivenessTests"/> covers the whole hookable set).
///
/// The build is only reachable through <c>GetLinkedItems</c> on a cold cache - quirk 1's <c>.Add</c>
/// throws on a rebuild - so every case queries its own freshly built service.
///
/// Mutates the shared <see cref="RagfairConfig"/> singleton and patches process-wide, so both are
/// restored per case and the fixture never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairLinkedItemPathDispatchTests
{
    private RagfairConfig _ragfairConfig = default!;

    /// <summary>
    /// Every template in the items table keys an entry on both paths, so any tpl triggers the build
    /// and survives <c>GetLinkedItems</c>' indexer read afterwards.
    /// </summary>
    private MongoId _knownTpl;

    private bool _forceLegacyOriginal;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _ragfairConfig = di.GetService<RagfairConfig>();
        _knownTpl = di.GetService<TemplateTable>().Items.Keys.First();

        _forceLegacyOriginal = _ragfairConfig.ForceLegacyRagfairLinkedItemBuild;
    }

    [TearDown]
    public void TearDown()
    {
        _ragfairConfig.ForceLegacyRagfairLinkedItemBuild = _forceLegacyOriginal;
    }

    /// <summary>
    /// The negative control: a stock container, no force flag and no patches take the native path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        var service = Build();

        service.GetLinkedItems(_knownTpl);

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(service.CacheForTests, Is.Not.Empty, "the native path built no table");
    }

    /// <summary>
    /// The container's own singleton, to pin that it resolves on the additive constructor rather than
    /// the frozen one. A warm cache from an earlier fixture skips the rebuild, but the path recorded
    /// is still the one that single build took.
    /// </summary>
    [Test]
    public void TheContainerResolvedServiceTakesTheNativePath()
    {
        var service = DI.GetInstance().GetService<RagfairLinkedItemService>();

        service.GetLinkedItems(_knownTpl);

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(service.CacheForTests, Is.Not.Empty, "the native path built no table");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _ragfairConfig.ForceLegacyRagfairLinkedItemBuild = true;

        var service = Build();

        service.GetLinkedItems(_knownTpl);

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(service.CacheForTests, Is.Not.Empty, "the legacy path built no table");
    }

    /// <summary>
    /// A mod compiled against 4.1.2 can construct the service itself, and the frozen constructor has
    /// no native seam wired - such an instance has to run the C# body it was built for.
    /// </summary>
    [Test]
    public void TheFrozenConstructorRoutesToTheLegacyPath()
    {
        var service = Build(narrowest: true);

        service.GetLinkedItems(_knownTpl);

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// A mod registering its own service with a higher TypePriority hands the container a subclass,
    /// whose overrides only the C# path can run. Built on the widest constructor, so the native seam
    /// is wired and the substituted type is the only reason left to fall back.
    /// </summary>
    [Test]
    public void AReplacedServiceRoutesToTheLegacyPath()
    {
        var service = (RagfairLinkedItemService)Construct(typeof(TestRagfairLinkedItemServiceSubclass));

        service.GetLinkedItems(_knownTpl);

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// GetSlotFilters is one of the frozen 4.1.2 members, so a live patch on it has to route the
    /// build to the only body the patch can hook.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAFrozenMemberForcesTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.ragfair-linked-item-path-dispatch.GetSlotFilters");
        var target = AccessTools.Method(typeof(RagfairLinkedItemService), "GetSlotFilters");
        Assert.That(target, Is.Not.Null, "frozen member GetSlotFilters not found");

        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(RagfairLinkedItemPathDispatchTests), nameof(Postfix)));

            var service = Build();

            service.GetLinkedItems(_knownTpl);

            Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(service.CacheForTests, Is.Not.Empty, "the legacy path built no table");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// A service built by hand off the container's own services, on either the frozen 4.1.2
    /// constructor or the additive one the container picks.
    /// </summary>
    private static RagfairLinkedItemService Build(bool narrowest = false)
    {
        return (RagfairLinkedItemService)Construct(typeof(RagfairLinkedItemService), narrowest);
    }

    private static object Construct(Type type, bool narrowest = false)
    {
        // The service carries the frozen 4.1.2 constructor plus the additive overload the container
        // uses; take the widest unless the frozen one is what is under test
        var constructors = type.GetConstructors();
        var constructor = narrowest
            ? constructors.MinBy(candidate => candidate.GetParameters().Length)!
            : constructors.MaxBy(candidate => candidate.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => DI.GetInstance().GetService(parameter.ParameterType)).ToArray();

        return constructor.Invoke(arguments);
    }

    /// <summary>
    /// The dispatch check reads Harmony's patch info, so the patch only has to exist - whether it
    /// fires is <see cref="RagfairLinkedItemHookLivenessTests"/>' business.
    /// </summary>
    private static void Postfix() { }

    /// <summary>
    /// Stands in for a mod-registered service: identical behaviour, different type. Chains the widest
    /// base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestRagfairLinkedItemServiceSubclass(
        TemplateTable templateTable,
        ItemHelper itemHelper,
        ISptLogger<RagfairLinkedItemService> logger,
        RagfairLinkedItemNativeRequestBuilder requestBuilder,
        RagfairConfig ragfairConfig
    ) : RagfairLinkedItemService(templateTable, itemHelper, logger, requestBuilder, ragfairConfig) { }
}
