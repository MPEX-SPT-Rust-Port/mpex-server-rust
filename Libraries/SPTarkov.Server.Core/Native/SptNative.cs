using System.Buffers.Binary;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Ragfair;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.BaseClass;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Native.RepeatableQuests;
using SPTarkov.Server.Core.Native.ScavCase;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Core.Native;

public sealed class VerifyFailure
{
    [JsonPropertyName("path")]
    public string Path { get; set; } = string.Empty;

    [JsonPropertyName("reason")]
    public string Reason { get; set; } = string.Empty;
}

public sealed class VerifyResult
{
    [JsonPropertyName("ok")]
    public bool Ok { get; set; }

    [JsonPropertyName("failures")]
    public List<VerifyFailure> Failures { get; set; } = [];

    [JsonPropertyName("checked")]
    public int Checked { get; set; }
}

/// <summary>
/// One eager file in the framed <c>spt_db_load</c> response: its manifest-style key and the
/// length of its blob, which follows the header in this entry's order.
/// </summary>
internal sealed class DbLoadFileEntry
{
    [JsonPropertyName("path")]
    public string Path { get; set; } = string.Empty;

    [JsonPropertyName("len")]
    public int Len { get; set; }
}

internal sealed class DbLoadHeader
{
    [JsonPropertyName("epoch")]
    public ulong? Epoch { get; set; }

    [JsonPropertyName("verify")]
    public VerifyResult? Verify { get; set; }

    [JsonPropertyName("files")]
    public List<DbLoadFileEntry> Files { get; set; } = [];
}

/// <summary>
/// What the fused load answers: the verification report when one was asked for, the epoch the
/// resident DB now holds (null when verification failed and nothing was installed), and the eager
/// file bytes, already copied out of the native buffer.
/// </summary>
internal sealed class DbLoadResult
{
    public required VerifyResult? Verify { get; init; }

    public required ulong? Epoch { get; init; }

    public required Dictionary<string, ReadOnlyMemory<byte>> Files { get; init; }
}

/// <summary>
/// The resident DB's per-root canonical digests and the epoch they were taken at: the roots map is
/// empty before the first publish, and omits any root that is not resident.
/// </summary>
internal sealed class DbDigestResult
{
    public required ulong Epoch { get; init; }

    public required Dictionary<string, string> Roots { get; init; }
}

/// <summary>
/// One profile off disk: its bytes exactly as they were stored, <c>default</c> when the file was
/// not there. Bytes and not a string on purpose - the profile goes straight into
/// <c>jsonUtil.Deserialize(span, type)</c>, so tens of MB are never widened to UTF-16.
/// </summary>
internal readonly record struct ProfileLoadResult(bool Found, ReadOnlyMemory<byte> Utf8Json);

/// <summary>
/// Picks which of the generation exports a request goes to.
/// </summary>
internal enum LootExport
{
    StaticContainers,
    DynamicLoot,
    CreateRandomLoot,
    CreateForcedLoot,
    SealedWeaponCase,
    RandomLootContainer,
    BotInventory,
    BotInventoryBatch,
    RepeatableQuest,
    ScavCaseRewards,
    RaidAdjustments,
    ItemBaseClass,
    RagfairLinkedItems,
}

public static class SptNative
{
    private const uint ExpectedAbiVersion = 34;

    // ffi.rs
    private const int StatusOk = 0;
    private const int StatusPanic = 2;
    private const int StatusError = 3;

    // ffi.rs: a request named a resident-DB epoch the native process does not hold
    private const int StatusStaleEpoch = 4;

    // No CancellationToken: the native hash pass is a single bounded blocking call that cannot be
    // interrupted once in flight, so accepting a token would promise cancellation it can't deliver.
    public static Task<VerifyResult> VerifyDatabaseAsync(string sptDataDir)
    {
        return Task.Run(() => VerifyDatabase(sptDataDir));
    }

    /// <summary>
    /// Forces the native library to load and checks its ABI version, so a missing or stale
    /// spt_native fails at startup with a clear message instead of mid-request.
    /// </summary>
    public static void EnsureLoadable()
    {
        var actual = NativeMethods.AbiVersion();
        if (actual != ExpectedAbiVersion)
        {
            throw new InvalidOperationException(
                $"spt_native ABI version mismatch: expected {ExpectedAbiVersion}, found {actual}. Rebuild the native library (dotnet build runs cargo automatically)."
            );
        }
    }

