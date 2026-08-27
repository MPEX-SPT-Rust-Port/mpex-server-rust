using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Raid;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the wire contract between <c>Native/Raid/RaidPayloads.cs</c> and
/// <c>spt_get_raid_adjustments</c>. A misspelled <c>JsonPropertyName</c> on either side silently
/// hands the native code a default - a null side, a zero escape time, an empty weight map - so it
/// fails here, against the real library, rather than later as a parity mismatch nobody can localise.
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
        Assert.That(changes.ExitChanges, Has.Count.EqualTo(1), "the train exits did not cross");
        Assert.That(changes.ExitChanges![0].Name, Is.EqualTo("LateTrain"));
        Assert.That(changes.ExitChanges[0].MinTime, Is.EqualTo(80));
        Assert.That(changes.ExitChanges[0].MaxTime, Is.EqualTo(180));
        Assert.That(changes.ExitChanges[0].Chance, Is.Null);
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
