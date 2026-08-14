using System.Buffers;
using System.Text;
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
    /// <summary> Longest key <see cref="ReadKey"/> decodes on the stack; a longer one is a mod's </summary>
    private const int MaxStackKeyLength = 256;

    /// <summary>
    /// <c>Item.ExtensionData</c> is IL-injected by Ceciler on Release and publish builds only, so
    /// it cannot be named in source. Null on a Debug build, where unknown keys are dropped - exactly
    /// what STJ does for a type without <c>[JsonExtensionData]</c>.
    /// </summary>
    private static readonly Func<Item, Dictionary<string, object>?>? _itemExtensionData = typeof(Item)
        .GetProperty("ExtensionData")
        ?.GetGetMethod()
        is { } getter
        ? (Func<Item, Dictionary<string, object>?>)Delegate.CreateDelegate(typeof(Func<Item, Dictionary<string, object>?>), getter)
        : null;

    /// <summary>
    /// Scratch the frame is copied into so <see cref="MessagePackReader"/> gets the
    /// <see cref="ReadOnlyMemory{T}"/> it needs without a <c>byte[]</c> per offer. Per-thread because
    /// frames are parsed under a <c>Parallel.For</c>, and grown to the largest frame the thread sees.
    /// </summary>
    // ponytail: this and the two transcoder buffers below are never released - every thread pool
    // thread that has parsed a frame holds a few KB for the process' life; move them to
    // ArrayPool<byte>.Shared with a rent/return finally if that retention ever shows up in RSS
    [ThreadStatic]
    private static byte[]? _frameScratch;

    /// <summary>
    /// The transcoder's writer and its output, reused per thread: a fresh pair per <c>upd</c> and
    /// <c>location</c> is ~166k throwaway writers a pass. Everything <see cref="Materialize{T}"/>
    /// returns owns its own copy of the bytes - <c>JsonDocument</c> copies the span it parses - so
    /// the buffer is free to be overwritten by the next value.
    /// </summary>
    [ThreadStatic]
    private static ArrayBufferWriter<byte>? _transcodeBuffer;

    [ThreadStatic]
    private static Utf8JsonWriter? _transcodeWriter;

    /// <summary>
    /// Every map key the wire contract names, so <see cref="ReadKey"/> hands the switches a cached
    /// instance instead of allocating one: a pass reads ~2M keys, and the STJ arm this reader
    /// replaced allocated none of them. A key a mod added is not in here and still allocates.
    /// </summary>
    // ponytail: hand-kept in step with the switch labels below - a label added without its key here
    // costs an allocation per occurrence, not correctness; pin it with a test that reflects over the
    // readers if the list ever drifts far enough to matter
    private static readonly HashSet<string> _wireKeys =
    [
        "rejectedCanSellTemplates",
        "diagnostics",
        "_id",
        "intId",
        "user",
        "root",
        "items",
        "itemsCost",
        "requirements",
        "requirementsCost",
        "summaryCost",
        "startTime",
        "endTime",
        "loyaltyLevel",
        "sellInOnePiece",
        "locked",
        "quantity",
        "id",
        "nickname",
        "rating",
        "memberType",
        "avatar",
        "isRatingGrowing",
        "aid",
        "_tpl",
        "count",
        "onlyFunctional",
        "level",
        "side",
        "parentId",
        "slotId",
        "desc",
        "location",
        "upd",
        "localeKey",
        "message",
        "args",
    ];

    private static readonly HashSet<string>.AlternateLookup<ReadOnlySpan<char>> _wireKeyLookup = _wireKeys.GetAlternateLookup<
        ReadOnlySpan<char>
    >();

    internal static DynamicOffersHeader ReadHeader(ReadOnlySpan<byte> payload)
    {
        var reader = new MessagePackReader(CopyToScratch(payload));
        List<MongoId> rejectedCanSellTemplates = [];
        List<Diagnostic> diagnostics = [];

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            switch (ReadKey(ref reader))
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
        var reader = new MessagePackReader(CopyToScratch(payload));
        var offer = new RagfairOffer { User = null! };

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            switch (ReadKey(ref reader))
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
            switch (ReadKey(ref reader))
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
            switch (ReadKey(ref reader))
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

        // Fetched on the first unknown key rather than up front: the wire carries none on a stock
        // build, and this runs for every item in the pass
        Dictionary<string, object>? extensionData = null;

        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            var key = ReadKey(ref reader);
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
                    extensionData ??= _itemExtensionData?.Invoke(item);
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
            switch (ReadKey(ref reader))
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
        var buffer = _transcodeBuffer ??= new ArrayBufferWriter<byte>();
        buffer.ResetWrittenCount();

        var writer = _transcodeWriter;
        if (writer is null)
        {
            writer = new Utf8JsonWriter(buffer);
            _transcodeWriter = writer;
        }
        else
        {
            // Also what clears the half-written state a value that violated the wire contract left
            writer.Reset();
        }

        TranscodeValue(ref reader, writer);
        writer.Flush();

        return JsonSerializer.Deserialize<T>(buffer.WrittenSpan, JsonOptions);
    }

    /// <summary>
    /// Reads a map key without allocating a string for it when the wire contract already names it.
    /// </summary>
    private static string? ReadKey(ref MessagePackReader reader)
    {
        if (!reader.TryReadStringSpan(out var utf8))
        {
            // A key split across sequence segments, or one that is not a string at all - the frame
            // is a single contiguous buffer, so this is only the nil-key contract violation
            return reader.ReadString();
        }

        if (utf8.Length > MaxStackKeyLength)
        {
            return Encoding.UTF8.GetString(utf8);
        }

        // A UTF-8 sequence never decodes to more chars than it has bytes
        Span<char> key = stackalloc char[utf8.Length];
        var length = Encoding.UTF8.GetChars(utf8, key);
        return _wireKeyLookup.TryGetValue(key[..length], out var cached) ? cached : new string(key[..length]);
    }

    /// <summary>
    /// Copies a frame into the calling thread's scratch buffer. The frame is a span over the native
    /// allocation, which <see cref="MessagePackReader"/> cannot take directly.
    /// </summary>
    private static ReadOnlyMemory<byte> CopyToScratch(ReadOnlySpan<byte> payload)
    {
        var scratch = _frameScratch;
        if (scratch is null || scratch.Length < payload.Length)
        {
            // Double on growth so a run of ascending frame sizes stops reallocating every frame.
            scratch = new byte[Math.Max(payload.Length, (scratch?.Length ?? 0) * 2)];
            _frameScratch = scratch;
        }

        payload.CopyTo(scratch);
        return new ReadOnlyMemory<byte>(scratch, 0, payload.Length);
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
                // The UTF-8 goes straight across; decoding it to a string first is pure garbage
                if (reader.TryReadStringSpan(out var utf8Value))
                {
                    writer.WriteStringValue(utf8Value);
                }
                else
                {
                    writer.WriteStringValue(reader.ReadString());
                }

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
                    if (reader.TryReadStringSpan(out var utf8Key))
                    {
                        writer.WritePropertyName(utf8Key);
                    }
                    else
                    {
                        writer.WritePropertyName(
                            reader.ReadString() ?? throw new InvalidOperationException("a ragfair msgpack map has a non-string key.")
                        );
                    }

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
