using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Ragfair;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Loot;

namespace SPTarkov.Server.Core.Native.Ragfair;

/// <summary>
/// The request/response envelope of <c>spt_generate_dynamic_offers</c>, mirroring
/// <c>rust/spt-native/src/ragfair/models.rs</c> member for member.
///
/// Config and database models are the existing records from <c>Models</c>, whose
/// <c>JsonPropertyName</c>s are what the Rust wire names were pinned to, so their shape stays
/// authoritative by construction - <see cref="Dynamic"/> rides across whole. The game-data views
/// (<see cref="PresetView"/>, <see cref="ItemView"/>, <see cref="Diagnostic"/>) are the loot port's.
///
/// Members Rust declares as <c>Option&lt;T&gt;</c> are nullable, everything else is
/// <c>required</c>: <see cref="Utils.JsonUtil"/> serialises with
/// <see cref="JsonIgnoreCondition.WhenWritingNull"/>, so a null member is omitted and a Rust member
/// that is not an <c>Option</c> would fail the parse.
/// </summary>
internal record GenerateDynamicOffersRequest
{
    /// <summary>
    ///     The <c>DatabaseMutationStamp</c> value the invariant slice was (or would be) built at.
    ///     The native side stores the slice under this value and, on a slice-less request, serves
    ///     its cached slice only when the stamps match.
    /// </summary>
    [JsonPropertyName("invariantStamp")]
    public required long InvariantStamp { get; set; }

    /// <summary>
    ///     Null on a cache-hit send: the native side reuses the slice it stored under
    ///     <see cref="InvariantStamp"/>. Always present until the cache gate lands.
    /// </summary>
    [JsonPropertyName("invariant")]
    public RagfairInvariantSlice? Invariant { get; set; }

    [JsonPropertyName("varying")]
    public required RagfairVaryingFields Varying { get; set; }
}

/// <summary>
/// The members that change every call - everything the projection does not read off the database.
/// </summary>
internal record RagfairVaryingFields
{
    /// <summary>
    /// Test-only: draws on the native side come from a seeded generator when set. Null - and
    /// therefore omitted from the wire JSON - on the production path.
    /// </summary>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }

    /// <summary>
    /// <c>TimeUtil.GetTimeStamp()</c> taken once for the whole batch. Legacy re-reads the clock per
    /// offer (<c>RagfairOfferGenerator.cs:491</c>); one timestamp per pass is a sanctioned
    /// divergence.
    /// </summary>
    [JsonPropertyName("timestamp")]
    public required long Timestamp { get; set; }

    /// <summary>
    /// The generator's <c>OfferCounter</c> before the pass; offers come back numbered from it.
    /// </summary>
    [JsonPropertyName("offerCounterStart")]
    public required int OfferCounterStart { get; set; }

    /// <summary>
    /// Null for a full pass; the cloned expired-offer item lists for a regeneration pass
    /// (<c>RagfairServer.cs:69-79</c>).
    /// </summary>
    [JsonPropertyName("expiredOffers")]
    public IEnumerable<List<Item>>? ExpiredOffers { get; set; }
}

/// <summary>
/// The call-invariant half of the request: the database, config and service projections, which only
/// change when the database does.
/// </summary>
internal record RagfairInvariantSlice
{
    /// <summary>
    /// <c>RagfairConfig.Dynamic</c>, the live object - mods mutate it at runtime and the native side
    /// has to see that.
    /// </summary>
    [JsonPropertyName("dynamic")]
    public required Dynamic Dynamic { get; set; }

    /// <summary>
    /// <c>GlobalTable.ItemPresets</c>, keyed by preset id. Scanned in order by the assort walk, so
    /// the insertion order of the source dictionary is the contract.
    /// </summary>
    [JsonPropertyName("itemPresets")]
    public required Dictionary<MongoId, PresetView> ItemPresets { get; set; }

