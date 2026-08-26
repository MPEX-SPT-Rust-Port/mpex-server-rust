using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
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
    IReadOnlyDictionary<Type, BaseConfig> configs,
    IReadOnlyList<SptMod> loadedMods,
    ISptLogger<DbPublisher> logger
)
{
    private readonly Lock _gate = new();
    private long? _lastPublishedStamp;
    private ulong _currentEpoch;

    public ulong EnsureCurrent()
    {
        lock (_gate)
        {
            ConsumeSeedOnce();

            var stamp = databaseMutationStamp.Current;
            if (_currentEpoch == 0 || _lastPublishedStamp != stamp)
            {
                PublishLocked(stamp);
            }

            return _currentEpoch;
        }
    }

    /// <summary>
    /// Republishes unconditionally — the self-heal after a <c>NativeStaleEpochException</c>. Never
    /// consumes the load-time seed: afterwards the epoch is non-zero, so a pending seed is dead for
    /// this publisher (production always reaches <see cref="EnsureCurrent"/> first; tests drain the
    /// slot in teardown).
    /// </summary>
    public ulong ForcePublish()
    {
        lock (_gate)
        {
            PublishLocked(databaseMutationStamp.Current);

            return _currentEpoch;
        }
    }

    /// <summary>
    /// Starts this publisher from the load-time install (spec § load-time seeding): modless only —
    /// a mod can schedule pre-GameCallbacks IOnLoad writes, and LazyLoad transformer registrations
    /// bump no stamp, so with mods loaded the first publish stays real. Forces the handbook
    /// hydration a first publish would have forced, under the same suppression, so the C#-visible
    /// hydration timing is unchanged and the first HandbookHelper use cannot bump the stamp.
    /// Logs whether the seed was honoured or voided — the Release boot proof greps both.
    /// </summary>
    private void ConsumeSeedOnce()
    {
        if (_currentEpoch != 0 || loadedMods.Count != 0)
        {
            return;
        }

        if (DbLoadSeed.TryTake() is { } seed)
        {
            using (WriteBarrier.Suppress())
            {
                handbookHelper.IsCategory(Money.ROUBLES);
            }

            if (seed.Stamp == databaseMutationStamp.Current)
            {
                logger.Info($"Load-time seed consumed at epoch {seed.Epoch}; first publish skipped.");
            }
            else
            {
                logger.Warning(
                    $"Load-time seed voided: stamp moved {seed.Stamp} -> {databaseMutationStamp.Current} before the first EnsureCurrent; republishing."
                );
            }

            _currentEpoch = seed.Epoch;
            _lastPublishedStamp = seed.Stamp;
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
