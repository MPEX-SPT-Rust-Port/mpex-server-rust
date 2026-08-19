namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The one resident-DB eligibility rule and the one stale-epoch self-heal, shared by
/// every flipped family (spec: epoch protocol, "C# driver"). Sites keep a one-line
/// <c>ResidentDbEligible()</c> wrapper that sources the flag pair from their own
/// config record and handles their frozen-constructor nullability.
/// </summary>
internal static class ResidentDbDispatch
{
    internal static bool Eligible(DbPublisher? publisher, int? loadedModCount, bool disableNativeRequestCache, bool trustWithMods)
    {
        if (publisher is null || loadedModCount is null || disableNativeRequestCache)
        {
            return false;
        }

        if (loadedModCount == 0)
        {
            return true;
        }

        // Trusting a mod means trusting the write barriers to see what it writes. A build Ceciler
        // never rewrote has none, so the flag cannot vouch for anything there (Phase 2).
        return trustWithMods && WriteBarrier.Installed;
    }

    /// <summary>
    /// Eligible arm only: stamp the current epoch into the request via <paramref name="send"/>;
    /// a native stale-epoch answer republishes everything and resends exactly once.
    /// </summary>
    internal static TResult Send<TResult>(DbPublisher publisher, Func<ulong, TResult> send)
    {
        var epoch = publisher.EnsureCurrent();
        try
        {
            return send(epoch);
        }
        catch (NativeStaleEpochException)
        {
            // The resident DB does not hold this epoch - republish everything and retry once
            return send(publisher.ForcePublish());
        }
    }
}
