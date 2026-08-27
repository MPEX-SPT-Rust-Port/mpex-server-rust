using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Game;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Location;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Raid;
using SPTarkov.Server.Core.Services.InRaid;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the mod hook contract for raid setup: a Harmony patch on any member of the family's frozen
/// set must route <em>all five</em> call sites - both <see cref="RaidTimeAdjustmentService"/> ones,
/// both <see cref="LocationLifecycleService"/> ones and <see cref="PmcWaveGenerator"/>'s - to the
/// legacy path, because those are the only bodies the patch can hook. The set is deliberately
/// family-wide, so a patch on a lifecycle member forces the time-adjustment service back to C# as
/// well.
///
/// The three entry points are the exceptions: they are the dispatchers, and a patch there wraps
/// whichever path runs. So is <c>AdjustLootMultipliers</c>, which never left C#.
///
/// Harmony patches are process-wide, so every patch is removed in a finally and the fixture never
/// runs in parallel with others. The one settings entry it replaces - both call sites resolve the
/// same key, the map's own lowercased id - is put back in the one-time teardown, every pass works on
/// a clone, and what the raid time pass parks on the session is cleared as it returns. The one leak
/// is the same one the parity fixture has: <see cref="ProfileActivityService"/> has no removal API,
/// so the fixture's session stays in its cache - inert, with nothing parked on it.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RaidAdjustmentHookLivenessTests
{
    /// <summary>
    /// The map every case runs against. Its <c>base.json</c> Id is already lowercase, so the key the
    /// map pass resolves with and the key the raid time pass resolves with are the same entry.
    /// </summary>
    private const string RaidMap = "bigmap";

    /// <summary>
    /// Anything that is not a case-insensitive <c>"pmc"</c> takes the adjusting path, which is the
    /// one that reaches the frozen members.
    /// </summary>
    private const string ScavSide = "Savage";

    private static bool _patchFired;
    private static bool _prefixFired;
    private static bool _postfixFired;

    /// <summary>
    /// Both lifecycle passes are <c>protected</c>, and a subclass that exposed them would flip the
    /// path predicate to legacy - so they are called by reflection, on the real registered instance.
    /// </summary>
    private static readonly MethodInfo _adjustExtracts = Member(typeof(LocationLifecycleService), "AdjustExtracts");

    private static readonly MethodInfo _adjustBotHostilitySettings = Member(typeof(LocationLifecycleService), "AdjustBotHostilitySettings");

    private static readonly MethodInfo _isSide = Member(typeof(LocationLifecycleService), "IsSide");

    private readonly MongoId _sessionId = new();

    private RaidTimeAdjustmentService _raidTimeAdjustmentService = default!;
    private LocationLifecycleService _locationLifecycleService = default!;
    private PmcWaveGenerator _pmcWaveGenerator = default!;
    private LocationConfig _locationConfig = default!;
    private LocationTable _locationTable = default!;
    private ProfileActivityService _profileActivityService = default!;
    private ICloner _cloner = default!;
    private ScavRaidTimeLocationSettings? _originalSettings;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _raidTimeAdjustmentService = di.GetService<RaidTimeAdjustmentService>();
        _locationLifecycleService = di.GetService<LocationLifecycleService>();
        _pmcWaveGenerator = di.GetService<PmcWaveGenerator>();
        _locationConfig = di.GetService<LocationConfig>();
        _locationTable = di.GetService<LocationTable>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _cloner = di.GetService<ICloner>();

        // Every frozen member has to be reachable for the liveness half to mean anything: the two
        // wave passes are gated on the settings flag, and GetExitAdjustments only runs once the
        // chance roll passed - which at 100% it always does
        var maps = _locationConfig.ScavRaidTimeSettings.Maps;
        _originalSettings = maps[RaidMap];

        Assert.That(
            _originalSettings,
            Is.Not.Null,
            $"{RaidMap} lost its scav raid time settings, so no case here reaches the frozen members"
        );

        maps[RaidMap] = _originalSettings! with { AdjustWaves = true, ReducedChancePercent = 100 };
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        // Only if the setup got as far as capturing it - putting a null back would leave every other
        // fixture with a settings-less map
        if (_originalSettings is not null)
        {
            _locationConfig.ScavRaidTimeSettings.Maps[RaidMap] = _originalSettings;
        }
    }

    [TestCaseSource(nameof(FrozenMembers))]
    public void HarmonyPatchOnAFrozenMemberForcesTheLegacyPath(MethodInfo member)
    {
        var harmony = new Harmony($"unit-tests.raid-adjustment-hook-liveness.{member.DeclaringType!.Name}.{member.Name}");

        try
        {
            harmony.Patch(member, postfix: new HarmonyMethod(typeof(RaidAdjustmentHookLivenessTests), nameof(PatchFired)));

            Assert.That(
                Harmony.GetPatchInfo(member)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {member.Name} was not registered"
            );

            AssertAllFiveCallSites(LootGenerationPath.Legacy, $"a patch on {member.Name}");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The legacy bodies are what a patch is installed to hook, so every one of them has to actually
    /// run one of these call sites - this is what proves a patch on the frozen set is live rather
    /// than a silently failed install that only trips the dispatch check.
    /// </summary>
    [TestCaseSource(nameof(FrozenMembers))]
    public void HarmonyPatchOnAFrozenMemberFiresOnTheLegacyPath(MethodInfo member)
    {
        var harmony = new Harmony($"unit-tests.raid-adjustment-hook-liveness.fires.{member.DeclaringType!.Name}.{member.Name}");

        _patchFired = false;
        try
        {
            harmony.Patch(member, postfix: new HarmonyMethod(typeof(RaidAdjustmentHookLivenessTests), nameof(PatchFired)));

            AssertAllFiveCallSites(LootGenerationPath.Legacy, $"a patch on {member.Name}");

            Assert.That(_patchFired, Is.True, $"postfix on {member.Name} never ran on the legacy path");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The three entry points are deliberately not in the frozen set: a patch on any of them wraps
    /// whichever path runs, so the family keeps its native bodies and the patch still sees the call.
    /// </summary>
    [TestCaseSource(nameof(EntryPoints))]
    public void HarmonyPatchOnAnEntryPointWrapsTheNativeBodyWithoutForcingLegacy(MethodInfo entryPoint)
    {
        var harmony = new Harmony($"unit-tests.raid-adjustment-hook-liveness.dispatcher.{entryPoint.Name}");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                entryPoint,
                prefix: new HarmonyMethod(typeof(RaidAdjustmentHookLivenessTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(RaidAdjustmentHookLivenessTests), nameof(Postfix))
            );

            AssertAllFiveCallSites(LootGenerationPath.Native, $"a patch on {entryPoint.Name}");

            Assert.That(_prefixFired, Is.True, $"prefix on {entryPoint.Name} never ran");
            Assert.That(_postfixFired, Is.True, $"postfix on {entryPoint.Name} never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The carve-out: <c>AdjustLootMultipliers</c> runs in C# on both arms - the native pass never
    /// touches the live multiplier dictionaries - so a patch on it stays live without costing the
    /// family its native path.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAdjustLootMultipliersDoesNotForceLegacyAndFiresOnTheNativeArm()
    {
        var harmony = new Harmony("unit-tests.raid-adjustment-hook-liveness.AdjustLootMultipliers");
        var target = Member(typeof(RaidTimeAdjustmentService), "AdjustLootMultipliers");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(RaidAdjustmentHookLivenessTests), nameof(PatchFired)));

            // The multiplier calls are gated on a percent below 100, and they precede the dispatch
            AssertAllFiveCallSites(
                LootGenerationPath.Native,
                "a patch on AdjustLootMultipliers",
                MapChanges(dynamicLootPercent: 50, staticLootPercent: 50)
            );

            Assert.That(_patchFired, Is.True, "postfix on AdjustLootMultipliers never ran on the native arm");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The strongest shape of the dispatcher rule, on the entry point whose whole legacy body is
    /// inline in it: a prefix that returns false takes <c>ApplyWaveChangesToMap</c> over outright, so
    /// the mod's own wave policy runs in place of either arm - and the postfix beside it still
    /// observes the call. A mod that wants to replace this pass never needs the family to decline.
    /// </summary>
    [Test]
    public void AHarmonyPrefixOnApplyWaveChangesToMapCanTakeThePassOverEntirely()
    {
        var harmony = new Harmony("unit-tests.raid-adjustment-hook-liveness.ApplyWaveChangesToMap.skip");
        var target = Member(typeof(PmcWaveGenerator), nameof(PmcWaveGenerator.ApplyWaveChangesToMap));

        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(RaidAdjustmentHookLivenessTests), nameof(SkippingPrefix)),
                postfix: new HarmonyMethod(typeof(RaidAdjustmentHookLivenessTests), nameof(Postfix))
            );

            var map = Clone();
            Assert.That(map.BossLocationSpawn, Is.Not.Empty, $"{RaidMap} lost its boss spawns, so the skip is no longer observable");

            _pmcWaveGenerator.ApplyWaveChangesToMap(map);

            Assert.That(map.BossLocationSpawn, Is.Empty, "the prefix did not take the pass over");
            Assert.That(_postfixFired, Is.True, "the postfix never ran beside the skipping prefix");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The frozen set is a hand-written list, and this recomputes it independently to pin its exact
    /// contents: the time-adjustment half is swept off the type - its whole public and protected
    /// surface minus the two dispatchers and the carve-out - so a new member added there without
    /// being added to the set fails loudly rather than silently going unhookable. The lifecycle half
    /// is named, because that service's protected surface is the entire post-raid pipeline. The
    /// named tail is a checklist, not a claim: it must hold every <c>LocationLifecycleService</c>
    /// member whose body the native arm dispatches <em>or reimplements</em> - today
    /// <c>AdjustExtracts</c>, <c>AdjustBotHostilitySettings</c> and <c>IsSide</c> (the side test
    /// <c>raid_start.rs</c> carries as <c>is_side</c>). A future port that moves another lifecycle
    /// member natively must extend this tail alongside the builder's set.
    ///
    /// <see cref="PmcWaveGenerator"/> adds nothing to it and is the reason the sweep half stays
    /// typed to <see cref="RaidTimeAdjustmentService"/>: the generator's whole legacy body is inline
    /// in <c>ApplyWaveChangesToMap</c>, which is a dispatcher, so the fifth export bypasses no
    /// hookable member of its own. A port that splits that body into helpers has to add them here.
    /// </summary>
    [Test]
    public void TheHookableSetIsTheFrozenSurfaceMinusTheDispatchersAndTheCarveOut()
    {
        var members =
            (List<MethodBase>)
                typeof(RaidNativeRequestBuilder).GetField("_frozenMembers", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

        Assert.That(
            members,
            Is.EquivalentTo(FrozenSurface()),
            "the frozen set is not the raid surface minus the dispatchers and the carve-out"
        );
    }

    private static IEnumerable<TestCaseData> FrozenMembers()
    {
        return FrozenSurface().Select(member => new TestCaseData(member).SetArgDisplayNames($"{member.DeclaringType!.Name}.{member.Name}"));
    }

    private static IEnumerable<TestCaseData> EntryPoints()
    {
        return new[]
        {
            Member(typeof(RaidTimeAdjustmentService), nameof(RaidTimeAdjustmentService.MakeAdjustmentsToMap)),
            Member(typeof(RaidTimeAdjustmentService), nameof(RaidTimeAdjustmentService.GetRaidAdjustments)),
            Member(typeof(PmcWaveGenerator), nameof(PmcWaveGenerator.ApplyWaveChangesToMap)),
        }.Select(member => new TestCaseData(member).SetArgDisplayNames(member.Name));
    }

    /// <summary>
    /// The seven members a mod can patch to take over part of raid setup, recomputed independently
    /// of the builder's own list.
    /// </summary>
    private static List<MethodInfo> FrozenSurface()
    {
        return
        [
            .. typeof(RaidTimeAdjustmentService)
                .GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
                .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
                .Where(method =>
                    method.Name
                        is not (
                            nameof(RaidTimeAdjustmentService.MakeAdjustmentsToMap)
                            or nameof(RaidTimeAdjustmentService.GetRaidAdjustments)
                            or "AdjustLootMultipliers"
                        )
                ),
            _adjustExtracts,
            _adjustBotHostilitySettings,
            _isSide,
        ];
    }

    private static MethodInfo Member(Type declaringType, string name)
    {
        return declaringType.GetMethod(
                name,
                BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly
            ) ?? throw new InvalidOperationException($"{declaringType.Name}.{name} is not declared any more");
    }

    /// <summary>
    /// Every call site the family has, each asserted against the path it was expected to take.
    /// </summary>
    private void AssertAllFiveCallSites(LootGenerationPath expected, string what, RaidChanges? changes = null)
    {
        RunGetRaidAdjustments();
        Assert.That(_raidTimeAdjustmentService.LastPathTaken, Is.EqualTo(expected), $"{what}: GetRaidAdjustments took the wrong path");

        RunMakeAdjustmentsToMap(changes);
        Assert.That(_raidTimeAdjustmentService.LastPathTaken, Is.EqualTo(expected), $"{what}: MakeAdjustmentsToMap took the wrong path");

        RunAdjustExtracts();
        Assert.That(_locationLifecycleService.LastPathTaken, Is.EqualTo(expected), $"{what}: AdjustExtracts took the wrong path");

        RunAdjustBotHostilitySettings();
        Assert.That(
            _locationLifecycleService.LastPathTaken,
            Is.EqualTo(expected),
            $"{what}: AdjustBotHostilitySettings took the wrong path"
        );

        RunApplyWaveChangesToMap();
        Assert.That(_pmcWaveGenerator.LastPathTaken, Is.EqualTo(expected), $"{what}: ApplyWaveChangesToMap took the wrong path");
    }

    /// <summary>
    /// One scav raid time adjustment, whose applied path is what reaches <c>GetMapSettings</c> and
    /// <c>GetExitAdjustments</c>. What it parks on the session is cleared straight afterwards.
    /// </summary>
    private void RunGetRaidAdjustments()
    {
        try
        {
            _raidTimeAdjustmentService.GetRaidAdjustments(_sessionId, new GetRaidTimeRequest { Side = ScavSide, Location = RaidMap });
        }
        finally
        {
            _profileActivityService.GetProfileActivityRaidData(_sessionId).RaidAdjustments = null;
        }
    }

    /// <summary>
    /// One map adjustment against a clone, which is what the real pipeline hands the pass. The live
    /// multiplier dictionaries are not cloned - both arms scale them in C# - so they are put back.
    /// </summary>
    private void RunMakeAdjustmentsToMap(RaidChanges? changes = null)
    {
        var looseMultipliers = new Dictionary<string, double>(_locationConfig.LooseLootMultiplier);
        var staticMultipliers = new Dictionary<string, double>(_locationConfig.StaticLootMultiplier);

        try
        {
            _raidTimeAdjustmentService.MakeAdjustmentsToMap(changes ?? MapChanges(), Clone());
        }
        finally
        {
            foreach (var (key, value) in looseMultipliers)
            {
                _locationConfig.LooseLootMultiplier[key] = value;
            }

            foreach (var (key, value) in staticMultipliers)
            {
                _locationConfig.StaticLootMultiplier[key] = value;
            }
        }
    }

    private void RunAdjustExtracts()
    {
        _adjustExtracts.Invoke(_locationLifecycleService, [ScavSide, RaidMap, Clone()]);
    }

    private void RunAdjustBotHostilitySettings()
    {
        _adjustBotHostilitySettings.Invoke(_locationLifecycleService, [Clone()]);
    }

    /// <summary>
    /// One PMC wave pass against a clone, which is what the real pipeline hands it. The shipped
    /// config's own wave objects are spliced into the clone by reference and nothing writes to them,
    /// so this leaves no state behind.
    /// </summary>
    private void RunApplyWaveChangesToMap()
    {
        _pmcWaveGenerator.ApplyWaveChangesToMap(Clone());
    }

    private LocationBase Clone()
    {
        return _cloner.Clone(_locationTable.GetLocation(RaidMap)!.Base);
    }

    /// <summary>
    /// A parked <see cref="RaidChanges"/> as <c>GetRaidAdjustments</c> would have left it. The loot
    /// percents default to 100, which is what keeps <c>AdjustLootMultipliers</c> - the carve-out -
    /// out of every case but its own.
    /// </summary>
    private static RaidChanges MapChanges(double dynamicLootPercent = 100, double staticLootPercent = 100)
    {
        return new RaidChanges
        {
            DynamicLootPercent = dynamicLootPercent,
            StaticLootPercent = staticLootPercent,
            SimulatedRaidStartSeconds = 600,
            RaidTimeMinutes = 30,
            NewSurviveTimeSeconds = 100,
            OriginalSurvivalTimeSeconds = 1000,
            ExitChanges = [],
        };
    }

    private static void PatchFired()
    {
        _patchFired = true;
    }

    private static void Prefix()
    {
        _prefixFired = true;
    }

    private static void Postfix()
    {
        _postfixFired = true;
    }

    /// <summary>
    /// A mod replacing the PMC wave pass outright: its own write, and <c>false</c> so the original
    /// never runs.
    /// </summary>
    private static bool SkippingPrefix(LocationBase location)
    {
        location.BossLocationSpawn = [];
        return false;
    }
}
