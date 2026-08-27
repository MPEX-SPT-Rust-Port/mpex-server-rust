using Microsoft.Extensions.Logging;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Constants;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Game;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Location;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Raid;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Core.Services.InRaid;

/// <summary>
/// Raid setup runs in <c>rust/spt-native</c> by default; <see cref="RaidNativeRequestBuilder"/>
/// projects the live database and config into the native payload. The full C# implementation is
/// retained below as the legacy path - it is the frozen mod contract - and runs instead of the
/// native path when a Harmony patch on any member of the family's frozen set is detected, when a mod
/// substituted the service, when the frozen constructor built the instance or when
/// <see cref="LocationConfig.ForceLegacyRaidAdjustments"/> is set, so mod hooks fire with genuine
/// baseline semantics.
/// </summary>
[Injectable(InjectionType.Singleton)]
public class RaidTimeAdjustmentService(
    ISptLogger<RaidTimeAdjustmentService> logger,
    GlobalTable globalTable,
    LocationTable locationTable,
    RandomUtil randomUtil,
    WeightedRandomHelper weightedRandomHelper,
    ProfileActivityService profileActivityService,
    LocationConfig locationConfig
)
{
    private readonly RaidNativeRequestBuilder? _requestBuilder;

    /// <summary>
    ///     The frozen constructor plus the native request builder. Additive and apicompat-verified.
    /// </summary>
    public RaidTimeAdjustmentService(
        ISptLogger<RaidTimeAdjustmentService> logger,
        GlobalTable globalTable,
        LocationTable locationTable,
        RandomUtil randomUtil,
        WeightedRandomHelper weightedRandomHelper,
        ProfileActivityService profileActivityService,
        LocationConfig locationConfig,
        RaidNativeRequestBuilder requestBuilder
    )
        : this(logger, globalTable, locationTable, randomUtil, weightedRandomHelper, profileActivityService, locationConfig)
    {
        _requestBuilder = requestBuilder;
    }

    /// <summary>
    ///     Which implementation the most recent adjustment call ran - the spt-native path or the
    ///     retained C# path. Test seam; also handy in a debugger. Unsynchronized on a singleton -
    ///     concurrent raid starts race it - which only the non-parallel fixtures that assert on it
    ///     may ignore.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     Test-only seed forwarded as <see cref="GetRaidAdjustmentsRequest.TestSeed"/> on every
    ///     native request.
    /// </summary>
    internal ulong? NativeTestSeed { get; set; }

    /// <summary>
    ///     The legacy path runs when the frozen constructor built this instance (it has no native
    ///     seam to dispatch to), when forced by config, when any member of the family's frozen set
    ///     carries a live Harmony patch, or when a mod has substituted the service itself - running
    ///     the retained C# implementation is the only way those hooks and replacements can take
    ///     effect with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_requestBuilder is null || locationConfig.ForceLegacyRaidAdjustments)
        {
            return true;
        }

        if (RaidNativeRequestBuilder.AnyFrozenMemberPatched())
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return GetType() != typeof(RaidTimeAdjustmentService);
    }

    /// <summary>
    ///     Make alterations to the base map data passed in
    ///     Loot multipliers/waves/wave start times
    /// </summary>
    /// <param name="raidAdjustments">Changes to process on map</param>
    /// <param name="mapBase">Map to adjust</param>
    public void MakeAdjustmentsToMap(RaidChanges raidAdjustments, LocationBase mapBase)
    {
        if (raidAdjustments.DynamicLootPercent < 100 || raidAdjustments.StaticLootPercent < 100)
        {
            if (logger.IsLogEnabled(LogLevel.Debug))
            {
                logger.Debug(
                    $"Adjusting dynamic loot multipliers to: {raidAdjustments.DynamicLootPercent}% and static loot multipliers to: {raidAdjustments.StaticLootPercent}% of original"
                );
            }
        }

        // Change loot multiplier values before they're used below
        if (raidAdjustments.DynamicLootPercent < 100)
        {
            AdjustLootMultipliers(locationConfig.LooseLootMultiplier, raidAdjustments.DynamicLootPercent);
        }

        if (raidAdjustments.StaticLootPercent < 100)
        {
            AdjustLootMultipliers(locationConfig.StaticLootMultiplier, raidAdjustments.StaticLootPercent);
        }

        if (!UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Native;

            var (nativeRequest, exits) = _requestBuilder!.BuildMakeAdjustmentsRequest(raidAdjustments, mapBase);
            var deltas = _requestBuilder.SendMakeAdjustments(nativeRequest);
            ApplyMapAdjustmentDeltas(deltas, raidAdjustments, mapBase, exits);

            return;
        }

        LastPathTaken = LootGenerationPath.Legacy;

        // Adjust the escape time limit
        mapBase.EscapeTimeLimit = raidAdjustments.RaidTimeMinutes;

        // Adjust map exits
        foreach (var exitChange in raidAdjustments.ExitChanges)
        {
            var exitToChange = mapBase.Exits.FirstOrDefault(exit => exit.Name == exitChange.Name);
            if (exitToChange is null)
            {
                if (logger.IsLogEnabled(LogLevel.Debug))
                {
                    logger.Debug($"Exit with Id: {exitChange.Name} not found, skipping");
                }

                return;
            }

            if (exitChange.Chance is not null)
            {
                exitToChange.Chance = exitChange.Chance;
            }

            if (exitChange.MinTime is not null)
            {
                exitToChange.MinTime = exitChange.MinTime;
            }

            if (exitChange.MaxTime is not null)
            {
                exitToChange.MaxTime = exitChange.MaxTime;
            }
        }

        // Make alterations to bot spawn waves now player is simulated spawning later
        var mapSettings = GetMapSettings(mapBase.Id);
        if (mapSettings.AdjustWaves)
        {
            AdjustWaves(mapBase, raidAdjustments);

            AdjustPMCSpawns(mapBase, raidAdjustments);
        }
    }

    /// <summary>
    ///     Lands one native pass's deltas on the live map, in the order the legacy body wrote them,
    ///     and re-emits the log lines the native side has no logger for.
    /// </summary>
    /// <param name="deltas">What the native pass worked out</param>
    /// <param name="raidAdjustments">The changes the pass ran on - read only for its debug lines</param>
    /// <param name="mapBase">The map to write to</param>
    /// <param name="exits">
    ///     The exit list the request was projected from, which every exit index indexes. Never
    ///     <c>mapBase.Exits</c> re-enumerated - that is a fresh sequence, not this one.
    /// </param>
    private void ApplyMapAdjustmentDeltas(
        MakeAdjustmentsResponse deltas,
        RaidChanges raidAdjustments,
        LocationBase mapBase,
        List<Exit> exits
    )
    {
        mapBase.EscapeTimeLimit = deltas.EscapeTimeLimit;

        foreach (var exitUpdate in deltas.ExitUpdates)
        {
            var exitToChange = exits[exitUpdate.Index];

            if (exitUpdate.Chance is not null)
            {
                exitToChange.Chance = exitUpdate.Chance;
            }

            if (exitUpdate.MinTime is not null)
            {
                exitToChange.MinTime = exitUpdate.MinTime;
            }

            if (exitUpdate.MaxTime is not null)
            {
                exitToChange.MaxTime = exitUpdate.MaxTime;
            }
        }

        if (deltas.Aborted)
        {
            // The unmatched-exit return: the updates above are what legacy had already written when
            // it bailed, and the map settings are never even looked at
            if (logger.IsLogEnabled(LogLevel.Debug))
            {
                logger.Debug($"Exit with Id: {deltas.AbortedExitName} not found, skipping");
            }

            return;
        }

        if (deltas.MapSettingsMissingValue)
        {
            logger.Warning($"Unable to find scav raid time settings for map: {mapBase.Id}, using defaults");
        }

        if (deltas.WaveAdjustments is not { } waveAdjustments)
        {
            return;
        }

        // Both lists are reassigned below, so the originals - the ones every keep and time index
        // points into - have to be held first
        var originalWaves = mapBase.Waves;
        var originalSpawns = mapBase.BossLocationSpawn;

        mapBase.Waves = waveAdjustments.WaveKeepIndices.Select(index => originalWaves[index]).ToList();
        for (var index = 0; index < mapBase.Waves.Count; index++)
        {
            mapBase.Waves[index].TimeMin = waveAdjustments.WaveTimes[index].TimeMin;
            mapBase.Waves[index].TimeMax = waveAdjustments.WaveTimes[index].TimeMax;
        }

        if (logger.IsLogEnabled(LogLevel.Debug))
        {
            logger.Debug(
                $"Removed: {waveAdjustments.RemovedWaveCount} wave from map due to simulated raid start time of: {raidAdjustments.SimulatedRaidStartSeconds / 60} minutes"
            );
        }

        mapBase.BossLocationSpawn = waveAdjustments.BossKeepIndices.Select(index => originalSpawns[index]).ToList();

        foreach (var bossTimeUpdate in waveAdjustments.BossTimeUpdates)
        {
            // Into the original list on purpose: legacy offset the spawn objects themselves, and on
            // a map that took the custom-PMC splice those objects are the live PmcConfig's
            originalSpawns[bossTimeUpdate.Index].Time = bossTimeUpdate.Time;
        }

        if (logger.IsLogEnabled(LogLevel.Debug))
        {
            if (waveAdjustments.PmcStartSeconds is not null)
            {
                logger.Debug($"Offset PMC spawns by: {waveAdjustments.PmcStartSeconds} seconds");
            }

            logger.Debug(
                $"Removed: {waveAdjustments.RemovedBossCount} boss waves from map due to simulated raid start time of: {raidAdjustments.SimulatedRaidStartSeconds / 60} minutes"
            );
        }
    }

    /// <summary>
    ///     Adjust the loot multiplier values passed in to be a % of their original value
    /// </summary>
    /// <param name="mapLootMultipliers">Multipliers to adjust</param>
    /// <param name="loosePercent">Percent to change values to</param>
    protected void AdjustLootMultipliers(Dictionary<string, double> mapLootMultipliers, double? loosePercent)
    {
        foreach (var location in mapLootMultipliers)
        {
            mapLootMultipliers[location.Key] = randomUtil.GetPercentOfValue(mapLootMultipliers[location.Key], loosePercent ?? 1);
        }
    }

    /// <summary>
    ///     Adjust bot waves to act as if player spawned later
    /// </summary>
    /// <param name="mapBase">Map to adjust</param>
    /// <param name="raidAdjustments">Map adjustments</param>
    protected void AdjustWaves(LocationBase mapBase, RaidChanges raidAdjustments)
    {
        // Remove waves that spawned before the player joined
        var originalWaveCount = mapBase.Waves.Count;
        mapBase.Waves = mapBase.Waves.Where(wave => wave.TimeMax > raidAdjustments.SimulatedRaidStartSeconds).ToList();

        // Adjust wave min/max times to match new simulated start
        var startSeconds = raidAdjustments.SimulatedRaidStartSeconds.GetValueOrDefault(1);
        foreach (var wave in mapBase.Waves)
        {
            // Don't let time fall below 0
            wave.TimeMin -= (int)Math.Max(startSeconds, 0);
            wave.TimeMax -= (int)Math.Max(startSeconds, 0);
        }
        if (logger.IsLogEnabled(LogLevel.Debug))
        {
            logger.Debug(
                $"Removed: {originalWaveCount - mapBase.Waves.Count} wave from map due to simulated raid start time of: {raidAdjustments.SimulatedRaidStartSeconds / 60} minutes"
            );
        }
    }

    /// <summary>
    ///
    /// </summary>
    /// <param name="mapBase">Map to adjust</param>
    /// <param name="raidAdjustments">Map adjustments</param>
    protected void AdjustPMCSpawns(LocationBase mapBase, RaidChanges raidAdjustments)
    {
        var originalPmcWaveCount = mapBase.BossLocationSpawn.Count;

        // Filter PMCs by spawn time but allow all normal boss types (e.g. Tagilla/Killa)
        mapBase.BossLocationSpawn = mapBase
            .BossLocationSpawn.Where(boss =>
                boss.Time > raidAdjustments.SimulatedRaidStartSeconds // Spawns after simulated player start
                || (
                    !string.Equals(boss.BossName, "pmcusec", StringComparison.OrdinalIgnoreCase) // or
                    && !string.Equals(boss.BossName, "pmcbear", StringComparison.OrdinalIgnoreCase) // isn't a pmc
                )
            )
            .ToList();

        // Adjust wave min/max times to match new simulated start
        var startSeconds = raidAdjustments.SimulatedRaidStartSeconds.GetValueOrDefault(1);
        foreach (var wave in mapBase.Waves)
        {
            // Don't let time fall below 0
            wave.TimeMin -= (int)Math.Max(startSeconds, 0);
            wave.TimeMax -= (int)Math.Max(startSeconds, 0);
        }

        // Now additionally move all PMCs back so they spawn starting at the beginning of the raid
        var pmcSpawns = mapBase.BossLocationSpawn.Where(boss => boss.BossName is Sides.PmcUsec or Sides.PmcBear);
        var firstPmcSpawn = pmcSpawns.OrderBy(boss => boss.Time).FirstOrDefault();
        if (firstPmcSpawn != null)
        {
            var pmcStartSeconds = firstPmcSpawn.Time.GetValueOrDefault(1);
            foreach (var spawn in pmcSpawns)
            {
                // Sanity check, the client won't spawn a time of 0
                spawn.Time = (double)Math.Max(spawn.Time.GetValueOrDefault(1) - pmcStartSeconds, 1);
            }

            if (logger.IsLogEnabled(LogLevel.Debug))
            {
                logger.Debug($"Offset PMC spawns by: {pmcStartSeconds} seconds");
            }
        }
        if (logger.IsLogEnabled(LogLevel.Debug))
        {
            logger.Debug(
                $"Removed: {originalPmcWaveCount - mapBase.BossLocationSpawn.Count} boss waves from map due to simulated raid start time of: {raidAdjustments.SimulatedRaidStartSeconds / 60} minutes"
            );
        }
    }

    /// <summary>
    ///     Create a randomised adjustment to the raid based on map data in location.json
    /// </summary>
    /// <param name="sessionId">Session id</param>
    /// <param name="request">Raid adjustment request</param>
    /// <returns>Response to send to client</returns>
    public RaidChanges GetRaidAdjustments(MongoId sessionId, GetRaidTimeRequest request)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;

            return GetRaidAdjustmentsLegacy(sessionId, request);
        }

        LastPathTaken = LootGenerationPath.Native;

        // The same dereference the legacy body opens with, so a null location or an unknown map
        // still throws here and not one call deeper
        var mapBase = locationTable.GetLocation(request.Location.ToLowerInvariant()).Base;
        var nativeRequest = _requestBuilder!.BuildGetRaidAdjustmentsRequest(request, mapBase, NativeTestSeed);
        var response = _requestBuilder.SendGetRaidAdjustments(nativeRequest);

        if (response.MapSettingsMissingValue)
        {
            logger.Warning($"Unable to find scav raid time settings for map: {request.Location}, using defaults");
        }

        if (response.Applied)
        {
            if (logger.IsLogEnabled(LogLevel.Debug))
            {
                logger.Debug(
                    $"Reduced: {request.Location} raid time by: {response.ChosenReductionPercent}% to {response.RaidChanges.RaidTimeMinutes} minutes"
                );

                foreach (var exitChange in response.RaidChanges.ExitChanges ?? [])
                {
                    // The disable branch is the one that sets a Chance, and its own debug line is a
                    // booked drop - the value it printed never crosses the wire
                    if (exitChange.Chance is null)
                    {
                        logger.Debug($"Train appears between: {exitChange.MinTime} and: {exitChange.MaxTime} seconds raid time");
                    }
                }
            }

            // Store state to use in loot generation
            profileActivityService.GetProfileActivityRaidData(sessionId).RaidAdjustments = response.RaidChanges;
        }

        return response.RaidChanges;
    }

    /// <summary>
    ///     The retained C# implementation of <see cref="GetRaidAdjustments"/>, unchanged.
    /// </summary>
    private RaidChanges GetRaidAdjustmentsLegacy(MongoId sessionId, GetRaidTimeRequest request)
    {
        var mapBase = locationTable.GetLocation(request.Location.ToLowerInvariant()).Base;
        var baseEscapeTimeMinutes = mapBase.EscapeTimeLimit;

        // Prep result object to return
        var result = new RaidChanges
        {
            NewSurviveTimeSeconds = globalTable.Configuration.Exp.MatchEnd.SurvivedSecondsRequirement,
            OriginalSurvivalTimeSeconds = globalTable.Configuration.Exp.MatchEnd.SurvivedSecondsRequirement,
            DynamicLootPercent = 100,
            StaticLootPercent = 100,
            SimulatedRaidStartSeconds = 0,
            RaidTimeMinutes = baseEscapeTimeMinutes,
            ExitChanges = [],
        };

        // Pmc raid, send default
        if (string.Equals(request.Side, "pmc", StringComparison.OrdinalIgnoreCase))
        {
            return result;
        }

        // We're scav, adjust values
        var mapSettings = GetMapSettings(request.Location);

        // Chance of reducing raid time for scav, not guaranteed
        if (!randomUtil.GetChance100(mapSettings.ReducedChancePercent))
        // Send default
        {
            return result;
        }

        // Get the weighted percent to reduce the raid time by
        var chosenRaidReductionPercent = int.Parse(weightedRandomHelper.GetWeightedValue(mapSettings.ReductionPercentWeights));
        var raidTimeRemainingPercent = 100 - chosenRaidReductionPercent;

        // How many minutes raid will last
        var newRaidTimeMinutes = Math.Floor(randomUtil.ReduceValueByPercent(baseEscapeTimeMinutes ?? 1d, chosenRaidReductionPercent));

        // Time player spawns into the raid if it was online
        var simulatedRaidStartTimeMinutes = baseEscapeTimeMinutes - newRaidTimeMinutes;
        result.SimulatedRaidStartSeconds = simulatedRaidStartTimeMinutes * 60d;
        result.RaidTimeMinutes = newRaidTimeMinutes;

        // Calculate how long player needs to be in raid to get a `survived` extract status, never falls below 0
        result.NewSurviveTimeSeconds = Math.Max(
            (result.OriginalSurvivalTimeSeconds - ((baseEscapeTimeMinutes - newRaidTimeMinutes) * 60)) ?? 0d,
            0d
        );

        if (mapSettings.ReduceLootByPercent)
        {
            result.DynamicLootPercent = Math.Max(raidTimeRemainingPercent, mapSettings.MinDynamicLootPercent);
            result.StaticLootPercent = Math.Max(raidTimeRemainingPercent, mapSettings.MinStaticLootPercent);
        }

        if (logger.IsLogEnabled(LogLevel.Debug))
        {
            logger.Debug($"Reduced: {request.Location} raid time by: {chosenRaidReductionPercent}% to {newRaidTimeMinutes} minutes");
        }

        var exitAdjustments = GetExitAdjustments(mapBase, newRaidTimeMinutes);
        if (exitAdjustments.Count != 0)
        {
            result.ExitChanges.AddRange(exitAdjustments);
        }

        // Store state to use in loot generation
        profileActivityService.GetProfileActivityRaidData(sessionId).RaidAdjustments = result;

        return result;
    }

    /// <summary>
    ///     Get raid start time settings for specific map
    /// </summary>
    /// <param name="location">Map Location e.g. bigmap</param>
    /// <returns>ScavRaidTimeLocationSettings</returns>
    protected ScavRaidTimeLocationSettings GetMapSettings(string location)
    {
        var mapSettings = locationConfig.ScavRaidTimeSettings.Maps[location.ToLowerInvariant()];
        if (mapSettings is null)
        {
            logger.Warning($"Unable to find scav raid time settings for map: {location}, using defaults");
            return new ScavRaidTimeLocationSettings();
        }

        return mapSettings;
    }

    /// <summary>
    ///     Adjust exit times to handle scavs entering raids part-way through
    /// </summary>
    /// <param name="mapBase">Map base file player is on</param>
    /// <param name="newRaidTimeMinutes">How long raid is in minutes</param>
    /// <returns>List of exit changes to send to client</returns>
    protected List<ExtractChange> GetExitAdjustments(LocationBase mapBase, double newRaidTimeMinutes)
    {
        List<ExtractChange> result = [];
        // Adjust train exits only
        foreach (var exit in mapBase.Exits)
        {
            if (exit.PassageRequirement != RequirementState.Train)
            {
                continue;
            }

            // Prepare train adjustment object
            var exitChange = new ExtractChange
            {
                Name = exit.Name,
                MinTime = null,
                MaxTime = null,
                Chance = null,
            };

            // At what minute we simulate the player joining the raid
            var simulatedRaidEntryTimeMinutes = mapBase.EscapeTimeLimit - newRaidTimeMinutes;

            // How many seconds have elapsed in the raid when the player joins
            var reductionSeconds = simulatedRaidEntryTimeMinutes * 60;

            // Delay between the train extract activating and it becoming available to board
            //
            // Test method for determining this value:
            // 1) Set MinTime, MaxTime, and Count for the train extract all to 120
            // 2) Load into Reserve or Lighthouse as a PMC (both have the same result)
            // 3) Board the train when it arrives
            // 4) Check the raid time on the Raid Ended Screen (it should always be the same)
            //
            // trainArrivalDelaySeconds = [raid time on raid-ended screen] - MaxTime - Count - ExfiltrationTime
            // Example: Raid Time = 5:33 = 333 seconds
            //          trainArrivalDelaySeconds = 333 - 120 - 120 - 5 = 88
            //
            // I added 2 seconds just to be safe...
            //
            var trainArrivalDelaySeconds = locationConfig.ScavRaidTimeSettings.Settings.TrainArrivalDelayObservedSeconds;

            // Determine the earliest possible time in the raid when the train would leave
            var earliestPossibleDepartureMinutes = (exit.MinTime + exit.Count + exit.ExfiltrationTime + trainArrivalDelaySeconds) / 60;

            // If raid is after last moment train can leave, assume train has already left, disable extract
            var mostPossibleTimeRemainingAfterDeparture = mapBase.EscapeTimeLimit - earliestPossibleDepartureMinutes;
            if (newRaidTimeMinutes < mostPossibleTimeRemainingAfterDeparture)
            {
                exitChange.Chance = 0;

                if (logger.IsLogEnabled(LogLevel.Debug))
                {
                    logger.Debug(
                        $"Train Exit: {exit.Name} disabled as new raid time: {newRaidTimeMinutes} minutes is below: {mostPossibleTimeRemainingAfterDeparture} minutes"
                    );
                }

                result.Add(exitChange);

                continue;
            }

            // Reduce extract arrival times. Negative values seem to make extract turn red in game.
            exitChange.MinTime = Math.Max(exit.MinTime - reductionSeconds ?? 0, 0);
            exitChange.MaxTime = Math.Max(exit.MaxTime - reductionSeconds ?? 0, 0);

            if (logger.IsLogEnabled(LogLevel.Debug))
            {
                logger.Debug($"Train appears between: {exitChange.MinTime} and: {exitChange.MaxTime} seconds raid time");
            }

            result.Add(exitChange);
        }

        return result;
    }
}
