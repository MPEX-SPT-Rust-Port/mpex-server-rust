//! `Generators/Weather/WeatherGenerator.cs` plus the three `IWeatherPreset` strategies, ported
//! bug-for-bug.
//!
//! Citation convention: a bare `` `:N` `` is a line of `WeatherGenerator.cs`; citations naming a
//! file (`SunnyPreset.cs:21`) are that file's.
//!
//! The three preset strategies collapse into match arms here. Everything the dispatcher resolves
//! before crossing — the season's refill table, `isNight`, one `presetBlocks` entry per enum
//! member, and the whole date/time tail — stays C#-side (spec D10), so this module is the pick,
//! the state mutation and the draws, nothing else.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loot::random_util::{TestSeedGuard, get_double, get_weighted_value, round_to_digits};

/// `WeatherPreset` (`WeatherConfig.cs:72-77`) — the numeric values the wire carries.
const SUNNY: i32 = 1;
const RAINY: i32 = 2;
const CLOUDY: i32 = 3;

/// What a weather pass can fail with: the message of a C#-sanctioned throw carried back to the
/// caller instead of unwinding (the [`crate::raid::RaidError`] shape — this port carries no epoch
/// either).
#[derive(Debug)]
pub enum WeatherError {
    Failed(String),
}

impl WeatherError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// One `Dictionary<WeatherPreset, double>` entry. The state rides an ordered pair list end to end
/// because the pick walks it in enumeration order (`WeightedRandomHelper.cs:23-108`).
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetWeightEntry {
    pub preset: i32,
    pub weight: f64,
}

/// One `PresetWeights` block, already resolved through legacy's `["default"]` fallback C#-side.
/// `block: None` is the unresolvable case — legacy's `KeyNotFoundException` point (spec D10).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetBlockEntry {
    pub preset: i32,
    pub block: Option<PresetWeightsWire>,
}

/// A `Dictionary<string, double>` weight table. The values stay strings on the wire and are parsed
/// at draw time, only the picked one, exactly as `AbstractWeatherPreset` does.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeightedValueEntry {
    pub value: String,
    pub weight: f64,
}

/// A `Dictionary<WindDirection, double>` weight table; the key crosses as its numeric enum value.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeightedDirectionEntry {
    pub direction: i32,
    pub weight: f64,
}

/// `MinMax<double>`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinMaxWire {
    pub min: f64,
    pub max: f64,
}

