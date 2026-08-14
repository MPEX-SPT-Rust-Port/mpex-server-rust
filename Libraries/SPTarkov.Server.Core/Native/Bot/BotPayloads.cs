using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Loot;

namespace SPTarkov.Server.Core.Native.Bot;

/// <summary>
/// The request/response envelope of <c>spt_generate_bot_inventory</c>, mirroring
/// <c>rust/spt-native/src/bot/models.rs</c> member for member.
///
/// Database and config models are the existing records from <c>Models</c>, whose
/// <c>JsonPropertyName</c>s are what the Rust wire names were pinned to, so their shape stays
/// authoritative by construction. Only the blocks that have no such record - the details slice,
/// whose C# wire name is <c>BotRoleLowercase</c>, and the narrowed <c>BotType</c> - are declared
/// here.
///
/// Members Rust declares as <c>Option&lt;T&gt;</c> are nullable, everything else is
/// <c>required</c>: <see cref="Utils.JsonUtil"/> serialises with
/// <see cref="JsonIgnoreCondition.WhenWritingNull"/>, so a null member is omitted and a Rust member
/// that is not an <c>Option</c> would fail the parse.
/// </summary>
internal record GenerateBotInventoryRequest
{
    /// <summary>
    /// Carried but unread natively - nothing on the far side keys anything by it.
    /// </summary>
    [JsonPropertyName("botId")]
    public required MongoId BotId { get; set; }

    /// <summary>
    /// Test-only: draws on the native side come from a seeded generator when set. Null - and
    /// therefore omitted from the wire JSON - on the production path.
    /// </summary>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }

    [JsonPropertyName("details")]
    public required BotGenerationDetailsView Details { get; set; }

    [JsonPropertyName("template")]
    public required BotTemplateView Template { get; set; }

    /// <summary>
    /// <c>pmcProfile?.Info?.Level ?? 1</c> (<c>BotInventoryGenerator.cs:260</c>). Unread natively -
    /// it exists so the level <see cref="EquipmentBlacklist"/> was resolved with is on the wire.
    /// </summary>
    [JsonPropertyName("generatingPlayerLevel")]
    public required int GeneratingPlayerLevel { get; set; }

    /// <summary>
    /// <c>WeatherHelper.IsNightTime</c> for the session's raid; day when there is no raid.
    /// </summary>
    [JsonPropertyName("isNightTime")]
    public required bool IsNightTime { get; set; }

    /// <summary>
    /// <c>BotConfig.Equipment</c> - the whole map, keyed by equipment role, not one resolved entry.
    /// </summary>
    [JsonPropertyName("equipment")]
    public required Dictionary<string, EquipmentFilters> Equipment { get; set; }

    [JsonPropertyName("bosses")]
    public required List<string> Bosses { get; set; }

    [JsonPropertyName("durability")]
    public required BotDurability Durability { get; set; }

    [JsonPropertyName("itemSpawnLimits")]
    public required Dictionary<string, Dictionary<MongoId, double>> ItemSpawnLimits { get; set; }

    [JsonPropertyName("walletLoot")]
    public required WalletLootSettings WalletLoot { get; set; }

    /// <summary>
    /// Bot role -> money tpl -> stack size -> weight.
    /// </summary>
    [JsonPropertyName("currencyStackSize")]
    public required Dictionary<string, Dictionary<string, Dictionary<string, double>>> CurrencyStackSize { get; set; }

    [JsonPropertyName("secureContainerAmmoStackCount")]
    public required int SecureContainerAmmoStackCount { get; set; }

    [JsonPropertyName("disableLootOnBotTypes")]
    public required HashSet<string> DisableLootOnBotTypes { get; set; }

    [JsonPropertyName("lowProfileGasBlockTpls")]
    public required HashSet<MongoId> LowProfileGasBlockTpls { get; set; }

    [JsonPropertyName("lootItemResourceRandomization")]
    public required Dictionary<string, RandomisedResourceDetails> LootItemResourceRandomization { get; set; }

    [JsonPropertyName("pmcConfig")]
    public required PmcConfig PmcConfig { get; set; }

