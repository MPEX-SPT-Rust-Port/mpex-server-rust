using System.Globalization;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weather;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Weather;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Golden parity gate on the weather port: at one seed, with one explicit timestamp (bar the
/// null-timestamp case, which is the request shape <c>client/weather</c> makes), the legacy C# path
/// and the spt-native pass must produce the same <c>Weather</c> - every drawn member, the
/// temperature, the chosen preset and the BSG date/time strings - and must leave the caller's
/// <c>ref</c> dictionary in the same state, with the same reference identity.
///
/// Both arms come off the one container-built generator, flipped with
/// <see cref="WeatherConfig.ForceLegacyWeatherGeneration"/> - the as-built recipe. The
/// frozen-constructor and subclass declines belong to <see cref="WeatherPathDispatchTests"/>.
///
/// SEEDING DISCIPLINE: every call re-seeds <em>both</em> arms.
/// <c>TestSeedGuard::install</c> starts a fresh xoshiro stream per FFI call, while one
/// <see cref="SeededRandomSource"/> would carry its stream across calls - so a multi-call
/// comparison on a single seed fails by construction (<c>random_util.rs:41-65</c>). A mismatch here
/// is a seeding or draw-order bug, never a reason to weaken an assertion.
///
/// State this fixture mutates, all restored: the force flag and, for the sequence case, one added
/// <c>weatherPresetWeight</c> season entry plus <see cref="RandomUtil.RandomSource"/>.
/// </summary>
[TestFixture]
[NonParallelizable]
public class WeatherParityTests
{
    /// <summary>
    /// Hour 6 off the seconds-as-ticks quirk (<c>new DateTime(seconds).Hour</c>), so
    /// <c>IsHourAtNightTime</c> is false - and small enough that
    /// <c>DateTimeOffset.FromUnixTimeSeconds</c> still has a date for it.
    /// </summary>
    private const long DayTimestamp = 6L * 36_000_000_000L;

    /// <summary>
    /// Hour 3 by the same quirk: night.
    /// </summary>
    private const long NightTimestamp = 3L * 36_000_000_000L;

    /// <summary>
    /// The season the sequence case overrides an entry for. Not a shipped
    /// <c>weatherPresetWeight</c> key, so the entry is added and removed rather than replaced.
    /// </summary>
    private const Season SequenceSeason = Season.SUMMER;

    private WeatherGenerator _generator = default!;
    private WeatherNativeRequestBuilder _requestBuilder = default!;
    private WeatherConfig _weatherConfig = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private IRandomSource _originalRandomSource = default!;
    private bool _originalForceLegacy;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _generator = di.GetService<WeatherGenerator>();
        _requestBuilder = di.GetService<WeatherNativeRequestBuilder>();
        _weatherConfig = di.GetService<WeatherConfig>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();

        _originalForceLegacy = _weatherConfig.ForceLegacyWeatherGeneration;
        _originalRandomSource = _randomUtil.RandomSource;
    }

    [TearDown]
    public void TearDown()
    {
        // The captured value, not false: a tree shipping the flag on would otherwise have it
        // silently flipped for every fixture that runs after this one
        _weatherConfig.ForceLegacyWeatherGeneration = _originalForceLegacy;
        _randomUtil.RandomSource = _originalRandomSource;
        _generator.NativeTestSeed = null;
    }

    /// <summary>
    /// One call per preset arm per day/night arm. The state is a single entry, so the pick is
    /// forced without consuming a draw on either side and the whole seeded stream goes to the
    /// preset's own draws plus the temperature.
    /// </summary>
    [Test]
    public void OneSeededCallMatchesLegacyPerPresetAndNightArm()
    {
        ulong seed = 20260827;

        foreach (var preset in Enum.GetValues<WeatherPreset>())
        {
            foreach (var (timestamp, arm) in new[] { (DayTimestamp, "day"), (NightTimestamp, "night") })
            {
                seed++;

                var legacyState = ForcedState(preset);
                var legacy = Generate(forceLegacy: true, seed, ref legacyState, timestamp, previousPreset: null);

                var nativeState = ForcedState(preset);
                var native = Generate(forceLegacy: false, seed, ref nativeState, timestamp, previousPreset: null);

                var what = $"{preset} {arm} seed={seed}";
                Assert.That(_jsonUtil.Serialize(native), Is.EqualTo(_jsonUtil.Serialize(legacy)), $"{what}: the generated weather differs");
                Assert.That(native.SptChosenPreset, Is.EqualTo(preset), $"{what}: the forced preset was not the one generated");
                Assert.That(StateOf(nativeState), Is.EqualTo(StateOf(legacyState)), $"{what}: the state left in the ref dict differs");
            }
        }
    }

    /// <summary>
    /// Four calls threading one <c>ref</c> dictionary and one <c>previousPreset</c> the way
    /// <c>RaidWeatherService</c>'s loop threads them - except that the loop pre-seeds the dictionary
    /// with <c>cloner.Clone(GetWeatherPresetWeightsBySeason(...))</c>, so its first call never
    /// refills, while this one starts empty and so exercises strictly more. Over the two-entry
    /// season table this fixture installs the sequence is deterministic and covers all four state
    /// transitions: call one refills an empty dictionary, call two decays the previous pick to
    /// zero, call three decays the second pick to zero as well and then draws an exhausted preset,
    /// which clears the state - and call four refills <em>and</em> decays a live
    /// <c>previousPreset</c> inside the same call, the combination the loop hits whenever its
    /// dictionary exhausts mid-forecast.
    ///
    /// Both arms are re-seeded before every call with that call's own seed - see the fixture note.
    /// </summary>
    [Test]
    public void AFourCallSequenceCarriesTheStateIdentically()
    {
        var weights = _weatherConfig.Weather.WeatherPresetWeight;
        var seasonKey = SequenceSeason.ToString();
        var seasonPresent = weights.TryGetValue(seasonKey, out var originalSeason);

        // Two equal weights: the first pick takes the uniform shortcut, the second takes the
        // cumulative scan once the decay unbalances it, and the third finds every weight at zero
        weights[seasonKey] = new Dictionary<WeatherPreset, double> { { WeatherPreset.SUNNY, 1 }, { WeatherPreset.RAINY, 1 } };

        try
        {
            var legacy = RunSequence(forceLegacy: true);
            var native = RunSequence(forceLegacy: false);

            for (var call = 0; call < legacy.Results.Count; call++)
            {
                Assert.That(
                    _jsonUtil.Serialize(native.Results[call]),
                    Is.EqualTo(_jsonUtil.Serialize(legacy.Results[call])),
                    $"call {call + 1}: the generated weather differs"
                );
            }

            Assert.That(
                StateOf(native.FinalState),
                Is.EqualTo(StateOf(legacy.FinalState)),
                "the final ref-dict contents or their order differ"
            );

            // The applier contract: legacy replaced the reference on refill and mutated in place
            // otherwise, and the native arm has to do both the same way
            Assert.That(native.Replaced, Is.EqualTo(legacy.Replaced), "the two arms disagree on which calls replaced the ref dict");
            Assert.That(
                legacy.Replaced,
                Is.EqualTo(new[] { true, false, false, true }),
                "the sequence did not refill on exactly its first and fourth calls"
            );

            // Call three drew a preset whose weight the decay had taken to zero, which is why call
            // four found the dictionary empty and refilled - and then decayed its own previous pick
            // out of the fresh table, inside that same call
            Assert.That(legacy.FinalState.Count, Is.EqualTo(2), "the fourth call did not leave a refilled table");
            Assert.That(
                legacy.FinalState.Values,
                Has.Exactly(1).EqualTo(0d),
                "the refill did not decay the preset call three had just picked"
            );
        }
        finally
        {
            if (seasonPresent)
            {
                weights[seasonKey] = originalSeason!;
            }
            else
            {
                weights.Remove(seasonKey);
            }
        }
    }

    /// <summary>
    /// The wire case with no other owner: a <c>previousPreset</c> the state does not hold decays
    /// nothing on either arm - legacy's <c>ContainsKey</c> guard, and the native
    /// <c>get_mut</c> that finds nothing.
    /// </summary>
    [Test]
    public void APreviousPresetOutsideTheStateDecaysNothingOnBothArms()
    {
        const ulong Seed = 771;

        var legacyState = ForcedState(WeatherPreset.SUNNY);
        var legacy = Generate(forceLegacy: true, Seed, ref legacyState, DayTimestamp, WeatherPreset.RAINY);

        var nativeState = ForcedState(WeatherPreset.SUNNY);
        var native = Generate(forceLegacy: false, Seed, ref nativeState, DayTimestamp, WeatherPreset.RAINY);

        Assert.That(_jsonUtil.Serialize(native), Is.EqualTo(_jsonUtil.Serialize(legacy)), "the generated weather differs");
        Assert.That(StateOf(nativeState), Is.EqualTo(StateOf(legacyState)), "the state left in the ref dict differs");
        Assert.That(legacyState[WeatherPreset.SUNNY], Is.EqualTo(5d), "an absent previous preset still decayed the state");
    }

    /// <summary>
    /// The shape <c>client/weather</c> actually takes and no other case covers: <c>WeatherController</c>
    /// passes no timestamp at all. The native arm hands <c>SetCurrentDateTime</c> the ORIGINAL
    /// nullable argument rather than the resolved one, because the resolved value takes the
    /// <c>GetDateTimeFromTimeStamp</c> branch, whose <c>Kind=Unspecified</c> result
    /// <c>FormatToBsgDate</c> then re-reads as host-local - so on a non-UTC host the two arms' date
    /// strings can part company. Only a null-timestamp call can see it: given a timestamp, the
    /// resolved and original values are the same <c>long</c>.
    ///
    /// <c>Timestamp</c> and the draws are deliberately not compared - the two calls read the wall
    /// clock, so only the two strings under test are comparable at all.
    /// </summary>
    [Test]
    public void ANullTimestampAgreesOnTheDateAndTimeStringsOnBothArms()
    {
        const ulong Seed = 4041;

        var legacyState = ForcedState(WeatherPreset.SUNNY);
        var legacy = Generate(forceLegacy: true, Seed, ref legacyState, timestamp: null, previousPreset: null);

        var nativeState = ForcedState(WeatherPreset.SUNNY);
        var native = Generate(forceLegacy: false, Seed, ref nativeState, timestamp: null, previousPreset: null);

        Assert.That(native.Date, Is.EqualTo(legacy.Date), "the two arms disagree on the BSG date");

        // Not equality: the two calls are milliseconds apart, GetTimeStamp has one-second
        // resolution and the in-raid clock runs at the config's acceleration, so the honest bound
        // is a few seconds. A date/time the resolved-timestamp branch shifted is out by whole
        // hours - or, once FormatToBsgDate has reduced it to a date, by a whole day
        Assert.That(
            (BsgDateTime(native.Time!) - BsgDateTime(legacy.Time!)).Duration(),
            Is.LessThan(TimeSpan.FromMinutes(1)),
            "the two arms' date/time strings differ by more than the clock could have moved between the calls"
        );
    }

    /// <summary>
    /// The other unowned wire case, C#-side: a preset resolvable neither by its own name nor
    /// through <c>["default"]</c> crosses as a <em>null block</em> rather than throwing in the
    /// builder. Legacy's indexer only throws when that preset is the one chosen, and the native
    /// side mirrors it there (its own absent-block test).
    /// </summary>
    [Test]
    public void AnUnresolvablePresetCrossesAsANullBlock()
    {
        var blocks = _weatherConfig.Weather.PresetWeights!;
        var key = WeatherPreset.RAINY.ToString();
        var original = blocks[key];

        Assert.That(
            blocks.ContainsKey("default"),
            Is.False,
            "the shipped preset blocks grew a default key, so this case no longer tests anything"
        );

        blocks.Remove(key);
        try
        {
            var request = _requestBuilder.BuildGenerateWeatherRequest(
                new Dictionary<WeatherPreset, double> { { WeatherPreset.SUNNY, 5 } },
                previousPreset: null,
                new Dictionary<WeatherPreset, double>(),
                isNight: false,
                testSeed: null
            );

            var rainy = request.PresetBlocks.Single(entry => entry.Preset == (int)WeatherPreset.RAINY);
            Assert.That(rainy.Block, Is.Null, "an unresolvable preset did not cross as a null block");

            var sunny = request.PresetBlocks.Single(entry => entry.Preset == (int)WeatherPreset.SUNNY);
            Assert.That(sunny.Block, Is.Not.Null, "a resolvable preset lost its block");
        }
        finally
        {
            blocks[key] = original;
        }
    }

    /// <summary>
    /// The same missing block, but chosen - end to end through <c>GenerateWeather</c> rather than
    /// stopping at the builder. Legacy's <c>["default"]</c> indexer throws
    /// <see cref="KeyNotFoundException"/>, and the native pass - which cannot throw a C# exception -
    /// reports the same point as a message that crosses in the failure envelope and surfaces as
    /// <see cref="InvalidOperationException"/>. One test, because it is also the only exercise the
    /// error envelope gets on this export.
    ///
    /// Every preset key involved is a defined enum member, so the out-of-enum decline cannot fire
    /// and the native call really is the native arm's.
    /// </summary>
    [Test]
    public void AnUnresolvableChosenPresetThrowsOnBothArms()
    {
        var blocks = _weatherConfig.Weather.PresetWeights!;
        var key = WeatherPreset.RAINY.ToString();
        var original = blocks[key];

        Assert.That(
            blocks.ContainsKey("default"),
            Is.False,
            "the shipped preset blocks grew a default key, so this case no longer tests anything"
        );

        blocks.Remove(key);
        try
        {
            // One entry, so RAINY is the pick on either arm without a draw
            var legacyState = ForcedState(WeatherPreset.RAINY);
            _weatherConfig.ForceLegacyWeatherGeneration = true;
            Assert.That(
                () => _generator.GenerateWeather(SequenceSeason, ref legacyState, DayTimestamp, null),
                Throws.TypeOf<KeyNotFoundException>()
            );
            Assert.That(_generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy), "the throwing legacy call took the wrong path");

            var nativeState = ForcedState(WeatherPreset.RAINY);
            _weatherConfig.ForceLegacyWeatherGeneration = false;
            Assert.That(
                () => _generator.GenerateWeather(SequenceSeason, ref nativeState, DayTimestamp, null),
                Throws
                    .TypeOf<InvalidOperationException>()
                    .With.Message.Contains($"no preset weights for chosen preset {(int)WeatherPreset.RAINY}"),
                "the native failure did not carry the native message"
            );
            Assert.That(_generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native), "the throwing native call took the wrong path");

            // The write-back runs after the crossing, so a pass that failed leaves the caller's own
            // dictionary exactly as it found it
            Assert.That(
                StateOf(nativeState),
                Is.EqualTo(StateOf(ForcedState(WeatherPreset.RAINY))),
                "the failed native call mutated the caller's state"
            );
        }
        finally
        {
            blocks[key] = original;
        }
    }

    /// <summary>
    /// One four-call sequence on one arm, threading the dictionary and the previous preset as
    /// <c>RaidWeatherService.GenerateFutureWeatherAndCache</c> threads them - bar its pre-seed, see
    /// the case above - and recording which calls replaced the dictionary instance.
    /// </summary>
    private Sequence RunSequence(bool forceLegacy)
    {
        var state = new Dictionary<WeatherPreset, double>();
        var results = new List<SPTarkov.Server.Core.Models.Eft.Weather.Weather>();
        var replaced = new List<bool>();
        WeatherPreset? previousPreset = null;

        foreach (var seed in new ulong[] { 101, 202, 303, 404 })
        {
            var before = state;

            results.Add(Generate(forceLegacy, seed, ref state, DayTimestamp, previousPreset));

            replaced.Add(!ReferenceEquals(before, state));
            previousPreset = results[^1].SptChosenPreset;
        }

        return new Sequence(results, state, replaced);
    }

    /// <summary>
    /// One generation on the named arm, with both arms re-seeded first and the path taken asserted
    /// before the result is used for anything.
    /// </summary>
    private SPTarkov.Server.Core.Models.Eft.Weather.Weather Generate(
        bool forceLegacy,
        ulong seed,
        ref Dictionary<WeatherPreset, double> presetWeights,
        long? timestamp,
        WeatherPreset? previousPreset
    )
    {
        _weatherConfig.ForceLegacyWeatherGeneration = forceLegacy;

        // Both, every call: the native stream restarts per call and the legacy one would not
        _randomUtil.RandomSource = new SeededRandomSource(seed);
        _generator.NativeTestSeed = seed;

        var result = _generator.GenerateWeather(SequenceSeason, ref presetWeights, timestamp, previousPreset);

        // Fail fast on silent fallback before comparing anything
        Assert.That(
            _generator.LastPathTaken,
            Is.EqualTo(forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native),
            $"forceLegacy={forceLegacy} seed={seed} took the wrong path"
        );

        return result;
    }

    /// <summary>
    /// A state holding one preset at a weight the exhaustion check will not trip: the pick is
    /// forced and, at one entry, costs no draw on either arm.
    /// </summary>
    private static Dictionary<WeatherPreset, double> ForcedState(WeatherPreset preset)
    {
        return new Dictionary<WeatherPreset, double> { { preset, 5 } };
    }

    /// <summary>
    /// The dictionary as an ordered pair list: the pick walks it in order, so order is part of what
    /// parity means here.
    /// </summary>
    private static List<KeyValuePair<WeatherPreset, double>> StateOf(Dictionary<WeatherPreset, double> state)
    {
        return [.. state];
    }

    /// <summary>
    /// <c>Weather.Time</c> is <c>"{FormatToBsgDate} {GetBsgFormattedWeatherTime}"</c> - the real UTC
    /// date and the in-raid time - so one parse covers both halves of it.
    /// </summary>
    private static DateTime BsgDateTime(string time)
    {
        return DateTime.ParseExact(time, "yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture);
    }

    private sealed record Sequence(
        List<SPTarkov.Server.Core.Models.Eft.Weather.Weather> Results,
        Dictionary<WeatherPreset, double> FinalState,
        List<bool> Replaced
    );
}
