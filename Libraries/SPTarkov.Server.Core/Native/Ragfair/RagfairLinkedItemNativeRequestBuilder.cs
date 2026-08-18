using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Tables;

namespace SPTarkov.Server.Core.Native.Ragfair;

/// <summary>
/// Assembles the <see cref="RagfairLinkedItemRequest"/> for one bulk table build out of the live
/// items table - everything <c>RagfairLinkedItemService.BuildLinkedItemTable</c> would have read
/// for itself.
/// </summary>
[Injectable]
public class RagfairLinkedItemNativeRequestBuilder(TemplateTable templateTable)
{
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