    /// <summary>
    /// <c>RepairConfig.RepairKit.Weapon</c> - the only <c>BonusSettings</c> bot generation uses.
    /// </summary>
    [JsonPropertyName("repairKitWeapon")]
    public required BonusSettings RepairKitWeapon { get; set; }

    /// <summary>
    /// <c>GetBotEquipmentBlacklist(equipmentRole, generatingPlayerLevel)</c>, resolved by the caller.
    /// An empty instance rather than null when the bot has no blacklist for that level.
    /// </summary>
    [JsonPropertyName("equipmentBlacklist")]
    public required EquipmentFilterDetails EquipmentBlacklist { get; set; }

    /// <summary>
    /// The resolved <c>BotLootCacheService</c> pools.
    /// </summary>
    [JsonPropertyName("lootPools")]
    public required BotLootCache LootPools { get; set; }

    /// <summary>
    /// <c>GlobalTable.ItemPresets</c>, keyed by preset id. Scanned in order by the preset-weapon
    /// lookup, so the insertion order of the source dictionary is the contract. This is also the map
    /// <c>PresetHelper.GetPreset(id)</c> reads, so the native side resolves both against it.
    /// </summary>
    [JsonPropertyName("itemPresets")]
    public required Dictionary<MongoId, PresetView> ItemPresets { get; set; }

    /// <summary>
    /// Which preset is the default for a tpl, as its id rather than the preset itself -
    /// <c>PresetHelper</c> resolves every default out of <c>GlobalTable.ItemPresets</c>, so the
    /// native side looks the id up in <see cref="ItemPresets"/>. Inlining the presets here put ~0.26
    /// MiB of duplicate on a wire that is rebuilt per bot.
    /// </summary>
    [JsonPropertyName("defaultPresetsByTpl")]
    public required Dictionary<MongoId, MongoId> DefaultPresetsByTpl { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetBlacklistedItems()</c>.
    /// </summary>
    [JsonPropertyName("configBlacklist")]
    public required HashSet<MongoId> ConfigBlacklist { get; set; }

    /// <summary>
    /// <c>HandbookHelper.GetTemplatePrice</c> over every tpl in <see cref="LootPools"/> - the only
    /// tpls the native side prices.
    /// </summary>
    [JsonPropertyName("handbookPrices")]
    public required Dictionary<MongoId, double> HandbookPrices { get; set; }

    /// <inheritdoc cref="LootCommon.ItemsView"/>
    [JsonPropertyName("items")]
    public required Dictionary<MongoId, ItemView> Items { get; set; }
}

/// <summary>
/// One wave in one call: the shared views once, then a slice per bot. The envelope of
/// <c>spt_generate_bot_inventory_batch</c>.
/// </summary>
internal record GenerateBotInventoryBatchRequest
{
    [JsonPropertyName("shared")]
    public required SharedBotViews Shared { get; set; }

    [JsonPropertyName("bots")]
    public required List<BotSlice> Bots { get; set; }
}

/// <summary>
/// The <see cref="GenerateBotInventoryRequest"/> members that do not vary between the bots of one
/// wave - every database view, every config slice, and the blacklist resolved from the wave's role
/// and the player's level. 95.7% of a single-bot request's bytes by measurement, which is what
/// makes batching worth anything.
/// </summary>
internal record SharedBotViews
{
    /// <inheritdoc cref="GenerateBotInventoryRequest.GeneratingPlayerLevel"/>
    [JsonPropertyName("generatingPlayerLevel")]
    public required int GeneratingPlayerLevel { get; set; }

    [JsonPropertyName("isNightTime")]
    public required bool IsNightTime { get; set; }

    [JsonPropertyName("equipment")]
    public required Dictionary<string, EquipmentFilters> Equipment { get; set; }

    [JsonPropertyName("bosses")]
    public required List<string> Bosses { get; set; }

    [JsonPropertyName("durability")]
    public required BotDurability Durability { get; set; }

    [JsonPropertyName("itemSpawnLimits")]
    public required Dictionary<string, Dictionary<MongoId, double>> ItemSpawnLimits { get; set; }

