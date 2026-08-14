using System.Buffers;
using System.Text.Json;
using MessagePack;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Ragfair;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Core.Native.Ragfair;

/// <summary>
/// The <c>encoding == 1</c> arm of the framed ragfair envelope: MessagePack payloads written by
/// <c>rmp_serde::to_vec_named</c>, so every map is string-keyed under the same names the JSON arm
/// reads and this reader is a straight transliteration of the STJ binding.
///
/// Only the members the wire actually carries are bound directly; anything the schema does not
/// pin - <c>upd</c>, <c>location</c>, a mod-added item key - is transcoded to JSON and handed to
/// <see cref="JsonSerializer"/>, so the object graph is indistinguishable from the JSON arm's.
/// </summary>
internal static class MsgpackOfferReader
{
    /// <summary>
    /// <c>Item.ExtensionData</c> is IL-injected by Ceciler on Release and publish builds only, so
    /// it cannot be named in source. Null on a Debug build, where unknown keys are dropped - exactly
    /// what STJ does for a type without <c>[JsonExtensionData]</c>.
    /// </summary>
    private static readonly Func<Item, Dictionary<string, object>?>? _itemExtensionData = typeof(Item).GetProperty("ExtensionData")
        is { } property
        ? item => (Dictionary<string, object>?)property.GetValue(item)
        : null;

    internal static DynamicOffersHeader ReadHeader(ReadOnlySpan<byte> payload)
    {
        var reader = new MessagePackReader(payload.ToArray());
        List<MongoId> rejectedCanSellTemplates = [];
        List<Diagnostic> diagnostics = [];

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            switch (reader.ReadString())
            {
                case "rejectedCanSellTemplates":
                    var templateCount = reader.ReadArrayHeader();
                    rejectedCanSellTemplates = new List<MongoId>(templateCount);
                    for (var j = 0; j < templateCount; j++)
                    {
                        rejectedCanSellTemplates.Add(new MongoId(reader.ReadString()));
                    }

                    break;
                case "diagnostics":
                    var diagnosticCount = reader.ReadArrayHeader();
                    diagnostics = new List<Diagnostic>(diagnosticCount);
                    for (var j = 0; j < diagnosticCount; j++)
                    {
                        diagnostics.Add(ReadDiagnostic(ref reader));
                    }

                    break;
                default:
                    reader.Skip();
                    break;
            }
        }

