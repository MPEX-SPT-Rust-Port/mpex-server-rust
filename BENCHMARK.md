# Benchmarks

Every benchmark measures a Rust port against the retained 4.1.2 C# implementation it replaced. They
live under `Testing/UnitTests/Tests/` as `[Explicit]` NUnit fixtures, so a plain `dotnet test` never
runs them. There are no `cargo bench` targets.

| Fixture | Measures |
|---|---|
| `LootBenchmarkTests.cs` | location loot — elapsed time per call, allocation, peak RSS |
| `RewardLootBenchmarkTests.cs` | airdrop loot — elapsed time per call |
| `BotBenchmarkTests.cs` | one bot's inventory — elapsed time per bot, payload projection timed separately |
| `RagfairBenchmarkTests.cs` | a dynamic flea offer pass — elapsed time per pass, projection timed separately |
| `RepeatableQuestBenchmarkTests.cs` | one repeatable quest of each type — cold and warm invariant slice |
| `ScavCaseBenchmarkTests.cs` | one scav case of each shipped recipe — elapsed time per call |
| `ItemBaseClassBenchmarkTests.cs` | one bulk item base class cache build — elapsed time per hydrate |
| `RagfairLinkedItemBenchmarkTests.cs` | one ragfair linked item table build — elapsed time per build |
| `DbPublishSpikeTests.cs` | phase 0 state-ownership spike — full-DB publish envelope: per-root size and projection time; paired with `rust/spt-native/tests/phase0_publish_spike.rs` for parse time and RSS |

## Running them

Release only — the cargo dev profile makes Debug numbers meaningless. `cargo` must be on `PATH`.

```bash
scripts/decompress-assets.sh    # once, if SPT_Data/database/locations/*/looseLoot.json is missing

dotnet test -c Release --filter "FullyQualifiedName~LootBenchmarkTests.NativeVersusLegacyCSharp" \
  --logger "console;verbosity=detailed"

# peak RSS — one path per invocation
dotnet test -c Release --filter "FullyQualifiedName~LootBenchmarkTests.NativePeakWorkingSet" \
  --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~LootBenchmarkTests.LegacyPeakWorkingSet" \
  --logger "console;verbosity=detailed"

dotnet test -c Release --filter "FullyQualifiedName~RewardLootBenchmarkTests"      --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~BotBenchmarkTests"             --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~RagfairBenchmarkTests"         --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~RepeatableQuestBenchmarkTests" --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~ScavCaseBenchmarkTests"        --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~ItemBaseClassBenchmarkTests"   --logger "console;verbosity=detailed"
dotnet test -c Release --filter "FullyQualifiedName~RagfairLinkedItemBenchmarkTests" --logger "console;verbosity=detailed"
```

`--logger "console;verbosity=detailed"` is required — the fixtures report through `TestContext.Out`.
Grep for `median`, `speedup`, or `peak RSS`.

## Phase 0 — full-DB publish spike (state ownership)

`c660f9e` — 2026-08-18. One full `{"schema":1,"roots":{...}}` envelope (design doc:
docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md), crossed via a temp file — no FFI
export exists yet. looseLoot (549 MiB raw) is excluded by design; locations project `Base` only.

    dotnet test -c Release --filter "FullyQualifiedName~DbPublishSpikeTests" --logger "console;verbosity=detailed"
    cd rust && cargo test --release --test phase0_publish_spike -- parse_full_publish_envelope_value_bound --ignored --nocapture

(Each spike test must run in its own process for its RSS figure to be valid — run the typed
locales test by its own name, `parse_locales_root_typed`, in a separate invocation.)

| measure | value |
|---|---|
| envelope size | 92.4 MiB |
| C# projection, warm (all roots) | 1260.8 ms |
| envelope assembly | 148.7 ms |
| Rust parse (`Value` bound) | 650.2 ms |
| Rust RSS delta (`Value` bound) | 405.2 MiB |
| end-to-end warm (projection + assembly + parse; FFI copy est. +10–50 ms) | 2059.7 ms |