    /// <summary>
    /// Hands the resolved server-locale table to the native side, where generator diagnostics
    /// render. Never throws: a failed push means generator log lines show locale keys instead of
    /// text, which must not stop the server.
    /// </summary>
    public static unsafe void SetServerLocales(Dictionary<string, string> locales)
    {
        // Default options on purpose: LootJsonOptions carries naming policies that would rewrite
        // the locale keys the native renderer looks up.
        var json = JsonSerializer.SerializeToUtf8Bytes(locales);
        byte* outPtr = null;
        nuint outLen = 0;

        fixed (byte* jsonPtr = json)
        {
            var status = NativeMethods.LocalesSet(jsonPtr, (nuint)json.Length, &outPtr, &outLen);
            if (status != StatusOk)
            {
                var message = outPtr == null ? $"internal status {status}" : Encoding.UTF8.GetString(outPtr, checked((int)outLen));
                Console.Error.WriteLine(
                    $"Failed to hand the server locales to spt_native: {message}. Generator log lines will show locale keys."
                );
            }

            NativeMethods.BufFree(outPtr, outLen);
        }
    }

    /// <summary>
    /// Fills a map's static containers with loot.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public static StaticContainersResult GenerateStaticContainers(StaticContainersRequest request)
    {
        return Generate<StaticContainersResult>(LootExport.StaticContainers, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Picks a map's loose loot spawn points and fills them.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public static DynamicLootResult GenerateDynamicLoot(DynamicLootRequest request)
    {
        return Generate<DynamicLootResult>(LootExport.DynamicLoot, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Rolls a random assortment of rewards - weapon and armor presets, sealed crates and loose
    /// items - within the counts the request asks for.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public static RewardLootResult CreateRandomLoot(CreateRandomLootRequest request)
    {
        return Generate<RewardLootResult>(LootExport.CreateRandomLoot, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Builds the rewards a forced loot list names, splitting any that exceed their stack size.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public static RewardLootResult CreateForcedLoot(CreateForcedLootRequest request)
    {
        return Generate<RewardLootResult>(LootExport.CreateForcedLoot, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Fills a sealed weapon case: one weapon preset plus its mod and non-mod reward types.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public static RewardLootResult GetSealedWeaponCaseLoot(SealedWeaponCaseRequest request)
    {
        return Generate<RewardLootResult>(LootExport.SealedWeaponCase, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Fills a random loot container from its reward pool.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public static RewardLootResult GetRandomLootContainerLoot(RandomLootContainerRequest request)
    {
        return Generate<RewardLootResult>(LootExport.RandomLootContainer, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Rolls the rewards of one completed scav case craft.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    /// <exception cref="NativeStaleEpochException">An override-less request named an epoch the resident DB does not hold.</exception>
    public static ScavCaseRewardsResponse GenerateScavCaseRewards(ScavCaseRewardsRequest request)
    {
        return Generate<ScavCaseRewardsResponse>(LootExport.ScavCaseRewards, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Rolls one scav raid's time adjustment: the reduced raid time, the loot percents and the
    /// train-exit changes. The raid family rides no resident DB - every config and location member
    /// it reads is projected into the request - so the call names no epoch and can never go stale.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public static TResponse GetRaidAdjustments<TResponse>(ReadOnlySpan<byte> requestUtf8)
    {
        return Generate<TResponse>(LootExport.RaidAdjustments, requestUtf8);
    }

    /// <summary>
    /// Walks every template's parent chain in one call, handing back the whole
    /// <c>_itemBaseClassesCache</c> and the root node ids beside it.
    /// </summary>
    /// <exception cref="InvalidOperationException">The build failed, or the native side misbehaved.</exception>
    public static ItemBaseClassResult BuildItemBaseClassCache(ItemBaseClassRequest request)
    {
        // Frozen pre-flip signature: an override send at epoch 0, never touching resident state.
        return BuildItemBaseClassCache(new ItemBaseClassNativeRequest { Epoch = 0, ViewsOverride = request });
    }

    /// <summary>
    /// Walks every template's parent chain in one call, off the resident templates root or the
    /// override the request carries.
    /// </summary>
    /// <exception cref="InvalidOperationException">The build failed, or the native side misbehaved.</exception>
    /// <exception cref="NativeStaleEpochException">An override-less request named an epoch the resident DB does not hold.</exception>
    internal static ItemBaseClassResult BuildItemBaseClassCache(ItemBaseClassNativeRequest request)
    {
        return Generate<ItemBaseClassResponse>(
            LootExport.ItemBaseClass,
            JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions)
        ).Result;
    }

    /// <summary>
    /// Builds the whole ragfair linked-item table in one call, handing back every template's
    /// linked-tpl set, reverse edges included.
    /// </summary>
    /// <exception cref="InvalidOperationException">The build failed, or the native side misbehaved.</exception>
    public static RagfairLinkedItemResult BuildRagfairLinkedItemTable(RagfairLinkedItemRequest request)
    {
        // Frozen pre-flip signature: an override send at epoch 0, never touching resident state.
        return BuildRagfairLinkedItemTable(new RagfairLinkedItemNativeRequest { Epoch = 0, ViewsOverride = request });
    }

    /// <summary>
    /// Builds the whole ragfair linked-item table in one call, off the resident templates root
    /// or the override the request carries.
    /// </summary>
    /// <exception cref="InvalidOperationException">The build failed, or the native side misbehaved.</exception>
    /// <exception cref="NativeStaleEpochException">An override-less request named an epoch the resident DB does not hold.</exception>
    internal static RagfairLinkedItemResult BuildRagfairLinkedItemTable(RagfairLinkedItemNativeRequest request)
    {
        return Generate<RagfairLinkedItemResponse>(
            LootExport.RagfairLinkedItems,
            JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions)
        ).Result;
    }

    /// <summary>
    /// Generates one bot's equipment, weapons and container loot.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    internal static BotInventoryResult GenerateBotInventory(GenerateBotInventoryRequest request)
    {
        return Generate<BotInventoryResult>(LootExport.BotInventory, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Generates a whole wave of bots in one call, with the shared config views - and, on the
    /// ineligible arm, the database views override - on the wire once instead of once per bot.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    internal static BotInventoryBatchResult GenerateBotInventoryBatch(GenerateBotInventoryBatchRequest request)
    {
        return Generate<BotInventoryBatchResult>(
            LootExport.BotInventoryBatch,
            JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions)
        );
    }

    /// <summary>
    /// Generates one full batch of dynamic flea offers. Unlike the other exports the response is
    /// a framed envelope — encoding tag, length-prefixed header, one length-prefixed payload per
    /// offer — deserialized in parallel straight into <see cref="RagfairOffer"/>.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    internal static FramedOffersResult GenerateDynamicOffers(GenerateDynamicOffersRequest request)
    {
        return GenerateDynamicOffersFramed(JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// Generates one repeatable quest of the type the request names, plus the pool the generator
    /// mutated on the way.
    /// </summary>
    /// <exception cref="NativeStaleEpochException">An override-less request named an epoch the resident DB does not hold.</exception>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    internal static RepeatableQuestResult GenerateRepeatableQuest(GenerateRepeatableQuestRequest request)
    {
        return GenerateRepeatableQuest(JsonSerializer.SerializeToUtf8Bytes(request, QuestJsonOptions));
    }

    /// <summary>
    /// The raw-bytes seam of <see cref="GenerateRepeatableQuest(GenerateRepeatableQuestRequest)"/>,
    /// kept internal so tests can hand it JSON no typed payload can express - a mod-added field, or
    /// a malformed request.
    /// </summary>
    internal static RepeatableQuestResult GenerateRepeatableQuest(ReadOnlySpan<byte> requestUtf8)
    {
        return Generate<RepeatableQuestResult>(LootExport.RepeatableQuest, requestUtf8, QuestJsonOptions);
    }

    /// <summary>
    /// The raw-bytes seam of <see cref="GenerateDynamicOffers"/>, kept internal so tests can hand
    /// it JSON no typed payload can express — a mod-added field, or a malformed request.
    /// </summary>
    internal static unsafe FramedOffersResult GenerateDynamicOffersFramed(ReadOnlySpan<byte> requestUtf8)
    {
        EnsureLoadable();

        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = NativeMethods.GenerateDynamicOffers(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen);
        }

        return DecodeResult(
            "DynamicOffers",
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                return ParseFramedOffers((byte*)buffer, length);
            }
        );
    }

    /// <summary>
    /// Publishes a full-table envelope into the native resident DB, answering the epoch now
    /// current — the value callers stamp into their requests.
    /// </summary>
    /// <exception cref="InvalidOperationException">The publish failed, or the native side misbehaved.</exception>
    internal static unsafe ulong DbPublish(ReadOnlySpan<byte> requestUtf8)
    {
        EnsureLoadable();

        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = NativeMethods.DbPublish(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen);
        }

        return DecodeResult(
            "DbPublish",
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                // The response body is {"epoch":N}
                using var document = JsonDocument.Parse(new ReadOnlySpan<byte>((byte*)buffer, length).ToArray());

                return document.RootElement.GetProperty("epoch").GetUInt64();
            }
        );
    }

    /// <summary>
    /// Per-root canonical digests of the native resident DB's typed lift surface — test support
    /// for the load/projection equivalence gate. Digests are toolchain-stable but no wire
    /// contract; compare only within one run.
    /// The <c>configs</c> digest is NOT a pure function of the parsed input (ConfigsRoot's HashSet
    /// lifts serialize in per-instance order) — compare only the five table roots across parses.
    /// </summary>
    /// <exception cref="InvalidOperationException">The native side misbehaved.</exception>
    internal static unsafe DbDigestResult DbResidentDigest()
    {
        EnsureLoadable();

        byte* outPtr = null;
        nuint outLen = 0;
        var status = NativeMethods.DbResidentDigest(&outPtr, &outLen);

        return DecodeResult(
            "DbResidentDigest",
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                using var document = JsonDocument.Parse(new ReadOnlySpan<byte>((byte*)buffer, length).ToArray());
                var roots = new Dictionary<string, string>();
                foreach (var member in document.RootElement.GetProperty("roots").EnumerateObject())
                {
                    roots[member.Name] = member.Value.GetString()!;
                }

                return new DbDigestResult { Epoch = document.RootElement.GetProperty("epoch").GetUInt64(), Roots = roots };
            }
        );
    }

    /// <summary>
    /// Loads SPT_Data in one native pass: hash-verify (when asked), read the tree, install the
    /// resident DB roots, and hand back the eager file bytes for the managed replica. Any
    /// <paramref name="handbookPriceOverride"/> entries are merged into the installed roots only -
    /// the returned file bytes stay exactly what is on disk, so the managed replica still hydrates
    /// them itself through <c>HandbookHelper</c>.
    /// </summary>
    /// <exception cref="InvalidOperationException">The load failed, or the native side misbehaved.</exception>
    internal static unsafe DbLoadResult DbLoad(
        string sptDataDir,
        bool verify,
        IReadOnlyDictionary<MongoId, HandbookPriceOverride>? handbookPriceOverride = null
    )
    {
        EnsureLoadable();

        // Default options on purpose: the request has three scalar members and no model types, so
        // the loot converters would only risk renaming them away from what db/load.rs reads. The
        // override map is projected to plain strings and numbers for the same reason - and because
        // a MongoId-keyed dictionary under default options throws NotSupportedException.
        var requestUtf8 = JsonSerializer.SerializeToUtf8Bytes(
            new
            {
                schema = 1,
                dir = sptDataDir,
                verify,
                handbookPriceOverride = handbookPriceOverride?.ToDictionary(
                    kv => kv.Key.ToString(),
                    kv => new { parentId = kv.Value.ParentId.ToString(), price = kv.Value.Price }
                ),
            }
        );
        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = NativeMethods.DbLoad(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen);
        }

        return DecodeResult(
            "DbLoad",
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                return ParseLoadFrame((byte*)buffer, length);
            }
        );
    }

    /// <summary>
    /// The framed load response: a u32-LE header length, the header JSON, then one blob per
    /// <c>files[]</c> entry in order. Every blob is copied into a managed array - the native buffer
    /// is freed as soon as this returns.
    /// </summary>
    private static unsafe DbLoadResult ParseLoadFrame(byte* buffer, int length)
    {
        var span = new ReadOnlySpan<byte>(buffer, length);
        var headerLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span));
        var at = 4;
        if (headerLength > length - at)
        {
            throw new InvalidOperationException(
                $"spt_native spt_db_load claims a {headerLength}-byte header with only {length - at} bytes in the frame."
            );
        }

        var header =
            JsonSerializer.Deserialize<DbLoadHeader>(span.Slice(at, headerLength))
            ?? throw new InvalidOperationException("spt_native returned an empty spt_db_load header.");
        at += headerLength;

        var files = new Dictionary<string, ReadOnlyMemory<byte>>(header.Files.Count);
        foreach (var entry in header.Files)
        {
            if (entry.Len < 0 || entry.Len > length - at)
            {
                throw new InvalidOperationException(
                    $"spt_native spt_db_load claims {entry.Len} bytes for {entry.Path} with only {length - at} bytes left."
                );
            }

            files[entry.Path] = span.Slice(at, entry.Len).ToArray();
            at += entry.Len;
        }

        if (at != length)
        {
            throw new InvalidOperationException($"spt_native spt_db_load frame has {length - at} trailing bytes.");
        }

        return new DbLoadResult
        {
            Verify = header.Verify,
            Epoch = header.Epoch,
            Files = files,
        };
    }

    /// <summary>
    /// Every file name in the profiles directory, sorted, the directory created when it is missing.
    /// Nothing is filtered here - the <c>.json</c> extension and MongoId-stem gates stay in
    /// SaveServer, verbatim.
    /// </summary>
    /// <exception cref="InvalidOperationException">The read failed, or the native side misbehaved.</exception>
    internal static Task<IReadOnlyList<string>> ProfileListAsync(string profilesDir)
    {
        // No CancellationToken: one bounded blocking directory read that cannot be interrupted once
        // in flight, so accepting a token would promise cancellation it can't deliver.
        return Task.Run(() => ProfileList(profilesDir));
    }

    private static unsafe IReadOnlyList<string> ProfileList(string profilesDir)
    {
        EnsureLoadable();

        // Default options on purpose: scalar members only, wire names pinned by profile.rs.
        var requestUtf8 = JsonSerializer.SerializeToUtf8Bytes(new { schema = 1, dir = profilesDir });
        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = NativeMethods.ProfileList(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen);
        }

        return DecodeResult<IReadOnlyList<string>>(
            "spt_profile_list",
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                // The response body is {"files":[...]}
                return JsonSerializer.Deserialize<ProfileListResponse>(new ReadOnlySpan<byte>((byte*)buffer, length))?.Files
                    ?? throw new InvalidOperationException("spt_native returned an empty spt_profile_list response.");
            }
        );
    }

    private sealed record ProfileListResponse([property: JsonPropertyName("files")] List<string> Files);

    /// <summary>
    /// One profile's stored bytes, or <see cref="ProfileLoadResult.Found"/> false when the file is
    /// not there. Corrupt JSON is not detected here - the caller discovers it on deserialize and
    /// runs its unchanged backup-recovery arm.
    /// </summary>
    /// <exception cref="InvalidOperationException">The read failed, or the native side misbehaved.</exception>
    internal static Task<ProfileLoadResult> ProfileLoadAsync(string profilesDir, MongoId sessionId)
    {
        // No CancellationToken: one bounded blocking file read (ProfileListAsync's reasoning).
        return Task.Run(() => ProfileLoad(profilesDir, sessionId));
    }

    private static unsafe ProfileLoadResult ProfileLoad(string profilesDir, MongoId sessionId)
    {
        EnsureLoadable();

        // Default options on purpose: scalar members only, wire names pinned by profile.rs.
        var requestUtf8 = JsonSerializer.SerializeToUtf8Bytes(
            new
            {
                schema = 1,
                dir = profilesDir,
                id = sessionId.ToString(),
            }
        );
        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = NativeMethods.ProfileLoad(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen);
        }

        return DecodeResult(
            "spt_profile_load",
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                return ParseProfileFrame((byte*)buffer, length);
            }
        );
    }

    /// <summary>
    /// The framed profile response: a u32-LE header length, the header JSON, then the file's bytes.
    /// The blob is copied into a managed array - the native buffer is freed as soon as this returns.
    /// </summary>
    private static unsafe ProfileLoadResult ParseProfileFrame(byte* buffer, int length)
    {
        var span = new ReadOnlySpan<byte>(buffer, length);
        var headerLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span));
        var at = 4;
        if (headerLength > length - at)
        {
            throw new InvalidOperationException(
                $"spt_native spt_profile_load claims a {headerLength}-byte header with only {length - at} bytes in the frame."
            );
        }

        var header =
            JsonSerializer.Deserialize<ProfileLoadHeader>(span.Slice(at, headerLength))
            ?? throw new InvalidOperationException("spt_native returned an empty spt_profile_load header.");
        at += headerLength;

        return new ProfileLoadResult(header.Found, header.Found ? span[at..].ToArray() : default);
    }

