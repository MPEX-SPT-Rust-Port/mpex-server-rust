using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;

namespace SPTarkov.Server.Core.Models.Spt.Config;

public record PlayerScavConfig : BaseConfig
{
    [JsonPropertyName("kind")]
    public override string Kind { get; set; } = "spt-playerscav";

    [JsonPropertyName("karmaLevel")]
    public required Dictionary<string, KarmaLevel> KarmaLevel { get; set; }

    /// <summary>
    ///     Force player scav generation down the retained 4.1.2 C# path instead of spt-native.
    ///     The escape hatch for hooks the patch detection cannot see - patches on the shared helpers
    ///     the generator calls into.
    /// </summary>
    [JsonPropertyName("forceLegacyPlayerScavGeneration")]
    public bool ForceLegacyPlayerScavGeneration { get; set; }

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

public record KarmaLevel
{
    [JsonPropertyName("botTypeForLoot")]
    public required string BotTypeForLoot { get; set; }

    [JsonPropertyName("modifiers")]
    public required Modifiers Modifiers { get; set; }

    [JsonPropertyName("itemLimits")]
    public required Dictionary<string, GenerationData> ItemLimits { get; set; }

    [JsonPropertyName("equipmentBlacklist")]
    public required Dictionary<EquipmentSlots, List<MongoId>> EquipmentBlacklist { get; set; }

    [JsonPropertyName("lootItemsToAddChancePercent")]
    public required Dictionary<MongoId, double> LootItemsToAddChancePercent { get; set; }
}

public record Modifiers
{
    [JsonPropertyName("equipment")]
    public required Dictionary<string, double> Equipment { get; set; }

    [JsonPropertyName("mod")]
    public required Dictionary<string, double> Mod { get; set; }
}