        return new DynamicOffersHeader { RejectedCanSellTemplates = rejectedCanSellTemplates, Diagnostics = diagnostics };
    }

    internal static RagfairOffer ReadOffer(ReadOnlySpan<byte> payload)
    {
        // ponytail: per-frame copy to satisfy MessagePackReader's ReadOnlySequence input; swap to
        // a pooled buffer or a MemoryManager over the native buffer if stage C profiling flags it
        var reader = new MessagePackReader(payload.ToArray());
        var offer = new RagfairOffer { User = null! };

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            switch (reader.ReadString())
            {
                case "_id":
                    offer.Id = new MongoId(reader.ReadString());
                    break;
                case "intId":
                    if (!reader.TryReadNil())
                    {
                        offer.InternalId = reader.ReadInt32();
                    }

                    break;
                case "user":
                    offer.User = ReadUser(ref reader);
                    break;
                case "root":
                    offer.Root = new MongoId(reader.ReadString());
                    break;
                case "items":
                    var itemCount = reader.ReadArrayHeader();
                    var items = new List<Item>(itemCount);
                    for (var j = 0; j < itemCount; j++)
                    {
                        items.Add(ReadItem(ref reader));
                    }

                    offer.Items = items;
                    break;
                case "itemsCost":
                    if (!reader.TryReadNil())
                    {
                        offer.ItemsCost = reader.ReadDouble();
                    }

                    break;
                case "requirements":
                    var requirementCount = reader.ReadArrayHeader();
                    var requirements = new List<OfferRequirement>(requirementCount);
                    for (var j = 0; j < requirementCount; j++)
                    {
                        requirements.Add(ReadRequirement(ref reader));
                    }

                    offer.Requirements = requirements;
                    break;
                case "requirementsCost":
                    if (!reader.TryReadNil())
                    {
                        offer.RequirementsCost = reader.ReadDouble();
                    }

                    break;
                case "summaryCost":
                    if (!reader.TryReadNil())
                    {
                        offer.SummaryCost = reader.ReadDouble();
                    }

                    break;
                case "startTime":
                    if (!reader.TryReadNil())
                    {
                        offer.StartTime = reader.ReadInt64();
                    }

                    break;
                case "endTime":
                    if (!reader.TryReadNil())
                    {
                        offer.EndTime = reader.ReadInt64();
                    }

                    break;
                case "loyaltyLevel":
                    if (!reader.TryReadNil())
                    {
                        offer.LoyaltyLevel = reader.ReadInt32();
                    }

                    break;
                case "sellInOnePiece":
                    if (!reader.TryReadNil())
                    {
                        offer.SellInOnePiece = reader.ReadBoolean();
                    }

                    break;
                case "locked":
                    if (!reader.TryReadNil())
                    {
                        offer.Locked = reader.ReadBoolean();
                    }

                    break;
                case "quantity":
                    offer.Quantity = reader.ReadInt32();
                    break;
                default:
                    reader.Skip();
                    break;
            }
        }

        if (offer.User is null)
        {
            throw new InvalidOperationException("spt_native returned a ragfair offer frame without a user.");
        }

        offer.CreatedBy = OfferCreator.FakePlayer;
        return offer;
    }

    private static RagfairOfferUser ReadUser(ref MessagePackReader reader)
    {
        var user = new RagfairOfferUser();

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            switch (reader.ReadString())
            {
                case "id":
                    user.Id = new MongoId(reader.ReadString());
                    break;
                case "nickname":
                    user.Nickname = reader.ReadString();
                    break;
                case "rating":
                    if (!reader.TryReadNil())
                    {
                        user.Rating = reader.ReadDouble();
                    }

                    break;
                case "memberType":
                    if (!reader.TryReadNil())
                    {
                        user.MemberType = (MemberCategory)reader.ReadInt32();
                    }

                    break;
                case "avatar":
                    user.Avatar = reader.ReadString();
                    break;
                case "isRatingGrowing":
                    if (!reader.TryReadNil())
                    {
                        user.IsRatingGrowing = reader.ReadBoolean();
                    }

                    break;
                case "aid":
                    if (!reader.TryReadNil())
                    {
                        user.Aid = reader.ReadInt32();
                    }

                    break;
                default:
                    reader.Skip();
                    break;
            }
        }

        return user;
    }

    private static OfferRequirement ReadRequirement(ref MessagePackReader reader)
    {
        var requirement = new OfferRequirement { TemplateId = default };

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            switch (reader.ReadString())
            {
                case "_tpl":
                    requirement.TemplateId = new MongoId(reader.ReadString());
                    break;
                case "count":
                    if (!reader.TryReadNil())
                    {
                        requirement.Count = reader.ReadDouble();
                    }

                    break;
                case "onlyFunctional":
                    if (!reader.TryReadNil())
                    {
                        requirement.OnlyFunctional = reader.ReadBoolean();
                    }

                    break;
                case "level":
                    if (!reader.TryReadNil())
                    {
                        requirement.Level = reader.ReadInt32();
                    }

                    break;
                case "side":
                    if (!reader.TryReadNil())
                    {
                        requirement.Side = (DogtagExchangeSide)reader.ReadInt32();
                    }

                    break;
                default:
                    reader.Skip();
                    break;
            }
        }

        return requirement;
    }

    private static Item ReadItem(ref MessagePackReader reader)
    {
        var item = new Item { Id = default };
        var extensionData = _itemExtensionData?.Invoke(item);

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            var key = reader.ReadString();
            switch (key)
            {
                case "_id":
                    item.Id = new MongoId(reader.ReadString());
                    break;
                case "_tpl":
                    item.Template = new MongoId(reader.ReadString());
                    break;
                case "parentId":
                    item.ParentId = reader.ReadString();
                    break;
                case "slotId":
                    item.SlotId = reader.ReadString();
                    break;
                case "desc":
                    item.Desc = reader.ReadString();
                    break;
                case "location":
                    if (reader.TryReadNil())
                    {
                        item.Location = null;
                    }
                    else
                    {
                        item.Location = Materialize<JsonElement>(ref reader);
                    }

                    break;
                case "upd":
                    if (reader.TryReadNil())
                    {
                        item.Upd = null;
                    }
                    else
                    {
                        item.Upd = Materialize<Upd>(ref reader);
                    }

                    break;
                default:
                    if (extensionData is null || key is null)
                    {
                        reader.Skip();
                    }
                    else
                    {
                        extensionData[key] = Materialize<JsonElement>(ref reader)!;
                    }

                    break;
            }
        }

        return item;
    }

    private static Diagnostic ReadDiagnostic(ref MessagePackReader reader)
    {
        var diagnostic = new Diagnostic { Level = string.Empty };

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            switch (reader.ReadString())
            {
                case "level":
                    diagnostic.Level = reader.ReadString() ?? string.Empty;
                    break;
                case "localeKey":
                    diagnostic.LocaleKey = reader.ReadString();
                    break;
                case "message":
                    diagnostic.Message = reader.ReadString();
                    break;
                case "args":
                    if (reader.TryReadNil())
                    {
                        diagnostic.Args = null;
                    }
                    else
                    {
                        diagnostic.Args = Materialize<JsonElement>(ref reader);
                    }

                    break;
                default:
                    reader.Skip();
                    break;
            }
        }

        return diagnostic;
    }

    /// <summary>
    /// Transcodes the next msgpack value to JSON and lets <see cref="JsonSerializer"/> bind it, so
    /// schema-free members materialize through exactly the converters the JSON arm uses.
    /// </summary>
    private static T? Materialize<T>(ref MessagePackReader reader)
    {
        var buffer = new ArrayBufferWriter<byte>();
        using (var writer = new Utf8JsonWriter(buffer))
        {
            TranscodeValue(ref reader, writer);
        }

        return JsonSerializer.Deserialize<T>(buffer.WrittenSpan, JsonOptions);
    }

    private static void TranscodeValue(ref MessagePackReader reader, Utf8JsonWriter writer)
    {
        switch (reader.NextMessagePackType)
        {
            case MessagePackType.Nil:
                reader.ReadNil();
                writer.WriteNullValue();
                break;
            case MessagePackType.Boolean:
                writer.WriteBooleanValue(reader.ReadBoolean());
                break;
            case MessagePackType.Integer:
                writer.WriteNumberValue(reader.ReadInt64());
                break;
            case MessagePackType.Float:
                writer.WriteNumberValue(reader.ReadDouble());
                break;
            case MessagePackType.String:
                writer.WriteStringValue(reader.ReadString());
                break;
            case MessagePackType.Array:
                var elementCount = reader.ReadArrayHeader();
                writer.WriteStartArray();
                for (var i = 0; i < elementCount; i++)
                {
                    TranscodeValue(ref reader, writer);
                }

                writer.WriteEndArray();
                break;
            case MessagePackType.Map:
                var memberCount = reader.ReadMapHeader();
                writer.WriteStartObject();
                for (var i = 0; i < memberCount; i++)
                {
                    writer.WritePropertyName(
                        reader.ReadString() ?? throw new InvalidOperationException("a ragfair msgpack map has a non-string key.")
                    );
                    TranscodeValue(ref reader, writer);
                }

                writer.WriteEndObject();
                break;
            default:
                // bin and ext have no JSON spelling, and nothing on the Rust side emits them.
                throw new InvalidOperationException($"msgpack {reader.NextMessagePackType} is not part of the ragfair wire contract.");
        }
    }

    /// <summary>
    /// The options JsonUtil publishes at startup: ragfair payloads need its MongoId and enum converters.
    /// </summary>
    private static JsonSerializerOptions JsonOptions
    {
        get
        {
            return JsonUtil.JsonSerializerOptionsNoIndent
                ?? throw new InvalidOperationException(
                    "JsonUtil has not been built yet, so the ragfair payload converters are unavailable."
                );
        }
    }
}
