using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Location;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Raid;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the wire contract between <c>Native/Raid/RaidPayloads.cs</c> and the raid-setup exports.
/// A misspelled <c>JsonPropertyName</c> on either side silently hands the native code a default - a
/// null side, a zero escape time, an empty weight map, an empty wave list - so it fails here,
/// against the real library, rather than later as a parity mismatch nobody can localise.
///
/// The requests go through the internal <c>Generate</c> ladder rather than the typed wrapper so the
/// bytes on the wire are the fixture's own, and they carry the same options production serialises
/// with.
/// </summary>
[TestFixture]
public class RaidAdjustmentWireContractTests
{
    /// <summary>
    /// The container build is what publishes <see cref="JsonUtil.JsonSerializerOptionsNoIndent"/>,
    /// and the payload options are that property - so a run filtered down to this fixture alone
    /// still has them.
    /// </summary>
    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        DI.GetInstance();
    }

    /// <summary>
    /// Every request member the response can observe, read back off a decoded response. The numbers
    /// are the ones <c>rust/spt-native/src/raid/adjustments.rs</c>'s own unit tests pin, so a
    /// mismatch here is a wire fault and not a port fault.
    /// </summary>
    [Test]
    public void GetRaidAdjustmentsRoundTripsThroughTheRealLibrary()
    {
        var response = Send(BuildRequest());

        // side: a dropped side reads as null, which fails the "pmc" test and applies anyway - so the
        // members below are what actually prove it crossed, together with the pmc case in
        // RaidAdjustmentParityTests.
        Assert.That(response.Applied, Is.True);
        Assert.That(response.ChosenReductionPercent, Is.EqualTo(20), "the weight map did not cross");
        Assert.That(response.MapSettingsMissingValue, Is.False, "the settings value did not cross");

        var changes = response.RaidChanges;
        // escapeTimeLimit: floor(60 - 20%) == 48
        Assert.That(changes.RaidTimeMinutes, Is.EqualTo(48));
        Assert.That(changes.SimulatedRaidStartSeconds, Is.EqualTo(720));
        // survivedSecondsRequirement: max(1000 - (60 - 48) * 60, 0) == 280
        Assert.That(changes.NewSurviveTimeSeconds, Is.EqualTo(280));
        Assert.That(changes.OriginalSurvivalTimeSeconds, Is.EqualTo(1000));
        // reduceLootByPercent plus the two floors: 100 - 20 == 80, under the static floor of 90
        Assert.That(changes.DynamicLootPercent, Is.EqualTo(80));
        Assert.That(changes.StaticLootPercent, Is.EqualTo(90));

        // trainExits, and the PascalCase ExtractChange names: (800 + 60 + 5 + 88) / 60 leaves 44.1
        // of the 60 minutes, above the 48-minute raid, so this exit reduces rather than disabling
        Assert.That(changes.ExitChanges, Has.Count.EqualTo(2), "the train exits did not cross");
        Assert.That(changes.ExitChanges![0].Name, Is.EqualTo("LateTrain"));
        Assert.That(changes.ExitChanges[0].MinTime, Is.EqualTo(80));
        Assert.That(changes.ExitChanges[0].MaxTime, Is.EqualTo(180));
        Assert.That(changes.ExitChanges[0].Chance, Is.Null);

        // The disable branch, and the only place a non-null Chance ever crosses: a dropped or
        // misspelled Chance decodes as the null above on every other exit, so without this the
        // "train has already left" case would arrive at the client fully enabled
        Assert.That(changes.ExitChanges[1].Name, Is.EqualTo("EarlyTrain"));
        Assert.That(changes.ExitChanges[1].Chance, Is.EqualTo(0));
        Assert.That(changes.ExitChanges[1].MinTime, Is.Null);
        Assert.That(changes.ExitChanges[1].MaxTime, Is.Null);
    }

    /// <summary>
    /// <c>location</c> reaches the response through one channel only - the missing-key error text -
    /// so nothing else can prove it crossed.
    /// </summary>
    [Test]
    public void AMissingMapSettingsKeyCarriesTheLocationBack()
    {
        var request = BuildRequest();
        request.MapSettings.Found = false;
        request.MapSettings.Value = null;

        var error = Assert.Throws<InvalidOperationException>(() => Send(request));

        Assert.That(error.Message, Does.Contain("bigmap"));
    }

    /// <summary>
    /// <c>testSeed</c> has no member of its own in the response, so it is pinned by its effect: a
    /// dropped seed leaves the native stream on the process seed, and two sends of one request would
    /// then almost never agree on a 16-way weighted draw.
    /// </summary>
    [Test]
    public void TheTestSeedPinsTheNativeStream()
    {
        var request = BuildRequest();
        request.MapSettings.Value!.ReductionPercentWeights = Enumerable
            .Range(0, 16)
            .ToDictionary(index => (index + 20).ToString(), index => index % 2 == 0 ? 2d : 1d);

        var first = Send(request);
        var second = Send(request);

        Assert.That(second.ChosenReductionPercent, Is.EqualTo(first.ChosenReductionPercent));
    }

    /// <summary>
    /// Every request member of the second export, read back off the deltas it produces. Each
    /// assertion below is the only channel its member has: drop <c>waves</c> and nothing is kept,
    /// drop <c>exits</c> and every exit change aborts, drop a <c>bossSpawns</c> time and the offset
    /// moves.
    /// </summary>
    [Test]
    public void MakeAdjustmentsToMapRoundTripsThroughTheRealLibrary()
    {
        var response = Send(BuildAdjustRequest());

        // raidChanges.raidTimeMinutes
        Assert.That(response.EscapeTimeLimit, Is.EqualTo(33));
        Assert.That(response.Aborted, Is.False);
        Assert.That(response.MapSettingsMissingValue, Is.False, "the settings value did not cross");

        // exits, plus the PascalCase ExtractChange names on the way in: "Exit_B" is at index 1, and
        // the null-named change matches the null-named exit at index 2
        Assert.That(response.ExitUpdates, Has.Count.EqualTo(2), "the exit changes did not cross");
        Assert.That(response.ExitUpdates[0].Index, Is.EqualTo(1));
        Assert.That(response.ExitUpdates[0].MinTime, Is.EqualTo(10));
        Assert.That(response.ExitUpdates[0].MaxTime, Is.EqualTo(20));
        Assert.That(response.ExitUpdates[0].Chance, Is.Null);
        Assert.That(response.ExitUpdates[1].Index, Is.EqualTo(2));
        Assert.That(response.ExitUpdates[1].Chance, Is.EqualTo(5));

        // mapSettings.value: false would have left this null
        var waves = response.WaveAdjustments;
        Assert.That(waves, Is.Not.Null, "the AdjustWaves setting did not cross");

        // waves, against raidChanges.simulatedRaidStartSeconds of 100: only the 500 survives, and it
        // loses the start seconds twice
        Assert.That(waves!.WaveKeepIndices, Is.EqualTo(new[] { 0 }));
        Assert.That(waves.WaveTimes, Has.Count.EqualTo(1));
        Assert.That(waves.WaveTimes[0].TimeMin, Is.EqualTo(100));
        Assert.That(waves.WaveTimes[0].TimeMax, Is.EqualTo(300));
        Assert.That(waves.RemovedWaveCount, Is.EqualTo(2));

        // bossSpawns: the ignore-case keep test drops only the early pmcBEAR, and the case-sensitive
        // offset test moves only the exactly-spelled pmcUSEC
        Assert.That(waves.BossKeepIndices, Is.EqualTo(new[] { 0, 1, 3, 4 }));
        Assert.That(waves.RemovedBossCount, Is.EqualTo(1));
        Assert.That(waves.PmcStartSeconds, Is.EqualTo(200));
        Assert.That(waves.BossTimeUpdates, Has.Count.EqualTo(1));
        Assert.That(waves.BossTimeUpdates[0].Index, Is.EqualTo(0));
        Assert.That(waves.BossTimeUpdates[0].Time, Is.EqualTo(1));
    }

    /// <summary>
    /// <c>aborted</c> and the name beside it: a bool of its own because a null name is a legitimate
    /// thing to have failed to match. The settings here are missing outright, and the abort still
    /// comes back rather than the error - the abort precedes the resolve.
    /// </summary>
    [Test]
    public void AnUnmatchedExitNameCrossesBackAsAnAbort()
    {
        var request = BuildAdjustRequest();
        request.RaidChanges.ExitChanges!.Add(new ExtractChange { Name = "Exit_Missing" });
        request.MapSettings.Found = false;
        request.MapSettings.Value = null;

        var response = Send(request);

        Assert.That(response.Aborted, Is.True);
        Assert.That(response.AbortedExitName, Is.EqualTo("Exit_Missing"));
        Assert.That(response.ExitUpdates, Has.Count.EqualTo(2), "the updates emitted before the abort were lost");
        Assert.That(response.WaveAdjustments, Is.Null);
        Assert.That(response.MapSettingsMissingValue, Is.False);
    }

    /// <summary>
    /// A present key with a null value reaches the applier as the warning flag alone, and disables
    /// the wave half on the way - that flag is the only thing the applier can see it by.
    /// </summary>
    [Test]
    public void ANullMapSettingsValueCrossesBackAsTheWarningFlag()
    {
        var request = BuildAdjustRequest();
        request.MapSettings.Value = null;

        var response = Send(request);

        Assert.That(response.MapSettingsMissingValue, Is.True);
        Assert.That(response.WaveAdjustments, Is.Null);
    }

    /// <summary>
    /// <c>mapId</c> reaches the caller through one channel only, the same as export 1's
    /// <c>location</c>.
    /// </summary>
    [Test]
    public void AMissingMapSettingsKeyOnTheAdjustPassCarriesTheMapIdBack()
    {
        var request = BuildAdjustRequest();
        request.MapSettings.Found = false;
        request.MapSettings.Value = null;

        var error = Assert.Throws<InvalidOperationException>(() => Send(request));

        Assert.That(error!.Message, Does.Contain("bigmap"));
    }

    /// <summary>
    /// A request with every member set: two matching exit changes, three waves of which one
    /// survives, and five boss spawns covering both of the pmc name tests.
    /// </summary>
    private static MakeAdjustmentsRequest BuildAdjustRequest()
    {
        return new MakeAdjustmentsRequest
        {
            MapId = "bigmap",
            RaidChanges = new RaidChanges
            {
                RaidTimeMinutes = 33,
                SimulatedRaidStartSeconds = 100,
                ExitChanges =
                [
                    new ExtractChange
                    {
                        Name = "Exit_B",
                        MinTime = 10,
                        MaxTime = 20,
                    },
                    // A null name matches the null-named exit: legacy's `==` is ordinal and matches
                    // null to null
                    new ExtractChange { Name = null, Chance = 5 },
                ],
            },
            MapSettings = new MapSettingsAdjustState { Found = true, Value = true },
            Exits = ["Exit_A", "Exit_B", null],
            Waves =
            [
                new WaveTimesWire { TimeMin = 300, TimeMax = 500 },
                new WaveTimesWire { TimeMin = 10, TimeMax = 50 },
                new WaveTimesWire { TimeMin = 5, TimeMax = null },
            ],
            BossSpawns =
            [
                new BossSpawnWire { BossName = "pmcUSEC", Time = 200 },
                // Kept by the ignore-case test, skipped by the case-sensitive offset test
                new BossSpawnWire { BossName = "PMCUSEC", Time = 300 },
                new BossSpawnWire { BossName = "pmcBEAR", Time = 50 },
                // A null name "isn't a pmc", so it is kept whatever its time
                new BossSpawnWire { BossName = null, Time = 10 },
                new BossSpawnWire { BossName = "bossKilla", Time = 20 },
            ],
        };
    }

    private static MakeAdjustmentsResponse Send(MakeAdjustmentsRequest request)
    {
        return SptNative.Generate<MakeAdjustmentsResponse>(
            LootExport.MakeAdjustmentsToMap,
            JsonSerializer.SerializeToUtf8Bytes(request, JsonUtil.JsonSerializerOptionsNoIndent)
        );
    }

    /// <summary>
    /// A request with every member set, the settings key present and one train exit on the reduce
    /// branch.
    /// </summary>
    private static GetRaidAdjustmentsRequest BuildRequest()
    {
        return new GetRaidAdjustmentsRequest
        {
            Side = "Savage",
            Location = "bigmap",
            EscapeTimeLimit = 60,
            SurvivedSecondsRequirement = 1000,
            TrainArrivalDelayObservedSeconds = 88,
            MapSettings = new MapSettingsState
            {
                Found = true,
                Value = new ScavRaidTimeLocationSettingsWire
                {
                    ReducedChancePercent = 100,
                    ReductionPercentWeights = new Dictionary<string, double> { ["20"] = 1 },
                    ReduceLootByPercent = true,
                    MinDynamicLootPercent = 50,
                    MinStaticLootPercent = 90,
                },
            },
            TrainExits =
            [
                new TrainExitWire
                {
                    Name = "LateTrain",
                    MinTime = 800,
                    MaxTime = 900,
                    Count = 60,
                    ExfiltrationTime = 5,
                },
                // (1 + 1 + 1 + 88) / 60 leaves 58.5 of the 60 minutes, above the 48-minute raid, so
                // this train has already left and the exit is disabled with a Chance of 0
                new TrainExitWire
                {
                    Name = "EarlyTrain",
                    MinTime = 1,
                    MaxTime = 2,
                    Count = 1,
                    ExfiltrationTime = 1,
                },
            ],
            TestSeed = 42,
        };
    }

    private static GetRaidAdjustmentsResponse Send(GetRaidAdjustmentsRequest request)
    {
        return SptNative.Generate<GetRaidAdjustmentsResponse>(
            LootExport.RaidAdjustments,
            JsonSerializer.SerializeToUtf8Bytes(request, JsonUtil.JsonSerializerOptionsNoIndent)
        );
    }
}
