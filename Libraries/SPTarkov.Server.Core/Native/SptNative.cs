using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Native.Loot;
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
/// Picks which of the two loot exports a request goes to.
/// </summary>
internal enum LootExport
{
    StaticContainers,
    DynamicLoot,
}

public static class SptNative
{
    private const uint ExpectedAbiVersion = 3;

    // ffi.rs
    private const int StatusOk = 0;
    private const int StatusError = 3;

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
    /// The shared body of the two generation wrappers, taking the request as the UTF-8 JSON the
    /// native side reads. Internal so tests can hand it JSON that no typed payload can express: a
    /// mod-added field, or a deliberately malformed request.
    /// </summary>
    internal static unsafe TResult Generate<TResult>(LootExport export, ReadOnlySpan<byte> requestUtf8)
    {
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
                _ => throw new ArgumentOutOfRangeException(nameof(export), export, null),
            };
        }

        // Unlike verify, these exports also write a buffer when they fail - the error message - so
        // ownership is decided by the pointer, never by the status. BufFree ignores the null pointer
        // a null-argument rejection or a panic leaves behind.
        try
        {
            if (status == StatusOk)
            {
                return JsonSerializer.Deserialize<TResult>(new ReadOnlySpan<byte>(outPtr, checked((int)outLen)), LootJsonOptions)
                    ?? throw new InvalidOperationException($"spt_native returned an empty {export} result.");
            }

            var message = outPtr == null ? "no message" : Encoding.UTF8.GetString(outPtr, checked((int)outLen));
            if (status == StatusError)
            {
                throw new InvalidOperationException($"spt_native {export} generation failed: {message}");
            }

            throw new InvalidOperationException(
                $"spt_native {export} generation failed with internal status {status}: {message}; this indicates a native library bug, not corrupt game data."
            );
        }
        finally
        {
            NativeMethods.BufFree(outPtr, outLen);
        }
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
        // success alone. Do NOT copy this shape into the generate exports (ABI 3): those also write
        // a message buffer on BAD_ARGS and ERROR, so their wrappers must branch on outPtr, never on
        // the status, and free whenever it is non-null (null-arg BAD_ARGS and PANIC write nothing).
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
