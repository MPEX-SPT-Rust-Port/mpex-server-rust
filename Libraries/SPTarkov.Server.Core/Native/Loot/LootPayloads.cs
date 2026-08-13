using System.Text.Json;
using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;

namespace SPTarkov.Server.Core.Native.Loot;

/// <summary>
/// The request/response envelopes of the two native loot exports, mirroring
/// <c>rust/spt-native/src/loot/models.rs</c> member for member. Database models are the existing
/// records from <c>Models.Eft.Common</c>, so their wire shape stays authoritative by construction;
/// only the projections the generator reads (<see cref="ItemView"/> and friends) are declared here.
///
/// Members Rust declares as <c>Option&lt;T&gt;</c> are nullable, everything else is
/// <c>required</c> - <see cref="Utils.JsonUtil"/> serialises with
/// <see cref="JsonIgnoreCondition.WhenWritingNull"/>, so a null member is omitted and Rust reads it
/// back as <c>None</c>.
/// </summary>
public record LootCommon
{
    /// <summary>
    /// Lowercased by the caller.
    /// </summary>
    [JsonPropertyName("locationId")]
    public required string LocationId { get; set; }

    /// <summary>
    /// The slice of every <c>TemplateItem</c> the generator reads, keyed by tpl. Templates without
    /// props have no representation here and must be left out.
    /// </summary>
    [JsonPropertyName("itemsView")]
    public required Dictionary<MongoId, ItemView> ItemsView { get; set; }

    [JsonPropertyName("defaultPresets")]
    public required Dictionary<MongoId, PresetView> DefaultPresets { get; set; }

    [JsonPropertyName("moneyTpls")]
    public required List<MongoId> MoneyTpls { get; set; }

    /// <summary>
    /// Keyed by caliber name, not by tpl.
    /// </summary>
    [JsonPropertyName("staticAmmoDist")]
    public required Dictionary<string, List<StaticAmmoDetails>> StaticAmmoDist { get; set; }

    [JsonPropertyName("config")]
    public required LootConfigView Config { get; set; }

    [JsonPropertyName("seasonal")]
    public required SeasonalView Seasonal { get; set; }

    [JsonPropertyName("lootableItemBlacklist")]
    public required HashSet<MongoId> LootableItemBlacklist { get; set; }

    [JsonPropertyName("counter")]
    public required CounterState Counter { get; set; }

    /// <summary>
    /// Test-only: draws on the native side come from a seeded generator when set. Null — and
    /// therefore omitted from the wire JSON — on the production path.
    /// </summary>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }
}

/// <summary>
/// A flattened <c>TemplateItem</c>: every member is a projection of a deeper C# path, so the
/// generator never walks <c>Props</c> itself.
/// </summary>
public record ItemView
{
    [JsonPropertyName("parent")]
    public MongoId? Parent { get; set; }

    [JsonPropertyName("width")]
    public int? Width { get; set; }

    [JsonPropertyName("height")]
    public int? Height { get; set; }

    [JsonPropertyName("stackMaxSize")]
    public int? StackMaxSize { get; set; }

    [JsonPropertyName("stackMinRandom")]
    public int? StackMinRandom { get; set; }

    [JsonPropertyName("stackMaxRandom")]
    public int? StackMaxRandom { get; set; }

    [JsonPropertyName("extraSizeUp")]
    public int? ExtraSizeUp { get; set; }

    [JsonPropertyName("extraSizeDown")]
    public int? ExtraSizeDown { get; set; }

    [JsonPropertyName("extraSizeLeft")]
    public int? ExtraSizeLeft { get; set; }

    [JsonPropertyName("extraSizeRight")]
    public int? ExtraSizeRight { get; set; }

    [JsonPropertyName("extraSizeForceAdd")]
    public bool? ExtraSizeForceAdd { get; set; }

    /// <summary>
    /// <c>Grids</c> first entry's <c>CellsH</c>.
    /// </summary>
    [JsonPropertyName("gridCellsH")]
    public int? GridCellsH { get; set; }

    /// <summary>
    /// <c>Grids</c> first entry's <c>CellsV</c>.
    /// </summary>
    [JsonPropertyName("gridCellsV")]
    public int? GridCellsV { get; set; }

    /// <summary>
    /// <c>StackSlots</c> first entry's <c>MaxCount</c>.
    /// </summary>
    [JsonPropertyName("stackSlotMaxCount")]
    public double? StackSlotMaxCount { get; set; }