Per-root breakdown (Release, verbatim fixture output):

```
root          size MiB   cold ms   warm ms
templates         22.4     692.5     391.7
bots               5.7     252.2      94.6
hideout            0.3      39.8       4.6
locales           58.6     924.0     629.1
locations          1.3      74.5      27.2
match              0.0       1.9       0.0
traders            2.5      71.2      33.4
globals            0.7     185.0       7.3
server             0.0       0.6       0.0
settings           0.3      34.1       5.2
configs            0.7     370.6      67.7
TOTAL             92.4    2646.5    1260.8
```

Gates (design doc § Phase 0): end-to-end warm < ~1 s and RSS delta < ~1 GB → **go** for
whole-DB publish. RSS-only trip → re-judge with `parse_locales_root_typed` (the `Value` bound
over-counts string-map roots) before falling back to per-root granularity. Verdict: **no-go** for
whole-DB publish. The latency gate trips — 2059.7 ms warm end-to-end, ≈2070–2110 ms with the
FFI-copy estimate, against the ~1 s bar — while RSS passes (405.2 MiB), so the typed-locales
refinement cannot rescue the verdict (it addresses RSS-only trips; for the record, the isolated-
process typed run measured 232.3 ms / 99.2 MiB for the locales root). Fallback per the design doc:
**per-root sync granularity**, which the breakdown supports — the C# projection dominates the
total and locales is the worst root at 629.1 ms warm projection; a locales-only republish is
≈861 ms (629.1 projection + 232.3 typed parse; ≈0.97–1.01 s with a byte-proportional assembly
share and the copy estimate), straddles the ~1 s bar, and every other root is far cheaper.

## Methodology

- Both paths run in one process against one live shipped database; the fixture asserts `LastPathTaken`
  before reporting.
- 2 warmups then 20 timed runs (ragfair: 1 then 5). `Stopwatch` per run, one full `GC.Collect()`
  before the timed phase and none inside it. Median is the headline figure.
- Every fixture was run twice, back to back, same arm order. The headline median and the mean/min/max
  beside it are the first invocation's; the `2nd run` column is the repeat.
- Allocation is `GC.GetTotalAllocatedBytes(precise: true)` over the phase / run count — managed only.
- Peak RSS needs its own `dotnet test` invocation per path; within a shared process only
  `WorkingSetGrowthMb` is comparable.
- Workstation GC (`<ServerGarbageCollection>` is set on `SPTarkov.Server.csproj`, not `UnitTests.csproj`).

## Machine

All figures below are from one machine, one commit, one sitting.

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 (Fedora 44) |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |

Earlier location-loot figures in this file were taken on a Ryzen 7 5800X3D and have been replaced;
its 2.61x is not comparable to the 2.21x below.

## Location loot

`06825b3` — 2026-08-17. `GenerateLocationLoot("bigmap")`, n=20 after 2 warmups.

