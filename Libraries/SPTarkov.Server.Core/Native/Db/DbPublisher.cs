using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.Server;

namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The one write path to the native resident DB (spec § The epoch protocol, amended
/// 2026-08-18). Dirty tracking is the global <see cref="DatabaseMutationStamp"/>: a publish
/// resends every supported root when the stamp has moved since the last publish. Callers stamp
/// the returned epoch into their requests; a <c>NativeStaleEpochException</c> self-heals with
/// <see cref="ForcePublish"/> + one retry.
/// </summary>
// ponytail: global stamp => all supported roots republished together; per-root stamps only if a
// hot mutation path ever shows up
[Injectable(InjectionType.Singleton)]
public class DbPublisher(
    DatabaseMutationStamp databaseMutationStamp,
    HandbookHelper handbookHelper,
    TemplateTable templateTable,
    TradersTable tradersTable,
    GlobalTable globalTable,
    LocationTable locationTable
)
{
    private readonly Lock _gate = new();
    private long? _lastPublishedStamp;
    private ulong _currentEpoch;

    public ulong EnsureCurrent()
    {
        lock (_gate)
        {
            var stamp = databaseMutationStamp.Current;
            if (_currentEpoch == 0 || _lastPublishedStamp != stamp)
            {
                PublishLocked(stamp);
            }

            return _currentEpoch;
        }
    }

    public ulong ForcePublish()
    {
        lock (_gate)
        {
            PublishLocked(databaseMutationStamp.Current);

            return _currentEpoch;
        }
    }

    private void PublishLocked(long stamp)
    {
        // HandbookHelper's first use lazily writes ItemConfig.HandbookPriceOverride entries INTO
        // templateTable.Handbook (HydrateHandbookCache). Force that hydration before the templates
        // root is serialized, or a publish that lands first would ship a handbook missing the
        // overrides. IsCategory is the cheapest public read that touches the cache without
        // mutating it.
        handbookHelper.IsCategory(Money.ROUBLES);

        _currentEpoch = SptNative.DbPublish(
            DbPayloadProjection.BuildPublishEnvelope(templateTable, tradersTable, globalTable, locationTable)
        );
        _lastPublishedStamp = stamp;
    }
}
