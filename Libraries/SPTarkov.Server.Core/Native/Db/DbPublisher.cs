using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.Server;

namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The one write path to the native resident DB (spec § The epoch protocol). Dirty tracking is
/// the global <see cref="DatabaseMutationStamp"/>: a publish
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
    LocationTable locationTable,
    HideoutTable hideoutTable,
    IReadOnlyDictionary<Type, BaseConfig> configs
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
        // Everything from here to the end of the envelope build writes into the tables it is about
        // to serialize - HandbookHelper's lazy hydration below, and DbPayloadProjection's
        // LazyLoad.Value materialisation - so the barriers stay silent for the duration. Those
        // writes are in this payload by construction; letting them bump would make every publish
        // dirty the stamp it just read. The exception is any mod-registered LazyLoad transformer
        // that runs during projection - a transformer writing outside the value it transforms is
        // invisible for the same reason a decode callback's writes are.
        using (WriteBarrier.Suppress())
        {
            // HandbookHelper's first use lazily writes ItemConfig.HandbookPriceOverride entries INTO
            // templateTable.Handbook (HydrateHandbookCache). Force that hydration before the templates
            // root is serialized, or a publish that lands first would ship a handbook missing the
            // overrides. IsCategory is the cheapest public read that touches the cache without
            // mutating it.
            handbookHelper.IsCategory(Money.ROUBLES);

            _currentEpoch = SptNative.DbPublish(
                DbPayloadProjection.BuildPublishEnvelope(templateTable, tradersTable, globalTable, locationTable, hideoutTable, configs)
            );
        }

        _lastPublishedStamp = stamp;
    }
}