    private sealed record ProfileLoadHeader([property: JsonPropertyName("found")] bool Found);

    /// <summary>
    /// Writes one profile through the native temp-then-rename protocol.
    /// </summary>
    /// <exception cref="InvalidOperationException">The write failed, or the native side misbehaved.</exception>
    internal static Task ProfileSaveAsync(string profilesDir, MongoId sessionId, string profileJson)
    {
        // No CancellationToken: one bounded blocking write+rename that cannot be interrupted once in
        // flight; SaveServer checks its token before it gets here.
        return Task.Run(() => ProfileSave(profilesDir, sessionId, profileJson));
    }

    private static unsafe void ProfileSave(string profilesDir, MongoId sessionId, string profileJson)
    {
        EnsureLoadable();

        // Capacity is a UTF-16 char count sizing a UTF-8 buffer: exact for the ASCII the profile
        // almost entirely is, one doubling for the odd non-ASCII player name. GetMaxByteCount would
        // reserve 3x of tens of MB to avoid a realloc that costs less than the reservation.
        using var request = new MemoryStream(profileJson.Length + 128);
        using (var writer = new Utf8JsonWriter(request))
        {
            writer.WriteStartObject();
            writer.WriteNumber("schema", 1);
            writer.WriteString("dir", profilesDir);
            writer.WriteString("id", sessionId.ToString());
            writer.WritePropertyName("profile");
            // Byte-fidelity is the contract: what jsonUtil.Serialize produced is what Rust writes to
            // disk, and RawValue carries it verbatim. Re-serialising through a JsonNode here would
            // re-escape every Cyrillic item name and normalise the whitespace, and the damage would
            // land on disk. WriteRawValue embeds it without touching a byte.
            writer.WriteRawValue(profileJson, skipInputValidation: true);
            writer.WriteEndObject();
        }

        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        // The backing array is pinned whole and bounded by the stream's length: slicing it first
        // would copy the entire envelope into a fresh array, which on a profile this size is the one
        // allocation worth avoiding here.
        fixed (byte* requestPtr = request.GetBuffer())
        {
            status = NativeMethods.ProfileSave(requestPtr, (nuint)request.Length, &outPtr, &outLen);
        }

        // The response body is {} - nothing to read, but the status ladder and the free still apply.
        DecodeResult<bool>(
            "spt_profile_save",
            status,
            outPtr,
            outLen,
            (_, _) =>
            {
                return true;
            }
        );
    }

