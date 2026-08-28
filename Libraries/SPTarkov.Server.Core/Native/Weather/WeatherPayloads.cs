using System.Text.Json.Serialization;

namespace SPTarkov.Server.Core.Native.Weather;

/// <summary>
/// The request/response envelopes of <c>spt_generate_weather</c>, mirroring
/// <c>rust/spt-native/src/weather.rs</c> member for member. Conventions are
/// <c>Native/Raid/RaidPayloads.cs</c>'s: an explicit <see cref="JsonPropertyNameAttribute"/> on
/// every member, members Rust declares as <c>Option&lt;T&gt;</c> nullable and everything else
/// <c>required</c>.
///
/// Enum members cross as their numeric values - <c>WeatherPreset</c> (SUNNY=1, RAINY=2, CLOUDY=3)
/// and <c>WindDirection</c> alike - so no enum converter can move them, and string-keyed weight
/// tables cross as entry lists in enumeration order because the pick walks them in order.
/// </summary>
public record GenerateWeatherRequest
{
    /// <summary>
    ///     The caller's <c>ref</c> state, in enumeration order. Empty means refill.
    /// </summary>
    [JsonPropertyName("presetWeights")]
    public required List<PresetWeightEntry> PresetWeights { get; set; }

    /// <summary>
    ///     The preset the previous call chose, if any: present <em>and</em> in the state is what
    ///     legacy decays.
    /// </summary>
    [JsonPropertyName("previousPreset")]
    public int? PreviousPreset { get; set; }

    /// <summary>
    ///     <c>GetWeatherPresetWeightsBySeason(currentSeason)</c>, resolved unconditionally C#-side -
    ///     the season string never crosses. Empty when a mod config has neither the season key nor
    ///     <c>"default"</c>, which errors natively on the refill legacy NREs on.
    /// </summary>
    [JsonPropertyName("refillWeights")]
    public required List<PresetWeightEntry> RefillWeights { get; set; }

    /// <summary>
    ///     One entry per <c>WeatherPreset</c> enum member, each already resolved through legacy's
    ///     <c>["default"]</c> fallback. A null block is the unresolvable case.
    /// </summary>
    [JsonPropertyName("presetBlocks")]
    public required List<PresetBlockEntry> PresetBlocks { get; set; }

    /// <summary>
    ///     <c>weatherHelper.IsHourAtNightTime(...)</c>, resolved C#-side off the seconds-as-ticks
    ///     quirk at legacy's own expression, so a patch on the helper fires on this arm too.
    /// </summary>
    [JsonPropertyName("isNight")]
    public required bool IsNight { get; set; }

    /// <summary>
    ///     Test-only seed for the native RNG stream; null in production.
    /// </summary>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }
}

/// <summary>
/// One <c>Dictionary&lt;WeatherPreset, double&gt;</c> entry.
/// </summary>
public record PresetWeightEntry
{
    [JsonPropertyName("preset")]
    public required int Preset { get; set; }

    [JsonPropertyName("weight")]
    public required double Weight { get; set; }
}

/// <summary>
/// One preset's resolved <c>PresetWeights</c> block. <see cref="Block"/> is null when the config
/// resolved neither the preset's own key nor <c>"default"</c> - legacy's
/// <see cref="KeyNotFoundException"/> point, which only fires there if that preset is chosen.
/// </summary>
public record PresetBlockEntry
{
    [JsonPropertyName("preset")]
    public required int Preset { get; set; }

    [JsonPropertyName("block")]
    public PresetWeightsWire? Block { get; set; }
}

/// <summary>
/// <c>PresetWeights</c>, with <c>Temp</c> flattened into its two ranges.
///
/// Every member is optional (spec D10): all but <c>Clouds</c> are nullable in the config model and
/// legacy dereferences them lazily - chosen block only, and never <c>Rain</c>/<c>RainIntensity</c>
/// on the sunny and cloudy arms. An absent member crosses as absent and errors natively only at the
/// draw that needs it, which is where legacy NREs. Projecting eagerly would NRE the builder on a
/// partially-null <em>unchosen</em> block that legacy runs clean.
/// </summary>
public record PresetWeightsWire
{
    [JsonPropertyName("clouds")]
    public List<WeightedEntryWire>? Clouds { get; set; }

