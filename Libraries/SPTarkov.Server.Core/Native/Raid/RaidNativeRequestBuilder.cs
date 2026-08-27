using System.Reflection;
using HarmonyLib;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Game;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.InRaid;

namespace SPTarkov.Server.Core.Native.Raid;

/// <summary>
/// Assembles the raid-setup requests out of the live database and config - everything
/// <c>RaidTimeAdjustmentService</c> would have read for itself - and sends them.
///
/// It also owns the family's frozen member set: <see cref="AnyFrozenMemberPatched"/> is consulted by
/// the legacy-path predicates of <em>both</em> raid-setup services, so a Harmony patch on any one of
/// the six forces legacy at every one of their call sites.
/// </summary>
[Injectable]
public class RaidNativeRequestBuilder(GlobalTable globalTable, LocationConfig locationConfig)
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
