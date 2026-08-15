using SPTarkov.DI.Annotations;

namespace SPTarkov.Server.Core.Services.Server;

/// <summary>
///     A monotonic counter every instrumentable database mutation path bumps, so a native
///     request-slice cache can tell staleness. A mod writing an injected table's dictionaries
///     directly is invisible to it — that gap is closed by the cache eligibility gate
///     (no mods loaded, or <c>RagfairConfig.TrustNativeRequestCacheWithMods</c>), never here.
/// </summary>
[Injectable(InjectionType.Singleton)]
public class DatabaseMutationStamp
{
    private long _current;

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
        Interlocked.Increment(ref _current);
    }
}
