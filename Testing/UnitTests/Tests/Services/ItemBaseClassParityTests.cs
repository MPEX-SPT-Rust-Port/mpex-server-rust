using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.BaseClass;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;

namespace UnitTests.Tests.Services;

/// <summary>
/// Parity between the native item base class cache build and the retained 4.1.2 C# walk over the
/// shipped items table. The walk is deterministic - no seeds, no ordering slack - so parity is exact
/// equality of both outputs (<c>_itemBaseClassesCache</c> and <c>_rootNodeIds</c>).
///
/// Mutates the shared <see cref="ItemConfig"/> singleton and, in the re-hydrate case, the shared
/// items table, so both are restored and the fixture never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ItemBaseClassParityTests
{
    private ISptLogger<ItemBaseClassService> _logger = default!;
    private TemplateTable _templateTable = default!;
    private ServerLocalisationService _serverLocalisationService = default!;
    private ItemBaseClassNativeRequestBuilder _requestBuilder = default!;
    private ItemConfig _itemConfig = default!;
    private ItemHelper _itemHelper = default!;
    private ItemBaseClassService _containerService = default!;

    private bool _forceLegacyOriginal;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _logger = di.GetService<ISptLogger<ItemBaseClassService>>();
        _templateTable = di.GetService<TemplateTable>();
        _serverLocalisationService = di.GetService<ServerLocalisationService>();
        _requestBuilder = di.GetService<ItemBaseClassNativeRequestBuilder>();
        _itemConfig = di.GetService<ItemConfig>();
        _itemHelper = di.GetService<ItemHelper>();
        _containerService = di.GetService<ItemBaseClassService>();

        _forceLegacyOriginal = _itemConfig.ForceLegacyItemBaseClassHydration;
    }

    [TearDown]
    public void TearDown()
    {
        _itemConfig.ForceLegacyItemBaseClassHydration = _forceLegacyOriginal;
    }

    [Test]
    public void NativeAndLegacyHydrateProduceIdenticalCaches()
    {
        var legacy = MakeService(native: false);
        var native = MakeService(native: true);

        legacy.HydrateItemBaseClassCache();
        native.HydrateItemBaseClassCache();

        Assert.That(legacy.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(native.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(legacy.CacheForTests, Is.Not.Empty, "the legacy path built no cache to compare against");

        Assert.That(native.CacheForTests.Keys, Is.EquivalentTo(legacy.CacheForTests.Keys));

        foreach (var (tpl, legacySet) in legacy.CacheForTests)
        {
            Assert.That(native.CacheForTests[tpl], Is.EquivalentTo(legacySet), $"base class chain diverged for tpl {tpl}");
        }

        Assert.That(native.RootNodeIdsForTests, Is.EquivalentTo(legacy.RootNodeIdsForTests));
    }

    /// <summary>
    /// Quirk 1, ported verbatim: hydrate resets only the cache dictionary, never <c>_rootNodeIds</c>,
    /// so root ids accumulate across re-hydrates on both paths. Pinned by seeding a root id, then
    /// deleting its template so neither path can re-derive it - the id has to survive every
    /// subsequent hydrate, in both same-path and cross-path directions, on the one instance.
    /// Swapping the native arm's UnionWith for a replacement fails here.
    /// </summary>
    [Test]
    public void RehydrateAccumulatesRootNodeIdsOnBothPaths()
    {
        var service = MakeService(native: true);

        service.HydrateItemBaseClassCache();

        var seeded = service.RootNodeIdsForTests.ToHashSet();
        Assert.That(seeded, Is.Not.Empty, "no root nodes to seed the accumulation with");

        var vanishing = seeded.First();
        var removedTemplate = _templateTable.Items[vanishing];
        _templateTable.Items.Remove(vanishing);

        try
        {
            AssertRootIdsSurvive(service, seeded, LootGenerationPath.Native, "native -> native");

            _itemConfig.ForceLegacyItemBaseClassHydration = true;
            AssertRootIdsSurvive(service, seeded, LootGenerationPath.Legacy, "native -> legacy");
            AssertRootIdsSurvive(service, seeded, LootGenerationPath.Legacy, "legacy -> legacy");

            _itemConfig.ForceLegacyItemBaseClassHydration = false;
            AssertRootIdsSurvive(service, seeded, LootGenerationPath.Native, "legacy -> native");
        }
        finally
        {
            _templateTable.Items[vanishing] = removedTemplate;
        }
    }

    /// <summary>
    /// The cache is only ever read through <see cref="ItemHelper.IsOfBaseclass"/>, so the answers it
    /// gives have to be the same whichever path filled it - including the root node case, which the
    /// cache answers by absence rather than by content.
    /// </summary>
    [Test]
    public void ItemHelperAnswersMatchAfterNativeHydrate()
    {
        (MongoId Tpl, MongoId BaseClass, bool Expected)[] pairs =
        [
            (ItemTpl.AMMO_762X39_BP, BaseClasses.AMMO, true),
            (ItemTpl.AMMO_762X39_BP, BaseClasses.WEAPON, false),
            (ItemTpl.ASSAULTRIFLE_COLT_M4A1_556X45_ASSAULT_RIFLE, BaseClasses.WEAPON, true),
            // A Node-type tpl is never cached, so it inherits from nothing - not even its real parent
            (BaseClasses.WEAPON, BaseClasses.ITEM, false),
        ];

        _containerService.HydrateItemBaseClassCache();
        Assert.That(_containerService.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));

        foreach (var (tpl, baseClass, expected) in pairs)
        {
            Assert.That(_itemHelper.IsOfBaseclass(tpl, baseClass), Is.EqualTo(expected), $"native answer for {tpl} vs {baseClass}");
        }

        _itemConfig.ForceLegacyItemBaseClassHydration = true;
        _containerService.HydrateItemBaseClassCache();
        Assert.That(_containerService.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));

        foreach (var (tpl, baseClass, expected) in pairs)
        {
            Assert.That(_itemHelper.IsOfBaseclass(tpl, baseClass), Is.EqualTo(expected), $"legacy answer for {tpl} vs {baseClass}");
        }
    }

    private static void AssertRootIdsSurvive(
        ItemBaseClassService service,
        IReadOnlyCollection<MongoId> seeded,
        LootGenerationPath expectedPath,
        string direction
    )
    {
        service.HydrateItemBaseClassCache();

        Assert.That(service.LastPathTaken, Is.EqualTo(expectedPath), direction);
        Assert.That(service.RootNodeIdsForTests, Is.SupersetOf(seeded), $"{direction} re-hydrate dropped accumulated root node ids");
    }

    /// <summary>
    /// A fresh service per path - a used instance's cache would contaminate the comparison. The
    /// frozen 4.1.2 constructor wires no native seam, so it hydrates legacy unconditionally.
    /// </summary>
    private ItemBaseClassService MakeService(bool native)
    {
        if (native)
        {
            return new ItemBaseClassService(_logger, _templateTable, _serverLocalisationService, _requestBuilder, _itemConfig);
        }

        return new ItemBaseClassService(_logger, _templateTable, _serverLocalisationService);
    }
}
