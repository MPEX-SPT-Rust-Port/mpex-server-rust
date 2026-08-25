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
    /// Resident-DB epoch this request was built against; 0 with <see cref="ViewsOverride"/> set.
    /// Checked natively only when the override is absent - a mismatch is a stale-epoch answer.
    /// </summary>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <summary>
    /// The C#-built database views. When set, the resident store is never consulted; when null
    /// (omitted from the wire), the native side reads the same views off the resident DB.
    /// </summary>
    [JsonPropertyName("viewsOverride")]
    public BotViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("shared")]
    public required SharedBotVarying Shared { get; set; }

    [JsonPropertyName("bot")]
    public required BotSlice Bot { get; set; }

    [JsonPropertyName("template")]
    public required BotTemplateView Template { get; set; }

    /// <summary>
    /// The resolved <c>BotLootCacheService</c> pools.
    /// </summary>
    [JsonPropertyName("lootPools")]
    public required BotLootCache LootPools { get; set; }
}

/// <summary>
/// One wave in one call: the shared varying block once, then a slice per bot. The envelope of
/// <c>spt_generate_bot_inventory_batch</c>. Epoch and override semantics as on
/// <see cref="GenerateBotInventoryRequest"/>.
/// </summary>
internal record GenerateBotInventoryBatchRequest
{
    /// <inheritdoc cref="GenerateBotInventoryRequest.Epoch"/>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <inheritdoc cref="GenerateBotInventoryRequest.ViewsOverride"/>
    [JsonPropertyName("viewsOverride")]
    public BotViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("shared")]
    public required SharedBotVarying Shared { get; set; }

    [JsonPropertyName("bots")]
    public required List<BotSlice> Bots { get; set; }
}

/// <summary>
/// The database half of both bot requests - <c>BotViewsWire</c> on the Rust side. An eligible
/// send omits it and the native side reads the same views off the resident DB instead
/// (<c>rust/spt-native/src/bot/views.rs</c>, derived at publish).
/// </summary>
internal record BotViewsOverride
{
    /// <inheritdoc cref="LootCommon.ItemsView"/>
    [JsonPropertyName("items")]
    public required Dictionary<MongoId, ItemView> Items { get; set; }

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
    /// native side looks the id up in <see cref="ItemPresets"/>.
    /// </summary>
    [JsonPropertyName("defaultPresetsByTpl")]
    public required Dictionary<MongoId, MongoId> DefaultPresetsByTpl { get; set; }

    /// <summary>
    /// <c>HandbookHelper.GetTemplatePrice</c> over every tpl the send's loot pools can draw - the
    /// union of the per-pool maps, collision-safe because every key maps to the same
    /// <c>GetTemplatePrice</c> value. A key missing from the map prices at 0 natively, which is
    /// what <c>GetTemplatePrice</c> returns for a tpl the handbook does not know.
    /// </summary>
    [JsonPropertyName("handbookPrices")]
    public required Dictionary<MongoId, double> HandbookPrices { get; set; }

    /// <summary>
    /// <c>GlobalTable.Configuration.Exp.Level.ExperienceTable</c> projected to its exp values, in
    /// order - what the PMC level draw sums out of (<c>BotLevelGenerator.cs:39</c>).
    /// </summary>
    [JsonPropertyName("expTable")]
    public required List<int> ExpTable { get; set; }

    // The config slices an eligible send reads off the resident DB instead: the spt-bot stem
    // (BotConfigLift), the spt-pmc and spt-repair stems, and ItemConfig.Blacklist.

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

    /// <summary>
    /// <c>RepairConfig.RepairKit.Weapon</c> - the only <c>BonusSettings</c> bot generation passes.
    /// </summary>
    [JsonPropertyName("repairKitWeapon")]
    public required BonusSettings RepairKitWeapon { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetBlacklistedItems()</c>, i.e. <c>ItemConfig.Blacklist</c> verbatim -
    /// <i>not</i> the runtime-augmented <c>ItemBlacklistCache</c>, which is what keeps this member
    /// safe to resolve off a published snapshot.
    /// </summary>
    [JsonPropertyName("configBlacklist")]
    public required HashSet<MongoId> ConfigBlacklist { get; set; }
}

/// <summary>
/// The request members that do not vary between the bots of one wave and are not database views:
/// live C# process state (the player's level, the raid's daylight), the one config slice a runtime
/// writer keeps out of the resident DB (<see cref="Equipment"/>), and (as
/// <see cref="TemplateVariants"/>) the templates and loot pools, which vary by level band rather
/// than by bot. Every other config slice, and every database view, lives on
/// <see cref="BotViewsOverride"/> or the resident DB.
/// </summary>
internal record SharedBotVarying
{
    /// <summary>
    /// <c>pmcProfile?.Info?.Level</c> <b>raw</b>, not pre-defaulted: the equipment path defaults an
    /// absent level to 1 (written at <c>BotInventoryGenerator.cs:614</c> and its six siblings,
    /// reaching the blacklist call as <c>GetValueOrDefault(1)</c> at <c>:937-939</c>) and the
    /// weapon-mod path to <b>0</b> (<c>BotEquipmentModGenerator.cs:546</c>), and level 0 matches no
    /// <c>levelRange</c> where level 1 may, so a pre-defaulted <c>1</c> could not tell "level 1 with
    /// a profile" from "no profile" and would collapse that divergence. The native side applies both
    /// defaults and picks both blacklist bands out of <see cref="Equipment"/> itself.
    ///
    /// <c>required</c> like every sibling, though it is nullable: a construction site that forgot it
    /// would silently ship "no profile" and flip the weapon-mod list to the level-0 band.
    /// </summary>
    [JsonPropertyName("generatingPlayerLevel")]
    public required int? GeneratingPlayerLevel { get; set; }

