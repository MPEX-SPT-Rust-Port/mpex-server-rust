using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;

namespace SPTarkov.Server.Core.Native.BaseClass;

/// <summary>
/// The request/response envelopes of <c>spt_build_item_base_class_cache</c>, mirroring
/// <c>rust/spt-native/src/base_class.rs</c> member for member. Conventions are
/// <c>ScavCaseRewardsRequest</c>'s: an explicit <see cref="JsonPropertyNameAttribute"/> on every
/// member, members Rust declares as <c>Option&lt;T&gt;</c> nullable and everything else
/// <c>required</c>.
/// </summary>
public record ItemBaseClassRequest
{
    /// <summary>
    /// <c>templateTable.Items</c>, projected to the two members the parent walk reads. The whole
    /// table crosses, <c>Node</c>s included: a chain climbs through them, so dropping them first
    /// would cut every chain at its first node.
    /// </summary>
    [JsonPropertyName("itemsView")]
    public required Dictionary<MongoId, ItemBaseClassItemView> ItemsView { get; init; }
}

/// <summary>
/// <c>TemplateItem</c> reduced to the two members the walk reads. Arrives on the native side as the
/// shared <c>ItemView</c>, whose remaining members are all <c>Option</c>s and land absent.
/// </summary>
public record ItemBaseClassItemView
{
    [JsonPropertyName("parent")]
    public MongoId? Parent { get; init; }

    [JsonPropertyName("type")]
    public string? Type { get; init; }
}

/// <summary>
/// The two fields <c>ItemBaseClassService.HydrateItemBaseClassCache</c> fills.
/// </summary>
public record ItemBaseClassResult
{
    /// <summary>
    /// <c>_itemBaseClassesCache</c> - key = item tpl, values = the ids of its parents.
    /// </summary>
    [JsonPropertyName("itemBaseClasses")]
    public required Dictionary<MongoId, HashSet<MongoId>> ItemBaseClasses { get; init; }

    /// <summary>
    /// <c>_rootNodeIds</c> - every tpl that failed the <c>_type == "Item"</c> test.
    /// </summary>
    [JsonPropertyName("rootNodeIds")]
    public required HashSet<MongoId> RootNodeIds { get; init; }
}

/// <summary>
/// The success envelope the export writes, unwrapped by
/// <see cref="SptNative.BuildItemBaseClassCache"/> - a failure is a non-zero status and a plain
/// message, never a JSON error object.
/// </summary>
internal record ItemBaseClassResponse
{
    [JsonPropertyName("result")]
    public required ItemBaseClassResult Result { get; init; }
}
