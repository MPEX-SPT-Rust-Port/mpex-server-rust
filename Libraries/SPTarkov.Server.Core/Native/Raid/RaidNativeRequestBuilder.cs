using System.Reflection;
using HarmonyLib;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Game;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Location;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.InRaid;

namespace SPTarkov.Server.Core.Native.Raid;

/// <summary>
/// Assembles the raid-setup requests out of the live database and config - everything
/// <c>RaidTimeAdjustmentService</c>, <c>LocationLifecycleService</c> and
/// <c>PmcWaveGenerator</c> would have read for themselves - and sends them.
///
/// It also owns the family's frozen member set: <see cref="AnyFrozenMemberPatched"/> is consulted by
/// the legacy-path predicates of <em>all three</em> raid-setup classes, so a Harmony patch on any one
/// of the seven forces legacy at every one of their call sites.
/// </summary>
[Injectable]
public class RaidNativeRequestBuilder(
    GlobalTable globalTable,
    LocationTable locationTable,
    LocationConfig locationConfig,
    PmcConfig pmcConfig
)
{
    /// <summary>
    ///     The seven members a mod can Harmony-patch to take over part of raid setup - the four
    ///     <c>RaidTimeAdjustmentService</c> halves, the two <c>LocationLifecycleService</c> passes,
    ///     and <c>IsSide</c>, whose body the native extract pass reimplements: a patch on it must
    ///     route the side tests back to C#, or its other call sites would see the hook while the
    ///     moved pass silently would not. One shared set on purpose: the three classes' native paths
    ///     cover overlapping work, so a patch anywhere in it has to route <em>all</em> of it back to
    ///     C# for the hook to see genuine baseline semantics.
    ///
    ///     Excluded are the three entry points, <c>MakeAdjustmentsToMap</c>,
    ///     <c>GetRaidAdjustments</c> and <c>PmcWaveGenerator.ApplyWaveChangesToMap</c> - they are the
    ///     dispatchers, and a patch there wraps whichever path runs - and
    ///     <c>AdjustLootMultipliers</c>, which stays on the C# side either way. The wave generator
    ///     contributes nothing else: its whole legacy body is inline in that one entry point.
    /// </summary>
    private static readonly List<MethodBase> _frozenMembers =
    [
        FrozenMember(typeof(RaidTimeAdjustmentService), "GetMapSettings"),
        FrozenMember(typeof(RaidTimeAdjustmentService), "AdjustWaves"),
        FrozenMember(typeof(RaidTimeAdjustmentService), "AdjustPMCSpawns"),
        FrozenMember(typeof(RaidTimeAdjustmentService), "GetExitAdjustments"),
        FrozenMember(typeof(LocationLifecycleService), "AdjustExtracts"),
        FrozenMember(typeof(LocationLifecycleService), "AdjustBotHostilitySettings"),
        FrozenMember(typeof(LocationLifecycleService), "IsSide"),
    ];

    /// <summary>
    ///     Whether any member of the frozen set carries a live Harmony patch.
    /// </summary>
    internal static bool AnyFrozenMemberPatched()
    {
        return _frozenMembers.Any(member =>
            Harmony.GetPatchInfo(member) is { } patches
            && (patches.Prefixes.Count > 0 || patches.Postfixes.Count > 0 || patches.Transpilers.Count > 0 || patches.Finalizers.Count > 0)
        );
    }

    /// <summary>
    ///     One scav raid time adjustment's inputs: the request as it arrived, the map members the
    ///     pass reads, and the config's settings for that map.
    /// </summary>
    /// <param name="request">The client's raid time request</param>
    /// <param name="mapBase">The base of the location the request names</param>
    /// <param name="testSeed">Test-only seed for the native RNG stream</param>
    public GetRaidAdjustmentsRequest BuildGetRaidAdjustmentsRequest(GetRaidTimeRequest request, LocationBase mapBase, ulong? testSeed)
    {
        // The lowercasing is load-bearing: it is what legacy GetMapSettings does, and the shipped
        // base.json Ids are mixed-case
        var found = locationConfig.ScavRaidTimeSettings.Maps.TryGetValue(request.Location.ToLowerInvariant(), out var mapSettings);

        return new GetRaidAdjustmentsRequest
        {
            Side = request.Side,
            Location = request.Location,
            EscapeTimeLimit = mapBase.EscapeTimeLimit,
            SurvivedSecondsRequirement = globalTable.Configuration.Exp.MatchEnd.SurvivedSecondsRequirement,
            TrainArrivalDelayObservedSeconds = locationConfig.ScavRaidTimeSettings.Settings.TrainArrivalDelayObservedSeconds,
            MapSettings = new MapSettingsState { Found = found, Value = ToWire(mapSettings) },
            TrainExits = BuildTrainExits(mapBase),
            TestSeed = testSeed,
        };
    }

    /// <summary>
    ///     Rolls one scav raid's time adjustment natively.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    public GetRaidAdjustmentsResponse SendGetRaidAdjustments(GetRaidAdjustmentsRequest request)
    {
        return SptNative.GetRaidAdjustments(request);
    }

    /// <summary>
    ///     One map-adjustment pass's inputs, plus the exit list they were projected from.
    ///
    ///     <c>LocationBase.Exits</c> is an <c>IEnumerable</c>, so it is materialised exactly once
    ///     here and handed back: every <c>index</c> in the response indexes <em>this</em> list, and
    ///     an applier that re-enumerated the property could be indexing a different sequence.
    /// </summary>
    /// <param name="raidAdjustments">The changes the session parked</param>
    /// <param name="mapBase">The map to work out deltas for</param>
    public (MakeAdjustmentsRequest Request, List<Exit> Exits) BuildMakeAdjustmentsRequest(RaidChanges raidAdjustments, LocationBase mapBase)
    {
        // None of the three arrays is `required`, so a mod-added base.json that omits one
        // deserialises to null. Legacy touches each only conditionally - Exits behind a non-empty
        // ExitChanges, Waves and BossLocationSpawn behind AdjustWaves - and those conditions are
        // partly native-side, so the projection cannot wait for them the way the hostility builder
        // does. An empty projection lands every absent-array case on legacy's no-op side instead of
        // NREing here; the would-have-touched half is the booked divergence
        var exits = mapBase.Exits?.ToList() ?? [];

        // The same load-bearing lowercasing legacy GetMapSettings does, and TryGetValue so the
        // projection itself cannot throw ahead of the exit walk that legacy runs first
        var found = locationConfig.ScavRaidTimeSettings.Maps.TryGetValue(mapBase.Id.ToLowerInvariant(), out var mapSettings);

        var request = new MakeAdjustmentsRequest
        {
            MapId = mapBase.Id,
            RaidChanges = raidAdjustments,
            MapSettings = new MapSettingsAdjustState { Found = found, Value = mapSettings?.AdjustWaves },
            Exits = exits.Select(exit => exit.Name).ToList(),
            Waves = (mapBase.Waves ?? []).Select(wave => new WaveTimesWire { TimeMin = wave.TimeMin, TimeMax = wave.TimeMax }).ToList(),
            BossSpawns = (mapBase.BossLocationSpawn ?? [])
                .Select(boss => new BossSpawnWire { BossName = boss.BossName, Time = boss.Time })
                .ToList(),
        };

        return (request, exits);
    }

    /// <summary>
    ///     Works out one map's raid-setup deltas natively.
    /// </summary>
    /// <exception cref="InvalidOperationException">The pass failed, or the native side misbehaved.</exception>
    public MakeAdjustmentsResponse SendMakeAdjustments(MakeAdjustmentsRequest request)
    {
        return SptNative.MakeAdjustmentsToMap(request);
    }

    /// <summary>
    ///     One raid-start hostility pass's inputs, plus the location settings list they were
    ///     projected from.
    ///
    ///     <c>AdditionalHostilitySettings</c> is an <c>IEnumerable</c>, so it is materialised
    ///     exactly once here and handed back: every <c>matchedIndex</c> in the response indexes
    ///     <em>this</em> list. A null one stays null - that is the map with no hostility settings at
    ///     all, where legacy's <c>?.FirstOrDefault</c> leaves every role unmatched.
    /// </summary>
    /// <param name="location">The map to work out hostility deltas for</param>
    public (AdjustHostilityRequest Request, List<AdditionalHostilitySettings>? LocationSettings) BuildAdjustHostilityRequest(
        LocationBase location
    )
    {
        // Legacy derefs BotLocationModifier inside its per-role loop, so a config with no roles at
        // all never touches it - and the member is not `required`, so a mod-added base.json that
        // omitted it deserialises to null. Materialising unconditionally would NRE where legacy
        // no-ops, so the deref waits for a role to need it
        var hostilityList =
            pmcConfig.HostilitySettings.Count > 0 ? location.BotLocationModifier.AdditionalHostilitySettings?.ToList() : null;

        var request = new AdjustHostilityRequest
        {
            // ToDictionary copies in source enumeration order, which is the order the legacy
            // foreach walks - and the order the native side reads back into its IndexMap
            HostilitySettings = pmcConfig.HostilitySettings.ToDictionary(
                botId => botId.Key,
                botId => new HostilityConfigWire
                {
                    AdditionalEnemyTypes = botId.Value.AdditionalEnemyTypes,

                    // A pure null check, exactly as legacy tests it: a non-null empty list is still
                    // the branch that clears the location's list
                    HasChancedEnemies = botId.Value.ChancedEnemies is not null,
                    AdditionalFriendlyTypes = botId.Value.AdditionalFriendlyTypes,
                    BearEnemyChance = botId.Value.BearEnemyChance,
                    UsecEnemyChance = botId.Value.UsecEnemyChance,
                    SavageEnemyChance = botId.Value.SavageEnemyChance,
                    SavagePlayerBehaviour = botId.Value.SavagePlayerBehaviour,
                }
            ),
            LocationSettings = hostilityList
                ?.Select(botSettings => new LocationHostilityWire
                {
                    BotRole = botSettings.BotRole,
                    AlwaysEnemiesIsNull = botSettings.AlwaysEnemies is null,
                })
                .ToList(),
        };

        return (request, hostilityList);
    }

    /// <summary>
    ///     Works out one map's bot-hostility deltas natively.
    /// </summary>
    /// <exception cref="InvalidOperationException">The pass failed, or the native side misbehaved.</exception>
    public AdjustHostilityResponse SendAdjustBotHostilitySettings(AdjustHostilityRequest request)
    {
        return SptNative.AdjustBotHostilitySettings(request);
    }

    /// <summary>
    ///     One extract pass's inputs, plus the extract list they were projected from.
    ///
    ///     <c>AllExtracts</c> is an <c>IEnumerable</c> on a location that may not exist at all, so
    ///     the lookup happens once here and the materialised list is handed back: every appended
    ///     index indexes <em>this</em> list, never a re-fetched one. Null covers both halves of
    ///     legacy's single null check - an unknown map and a map with no extract data.
    /// </summary>
    /// <param name="playerSide">The side the player is entering the raid on</param>
    /// <param name="location">The map's name, as the client spelled it</param>
    public (AdjustExtractsRequest Request, List<AllExtractsExit>? Extracts) BuildAdjustExtractsRequest(string playerSide, string location)
    {
        var mapExtracts = locationTable.GetLocation(location)?.AllExtracts?.ToList();

        var request = new AdjustExtractsRequest
        {
            PlayerSide = playerSide,
            MapFound = mapExtracts is not null,
            ExtractSides = mapExtracts?.Select(extract => extract.Side).ToList() ?? [],
        };

        return (request, mapExtracts);
    }

    /// <summary>
    ///     Works out which extracts a scav player's exit list gains natively.
    /// </summary>
    /// <exception cref="InvalidOperationException">The pass failed, or the native side misbehaved.</exception>
    public AdjustExtractsResponse SendAdjustExtracts(AdjustExtractsRequest request)
    {
        return SptNative.AdjustExtracts(request);
    }

    /// <summary>
    ///     One PMC wave pass's inputs, plus the config's wave list they were resolved from.
    ///
    ///     The list is looked up exactly once here and handed back: the applier appends
    ///     <em>these</em> instances into the location by reference, which is the aliasing channel the
    ///     splice has always had - a second lookup, or a serde round trip, would hand it copies.
    /// </summary>
    /// <param name="location">The map to work out the wave splice for</param>
    public (ApplyPmcWavesRequest Request, List<BossLocationSpawn>? WavesToAdd) BuildApplyPmcWavesRequest(LocationBase location)
    {
        // Legacy gates the lookup - and every dereference - behind the flag
        // (PmcWaveGenerator.cs:54-56): with it off, a null Id or a null BossLocationSpawn must stay
        // untouched on this arm too. The lowercasing is the same load-bearing one the legacy body
        // does; a null Id NREs identically on both arms when the flag is set
        List<BossLocationSpawn>? wavesToAdd = null;
        var wavesFound =
            pmcConfig.RemoveExistingPmcWaves && pmcConfig.CustomPmcWaves.TryGetValue(location.Id.ToLowerInvariant(), out wavesToAdd);

        var request = new ApplyPmcWavesRequest
        {
            RemoveExistingPmcWaves = pmcConfig.RemoveExistingPmcWaves,
            WavesFound = wavesFound,

            // A null list value no-ops natively where legacy NREs on the unguarded .Count - booked,
            // mod-only
            WaveCount = wavesToAdd?.Count ?? 0,

            // Legacy first touches BossLocationSpawn only once all three gates pass
            BossNames =
                wavesFound && wavesToAdd is { Count: > 0 }
                    ? (location.BossLocationSpawn ?? []).Select(bossSpawn => bossSpawn.BossName).ToList()
                    : [],
        };

        return (request, wavesToAdd);
    }

    /// <summary>
    ///     Works out which of a map's boss waves the custom-PMC splice drops natively.
    /// </summary>
    /// <exception cref="InvalidOperationException">The pass failed, or the native side misbehaved.</exception>
    public ApplyPmcWavesResponse SendApplyPmcWaves(ApplyPmcWavesRequest request)
    {
        return SptNative.ApplyPmcWaveChanges(request);
    }

    /// <summary>
    ///     The map's train exits, in map order. The <c>PassageRequirement != Train</c> skip is
    ///     pre-applied here: it draws nothing and logs nothing, so the filtered list is the whole
    ///     walk the native side has to make.
    /// </summary>
    private static List<TrainExitWire> BuildTrainExits(LocationBase mapBase)
    {
        var trainExits = new List<TrainExitWire>();

        // Null-tolerant for the same reason as BuildMakeAdjustmentsRequest: legacy walks Exits only
        // once the side and chance gates passed, which the projection cannot know yet
        foreach (var exit in mapBase.Exits ?? [])
        {
            if (exit.PassageRequirement != RequirementState.Train)
            {
                continue;
            }

            trainExits.Add(
                new TrainExitWire
                {
                    Name = exit.Name,
                    MinTime = exit.MinTime,
                    MaxTime = exit.MaxTime,
                    Count = exit.Count,
                    ExfiltrationTime = exit.ExfiltrationTime,
                }
            );
        }

        return trainExits;
    }

    /// <summary>
    ///     A null settings value crosses as a null: it is the warn-and-defaults branch, which the
    ///     native side reproduces from the C# record's own auto-property defaults.
    /// </summary>
    private static ScavRaidTimeLocationSettingsWire? ToWire(ScavRaidTimeLocationSettings? mapSettings)
    {
        if (mapSettings is null)
        {
            return null;
        }

        return new ScavRaidTimeLocationSettingsWire
        {
            ReducedChancePercent = mapSettings.ReducedChancePercent,
            ReductionPercentWeights = mapSettings.ReductionPercentWeights,
            ReduceLootByPercent = mapSettings.ReduceLootByPercent,
            MinDynamicLootPercent = mapSettings.MinDynamicLootPercent,
            MinStaticLootPercent = mapSettings.MinStaticLootPercent,
        };
    }

    /// <summary>
    ///     One frozen member, by name. A rename that the predicate did not follow would leave it
    ///     blind to patches on that member, so a miss fails loudly the first time the set is built.
    /// </summary>
    private static MethodBase FrozenMember(Type declaringType, string name)
    {
        return declaringType.GetMethod(name, BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.DeclaredOnly)
            ?? throw new InvalidOperationException(
                $"{declaringType.Name}.{name} is not declared any more, so the raid-setup legacy-path predicate cannot see patches on it."
            );
    }
}