    [JsonPropertyName("isNightTime")]
    public required bool IsNightTime { get; set; }

    /// <summary>
    /// <c>BotConfig.Equipment</c> minus its null values. Deliberately <b>not</b> resident:
    /// <see cref="BotInventoryGenerator.ReplayRandomisationClamps"/> writes the nighttime mod
    /// chances back into <c>Equipment[role].Randomisation[band].EquipmentMods</c> through the
    /// dictionary indexer after every native send, which trips no write barrier, so a published
    /// copy would freeze at the config's on-disk values and diverge from the second bot of a
    /// nighttime raid on.
    /// </summary>
    [JsonPropertyName("equipment")]
    public required Dictionary<string, EquipmentFilters> Equipment { get; set; }

    /// <summary>
    /// The wave's level-draw inputs, for the native side to draw each bot's level with. Set only
    /// when the wave is PMC - every other bot takes the constant level 1 without drawing
    /// (<c>BotLevelGenerator.cs:23-26</c>), so there is nothing to send and this stays null, which
    /// omits it from the wire.
    /// </summary>
    [JsonPropertyName("levelGeneration")]
    public LevelGenerationView? LevelGeneration { get; set; }

    /// <summary>
    /// Ascending, contiguous, covering <c>[LevelMin..LevelMax]</c> of
    /// <see cref="LevelGeneration"/>; exactly one <c>[1..1]</c> entry for a non-PMC wave. Null -
    /// and therefore omitted, which the Rust default tolerates - on the single-bot request, whose
    /// template and loot pools ride the envelope's own members instead.
    /// </summary>
    [JsonPropertyName("templateVariants")]
    public List<BotTemplateVariantView>? TemplateVariants { get; set; }
}

/// <summary>
/// <c>BotLevelGenerator.GenerateBotLevel</c>'s inputs for a whole wave: the range
/// <c>GetRelativePmcBotLevelRange</c> resolved from the wave's details
/// (<c>BotLevelGenerator.cs:67-101</c>). Plain ints rather than a <c>MinMax</c> envelope - nothing
/// on the far side reads them as one. The exp table the drawn level is turned into an exp total
/// with rides the views (<see cref="BotViewsOverride.ExpTable"/> or the resident DB) instead.
/// </summary>
internal record LevelGenerationView
{
    [JsonPropertyName("levelMin")]
    public required int LevelMin { get; set; }

    [JsonPropertyName("levelMax")]
    public required int LevelMax { get; set; }
}

/// <summary>
/// The template and the two loot views for one band of levels. Every level-dependent step the
/// prelude runs before the native call is a band lookup over inclusive ranges
/// (<c>BotEquipmentFilterService.cs:137-189</c>, <c>BotHelper.cs:83-90</c>,
/// <c>BotPayloadProjection.GetSingleItemLootPriceLimits</c>) and none of them draws, so the caller
/// runs the unchanged C# filter and pool hydration once per band on which all of them are constant
/// and ships one variant per band instead of one filtered template per bot.
/// </summary>
internal record BotTemplateVariantView
{
    [JsonPropertyName("levelMin")]
    public required int LevelMin { get; set; }

    [JsonPropertyName("levelMax")]
    public required int LevelMax { get; set; }

    [JsonPropertyName("template")]
    public required BotTemplateView Template { get; set; }

    [JsonPropertyName("lootPools")]
    public required BotLootCache LootPools { get; set; }
}

/// <summary>
/// The <see cref="GenerateBotInventoryRequest"/> members that do vary per bot: identity, the test
/// seed and the generation details. The template and the two loot views ride per level band on
/// <see cref="SharedBotVarying.TemplateVariants"/> instead, because the batch path draws the level
/// natively and picks the band that covers it.
///
/// <c>Details.BotLevel</c> still rides the wire - the single-bot request reuses the view - but the
/// batch projection sends 0 and the native side overwrites it with the drawn level before any
/// consumer reads it.
/// </summary>
internal record BotSlice
{
    [JsonPropertyName("botId")]
    public required MongoId BotId { get; set; }

    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }

    [JsonPropertyName("details")]
    public required BotGenerationDetailsView Details { get; set; }
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

    /// <summary>
    /// The level this bot drew, for the caller to write into <c>details.BotLevel</c> and
    /// <c>Info.Level</c> (<c>BotGenerator.cs:222-225</c>, <c>:270</c>). Set by the batch path only -
    /// the single-bot path keeps its C# level generation, so that response omits it.
    /// </summary>
    [JsonPropertyName("level")]
    public int? Level { get; set; }

    /// <summary>
    /// The experience total that goes with <see cref="Level"/>
    /// (<c>BotLevelGenerator.cs:39-44</c>) -> <c>Info.Experience</c>. Null alongside it, for the
    /// same reason.
    /// </summary>
    [JsonPropertyName("exp")]
    public int? Exp { get; set; }
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