| Path | median | median (2nd run) | mean | min | max | alloc/run | GC gen0/1/2 |
|---|---|---|---|---|---|---|---|
| native (rust) | **461.92 ms** | 447.86 ms | 460.42 ms | 389.96 ms | 566.27 ms | 104.8 MB | 46/27/6 |
| legacy (C# 4.1.2) | 1019.11 ms | 1048.97 ms | 1016.59 ms | 892.19 ms | 1158.30 ms | 311.2 MB | 796/679/16 |

Speedup: **2.21x** (2.34x on the second invocation). Managed allocation: **2.97x** less. The
`LazyLoad` transformer case (~1347 ms on the native typed path) was not re-measured.

### Peak working set

Separate invocations per path, two of each.

| Path | process peak RSS | settled RSS | managed heap | alloc/run |
|---|---|---|---|---|
| native (rust) | 1603 / 1715 MB | 1600 / 1692 MB | 341 MB | 105.0 MB |
| legacy (C# 4.1.2) | 1415 / 1356 MB | 1101 / 987 MB | 390 MB | 311.5 / 314.0 MB |

### Binary sizes

| | |
|---|---|
| `libspt_native.so` | 20.29 MB |
| `SPTarkov.Server.Core.dll` | 5.59 MB |

The `.so` is unstripped — `rust/Cargo.toml` sets `debug = "line-tables-only"` on the release profile
and nothing strips it. Stripped it is a fraction of that; the size does not affect any timing here.

## Airdrop loot

`06825b3` — 2026-08-17. One `LootGenerator.CreateRandomLoot` call, n=20 after 2 warmups.

| Path | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|
| **native (rust)** | **75.55 ms** | 80.97 ms | 75.83 ms | 62.11 ms | 90.44 ms |
| **legacy (C# 4.1.2)** | **15.05 ms** | 14.37 ms | 14.55 ms | 4.53 ms | 28.12 ms |

Speedup: **0.20x** (0.18x on the second invocation) — ~5x slower.

## Bot generation

`06825b3` — 2026-08-17. One `BotInventoryGenerator.GenerateInventory` call, n=20 after 2 warmups.
Assault is measured first.

| Role | Path | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|---|
| assault | native (rust) | **90.32 ms** | 87.92 ms | 89.06 ms | 75.35 ms | 101.63 ms |
| assault | legacy (C# 4.1.2) | 1.99 ms | 2.05 ms | 2.47 ms | 0.74 ms | 8.14 ms |
| assault | `BuildRequest` only | 17.46 ms | 18.09 ms | 18.20 ms | 14.55 ms | 23.27 ms |
| usec | native (rust) | **55.98 ms** | 57.89 ms | 57.75 ms | 49.03 ms | 82.76 ms |
| usec | legacy (C# 4.1.2) | 1.29 ms | 1.35 ms | 1.50 ms | 0.87 ms | 3.51 ms |
| usec | `BuildRequest` only | 15.82 ms | 16.24 ms | 17.41 ms | 12.76 ms | 26.50 ms |

Speedup: **0.02x** for both roles. Projection share of the native median: **19.3%** (assault),
**28.3%** (usec); 20.6% / 28.1% on the second invocation. Steady state is ~56-58 ms per bot; the role
timed first still reads ~32 ms high.

`BuildRequest` has roughly doubled against the `94fe128` measurement (9.89 ms assault, 5.09 ms usec)
while the native totals held, so the phase split recorded then — `BuildRequest` 10.3 ms, C# serialise
21.6 ms, Rust deserialise 14.9 ms, Rust generation 2.9 ms, FFI + result deserialise ~1.4 ms — no
longer adds up and has not been re-measured.

### Batched wave

`aa733a7` — 2026-08-14, not re-measured. `BotBatchTests.WaveCostPerBot`, medians of 5 per arm, two
invocations.

| wave | serial per-bot | `.AsParallel()` per-bot | batched (rayon) | batched vs parallel |
|---|---|---|---|---|
| 45 | 44.10 / 43.16 ms | 12.33 / 11.98 ms | **5.73 / 6.22 ms** | 2.15x / 1.92x |
| 20 | 43.94 / 42.83 ms | 12.72 / 12.26 ms | **6.02 / 5.95 ms** | 2.11x / 2.06x |
| 10 | 43.11 / 42.67 ms | 11.67 / 11.85 ms | **7.63 / 7.85 ms** | 1.53x / 1.51x |
| 5 | 42.48 / 42.71 ms | 13.89 / 13.62 ms | **11.83 / 11.97 ms** | 1.17x / 1.14x |
| 1 | 41.42 / 41.46 ms | 43.29 / 43.25 ms | **42.72 / 41.81 ms** | 1.01x / 1.03x |

All figures ms per bot. Request bytes per bot: 3.78 MiB single-bot, against 0.30 (wave 45), 0.40 (20),
0.58 (10), 0.94 (5), 3.78 (1) MiB batched.

## Ragfair offer generation

`06825b3` — 2026-08-17. Two `RagfairOfferGenerator.GenerateDynamicOffers` calls (full pass;
regeneration pass over 1,400 expired offers), n=5 after 1 warmup, against a pre-filled flea.

| Scenario | Path | median | median (2nd run) | mean | min | max | offers | alloc/run |
|---|---|---|---|---|---|---|---|---|
| full pass | native (rust) | **699.55 ms** | 719.06 ms | 645.09 ms | 533.60 ms | 736.57 ms | 24,100 | 206.2 MB |
| full pass | legacy (C# 4.1.2) | 453.59 ms | 476.02 ms | 465.08 ms | 442.01 ms | 524.05 ms | 24,053 | 283.2 MB |
| full pass | `BuildRequest` only | 13.55 ms | 13.47 ms | 13.42 ms | 10.05 ms | 15.65 ms | — | 7.1 MB |
| regeneration | native (rust) | **14.19 ms** | 14.25 ms | 15.01 ms | 10.12 ms | 19.91 ms | 862 | 5.2 MB |
| regeneration | legacy (C# 4.1.2) | 10.80 ms | 10.33 ms | 10.17 ms | 6.86 ms | 13.94 ms | 885 | 5.9 MB |
| regeneration | `BuildRequest` only | 15.50 ms | 13.36 ms | 14.67 ms | 8.76 ms | 20.09 ms | — | 7.1 MB |
| regeneration | native, slice cache **cold** | 80.85 ms | 82.87 ms | 81.28 ms | 70.97 ms | 93.04 ms | 874 | 17.6 MB |
| regeneration | native, slice cache **warm** | **10.59 ms** | 10.17 ms | 13.63 ms | 10.55 ms | 19.23 ms | 878 | 5.2 MB |

Speedup: **0.65x** full pass (0.66x), **0.76x** regeneration (0.72x). Slice cache warm vs cold:
**7.64x** on the median (8.15x), **0.30x** the allocation. Projection share of the native median:
**1.9%** (full pass). On the regeneration pass the projection is timed on its own and reads 94-109%
of the pass it is part of — its own spread swamps a 14 ms pass, so read it as "most of it", not as a
ratio.

Working set over the timed phase: native +387 MB on the full pass, legacy +0 MB.

Wire volume, framed MessagePack (ABI 10, the shipped path), 57,842 offers: **35.4 MB** — measured on
`6d975e4`, not re-measured; the fixture does not report it.

Spread: native full pass 533-773 ms across both invocations, ±20 ms run-to-run on the median —
changes under ~5% cannot be resolved here.

## Repeatable quest generation

`06825b3` — 2026-08-17. One `Generate` call per quest type, n=20 after 2 warmups.

| Type | Arm | median | median (2nd run) | mean | min | max | alloc/run |
|---|---|---|---|---|---|---|---|
| Elimination | legacy (C# 4.1.2) | 23.78 ms | 23.03 ms | 24.13 ms | 22.74 ms | 27.21 ms | 1.3 MB |
| Elimination | native, slice cold | 92.41 ms | 92.95 ms | 91.56 ms | 79.17 ms | 107.40 ms | 10.9 MB |
| Elimination | **native, slice warm** | **2.60 ms** | 2.76 ms | 2.66 ms | 2.40 ms | 4.14 ms | 0.1 MB |
| Completion | legacy (C# 4.1.2) | 56.15 ms | 56.94 ms | 55.17 ms | 21.15 ms | 99.78 ms | 3.4 MB |
| Completion | native, slice cold | 53.41 ms | 54.12 ms | 55.97 ms | 48.51 ms | 79.45 ms | 10.6 MB |
| Completion | **native, slice warm** | **4.95 ms** | 4.84 ms | 5.11 ms | 4.67 ms | 6.64 ms | 0.1 MB |
| Exploration | legacy (C# 4.1.2) | 3.50 ms | 2.99 ms | 3.72 ms | 3.20 ms | 6.93 ms | 1.3 MB |
| Exploration | native, slice cold | 50.06 ms | 49.41 ms | 51.03 ms | 44.93 ms | 60.11 ms | 10.7 MB |
| Exploration | **native, slice warm** | **2.35 ms** | 2.36 ms | 2.42 ms | 2.24 ms | 3.83 ms | 0.1 MB |
| Pickup | legacy (C# 4.1.2) | 3.25 ms | 3.11 ms | 3.57 ms | 2.91 ms | 6.34 ms | 1.2 MB |
| Pickup | native, slice cold | 50.20 ms | 48.89 ms | 51.37 ms | 44.99 ms | 60.50 ms | 10.7 MB |
| Pickup | **native, slice warm** | **2.29 ms** | 2.24 ms | 2.44 ms | 2.16 ms | 4.00 ms | 0.0 MB |
| — | `BuildInvariantSlice` only | 10.47 ms | 9.64 ms | 11.49 ms | 6.55 ms | 19.52 ms | 6.8 MB |

Speedup, legacy against warm native — what a stock server runs:

| Type | speedup (run 1 / run 2) | read as |
|---|---|---|
| Elimination | **9.16x / 8.34x** | |
| Completion | 11.35x / 11.77x | **~4.3x** — see below |
| Exploration | **1.49x / 1.27x** | |
| Pickup | **1.42x / 1.39x** | |

The Completion legacy arm is bimodal: median 56 ms, min 21.15 ms, max 99.78 ms, and its min matches
the 23.31 ms median recorded on `1819c0c`. Take its speedup off the min (~4.3x), not the median.

Cost of a full slice send (cold − warm), per call: Completion 48.47 / 49.28 ms, Exploration
47.71 / 47.05 ms, Pickup 47.91 / 46.65 ms, Elimination 89.81 / 90.19 ms (first-position inflation —
read its cold cost as ~47 ms like the rest). ~10.6 MB managed allocation per full send, of which
`BuildInvariantSlice()` is 10.47 / 9.64 ms and 6.8 MB.

Cold (modded server) against legacy: Elimination ~3.9x slower, Completion about level on the median
and ~2.5x slower against the legacy min, Exploration and Pickup ~15x slower.

## Scav case rewards

`06825b3` — 2026-08-17. One `ScavCaseRewardGenerator.Generate(recipeId)` per shipped recipe, n=20
after 2 warmups.

| Recipe | End products (common/rare/superrare) | Arm | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|---|---|
| `6271093e…` moonshine | 0 / 1 / 3-5 | native (rust) | 75.93 ms | 79.37 ms | 75.92 ms | 60.24 ms | 97.05 ms |
| `6271093e…` moonshine | 0 / 1 / 3-5 | legacy (C# 4.1.2) | 1.67 ms | 1.59 ms | 1.98 ms | 1.58 ms | 3.82 ms |
| `62710a8c…` 15,000 ₽ | 1 / 1-3 / 0 | native (rust) | 42.64 ms | 43.12 ms | 48.04 ms | 36.05 ms | 72.31 ms |
| `62710a8c…` 15,000 ₽ | 1 / 1-3 / 0 | legacy (C# 4.1.2) | 0.80 ms | 0.79 ms | 0.85 ms | 0.74 ms | 1.28 ms |
| `62710974…` 2,500 ₽ | 1-2 / 0-1 / 0 | **native (rust)** | **39.35 ms** | **39.01 ms** | 41.35 ms | 35.65 ms | 57.22 ms |
| `62710974…` 2,500 ₽ | 1-2 / 0-1 / 0 | **legacy (C# 4.1.2)** | **0.40 ms** | **0.44 ms** | 0.42 ms | 0.38 ms | 0.79 ms |
| `62710a69…` 95,000 ₽ | 0 / 1-3 / 1-2 | **native (rust)** | **39.10 ms** | **38.41 ms** | 41.05 ms | 35.06 ms | 58.56 ms |
| `62710a69…` 95,000 ₽ | 0 / 1-3 / 1-2 | **legacy (C# 4.1.2)** | **0.43 ms** | **0.45 ms** | 0.45 ms | 0.39 ms | 0.74 ms |
| `62710a0e…` intel folder | 0 / 2-4 / 2-3 | **native (rust)** | **38.51 ms** | **38.69 ms** | 40.92 ms | 35.04 ms | 52.89 ms |
| `62710a0e…` intel folder | 0 / 2-4 / 2-3 | **legacy (C# 4.1.2)** | **0.46 ms** | **0.47 ms** | 0.49 ms | 0.40 ms | 0.89 ms |
| — | — | `Build` (request only) | 7.75 ms | 6.86 ms | 9.23 ms | 4.42 ms | 16.72 ms |

Steady state (positions 3-5): **~39 ms native against ~0.44 ms legacy — 0.011x, ~88x slower**, flat
across recipes. The first two positions measured are inflated on both arms; reversing the recipe
order on `d31a000` reproduced the gradient positionally, so read those two rows as ~39 / ~0.44 ms.

`ScavCaseConfig.ForceLegacyScavCaseGeneration` is the opt-out.

## Item base class cache

`06825b3` — 2026-08-17. One `ItemBaseClassService.HydrateItemBaseClassCache()` over the shipped items
table (4,673 templates in; 4,553 tpls / 20,218 ancestor ids out), n=20 after 2 warmups, fresh service
instance per run.

| Arm | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|
| **native (rust)** | **32.47 ms** | 32.55 ms | 32.20 ms | 24.63 ms | 41.39 ms |
| **legacy (C# 4.1.2)** | **7.53 ms** | 7.44 ms | 9.92 ms | 7.20 ms | 24.27 ms |
| `Build` (request only) | 0.28 ms | 0.28 ms | 0.29 ms | 0.27 ms | 0.60 ms |

Speedup: **0.23x** on both invocations — **~4.3x slower**. `Build()` is ~1% of the native median.
Arm order was not re-tested here; the reversed read on `526704c` was 27.65 / 8.56 ms (0.31x).

Hydrate runs once per startup, so the cost is ~25 ms added to startup.
`ItemConfig.ForceLegacyItemBaseClassHydration` is the opt-out.

## Ragfair linked item table

`06825b3` — 2026-08-17. One linked item table build over the shipped items table (4,673 templates in;
4,673 tpls / 63,530 linked ids out), timed as `GetLinkedItems(tpl)` on a fresh instance, n=20 after
2 warmups.

| Arm | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|
| **native (rust)** | **93.69 ms** | 92.50 ms | 90.02 ms | 48.09 ms | 118.96 ms |
| **legacy (C# 4.1.2)** | **15.38 ms** | 14.88 ms | 15.03 ms | 10.57 ms | 20.20 ms |
| `Build` (request only) | 3.61 ms | 3.92 ms | 6.89 ms | 2.24 ms | 17.02 ms |

Speedup: **0.16x** as the fixture measures it, on both invocations. `Build()` is ~4% of the native
median.

Both arms move with measurement order. Measured on `dd96eb1`, not re-tested here:

| Arm | measured first | measured second |
|---|---|---|
| native (rust) | 92.53 / 92.91 ms | 74.25 / 81.20 ms |
| legacy (C# 4.1.2) | 30.12 / 29.74 ms | 14.65 / 15.19 ms |

Position for position: **~3.1x** slower (both first) or **~5.1-5.4x** (both second). Read the band.

The build is lazy and single-shot, so the loss is ~78 ms on whichever request arrives first.
`RagfairConfig.ForceLegacyRagfairLinkedItemBuild` is the opt-out.

## Caveats

- **Spread is wide.** Treat differences under ~10% between runs as noise; no outlier rejection, no
  confidence intervals.
- **One map** for location loot: `bigmap`.
- **A mod changes what is measured.** `UseLegacyPath()` also returns true when any frozen 4.1.2
  protected member carries a live Harmony patch, so such a server runs legacy regardless of these
  numbers.
- **No historical series.** Figures cannot be diffed against an earlier commit without re-running it
  on the same machine.
