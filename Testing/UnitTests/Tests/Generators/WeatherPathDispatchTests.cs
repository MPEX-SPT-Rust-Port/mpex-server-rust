using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weather;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Weather;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the dual-path dispatch for weather generation: native by default, the retained C# bodies
/// when <see cref="WeatherConfig.ForceLegacyWeatherGeneration"/> is set, when the frozen constructor
/// built the instance, when a mod substituted the generator, when the injected
/// <see cref="IWeatherPreset"/> set is not exactly the three built-ins (spec D8), when a crossing
/// weight table carries a <see cref="WeatherPreset"/> key outside the enum, or when any of the
/// seventeen frozen members carries a live Harmony patch (spec D7).
///
/// The seventeen span five classes, because the native arm reimplements all five bodies: the five
/// <see cref="WeatherGenerator"/> members below the dispatcher, <c>Generate</c> and
/// <c>CanHandle</c> on each concrete preset, and <see cref="AbstractWeatherPreset"/>'s six draw
/// helpers. <c>GenerateWeather</c> itself is excluded - it is the dispatcher, and a patch there
/// wraps whichever path runs.
///
/// Harmony patches are process-wide, so every patch is removed in a finally and the fixture never
/// runs in parallel. The force flag on the shared config singleton is restored per case.
/// </summary>
[TestFixture]
[NonParallelizable]
public class WeatherPathDispatchTests
{
    /// <summary>
    /// The season every case generates for. Not a shipped <c>weatherPresetWeight</c> key, so the
    /// refill case adds and removes its entry rather than replacing one.
    /// </summary>
    private const Season FixtureSeason = Season.SUMMER;

    /// <summary>
    /// A <see cref="WeatherPreset"/> key outside the enum, which <c>EftEnumConverter</c> will parse
    /// out of an undefined numeric key in a config-edited <c>weather.json</c>.
    /// </summary>
    private const WeatherPreset OutOfEnumPreset = (WeatherPreset)4;

    private static bool _prefixFired;
    private static bool _postfixFired;
    private static bool _patchFired;

    private WeatherGenerator _generator = default!;
    private WeatherConfig _weatherConfig = default!;
    private WeatherNativeRequestBuilder _requestBuilder = default!;
    private bool _originalForceLegacy;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _generator = di.GetService<WeatherGenerator>();
        _weatherConfig = di.GetService<WeatherConfig>();
        _requestBuilder = di.GetService<WeatherNativeRequestBuilder>();

