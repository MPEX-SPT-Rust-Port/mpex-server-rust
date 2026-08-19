using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;

namespace SPTarkov.Server.Core.Native.BaseClass;

/// <summary>
/// Assembles the <see cref="ItemBaseClassRequest"/> for one bulk cache build out of the live items
/// table - everything <c>ItemBaseClassService.HydrateItemBaseClassCache</c> would have read for
/// itself.
/// </summary>
[Injectable]
public class ItemBaseClassNativeRequestBuilder(TemplateTable templateTable)
{
    private readonly ItemConfig? _itemConfig;
    private readonly IReadOnlyList<SptMod>? _loadedMods;
    private readonly DbPublisher? _dbPublisher;

    /// <summary>
    ///     The constructor the container uses: the frozen one plus the epoch-protocol services.
    ///     Additive and apicompat-verified.
    /// </summary>
    public ItemBaseClassNativeRequestBuilder(
        TemplateTable templateTable,
        ItemConfig itemConfig,
        IReadOnlyList<SptMod> loadedMods,
        DbPublisher dbPublisher
    )
        : this(templateTable)
    {
        _itemConfig = itemConfig;
        _loadedMods = loadedMods;
        _dbPublisher = dbPublisher;
    }

    /// <summary>
    ///     Whether the most recent native send carried the C#-built views override rather than
    ///     naming a resident-DB epoch. Test seam.
    /// </summary>
    internal bool LastSendIncludedViewsOverride { get; private set; }

    /// <summary>
    ///     Whether an override-less send off the resident DB is ever allowed: the services exist,
    ///     the kill switch is off, and either no mods are loaded or the user vouched their mods
    ///     don't write tables directly. A builder built on the frozen constructor has none of the
    ///     three services and always sends the override.
    /// </summary>
    internal bool ResidentDbEligible()
    {
        return _itemConfig is not null
            && ResidentDbDispatch.Eligible(
                _dbPublisher,
                _loadedMods?.Count,
                _itemConfig.DisableNativeRequestCache,
                _itemConfig.TrustNativeRequestCacheWithMods
            );
    }

    /// <summary>
    ///     One bulk cache build: off the resident DB when eligible (a stale epoch self-heals with
    ///     one republish and retry), the C#-built override at epoch 0 otherwise.
    /// </summary>
    internal ItemBaseClassResult Send()
    {
        if (!ResidentDbEligible())
        {
            LastSendIncludedViewsOverride = true;

            return SptNative.BuildItemBaseClassCache(new ItemBaseClassNativeRequest { Epoch = 0, ViewsOverride = Build() });
        }

        var result = ResidentDbDispatch.Send(
            _dbPublisher!,
            epoch => SptNative.BuildItemBaseClassCache(new ItemBaseClassNativeRequest { Epoch = epoch })
        );

        LastSendIncludedViewsOverride = false;

        return result;
    }

    public ItemBaseClassRequest Build()
    {
        // Deliberately not PayloadProjection.BuildItemsView: that projection drops templates
        // without _props, and the walk reads every entry the C# service iterates.
        var itemsView = new Dictionary<MongoId, ItemBaseClassItemView>(templateTable.Items.Count);

        foreach (var item in templateTable.Items)
        {
            itemsView[item.Key] = new ItemBaseClassItemView { Parent = item.Value.Parent, Type = item.Value.Type };
        }

        return new ItemBaseClassRequest { ItemsView = itemsView };
    }
}
