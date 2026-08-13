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

internal record DynamicOffersResult
{
    [JsonPropertyName("offers")]
    public required List<RagfairOfferWire> Offers { get; set; }

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
/// The native offer, declared rather than deserialised straight into <see cref="RagfairOffer"/>:
/// that record's <c>Requirements</c> is an <c>IEnumerable</c> and its <c>User</c> is
/// <c>required</c>. Every id-shaped member is a <see cref="MongoId"/>, which round-trips through the
/// hex string Rust emits, so a malformed id fails the deserialize instead of reaching the holder.
/// </summary>
internal record RagfairOfferWire
{
    [JsonPropertyName("_id")]
    public required MongoId Id { get; set; }

    [JsonPropertyName("intId")]
    public required int InternalId { get; set; }

    [JsonPropertyName("user")]
    public required RagfairOfferUserWire User { get; set; }

    [JsonPropertyName("root")]
    public required MongoId Root { get; set; }

    [JsonPropertyName("items")]
    public required List<Item> Items { get; set; }

    [JsonPropertyName("itemsCost")]
    public required double ItemsCost { get; set; }

    [JsonPropertyName("requirements")]
    public required List<OfferRequirementWire> Requirements { get; set; }

    [JsonPropertyName("requirementsCost")]
    public required double RequirementsCost { get; set; }

    [JsonPropertyName("summaryCost")]
    public required double SummaryCost { get; set; }

    [JsonPropertyName("startTime")]
    public required long StartTime { get; set; }

    [JsonPropertyName("endTime")]
    public required long EndTime { get; set; }

    [JsonPropertyName("loyaltyLevel")]
    public required int LoyaltyLevel { get; set; }

    [JsonPropertyName("sellInOnePiece")]
    public required bool SellInOnePiece { get; set; }

    [JsonPropertyName("locked")]
    public required bool Locked { get; set; }

    [JsonPropertyName("quantity")]
    public required int Quantity { get; set; }
}

internal record RagfairOfferUserWire
{
    [JsonPropertyName("id")]
    public required MongoId Id { get; set; }

    [JsonPropertyName("nickname")]
    public string? Nickname { get; set; }

    [JsonPropertyName("rating")]
    public required double Rating { get; set; }

    /// <summary>
    /// The numeric <see cref="MemberCategory"/> - <c>EftEnumConverter</c> writes enums as numbers,
    /// so this stays an integer on the wire and is cast back by the mapper.
    /// </summary>
    [JsonPropertyName("memberType")]
    public required int MemberType { get; set; }

    [JsonPropertyName("avatar")]
    public string? Avatar { get; set; }

    [JsonPropertyName("isRatingGrowing")]
    public required bool IsRatingGrowing { get; set; }

    [JsonPropertyName("aid")]
    public required int Aid { get; set; }
}

/// <summary>
/// <c>Level</c> and <c>Side</c> are only set for dogtag barters, which the dynamic path never
/// produces - they are nullable so the wire stays faithful if that ever changes.
/// </summary>
internal record OfferRequirementWire
{
    [JsonPropertyName("_tpl")]
    public required MongoId TemplateId { get; set; }

    [JsonPropertyName("count")]
    public required double Count { get; set; }

    [JsonPropertyName("onlyFunctional")]
    public required bool OnlyFunctional { get; set; }

    [JsonPropertyName("level")]
    public int? Level { get; set; }

    [JsonPropertyName("side")]
    public int? Side { get; set; }
}

internal static class RagfairOfferWireExtensions
{
    /// <summary>
    ///     The native offer as the frozen 4.1.2 DTO the holder stores. <c>SellResults</c>,
    ///     <c>UnlimitedCount</c> and the two buy-restriction members stay at their defaults: the
    ///     dynamic path never sets them (<c>RagfairOfferGenerator.cs:118-138</c>).
    /// </summary>
    internal static RagfairOffer ToRagfairOffer(this RagfairOfferWire wire)
    {
        return new RagfairOffer
        {
            Id = wire.Id,
            InternalId = wire.InternalId,
            User = new RagfairOfferUser
            {
                Id = wire.User.Id,
                Nickname = wire.User.Nickname,
                Rating = wire.User.Rating,
                // The wire carries the numeric EftEnumConverter value, matching CreateUserDataForFleaOffer
                MemberType = (MemberCategory)wire.User.MemberType,
                Avatar = wire.User.Avatar,
                IsRatingGrowing = wire.User.IsRatingGrowing,
                Aid = wire.User.Aid,
            },
            Root = wire.Root,
            Items = wire.Items,
            ItemsCost = wire.ItemsCost,
            Requirements = wire
                .Requirements.Select(requirement => new OfferRequirement
                {
                    TemplateId = requirement.TemplateId,
                    Count = requirement.Count,
                    OnlyFunctional = requirement.OnlyFunctional,
                    Level = requirement.Level,
                    Side = (DogtagExchangeSide?)requirement.Side,
                })
                .ToList(),
            RequirementsCost = wire.RequirementsCost,
            SummaryCost = wire.SummaryCost,
            StartTime = wire.StartTime,
            EndTime = wire.EndTime,
            LoyaltyLevel = wire.LoyaltyLevel,
            SellInOnePiece = wire.SellInOnePiece,
            Locked = wire.Locked,
            Quantity = wire.Quantity,
            // What CreateAndAddFleaOffer:72 sets; the holder's fake-player cap keys off it
            CreatedBy = OfferCreator.FakePlayer,
        };
    }
}
