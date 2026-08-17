using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.BaseClass;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the dual-path dispatch for the item base class cache build: native by default, the retained
/// 4.1.2 C# implementation when <c>ItemConfig.ForceLegacyItemBaseClassHydration</c> is set, when the
/// frozen 4.1.2 constructor built the instance (no native seam to dispatch to), when a mod
/// substituted the service itself, or when a Harmony patch on a frozen member is live
/// (<see cref="ItemBaseClassHookLivenessTests"/> covers the whole hookable set).
///
/// Mutates the shared <see cref="ItemConfig"/> singleton and patches process-wide, so both are
/// restored per case and the fixture never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ItemBaseClassPathDispatchTests
{
    private ItemBaseClassService _itemBaseClassService = default!;
    private ItemConfig _itemConfig = default!;

    private bool _forceLegacyOriginal;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _itemBaseClassService = di.GetService<ItemBaseClassService>();
        _itemConfig = di.GetService<ItemConfig>();

        _forceLegacyOriginal = _itemConfig.ForceLegacyItemBaseClassHydration;
    }

    [TearDown]
    public void TearDown()
    {
        _itemConfig.ForceLegacyItemBaseClassHydration = _forceLegacyOriginal;
    }

    /// <summary>
    /// The negative control: a stock container, no force flag and no patches take the native path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        _itemBaseClassService.HydrateItemBaseClassCache();

        Assert.That(_itemBaseClassService.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(_itemBaseClassService.CacheForTests, Is.Not.Empty, "the native path built no cache");
        Assert.That(_itemBaseClassService.RootNodeIdsForTests, Is.Not.Empty, "the native path found no root nodes");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _itemConfig.ForceLegacyItemBaseClassHydration = true;

        _itemBaseClassService.HydrateItemBaseClassCache();

        Assert.That(_itemBaseClassService.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(_itemBaseClassService.CacheForTests, Is.Not.Empty, "the legacy path built no cache");
        Assert.That(_itemBaseClassService.RootNodeIdsForTests, Is.Not.Empty, "the legacy path found no root nodes");
    }

    /// <summary>
    /// A mod compiled against 4.1.2 can construct the service itself, and the frozen constructor has
    /// no native seam wired - such an instance has to run the C# body it was built for.
    /// </summary>
    [Test]
    public void TheFrozenConstructorRoutesToTheLegacyPath()
    {
        var service = Build(narrowest: true);

        service.HydrateItemBaseClassCache();

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// The negative control for the two cases above and below: hand-building the service off the
    /// container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltServiceWithStockServicesTakesTheNativePath()
    {
        var service = Build();

        service.HydrateItemBaseClassCache();

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
    }

    /// <summary>
    /// A mod registering its own service with a higher TypePriority hands the container a subclass,
    /// whose overrides only the C# path can run. Built on the widest constructor, so the native seam
    /// is wired and the substituted type is the only reason left to fall back.
    /// </summary>
    [Test]
    public void AReplacedServiceRoutesToTheLegacyPath()
    {
        var service = (ItemBaseClassService)Construct(typeof(TestItemBaseClassServiceSubclass));

        service.HydrateItemBaseClassCache();

        Assert.That(service.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// AddBaseItems is one of the frozen 4.1.2 members, so a live patch on it has to route the
    /// hydrate to the only body the patch can hook.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAFrozenMemberForcesTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.item-base-class-path-dispatch.AddBaseItems");
        var target = AccessTools.Method(typeof(ItemBaseClassService), "AddBaseItems");
        Assert.That(target, Is.Not.Null, "frozen member AddBaseItems not found");

        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(ItemBaseClassPathDispatchTests), nameof(Postfix)));

            _itemBaseClassService.HydrateItemBaseClassCache();

            Assert.That(_itemBaseClassService.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(_itemBaseClassService.CacheForTests, Is.Not.Empty, "the legacy path built no cache");
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
    private static ItemBaseClassService Build(bool narrowest = false)
    {
        return (ItemBaseClassService)Construct(typeof(ItemBaseClassService), narrowest);
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
    /// fires is <see cref="ItemBaseClassHookLivenessTests"/>' business.
    /// </summary>
    private static void Postfix() { }

    /// <summary>
    /// Stands in for a mod-registered service: identical behaviour, different type. Chains the widest
    /// base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestItemBaseClassServiceSubclass(
        ISptLogger<ItemBaseClassService> logger,
        TemplateTable templateTable,
        ServerLocalisationService serverLocalisationService,
        ItemBaseClassNativeRequestBuilder requestBuilder,
        ItemConfig itemConfig
    ) : ItemBaseClassService(logger, templateTable, serverLocalisationService, requestBuilder, itemConfig) { }
}