    [JsonPropertyName("windSpeed")]
    public List<WeightedEntryWire>? WindSpeed { get; set; }

    [JsonPropertyName("windDirection")]
    public List<DirectionEntryWire>? WindDirection { get; set; }

    [JsonPropertyName("windGustiness")]
    public MinMaxWire? WindGustiness { get; set; }

    [JsonPropertyName("rain")]
    public List<WeightedEntryWire>? Rain { get; set; }

    [JsonPropertyName("rainIntensity")]
    public MinMaxWire? RainIntensity { get; set; }

    [JsonPropertyName("fog")]
    public List<WeightedEntryWire>? Fog { get; set; }

    [JsonPropertyName("tempDay")]
    public MinMaxWire? TempDay { get; set; }

    [JsonPropertyName("tempNight")]
    public MinMaxWire? TempNight { get; set; }

    [JsonPropertyName("pressure")]
    public MinMaxWire? Pressure { get; set; }
}

/// <summary>
/// One <c>Dictionary&lt;string, double&gt;</c> weight entry. The value stays a string across the
/// wire and is parsed at draw time, only if picked - so a non-numeric entry that never wins never
/// throws, exactly as legacy's <c>double.Parse</c> of the picked key does.
/// </summary>
public record WeightedEntryWire
{
    [JsonPropertyName("value")]
    public required string Value { get; set; }

    [JsonPropertyName("weight")]
    public required double Weight { get; set; }
}

/// <summary>
/// One <c>Dictionary&lt;WindDirection, double&gt;</c> weight entry; the key crosses as its numeric
/// enum value, which is also the drawn result.
/// </summary>
public record DirectionEntryWire
{
    [JsonPropertyName("direction")]
    public required int Direction { get; set; }

    [JsonPropertyName("weight")]
    public required double Weight { get; set; }
}

/// <summary>
/// <c>MinMax&lt;double&gt;</c>, without the <c>type</c> member no weather draw reads.
/// </summary>
public record MinMaxWire
{
    [JsonPropertyName("min")]
    public required double Min { get; set; }

    [JsonPropertyName("max")]
    public required double Max { get; set; }
}

/// <summary>
/// One generated <c>Weather</c>'s drawn half, plus the state the applier copies back. The date,
/// time and timestamp members are not here: <c>SetCurrentDateTime</c> runs C#-side on both arms.
/// </summary>
public record GenerateWeatherResponse
{
    [JsonPropertyName("chosenPreset")]
    public required int ChosenPreset { get; set; }

    /// <summary>
    ///     The state was empty and was refilled, so the applier replaces the caller's dictionary
    ///     rather than mutating it - legacy's <c>cloner.Clone</c> assignment.
    /// </summary>
    [JsonPropertyName("refilled")]
    public required bool Refilled { get; set; }

    /// <summary>
    ///     The post-mutation state, in order. Empty when the pick exhausted it.
    /// </summary>
    [JsonPropertyName("updatedPresetWeights")]
    public required List<PresetWeightEntry> UpdatedPresetWeights { get; set; }

    [JsonPropertyName("cloud")]
    public required double Cloud { get; set; }

    [JsonPropertyName("windSpeed")]
    public required double WindSpeed { get; set; }

    [JsonPropertyName("windGustiness")]
    public required double WindGustiness { get; set; }

    [JsonPropertyName("rain")]
    public required double Rain { get; set; }

    [JsonPropertyName("rainIntensity")]
    public required double RainIntensity { get; set; }

    [JsonPropertyName("fog")]
    public required double Fog { get; set; }

    [JsonPropertyName("pressure")]
    public required double Pressure { get; set; }

    [JsonPropertyName("temperature")]
    public required double Temperature { get; set; }

    [JsonPropertyName("windDirection")]
    public required int WindDirection { get; set; }
}
