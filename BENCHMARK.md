# Benchmarks

Every benchmark in this repo measures a Rust port against the retained 4.1.2 C# implementation it
replaced. They live in `Testing/UnitTests/Tests/Generators/` as `[Explicit]` NUnit fixtures, so a
plain `dotnet test` never runs them:

| Fixture | Measures |
|---|---|
| `LootBenchmarkTests.cs` | location loot — wall clock, allocation, peak RSS |
| `BotBenchmarkTests.cs` | one bot's inventory — wall clock, with the payload projection timed separately |

Reward loot (`RewardLootBenchmarkTests.cs`) has the same shape; its numbers are recorded in the
reward-loot section of [ARCHITECTURE.md](ARCHITECTURE.md) rather than here.

There are no `cargo bench` targets. `Containerfile.dev` mentions `cargo bench` as a toolchain
capability, not as a suite that exists.

## Running them

Release only — the cargo dev profile makes Debug numbers meaningless.

```bash
scripts/decompress-assets.sh    # once, if SPT_Data/database/locations/*/looseLoot.json is missing

# head-to-head wall clock and allocation
dotnet test -c Release --filter "FullyQualifiedName~LootBenchmarkTests.NativeVersusLegacyCSharp" \
  --logger "console;verbosity=detailed"

# peak RSS — one path per invocation, see Methodology
dotnet test -c Release --filter "FullyQualifiedName~LootBenchmarkTests.NativePeakWorkingSet" \
  --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~LootBenchmarkTests.LegacyPeakWorkingSet" \
  --logger "console;verbosity=detailed"

# one bot per call, both paths, plus the payload projection on its own
dotnet test -c Release --filter "FullyQualifiedName~BotBenchmarkTests" \
  --logger "console;verbosity=detailed"
```

`--logger "console;verbosity=detailed"` is required: the fixture reports through `TestContext.Out`,
and the default logger swallows it. Grep the output for `median`, `speedup`, or `peak RSS`.

`cargo` must be on `PATH` — the Release build compiles `rust/spt-native` first. Release also
regenerates `SPT_Data/checks.dat`, which pulls `System.IO.Hashing` from NuGet, so a cold NuGet cache
needs network access.

To run all three in one go, filter on `~LootBenchmarkTests` — but the two RSS figures are then
measured in a single process and are not comparable to each other. See Methodology.

## Methodology

**Workload.** `GenerateLocationLoot("bigmap")` — full static containers plus dynamic loose loot on
the live shipped database, not a fixture. One call is what a raid start pays.

**Both paths, one process, one database.** The native path is the default; the legacy path is
selected by flipping `LocationConfig.ForceLegacyLootGeneration`, which the fixture restores in a
`finally`. After the warmups it asserts `LocationLootGenerator.LastPathTaken` matches the path it
means to be timing — a benchmark that silently timed the same path twice would report a flat 1.00x
instead of failing.

**Sampling.** 2 warmup runs (JIT, the native library load, the first lazy-load deserialise), then 20
timed runs. `Stopwatch` per run. The heap is settled with a full `GC.Collect()` /
`WaitForPendingFinalizers()` / `GC.Collect()` once before the timed phase and never between timed
runs — collecting inside the phase would distort the wall clock the fixture exists to measure.
Median is the headline figure; mean, min and max are reported alongside because run-to-run spread is
wide (see Caveats).

**Allocation.** `GC.GetTotalAllocatedBytes(precise: true)` across the phase, divided by run count.
This is managed allocation only. Whatever the native side allocates on the Rust heap never reaches
the GC, so on the native path this figure understates real memory traffic by design — peak RSS is
the only number that sees both sides.

**Peak RSS needs its own process.** `Environment.WorkingSet` is process-wide, and the managed heap
does not hand pages back to the OS. Whichever path runs second inherits everything the first left
resident, so its absolute peak is meaningless. That is why `NativePeakWorkingSet` and
`LegacyPeakWorkingSet` are separate tests meant to be run one per `dotnet test` invocation and
compared only against each other. Inside the head-to-head test the comparable figure is
`WorkingSetGrowthMb` (growth over that phase), not the absolute peak.

**Binary sizes** are reported from `AppContext.BaseDirectory` at the end of the head-to-head run.
The legacy C# path still ships inside `SPTarkov.Server.Core.dll` — it is the frozen mod contract —
so the native library is additive on disk today, not a saving.

## Results — location loot

Recorded 2026-08-12 on `e7cb120` plus the working-tree bump of `rand` 0.9 → 0.10.2 and
`rand_xoshiro` 0.7 → 0.8.1.

| | |
|---|---|
| CPU | AMD Ryzen 7 5800X3D (8C/16T) |
| RAM | 62 GB |
| OS | Linux 7.1.8-1-cachyos-bore |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |
| Configuration | Release, `bigmap`, n=20 after 2 warmups |

### Wall clock and allocation

