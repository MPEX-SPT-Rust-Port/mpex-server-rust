using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Weather;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Generators.Weather;

/// <summary>
/// Weather is generated in <c>rust/spt-native</c> by default; <see cref="WeatherNativeRequestBuilder"/>
/// projects the live config into the native payload. The full C# implementation is retained below as
/// the legacy path - it is the frozen mod contract - and runs instead of the native path when a
/// Harmony patch on any member of the frozen set is detected, when the injected
/// <see cref="IWeatherPreset"/> set is not exactly the three built-ins, when a mod substituted the
/// generator, when the frozen constructor built the instance or when
/// <see cref="WeatherConfig.ForceLegacyWeatherGeneration"/> is set, so mod hooks fire with genuine
/// baseline semantics.
/// </summary>
[Injectable]
public class WeatherGenerator(
    ISptLogger<WeatherGenerator> logger,
    TimeUtil timeUtil,
    WeatherHelper weatherHelper,
    WeatherConfig weatherConfig,
    WeightedRandomHelper weightedRandomHelper,
    RandomUtil randomUtil,
    IEnumerable<IWeatherPreset> weatherGenerators,
    ICloner cloner
)
{
    private readonly WeatherNativeRequestBuilder? _requestBuilder;

    /// <summary>
    ///     The frozen constructor plus the native request builder. Additive and apicompat-verified.
    /// </summary>
    public WeatherGenerator(
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
        : this(logger, timeUtil, weatherHelper, weatherConfig, weightedRandomHelper, randomUtil, weatherGenerators, cloner)
    {
        _requestBuilder = requestBuilder;
    }

    /// <summary>
    ///     Which implementation the most recent generation ran - the spt-native path or the retained
    ///     C# path. Test seam; also handy in a debugger. Unsynchronized - concurrent weather
    ///     requests race it - which only the non-parallel fixtures that assert on it may ignore.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     Test-only seed forwarded as <see cref="GenerateWeatherRequest.TestSeed"/> on every native
    ///     request.
    /// </summary>
    internal ulong? NativeTestSeed { get; set; }

    /// <summary>
    ///     The legacy path runs when the frozen constructor built this instance (it has no native
    ///     seam to dispatch to), when forced by config, when any member of the frozen set carries a
    ///     live Harmony patch, when the injected preset strategies are not exactly the three
    ///     built-ins, or when a mod has substituted the generator itself - running the retained C#
    ///     implementation is the only way those hooks and replacements can take effect with real
    ///     baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_requestBuilder is null || weatherConfig.ForceLegacyWeatherGeneration)
        {
            return true;
        }

        if (WeatherNativeRequestBuilder.AnyFrozenMemberPatched())
        {
            return true;
        }

        // A mod preset (extra, missing, or substituted) must run for real - legacy calls its
        // CanHandle/Generate, and the native arm has only the three built-in bodies
        var presetTypes = weatherGenerators.Select(generator => generator.GetType()).ToHashSet();
        if (presetTypes.Count != 3 || !presetTypes.SetEquals([typeof(SunnyPreset), typeof(CloudyPreset), typeof(RainyPreset)]))
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return GetType() != typeof(WeatherGenerator);
    }

    /// <summary>
    /// Generate a weather object to send to client
    /// </summary>
    /// <param name="currentSeason">What season is weather being generated for</param>
    /// <param name="presetWeights">Weather preset weights to pick from (values will be altered when generating more than 1)</param>
    /// <param name="timestamp">Optional - Current time in millisecond ticks</param>
    /// <param name="previousPreset">Optional -What weather preset was last generated</param>
    /// <returns>A generated <see cref="Weather"/> object</returns>
    public Models.Eft.Weather.Weather GenerateWeather(
        Season currentSeason,
        ref Dictionary<WeatherPreset, double> presetWeights,
        long? timestamp = null,
        WeatherPreset? previousPreset = null
    )
    {
        if (!UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Native;

            // effectiveTimestamp exists ONLY to derive isNight - SetCurrentDateTime below gets the
            // ORIGINAL nullable timestamp. Substituting the resolved value would flip null-timestamp
            // calls (client/weather, always null) onto the branch whose FormatToBsgDate applies
            // ToUniversalTime() to a Kind=Unspecified DateTime - a host-timezone Date/Time shift
            var effectiveTimestamp = timestamp ?? timeUtil.GetTimeStamp();

            // The seconds-as-ticks quirk preserved at legacy's exact expression - and the live
            // WeatherHelper call keeps a patch there firing on this arm
            var isNight = weatherHelper.IsHourAtNightTime(new DateTime(effectiveTimestamp).Hour);

            var request = _requestBuilder!.BuildGenerateWeatherRequest(
                presetWeights,
                previousPreset,
                GetWeatherPresetWeightsBySeason(currentSeason),
                isNight,
                NativeTestSeed
            );
            var response = _requestBuilder.SendGenerateWeather(request);

            var result = new Models.Eft.Weather.Weather
            {
                Pressure = response.Pressure,
                Temperature = response.Temperature,
                Fog = response.Fog,
                RainIntensity = response.RainIntensity,
                Rain = response.Rain,
                WindGustiness = response.WindGustiness,
                WindDirection = (WindDirection)response.WindDirection,
                WindSpeed = response.WindSpeed,
                Cloud = response.Cloud,
                Time = string.Empty,
                Date = string.Empty,
                Timestamp = 0,
                SptInRaidTimestamp = 0,
            };

            SetCurrentDateTime(result, timestamp); // the ORIGINAL nullable argument - see above
            result.SptChosenPreset = (WeatherPreset)response.ChosenPreset;

            // State write-back: refill replaced the reference in legacy, everything else mutated in
            // place - both preserved
            if (response.Refilled)
            {
                presetWeights = new Dictionary<WeatherPreset, double>();
            }
            else
            {
                presetWeights.Clear();
            }

            foreach (var entry in response.UpdatedPresetWeights)
            {
                presetWeights[(WeatherPreset)entry.Preset] = entry.Weight;
            }

            return result;
        }

        LastPathTaken = LootGenerationPath.Legacy;

        if (presetWeights.Count == 0)
        {
            // No presets, get fresh cloned weights from config
            presetWeights = cloner.Clone(GetWeatherPresetWeightsBySeason(currentSeason));
        }

        // Only process when we have weights + there was previous preset chosen
        if (previousPreset.HasValue && presetWeights.ContainsKey(previousPreset.Value))
        {
            // We know last picked preset, Adjust weights
            // Make it less likely to be picked now
            // Clamp to 0
            presetWeights[previousPreset.Value] = Math.Max(0, presetWeights[previousPreset.Value] - 1);
        }

        // Assign value to previousPreset to be picked up next loop
        previousPreset = weightedRandomHelper.GetWeightedValue(presetWeights);

        // Check if chosen preset has been exhausted and reset if necessary
        if (presetWeights[previousPreset.Value] == 0)
        {
            // Flag for fresh presets
            presetWeights.Clear();
        }

        return GenerateWeatherByPreset(previousPreset.Value, timestamp);
    }

    /// <summary>
    /// Gets weather property weights for the provided season
    /// </summary>
    /// <param name="currentSeason">Desired season to get weights for</param>
    /// <returns>A dictionary of weather preset weights</returns>
    public Dictionary<WeatherPreset, double> GetWeatherPresetWeightsBySeason(Season currentSeason)
    {
        return weatherConfig.Weather.WeatherPresetWeight.TryGetValue(currentSeason.ToString(), out var weights)
            ? weights
            : weatherConfig.Weather.WeatherPresetWeight.GetValueOrDefault("default")!;
    }

    /// <summary>
    /// Creates a <see cref="Weather"/> object that adheres to the chosen preset
    /// </summary>
    /// <param name="chosenPreset">The weather preset chosen to generate</param>
    /// <param name="timestamp">OPTIONAL - generate the weather object with a specific time instead of now</param>
    /// <returns>A generated <see cref="Weather"/> object</returns>
    protected SPTarkov.Server.Core.Models.Eft.Weather.Weather GenerateWeatherByPreset(WeatherPreset chosenPreset, long? timestamp)
    {
        var generator = weatherGenerators.FirstOrDefault(gen => gen.CanHandle(chosenPreset));
        if (generator is null)
        {
            logger.Warning($"Unable to find weather generator for: {chosenPreset}, falling back to sunny");

            generator = weatherGenerators.FirstOrDefault(gen => gen.CanHandle(WeatherPreset.SUNNY));
        }

        var presetWeights = GetWeatherWeightsByPreset(chosenPreset);
        var result = generator.Generate(presetWeights);

        // Set time values in result using now or passed in timestamp
        SetCurrentDateTime(result, timestamp);

        // Must occur after SetCurrentDateTime(), temp depends on timestamp
        result.Temperature = GetRaidTemperature(presetWeights, result.SptInRaidTimestamp ?? 0);

        // Needed by RaidWeatherService
        result.SptChosenPreset = chosenPreset;

        return result;
    }

    /// <summary>
    /// Get the weather preset weights based on passed in preset, get defaults if preset not found in config
    /// </summary>
    /// <param name="weatherPreset">Desired preset</param>
    /// <returns>PresetWeights</returns>
    protected PresetWeights GetWeatherWeightsByPreset(WeatherPreset weatherPreset)
    {
        return weatherConfig.Weather.PresetWeights.TryGetValue(weatherPreset.ToString(), out var value)
            ? value
            : weatherConfig.Weather.PresetWeights["default"];
    }

    /// <summary>
    ///     Choose a temperature for the raid based on time of day
    /// </summary>
    /// <param name="weather"> What season Tarkov is currently in </param>
    /// <param name="inRaidTimestamp"> What time is the raid running at </param>
    /// <returns> Timestamp </returns>
    protected double GetRaidTemperature(PresetWeights weather, long inRaidTimestamp)
    {
        // Convert timestamp to date so we can get current hour and check if its day or night
        var currentRaidTime = new DateTime(inRaidTimestamp);
        var minMax = weatherHelper.IsHourAtNightTime(currentRaidTime.Hour) ? weather.Temp.Night : weather.Temp.Day;

        return Math.Round(randomUtil.GetDouble(minMax.Min, minMax.Max), 2);
    }

    /// <summary>
    ///     Set Weather date/time/timestamp values to now
    /// </summary>
    /// <param name="weather"> Object to update </param>
    /// <param name="timestamp"> Optional, timestamp used </param>
    protected void SetCurrentDateTime(Models.Eft.Weather.Weather weather, long? timestamp = null)
    {
        var inRaidTime = timestamp is null ? weatherHelper.GetInRaidTime() : weatherHelper.GetInRaidTime(timestamp.Value);
        var normalTime = inRaidTime.GetBsgFormattedWeatherTime();
        var formattedDate = (timestamp.HasValue ? timeUtil.GetDateTimeFromTimeStamp(timestamp.Value) : DateTime.UtcNow).FormatToBsgDate();
        var datetimeBsgFormat = $"{formattedDate} {normalTime}";

        weather.Timestamp = timestamp ?? timeUtil.GetTimeStamp(); // matches weather.date
        weather.Date = formattedDate; // matches weather.timestamp
        weather.Time = datetimeBsgFormat; // matches weather.timestamp
        weather.SptInRaidTimestamp = weather.Timestamp;
    }
}