    [JsonPropertyName("walletLoot")]
    public required WalletLootSettings WalletLoot { get; set; }

    [JsonPropertyName("currencyStackSize")]
    public required Dictionary<string, Dictionary<string, Dictionary<string, double>>> CurrencyStackSize { get; set; }

    [JsonPropertyName("secureContainerAmmoStackCount")]
    public required int SecureContainerAmmoStackCount { get; set; }

    [JsonPropertyName("disableLootOnBotTypes")]
    public required HashSet<string> DisableLootOnBotTypes { get; set; }

    [JsonPropertyName("lowProfileGasBlockTpls")]
    public required HashSet<MongoId> LowProfileGasBlockTpls { get; set; }

    [JsonPropertyName("lootItemResourceRandomization")]
    public required Dictionary<string, RandomisedResourceDetails> LootItemResourceRandomization { get; set; }

    [JsonPropertyName("pmcConfig")]
    public required PmcConfig PmcConfig { get; set; }

    [JsonPropertyName("repairKitWeapon")]
    public required BonusSettings RepairKitWeapon { get; set; }

    [JsonPropertyName("equipmentBlacklist")]
    public required EquipmentFilterDetails EquipmentBlacklist { get; set; }

    [JsonPropertyName("itemPresets")]
    public required Dictionary<MongoId, PresetView> ItemPresets { get; set; }

    /// <inheritdoc cref="GenerateBotInventoryRequest.DefaultPresetsByTpl"/>
    [JsonPropertyName("defaultPresetsByTpl")]
    public required Dictionary<MongoId, MongoId> DefaultPresetsByTpl { get; set; }

    [JsonPropertyName("configBlacklist")]
    public required HashSet<MongoId> ConfigBlacklist { get; set; }

    /// <inheritdoc cref="LootCommon.ItemsView"/>
    [JsonPropertyName("items")]
    public required Dictionary<MongoId, ItemView> Items { get; set; }
}

/// <summary>
/// The <see cref="GenerateBotInventoryRequest"/> members that do vary per bot.
/// <see cref="Template"/> is per-bot because <c>BotEquipmentFilterService.FilterBotEquipment</c>
/// mutates a fresh clone for each one, and the two loot members because the price bands are
/// resolved from the bot's own level.
/// </summary>
internal record BotSlice
{
    [JsonPropertyName("botId")]
    public required MongoId BotId { get; set; }

    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }

    [JsonPropertyName("details")]
    public required BotGenerationDetailsView Details { get; set; }

    [JsonPropertyName("template")]
    public required BotTemplateView Template { get; set; }

    [JsonPropertyName("lootPools")]
    public required BotLootCache LootPools { get; set; }

    [JsonPropertyName("handbookPrices")]
    public required Dictionary<MongoId, double> HandbookPrices { get; set; }
}

/// <summary>
/// One envelope per requested bot, in request order.
/// </summary>
internal record BotInventoryBatchResult
{
    [JsonPropertyName("bots")]
    public required List<BotResultEnvelope> Bots { get; set; }
}

/// <summary>
/// Exactly one of <see cref="Result"/> or <see cref="Error"/> is set. A failed bot is skipped
/// with a Critical log, the same choice <c>BotController.TryGenerateSingleBot</c> makes.
/// </summary>
internal record BotResultEnvelope
{
    [JsonPropertyName("result")]
    public BotInventoryResult? Result { get; set; }

    [JsonPropertyName("error")]
    public string? Error { get; set; }
}

/// <summary>
/// The <c>BotGenerationDetails</c> members bot generation reads. Declared rather than reused:
/// <c>BotGenerationDetails.RoleLowercase</c> rides as <c>BotRoleLowercase</c> on its own wire, and
/// the request envelope is plain camelCase.
/// </summary>
internal record BotGenerationDetailsView
{
    [JsonPropertyName("role")]
    public required string Role { get; set; }

    [JsonPropertyName("roleLowercase")]
    public required string RoleLowercase { get; set; }

    [JsonPropertyName("side")]
    public required string Side { get; set; }

    [JsonPropertyName("botLevel")]
    public required int BotLevel { get; set; }