    /// <summary>
    /// <c>StackSlots[0].Props.Filters[0].Filter</c> first entry.
    /// </summary>
    [JsonPropertyName("stackSlotFirstFilterFirst")]
    public MongoId? StackSlotFirstFilterFirst { get; set; }

    /// <summary>
    /// <c>Cartridges</c> first entry's <c>MaxCount</c>.
    /// </summary>
    [JsonPropertyName("cartridgesMaxCount")]
    public double? CartridgesMaxCount { get; set; }

    /// <summary>
    /// <c>Cartridges[0].Props.Filters[0].Filter</c>.
    /// </summary>
    [JsonPropertyName("cartridgesFirstFilter")]
    public HashSet<MongoId>? CartridgesFirstFilter { get; set; }

    /// <summary>
    /// <c>Chambers[0].Props.Filters[0].Filter</c>.
    /// </summary>
    [JsonPropertyName("chambersFirstFilter")]
    public HashSet<MongoId>? ChambersFirstFilter { get; set; }

    [JsonPropertyName("slots")]
    public List<SlotView>? Slots { get; set; }

    [JsonPropertyName("conflictingItems")]
    public HashSet<MongoId>? ConflictingItems { get; set; }

    [JsonPropertyName("caliber")]
    public string? Caliber { get; set; }

    [JsonPropertyName("ammoCaliber")]
    public string? AmmoCaliber { get; set; }

    [JsonPropertyName("defAmmo")]
    public MongoId? DefAmmo { get; set; }

    /// <summary>
    /// <c>TemplateItem.Name</c> - the template's <c>_name</c>, not a localised one.
    /// </summary>
    [JsonPropertyName("name")]
    public string? Name { get; set; }

    /// <summary>
    /// <c>TemplateItem.Type</c> - the template's <c>_type</c>.
    /// </summary>
    [JsonPropertyName("type")]
    public string? Type { get; set; }

    [JsonPropertyName("armorClass")]
    public int? ArmorClass { get; set; }

    /// <summary>
    /// Null and <c>false</c> are not interchangeable: the reward pool filters on one and the sealed
    /// container pool on the other.
    /// </summary>
    [JsonPropertyName("questItem")]
    public bool? QuestItem { get; set; }

    // Added for the bot port. Every one is nullable, so the loot payloads that never read them stay
    // absent from the wire and the loot side of the contract is unchanged.

    /// <summary>
    /// <c>Chambers</c> as slot objects - <see cref="ChambersFirstFilter"/> flattens the first
    /// chamber only. Projected verbatim: an empty list must stay an empty list, because the native
    /// <c>patron_in_weapon</c> lookup tells "no chambers" and "chambers with nothing in them" apart.
    /// </summary>
    [JsonPropertyName("chambers")]
    public List<SlotView>? Chambers { get; set; }

    /// <summary>
    /// <c>Cartridges</c> as slot objects - <see cref="CartridgesMaxCount"/> and
    /// <see cref="CartridgesFirstFilter"/> flatten the first cartridge only.
    /// </summary>
    [JsonPropertyName("cartridges")]
    public List<SlotView>? Cartridges { get; set; }

    /// <summary>
    /// The <c>ReloadMode</c> enum member name - the native side compares it against
    /// <c>OnlyBarrel</c> as a string.
    /// </summary>
    [JsonPropertyName("reloadMode")]
    public string? ReloadMode { get; set; }

    /// <summary>
    /// The <c>ReloadMagType</c> enum member name, compared against <c>InternalMagazine</c>.
    /// </summary>
    [JsonPropertyName("reloadMagType")]
    public string? ReloadMagType { get; set; }

    [JsonPropertyName("isChamberLoad")]
    public bool? IsChamberLoad { get; set; }

    [JsonPropertyName("defMagType")]
    public MongoId? DefMagType { get; set; }

    [JsonPropertyName("linkedWeapon")]
    public string? LinkedWeapon { get; set; }

    [JsonPropertyName("maxDurability")]
    public double? MaxDurability { get; set; }

    /// <summary>
    /// Only its presence is read - it is what marks a template "is a weapon".
    /// </summary>
    [JsonPropertyName("weapClass")]
    public string? WeapClass { get; set; }

    [JsonPropertyName("hasHinge")]
    public bool? HasHinge { get; set; }

    [JsonPropertyName("foldable")]
    public bool? Foldable { get; set; }