| Path | median | mean | min | max | alloc/run | GC gen0/1/2 |
|---|---|---|---|---|---|---|
| native (rust) | **333.23 ms** | 341.91 ms | 308.92 ms | 423.43 ms | 100.5 MB | 11/10/6 |
| legacy (C# 4.1.2) | 871.17 ms | 864.28 ms | 712.36 ms | 1005.25 ms | 315.1 MB | 147/141/16 |

Speedup on median wall clock: **2.61x**. Managed allocation: **3.14x** less. The GC counts are the
sharper signal — the native path triggers 11 gen0 collections over the phase against the legacy
path's 147.

### Peak working set

Separate `dotnet test` invocations, as above.

| Path | process peak RSS | settled RSS | managed heap | alloc/run |
|---|---|---|---|---|
| native (rust) | 1456 MB | 1445 MB | 327 MB | 100.2 MB |
| legacy (C# 4.1.2) | 1185 MB | 941 MB | 382 MB | 314.3 MB |

Native trades roughly 270 MB of peak RSS for the speed. Those pages are the Rust heap: invisible to
the GC, which is also why native's settled RSS barely drops after a forced collection while the
legacy path's falls by ~240 MB. This is the shape the design predicts, not a regression.

### Binary sizes

| | |
|---|---|
| `libspt_native.so` | 1.95 MB |
| `SPTarkov.Server.Core.dll` | 5.43 MB |

## Results — bot generation

Recorded 2026-08-13 on `fba1733`. **Different machine from the location-loot figures above — the two
result sets are not comparable to each other.**

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |
| Configuration | Release, n=20 after 2 warmups, per path per role |

**Workload.** One `BotInventoryGenerator.GenerateInventory` call — one bot, equipment, mods, weapons
and loot — on the live shipped database, for `assault` and for a level-1 `pmcUSEC`. The template
clone and `BotEquipmentFilterService.FilterBotEquipment` are rebuilt outside the stopwatch for every
run, because the legacy path mutates the template it is handed. `BotPayloadProjection.BuildRequest`
is timed on its own in a third phase: it is the fixed per-call payload cost, and its share is what an
items-view cache would be buying back.

| Role | Path | median | mean | min | max |
|---|---|---|---|---|---|
| assault | native (rust) | **94.07 ms** | 89.94 ms | 52.44 ms | 116.94 ms |
| assault | legacy (C# 4.1.2) | 1.41 ms | 3.31 ms | 0.57 ms | 17.75 ms |
| assault | `BuildRequest` only | 12.79 ms | 12.50 ms | 6.04 ms | 25.45 ms |
| usec | native (rust) | **56.05 ms** | 56.46 ms | 48.41 ms | 69.24 ms |
| usec | legacy (C# 4.1.2) | 1.16 ms | 1.91 ms | 0.83 ms | 11.27 ms |
| usec | `BuildRequest` only | 18.82 ms | 20.53 ms | 8.96 ms | 30.71 ms |

Speedup on median wall clock: **0.01x** for `assault`, **0.02x** for `usec` — the native path is
roughly 40-80x slower per bot. Projection share of the native median: **13.6%** (assault), **33.6%**
(usec).

The shape is the reward-loot result made worse. A 4.1.2 bot is one or two milliseconds of work, and
the native path pays a fixed cost per bot to hand the whole items table (~30k `ItemView`s) and every
global preset across the boundary. `BuildRequest` is only the *building* of that payload; most of the
remaining ~66-86% is serialising it to JSON, crossing the FFI, and deserialising it on the Rust side.
Generation itself is not the cost. See the bot-generation section of [ARCHITECTURE.md](ARCHITECTURE.md)
for why the projection is not cached today and what the sanctioned fix is.

Bot-specific caveats, on top of the general ones below:

- **Two roles, one level, one difficulty.** `assault` and level-1 `pmcUSEC`, `normal`, `standard`
  game version. Bots differ enormously in how much they generate; a boss or a high-level PMC is not
  measured. The absolute native cost is dominated by the fixed payload, so it should move less
  between roles than the legacy figure does.
- **No allocation or RSS figures.** Only wall clock. The native path allocates ~30k view objects per
  bot on the managed heap before serialising them, so its GC pressure is by construction far worse
  than the legacy path's — unmeasured here.
- **The legacy figures include a warm `BotLootCacheService`.** Hydration happens during the warmup
  runs. A cold first bot of a role costs more on both paths.

## Caveats

- **Wall clock spread is wide** — native min-to-max is 309–423 ms on an otherwise idle machine.
  Treat differences under ~10% between runs as noise; the fixture does no outlier rejection and
  reports no confidence interval.
- **One map.** `bigmap` only. Maps differ enormously in loot volume, so the speedup does not
  transfer to other locations unmeasured.
- **A mod changes what is measured.** `UseLegacyPath()` also returns true when any frozen 4.1.2
  protected member carries a live Harmony patch. With such a mod loaded the server runs the legacy
  path in production regardless of these numbers.
- **No historical series.** Nothing records past results, so these figures cannot be diffed against
  an earlier commit without re-running that commit on the same machine.