        _originalForceLegacy = _weatherConfig.ForceLegacyWeatherGeneration;
    }

    [TearDown]
    public void TearDown()
    {
        // The captured value, not false: a tree shipping the flag on would otherwise have it
        // silently flipped for every fixture that runs after this one
        _weatherConfig.ForceLegacyWeatherGeneration = _originalForceLegacy;
    }

    /// <summary>
    /// The negative control: a stock generator with no force flag and no patches takes the native
    /// path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        AssertPath(_generator, LootGenerationPath.Native, "stock");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _weatherConfig.ForceLegacyWeatherGeneration = true;

        AssertPath(_generator, LootGenerationPath.Legacy, "force flag");
    }

    /// <summary>
    /// A mod compiled against the frozen contract can construct the generator itself, and the frozen
    /// constructor has no native seam wired - such an instance has to run the C# bodies it was built
    /// for.
    /// </summary>
    [Test]
    public void TheFrozenConstructorRoutesToTheLegacyPath()
    {
        AssertPath((WeatherGenerator)Construct(typeof(WeatherGenerator), narrowest: true), LootGenerationPath.Legacy, "frozen constructor");
    }

    /// <summary>
    /// The negative control for the cases either side of it: hand-building the generator off the
    /// container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltGeneratorWithStockServicesTakesTheNativePath()
    {
        AssertPath((WeatherGenerator)Construct(typeof(WeatherGenerator)), LootGenerationPath.Native, "hand-built");
    }

    /// <summary>
    /// A mod registering its own generator with a higher TypePriority hands the container a
    /// subclass, whose overrides only the C# path can run.
    /// </summary>
    [Test]
    public void AReplacedGeneratorRoutesToTheLegacyPath()
    {
        AssertPath((WeatherGenerator)Construct(typeof(TestWeatherGeneratorSubclass)), LootGenerationPath.Legacy, "replaced generator");
    }

    /// <summary>
    /// Spec D8, first variant: a mod registering an extra <see cref="IWeatherPreset"/> makes the
    /// resolved set larger than the three the native arm knows, so its <c>CanHandle</c> has to get
    /// a real vote - which only the legacy path gives it.
    /// </summary>
    [Test]
    public void AnExtraWeatherPresetRoutesToTheLegacyPath()
    {
        AssertPath(BuildWith([.. BuiltInPresets(), new TestExtraPreset()]), LootGenerationPath.Legacy, "an extra preset");
    }

    /// <summary>
    /// Spec D8, second variant: a mod that removed one of the built-ins. The forced preset is
    /// SUNNY, which the shortened set still handles, so the legacy path it falls back to runs for
    /// real.
    /// </summary>
    [Test]
    public void AMissingWeatherPresetRoutesToTheLegacyPath()
    {
        var presets = BuiltInPresets().Where(preset => preset is not RainyPreset).ToList();

        Assert.That(presets, Has.Count.EqualTo(2), "the built-in preset set is not what this case assumed");

        AssertPath(BuildWith(presets), LootGenerationPath.Legacy, "a missing preset");
    }

    /// <summary>
    /// Spec D8, third variant: the set is still three strategies, but one is a mod's subclass of a
    /// built-in - a substitution the type check catches even though the count does not.
    /// </summary>
    [Test]
    public void ASubstitutedWeatherPresetRoutesToTheLegacyPath()
    {
        var di = DI.GetInstance();
        var presets = BuiltInPresets().Where(preset => preset is not SunnyPreset).ToList();
        presets.Insert(0, new TestSunnyPresetSubclass(di.GetService<WeightedRandomHelper>(), di.GetService<RandomUtil>()));

        AssertPath(BuildWith(presets), LootGenerationPath.Legacy, "a substituted preset");
    }

    /// <summary>
    /// The out-of-enum hole, caller-state half: <c>EftEnumConverter</c> parses an undefined numeric
    /// key in a config-edited <c>weather.json</c> into a <see cref="WeatherPreset"/> the enum does
    /// not define, and the native arm mints one preset block per <em>defined</em> member - so a
    /// draw landing on such a key would find no block and error, where legacy warns and falls back
    /// to the sunny generator. The state carrying one therefore has to run legacy.
    /// </summary>
    [Test]
    public void AnOutOfEnumPresetInTheCallerStateRoutesToTheLegacyPath()
    {
        // SUNNY first and holding the whole weight: its cumulative weight already covers the entire
        // [0, sum] draw, so the out-of-enum entry can never be picked whichever arm runs - which
        // matters because legacy's fallback would then throw looking for its (absent) preset block
        var state = new Dictionary<WeatherPreset, double> { { WeatherPreset.SUNNY, 5 }, { OutOfEnumPreset, 0 } };

        AssertPath(_generator, LootGenerationPath.Legacy, "an out-of-enum preset in the caller state", state);
    }

    /// <summary>
    /// The same hole on the other table the request carries unfiltered: the season's refill
    /// weights, which an empty caller state makes the generator draw from.
    /// </summary>
    [Test]
    public void AnOutOfEnumPresetInTheSeasonRefillTableRoutesToTheLegacyPath()
    {
        var weights = _weatherConfig.Weather.WeatherPresetWeight;
        var seasonKey = FixtureSeason.ToString();
        var seasonPresent = weights.TryGetValue(seasonKey, out var originalSeason);

        // SUNNY first and holding the whole weight, for the reason the caller-state case gives
        weights[seasonKey] = new Dictionary<WeatherPreset, double> { { WeatherPreset.SUNNY, 5 }, { OutOfEnumPreset, 0 } };

        try
        {
            AssertPath(
                _generator,
                LootGenerationPath.Legacy,
                "an out-of-enum preset in the season refill table",
                new Dictionary<WeatherPreset, double>()
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
    /// Spec D7: a live patch on any of the seventeen members the native arm reimplements routes the
    /// generator back to C#, because those bodies are the only thing the patch can hook.
    /// </summary>
    [TestCaseSource(nameof(FrozenMembers))]
    public void AHarmonyPatchOnAFrozenMemberForcesTheLegacyPath(MethodInfo member)
    {
        var harmony = new Harmony($"unit-tests.weather-path-dispatch.{member.DeclaringType!.Name}.{member.Name}");

        try
        {
            harmony.Patch(member, postfix: new HarmonyMethod(typeof(WeatherPathDispatchTests), nameof(PatchFired)));

            Assert.That(
                Harmony.GetPatchInfo(member)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {member.Name} was not registered"
            );

            AssertPath(_generator, LootGenerationPath.Legacy, $"a patch on {member.DeclaringType.Name}.{member.Name}");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The frozen set is a hand-written list in the builder; this recomputes it off the four types'
    /// own surfaces, so a member added to any of them without being added to the set fails loudly
    /// rather than silently going unhookable.
    /// </summary>
    [Test]
    public void TheFrozenSetIsTheFourTypesSurfacesMinusTheDispatcher()
    {
        var members =
            (List<MethodBase>)
                typeof(WeatherNativeRequestBuilder)
                    .GetField("_frozenMembers", BindingFlags.Static | BindingFlags.NonPublic)!
                    .GetValue(null)!;

        Assert.That(members, Is.EquivalentTo(FrozenSurface()), "the frozen set is not the weather surface minus the dispatcher");
        Assert.That(members, Has.Count.EqualTo(17), "spec D7's set is seventeen members");
    }

    /// <summary>
    /// The dispatcher rule: <c>GenerateWeather</c> is the entry point, so a patch on it wraps the
    /// native body rather than forcing a fall back, and the mod's hooks still see every call.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnGenerateWeatherWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.weather-path-dispatch.GenerateWeather");
        var target = Member(typeof(WeatherGenerator), nameof(WeatherGenerator.GenerateWeather));

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(WeatherPathDispatchTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(WeatherPathDispatchTests), nameof(Postfix))
            );

            AssertPath(_generator, LootGenerationPath.Native, "a patch on GenerateWeather");

            Assert.That(_prefixFired, Is.True, "prefix on GenerateWeather never ran");
            Assert.That(_postfixFired, Is.True, "postfix on GenerateWeather never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// Hook liveness for the collaborator the native arm still calls for itself: the dispatcher
    /// derives <c>isNight</c> through <see cref="WeatherHelper.IsHourAtNightTime"/> at legacy's own
    /// expression, so a patch on it is live without costing the port its native path.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnIsHourAtNightTimeFiresOnTheNativeArm()
    {
        var harmony = new Harmony("unit-tests.weather-path-dispatch.IsHourAtNightTime");
        var target = Member(typeof(WeatherHelper), nameof(WeatherHelper.IsHourAtNightTime));

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(WeatherPathDispatchTests), nameof(PatchFired)));

            AssertPath(_generator, LootGenerationPath.Native, "a patch on IsHourAtNightTime");

            Assert.That(_patchFired, Is.True, "postfix on IsHourAtNightTime never ran on the native arm");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    private void AssertPath(
        WeatherGenerator generator,
        LootGenerationPath expected,
        string what,
        Dictionary<WeatherPreset, double>? state = null
    )
    {
        // One entry, so the pick is forced without a draw and every preset set this fixture builds
        // has something that handles it
        var presetWeights = state ?? new Dictionary<WeatherPreset, double> { { WeatherPreset.SUNNY, 5 } };

        generator.GenerateWeather(FixtureSeason, ref presetWeights);

        Assert.That(generator.LastPathTaken, Is.EqualTo(expected), $"{what}: GenerateWeather took the wrong path");
    }

    /// <summary>
    /// One generator off the container's own services, with the preset set swapped for the one a
    /// case wants to test.
    /// </summary>
    private WeatherGenerator BuildWith(IEnumerable<IWeatherPreset> presets)
    {
        var di = DI.GetInstance();

        return new WeatherGenerator(
            di.GetService<ISptLogger<WeatherGenerator>>(),
            di.GetService<TimeUtil>(),
            di.GetService<WeatherHelper>(),
            _weatherConfig,
            di.GetService<WeightedRandomHelper>(),
            di.GetService<RandomUtil>(),
            presets,
            di.GetService<ICloner>(),
            _requestBuilder
        );
    }

    private static List<IWeatherPreset> BuiltInPresets()
    {
        return [.. (IEnumerable<IWeatherPreset>)DI.GetInstance().GetService(typeof(IEnumerable<IWeatherPreset>))];
    }

    /// <summary>
    /// One generator built by hand off the container's own services, on either the frozen
    /// constructor or the additive one the container picks.
    /// </summary>
    private static object Construct(Type type, bool narrowest = false)
    {
        var constructors = type.GetConstructors();
        var constructor = narrowest
            ? constructors.MinBy(candidate => candidate.GetParameters().Length)!
            : constructors.MaxBy(candidate => candidate.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => DI.GetInstance().GetService(parameter.ParameterType)).ToArray();

        return constructor.Invoke(arguments);
    }

    private static IEnumerable<TestCaseData> FrozenMembers()
    {
        return FrozenSurface().Select(member => new TestCaseData(member).SetArgDisplayNames($"{member.DeclaringType!.Name}.{member.Name}"));
    }

    /// <summary>
    /// The seventeen members a mod can patch to take over part of weather generation, recomputed
    /// independently of the builder's own list: the generator's own surface minus the dispatcher,
    /// each concrete preset's two overrides, and the abstract base's six draw helpers (its two
    /// abstract declarations excluded - there is no body there to hook).
    /// </summary>
    private static List<MethodInfo> FrozenSurface()
    {
        return
        [
            .. DeclaredSurface(typeof(WeatherGenerator)).Where(method => method.Name != nameof(WeatherGenerator.GenerateWeather)),
            .. DeclaredSurface(typeof(SunnyPreset)),
            .. DeclaredSurface(typeof(CloudyPreset)),
            .. DeclaredSurface(typeof(RainyPreset)),
            .. DeclaredSurface(typeof(AbstractWeatherPreset)),
        ];
    }

    /// <summary>
    /// A type's own hookable instance surface: declared, public or protected, not a property
    /// accessor, and not an abstract declaration.
    /// </summary>
    private static IEnumerable<MethodInfo> DeclaredSurface(Type type)
    {
        return type.GetMethods(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly)
            .Where(method => !method.IsSpecialName && !method.IsAbstract)
            .Where(method => method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly);
    }

    private static MethodInfo Member(Type declaringType, string name)
    {
        return declaringType.GetMethod(
                name,
                BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly
            ) ?? throw new InvalidOperationException($"{declaringType.Name}.{name} is not declared any more");
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
    /// Stands in for a mod-registered generator: identical behaviour, different type. Chains the
    /// widest base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestWeatherGeneratorSubclass(
        ISptLogger<WeatherGenerator> logger,
        TimeUtil timeUtil,
        WeatherHelper weatherHelper,
        WeatherConfig weatherConfig,
        WeightedRandomHelper weightedRandomHelper,
        RandomUtil randomUtil,
        IEnumerable<IWeatherPreset> weatherGenerators,
        ICloner cloner,
        WeatherNativeRequestBuilder requestBuilder
    )
        : WeatherGenerator(
            logger,
            timeUtil,
            weatherHelper,
            weatherConfig,
            weightedRandomHelper,
            randomUtil,
            weatherGenerators,
            cloner,
            requestBuilder
        ) { }

    /// <summary>
    /// Stands in for a mod-registered preset type: it handles nothing, so the built-ins still
    /// generate - only the size of the resolved set changes.
    /// </summary>
    private sealed class TestExtraPreset : IWeatherPreset
    {
        public bool CanHandle(WeatherPreset preset)
        {
            return false;
        }

        public SPTarkov.Server.Core.Models.Eft.Weather.Weather Generate(PresetWeights weatherWeights)
        {
            throw new NotSupportedException("the extra preset handles nothing, so nothing may ask it to generate");
        }
    }

    /// <summary>
    /// Stands in for a mod replacing a built-in with its own subclass: same behaviour, different
    /// concrete type, so only the type-set check can catch it.
    /// </summary>
    private sealed class TestSunnyPresetSubclass(WeightedRandomHelper weightedRandomHelper, RandomUtil randomUtil)
        : SunnyPreset(weightedRandomHelper, randomUtil) { }
}
