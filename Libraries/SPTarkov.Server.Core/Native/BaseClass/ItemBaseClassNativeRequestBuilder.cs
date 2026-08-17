using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Tables;

namespace SPTarkov.Server.Core.Native.BaseClass;

/// <summary>
/// Assembles the <see cref="ItemBaseClassRequest"/> for one bulk cache build out of the live items
/// table - everything <c>ItemBaseClassService.HydrateItemBaseClassCache</c> would have read for
/// itself.
/// </summary>
[Injectable]
public class ItemBaseClassNativeRequestBuilder(TemplateTable templateTable)
{
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
