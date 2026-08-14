# Benchmarks

Every benchmark in this repo measures a Rust port against the retained 4.1.2 C# implementation it
replaced. They live in `Testing/UnitTests/Tests/Generators/` as `[Explicit]` NUnit fixtures, so a
plain `dotnet test` never runs them:

| Fixture | Measures |
|---|---|
| `LootBenchmarkTests.cs` | location loot — wall clock, allocation, peak RSS |
| `RewardLootBenchmarkTests.cs` | airdrop loot — wall clock; numbers recorded in the reward-loot section of [ARCHITECTURE.md](ARCHITECTURE.md), not here |
| `BotBenchmarkTests.cs` | one bot's inventory — wall clock, with the payload projection timed separately |
| `RagfairBenchmarkTests.cs` | a dynamic flea offer pass — wall clock, with the payload projection timed separately |

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

# a full flea pass and a regeneration pass, both paths, plus the payload projection on its own
dotnet test -c Release --filter "FullyQualifiedName~RagfairBenchmarkTests" \
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

Recorded 2026-08-13 on `94fe128` plus the working-tree fix to the projection phase's timed region.
**Different machine from the location-loot figures above — the two result sets are not comparable to
each other.**

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
is timed on its own in a third phase — with the same hoisting, so the clone is not counted twice: it
is the fixed per-call payload cost, and its share bounds what any projection-side fix could buy back.

