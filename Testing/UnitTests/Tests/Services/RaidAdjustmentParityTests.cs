using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Game;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Location;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.InRaid;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Services;

/// <summary>
/// Golden parity gate on the scav raid time port: the same seed must make the legacy C# path and the
/// spt-native path produce the same <see cref="RaidChanges"/> and park the same thing on the
/// session.
///
/// State this fixture mutates, all of it restored in <see cref="Adjust"/>'s <c>finally</c>:
/// <see cref="LocationConfig.ForceLegacyRaidAdjustments"/> on the shared config singleton - the path
/// selector - the one <c>ScavRaidTimeSettings.Maps</c> entry a case forces,
/// <see cref="RandomUtil.RandomSource"/>, which is the seam the legacy path draws through,
/// <c>NativeTestSeed</c>, and the session's parked adjustments. Map entries are always
/// <em>replaced</em> rather than edited in place, so restoring the original reference restores
/// everything. Nothing here reads resident database state, so no mutation stamp bumps are needed.
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

    private readonly MongoId _sessionId = new();

    private RaidTimeAdjustmentService _raidTimeAdjustmentService = default!;
    private LocationConfig _locationConfig = default!;
    private LocationTable _locationTable = default!;
    private RandomUtil _randomUtil = default!;
    private ProfileActivityService _profileActivityService = default!;
    private JsonUtil _jsonUtil = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _raidTimeAdjustmentService = di.GetService<RaidTimeAdjustmentService>();
        _locationConfig = di.GetService<LocationConfig>();
        _locationTable = di.GetService<LocationTable>();
        _randomUtil = di.GetService<RandomUtil>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _jsonUtil = di.GetService<JsonUtil>();
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