    [JsonPropertyName("foldedSlot")]
    public string? FoldedSlot { get; set; }

    [JsonPropertyName("sizeReduceRight")]
    public int? SizeReduceRight { get; set; }

    [JsonPropertyName("weapFireType")]
    public HashSet<string>? WeapFireType { get; set; }

    [JsonPropertyName("maxHpResource")]
    public int? MaxHpResource { get; set; }

    [JsonPropertyName("maxResource")]
    public int? MaxResource { get; set; }

    [JsonPropertyName("foodUseTime")]
    public double? FoodUseTime { get; set; }

    [JsonPropertyName("faceShieldComponent")]
    public bool? FaceShieldComponent { get; set; }

    [JsonPropertyName("blocksEarpiece")]
    public bool? BlocksEarpiece { get; set; }

    [JsonPropertyName("blocksEyewear")]
    public bool? BlocksEyewear { get; set; }

    [JsonPropertyName("blocksFaceCover")]
    public bool? BlocksFaceCover { get; set; }

    [JsonPropertyName("blocksHeadwear")]
    public bool? BlocksHeadwear { get; set; }

    [JsonPropertyName("blocksFolding")]
    public bool? BlocksFolding { get; set; }

    [JsonPropertyName("blocksCollapsible")]
    public bool? BlocksCollapsible { get; set; }

    /// <summary>
    /// The C# prop is <c>BlockLeftStance</c>, not <c>Blocks…</c>.
    /// </summary>
    [JsonPropertyName("blockLeftStance")]
    public bool? BlockLeftStance { get; set; }

    [JsonPropertyName("blocksArmorVest")]
    public bool? BlocksArmorVest { get; set; }

    /// <summary>
    /// Every grid, not just the first: a bot container is walked grid by grid.
    /// <see cref="GridCellsH"/> / <see cref="GridCellsV"/> flatten the first one.
    /// </summary>
    [JsonPropertyName("grids")]
    public List<GridView>? Grids { get; set; }
}

/// <summary>
/// A <c>Slot</c> flattened onto its first filter list.
/// </summary>
public record SlotView
{
    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("required")]
    public bool? Required { get; set; }

    /// <summary>
    /// <c>Props.Filters[0].Filter</c>.
    /// </summary>
    [JsonPropertyName("filter")]
    public HashSet<MongoId>? Filter { get; set; }

    /// <summary>
    /// <c>Props.Filters[0].Plate</c> - read by the bot port's default-plate lookup only.
    /// </summary>
    [JsonPropertyName("plate")]
    public MongoId? Plate { get; set; }
}

/// <summary>
/// A <c>Grid</c> flattened onto its <c>_props</c>.
/// </summary>
public record GridView
{
    /// <summary>
    /// <c>Grid._name</c> - becomes the placed item's <c>slotId</c>.
    /// </summary>
    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("cellsH")]
    public int? CellsH { get; set; }

    [JsonPropertyName("cellsV")]
    public int? CellsV { get; set; }

    [JsonPropertyName("filters")]
    public List<GridFilterView>? Filters { get; set; }
}

public record GridFilterView
{
    [JsonPropertyName("filter")]
    public HashSet<MongoId>? Filter { get; set; }

    [JsonPropertyName("excludedFilter")]
    public HashSet<MongoId>? ExcludedFilter { get; set; }
}

public record PresetView
{
    [JsonPropertyName("items")]
    public required List<Item> Items { get; set; }

    /// <summary>
    /// <c>Preset.Id</c>, read only for diagnostics.
    /// </summary>
    [JsonPropertyName("id")]
    public MongoId? Id { get; set; }

    /// <summary>
    /// <c>Preset.Name</c>, read only for diagnostics.
    /// </summary>
    [JsonPropertyName("name")]
    public string? Name { get; set; }

    /// <summary>
    /// <c>Preset.Encyclopedia</c> - the root tpl of a default preset.
    /// </summary>
    [JsonPropertyName("encyclopedia")]
    public MongoId? Encyclopedia { get; set; }
}

/// <summary>
/// Every config value the generator reads, resolved for one location by the caller.
/// </summary>
public record LootConfigView
{
    [JsonPropertyName("containerRandomisationEnabled")]
    public required bool ContainerRandomisationEnabled { get; set; }