| Role | Path | median | mean | min | max |
|---|---|---|---|---|---|
| assault | native (rust) | **88.04 ms** | 84.06 ms | 48.62 ms | 119.37 ms |
| assault | legacy (C# 4.1.2) | 1.36 ms | 2.18 ms | 0.59 ms | 12.99 ms |
| assault | `BuildRequest` only | 9.89 ms | 9.88 ms | 5.20 ms | 25.14 ms |
| usec | native (rust) | **54.43 ms** | 55.47 ms | 47.49 ms | 71.12 ms |
| usec | legacy (C# 4.1.2) | 1.20 ms | 1.59 ms | 0.82 ms | 5.22 ms |
| usec | `BuildRequest` only | 5.09 ms | 5.41 ms | 4.83 ms | 8.14 ms |

Speedup on median wall clock: **0.02x** for both roles — the native path is tens of times slower per
bot. Projection share of the native median: **11.2%** (assault), **9.4%** (usec).

**The assault-vs-usec gap is measurement order, not role.** Whichever role is timed first pays
roughly 40 ms of extra serialise time against a still-growing heap; reverse the order and the two
figures swap. Steady state is ~51-54 ms per bot for both roles, so the per-role reading of the table
above — and any single ratio derived from it — is an artifact of the harness.

The shape is the reward-loot result made worse. A 4.1.2 bot is one or two milliseconds of work, and
the native path pays a fixed cost per bot to hand the whole items table across the boundary: 4,673
`ItemView`s plus ~5,000 nested slot/grid/cartridge/chamber views (~9.7k objects), and every global
preset. A phase-by-phase split of a ~51 ms bot reads: `BuildRequest` 10.3 ms, C# serialise 21.6 ms,
Rust deserialise 14.9 ms, **Rust generation 2.9 ms**, FFI plus result deserialise ~1.4 ms — so
**~92% of the cost is payload transport**, not generation, and no projection-side fix reaches more
than the build phase's fifth of it.

That is what the batched wave path attacks: `BotWaveBatcher` sends the shared views once per wave
instead of once per bot, so a wave of N divides ~95.7% of the request between N bots. Measured
against the baseline production actually runs — `BotController.GenerateBotWave`'s `.AsParallel()`
per-bot loop — a **single-threaded** batch was worth **~1.7x** at the median wave of 10 (12.5 → 7.3
ms/bot) and **~2.4-2.5x** at `assault`'s real wave of 45 (13.2 → 5.2-5.5 ms/bot). Those are the
pre-rayon readings, kept for comparison. The batch loop now runs under rayon, and the measured
figures in the next section come out slightly *above* those batch times — **1.9-2.2x** at wave 45 and
**~1.5x** at wave 10 — so treat the ratios in this paragraph as the historical single-threaded
reading, not the current one. `BotBatchTests.WaveCostPerBot` reports the serial, parallel and batched
arms together so the sequential-baseline ratio cannot be quoted alone again. Two payload trims landed
alongside it — sixteen always-default item members (−640 KB, −13.8%) and `slots[].required` (−49 KB),
taking the `pmcUSEC` request the size guards measure to ~4.18 MB — worth ~4-6% on the per-bot path
and ~0% batched, since the batch already divides the block they shrink.

Bot-specific caveats, on top of the general ones below:

- **Two roles, one level, one difficulty.** `assault` and level-1 `pmcUSEC`, `normal`, `standard`
  game version. Bots differ enormously in how much they generate; a boss or a high-level PMC is not
  measured. The absolute native cost is dominated by the fixed payload, so it should move less
  between roles than the legacy figure does.
- **No allocation or RSS figures.** Only wall clock. The native path allocates ~9.7k view objects per
  bot on the managed heap before serialising them, so its GC pressure is by construction far worse
  than the legacy path's — unmeasured here.
- **The legacy figures include a warm `BotLootCacheService`.** Hydration happens during the warmup
  runs. A cold first bot of a role costs more on both paths.

### Batched wave, rayon batch loop — measured

Recorded 2026-08-14 on `aa733a7`, same machine and environment table as the bot-generation figures
above (AMD Ryzen 5 5600H, 6C/12T; Release; .NET SDK 10.0.110; rustc 1.97.1), so the same caveats
apply — one machine, `assault`, warm caches, wall clock only. Source: `BotBatchTests.WaveCostPerBot`
(`[Explicit]`, `dotnet test -c Release --filter "Name=WaveCostPerBot"`), medians of 5 runs per arm
per wave. Two independent invocations of the whole test are reported, because the spread between
them is the honest error bar on any single figure here.

| wave | serial per-bot | `.AsParallel()` per-bot | batched (rayon) | batched vs parallel | pre-rayon batched |
|---|---|---|---|---|---|
| 45 | 44.10 / 43.16 ms | 12.33 / 11.98 ms | **5.73 / 6.22 ms** | 2.15x / 1.92x | 5.2-5.5 ms |
| 20 | 43.94 / 42.83 ms | 12.72 / 12.26 ms | **6.02 / 5.95 ms** | 2.11x / 2.06x | not recorded |
| 10 | 43.11 / 42.67 ms | 11.67 / 11.85 ms | **7.63 / 7.85 ms** | 1.53x / 1.51x | 7.3 ms |
| 5 | 42.48 / 42.71 ms | 13.89 / 13.62 ms | **11.83 / 11.97 ms** | 1.17x / 1.14x | not recorded |
| 1 | 41.42 / 41.46 ms | 43.29 / 43.25 ms | **42.72 / 41.81 ms** | 1.01x / 1.03x | not recorded |

All figures are ms per bot. Request bytes per bot over the same waves: 3.78 MiB single-bot, against
0.30 (wave 45), 0.40 (20), 0.58 (10), 0.94 (5), 3.78 (1) MiB batched.

**Rayon did not move the batched number.** 5.73/6.22 ms/bot at wave 45 against a pre-rayon 5.2-5.5,
and 7.63/7.85 at wave 10 against a pre-rayon 7.3 — the parallel batch is at best level with the
single-threaded batch it replaced, and both readings sit slightly above it. That is what the ~92%
transport share predicts: rayon parallelises the ~3 ms of generation inside a ~6 ms/bot batch, while
the serialise/deserialise around it stays single-threaded on the C# side and does not shrink. The
batch's win over production's `.AsParallel()` per-bot loop is amortising the shared block, not
parallelism — it was already won before rayon. Rayon's value is therefore a ceiling on wave sizes and
bot kinds that generate more than `assault` does, not a gain visible here; it is kept because it
costs nothing measurable, not because this table justifies it.

Wave 1 is the degenerate case and reads as expected: a one-bot batch is a one-bot request, `1.01x`,
and `.AsParallel()` over a single element is the serial arm.

## Results — ragfair offer generation

Recorded 2026-08-13 on `ad1b6ea`. Same machine as the bot-generation figures above, **not** the
machine the location-loot figures came from.

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |
| Configuration | Release, n=5 after 1 warmup, per path per scenario |

**Workload.** Two `RagfairOfferGenerator.GenerateDynamicOffers` calls on the live shipped database,
the two the server actually makes. *Full pass*: no expired offers, so the assort is generated and
every sellable template gets its offers — ~24k offers with full item trees. *Regeneration pass*:
1,400 cloned single-item lists (the configured `expiredOfferThreshold`), the shape
`RagfairServer.ProcessExpiredFleaOffers` hands over. `RagfairPayloadProjection.BuildRequest` is
timed on its own in a third phase per scenario.

**The flea is already full when the timing starts.** The test harness runs every `IOnLoad`, so
`RagfairCallbacks.OnLoadAsync` → `RagfairServer.Load()` has already generated a complete set of
dynamic offers into the holder before the fixture takes its baseline snapshot. The fixture removes
everything each run adds, outside the stopwatch, but deliberately preserves that pre-existing set —
so every timed run is a pass over an *already-populated* flea, on both paths. That is the state the
`ProcessExpiredFleaOffers` call runs in for real, and it is symmetric across paths, so the verdict
below is unaffected; it does change what the offer column means (see below).

### Wall clock

| Scenario | Path | median | mean | min | max | offers | alloc/run |
|---|---|---|---|---|---|---|---|
| full pass | native (rust) | **1485.30 ms** | 1463.00 ms | 1385.77 ms | 1505.78 ms | 24,190 | 184.1 MB |
| full pass | legacy (C# 4.1.2) | 436.57 ms | 439.95 ms | 409.66 ms | 483.10 ms | 24,296 | 283.9 MB |
| full pass | `BuildRequest` only | 14.90 ms | 14.80 ms | 8.37 ms | 21.58 ms | — | 7.1 MB |
| regeneration | native (rust) | **94.70 ms** | 93.62 ms | 88.12 ms | 97.71 ms | 893 | 17.5 MB |
| regeneration | legacy (C# 4.1.2) | 10.74 ms | 10.64 ms | 7.26 ms | 13.62 ms | 873 | 5.9 MB |
| regeneration | `BuildRequest` only | 13.03 ms | 13.16 ms | 8.37 ms | 18.09 ms | — | 7.1 MB |

Speedup on median wall clock: **0.29x** on the full pass (**3.4x slower**) and **0.11x** on the
regeneration pass (**8.8x slower**). Projection share of the native median: **1.0%** (full pass),
**13.8%** (regeneration).

The offer column is **offers accepted into an already-full flea**, not offers the path produced.
`RagfairOfferHolder.AddOffer` drops a fake-player offer once that template is at its cap, and the
cap is re-rolled per call (`RagfairServerHelper.GetOfferCountByBaseType` is a `RandomUtil.GetInt`
over the configured range), so a full flea still admits whatever a high roll leaves room for. That
is why 1,400 distinct expired templates land as ~880 offers — the pre-filled holder, not anything
about the run's own interleaving. Against an empty holder the same input would land as ~1,400. The
two paths land within 0.5% of each other on the full pass, so both are doing the same amount of
work.

Working set over the timed phase: native grows +265 MB on the full pass against the legacy path's
+0 MB; peak RSS is process-wide and cumulative, so only the growth figures are comparable, and only
within one invocation.

**This is a gate failure.** The full pass is the one the plan said to judge on, and native loses it.
The shape differs from bot generation: there the fixed payload was the whole story, here it is not.
`BuildRequest` is 15 ms of a 1,485 ms native pass — **1%** — so nothing on the projection side can
buy back a loss of this size (and caching the projection is off the table anyway: see
[RUST-ROADMAP.md](RUST-ROADMAP.md) roadmap #3).

The remaining ~1.47 s splits in two, and the fixture's own log says how. The native side replays its
internal timings through the same logger the legacy path uses, and they read `Took 709-722ms to
CreateOffersFromAssort` on every full-pass run: **~713 ms — 48% of the native median — is the Rust
generation itself**, running single-threaded. The legacy path's own diagnostic for the same work is
375-469 ms (median ~405 ms of its 437 ms total), spread across 12 threads. The other **~772 ms** of
the native pass is the wrapper: serialising the request, the FFI crossing, deserialising a response
of ~24k offers with full item trees, and inserting them into the holder — the response volume
spec §7 flagged as the unknown. So neither half alone explains the loss: a free projection saves 1%,
a perfect wrapper still leaves single-threaded Rust generation at ~1.75x the parallel C#, and a
parallelised Rust batch still pays ~772 ms of wrapper. On the regeneration pass the internal figure
is 16 ms of a 94.7 ms median — there the wrapper is nearly the whole cost.

Ragfair-specific caveats, on top of the general ones below:

- **The legacy path is parallel, the native path is not.** `GenerateDynamicOffersLegacy` fans one
  `Task.Factory.StartNew` per assort entry across the thread pool; on this 12-thread machine it does
  its 405 ms of generation in 437 ms of wall clock. The native side runs the ragfair batch on the
  calling thread — rayon is in the crate, but only the bot batch uses it — so its 713 ms of
  generation is 713 ms of wall clock. A single-threaded legacy comparison is not measured; against
  one, native would look better, but the parallel version is what the server actually runs.
- **Every timed run works on a pre-filled flea** (see Workload). Both paths pay the holder's
  per-template cap checks against a full `_fakePlayerOffers` index, so the absolute numbers include
  work an empty-flea pass would not do — but symmetrically, and this is the state the real
  regeneration pass runs in.
- **n=5 after 1 warmup**, not the 20 the other fixtures use — a full pass is seconds, not
  milliseconds. Spread is narrow (native full pass 1386-1506 ms), so the medians are still usable.
- **Regeneration input is fixed.** The same 1,400 single-item lists every run, sampled from the head
  of the assort rather than from offers that actually expired. Real expired offers skew towards
  fast-selling templates and can carry item trees.
- **First-call effects are warmed away.** One warmup per phase covers JIT, the native library load
  and the assort generation's first pass. The server's real first flea pass at startup costs more on
  both paths.
- **Native is measured first in each scenario**, so the legacy phase inherits a warmer process (and
  a heap already grown to the native path's high-water mark). That biases mildly *against* native —
  it does not account for a 3.4x gap, but the gap is not smaller than measured.

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
