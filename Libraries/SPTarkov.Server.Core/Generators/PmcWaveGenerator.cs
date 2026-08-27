using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Raid;

namespace SPTarkov.Server.Core.Generators;

/// <summary>
/// The custom-PMC wave splice runs in <c>rust/spt-native</c> by default, as the raid-setup family's
/// fifth pass; <see cref="RaidNativeRequestBuilder"/> projects the live config into the native
/// payload and the removal set comes back as indices. The full C# implementation is retained below
/// as the legacy path - it is the frozen mod contract - and runs instead of the native path when a
/// Harmony patch on any member of the family's frozen set is detected, when a mod substituted the
/// generator, when the frozen constructor built the instance or when
/// <see cref="LocationConfig.ForceLegacyRaidAdjustments"/> is set, so mod hooks fire with genuine
/// baseline semantics.
/// </summary>
[Injectable]
public class PmcWaveGenerator(LocationTable locationTable, PmcConfig pmcConfig)
{
    private readonly RaidNativeRequestBuilder? _requestBuilder;
    private readonly LocationConfig? _locationConfig;

    /// <summary>
    ///     The frozen constructor plus the native request builder. Additive and apicompat-verified.
    /// </summary>
    public PmcWaveGenerator(
        LocationTable locationTable,
        PmcConfig pmcConfig,
        RaidNativeRequestBuilder requestBuilder,
        LocationConfig locationConfig
    )
        : this(locationTable, pmcConfig)
    {
        _requestBuilder = requestBuilder;
        _locationConfig = locationConfig;
    }

    /// <summary>
    ///     Which implementation the most recent wave pass ran - the spt-native path or the retained
    ///     C# path. Test seam; also handy in a debugger. Unsynchronized - concurrent raid starts race
    ///     it - which only the non-parallel fixtures that assert on it may ignore.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     The legacy path runs when the frozen constructor built this instance (it has no native
    ///     seam to dispatch to), when forced by config, when any member of the family's frozen set
    ///     carries a live Harmony patch - a patch on a <c>RaidTimeAdjustmentService</c> member
    ///     counts, the set is family-wide - or when a mod has substituted the generator itself.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_requestBuilder is null || _locationConfig!.ForceLegacyRaidAdjustments)
        {
            return true;
        }

        if (RaidNativeRequestBuilder.AnyFrozenMemberPatched())
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return GetType() != typeof(PmcWaveGenerator);
    }

    /// <summary>
    ///     Add a pmc wave to a map
    /// </summary>
    /// <param name="locationId"> e.g. factory4_day, bigmap </param>
    /// <param name="waveToAdd"> Boss wave to add to map </param>
    public void AddPmcWaveToLocation(string locationId, BossLocationSpawn waveToAdd)
    {
        pmcConfig.CustomPmcWaves[locationId].Add(waveToAdd);
    }

    /// <summary>
    ///     Add custom boss and normal waves to all maps found in config/location.json to db
    /// </summary>
    public void ApplyWaveChangesToAllMaps()
    {
        foreach (var location in pmcConfig.CustomPmcWaves)
        {
            ApplyWaveChangesToMapByName(location.Key);
        }
    }

    /// <summary>
    ///     Add custom boss and normal waves to a map found in config/location.json to db by name
    /// </summary>
    /// <param name="name"> e.g. factory4_day, bigmap </param>
    public void ApplyWaveChangesToMapByName(string name)
    {
        if (!pmcConfig.CustomPmcWaves.TryGetValue(name, out var pmcWavesToAdd))
        {
            return;
        }

        var location = locationTable.GetLocation(name);
        location?.Base.BossLocationSpawn.AddRange(pmcWavesToAdd);
    }

    /// <summary>
    ///     Add custom boss and normal waves to a map found in config/location.json to db by LocationBase
    /// </summary>
    /// <param name="location"> Location Object </param>
    public void ApplyWaveChangesToMap(LocationBase location)
    {
        if (!UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Native;

            var (request, wavesToAdd) = _requestBuilder!.BuildApplyPmcWavesRequest(location);
            var response = _requestBuilder.SendApplyPmcWaves(request);
            if (response.Apply)
            {
                var spawns = location.BossLocationSpawn ?? [];
                var remove = response.RemoveIndices.ToHashSet();
                var kept = new List<BossLocationSpawn>(spawns.Count);
                for (var i = 0; i < spawns.Count; i++)
                {
                    if (!remove.Contains(i))
                    {
                        kept.Add(spawns[i]);
                    }
                }

                // Legacy's .Where().ToList() replaces the reference; the append aliases the config
                // objects in by reference - both preserved (spec D3)
                location.BossLocationSpawn = kept;
                location.BossLocationSpawn.AddRange(wavesToAdd!);
            }

            return;
        }

        LastPathTaken = LootGenerationPath.Legacy;

        // Only remove existing PMC waves if there are custom PMC waves to replace them with
        if (pmcConfig.RemoveExistingPmcWaves)
        {
            if (pmcConfig.CustomPmcWaves.TryGetValue(location.Id.ToLowerInvariant(), out var pmcWavesToAdd) && pmcWavesToAdd.Count > 0)
            {
                var pmcTypes = new HashSet<string> { "pmcUSEC", "pmcBEAR" };
                location.BossLocationSpawn = location.BossLocationSpawn.Where(bossSpawn => !pmcTypes.Contains(bossSpawn.BossName)).ToList();

                location.BossLocationSpawn.AddRange(pmcWavesToAdd);
            }
        }
    }
}