    /// <summary>
    /// <c>ContainerRandomisationSettings.Maps.ContainsKey(locationId)</c>.
    /// </summary>
    [JsonPropertyName("locationInRandomisationMaps")]
    public required bool LocationInRandomisationMaps { get; set; }

    [JsonPropertyName("containerTypesToNotRandomise")]
    public required HashSet<MongoId> ContainerTypesToNotRandomise { get; set; }

    [JsonPropertyName("containerGroupMinSizeMultiplier")]
    public required double ContainerGroupMinSizeMultiplier { get; set; }

    [JsonPropertyName("containerGroupMaxSizeMultiplier")]
    public required double ContainerGroupMaxSizeMultiplier { get; set; }

    [JsonPropertyName("allowDuplicateItemsInStaticContainers")]
    public required bool AllowDuplicateItemsInStaticContainers { get; set; }

    [JsonPropertyName("tplsToStripChildItemsFrom")]
    public required HashSet<MongoId> TplsToStripChildItemsFrom { get; set; }

    [JsonPropertyName("fitLootIntoContainerAttempts")]
    public required int FitLootIntoContainerAttempts { get; set; }

    [JsonPropertyName("magazineLootHasAmmoChancePercent")]
    public required double MagazineLootHasAmmoChancePercent { get; set; }

    [JsonPropertyName("staticMagazineLootHasAmmoChancePercent")]
    public required double StaticMagazineLootHasAmmoChancePercent { get; set; }

    [JsonPropertyName("minFillLooseMagazinePercent")]
    public required double MinFillLooseMagazinePercent { get; set; }

    [JsonPropertyName("minFillStaticMagazinePercent")]
    public required double MinFillStaticMagazinePercent { get; set; }

    /// <summary>
    /// Resolved for this location by the caller.
    /// </summary>
    [JsonPropertyName("staticLootMultiplier")]
    public required double StaticLootMultiplier { get; set; }

    /// <summary>
    /// Resolved for this location by the caller.
    /// </summary>
    [JsonPropertyName("looseLootMultiplier")]
    public required double LooseLootMultiplier { get; set; }

    /// <summary>
    /// <c>EquipmentLootSettings.ModSpawnChancePercent</c>, keyed by slot name.
    /// </summary>
    [JsonPropertyName("modSpawnChancePercent")]
    public required Dictionary<string, double> ModSpawnChancePercent { get; set; }

    /// <summary>
    /// Resolved for this location by the caller. Holds spawn point template ids, not tpls.
    /// </summary>
    [JsonPropertyName("looseLootBlacklist")]
    public required HashSet<string> LooseLootBlacklist { get; set; }
}

public record SeasonalView
{
    [JsonPropertyName("seasonalEventActive")]
    public required bool SeasonalEventActive { get; set; }

    [JsonPropertyName("christmasEventEnabled")]
    public required bool ChristmasEventEnabled { get; set; }

    [JsonPropertyName("inactiveSeasonalItems")]
    public required HashSet<MongoId> InactiveSeasonalItems { get; set; }

    /// <summary>
    /// Spawn point ids, not tpls.
    /// </summary>
    [JsonPropertyName("christmasContainerIds")]
    public required HashSet<string> ChristmasContainerIds { get; set; }
}

/// <summary>
/// <c>CounterTrackerHelper</c>'s state, handed over the boundary and handed back on the result so
/// the two generation calls share one set of spawn limits.
/// </summary>
public record CounterState
{
    [JsonPropertyName("maxCounts")]
    public required Dictionary<MongoId, int> MaxCounts { get; set; }

    [JsonPropertyName("trackedCounts")]
    public required Dictionary<MongoId, int> TrackedCounts { get; set; }
}

public record StaticContainersRequest : LootCommon
{
    /// <summary>
    /// Null when the map's <c>StaticContainerDetails</c> is missing the list; the native side logs a
    /// map-specific error for each, so an empty list is not a substitute.
    /// </summary>
    [JsonPropertyName("staticWeapons")]
    public IEnumerable<SpawnpointTemplate>? StaticWeapons { get; set; }

    /// <inheritdoc cref="StaticWeapons"/>
    [JsonPropertyName("staticContainers")]
    public IEnumerable<StaticContainerData>? StaticContainers { get; set; }

    /// <inheritdoc cref="StaticWeapons"/>
    [JsonPropertyName("staticForced")]
    public IEnumerable<StaticForced>? StaticForced { get; set; }