    /// <summary>
    /// <c>PresetHelper.GetDefaultPresets().Values</c> - the assort walk's preset source when
    /// <c>showDefaultPresetsOnly</c> is set.
    /// </summary>
    [JsonPropertyName("defaultPresets")]
    public required List<PresetView> DefaultPresets { get; set; }

    [JsonPropertyName("defaultPresetsByTpl")]
    public required Dictionary<MongoId, PresetView> DefaultPresetsByTpl { get; set; }

    /// <summary>
    /// <c>PresetHelper.GetPresets(tpl)</c> for every tpl that has presets - the fallback arm of
    /// <c>RagfairPriceService.GetWeaponPreset</c>.
    /// </summary>
    [JsonPropertyName("presetsByTpl")]
    public required Dictionary<MongoId, List<PresetView>> PresetsByTpl { get; set; }

    /// <summary>
    /// <c>TemplateTable.Prices</c>, whole and in source order: that order is what
    /// <c>GetFleaPricesAsArray</c> draws an index into.
    /// </summary>
    [JsonPropertyName("fleaPrices")]
    public required Dictionary<MongoId, double> FleaPrices { get; set; }

    /// <summary>
    /// <c>HandbookHelper.GetTemplatePrice</c> for the whole items table - the pricing math reaches
    /// arbitrary tpls through barter schemes and preset children.
    /// </summary>
    [JsonPropertyName("handbookPrices")]
    public required Dictionary<MongoId, double> HandbookPrices { get; set; }

    /// <summary>
    /// <c>TraderHelper.GetHighestSellToTraderPrice</c> for the whole items table; cache-backed on
    /// this side, so the loop is cheap after the first pass.
    /// </summary>
    [JsonPropertyName("highestTraderPrices")]
    public required Dictionary<MongoId, double> HighestTraderPrices { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetBlacklistedItems()</c>.
    /// </summary>
    [JsonPropertyName("configBlacklist")]
    public required HashSet<MongoId> ConfigBlacklist { get; set; }

    [JsonPropertyName("seasonalEventActive")]
    public required bool SeasonalEventActive { get; set; }

    [JsonPropertyName("seasonalItemTplBlacklist")]
    public required HashSet<MongoId> SeasonalItemTplBlacklist { get; set; }

    /// <summary>
    /// <c>BotHelper.GatherPmcNamesOfLength</c> for each faction at
    /// <c>BotConfig.BotNameLengthLimit</c>, pre-filtered. The faction itself is still drawn natively.
    /// </summary>
    [JsonPropertyName("pmcNamesUsec")]
    public required List<string> PmcNamesUsec { get; set; }

    [JsonPropertyName("pmcNamesBear")]
    public required List<string> PmcNamesBear { get; set; }

    /// <inheritdoc cref="LootCommon.ItemsView"/>
    [JsonPropertyName("items")]
    public required Dictionary<MongoId, ItemView> Items { get; set; }
}

/// <summary>
/// The header section of the framed <c>spt_generate_dynamic_offers</c> response — everything
/// except the offer frames, which deserialize straight into <see cref="RagfairOffer"/>.
/// </summary>
internal record DynamicOffersHeader
{
    /// <summary>
    /// Template ids the custom-blacklist arm of <c>IsItemValidRagfairItem</c> set
    /// <c>CanSellOnRagfair</c> to <c>false</c> for. The caller replays these onto the live template
    /// table; nothing else in this port mutates the database.
    /// </summary>
    [JsonPropertyName("rejectedCanSellTemplates")]
    public required List<MongoId> RejectedCanSellTemplates { get; set; }

    [JsonPropertyName("diagnostics")]
    public required List<Diagnostic> Diagnostics { get; set; }
}

/// <summary>
/// A parsed framed response: the header sections plus the materialized offers, each already
/// stamped <see cref="OfferCreator.FakePlayer"/> the way <c>CreateAndAddFleaOffer:72</c> does.
/// </summary>
internal record FramedOffersResult
{
    public required List<RagfairOffer> Offers { get; set; }

    public required List<MongoId> RejectedCanSellTemplates { get; set; }

    public required List<Diagnostic> Diagnostics { get; set; }
}
