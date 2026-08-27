using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weather;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Weather;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Golden parity gate on the weather port: at one seed, with one explicit timestamp, the legacy C#
/// path and the spt-native pass must produce the same <c>Weather</c> - every drawn member, the
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
    /// Three calls threading one <c>ref</c> dictionary and one <c>previousPreset</c>, the way
    /// <c>RaidWeatherService</c>'s loop does. Over the two-entry season table this fixture installs
    /// the sequence is deterministic and covers all three state transitions: call one refills an
    /// empty dictionary, call two decays the previous pick to zero, and call three decays the second
    /// pick to zero as well and then draws an exhausted preset, which clears the state.
    ///
    /// Both arms are re-seeded before every call with that call's own seed - see the fixture note.
    /// </summary>
    [Test]
    public void AThreeCallSequenceCarriesTheStateIdentically()
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
                Is.EqualTo(new[] { true, false, false }),
                "the sequence did not refill exactly once, on its first call"
            );

            // Call three drew a preset whose weight the decay had taken to zero
            Assert.That(legacy.FinalState, Is.Empty, "the exhausted pick did not clear the state");
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
    /// One three-call sequence on one arm, threading the dictionary and the previous preset exactly
    /// as <c>RaidWeatherService.GenerateFutureWeatherAndCache</c> does, and recording which calls
    /// replaced the dictionary instance.
    /// </summary>
    private Sequence RunSequence(bool forceLegacy)
    {
        var state = new Dictionary<WeatherPreset, double>();
        var results = new List<SPTarkov.Server.Core.Models.Eft.Weather.Weather>();
        var replaced = new List<bool>();
        WeatherPreset? previousPreset = null;

        foreach (var seed in new ulong[] { 101, 202, 303 })
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

    private sealed record Sequence(
        List<SPTarkov.Server.Core.Models.Eft.Weather.Weather> Results,
        Dictionary<WeatherPreset, double> FinalState,
        List<bool> Replaced
    );
}
