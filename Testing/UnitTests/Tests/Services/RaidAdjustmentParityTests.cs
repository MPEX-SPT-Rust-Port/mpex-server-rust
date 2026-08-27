using System.Reflection;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Game;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Location;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.InRaid;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Services;

/// <summary>
/// Golden parity gate on the raid-setup port: the same seed must make the legacy C# path and the
/// spt-native path produce the same <see cref="RaidChanges"/> and park the same thing on the
/// session, and the map-adjustment and raid-start passes must leave the same <c>LocationBase</c>
/// behind whichever arm applied it.
///
/// State this fixture mutates, all of it restored in <see cref="Adjust"/>'s,
/// <see cref="AdjustMap"/>'s, <see cref="AdjustHostility"/>'s and <see cref="AdjustRaidExtracts"/>'s
/// <c>finally</c> - the last two also replace <c>PmcConfig.HostilitySettings</c> wholesale, which is
/// a single reference assignment to put back:
/// <see cref="LocationConfig.ForceLegacyRaidAdjustments"/> on the shared config singleton - the path
/// selector - the one <c>ScavRaidTimeSettings.Maps</c> entry a case forces,
/// <see cref="RandomUtil.RandomSource"/>, which is the seam the legacy path draws through,
/// <c>NativeTestSeed</c>, the session's parked adjustments, and - because the multiplier half runs
/// against the live config on both arms - the two loot multiplier dictionaries. Map entries are
/// always <em>replaced</em> rather than edited in place, so restoring the original reference
/// restores everything. Nothing here reads resident database state, so no mutation stamp bumps are
/// needed.
///
/// The map cases always work on a <see cref="ICloner"/> clone, which is what the real pipeline hands
/// the pass, so the resident location table is never written to.
///
/// The one leak: <see cref="ProfileActivityService"/> has no removal API, so the fixture's session
/// stays in its cache. It is left inert - its parked adjustments are nulled in the same
/// <c>finally</c> - which is what a freshly seen profile looks like anyway.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RaidAdjustmentParityTests
{
    private static readonly ulong[] _seeds = [42, 1337];

    /// <summary>
    /// Scav side. Anything that is not a case-insensitive <c>"pmc"</c> takes the adjusting path.
    /// </summary>
    private const string ScavSide = "Savage";

    /// <summary>
    /// The map every raid-start case works a clone of: four <c>AdditionalHostilitySettings</c>
    /// entries covering both shipped config roles, and 17 scav extracts among 26.
    /// </summary>
    private const string RaidStartMap = "bigmap";

    /// <summary>
    /// Both raid-start passes are <c>protected</c>, and a subclass that exposed them would flip the
    /// path predicate to legacy - so they are called by reflection, on the real registered instance.
    /// </summary>
    private static readonly MethodInfo _adjustExtracts = typeof(LocationLifecycleService).GetMethod(
        "AdjustExtracts",
        BindingFlags.Instance | BindingFlags.NonPublic
    )!;

    private static readonly MethodInfo _adjustBotHostilitySettings = typeof(LocationLifecycleService).GetMethod(
        "AdjustBotHostilitySettings",
        BindingFlags.Instance | BindingFlags.NonPublic
    )!;

    private readonly MongoId _sessionId = new();

    private RaidTimeAdjustmentService _raidTimeAdjustmentService = default!;
    private LocationLifecycleService _locationLifecycleService = default!;
    private LocationConfig _locationConfig = default!;
    private LocationTable _locationTable = default!;
    private RandomUtil _randomUtil = default!;
    private ProfileActivityService _profileActivityService = default!;
    private JsonUtil _jsonUtil = default!;
    private ICloner _cloner = default!;
    private PmcConfig _pmcConfig = default!;
    private PmcWaveGenerator _pmcWaveGenerator = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _raidTimeAdjustmentService = di.GetService<RaidTimeAdjustmentService>();
        _locationLifecycleService = di.GetService<LocationLifecycleService>();
        _locationConfig = di.GetService<LocationConfig>();
        _locationTable = di.GetService<LocationTable>();
        _randomUtil = di.GetService<RandomUtil>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _jsonUtil = di.GetService<JsonUtil>();
        _cloner = di.GetService<ICloner>();
        _pmcConfig = di.GetService<PmcConfig>();
        _pmcWaveGenerator = di.GetService<PmcWaveGenerator>();
    }

    /// <summary>
    /// The ordinary applied path against shipped map data. <c>lighthouse</c> is here because it is
    /// one of the two shipped maps with a train exit, so it is the only arm that compares an actual
    /// exit change; <c>bigmap</c> has none and covers the empty-exit case beside it.
    /// </summary>
    [Test]
    public void AScavRequestMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed, [Values("bigmap", "lighthouse")] string location)
    {
        var legacy = Adjust(ScavSide, location, seed, forceLegacy: true, WithSettings(reducedChancePercent: 100));
        var native = Adjust(ScavSide, location, seed, forceLegacy: false, WithSettings(reducedChancePercent: 100));

        AssertParity(legacy, native, location, seed);

        // A case that never applied would be comparing two untouched defaults
        Assert.That(native.Parked, Is.Not.Null, $"seed={seed} location={location} never took the applied path");

        if (location == "lighthouse")
        {
            Assert.That(
                native.Changes.ExitChanges,
                Is.Not.Empty,
                "lighthouse lost its train exit, so no case here compares an exit change any more"
            );
        }
    }

    /// <summary>
    /// The pmc early return, which precedes both the map-settings resolve and every draw.
    /// </summary>
    [Test]
    public void APmcRequestReturnsTheDefaultOnBothPathsAndParksNothing([ValueSource(nameof(_seeds))] ulong seed)
    {
        var legacy = Adjust("pmc", "bigmap", seed, forceLegacy: true);
        var native = Adjust("pmc", "bigmap", seed, forceLegacy: false);

        AssertParity(legacy, native, "pmc", seed);

        Assert.That(native.Changes.RaidTimeMinutes, Is.EqualTo(_locationTable.GetLocation("bigmap")!.Base.EscapeTimeLimit));
        Assert.That(native.Parked, Is.Null);
        Assert.That(legacy.Parked, Is.Null);
    }

    /// <summary>
    /// The draw is spent even at 0%, and the failed roll returns the untouched default without the
    /// session write.
    /// </summary>
    [Test]
    public void AFailedChanceRollParksNothingOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var legacy = Adjust(ScavSide, "bigmap", seed, forceLegacy: true, WithSettings(reducedChancePercent: 0));
        var native = Adjust(ScavSide, "bigmap", seed, forceLegacy: false, WithSettings(reducedChancePercent: 0));

        AssertParity(legacy, native, "failed-chance", seed);

        Assert.That(native.Parked, Is.Null);
        Assert.That(legacy.Parked, Is.Null);
    }

    /// <summary>
    /// One weight entry short-circuits the weighted pick before any draw, so this pins the
    /// zero-draw arm - and with it that the two paths spend the same number of draws up to there.
    /// </summary>
    [Test]
    public void ASingleEntryWeightMapMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var weights = new Dictionary<string, double> { ["40"] = 1 };

        var legacy = Adjust(ScavSide, "bigmap", seed, forceLegacy: true, WithSettings(100, weights));
        var native = Adjust(ScavSide, "bigmap", seed, forceLegacy: false, WithSettings(100, weights));

        AssertParity(legacy, native, "single-entry-weights", seed);

        Assert.That(native.Parked, Is.Not.Null);
    }

    /// <summary>
    /// Weights summing to 6 over 2 entries miss the all-equal early exit, so the pick takes the
    /// <c>GetDouble(0, 1)</c> arm - one draw, and a different one from the integer arm.
    /// </summary>
    [Test]
    public void AMultiEntryWeightMapMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var weights = new Dictionary<string, double> { ["20"] = 5, ["40"] = 1 };

        var legacy = Adjust(ScavSide, "bigmap", seed, forceLegacy: true, WithSettings(100, weights));
        var native = Adjust(ScavSide, "bigmap", seed, forceLegacy: false, WithSettings(100, weights));

        AssertParity(legacy, native, "multi-entry-weights", seed);

        Assert.That(native.Parked, Is.Not.Null);
    }

    /// <summary>
    /// A present key with a null value is the warn-and-defaults branch, whose 0% chance makes the
    /// roll a silent no-op. A missing key is a different thing entirely - it throws on both paths.
    /// </summary>
    [Test]
    public void TheSettingsNullDefaultPathMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var legacy = Adjust(ScavSide, "bigmap", seed, forceLegacy: true, _ => null);
        var native = Adjust(ScavSide, "bigmap", seed, forceLegacy: false, _ => null);

        AssertParity(legacy, native, "null-settings", seed);

        Assert.That(native.Parked, Is.Null);
        Assert.That(legacy.Parked, Is.Null);
    }

    /// <summary>
    /// Pins the load-bearing lowercased settings lookup: shipped <c>base.json</c> Ids are mixed-case,
    /// and a lookup that skipped the lowercasing would miss the key - which throws, on both paths.
    /// </summary>
    [Test]
    public void AMixedCaseMapIdResolvesSettingsOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var legacy = Adjust(ScavSide, "Woods", seed, forceLegacy: true, WithSettings(reducedChancePercent: 100));
        var native = Adjust(ScavSide, "Woods", seed, forceLegacy: false, WithSettings(reducedChancePercent: 100));

        AssertParity(legacy, native, "Woods", seed);

        Assert.That(native.Parked, Is.Not.Null, "the mixed-case id did not resolve to its settings");
    }

    /// <summary>
    /// <c>labyrinth</c> has no <c>scavRaidTimeSettings.maps</c> entry at all, so it can only be
    /// asked for as a pmc - the early return precedes the resolve that would otherwise throw.
    /// </summary>
    [Test]
    public void APmcRequestOnASettingsLessMapReturnsTheDefaultOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        Assert.That(
            _locationConfig.ScavRaidTimeSettings.Maps.ContainsKey("labyrinth"),
            Is.False,
            "labyrinth gained a scav raid time settings entry, so this case no longer covers the settings-less map"
        );

        var legacy = Adjust("pmc", "labyrinth", seed, forceLegacy: true);
        var native = Adjust("pmc", "labyrinth", seed, forceLegacy: false);

        AssertParity(legacy, native, "labyrinth", seed);

        Assert.That(native.Parked, Is.Null);
        Assert.That(legacy.Parked, Is.Null);
    }

    /// <summary>
    /// The legacy path parks the very object it returns, and loot generation later reads that
    /// object. A native path that decoded one instance and parked a copy would compare equal above
    /// and still be wrong.
    /// </summary>
    [Test]
    public void TheSessionObjectIsTheReturnedInstance()
    {
        var native = Adjust(ScavSide, "bigmap", _seeds[0], forceLegacy: false, WithSettings(reducedChancePercent: 100));

        Assert.That(native.Parked, Is.SameAs(native.Changes));
    }

    /// <summary>
    /// The ordinary map-adjustment pass against shipped <c>bigmap</c> data, with the wave half on:
    /// real exits, real waves and real boss spawns, compared as the whole <c>LocationBase</c>.
    ///
    /// One wave and one PMC spawn are added that actually survive, because no shipped map has any
    /// that do - every shipped wave carries a null <c>TimeMax</c> and every shipped PMC spawn a
    /// <c>Time</c> of -1, so real data on its own only ever exercises the drop half.
    /// </summary>
    [Test]
    public void MapAdjustmentsMatchOnBothPaths()
    {
        var changes = MapChanges(simulatedRaidStartSeconds: 600);

        var legacy = AdjustMap("bigmap", changes, forceLegacy: true, WithASurvivorOfEach, WithAdjustWaves(true));
        var native = AdjustMap("bigmap", changes, forceLegacy: false, WithASurvivorOfEach, WithAdjustWaves(true));

        AssertMapParity(legacy, native, "bigmap");

        // A case that adjusted nothing would be comparing two untouched clones
        var original = _locationTable.GetLocation("bigmap")!.Base;
        Assert.That(native.Map.EscapeTimeLimit, Is.EqualTo(30));

        // Only the added wave survives - the shipped ones all have a null TimeMax - and it loses the
        // 600 start seconds twice
        Assert.That(native.Map.Waves, Has.Count.EqualTo(1), "the shipped waves were expected to drop and the added one to survive");
        Assert.That(native.Map.Waves[0].TimeMin, Is.EqualTo(800));
        Assert.That(native.Map.Waves[0].TimeMax, Is.EqualTo(3800));

        // The shipped PMC spawns all sit at -1 and drop; the added one survives and is offset to the
        // floor of 1
        Assert.That(native.Map.BossLocationSpawn, Has.Count.LessThan(original.BossLocationSpawn.Count), "no shipped PMC spawn was dropped");
        Assert.That(native.Map.BossLocationSpawn[^1].BossName, Is.EqualTo("pmcUSEC"));
        Assert.That(native.Map.BossLocationSpawn[^1].Time, Is.EqualTo(1));
    }

    /// <summary>
    /// One exit change naming an exit the map does not have returns out of the whole method, so the
    /// changes before it land, the wave half never runs, and only a debug line marks it.
    /// </summary>
    [Test]
    public void AnUnmatchedExitChangeAbortsIdenticallyOnBothPaths()
    {
        var firstExit = _locationTable.GetLocation("bigmap")!.Base.Exits.First().Name;
        var changes = MapChanges(
            simulatedRaidStartSeconds: 600,
            exitChanges:
            [
                new ExtractChange
                {
                    Name = firstExit,
                    MinTime = 111,
                    MaxTime = 222,
                    Chance = 33,
                },
                new ExtractChange { Name = "no exit is called this" },
            ]
        );

        var legacy = AdjustMap("bigmap", changes, forceLegacy: true, replaceSettings: WithAdjustWaves(true));
        var native = AdjustMap("bigmap", changes, forceLegacy: false, replaceSettings: WithAdjustWaves(true));

        AssertMapParity(legacy, native, "aborted-exit");

        var adjustedExit = native.Map.Exits.First();
        Assert.That(adjustedExit.MinTime, Is.EqualTo(111), "the change before the unmatched one was lost");
        Assert.That(adjustedExit.MaxTime, Is.EqualTo(222));
        Assert.That(adjustedExit.Chance, Is.EqualTo(33));
        Assert.That(native.Map.EscapeTimeLimit, Is.EqualTo(30), "the escape time limit precedes the exit walk");
        Assert.That(
            native.Map.Waves,
            Has.Count.EqualTo(_locationTable.GetLocation("bigmap")!.Base.Waves.Count),
            "the abort did not stop the wave half"
        );
    }

    /// <summary>
    /// The abort precedes the map-settings resolve, so a map with no settings entry at all - which
    /// throws on both arms otherwise - returns silently once an exit change fails to match.
    /// </summary>
    [Test]
    public void AnAbortedRunSkipsTheMissingSettingsThrowOnBothPaths()
    {
        Assert.That(
            _locationConfig.ScavRaidTimeSettings.Maps.ContainsKey("labyrinth"),
            Is.False,
            "labyrinth gained a scav raid time settings entry, so this case no longer covers the settings-less map"
        );

        var changes = MapChanges(simulatedRaidStartSeconds: 600, exitChanges: [new ExtractChange { Name = "no exit is called this" }]);

        var legacy = AdjustMap("labyrinth", changes, forceLegacy: true);
        var native = AdjustMap("labyrinth", changes, forceLegacy: false);

        AssertMapParity(legacy, native, "aborted-settings-less");

        Assert.That(native.Map.EscapeTimeLimit, Is.EqualTo(30));
    }

    /// <summary>
    /// Quirk 2: the wave time loop runs twice - once in <c>AdjustWaves</c>, once again in
    /// <c>AdjustPMCSpawns</c> over the list the first pass already reduced - so every surviving wave
    /// loses the start seconds twice.
    /// </summary>
    [Test]
    public void SurvivingWavesLoseTwiceTheStartSecondsOnBothPaths()
    {
        var changes = MapChanges(simulatedRaidStartSeconds: 100);

        var legacy = AdjustMap("bigmap", changes, forceLegacy: true, WithWaves(NewWave(300, 500)), WithAdjustWaves(true));
        var native = AdjustMap("bigmap", changes, forceLegacy: false, WithWaves(NewWave(300, 500)), WithAdjustWaves(true));

        AssertMapParity(legacy, native, "double-subtraction");

        Assert.That(native.Map.Waves, Has.Count.EqualTo(1));
        Assert.That(native.Map.Waves[0].TimeMin, Is.EqualTo(100), "the start seconds came off once, not twice");
        Assert.That(native.Map.Waves[0].TimeMax, Is.EqualTo(300));
    }

    /// <summary>
    /// Quirk 3: the keep test is a lifted <c>&gt;</c>, which is false whenever either side is null -
    /// so a wave with no <c>TimeMax</c> drops itself.
    /// </summary>
    [Test]
    public void ANullTimeMaxWaveIsDroppedOnBothPaths()
    {
        var changes = MapChanges(simulatedRaidStartSeconds: 100);
        var waves = WithWaves(NewWave(10, null), NewWave(300, 500), NewWave(5, 50));

        var legacy = AdjustMap("bigmap", changes, forceLegacy: true, waves, WithAdjustWaves(true));
        var native = AdjustMap("bigmap", changes, forceLegacy: false, waves, WithAdjustWaves(true));

        AssertMapParity(legacy, native, "null-timemax");

        Assert.That(native.Map.Waves, Has.Count.EqualTo(1), "the null TimeMax wave survived");
        Assert.That(native.Map.Waves[0].TimeMax, Is.EqualTo(300));
    }

    /// <summary>
    /// Quirk 4, all of it: the keep test is <c>OrdinalIgnoreCase</c> and treats a null name as "not
    /// a pmc", the offset filter beside it is a case-sensitive constant pattern, and the offset
    /// itself floors at 1 rather than at 0.
    /// </summary>
    [Test]
    public void BossKeepAndOffsetClampToOneOnBothPaths()
    {
        var changes = MapChanges(simulatedRaidStartSeconds: 100);
        var spawns = WithBossSpawns(
            NewSpawn("pmcUSEC", 200),
            NewSpawn("PMCUSEC", 300),
            NewSpawn("pmcBEAR", 50),
            NewSpawn(null, 10),
            NewSpawn("bossKilla", 20)
        );

        var legacy = AdjustMap("bigmap", changes, forceLegacy: true, spawns, WithAdjustWaves(true));
        var native = AdjustMap("bigmap", changes, forceLegacy: false, spawns, WithAdjustWaves(true));

        AssertMapParity(legacy, native, "boss-keep-and-offset");

        Assert.That(native.Map.BossLocationSpawn, Has.Count.EqualTo(4), "only the early pmcBEAR should have been dropped");
        Assert.That(native.Map.BossLocationSpawn[0].Time, Is.EqualTo(1), "the offset did not floor at 1");
        Assert.That(native.Map.BossLocationSpawn[1].Time, Is.EqualTo(300), "the case-sensitive offset filter matched PMCUSEC");
        Assert.That(native.Map.BossLocationSpawn[2].BossName, Is.Null, "the null-named spawn was dropped");
    }

    /// <summary>
    /// Aliasing channel 1: the two multiplier calls run in C# on both arms, against the live
    /// <see cref="LocationConfig"/> dictionaries rather than against the clone.
    /// </summary>
    [Test]
    public void TheLiveConfigMultipliersAreScaledIdenticallyOnBothArms()
    {
        var changes = MapChanges(simulatedRaidStartSeconds: 600, dynamicLootPercent: 50, staticLootPercent: 25);

        var legacy = AdjustMap("bigmap", changes, forceLegacy: true, replaceSettings: WithAdjustWaves(true));
        var native = AdjustMap("bigmap", changes, forceLegacy: false, replaceSettings: WithAdjustWaves(true));

        Assert.That(native.LooseLootMultiplier, Is.EqualTo(legacy.LooseLootMultiplier), "the loose loot multipliers differ");
        Assert.That(native.StaticLootMultiplier, Is.EqualTo(legacy.StaticLootMultiplier), "the static loot multipliers differ");

        // Both that the scaling happened at all and that the fixture put it back
        Assert.That(native.LooseLootMultiplier, Is.Not.EqualTo(_locationConfig.LooseLootMultiplier));
        Assert.That(native.StaticLootMultiplier, Is.Not.EqualTo(_locationConfig.StaticLootMultiplier));
    }

    /// <summary>
    /// Aliasing channel 2: the PMC wave splice puts the live <c>PmcConfig.CustomPmcWaves</c>
    /// instances into the clone, so the spawn-time offset is a permanent config mutation that
    /// compounds across raids. An upstream bug, preserved deliberately - and structural, so the
    /// native arm has to write the very same objects.
    /// </summary>
    [Test]
    public void APmcSpawnTimeWriteLandsOnTheLiveConfigObject()
    {
        var wavesKey = _locationTable.GetLocation("bigmap")!.Base.Id.ToLowerInvariant();
        var originalRemove = _pmcConfig.RemoveExistingPmcWaves;
        var wavesPresent = _pmcConfig.CustomPmcWaves.TryGetValue(wavesKey, out var originalWaves);

        try
        {
            var early = NewSpawn("pmcUSEC", 500);
            var late = NewSpawn("pmcUSEC", 900);

            // The channel is gated on both of these - the generator no-ops without them
            _pmcConfig.RemoveExistingPmcWaves = true;
            _pmcConfig.CustomPmcWaves[wavesKey] = [early, late];

            var changes = MapChanges(simulatedRaidStartSeconds: 100);

            AdjustMap("bigmap", changes, forceLegacy: false, _pmcWaveGenerator.ApplyWaveChangesToMap, WithAdjustWaves(true));

            Assert.That(early.Time, Is.EqualTo(1), "the offset did not land on the config's own instance");
            Assert.That(late.Time, Is.EqualTo(400));

            // The next raid splices the same instances into a fresh clone and offsets them again
            AdjustMap("bigmap", changes, forceLegacy: false, _pmcWaveGenerator.ApplyWaveChangesToMap, WithAdjustWaves(true));

            Assert.That(late.Time, Is.EqualTo(1), "the offset did not compound across raids");
            Assert.That(early.Time, Is.EqualTo(1));
        }
        finally
        {
            _pmcConfig.RemoveExistingPmcWaves = originalRemove;

            if (wavesPresent)
            {
                _pmcConfig.CustomPmcWaves[wavesKey] = originalWaves!;
            }
            else
            {
                _pmcConfig.CustomPmcWaves.Remove(wavesKey);
            }
        }
    }

    /// <summary>
    /// The whole PMC wave pass against a prepared spawn list: every arm of the removal filter - a
    /// non-PMC name, both PMC names and a null one - and the append behind it, compared as the whole
    /// <c>LocationBase</c>.
    /// </summary>
    [Test]
    public void PmcWaveChangesMatchLegacyAndPreserveConfigAliasing()
    {
        // The very same instances on both arms: the pass appends them by reference and writes
        // nothing to them, which is what lets one config list serve both passes
        var waves = new List<BossLocationSpawn> { NewSpawn("pmcUSEC", 500), NewSpawn("pmcBEAR", 900) };

        var legacySpawns = PmcWaveSpawns();
        var nativeSpawns = PmcWaveSpawns();

        var legacy = ApplyPmcWaves(forceLegacy: true, removeExistingPmcWaves: true, waves, legacySpawns);
        var native = ApplyPmcWaves(forceLegacy: false, removeExistingPmcWaves: true, waves, nativeSpawns);

        AssertMapParity(legacy, native, "pmc-waves");

        // A pass that dropped nothing and appended nothing would compare equal above and still be
        // wrong: the two PMC-named spawns go, the null-named one stays, and the two waves land
        Assert.That(native.BossLocationSpawn, Has.Count.EqualTo(4), "the removal filter did not drop exactly the two PMC spawns");

        // Aliasing channel 2's other half: the append puts the live config instances into the clone,
        // which is what lets the later spawn-time offset land on them (spec D3)
        Assert.That(native.BossLocationSpawn[^2], Is.SameAs(waves[0]), "the appended wave was a copy, not the config's own instance");
        Assert.That(native.BossLocationSpawn[^1], Is.SameAs(waves[1]));

        // Legacy's .Where().ToList() replaces the reference rather than editing the list in place
        Assert.That(native.BossLocationSpawn, Is.Not.SameAs(nativeSpawns), "the native arm edited the pre-call list in place");
        Assert.That(legacy.BossLocationSpawn, Is.Not.SameAs(legacySpawns));
    }

    /// <summary>
    /// Legacy appends only <em>inside</em> the <c>RemoveExistingPmcWaves</c> branch, so a false flag
    /// means no append either - the list is left exactly as it was, reference and all. Preserved
    /// bug-for-bug (the tier-1 tail design's Port 1).
    /// </summary>
    [Test]
    public void PmcWaveNoOpGatesMatchLegacy()
    {
        var waves = new List<BossLocationSpawn> { NewSpawn("pmcUSEC", 500) };

        var legacySpawns = PmcWaveSpawns();
        var nativeSpawns = PmcWaveSpawns();

        var legacy = ApplyPmcWaves(forceLegacy: true, removeExistingPmcWaves: false, waves, legacySpawns);
        var native = ApplyPmcWaves(forceLegacy: false, removeExistingPmcWaves: false, waves, nativeSpawns);

        AssertMapParity(legacy, native, "pmc-waves-no-op");

        Assert.That(native.BossLocationSpawn, Is.SameAs(nativeSpawns), "the native arm touched the list the flag told it to leave alone");
        Assert.That(legacy.BossLocationSpawn, Is.SameAs(legacySpawns));
    }

    /// <summary>
    /// The one booked divergence reachable on shipped data without a mod: <c>labyrinth</c> is in the
    /// location table and absent from <c>scavRaidTimeSettings.maps</c>, so the settings resolve
    /// throws - a <c>KeyNotFoundException</c> on the legacy arm, and the native arm's
    /// message-carrying <see cref="InvalidOperationException"/> instead. Both still throw, and
    /// neither applies a wave delta.
    /// </summary>
    [Test]
    public void AMissingMapSettingsKeyThrowsOnBothArms()
    {
        Assert.That(
            _locationConfig.ScavRaidTimeSettings.Maps.ContainsKey("labyrinth"),
            Is.False,
            "labyrinth gained a scav raid time settings entry, so this case no longer covers the settings-less map"
        );

        var changes = MapChanges(simulatedRaidStartSeconds: 600);

        Assert.Throws<KeyNotFoundException>(() => AdjustMap("labyrinth", changes, forceLegacy: true));

        var error = Assert.Throws<InvalidOperationException>(() => AdjustMap("labyrinth", changes, forceLegacy: false));

        // The un-lowercased id, which is what the warning on the sibling branch prints too
        Assert.That(
            error!.Message,
            Does.Contain(_locationTable.GetLocation("labyrinth")!.Base.Id),
            "the native error does not name the map the legacy key miss named"
        );
    }

    /// <summary>
    /// The whole hostility pass against shipped data: the config's two roles match the map's
    /// <c>pmcBEAR</c> and <c>pmcUSEC</c> entries case-insensitively, and every op the pass has - the
    /// enemy adds, the chanced-enemy refill, the friendly reset and the four scalars - is exercised
    /// by the shipped config.
    /// </summary>
    [Test]
    public void HostilityAdjustmentsMatchOnBothPaths()
    {
        var legacy = AdjustHostility(forceLegacy: true);
        var native = AdjustHostility(forceLegacy: false);

        AssertMapParity(legacy, native, "hostility");

        // A case that matched nothing would be comparing two untouched clones
        var config = _pmcConfig.HostilitySettings["pmcbear"];
        var applied = Settings(native, "pmcBEAR");
        Assert.That(applied.SavagePlayerBehaviour, Is.EqualTo(config.SavagePlayerBehaviour), "the ignore-case role match found nothing");
        Assert.That(applied.BearEnemyChance, Is.EqualTo(config.BearEnemyChance));
        Assert.That(applied.ChancedEnemies, Has.Count.EqualTo(config.ChancedEnemies!.Count), "the chanced enemies were not refilled");
        Assert.That(applied.AlwaysEnemies, Is.SupersetOf(config.AdditionalEnemyTypes!));
    }

    /// <summary>
    /// An unmatched role warns and skips inside the same loop that applies the matched ones, so the
    /// role after it still lands - which is what the single ordered entry list preserves.
    /// </summary>
    [Test]
    public void AnUnmatchedHostilityRoleWarnsAndSkipsOnBothPaths()
    {
        var legacy = AdjustHostility(forceLegacy: true, UnmatchedRoleConfig());
        var native = AdjustHostility(forceLegacy: false, UnmatchedRoleConfig());

        AssertMapParity(legacy, native, "unmatched-hostility-role");

        Assert.That(Settings(native, "pmcBEAR").SavageEnemyChance, Is.EqualTo(34), "the role after the unmatched one was skipped too");
    }

    /// <summary>
    /// Quirk 8's live merge branch: the clear precedes the loop, but the probe target is the list the
    /// loop itself refills, so a second config entry with the same <c>Role</c> reaches the merge and
    /// writes <c>EnemyChance</c> on the instance the first one appended. Unreachable on shipped data,
    /// preserved bug-for-bug.
    /// </summary>
    [Test]
    public void DuplicateRoleChancedEnemiesMergeIdenticallyOnBothPaths()
    {
        var legacy = AdjustHostility(forceLegacy: true, DuplicateRoleConfig());
        var native = AdjustHostility(forceLegacy: false, DuplicateRoleConfig());

        AssertMapParity(legacy, native, "duplicate-chanced-role");

        var chancedEnemies = Settings(native, "pmcBEAR").ChancedEnemies;
        Assert.That(chancedEnemies, Has.Count.EqualTo(1), "the duplicate role was appended instead of merged");
        Assert.That(chancedEnemies![0].EnemyChance, Is.EqualTo(90), "the merge did not write the later chance");
    }

    /// <summary>
    /// Quirk 8's null check is pure: a non-null <em>empty</em> config list still enters the branch,
    /// so it clears the location's list and refills it with nothing.
    /// </summary>
    [Test]
    public void AnEmptyChancedEnemiesListStillClearsOnBothPaths()
    {
        Assert.That(
            Settings(_locationTable.GetLocation(RaidStartMap)!.Base, "pmcBEAR").ChancedEnemies,
            Is.Not.Empty,
            "bigmap's pmcBEAR entry lost its chanced enemies, so this case no longer proves the clear"
        );

        var legacy = AdjustHostility(forceLegacy: true, EmptyChancedEnemiesConfig());
        var native = AdjustHostility(forceLegacy: false, EmptyChancedEnemiesConfig());

        AssertMapParity(legacy, native, "empty-chanced-enemies");

        Assert.That(Settings(native, "pmcBEAR").ChancedEnemies, Is.Empty, "the empty list did not clear the location's own");
    }

    /// <summary>
    /// Legacy touches <c>BotLocationModifier</c> only from inside its per-role loop, so a config with
    /// no roles at all never reads it - and the member is not <c>required</c>, so a mod-added
    /// <c>base.json</c> that omits it deserialises to null. Both are mod-shaped, and together they
    /// are the one case where materialising the hostility list up front would throw where legacy
    /// returns silently.
    /// </summary>
    [Test]
    public void AnEmptyHostilityConfigNeverTouchesTheLocationModifierOnBothPaths()
    {
        var legacy = AdjustHostility(forceLegacy: true, new Dictionary<string, HostilitySettings>(), NoLocationModifier);
        var native = AdjustHostility(forceLegacy: false, new Dictionary<string, HostilitySettings>(), NoLocationModifier);

        AssertMapParity(legacy, native, "empty-hostility-config");

        Assert.That(native.BotLocationModifier, Is.Null, "the pass put a modifier back on a map that had none");
    }

    /// <summary>
    /// Aliasing channel 3: the refill appends the live <c>PmcConfig</c> instances themselves, so a
    /// later write through the map reaches the config. Structural, so the native arm has to hand the
    /// applier the very same objects rather than decoded copies.
    /// </summary>
    [Test]
    public void ChancedEnemyInstancesAreTheLiveConfigObjectsOnTheNativeArm()
    {
        var config = EmptyChancedEnemiesConfig();
        config["pmcbear"].ChancedEnemies!.Add(new ChancedEnemy { Role = "assault", EnemyChance = 10 });

        var native = AdjustHostility(forceLegacy: false, config);

        Assert.That(Settings(native, "pmcBEAR").ChancedEnemies![0], Is.SameAs(config["pmcbear"].ChancedEnemies![0]));
    }

    /// <summary>
    /// The ordinary scav append against shipped data: <c>bigmap</c> carries 17 scav extracts among
    /// its 26, and an <c>AllExtractsExit</c> never equals a base <c>Exit</c>, so the union's dedup
    /// drops none of them.
    /// </summary>
    [Test]
    public void ScavExtractsAreAppendedIdenticallyOnBothPaths()
    {
        var legacy = AdjustRaidExtracts(ScavSide, RaidStartMap, forceLegacy: true);
        var native = AdjustRaidExtracts(ScavSide, RaidStartMap, forceLegacy: false);

        AssertMapParity(legacy, native, "scav-extracts");

        var originalExitCount = _locationTable.GetLocation(RaidStartMap)!.Base.Exits.Count();
        Assert.That(native.Exits.Count(), Is.GreaterThan(originalExitCount), "no extract was appended");
    }

    /// <summary>
    /// The side gate precedes the map lookup, so a pmc-side raid keeps the map's own exits.
    /// </summary>
    [Test]
    public void ANonScavSideIsANoOpOnBothPaths()
    {
        var legacy = AdjustRaidExtracts("Usec", RaidStartMap, forceLegacy: true);
        var native = AdjustRaidExtracts("Usec", RaidStartMap, forceLegacy: false);

        AssertMapParity(legacy, native, "non-scav-side");

        var originalExitCount = _locationTable.GetLocation(RaidStartMap)!.Base.Exits.Count();
        Assert.That(native.Exits.Count(), Is.EqualTo(originalExitCount), "a non-scav side had its exits replaced");
    }

    /// <summary>
    /// A map the location table does not have warns and makes no adjustment - the map name the
    /// warning prints never crosses the wire, so the applier is what emits it.
    /// </summary>
    [Test]
    public void AnUnknownExtractMapWarnsOnBothPaths()
    {
        var legacy = AdjustRaidExtracts(ScavSide, "no map is called this", forceLegacy: true);
        var native = AdjustRaidExtracts(ScavSide, "no map is called this", forceLegacy: false);

        AssertMapParity(legacy, native, "unknown-extract-map");

        var originalExitCount = _locationTable.GetLocation(RaidStartMap)!.Base.Exits.Count();
        Assert.That(native.Exits.Count(), Is.EqualTo(originalExitCount), "the unknown map still adjusted the exits");
    }

    /// <summary>
    /// The appended exits are the location table's own <c>AllExtracts</c> instances, not copies -
    /// the same live-object aliasing legacy's deferred <c>Where</c> hands the union.
    /// </summary>
    [Test]
    public void AppendedExtractInstancesAreTheLiveTableObjects()
    {
        var native = AdjustRaidExtracts(ScavSide, RaidStartMap, forceLegacy: false);

        var liveExtract = _locationTable
            .GetLocation(RaidStartMap)!
            .AllExtracts.First(extract => string.Equals(extract.Side, "scav", StringComparison.OrdinalIgnoreCase));

        Assert.That(native.Exits, Has.Some.SameAs(liveExtract), "the appended extract was a copy, not the table's own instance");
    }

    /// <summary>
    /// One hostility pass on one path, against a fresh clone of the raid-start map and whichever
    /// config map the case owns, with every singleton it touches restored afterwards.
    /// </summary>
    /// <param name="forceLegacy">Which path to take</param>
    /// <param name="hostilitySettings">
    ///     Replaces <c>pmcConfig.HostilitySettings</c> for the duration of the call; null leaves the
    ///     shipped one alone
    /// </param>
    /// <param name="prepare">Runs on the clone before the pass, as a mod's own base.json would have</param>
    private LocationBase AdjustHostility(
        bool forceLegacy,
        Dictionary<string, HostilitySettings>? hostilitySettings = null,
        Action<LocationBase>? prepare = null
    )
    {
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
        var map = _cloner.Clone(_locationTable.GetLocation(RaidStartMap)!.Base)!;
        prepare?.Invoke(map);

        var originalSettings = _pmcConfig.HostilitySettings;
        var originalForce = _locationConfig.ForceLegacyRaidAdjustments;

        try
        {
            if (hostilitySettings is not null)
            {
                _pmcConfig.HostilitySettings = hostilitySettings;
            }

            _locationConfig.ForceLegacyRaidAdjustments = forceLegacy;

            _adjustBotHostilitySettings.Invoke(_locationLifecycleService, [map]);

            // Fail fast on silent fallback before comparing anything
            Assert.That(
                _locationLifecycleService.LastPathTaken,
                Is.EqualTo(expected),
                $"the hostility pass did not take the {expected} path"
            );

            return map;
        }
        finally
        {
            _pmcConfig.HostilitySettings = originalSettings;
            _locationConfig.ForceLegacyRaidAdjustments = originalForce;
        }
    }

    /// <summary>
    /// One extract pass on one path, against a fresh clone of the raid-start map.
    /// </summary>
    /// <param name="playerSide">The side the raid is being entered on</param>
    /// <param name="location">The map name the extract lookup is made with</param>
    /// <param name="forceLegacy">Which path to take</param>
    private LocationBase AdjustRaidExtracts(string playerSide, string location, bool forceLegacy)
    {
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
        var map = _cloner.Clone(_locationTable.GetLocation(RaidStartMap)!.Base)!;
        var originalForce = _locationConfig.ForceLegacyRaidAdjustments;

        try
        {
            _locationConfig.ForceLegacyRaidAdjustments = forceLegacy;

            _adjustExtracts.Invoke(_locationLifecycleService, [playerSide, location, map]);

            // Fail fast on silent fallback before comparing anything
            Assert.That(
                _locationLifecycleService.LastPathTaken,
                Is.EqualTo(expected),
                $"the extract pass did not take the {expected} path"
            );

            return map;
        }
        finally
        {
            _locationConfig.ForceLegacyRaidAdjustments = originalForce;
        }
    }

    /// <summary>
    /// One PMC wave pass on one path, against a fresh clone of the raid-start map, with the two
    /// config members the pass reads replaced for the duration of the call.
    /// </summary>
    /// <param name="forceLegacy">Which path to take</param>
    /// <param name="removeExistingPmcWaves">The config flag the whole pass is gated behind</param>
    /// <param name="customWaves">The map's <c>CustomPmcWaves</c> entry for the call</param>
    /// <param name="spawns">
    ///     The map's boss spawns for the call, handed in so the caller owns the list instance the
    ///     reference-replacement assertions compare against
    /// </param>
    private LocationBase ApplyPmcWaves(
        bool forceLegacy,
        bool removeExistingPmcWaves,
        List<BossLocationSpawn> customWaves,
        List<BossLocationSpawn> spawns
    )
    {
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
        var map = _cloner.Clone(_locationTable.GetLocation(RaidStartMap)!.Base)!;
        map.BossLocationSpawn = spawns;

        // The key the pass itself resolves with, which is the map's own lowercased id
        var wavesKey = map.Id.ToLowerInvariant();
        var originalRemove = _pmcConfig.RemoveExistingPmcWaves;
        var wavesPresent = _pmcConfig.CustomPmcWaves.TryGetValue(wavesKey, out var originalWaves);
        var originalForce = _locationConfig.ForceLegacyRaidAdjustments;

        try
        {
            _pmcConfig.RemoveExistingPmcWaves = removeExistingPmcWaves;
            _pmcConfig.CustomPmcWaves[wavesKey] = customWaves;
            _locationConfig.ForceLegacyRaidAdjustments = forceLegacy;

            _pmcWaveGenerator.ApplyWaveChangesToMap(map);

            // Fail fast on silent fallback before comparing anything
            Assert.That(_pmcWaveGenerator.LastPathTaken, Is.EqualTo(expected), $"the wave pass did not take the {expected} path");

            return map;
        }
        finally
        {
            _pmcConfig.RemoveExistingPmcWaves = originalRemove;

            if (wavesPresent)
            {
                _pmcConfig.CustomPmcWaves[wavesKey] = originalWaves!;
            }
            else
            {
                _pmcConfig.CustomPmcWaves.Remove(wavesKey);
            }

            _locationConfig.ForceLegacyRaidAdjustments = originalForce;
        }
    }

    /// <summary>
    /// The four spawns every arm of the removal filter needs: a non-PMC name, both PMC names and a
    /// null one. Fresh instances per call, so each arm owns the list it hands the pass.
    /// </summary>
    private static List<BossLocationSpawn> PmcWaveSpawns()
    {
        return [NewSpawn("bossBully", 10), NewSpawn("pmcUSEC", 20), NewSpawn(null, 30), NewSpawn("pmcBEAR", 40)];
    }

    /// <summary>
    /// One map's hostility entry for a bot role. Every raid-start case reads its result through
    /// this, so a pass that wrote the wrong entry shows up as a missing write.
    /// </summary>
    private static AdditionalHostilitySettings Settings(LocationBase map, string botRole)
    {
        return map.BotLocationModifier.AdditionalHostilitySettings!.First(settings => settings.BotRole == botRole);
    }

    /// <summary>
    /// A role no map has, ahead of one every map has: the warn-and-skip has to leave the second one
    /// applying. Fresh instances per call - the passes write config objects in place.
    /// </summary>
    private static Dictionary<string, HostilitySettings> UnmatchedRoleConfig()
    {
        return new Dictionary<string, HostilitySettings>
        {
            ["noBotIsCalledThis"] = new() { BearEnemyChance = 12 },
            ["pmcbear"] = new() { SavageEnemyChance = 34 },
        };
    }

    /// <summary>
    /// Mod-shaped: one role whose chanced enemies name the same <c>Role</c> twice.
    /// </summary>
    private static Dictionary<string, HostilitySettings> DuplicateRoleConfig()
    {
        return new Dictionary<string, HostilitySettings>
        {
            ["pmcbear"] = new()
            {
                ChancedEnemies =
                [
                    new ChancedEnemy { Role = "assault", EnemyChance = 10 },
                    new ChancedEnemy { Role = "assault", EnemyChance = 90 },
                ],
            },
        };
    }

    /// <summary>
    /// A map whose <c>base.json</c> omitted <c>BotLocationModifier</c> altogether.
    /// </summary>
    private static void NoLocationModifier(LocationBase map)
    {
        map.BotLocationModifier = null!;
    }

    /// <summary>
    /// A non-null, empty chanced-enemy list - the pure-null-check case.
    /// </summary>
    private static Dictionary<string, HostilitySettings> EmptyChancedEnemiesConfig()
    {
        return new Dictionary<string, HostilitySettings> { ["pmcbear"] = new() { ChancedEnemies = [] } };
    }

    /// <summary>
    /// The whole adjusted map, for the raid-start passes: a delta applied to the wrong member, or one
    /// the native arm forgot, shows up here whatever it was.
    /// </summary>
    private void AssertMapParity(LocationBase legacy, LocationBase native, string what)
    {
        Assert.That(_jsonUtil.Serialize(native), Is.EqualTo(_jsonUtil.Serialize(legacy)), $"{what}: the adjusted maps differ");
    }

    /// <summary>
    /// One map adjustment on one path, against a fresh clone, with every singleton it touches
    /// restored afterwards.
    /// </summary>
    /// <param name="location">The map to clone and adjust</param>
    /// <param name="changes">The changes a <c>GetRaidAdjustments</c> call would have parked</param>
    /// <param name="forceLegacy">Which path to take</param>
    /// <param name="prepare">Runs on the clone before the adjustment, as the pipeline's PMC splice does</param>
    /// <param name="replaceSettings">
    ///     Replaces the map's settings entry for the duration of the call; null leaves it alone
    /// </param>
    private MapAdjustment AdjustMap(
        string location,
        RaidChanges changes,
        bool forceLegacy,
        Action<LocationBase>? prepare = null,
        Func<ScavRaidTimeLocationSettings?, ScavRaidTimeLocationSettings?>? replaceSettings = null
    )
    {
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
        var mapBase = _cloner.Clone(_locationTable.GetLocation(location)!.Base);
        prepare?.Invoke(mapBase);

        var maps = _locationConfig.ScavRaidTimeSettings.Maps;

        // The key the pass itself resolves with, which is the map's own id and not the name it was
        // asked for by
        var settingsKey = mapBase.Id.ToLowerInvariant();
        var settingsPresent = maps.TryGetValue(settingsKey, out var originalSettings);
        var originalForce = _locationConfig.ForceLegacyRaidAdjustments;

        // AdjustLootMultipliers only ever overwrites keys it found, so putting the values back is
        // the whole restore
        var looseMultipliers = new Dictionary<string, double>(_locationConfig.LooseLootMultiplier);
        var staticMultipliers = new Dictionary<string, double>(_locationConfig.StaticLootMultiplier);

        try
        {
            if (replaceSettings is not null)
            {
                maps[settingsKey] = replaceSettings(originalSettings);
            }

            _locationConfig.ForceLegacyRaidAdjustments = forceLegacy;

            _raidTimeAdjustmentService.MakeAdjustmentsToMap(changes, mapBase);

            // Fail fast on silent fallback before comparing anything
            Assert.That(_raidTimeAdjustmentService.LastPathTaken, Is.EqualTo(expected), $"the adjustment did not take the {expected} path");

            return new MapAdjustment(
                mapBase,
                new Dictionary<string, double>(_locationConfig.LooseLootMultiplier),
                new Dictionary<string, double>(_locationConfig.StaticLootMultiplier)
            );
        }
        finally
        {
            if (replaceSettings is not null)
            {
                if (settingsPresent)
                {
                    maps[settingsKey] = originalSettings;
                }
                else
                {
                    maps.Remove(settingsKey);
                }
            }

            _locationConfig.ForceLegacyRaidAdjustments = originalForce;

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

    /// <summary>
    /// The whole adjusted map, not just the members a case names: a delta applied to the wrong
    /// member, or one the native arm forgot, shows up here whatever it was.
    /// </summary>
    private void AssertMapParity(MapAdjustment legacy, MapAdjustment native, string what)
    {
        Assert.That(_jsonUtil.Serialize(native.Map), Is.EqualTo(_jsonUtil.Serialize(legacy.Map)), $"{what}: the adjusted maps differ");
    }

    /// <summary>
    /// A parked <see cref="RaidChanges"/> as <c>GetRaidAdjustments</c> would have left it. The loot
    /// percents default to 100 so no case scales the live multipliers by accident.
    /// </summary>
    private static RaidChanges MapChanges(
        double simulatedRaidStartSeconds,
        List<ExtractChange>? exitChanges = null,
        double dynamicLootPercent = 100,
        double staticLootPercent = 100
    )
    {
        return new RaidChanges
        {
            DynamicLootPercent = dynamicLootPercent,
            StaticLootPercent = staticLootPercent,
            SimulatedRaidStartSeconds = simulatedRaidStartSeconds,
            RaidTimeMinutes = 30,
            NewSurviveTimeSeconds = 100,
            OriginalSurvivalTimeSeconds = 1000,
            ExitChanges = exitChanges ?? [],
        };
    }

    private static Func<ScavRaidTimeLocationSettings?, ScavRaidTimeLocationSettings?> WithAdjustWaves(bool adjustWaves)
    {
        return original => original! with { AdjustWaves = adjustWaves };
    }

    /// <summary>
    /// Deterministic waves in place of the map's own, and no boss spawns beside them. The
    /// prototypes are copied per call: the pass writes the wave objects in place, so handing the
    /// same instances to both arms would have the second one adjusting what the first already did.
    /// </summary>
    private static Action<LocationBase> WithWaves(params Wave[] waves)
    {
        return map =>
        {
            map.Waves = waves.Select(wave => wave with { }).ToList();
            map.BossLocationSpawn = [];
        };
    }

    /// <summary>
    /// Deterministic boss spawns in place of the map's own, copied per call for the same reason
    /// <see cref="WithWaves"/> copies.
    /// </summary>
    private static Action<LocationBase> WithBossSpawns(params BossLocationSpawn[] spawns)
    {
        return map =>
        {
            map.Waves = [];
            map.BossLocationSpawn = spawns.Select(spawn => spawn with { }).ToList();
        };
    }

    /// <summary>
    /// One wave and one PMC spawn late enough to survive the simulated start, on top of the map's
    /// own. Fresh instances per call - the pass writes them in place.
    /// </summary>
    private static void WithASurvivorOfEach(LocationBase map)
    {
        map.Waves.Add(NewWave(2000, 5000));
        map.BossLocationSpawn.Add(NewSpawn("pmcUSEC", 5000));
    }

    private static Wave NewWave(int timeMin, int? timeMax)
    {
        return new Wave { TimeMin = timeMin, TimeMax = timeMax };
    }

    private static BossLocationSpawn NewSpawn(string? bossName, double time)
    {
        return new BossLocationSpawn { BossName = bossName, Time = time };
    }

    /// <summary>
    /// One path's map adjustment: the map it left behind, and the live multiplier dictionaries as
    /// they stood immediately after the call.
    /// </summary>
    private sealed record MapAdjustment(
        LocationBase Map,
        Dictionary<string, double> LooseLootMultiplier,
        Dictionary<string, double> StaticLootMultiplier
    );

    /// <summary>
    /// One adjustment on one path, with every singleton it touches restored afterwards.
    /// </summary>
    /// <param name="side">Request side</param>
    /// <param name="location">Request location, as the client would spell it</param>
    /// <param name="seed">Seed for whichever RNG seam the chosen path draws through</param>
    /// <param name="forceLegacy">Which path to take</param>
    /// <param name="replaceSettings">
    ///     Replaces the map's settings entry for the duration of the call; null leaves it alone
    /// </param>
    private Adjustment Adjust(
        string side,
        string location,
        ulong seed,
        bool forceLegacy,
        Func<ScavRaidTimeLocationSettings?, ScavRaidTimeLocationSettings?>? replaceSettings = null
    )
    {
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
        var maps = _locationConfig.ScavRaidTimeSettings.Maps;
        var settingsKey = location.ToLowerInvariant();
        var settingsPresent = maps.TryGetValue(settingsKey, out var originalSettings);
        var originalForce = _locationConfig.ForceLegacyRaidAdjustments;
        var originalSource = _randomUtil.RandomSource;

        try
        {
            if (replaceSettings is not null)
            {
                maps[settingsKey] = replaceSettings(originalSettings);
            }

            _locationConfig.ForceLegacyRaidAdjustments = forceLegacy;

            if (forceLegacy)
            {
                _randomUtil.RandomSource = new SeededRandomSource(seed);
            }
            else
            {
                _raidTimeAdjustmentService.NativeTestSeed = seed;
            }

            // Whatever an earlier case parked must not be read as this one's result
            _profileActivityService.GetProfileActivityRaidData(_sessionId).RaidAdjustments = null;

            var changes = _raidTimeAdjustmentService.GetRaidAdjustments(
                _sessionId,
                new GetRaidTimeRequest { Side = side, Location = location }
            );

            // Fail fast on silent fallback before comparing anything
            Assert.That(_raidTimeAdjustmentService.LastPathTaken, Is.EqualTo(expected), $"the adjustment did not take the {expected} path");

            return new Adjustment(changes, _profileActivityService.GetProfileActivityRaidData(_sessionId).RaidAdjustments);
        }
        finally
        {
            if (replaceSettings is not null)
            {
                if (settingsPresent)
                {
                    maps[settingsKey] = originalSettings;
                }
                else
                {
                    maps.Remove(settingsKey);
                }
            }

            _locationConfig.ForceLegacyRaidAdjustments = originalForce;
            _randomUtil.RandomSource = originalSource;
            _raidTimeAdjustmentService.NativeTestSeed = null;
            _profileActivityService.GetProfileActivityRaidData(_sessionId).RaidAdjustments = null;
        }
    }

    private void AssertParity(Adjustment legacy, Adjustment native, string what, ulong seed)
    {
        Assert.That(Normalise(native.Changes), Is.EqualTo(Normalise(legacy.Changes)), $"{what} seed={seed}: the raid changes differ");
        Assert.That(
            Normalise(native.Parked),
            Is.EqualTo(Normalise(legacy.Parked)),
            $"{what} seed={seed}: what the two paths parked on the session differs"
        );
    }

    /// <summary>
    /// The comparison form. No ids are minted anywhere in this family, so serialising is the whole
    /// normalisation.
    /// </summary>
    private string Normalise(RaidChanges? changes)
    {
        return changes is null ? "<nothing parked>" : _jsonUtil.Serialize(changes)!;
    }

    /// <summary>
    /// A replacement settings entry off the map's own, so only the members a case cares about move.
    /// A fresh record rather than an edit in place, which is what lets the restore be a single
    /// reference assignment.
    /// </summary>
    private static Func<ScavRaidTimeLocationSettings?, ScavRaidTimeLocationSettings?> WithSettings(
        double reducedChancePercent,
        Dictionary<string, double>? reductionPercentWeights = null
    )
    {
        return original =>
            original! with
            {
                ReducedChancePercent = reducedChancePercent,
                ReductionPercentWeights = reductionPercentWeights ?? original!.ReductionPercentWeights,
            };
    }

    /// <summary>
    /// One path's result: what it returned and what it left on the session.
    /// </summary>
    private sealed record Adjustment(RaidChanges Changes, RaidChanges? Parked);
}