    [JsonPropertyName("isPmc")]
    public required bool IsPmc { get; set; }

    [JsonPropertyName("isPlayerScav")]
    public required bool IsPlayerScav { get; set; }

    [JsonPropertyName("gameVersion")]
    public required string GameVersion { get; set; }

    [JsonPropertyName("location")]
    public string? Location { get; set; }

    [JsonPropertyName("botDifficulty")]
    public required string BotDifficulty { get; set; }

    [JsonPropertyName("clearBotContainerCacheAfterGeneration")]
    public required bool ClearBotContainerCacheAfterGeneration { get; set; }
}

/// <summary>
/// The three <c>BotType</c> blocks the inventory generator reads. The rest of the record -
/// appearance, difficulty, experience, health, names, skills - is several times the size of these
/// three and is read by nothing on the far side, so it is left off the wire.
/// </summary>
internal record BotTemplateView
{
    [JsonPropertyName("inventory")]
    public required BotTypeInventoryView Inventory { get; set; }

    [JsonPropertyName("chances")]
    public required Chances Chances { get; set; }

    [JsonPropertyName("generation")]
    public required Generation Generation { get; set; }
}

/// <summary>
/// <c>BotTypeInventory</c> with its equipment pools re-keyed. The C# record keys them by the
/// <see cref="EquipmentSlots"/> enum, and System.Text.Json writes enum dictionary keys as their
/// <i>numeric</i> value - which would leave the native side unable to find a pool for any slot and
/// quietly produce an unequipped bot. The other three members keep their C# wire names, <c>Ammo</c>
/// included, which is PascalCase because its property carries no <c>JsonPropertyName</c>.
/// </summary>
internal record BotTypeInventoryView
{
    [JsonPropertyName("equipment")]
    public required Dictionary<string, Dictionary<MongoId, double>> Equipment { get; set; }

    [JsonPropertyName("Ammo")]
    public required Dictionary<string, Dictionary<MongoId, double>> Ammo { get; set; }

    [JsonPropertyName("items")]
    public required ItemPools Items { get; set; }

    [JsonPropertyName("mods")]
    public required GlobalMods Mods { get; set; }
}

internal record BotInventoryResult
{
    [JsonPropertyName("inventory")]
    public required BotBaseInventory Inventory { get; set; }

    [JsonPropertyName("diagnostics")]
    public required List<Diagnostic> Diagnostics { get; set; }

    /// <summary>
    /// Equipment slot <i>name</i> - not the enum, which System.Text.Json keys numerically - to the
    /// container state <c>BotInventoryContainerService</c> would have cached. Empty when the request
    /// asked for the cache to be cleared.
    /// </summary>
    [JsonPropertyName("containerGrids")]
    public required Dictionary<string, ContainerDetailsView> ContainerGrids { get; set; }

    /// <summary>
    /// Equipment <i>mod</i> slot -> the chance the nighttime clamp left behind, for the caller to
    /// write back into its shared <c>BotConfig</c> object.
    /// </summary>
    [JsonPropertyName("randomisationClamps")]
    public required Dictionary<string, double> RandomisationClamps { get; set; }
}

/// <summary>
/// <c>BotInventoryContainerService.ContainerDetails</c>, with its two item references as ids.
/// </summary>
internal record ContainerDetailsView
{
    [JsonPropertyName("containerTpl")]
    public required MongoId ContainerTpl { get; set; }

    [JsonPropertyName("containerItemId")]
    public required MongoId ContainerItemId { get; set; }

    [JsonPropertyName("grids")]
    public required List<ContainerMapDetailsView> Grids { get; set; }
}

/// <summary>
/// <c>BotInventoryContainerService.ContainerMapDetails</c>. <see cref="GridMap"/> is
/// <c>int[CellsV, CellsH]</c> as rows of columns, <c>1</c> = occupied.
/// </summary>
internal record ContainerMapDetailsView
{
    [JsonPropertyName("gridMap")]
    public required List<List<int>> GridMap { get; set; }

    [JsonPropertyName("gridFull")]
    public required bool GridFull { get; set; }
}