/// `PresetWeights` (`WeatherConfig.cs:79-107`), with `Temp` flattened into its two ranges.
///
/// **Every member is optional** (spec D10): all but `Clouds` are nullable in the C# model and
/// legacy dereferences them lazily — chosen block only, and never `Rain`/`RainIntensity` on the
/// Sunny/Cloudy arms. An absent member crosses as absent and errors only at the draw that needs
/// it, which is where legacy NREs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetWeightsWire {
    pub clouds: Option<Vec<WeightedValueEntry>>,
    pub wind_speed: Option<Vec<WeightedValueEntry>>,
    pub wind_direction: Option<Vec<WeightedDirectionEntry>>,
    pub wind_gustiness: Option<MinMaxWire>,
    pub rain: Option<Vec<WeightedValueEntry>>,
    pub rain_intensity: Option<MinMaxWire>,
    pub fog: Option<Vec<WeightedValueEntry>>,
    pub temp_day: Option<MinMaxWire>,
    pub temp_night: Option<MinMaxWire>,
    pub pressure: Option<MinMaxWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateWeatherRequest {
    /// The caller's `ref` state, in enumeration order. Empty means refill (`:40`).
    pub preset_weights: Vec<PresetWeightEntry>,
    pub previous_preset: Option<i32>,
    /// `GetWeatherPresetWeightsBySeason(currentSeason)`, resolved unconditionally C#-side — the
    /// season string never crosses (spec D10).
    pub refill_weights: Vec<PresetWeightEntry>,
    /// One entry per `WeatherPreset` enum member.
    pub preset_blocks: Vec<PresetBlockEntry>,
    /// `weatherHelper.IsHourAtNightTime(...)`, resolved C#-side off the seconds-as-ticks quirk at
    /// legacy's own expression (`:132-133`), so a patch on the helper fires on this arm too.
    pub is_night: bool,
    pub test_seed: Option<u64>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateWeatherResponse {
    pub chosen_preset: i32,
    /// The state was empty and was refilled — the applier replaces the `ref` dict rather than
    /// mutating it, as legacy's `cloner.Clone` assignment does (`:43`).
    pub refilled: bool,
    /// The post-mutation state, in order. Empty when the pick exhausted it (`:62`).
    pub updated_preset_weights: Vec<PresetWeightEntry>,
    pub cloud: f64,
    pub wind_speed: f64,
    pub wind_gustiness: f64,
    pub rain: f64,
    pub rain_intensity: f64,
    pub fog: f64,
    pub pressure: f64,
    pub temperature: f64,
    pub wind_direction: i32,
}

/// The module boundary: one `Weather`'s worth of draws, on the request's seeded stream when it
/// carries one. The guard is installed here rather than in `ffi.rs` so no draw can precede it.
///
/// # Errors
///
/// [`WeatherError::Failed`] carrying the message of a C#-sanctioned throw — the empty preset
/// state, the absent chosen block, an absent member the chosen arm draws, or a picked weight value
/// that is not a number — thrown or panicked.
pub fn generate_weather(
    request: GenerateWeatherRequest,
) -> Result<GenerateWeatherResponse, WeatherError> {
    let _seed_guard = request.test_seed.map(TestSeedGuard::install);

    catch_unwind(AssertUnwindSafe(|| generate(&request)))
        .unwrap_or_else(|payload| Err(panic_message(payload)))
}

/// `GenerateWeather` (`:33-66`) and the `GenerateWeatherByPreset` tail its draws live in.
fn generate(request: &GenerateWeatherRequest) -> Result<GenerateWeatherResponse, WeatherError> {
    // `:40-44`. The `cloner.Clone` is the standing documented collaborator hole: the refill table
    // is a fresh projection per call, so there is nothing left to alias.
    let refilled = request.preset_weights.is_empty();
    let source = if refilled {
        &request.refill_weights
    } else {
        &request.preset_weights
    };
    let mut state: IndexMap<i32, f64> = source
        .iter()
        .map(|entry| (entry.preset, entry.weight))
        .collect();

    // `:47-53`: only when a previous preset was chosen *and* it is still in the state.
    if let Some(previous) = request.previous_preset
        && let Some(weight) = state.get_mut(&previous)
    {
        *weight = (*weight - 1.0).max(0.0);
    }

    // `:56`. An empty state errors where legacy's uniform shortcut throws
    // (`WeightedRandomHelper.cs:88-93`); `LootError` has no `Display`, hence `.message`.
    let chosen = get_weighted_value(&state).map_err(|error| WeatherError::new(error.message))?;

    // `:59-63`: the chosen preset is exhausted, so flag the caller for a fresh refill next call.
    if state.get(&chosen).is_some_and(|weight| *weight == 0.0) {
        state.clear();
    }

    // `GetWeatherWeightsByPreset` (`:116-121`), already run C#-side including its `["default"]`
    // fallback. Absent = legacy's `KeyNotFoundException`, as a message — which is also where an
    // out-of-range chosen preset lands, since D10 enumerates the enum and mints no entry for it
    // (spec step 8: no native warning arm, no native fallback).
    let block = request
        .preset_blocks
        .iter()
        .find(|entry| entry.preset == chosen)
        .and_then(|entry| entry.block.as_ref())
        .ok_or_else(|| {
            WeatherError::new(format!("no preset weights for chosen preset {chosen}"))
        })?;

    let draws = draw(block, chosen, request.is_night)?;

    Ok(GenerateWeatherResponse {
        chosen_preset: chosen,
        refilled,
        updated_preset_weights: state
            .iter()
            .map(|(&preset, &weight)| PresetWeightEntry { preset, weight })
            .collect(),
        cloud: draws.cloud,
        wind_speed: draws.wind_speed,
        wind_gustiness: draws.wind_gustiness,
        rain: draws.rain,
        rain_intensity: draws.rain_intensity,
        fog: draws.fog,
        pressure: draws.pressure,
        temperature: draws.temperature,
        wind_direction: draws.wind_direction,
    })
}

/// The drawn half of a `Weather`, in the field order the wire uses rather than the draw order.
struct Draws {
    cloud: f64,
    wind_speed: f64,
    wind_gustiness: f64,
    rain: f64,
    rain_intensity: f64,
    fog: f64,
    pressure: f64,
    temperature: f64,
    wind_direction: i32,
}

/// The three `IWeatherPreset.Generate` bodies, as match arms.
///
/// **Draw order is load-bearing and is each preset's own C# text order** — a C# object initializer
/// evaluates its members top to bottom, so the initializer's layout *is* the sequence the shared
/// RNG sees. The three arms genuinely differ: Cloudy and Rainy hoist their clouds draw into a
/// `var clouds = …` line above the initializer, Sunny draws it inline last.
fn draw(block: &PresetWeightsWire, preset: i32, is_night: bool) -> Result<Draws, WeatherError> {
    let mut draws = match preset {
        // `SunnyPreset.cs:19-34`
        SUNNY => {
            let pressure = ranged(block.pressure.as_ref(), 3, preset, "pressure")?;
            let fog = weighted_number(block.fog.as_deref(), preset, "fog")?;
            let wind_gustiness = ranged(block.wind_gustiness.as_ref(), 2, preset, "windGustiness")?;
            let wind_direction =
                weighted_direction(block.wind_direction.as_deref(), preset, "windDirection")?;
            let wind_speed = weighted_number(block.wind_speed.as_deref(), preset, "windSpeed")?;
            let cloud = weighted_number(block.clouds.as_deref(), preset, "clouds")?;

            Draws {
                cloud,
                wind_speed,
                wind_gustiness,
                // Constants, not draws (`SunnyPreset.cs:24-25`) — so a block with no `rain` or
                // `rainIntensity` runs clean on this arm, exactly as legacy's does.
                rain: 0.0,
                rain_intensity: 0.0,
                fog,
                pressure,
                temperature: 0.0, // `Temperature = 0, // Handled in caller`
                wind_direction,
            }
        }
        // `CloudyPreset.cs:19-36` — clouds first, then Sunny's order.
        CLOUDY => {
            let cloud = weighted_number(block.clouds.as_deref(), preset, "clouds")?;
            let pressure = ranged(block.pressure.as_ref(), 3, preset, "pressure")?;
            let fog = weighted_number(block.fog.as_deref(), preset, "fog")?;
            let wind_gustiness = ranged(block.wind_gustiness.as_ref(), 2, preset, "windGustiness")?;
            let wind_direction =
                weighted_direction(block.wind_direction.as_deref(), preset, "windDirection")?;
            let wind_speed = weighted_number(block.wind_speed.as_deref(), preset, "windSpeed")?;

            Draws {
                cloud,
                wind_speed,
                wind_gustiness,
                rain: 0.0,
                rain_intensity: 0.0,
                fog,
                pressure,
                temperature: 0.0,
                wind_direction,
            }
        }
        // `RainyPreset.cs:19-36` — clouds first, and the two rain members between fog and gustiness.
        RAINY => {
            let cloud = weighted_number(block.clouds.as_deref(), preset, "clouds")?;
            let pressure = ranged(block.pressure.as_ref(), 3, preset, "pressure")?;
            let fog = weighted_number(block.fog.as_deref(), preset, "fog")?;
            let rain_intensity = ranged(block.rain_intensity.as_ref(), 3, preset, "rainIntensity")?;
            let rain = weighted_number(block.rain.as_deref(), preset, "rain")?;
            let wind_gustiness = ranged(block.wind_gustiness.as_ref(), 2, preset, "windGustiness")?;
            let wind_direction =
                weighted_direction(block.wind_direction.as_deref(), preset, "windDirection")?;
            let wind_speed = weighted_number(block.wind_speed.as_deref(), preset, "windSpeed")?;

            Draws {
                cloud,
                wind_speed,
                wind_gustiness,
                rain,
                rain_intensity,
                fog,
                pressure,
                temperature: 0.0,
                wind_direction,
            }
        }
        // Unreachable by construction — a preset outside the enum has no `presetBlocks` entry, so
        // the caller's lookup already errored. Kept as an error rather than a panic all the same.
        _ => {
            return Err(WeatherError::new(format!(
                "unknown weather preset {preset}"
            )));
        }
    };

    // `:103` — drawn *after* the preset's own draws, off `GetRaidTemperature` (`:129-136`).
    let (range, member) = if is_night {
        (block.temp_night.as_ref(), "temp.night")
    } else {
        (block.temp_day.as_ref(), "temp.day")
    };
    draws.temperature = ranged(range, 2, preset, member)?;

    Ok(draws)
}

/// `AbstractWeatherPreset.GetRandomDouble` (`AbstractWeatherPreset.cs:40-43`): one `get_double`,
/// `Math.Round`ed to `digits`.
fn ranged(
    range: Option<&MinMaxWire>,
    digits: i32,
    preset: i32,
    member: &str,
) -> Result<f64, WeatherError> {
    let range = range.ok_or_else(|| missing(preset, member))?;

    Ok(round_to_digits(get_double(range.min, range.max), digits))
}

/// The four string-keyed `GetWeighted*` draws (`AbstractWeatherPreset.cs:20-38`). Only the picked
/// value is parsed, so a non-numeric entry that never wins never throws — matching legacy.
///
/// The C# source is a `Dictionary`, so no two entries can share a value and the `IndexMap` cannot
/// collapse one (which would change the entry count `get_weighted_value`'s equal-weights shortcut
/// tests against).
fn weighted_number(
    entries: Option<&[WeightedValueEntry]>,
    preset: i32,
    member: &str,
) -> Result<f64, WeatherError> {
    let entries = entries.ok_or_else(|| missing(preset, member))?;
    let table: IndexMap<&str, f64> = entries
        .iter()
        .map(|entry| (entry.value.as_str(), entry.weight))
        .collect();

    let picked = get_weighted_value(&table).map_err(|error| WeatherError::new(error.message))?;

    picked
        .parse::<f64>()
        .map_err(|_| WeatherError::new(format!("weather weight value is not a number: {picked}")))
}

/// `GetWeightedWindDirection` (`AbstractWeatherPreset.cs:15-18`) — the picked key is the value, so
/// there is nothing to parse.
fn weighted_direction(
    entries: Option<&[WeightedDirectionEntry]>,
    preset: i32,
    member: &str,
) -> Result<i32, WeatherError> {
    let entries = entries.ok_or_else(|| missing(preset, member))?;
    let table: IndexMap<i32, f64> = entries
        .iter()
        .map(|entry| (entry.direction, entry.weight))
        .collect();

    get_weighted_value(&table).map_err(|error| WeatherError::new(error.message))
}

/// A block member the chosen arm reached for and did not find — legacy's NRE point, as a message.
fn missing(preset: i32, member: &str) -> WeatherError {
    WeatherError::new(format!("preset {preset} weights have no {member}"))
}

/// The text a caught panic carries — `panic!`/`expect` payloads are a `String` or a `&str`.
fn panic_message(payload: Box<dyn Any + Send>) -> WeatherError {
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "weather generation panicked".to_owned());

    WeatherError::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::loot::random_util::TestSeedGuard;

    fn entries(pairs: &[(i32, f64)]) -> Vec<PresetWeightEntry> {
        pairs
            .iter()
            .map(|&(preset, weight)| PresetWeightEntry { preset, weight })
            .collect()
    }

    fn weighted(pairs: &[(&str, f64)]) -> Vec<WeightedValueEntry> {
        pairs
            .iter()
            .map(|&(value, weight)| WeightedValueEntry {
                value: value.to_owned(),
                weight,
            })
            .collect()
    }

    /// Every member present, and every value pinned *by construction* rather than by seed: a
    /// single-candidate weighted table returns its only key with no draw at all
    /// (`random_util.rs:458-462`), and a `min == max` range makes `get_double`'s value
    /// seed-independent — it still consumes its draw, so draw *counts* stay honest.
    fn full_block() -> PresetWeightsWire {
        PresetWeightsWire {
            clouds: Some(weighted(&[("0.5", 1.0)])),
            wind_speed: Some(weighted(&[("1", 1.0)])),
            wind_direction: Some(vec![WeightedDirectionEntry {
                direction: 3,
                weight: 1.0,
            }]),
            wind_gustiness: Some(MinMaxWire { min: 2.0, max: 2.0 }),
            rain: Some(weighted(&[("2", 1.0)])),
            rain_intensity: Some(MinMaxWire { min: 3.0, max: 3.0 }),
            fog: Some(weighted(&[("0.25", 1.0)])),
            temp_day: Some(MinMaxWire { min: 4.0, max: 4.0 }),
            temp_night: Some(MinMaxWire { min: 5.0, max: 5.0 }),
            pressure: Some(MinMaxWire { min: 1.0, max: 1.0 }),
        }
    }

    fn request_with_blocks(
        state: &[(i32, f64)],
        previous: Option<i32>,
        refill: &[(i32, f64)],
        blocks: Vec<(i32, Option<PresetWeightsWire>)>,
    ) -> GenerateWeatherRequest {
        GenerateWeatherRequest {
            preset_weights: entries(state),
            previous_preset: previous,
            refill_weights: entries(refill),
            preset_blocks: blocks
                .into_iter()
                .map(|(preset, block)| PresetBlockEntry { preset, block })
                .collect(),
            is_night: false,
            // The tests install their own guard; a `Some` here would nest a second one.
            test_seed: None,
        }
    }

    fn request_with(
        state: &[(i32, f64)],
        previous: Option<i32>,
        refill: &[(i32, f64)],
    ) -> GenerateWeatherRequest {
        request_with_blocks(
            state,
            previous,
            refill,
            vec![
                (SUNNY, Some(full_block())),
                (RAINY, Some(full_block())),
                (CLOUDY, Some(full_block())),
            ],
        )
    }

    fn weight_of(response: &GenerateWeatherResponse, preset: i32) -> Option<f64> {
        response
            .updated_preset_weights
            .iter()
            .find(|entry| entry.preset == preset)
            .map(|entry| entry.weight)
    }

    #[test]
    fn empty_state_refills_decays_nothing_and_reports_refilled() {
        let _guard = TestSeedGuard::install(42);
        let response =
            generate_weather(request_with(&[], None, &[(1, 5.0), (2, 5.0), (3, 5.0)])).unwrap();

        assert!(response.refilled);
        // Refilled verbatim and in wire order: nothing decayed, nothing exhausted at weight 5.
        assert_eq!(
            response
                .updated_preset_weights
                .iter()
                .map(|entry| (entry.preset, entry.weight))
                .collect::<Vec<_>>(),
            vec![(1, 5.0), (2, 5.0), (3, 5.0)]
        );
    }

    #[test]
    fn previous_preset_decays_by_one_clamped_at_zero() {
        let _guard = TestSeedGuard::install(42);
        let response = generate_weather(request_with(
            &[(1, 0.0), (2, 10.0)],
            Some(1),
            &[(1, 5.0), (2, 5.0)],
        ))
        .unwrap();

        assert!(!response.refilled);
        // `Math.Max(0, 0 - 1)` (`:52`), not -1. Preset 2 is the only reachable pick: preset 1's
        // cumulative weight is 0 and `get_double(0, 1)` never returns 0 (`random_util.rs:144`).
        assert_eq!(weight_of(&response, 1), Some(0.0));
        assert_eq!(response.chosen_preset, 2);
    }

    #[test]
    fn an_exhausted_pick_clears_the_state() {
        // previous=2 over {2: 1.0} decays to {2: 0.0}. The map has ONE entry, so
        // `get_weighted_value`'s single-entry no-draw shortcut returns it (`random_util.rs:458-462`)
        // — no draw occurs, no zero-total table is ever consulted; picked weight == 0 → cleared.
        let _guard = TestSeedGuard::install(42);
        let response = generate_weather(request_with(&[(2, 1.0)], Some(2), &[(2, 5.0)])).unwrap();

        assert!(response.updated_preset_weights.is_empty());
    }

    #[test]
    fn absent_block_members_error_only_when_the_chosen_arm_draws_them() {
        // A single-entry state forces the pick with no draw at all, so the arm is deterministic.
        let mut sunny = full_block();
        sunny.rain = None;
        sunny.rain_intensity = None;

        let _guard = TestSeedGuard::install(42);
        // Sunny never touches either — both are constant 0 in `SunnyPreset.cs:24-25`.
        let response = generate_weather(request_with_blocks(
            &[(SUNNY, 5.0)],
            None,
            &[],
            vec![(SUNNY, Some(sunny))],
        ))
        .unwrap();
        assert_eq!(response.rain, 0.0);
        assert_eq!(response.rain_intensity, 0.0);

        let mut rainy = full_block();
        rainy.rain_intensity = None;
        let result = generate_weather(request_with_blocks(
            &[(RAINY, 5.0)],
            None,
            &[],
            vec![(RAINY, Some(rainy))],
        ));

        // Legacy's lazy NRE point (`RainyPreset.cs:26`), crossing as a message (spec D10).
        let message = match result {
            Err(WeatherError::Failed(message)) => message,
            Ok(_) => panic!("a rainy arm without rainIntensity must not generate"),
        };
        assert!(message.contains("rainIntensity"), "{message}");
    }

    #[test]
    fn rainy_draws_rain_fields_and_sunny_zeroes_them() {
        let _guard = TestSeedGuard::install(42);

        let sunny = generate_weather(request_with(&[(SUNNY, 5.0)], None, &[])).unwrap();
        assert_eq!(sunny.rain, 0.0);
        assert_eq!(sunny.rain_intensity, 0.0);

        let rainy = generate_weather(request_with(&[(RAINY, 5.0)], None, &[])).unwrap();
        // The single candidate / degenerate range of `full_block`, so seed-independent.
        assert_eq!(rainy.rain, 2.0);
        assert_eq!(rainy.rain_intensity, 3.0);

        // Everything both arms share comes off the same fixture, drawn or not.
        assert_eq!((sunny.cloud, sunny.fog, sunny.wind_speed), (0.5, 0.25, 1.0));
        assert_eq!((rainy.cloud, rainy.fog, rainy.wind_speed), (0.5, 0.25, 1.0));
        assert_eq!(sunny.wind_direction, 3);
        assert_eq!((sunny.pressure, sunny.wind_gustiness), (1.0, 2.0));
    }

    #[test]
    fn temperature_comes_off_the_night_range_when_is_night() {
        let _guard = TestSeedGuard::install(42);
        let mut request = request_with(&[(SUNNY, 5.0)], None, &[]);
        request.is_night = true;

        // `GetRaidTemperature` (`:133`) picks `Temp.Night` over `Temp.Day`; `isNight` itself is
        // resolved C#-side off the seconds-as-ticks quirk (spec D10).
        assert_eq!(generate_weather(request).unwrap().temperature, 5.0);

        let day = generate_weather(request_with(&[(SUNNY, 5.0)], None, &[])).unwrap();
        assert_eq!(day.temperature, 4.0);
    }

    #[test]
    fn cloudy_draws_its_clouds_before_the_pressure_the_other_arms_draw_first() {
        // Only the draw *order* separates the two arms, so give clouds a table that consumes a
        // draw (two weight-1 entries take `get_int(0, 1)`) and pressure a real range. Cloudy's
        // `var clouds = …` line precedes its initializer (`CloudyPreset.cs:19`), so its pressure
        // comes one draw later in the stream than sunny's.
        let block = || {
            let mut block = full_block();
            block.clouds = Some(weighted(&[("0.1", 1.0), ("0.9", 1.0)]));
            block.pressure = Some(MinMaxWire {
                min: 700.0,
                max: 800.0,
            });
            block
        };
        let request = |preset| {
            request_with_blocks(&[(preset, 5.0)], None, &[], vec![(preset, Some(block()))])
        };

        let sunny = {
            let _guard = TestSeedGuard::install(42);
            generate_weather(request(SUNNY)).unwrap()
        };
        let cloudy = {
            let _guard = TestSeedGuard::install(42);
            generate_weather(request(CLOUDY)).unwrap()
        };

        assert_ne!(sunny.pressure, cloudy.pressure);
    }

    #[test]
    fn an_absent_chosen_block_is_an_error() {
        let _guard = TestSeedGuard::install(42);
        let result = generate_weather(request_with_blocks(
            &[(SUNNY, 5.0)],
            None,
            &[],
            // Resolvable neither by name nor by `["default"]` — legacy's `KeyNotFoundException`.
            vec![(SUNNY, None)],
        ));

        assert!(result.is_err());
    }

    #[test]
    fn an_empty_state_and_refill_table_is_an_error() {
        // `WeightedRandomHelper`'s uniform shortcut indexes out of bounds on an empty map; the
        // `LootError` message crosses through `.message` (it has no `Display`).
        let _guard = TestSeedGuard::install(42);
        let result = generate_weather(request_with(&[], None, &[]));

        assert!(result.is_err());
    }

    /// The shipped `SUNNY` block's shape: every weighted table has several candidates and every
    /// range is wide, so the pinned outputs below are a function of the seed and the draw order
    /// rather than of the fixture - unlike [`full_block`], which is deliberately degenerate.
    fn kat_block() -> PresetWeightsWire {
        PresetWeightsWire {
            clouds: Some(weighted(&[("-1", 5.0), ("-0.8", 2.0)])),
            wind_speed: Some(weighted(&[
                ("0", 6.0),
                ("1", 3.0),
                ("2", 2.0),
                ("3", 1.0),
                ("4", 1.0),
            ])),
            wind_direction: Some(
                (1..=8)
                    .map(|direction| WeightedDirectionEntry {
                        direction,
                        weight: 1.0,
                    })
                    .collect(),
            ),
            wind_gustiness: Some(MinMaxWire { min: 0.0, max: 1.0 }),
            rain: Some(weighted(&[("1", 2.0), ("2", 1.0), ("3", 1.0)])),
            rain_intensity: Some(MinMaxWire { min: 0.0, max: 1.0 }),
            fog: Some(weighted(&[
                ("0.0013", 30.0),
                ("0.0018", 6.0),
                ("0.002", 4.0),
                ("0.004", 3.0),
                ("0.006", 1.0),
            ])),
            temp_day: Some(MinMaxWire {
                min: 9.0,
                max: 32.0,
            }),
            temp_night: Some(MinMaxWire {
                min: 2.0,
                max: 16.0,
            }),
            pressure: Some(MinMaxWire {
                min: 760.0,
                max: 780.0,
            }),
        }
    }

    /// One pass on one arm at the KAT seed. The state holds the forced preset alone, so the pick
    /// costs no draw and the whole stream belongs to that arm's own draws plus the temperature.
    fn kat(preset: i32) -> GenerateWeatherResponse {
        let _guard = TestSeedGuard::install(20260827);
        let request = request_with_blocks(
            &[(preset, 5.0)],
            None,
            &[],
            vec![(preset, Some(kat_block()))],
        );

        generate_weather(request).unwrap()
    }

    /// Known answer, sunny arm: pressure, fog, windGustiness, windDirection, windSpeed, clouds,
    /// then the temperature. Captured from a run whose C# parity fixture was green, so a change to
    /// any draw, rounding or arm ordering moves these numbers and fails here.
    #[test]
    fn sunny_arm_known_answer() {
        assert_eq!(
            kat(SUNNY),
            GenerateWeatherResponse {
                chosen_preset: SUNNY,
                refilled: false,
                updated_preset_weights: vec![PresetWeightEntry {
                    preset: SUNNY,
                    weight: 5.0,
                }],
                cloud: -1.0,
                wind_speed: 0.0,
                wind_gustiness: 0.42,
                rain: 0.0,
                rain_intensity: 0.0,
                fog: 0.0013,
                pressure: 769.705,
                temperature: 19.77,
                wind_direction: 5,
            }
        );
    }

    /// Known answer, cloudy arm: the same draws as sunny with clouds hoisted to the front, so the
    /// whole stream after it is shifted by one draw and the pins below differ from sunny's.
    #[test]
    fn cloudy_arm_known_answer() {
        assert_eq!(
            kat(CLOUDY),
            GenerateWeatherResponse {
                chosen_preset: CLOUDY,
                refilled: false,
                updated_preset_weights: vec![PresetWeightEntry {
                    preset: CLOUDY,
                    weight: 5.0,
                }],
                cloud: -1.0,
                wind_speed: 1.0,
                wind_gustiness: 0.8,
                rain: 0.0,
                rain_intensity: 0.0,
                fog: 0.0013,
                pressure: 768.018,
                temperature: 19.77,
                wind_direction: 4,
            }
        );
    }

    /// Known answer, rainy arm: clouds, pressure, fog, rainIntensity, rain, windGustiness,
    /// windDirection, windSpeed - the only arm that draws the two rain members.
    #[test]
    fn rainy_arm_known_answer() {
        assert_eq!(
            kat(RAINY),
            GenerateWeatherResponse {
                chosen_preset: RAINY,
                refilled: false,
                updated_preset_weights: vec![PresetWeightEntry {
                    preset: RAINY,
                    weight: 5.0,
                }],
                cloud: -1.0,
                wind_speed: 0.0,
                wind_gustiness: 0.61,
                rain: 1.0,
                rain_intensity: 0.804,
                fog: 0.0013,
                pressure: 768.018,
                temperature: 27.63,
                wind_direction: 8,
            }
        );
    }

    #[test]
    fn a_picked_non_numeric_weight_value_is_an_error() {
        let mut block = full_block();
        block.fog = Some(weighted(&[("thick", 1.0)]));

        let _guard = TestSeedGuard::install(42);
        let result = generate_weather(request_with_blocks(
            &[(SUNNY, 5.0)],
            None,
            &[],
            vec![(SUNNY, Some(block))],
        ));

        // Legacy: `double.Parse`'s `FormatException` (`AbstractWeatherPreset.cs:32`).
        let message = match result {
            Err(WeatherError::Failed(message)) => message,
            Ok(_) => panic!("a non-numeric picked weight value must not generate"),
        };
        assert!(message.contains("thick"), "{message}");
    }
}
