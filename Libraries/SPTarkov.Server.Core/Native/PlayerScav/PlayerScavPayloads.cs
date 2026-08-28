using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Native.Bot;

namespace SPTarkov.Server.Core.Native.PlayerScav;

/// <summary>
///     spt_generate_player_scav request: the single-bot request members plus the karma slice.
///     The template's generation arrives already karma-adjusted C#-side (item limits feed
///     BotLootCacheService hydration); chances and inventory arrive raw for the native pieces.
/// </summary>
internal record GeneratePlayerScavRequest
{
    /// <inheritdoc cref="GenerateBotInventoryRequest.Epoch"/>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <inheritdoc cref="GenerateBotInventoryRequest.ViewsOverride"/>
    [JsonPropertyName("viewsOverride")]
    public BotViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("shared")]
    public required SharedBotVarying Shared { get; set; }

    [JsonPropertyName("bot")]
    public required BotSlice Bot { get; set; }

    [JsonPropertyName("template")]
    public required BotTemplateView Template { get; set; }

    /// <inheritdoc cref="GenerateBotInventoryRequest.LootPools"/>
    [JsonPropertyName("lootPools")]
    public required BotLootCache LootPools { get; set; }

    [JsonPropertyName("karma")]
    public required KarmaSettingsView Karma { get; set; }
}

/// <summary>
///     The native slice of one KarmaLevel: modifiers, equipment blacklist (re-keyed from
///     EquipmentSlots to slot.ToString() - STJ writes enum keys numerically), and the
///     additional-loot chances. ItemLimits deliberately stays off the wire: it is applied
///     C#-side to the template's generation block before the request is built.
/// </summary>
internal record KarmaSettingsView
{
    [JsonPropertyName("equipmentModifiers")]
    public required Dictionary<string, double> EquipmentModifiers { get; set; }

    [JsonPropertyName("modModifiers")]
    public required Dictionary<string, double> ModModifiers { get; set; }

    [JsonPropertyName("equipmentBlacklist")]
    public required Dictionary<string, List<MongoId>> EquipmentBlacklist { get; set; }

    [JsonPropertyName("lootItemsToAddChancePercent")]
    public required Dictionary<MongoId, double> LootItemsToAddChancePercent { get; set; }
}
