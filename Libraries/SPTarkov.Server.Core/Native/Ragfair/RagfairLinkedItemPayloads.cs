using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;

namespace SPTarkov.Server.Core.Native.Ragfair;

/// <summary>
/// The request/response envelopes of <c>spt_build_ragfair_linked_item_table</c>, mirroring
/// <c>rust/spt-native/src/linked_items.rs</c> member for member. Conventions are
/// <c>ItemBaseClassRequest</c>'s: an explicit <see cref="JsonPropertyNameAttribute"/> on every
/// member, members Rust declares as <c>Option&lt;T&gt;</c> nullable and everything else
/// <c>required</c>.
/// </summary>
public record RagfairLinkedItemRequest
{
    /// <summary>
    /// <c>templateTable.Items</c>, projected to the four members the walk reads. The whole table
    /// crosses, templates without <c>_props</c> included: each seeds an empty linked set
    /// (<c>RagfairLinkedItemService.cs:67</c>) that <c>GetLinkedItems</c> answers with.
    /// </summary>
    [JsonPropertyName("itemsView")]
    public required Dictionary<MongoId, RagfairLinkedItemView> ItemsView { get; init; }
}

/// <summary>
/// <c>TemplateItem</c> reduced to the four members the walk reads. Arrives on the native side as
/// the shared <c>ItemView</c>, whose remaining members are all <c>Option</c>s and land absent.
/// </summary>
public record RagfairLinkedItemView
{
    /// <summary>
    /// <c>TemplateItem.Parent</c> - only the revolver special case reads it
    /// (<c>RagfairLinkedItemService.cs:98</c>).
    /// </summary>
    [JsonPropertyName("parent")]
    public MongoId? Parent { get; init; }

    /// <summary>
    /// <c>Properties.Slots</c> - <c>GetSlotFilters</c>, and the cylinder lookup's
    /// <c>mod_magazine</c> match.
    /// </summary>
    [JsonPropertyName("slots")]
    public List<RagfairLinkedSlotView>? Slots { get; init; }

    /// <summary>
    /// <c>Properties.Chambers</c> - <c>GetChamberFilters</c>.
    /// </summary>
    [JsonPropertyName("chambers")]
    public List<RagfairLinkedSlotView>? Chambers { get; init; }

    /// <summary>
    /// <c>Properties.Cartridges</c> - <c>GetCartridgeFilters</c>.
    /// </summary>
    [JsonPropertyName("cartridges")]
    public List<RagfairLinkedSlotView>? Cartridges { get; init; }
}

/// <summary>
/// A <c>Slot</c> flattened onto the union of all its filter groups, in order - not
/// <c>Filters[0].Filter</c> like <c>PayloadProjection.ToSlotViews</c> (<c>LootPayloads.cs:319</c>) -
/// because the walk unions every group (<c>RagfairLinkedItemService.cs:163-166</c>).
/// </summary>
public record RagfairLinkedSlotView
{
    /// <summary>
    /// <c>Slot.Name</c>, filled on <c>Slots</c> alone: only the revolver cylinder lookup reads it
    /// (<c>RagfairLinkedItemService.cs:119</c>).
    /// </summary>
    [JsonPropertyName("name")]
    public string? Name { get; init; }

    /// <summary>
    /// Every <c>Properties.Filters</c> group's <c>Filter</c> ids, flattened in order. A
    /// <see cref="List{T}"/> rather than a set because order is contract: the revolver cylinder
    /// lookup takes the <b>first</b> element (<c>RagfairLinkedItemService.cs:126</c>).
    /// </summary>
    [JsonPropertyName("filter")]
    public List<MongoId>? Filter { get; init; }
}

/// <summary>
/// The one field <c>RagfairLinkedItemService.BuildLinkedItemTable</c> fills.
/// </summary>
public record RagfairLinkedItemResult
{
    /// <summary>
    /// <c>_linkedItemsCache</c> - key = item tpl, values = the tpls linked to it. Reverse edges may
    /// key entries the items table itself never held.
    /// </summary>
    [JsonPropertyName("linkedItems")]
    public required Dictionary<MongoId, HashSet<MongoId>> LinkedItems { get; init; }
}

/// <summary>
/// The success envelope the export writes, unwrapped by
/// <see cref="SptNative.BuildRagfairLinkedItemTable"/> - a failure is a non-zero status and a plain
/// message, never a JSON error object.
/// </summary>
internal record RagfairLinkedItemResponse
{
    [JsonPropertyName("result")]
    public required RagfairLinkedItemResult Result { get; init; }
}
