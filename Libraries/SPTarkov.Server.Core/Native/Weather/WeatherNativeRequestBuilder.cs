using System.Reflection;
using HarmonyLib;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Generators.Weather;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;

namespace SPTarkov.Server.Core.Native.Weather;

/// <summary>
/// Assembles the weather request out of the live config - everything
/// <c>WeatherGenerator.GenerateWeather</c> and the three <c>IWeatherPreset</c> strategies would
/// have read for themselves - and sends it.
///
/// It also owns the port's frozen member set: <see cref="AnyFrozenMemberPatched"/> is consulted by
/// <c>WeatherGenerator</c>'s legacy-path predicate, so a Harmony patch on any one of the seventeen
/// forces the whole generation back to C#.
/// </summary>
[Injectable]
public class WeatherNativeRequestBuilder(WeatherConfig weatherConfig)
{
    /// <summary>
    ///     The seventeen members a mod can Harmony-patch to take over part of weather generation -
    ///     one entry for every body the native pass reimplements, across all four classes it
    ///     collapsed into match arms.
    ///
    ///     On <c>WeatherGenerator</c>: the preset-weight lookups, the by-preset generation, the
    ///     temperature draw and the date/time tail. On each concrete preset: its <c>Generate</c>
    ///     and <c>CanHandle</c> overrides, which are declared per class - without them a patch on
    ///     (say) <c>SunnyPreset.Generate</c> would leave the type-set check happy and the native arm
    ///     would silently bypass the hook. On <c>AbstractWeatherPreset</c>: the six draw helpers the
    ///     preset arms are made of.
    ///
    ///     Excluded is <c>GenerateWeather</c>, the dispatcher - a patch there wraps whichever path
    ///     runs.
    /// </summary>
    private static readonly List<MethodBase> _frozenMembers =
    [
        FrozenMember(typeof(WeatherGenerator), "GetWeatherPresetWeightsBySeason"),
        FrozenMember(typeof(WeatherGenerator), "GenerateWeatherByPreset"),
        FrozenMember(typeof(WeatherGenerator), "GetWeatherWeightsByPreset"),
        FrozenMember(typeof(WeatherGenerator), "GetRaidTemperature"),
        FrozenMember(typeof(WeatherGenerator), "SetCurrentDateTime"),
        FrozenMember(typeof(SunnyPreset), "Generate"),
        FrozenMember(typeof(SunnyPreset), "CanHandle"),
        FrozenMember(typeof(CloudyPreset), "Generate"),
        FrozenMember(typeof(CloudyPreset), "CanHandle"),
        FrozenMember(typeof(RainyPreset), "Generate"),
        FrozenMember(typeof(RainyPreset), "CanHandle"),
        FrozenMember(typeof(AbstractWeatherPreset), "GetWeightedWindDirection"),
        FrozenMember(typeof(AbstractWeatherPreset), "GetWeightedClouds"),
        FrozenMember(typeof(AbstractWeatherPreset), "GetWeightedWindSpeed"),
        FrozenMember(typeof(AbstractWeatherPreset), "GetWeightedFog"),
        FrozenMember(typeof(AbstractWeatherPreset), "GetWeightedRain"),
        FrozenMember(typeof(AbstractWeatherPreset), "GetRandomDouble"),
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
    ///     One weather pass's inputs: the caller's state, the season's refill table, one resolved
    ///     block per preset and the day/night flag, all resolved before the crossing.
    /// </summary>
    /// <param name="presetWeights">The caller's ref state, in enumeration order</param>
    /// <param name="previousPreset">What the previous call chose, if anything</param>
    /// <param name="refillWeights">The season's preset weights; null when the config resolved neither the season nor "default"</param>
    /// <param name="isNight">Whether the raid hour is a night hour</param>
    /// <param name="testSeed">Test-only seed for the native RNG stream</param>
    public GenerateWeatherRequest BuildGenerateWeatherRequest(
        Dictionary<WeatherPreset, double> presetWeights,
        WeatherPreset? previousPreset,
        Dictionary<WeatherPreset, double>? refillWeights,
        bool isNight,
        ulong? testSeed
    )
    {
        return new GenerateWeatherRequest
        {
            PresetWeights = ToEntries(presetWeights),
            PreviousPreset = previousPreset.HasValue ? (int)previousPreset.Value : null,
            // A mod config with neither the season key nor "default" hands legacy a null here and
            // NREs on the refill; an empty list errors natively on the same call
            RefillWeights = ToEntries(refillWeights),
            PresetBlocks = BuildPresetBlocks(),
            IsNight = isNight,
            TestSeed = testSeed,
        };
    }

    /// <summary>
    ///     Generates one weather object natively.
    /// </summary>
    /// <exception cref="InvalidOperationException">The pass failed, or the native side misbehaved.</exception>
    public GenerateWeatherResponse SendGenerateWeather(GenerateWeatherRequest request)
    {
        return SptNative.GenerateWeather(request);
    }

    /// <summary>
    ///     One block per <c>WeatherPreset</c> enum member, each resolved through legacy's own
    ///     fallback - the preset's own key, else <c>"default"</c> - but with <c>TryGetValue</c>
    ///     rather than legacy's indexer: an unresolvable preset crosses as a null block instead of
    ///     throwing here, because legacy only throws when that preset is the one chosen.
    /// </summary>
    private List<PresetBlockEntry> BuildPresetBlocks()
    {
        var blocks = weatherConfig.Weather.PresetWeights;

        return
        [
            .. Enum.GetValues<WeatherPreset>()
                .Select(preset => new PresetBlockEntry { Preset = (int)preset, Block = ToWire(Resolve(blocks, preset)) }),
        ];
    }

    /// <summary>
    ///     Legacy's <c>GetWeatherWeightsByPreset</c> lookup, made non-throwing.
    /// </summary>
    private static PresetWeights? Resolve(Dictionary<string, PresetWeights>? blocks, WeatherPreset preset)
    {
        if (blocks is null)
        {
            return null;
        }

        return blocks.TryGetValue(preset.ToString(), out var block) ? block : blocks.GetValueOrDefault("default");
    }

    /// <summary>
    ///     Member-wise null tolerant on purpose: every member but <c>Clouds</c> is nullable in the
    ///     model, and an eager projection would NRE on a partially-null block that legacy - which
    ///     dereferences only the chosen arm's members - runs clean.
    /// </summary>
    private static PresetWeightsWire? ToWire(PresetWeights? block)
    {
        if (block is null)
        {
            return null;
        }

        return new PresetWeightsWire
        {
            Clouds = ToWeighted(block.Clouds),
            WindSpeed = ToWeighted(block.WindSpeed),
            WindDirection = ToDirections(block.WindDirection),
            WindGustiness = ToWire(block.WindGustiness),
            Rain = ToWeighted(block.Rain),
            RainIntensity = ToWire(block.RainIntensity),
            Fog = ToWeighted(block.Fog),
            TempDay = ToWire(block.Temp?.Day),
            TempNight = ToWire(block.Temp?.Night),
            Pressure = ToWire(block.Pressure),
        };
    }

    private static List<WeightedEntryWire>? ToWeighted(Dictionary<string, double>? values)
    {
        return values?.Select(kvp => new WeightedEntryWire { Value = kvp.Key, Weight = kvp.Value }).ToList();
    }

    private static List<DirectionEntryWire>? ToDirections(Dictionary<Models.Enums.WindDirection, double>? values)
    {
        return values?.Select(kvp => new DirectionEntryWire { Direction = (int)kvp.Key, Weight = kvp.Value }).ToList();
    }

    private static MinMaxWire? ToWire(MinMax<double>? range)
    {
        return range is null ? null : new MinMaxWire { Min = range.Min, Max = range.Max };
    }

    private static List<PresetWeightEntry> ToEntries(Dictionary<WeatherPreset, double>? weights)
    {
        return weights is null ? [] : [.. weights.Select(kvp => new PresetWeightEntry { Preset = (int)kvp.Key, Weight = kvp.Value })];
    }

    /// <summary>
    ///     One frozen member, by name. A rename that the predicate did not follow would leave it
    ///     blind to patches on that member, so a miss fails loudly the first time the set is built.
    ///     <see cref="BindingFlags.Public"/> is in the mask because five of the seventeen are public.
    /// </summary>
    private static MethodBase FrozenMember(Type declaringType, string name)
    {
        return declaringType.GetMethod(
                name,
                BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly
            )
            ?? throw new InvalidOperationException(
                $"{declaringType.Name}.{name} is not declared any more, so the weather legacy-path predicate cannot see patches on it."
            );
    }
}
