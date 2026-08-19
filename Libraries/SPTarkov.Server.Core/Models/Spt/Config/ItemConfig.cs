using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Tables;

namespace SPTarkov.Server.Core.Models.Spt.Config;

public record ItemConfig : BaseConfig
{
    [JsonPropertyName("kind")]
    public override string Kind { get; set; } = "spt-item";

    /// <summary>
    ///     Items that should be globally blacklisted
    /// </summary>
    [JsonPropertyName("blacklist")]
    public required HashSet<MongoId> Blacklist { get; set; }

    /// <summary>
    ///     Items that should not be lootable from any location
    /// </summary>
    [JsonPropertyName("lootableItemBlacklist")]
    public required HashSet<MongoId> LootableItemBlacklist { get; set; }

    /// <summary>
    ///     items that should not be given as rewards
    /// </summary>
    [JsonPropertyName("rewardItemBlacklist")]
    public required HashSet<MongoId> RewardItemBlacklist { get; set; }

    /// <summary>
    ///     Item base types that should not be given as rewards
    /// </summary>
    [JsonPropertyName("rewardItemTypeBlacklist")]
    public required HashSet<MongoId> RewardItemTypeBlacklist { get; set; }

    /// <summary>
    ///     Items that can only be found on bosses
    /// </summary>
    [JsonPropertyName("bossItems")]
    public required HashSet<MongoId> BossItems { get; set; }

    [JsonPropertyName("handbookPriceOverride")]
    public required Dictionary<MongoId, HandbookPriceOverride> HandbookPriceOverride { get; set; }

    /// <summary>
    ///     Presets to add to the globals.json `ItemPresets` dictionary on server start
    /// </summary>
    [JsonPropertyName("customItemGlobalPresets")]
    public required List<Preset> CustomItemGlobalPresets { get; set; }

    /// <summary>
    ///     Force the item base class cache build down the retained 4.1.2 C# path instead of
    ///     spt-native. The escape hatch for hooks the patch detection cannot see - patches on the
    ///     shared helpers the service calls into.
    /// </summary>
    [JsonPropertyName("forceLegacyItemBaseClassHydration")]
    public bool ForceLegacyItemBaseClassHydration { get; set; }

    /// <summary>
    ///     Keep the native resident-DB fast path live with mods loaded. On by default since the
    ///     Phase 2 write barriers: the Ceciler-injected barriers on the model setters reachable
    ///     from the published roots make a mod's scalar writes visible to the mutation stamp
    ///     without any hand-written bump. Still invisible: container mutations - a mod calling
    ///     Add/Remove/indexer-set on a table collection - and reflection-driven writes. Turn this
    ///     off if a mod of yours does that and you see stale game data.
    /// </summary>
    [JsonPropertyName("trustNativeRequestCacheWithMods")]
    public bool TrustNativeRequestCacheWithMods { get; set; } = true;

    /// <summary>
    ///     Always send the C#-built views override: disables the resident-DB fast path without
    ///     touching the native path itself.
    /// </summary>
    [JsonPropertyName("disableNativeRequestCache")]
    public bool DisableNativeRequestCache { get; set; }
}

public record HandbookPriceOverride
{
    /// <summary>
    ///     Price in roubles
    /// </summary>
    [JsonPropertyName("price")]
    public double Price { get; set; }

    /// <summary>
    ///     NOT parentId from items.json, but handbook.json
    /// </summary>
    [JsonPropertyName("parentId")]
    public MongoId ParentId { get; set; } = MongoId.Empty();
}
