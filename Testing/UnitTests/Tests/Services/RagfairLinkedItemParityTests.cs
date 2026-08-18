using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Services.Ragfair;

namespace UnitTests.Tests.Services;

/// <summary>
/// Parity between the native linked item table build and the retained 4.1.2 C# walk over the shipped
/// items table. The walk is deterministic - no seeds, no ordering slack - so parity is exact equality
/// of the built tables (key sets, and each per-key set).
///
/// Quirk 1 makes the build single-shot per instance (the final copy loop uses <c>Dictionary.Add</c>),
/// so every case builds its own fresh service per path and never re-queries a warm one for a miss
/// unless the throw is what is under test. The fixture never runs in parallel with the ones that
/// mutate the shared <see cref="RagfairConfig"/> singleton or patch process-wide.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairLinkedItemParityTests
{
    private TemplateTable _templateTable = default!;
    private ItemHelper _itemHelper = default!;
    private ISptLogger<RagfairLinkedItemService> _logger = default!;
    private RagfairLinkedItemNativeRequestBuilder _requestBuilder = default!;
    private RagfairConfig _ragfairConfig = default!;

    /// <summary>
    /// Every template in the items table keys an entry on both paths, so any tpl triggers the build
    /// and survives <c>GetLinkedItems</c>' indexer read afterwards.
    /// </summary>
    private MongoId _anyTableTpl;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _templateTable = di.GetService<TemplateTable>();
        _itemHelper = di.GetService<ItemHelper>();
        _logger = di.GetService<ISptLogger<RagfairLinkedItemService>>();
        _requestBuilder = di.GetService<RagfairLinkedItemNativeRequestBuilder>();
        _ragfairConfig = di.GetService<RagfairConfig>();

        _anyTableTpl = _templateTable.Items.Keys.First();
    }

    [Test]
    public void NativeAndLegacyBuildsProduceIdenticalTables()
    {
        var legacy = MakeService(native: false);
        var native = MakeService(native: true);

        legacy.GetLinkedItems(_anyTableTpl);
        native.GetLinkedItems(_anyTableTpl);
        Assert.That(legacy.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(native.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(legacy.CacheForTests, Is.Not.Empty, "the legacy path built no table to compare against");

        Assert.That(native.CacheForTests.Keys, Is.EquivalentTo(legacy.CacheForTests.Keys));

        foreach (var (tpl, legacySet) in legacy.CacheForTests)
        {
            Assert.That(native.CacheForTests[tpl], Is.EquivalentTo(legacySet), $"linked set diverged for tpl {tpl}");
        }
    }

    /// <summary>
    /// Quirk 2's forward direction, spot-checked against the table rather than the service's own
    /// helpers: a revolver's set gains the camora ammo of the cylinder behind its <c>mod_magazine</c>
    /// slot. The expectation is recomputed here the way
    /// <c>AddRevolverCylinderAmmoToLinkedItems</c>/<c>GetSlotFilters</c> would, so a build that drops
    /// the special case entirely - on either path - fails even though both paths would still agree.
    /// </summary>
    [Test]
    public void RevolverSetsContainCamoraAmmoOnBothPaths()
    {
        var revolver = _templateTable.Items.Values.FirstOrDefault(item =>
            item.Parent == BaseClasses.REVOLVER && CylinderCamoraAmmo(item).Count > 0
        );
        Assert.That(revolver, Is.Not.Null, "the shipped table has no revolver with camora ammo behind its mod_magazine slot");

        var ammo = CylinderCamoraAmmo(revolver!);
        Assert.That(ammo, Is.Not.Empty);

        var legacy = MakeService(native: false);
        var native = MakeService(native: true);

        var legacySet = legacy.GetLinkedItems(revolver!.Id);
        var nativeSet = native.GetLinkedItems(revolver.Id);
        Assert.That(legacy.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(native.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));

        Assert.That(legacySet, Is.SupersetOf(ammo), $"legacy dropped the camora ammo of revolver {revolver.Id}");
        Assert.That(nativeSet, Is.SupersetOf(ammo), $"native dropped the camora ammo of revolver {revolver.Id}");
    }

    /// <summary>
    /// Quirk 1: with a warm cache, a miss re-runs the build, whose <c>.Add</c> of an existing key
    /// throws ArgumentException before the indexer can miss - identically on both paths.
    /// </summary>
    [Test]
    public void UnknownIdThrowsTheSameWayOnBothPaths()
    {
        var legacy = MakeService(native: false);
        var native = MakeService(native: true);
        legacy.GetLinkedItems(_anyTableTpl);
        native.GetLinkedItems(_anyTableTpl);

        var unknown = new MongoId("ffffffffffffffffffffffff");
        Assert.That(() => legacy.GetLinkedItems(unknown), Throws.ArgumentException);
        Assert.That(() => native.GetLinkedItems(unknown), Throws.ArgumentException);
    }

    /// <summary>
    /// Every table tpl keys an entry even when nothing links to it, so an unlinked template answers
    /// with an empty set rather than the rebuild-and-throw a missing key would get.
    /// </summary>
    [Test]
    public void UnlinkedTemplatesAnswerWithEmptySetsOnBothPaths()
    {
        var legacy = MakeService(native: false);
        var native = MakeService(native: true);

        legacy.GetLinkedItems(_anyTableTpl);
        Assert.That(legacy.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));

        var unlinked = legacy.CacheForTests.FirstOrDefault(entry => entry.Value.Count == 0).Key;
        if (unlinked.IsEmpty)
        {
            Assert.Pass("the shipped table has no unlinked template; the full-table equality above carries the general claim");
        }

        Assert.That(legacy.GetLinkedItems(unlinked), Is.Empty);
        Assert.That(native.GetLinkedItems(unlinked), Is.Empty);
        Assert.That(native.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
    }

    /// <summary>
    /// The camora ammo <c>AddRevolverCylinderAmmoToLinkedItems</c> would add for a revolver, recomputed
    /// from the raw table: the cylinder behind the <c>mod_magazine</c> slot
    /// (<c>RagfairLinkedItemService.cs:252-259</c>), then that cylinder's own slot filter tpls
    /// (<c>:269</c>, <c>GetSlotFilters</c>). Empty when anything on the way is missing - the shapes
    /// quirks 3 and 4 sanction a divergence for are simply not candidates for the spot-check.
    /// </summary>
    private HashSet<MongoId> CylinderCamoraAmmo(TemplateItem revolver)
    {
        var cylinderMod = revolver.Properties?.Slots?.FirstOrDefault(slot => slot.Name == "mod_magazine");
        var cylinderTpl = cylinderMod?.Properties?.Filters?.FirstOrDefault()?.Filter?.FirstOrDefault() ?? MongoId.Empty();

        // The IsValidMongoId gate (:261) is subsumed here: a malformed tpl is never a table key
        if (!_templateTable.Items.TryGetValue(cylinderTpl, out var cylinder))
        {
            return [];
        }

        return SlotFilterTpls(cylinder);
    }

    /// <summary>
    /// <c>GetSlotFilters</c> (<c>RagfairLinkedItemService.cs:277-303</c>) recomputed off the table -
    /// the protected helper is the authority for the shape, not the source of the expectation.
    /// </summary>
    private static HashSet<MongoId> SlotFilterTpls(TemplateItem item)
    {
        return
        [
            .. (item.Properties?.Slots ?? []).SelectMany(slot => slot.Properties?.Filters ?? []).SelectMany(group => group.Filter ?? []),
        ];
    }

    /// <summary>
    /// A fresh service per path - quirk 1 makes the build single-shot per instance, and a used
    /// instance's cache would contaminate the comparison. The frozen 4.1.2 constructor wires no native
    /// seam, so it builds legacy unconditionally.
    /// </summary>
    private RagfairLinkedItemService MakeService(bool native)
    {
        if (native)
        {
            return new RagfairLinkedItemService(_templateTable, _itemHelper, _logger, _requestBuilder, _ragfairConfig);
        }

        return new RagfairLinkedItemService(_templateTable, _itemHelper, _logger);
    }
}