    [JsonPropertyName("staticLootDist")]
    public required Dictionary<MongoId, StaticLootDetails> StaticLootDist { get; set; }

    [JsonPropertyName("statics")]
    public StaticContainer? Statics { get; set; }
}

public record DynamicLootRequest : LootCommon
{
    /// <summary>
    /// Either a <see cref="Models.Eft.Common.LooseLoot"/> - assign one directly, it converts - or the
    /// raw JSON of a location's <c>looseLoot.json</c>. Same wire shape either way.
    /// </summary>
    [JsonPropertyName("looseLoot")]
    public required LooseLootPayload LooseLoot { get; set; }
}

/// <summary>
/// The <c>looseLoot</c> member of <see cref="DynamicLootRequest"/> in one of its two forms: a
/// <see cref="Models.Eft.Common.LooseLoot"/> to serialise as usual, or the raw JSON of the location's
/// <c>looseLoot.json</c> to write through verbatim. The raw form exists because the typed form costs
/// a parse and a re-encode of 42 MB for bigmap; see <c>LocationLootGenerator.GenerateDynamicLoot</c>
/// for when each is used.
/// </summary>
[JsonConverter(typeof(LooseLootPayloadConverter))]
public sealed record LooseLootPayload
{
    private LooseLootPayload() { }

    public LooseLoot? Typed { get; private init; }

    /// <summary>
    /// UTF-8 JSON, written into the request unchanged.
    /// </summary>
    public ReadOnlyMemory<byte>? RawJson { get; private init; }

    public static LooseLootPayload FromRawJson(ReadOnlyMemory<byte> rawJson)
    {
        return new LooseLootPayload { RawJson = rawJson };
    }

    public static implicit operator LooseLootPayload(LooseLoot typed)
    {
        return new LooseLootPayload { Typed = typed };
    }
}

/// <summary>
/// Write-only: the payload is a request member, and nothing deserialises one.
/// </summary>
public sealed class LooseLootPayloadConverter : JsonConverter<LooseLootPayload>
{
    public override LooseLootPayload Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        throw new NotSupportedException($"{nameof(LooseLootPayload)} is only ever written.");
    }

    public override void Write(Utf8JsonWriter writer, LooseLootPayload value, JsonSerializerOptions options)
    {
        if (value.RawJson is { } rawJson)
        {
            // Validation is skipped because the native parser validates the same bytes immediately
            // after, and re-scanning 42 MB here would give back part of what the raw path saves
            writer.WriteRawValue(rawJson.Span, skipInputValidation: true);

            return;
        }

        JsonSerializer.Serialize(writer, value.Typed, options);
    }
}

/// <summary>
/// One log line the native side would have written itself. <c>Level</c> is one of <c>debug</c>,
/// <c>warning</c>, <c>error</c> or <c>success</c>; either <see cref="LocaleKey"/> or
/// <see cref="Message"/> is set, and the other members are written as explicit nulls.
/// </summary>
public record Diagnostic
{
    [JsonPropertyName("level")]
    public required string Level { get; set; }

    [JsonPropertyName("localeKey")]
    public string? LocaleKey { get; set; }

    /// <summary>
    /// Arguments for <see cref="LocaleKey"/> - an object of named replacements, or a bare scalar for
    /// the single-value locale overload.
    /// </summary>
    [JsonPropertyName("args")]
    public JsonElement? Args { get; set; }

    [JsonPropertyName("message")]
    public string? Message { get; set; }
}

public record StaticContainersResult
{
    [JsonPropertyName("spawnpoints")]
    public required List<SpawnpointTemplate> Spawnpoints { get; set; }

    [JsonPropertyName("trackedCounts")]
    public required Dictionary<MongoId, int> TrackedCounts { get; set; }

    [JsonPropertyName("staticLootItemCount")]
    public required int StaticLootItemCount { get; set; }

    [JsonPropertyName("staticContainerCount")]
    public required int StaticContainerCount { get; set; }

    [JsonPropertyName("diagnostics")]
    public required List<Diagnostic> Diagnostics { get; set; }
}

public record DynamicLootResult
{
    [JsonPropertyName("spawnpoints")]
    public required List<SpawnpointTemplate> Spawnpoints { get; set; }

    [JsonPropertyName("trackedCounts")]
    public required Dictionary<MongoId, int> TrackedCounts { get; set; }

    [JsonPropertyName("diagnostics")]
    public required List<Diagnostic> Diagnostics { get; set; }
}
