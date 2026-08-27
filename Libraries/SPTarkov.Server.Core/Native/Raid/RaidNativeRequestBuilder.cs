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
/// <c>RaidTimeAdjustmentService</c> and <c>LocationLifecycleService</c> would have read for
/// themselves - and sends them.
///
/// It also owns the family's frozen member set: <see cref="AnyFrozenMemberPatched"/> is consulted by
/// the legacy-path predicates of <em>both</em> raid-setup services, so a Harmony patch on any one of
/// the six forces legacy at every one of their call sites.
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
    ///     The six members a mod can Harmony-patch to take over part of raid setup - the four
    ///     <c>RaidTimeAdjustmentService</c> halves and the two <c>LocationLifecycleService</c> ones.
    ///     One shared set on purpose: the two services' native paths cover overlapping work, so a
    ///     patch anywhere in it has to route <em>all</em> of it back to C# for the hook to see
    ///     genuine baseline semantics.
    ///
    ///     Excluded are the two entry points, <c>MakeAdjustmentsToMap</c> and
    ///     <c>GetRaidAdjustments</c> - they are the dispatchers, and a patch there wraps whichever
    ///     path runs - and <c>AdjustLootMultipliers</c>, which stays on the C# side either way.
    /// </summary>
    private static readonly List<MethodBase> _frozenMembers =
    [
        FrozenMember(typeof(RaidTimeAdjustmentService), "GetMapSettings"),
        FrozenMember(typeof(RaidTimeAdjustmentService), "AdjustWaves"),
        FrozenMember(typeof(RaidTimeAdjustmentService), "AdjustPMCSpawns"),
        FrozenMember(typeof(RaidTimeAdjustmentService), "GetExitAdjustments"),
        FrozenMember(typeof(LocationLifecycleService), "AdjustExtracts"),
        FrozenMember(typeof(LocationLifecycleService), "AdjustBotHostilitySettings"),
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
        var exits = mapBase.Exits.ToList();

        // The same load-bearing lowercasing legacy GetMapSettings does, and TryGetValue so the
        // projection itself cannot throw ahead of the exit walk that legacy runs first
        var found = locationConfig.ScavRaidTimeSettings.Maps.TryGetValue(mapBase.Id.ToLowerInvariant(), out var mapSettings);

        var request = new MakeAdjustmentsRequest
        {
            MapId = mapBase.Id,
            RaidChanges = raidAdjustments,
            MapSettings = new MapSettingsAdjustState { Found = found, Value = mapSettings?.AdjustWaves },
            Exits = exits.Select(exit => exit.Name).ToList(),
            Waves = mapBase.Waves.Select(wave => new WaveTimesWire { TimeMin = wave.TimeMin, TimeMax = wave.TimeMax }).ToList(),
            BossSpawns = mapBase
                .BossLocationSpawn.Select(boss => new BossSpawnWire { BossName = boss.BossName, Time = boss.Time })
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
    ///     The map's train exits, in map order. The <c>PassageRequirement != Train</c> skip is
    ///     pre-applied here: it draws nothing and logs nothing, so the filtered list is the whole
    ///     walk the native side has to make.
    /// </summary>
    private static List<TrainExitWire> BuildTrainExits(LocationBase mapBase)
    {
        var trainExits = new List<TrainExitWire>();

        foreach (var exit in mapBase.Exits)
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
