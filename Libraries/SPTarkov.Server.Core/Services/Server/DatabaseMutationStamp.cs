using SPTarkov.DI.Annotations;

namespace SPTarkov.Server.Core.Services.Server;

/// <summary>
///     A monotonic counter every instrumentable database mutation path bumps, so the resident-DB
///     publisher can tell when a republish is due. Since Phase 2 the bulk of those paths are
///     Ceciler-injected write barriers on the model setters reachable from the published roots
///     (<see cref="SPTarkov.Server.Core.Native.Db.WriteBarrier"/>), not hand-written call sites.
///     Container mutations - a mod calling Add/Remove/indexer-set on a table collection - remain
///     invisible; the kill switches cover them.
/// </summary>
[Injectable(InjectionType.Singleton)]
public class DatabaseMutationStamp
{
    // Static because the Ceciler-injected barriers call from inside model-type setters, which have
    // no path to the DI container. The resident DB's Rust-side store is process-global too, so a
    // process-global counter is the matching scope.
    private static long _current;

    /// <summary>
    ///     The current stamp value. Compared, never interpreted.
    /// </summary>
    public long Current
    {
        get { return Interlocked.Read(ref _current); }
    }

    /// <summary>
    ///     Record that database state a native request slice projects may have changed.
    /// </summary>
    public void Bump()
    {
        BumpGlobal();
    }

    internal static void BumpGlobal()
    {
        Interlocked.Increment(ref _current);
    }
}
