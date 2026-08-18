using System.Text.Json;
using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Traders;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the wire contract between the ragfair payload records and <c>spt_generate_dynamic_offers</c>.
/// The request is built by <see cref="RagfairPayloadProjection"/> off the live test database, so a
/// renamed member, a dropped projection or a dictionary key that serialises as a number fails here
/// rather than as a silently emptier flea market at runtime.
/// </summary>
[TestFixture]
public class SptNativeRagfairWireTests
{
    private const ulong TestSeed = 42;

    private GenerateDynamicOffersRequest _request = default!;
    private RagfairConfig _ragfairConfig = default!;

    [OneTimeSetUp]
    public void Initialize()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions the wrapper serialises the payload with
        di.GetService<JsonUtil>();
        _ragfairConfig = di.GetService<RagfairConfig>();

        _request = RagfairPayloadProjection.BuildRequest(
            RagfairPayloadProjection.BuildInvariantSlice(
                di.GetService<TemplateTable>(),
                di.GetService<HandbookHelper>(),
                di.GetService<TraderHelper>(),
                di.GetService<PresetHelper>(),
                di.GetService<ItemFilterService>(),
                di.GetService<SeasonalEventService>(),
                di.GetService<BotTable>(),
                di.GetService<ItemHelper>(),
                di.GetService<BotConfig>(),
                _ragfairConfig
            ),
            0,
            null,
            di.GetService<TimeUtil>().GetTimeStamp(),
            0,
            TestSeed
        );
    }

    [Test]
    public void TheProjectionFillsEveryBlockTheNativeSideReads()
    {
        Assert.Multiple(() =>
        {
            Assert.That(_request.Invariant!.Items, Is.Not.Empty);
            Assert.That(_request.Invariant!.FleaPrices, Is.Not.Empty);
            Assert.That(_request.Invariant!.HandbookPrices, Is.Not.Empty);
            Assert.That(_request.Invariant!.HighestTraderPrices, Is.Not.Empty);
            Assert.That(_request.Invariant!.ItemPresets, Is.Not.Empty);
            Assert.That(_request.Invariant!.DefaultPresets, Is.Not.Empty);
            Assert.That(_request.Invariant!.DefaultPresetsByTpl, Is.Not.Empty);
            Assert.That(_request.Invariant!.PresetsByTpl, Is.Not.Empty);
            Assert.That(_request.Invariant!.PmcNamesUsec, Is.Not.Empty);
            Assert.That(_request.Invariant!.PmcNamesBear, Is.Not.Empty);
            Assert.That(_request.Invariant!.ConfigBlacklist, Is.Not.Empty);
            Assert.That(_request.Invariant!.SeasonalItemTplBlacklist, Is.Not.Empty);
            Assert.That(_request.Varying.ExpiredOffers, Is.Null);
        });
    }

    /// <summary>
    /// The EftEnumConverter pitfall the bot port hit: System.Text.Json writes enum dictionary keys
    /// as numbers. Every ragfair map that crosses must be keyed by a string or a MongoId, never by
    /// an enum, or the native side silently finds nothing.
    /// </summary>
    [Test]
    public void EveryProjectedDictionaryKeyIsAStringOnTheWire()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!.AsObject()[
            "invariant"
        ]!.AsObject();

        foreach (var block in new[] { "fleaPrices", "handbookPrices", "highestTraderPrices", "itemPresets", "defaultPresetsByTpl" })
        {
            foreach (var entry in json[block]!.AsObject())
            {
                Assert.That(long.TryParse(entry.Key, out _), Is.False, $"{block} key '{entry.Key}' serialised as a number");
            }
        }

        // dynamic.condition and dynamic.offerItemCount are the two config maps the native side
        // iterates by key; a numeric key here would break the baseclass match and the offer count
        foreach (var entry in json["dynamic"]!["condition"]!.AsObject())
        {
            Assert.That(new MongoId(entry.Key).IsEmpty, Is.False, $"condition key '{entry.Key}' is not a tpl");
        }
    }

    [Test]
    public void TheRequestRoundTripsThroughTheNativeSide()
    {
        var result = SptNative.GenerateDynamicOffers(_request);

        Assert.That(result.Offers, Is.Not.Empty);
        Assert.That(result.Offers[0].Items, Is.Not.Empty);
        Assert.That(result.Offers[0].User.Nickname, Is.Not.Null.And.Not.Empty);
        Assert.That(result.Offers[0].SummaryCost, Is.GreaterThan(0));
        // Id is declared MongoId on the C# record, so a malformed hex string would already have
        // failed the deserialize; this catches an all-zero default
        Assert.That(result.Offers[0].Id.IsEmpty, Is.False);
        // The frames are deserialized in parallel: the envelope order is what pins the result order
        Assert.That(result.Offers.Select(offer => offer.InternalId), Is.EqualTo(Enumerable.Range(0, result.Offers.Count)));
        Assert.That(result.Offers[0].CreatedBy, Is.EqualTo(OfferCreator.FakePlayer));
    }

    /// <summary>
    /// The seed seam every parity harness stands on: a mistyped <c>testSeed</c> wire name would leave
    /// the native side seeding from entropy with every other test still green. Minted ids are
    /// excluded - <c>mongo_id::generate</c> is a clock plus an atomic counter, deliberately outside
    /// the seeded draw stream, exactly as C#'s <see cref="MongoId"/> is; every value compared here is
    /// a draw (the price variance, the stack count, the offer duration).
    /// </summary>
    [Test]
    public void TheSameSeedProducesTheSameOffers()
    {
        var first = SptNative.GenerateDynamicOffers(_request);
        var second = SptNative.GenerateDynamicOffers(_request);

        Assert.That(
            second.Offers.Select(offer => (offer.SummaryCost, offer.Quantity, offer.EndTime - offer.StartTime)),
            Is.EqualTo(first.Offers.Select(offer => (offer.SummaryCost, offer.Quantity, offer.EndTime - offer.StartTime)))
        );
    }

    /// <summary>
    /// The miss half of the cache gate: the native side keeps one slice slot, so a slice-less
    /// request naming a stamp it has never stored must report itself distinctly rather than fail
    /// as a generation error - that is what lets the caller self-heal by resending the slice.
    /// </summary>
    [Test]
    public void ASliceLessRequestWithAnUnknownStampThrowsStaleSlice()
    {
        var request = RagfairPayloadProjection.BuildRequest(
            invariant: null,
            invariantStamp: long.MaxValue,
            expiredOffers: null,
            timestamp: 1_700_000_000,
            offerCounterStart: 0,
            testSeed: 1234
        );

        Assert.Throws<NativeStaleEpochException>(() => SptNative.GenerateDynamicOffers(request));
    }

    /// <summary>
    /// A mod-added field on a game-data object inside the payload must survive the round trip - the
    /// `[serde(flatten)] extra` contract that mirrors Ceciler's `[JsonExtensionData]`.
    /// </summary>
    [Test]
    public void AModAddedConfigFieldSurvivesTheRoundTrip()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!.AsObject();
        json["invariant"]!["dynamic"]!["modAddedField"] = "kept";

        // No assertion on the value coming back - the native result carries offers, not the config;
        // this asserts only that an unknown key does not fail the parse.
        var result = SptNative.GenerateDynamicOffersFramed(System.Text.Encoding.UTF8.GetBytes(json.ToJsonString()));

        Assert.That(result.Offers, Is.Not.Empty);
    }

    /// <summary>
    /// A mod-added field on an expired offer's item must survive Rust's `extra` flatten and come
    /// back through the MessagePack frames into the Ceciler-injected extension data.
    /// </summary>
    [Test]
    public void AModAddedItemFieldSurvivesTheRoundTrip()
    {
        if (typeof(Item).GetProperty("ExtensionData") is not { } extensionData)
        {
            Assert.Ignore("extension data is Ceciler-injected in Release builds only");
            return;
        }

        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!.AsObject();
        var itemTpl = json["invariant"]!["items"]!.AsObject().First().Key;
        json["varying"]!["expiredOffers"] = new JsonArray(
            new JsonArray(
                new JsonObject
                {
                    ["_id"] = "0123456789abcdef01234567",
                    ["_tpl"] = itemTpl,
                    ["modField"] = "kept",
                }
            )
        );

        var result = SptNative.GenerateDynamicOffersFramed(System.Text.Encoding.UTF8.GetBytes(json.ToJsonString()));

        Assert.That(result.Offers, Has.Count.EqualTo(1));
        var extension = (Dictionary<string, object>)extensionData.GetValue(result.Offers[0].Items![0])!;
        Assert.That(((JsonElement)extension["modField"]).GetString(), Is.EqualTo("kept"));
    }
}
