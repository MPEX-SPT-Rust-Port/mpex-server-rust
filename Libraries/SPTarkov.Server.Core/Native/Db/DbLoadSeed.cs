using SPTarkov.Server.Core.Services.Server;

namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The one-shot handoff from the early-provider <c>DatabaseImporter</c> to the host container's
/// <see cref="DbPublisher"/>: the epoch the load-time install and configs publish left resident,
/// and the stamp read once the import walk finished. Static for the same reason
/// <see cref="DatabaseMutationStamp"/>'s counter is — the two containers share a process, not a
/// scope. Startup-internal; mods never touch it, and the test bootstrap never writes it
/// (seeding is opt-in at <c>LoadDatabaseAsync</c>).
/// </summary>
internal static class DbLoadSeed
{
    private static readonly Lock _gate = new();
    private static (ulong Epoch, long Stamp)? _seed;

    internal static void Set(ulong epoch, long stamp)
    {
        lock (_gate)
        {
            _seed = (epoch, stamp);
        }
    }

    /// <summary>Consumes the seed — a second call answers null.</summary>
    internal static (ulong Epoch, long Stamp)? TryTake()
    {
        lock (_gate)
        {
            var seed = _seed;
            _seed = null;

            return seed;
        }
    }
}
