using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;

namespace SPTarkov.Server.Core.Native.Ragfair;

/// <summary>
/// Assembles the <see cref="RagfairLinkedItemRequest"/> for one bulk table build out of the live
/// items table - everything <c>RagfairLinkedItemService.BuildLinkedItemTable</c> would have read
/// for itself.
/// </summary>
[Injectable]
public class RagfairLinkedItemNativeRequestBuilder(TemplateTable templateTable)
{
    private readonly RagfairConfig? _ragfairConfig;
    private readonly IReadOnlyList<SptMod>? _loadedMods;
    private readonly DbPublisher? _dbPublisher;

    /// <summary>
    ///     The constructor the container uses: the frozen one plus the epoch-protocol services.
    ///     Additive and apicompat-verified.
    /// </summary>
    public RagfairLinkedItemNativeRequestBuilder(
        TemplateTable templateTable,
        RagfairConfig ragfairConfig,
        IReadOnlyList<SptMod> loadedMods,
        DbPublisher dbPublisher
    )
        : this(templateTable)
    {
        _ragfairConfig = ragfairConfig;
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
        return _ragfairConfig is not null
            && ResidentDbDispatch.Eligible(
                _dbPublisher,
                _loadedMods?.Count,
                _ragfairConfig.DisableNativeRequestCache,
                _ragfairConfig.TrustNativeRequestCacheWithMods
            );
    }

    /// <summary>
    ///     One bulk table build: off the resident DB when eligible (a stale epoch self-heals with
    ///     one republish and retry), the C#-built override at epoch 0 otherwise.
    /// </summary>
    internal RagfairLinkedItemResult Send()
    {
        if (!ResidentDbEligible())
        {
            LastSendIncludedViewsOverride = true;

            return SptNative.BuildRagfairLinkedItemTable(new RagfairLinkedItemNativeRequest { Epoch = 0, ViewsOverride = Build() });
        }

        var result = ResidentDbDispatch.Send(
            _dbPublisher!,
            epoch => SptNative.BuildRagfairLinkedItemTable(new RagfairLinkedItemNativeRequest { Epoch = epoch })
        );

        LastSendIncludedViewsOverride = false;

        return result;
    }

    public RagfairLinkedItemRequest Build()
    {
        // Deliberately not PayloadProjection.BuildItemsView: every template must cross, propless
        // ones included - each seeds an empty linked set (RagfairLinkedItemService.cs:67) that
        // GetLinkedItems answers with - and the walk unions all Filters groups where that
        // projection keeps only the first.
        var itemsView = new Dictionary<MongoId, RagfairLinkedItemView>(templateTable.Items.Count);

        foreach (var item in templateTable.Items)
        {
            itemsView[item.Key] = new RagfairLinkedItemView
            {
                Parent = item.Value.Parent,
                Slots = ToLinkedSlotViews(item.Value.Properties?.Slots, includeName: true),
                Chambers = ToLinkedSlotViews(item.Value.Properties?.Chambers, includeName: false),
                Cartridges = ToLinkedSlotViews(item.Value.Properties?.Cartridges, includeName: false),
            };
        }

        return new RagfairLinkedItemRequest { ItemsView = itemsView };
    }

    private static List<RagfairLinkedSlotView>? ToLinkedSlotViews(IEnumerable<Slot>? slots, bool includeName)
    {
        if (slots is null)
        {
            return null;
        }

        var views = new List<RagfairLinkedSlotView>();
        foreach (var slot in slots)
        {
            views.Add(
                new RagfairLinkedSlotView
                {
                    // Only the revolver cylinder lookup reads names, and only on Slots.
                    Name = includeName ? slot.Name : null,
                    // Quirk 5, sanctioned divergence: a null Filter group crosses as nothing where
                    // legacy throws ArgumentNullException (RagfairLinkedItemService.cs:165).
                    Filter = slot.Properties?.Filters?.SelectMany(filterGroup => filterGroup.Filter ?? []).ToList(),
                }
            );
        }

        return views;
    }
}