    /// <summary>
    /// Removes one profile. <c>true</c> = the file was there and is gone; <c>false</c> = it was not
    /// there, which the caller logs exactly as <c>fileUtil.DeleteFile</c>'s false return does today.
    /// </summary>
    /// <exception cref="InvalidOperationException">The delete failed, or the native side misbehaved.</exception>
    internal static unsafe bool ProfileDelete(string profilesDir, MongoId sessionId)
    {
        EnsureLoadable();

        // Default options on purpose: scalar members only, wire names pinned by profile.rs.
        var requestUtf8 = JsonSerializer.SerializeToUtf8Bytes(
            new
            {
                schema = 1,
                dir = profilesDir,
                id = sessionId.ToString(),
            }
        );
        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = NativeMethods.ProfileDelete(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen);
        }

        return DecodeResult(
            "spt_profile_delete",
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                // The response body is {"deleted":bool}
                using var document = JsonDocument.Parse(new ReadOnlySpan<byte>((byte*)buffer, length).ToArray());

                return document.RootElement.GetProperty("deleted").GetBoolean();
            }
        );
    }

    /// <summary>
    /// The status and ownership ladder the buffer-returning exports share: decode on success, otherwise
    /// read the failure message out of the same buffer, and free it either way.
    /// </summary>
    private static unsafe TResult DecodeResult<TResult>(
        string export,
        int status,
        byte* outPtr,
        nuint outLen,
        Func<nint, int, TResult> decode
    )
    {
        // Unlike verify, these exports also write a buffer when they fail - the error message - so
        // ownership is decided by the pointer, never by the status. BufFree ignores the null pointer
        // a null-argument rejection leaves behind.
        try
        {
            if (status == StatusOk)
            {
                // A response decode is `JsonSerializer.Deserialize` into objects that do not exist
                // yet, but plenty of them are the same model types the published roots are made of -
                // a repeatable quest carries Quest's condition types, a spawn point carries
                // SpawnpointTemplate. Barriered, every native call would dirty the stamp it just
                // read and pay a five-root republish on the next one. Nothing here can reach an
                // object a published root already points at, so the writes convey no freshness.
                // Scoped to the calling thread: the framed-offers decode fans out to worker threads,
                // which carry no barriered types today (RagfairOffer holds Item, and Item is denied).
                using (WriteBarrier.Suppress())
                {
                    return decode((nint)outPtr, checked((int)outLen));
                }
            }

            var message = outPtr == null ? "no message" : Encoding.UTF8.GetString(outPtr, checked((int)outLen));
            if (status == StatusStaleEpoch)
            {
                throw new NativeStaleEpochException(message);
            }

            if (status == StatusError)
            {
                // "failed", not "generation failed": the profile exports write and delete files, so
                // the shared verb has to fit an I/O failure as well as a generation one.
                throw new InvalidOperationException($"spt_native {export} failed: {message}");
            }

            if (status == StatusPanic)
            {
                throw new InvalidOperationException(
                    $"spt_native {export} panicked: {message}; this indicates a native library bug, not corrupt game data."
                );
            }

            throw new InvalidOperationException(
                $"spt_native {export} failed with internal status {status}: {message}; this indicates a native library bug, not corrupt game data."
            );
        }
        finally
        {
            NativeMethods.BufFree(outPtr, outLen);
        }
    }

    private const byte PayloadJson = 0;
    private const byte PayloadMsgpack = 1;

    private static unsafe FramedOffersResult ParseFramedOffers(byte* buffer, int length)
    {
        var span = new ReadOnlySpan<byte>(buffer, length);
        var encoding = span[0];
        var at = 1;

        var headerLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span[at..]));
        at += 4;
        var header =
            DeserializeHeader(encoding, span.Slice(at, headerLength))
            ?? throw new InvalidOperationException("spt_native returned an empty DynamicOffers header.");
        at += headerLength;

        var offerCount = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span[at..]));
        at += 4;
        // Every frame carries at least its own 4-byte length prefix, so a count past that bound is
        // corrupt - reject it before it sizes an allocation.
        if (offerCount > (length - at) / 4)
        {
            throw new InvalidOperationException(
                $"spt_native DynamicOffers envelope claims {offerCount} offers with only {length - at} bytes left."
            );
        }

        var frames = new (int Offset, int Length)[offerCount];
        for (var i = 0; i < offerCount; i++)
        {
            var frameLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span[at..]));
            frames[i] = (at + 4, frameLength);
            at += 4 + frameLength;
        }

        if (at != length)
        {
            throw new InvalidOperationException($"spt_native DynamicOffers envelope has {length - at} trailing bytes.");
        }

        var offers = new RagfairOffer[offerCount];
        var basePointer = (nint)buffer;
        Parallel.For(
            0,
            offerCount,
            i =>
            {
                offers[i] = DeserializeOfferFrame(encoding, basePointer, frames[i].Offset, frames[i].Length);
            }
        );

        return new FramedOffersResult { Offers = [.. offers], RejectedCanSellTemplates = header.RejectedCanSellTemplates };
    }

    private static DynamicOffersHeader? DeserializeHeader(byte encoding, ReadOnlySpan<byte> payload)
    {
        if (encoding == PayloadMsgpack)
        {
            return MsgpackOfferReader.ReadHeader(payload);
        }

        if (encoding != PayloadJson)
        {
            throw new InvalidOperationException($"unknown ragfair payload encoding {encoding}.");
        }

        return JsonSerializer.Deserialize<DynamicOffersHeader>(payload, LootJsonOptions);
    }

    private static unsafe RagfairOffer DeserializeOfferFrame(byte encoding, nint buffer, int offset, int length)
    {
        var frame = new ReadOnlySpan<byte>((byte*)buffer + offset, length);
        if (encoding == PayloadMsgpack)
        {
            return MsgpackOfferReader.ReadOffer(frame);
        }

        if (encoding != PayloadJson)
        {
            throw new InvalidOperationException($"unknown ragfair payload encoding {encoding}.");
        }

        var offer =
            JsonSerializer.Deserialize<RagfairOffer>(frame, LootJsonOptions)
            ?? throw new InvalidOperationException("spt_native returned an empty ragfair offer frame.");
        offer.CreatedBy = OfferCreator.FakePlayer;
        return offer;
    }

    /// <summary>
    /// The shared body of the generation wrappers, taking the request as the UTF-8 JSON the
    /// native side reads. Internal so tests can hand it JSON that no typed payload can express: a
    /// mod-added field, or a deliberately malformed request.
    /// </summary>
    internal static unsafe TResult Generate<TResult>(
        LootExport export,
        ReadOnlySpan<byte> requestUtf8,
        JsonSerializerOptions? options = null
    )
    {
        options ??= LootJsonOptions;

        EnsureLoadable();

        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = export switch
            {
                LootExport.StaticContainers => NativeMethods.GenerateStaticContainers(
                    requestPtr,
                    (nuint)requestUtf8.Length,
                    &outPtr,
                    &outLen
                ),
                LootExport.DynamicLoot => NativeMethods.GenerateDynamicLoot(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen),
                LootExport.CreateRandomLoot => NativeMethods.CreateRandomLoot(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen),
                LootExport.CreateForcedLoot => NativeMethods.CreateForcedLoot(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen),
                LootExport.SealedWeaponCase => NativeMethods.GetSealedWeaponCaseLoot(
                    requestPtr,
                    (nuint)requestUtf8.Length,
                    &outPtr,
                    &outLen
                ),
                LootExport.RandomLootContainer => NativeMethods.GetRandomLootContainerLoot(
                    requestPtr,
                    (nuint)requestUtf8.Length,
                    &outPtr,
                    &outLen
                ),
                LootExport.BotInventory => NativeMethods.GenerateBotInventory(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen),
                LootExport.BotInventoryBatch => NativeMethods.GenerateBotInventoryBatch(
                    requestPtr,
                    (nuint)requestUtf8.Length,
                    &outPtr,
                    &outLen
                ),
                LootExport.RepeatableQuest => NativeMethods.GenerateRepeatableQuest(
                    requestPtr,
                    (nuint)requestUtf8.Length,
                    &outPtr,
                    &outLen
                ),
                LootExport.ScavCaseRewards => NativeMethods.GenerateScavCaseRewards(
                    requestPtr,
                    (nuint)requestUtf8.Length,
                    &outPtr,
                    &outLen
                ),
                LootExport.RaidAdjustments => NativeMethods.GetRaidAdjustments(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen),
                LootExport.ItemBaseClass => NativeMethods.BuildItemBaseClassCache(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen),
                LootExport.RagfairLinkedItems => NativeMethods.BuildRagfairLinkedItemTable(
                    requestPtr,
                    (nuint)requestUtf8.Length,
                    &outPtr,
                    &outLen
                ),
                _ => throw new ArgumentOutOfRangeException(nameof(export), export, null),
            };
        }

        return DecodeResult(
            export.ToString(),
            status,
            outPtr,
            outLen,
            (buffer, length) =>
            {
                return JsonSerializer.Deserialize<TResult>(new ReadOnlySpan<byte>((byte*)buffer, length), options)
                    ?? throw new InvalidOperationException($"spt_native returned an empty {export} result.");
            }
        );
    }

    /// <summary>
    /// The options JsonUtil publishes at startup: loot payloads need its MongoId and enum converters.
    /// </summary>
    private static JsonSerializerOptions LootJsonOptions
    {
        get
        {
            return JsonUtil.JsonSerializerOptionsNoIndent
                ?? throw new InvalidOperationException("JsonUtil has not been built yet, so the loot payload converters are unavailable.");
        }
    }

    /// <summary>
    /// The loot payload options plus a string converter for <see cref="ELocationName"/>, which the
    /// quest payloads carry as dictionary keys on both the pool and the repeatable config.
    /// <c>EftEnumConverter</c> writes every unattributed enum as its numeric value, and the native
    /// side reads those keys as location names - so this one goes in front of it. Scoped to this
    /// family: the global options also write the profiles, whose stored shape is not ours to change.
    ///
    /// Derived once per source instance rather than once per process: <see cref="JsonUtil"/> replaces
    /// the static it reads on every container build, so a permanent memo would keep serialising with
    /// the converters of a container that is gone. Source and derived options publish as one
    /// reference, so a reader can never pair a source with options built from a different one; two
    /// threads racing a rebuild both produce equivalent options and the loser's copy is dropped.
    /// </summary>
    internal static JsonSerializerOptions QuestJsonOptions
    {
        get
        {
            var source = LootJsonOptions;
            var cached = _questJsonOptions;

            if (cached is null || !ReferenceEquals(cached.Source, source))
            {
                var options = new JsonSerializerOptions(source);
                // Appending would leave the global enum factory ahead of it: first match wins
                options.Converters.Insert(0, new JsonStringEnumConverter<ELocationName>());
                cached = new QuestOptions(source, options);
                _questJsonOptions = cached;
            }

            return cached.Derived;
        }
    }

    private sealed record QuestOptions(JsonSerializerOptions Source, JsonSerializerOptions Derived);

    private static QuestOptions? _questJsonOptions;

    private static unsafe VerifyResult VerifyDatabase(string sptDataDir)
    {
        EnsureLoadable();

        var dirUtf8 = Encoding.UTF8.GetBytes(sptDataDir);
        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* dirPtr = dirUtf8)
        {
            status = NativeMethods.VerifyDatabase(dirPtr, (nuint)dirUtf8.Length, &outPtr, &outLen);
        }

        // Throwing before the try/finally is safe here only because verify writes a buffer on
        // success alone. Do NOT copy this shape into the generate exports (ABI 4): those also write
        // a message buffer on BAD_ARGS and ERROR, so their wrappers must branch on outPtr, never on
        // the status, and free whenever it is non-null (only a null-arg BAD_ARGS writes nothing;
        // since ABI 18 PANIC carries the panic text).
        if (status != 0)
        {
            throw new InvalidOperationException(
                $"spt_native verification failed with internal status {status}; this indicates a native library bug, not corrupt game data."
            );
        }

        try
        {
            var json = new ReadOnlySpan<byte>(outPtr, checked((int)outLen));
            return JsonSerializer.Deserialize<VerifyResult>(json)
                ?? throw new InvalidOperationException("spt_native returned an empty verification result.");
        }
        finally
        {
            NativeMethods.BufFree(outPtr, outLen);
        }
    }
}

/// <summary>
///     A request named a resident-DB epoch the native process does not hold. The caller
///     self-heals by republishing the resident DB (<c>DbPublisher.ForcePublish()</c>) and
///     retrying once.
/// </summary>
internal sealed class NativeStaleEpochException(string message) : InvalidOperationException(message);
