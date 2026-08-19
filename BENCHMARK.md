# Benchmarks

Every benchmark measures a Rust port against the retained 4.1.2 C# implementation it replaced. They
live under `Testing/UnitTests/Tests/` as `[Explicit]` NUnit fixtures, so a plain `dotnet test` never
runs them. There are no `cargo bench` targets.

| Fixture | Measures |
|---|---|
| `LootBenchmarkTests.cs` | location loot — elapsed time per call, allocation, peak RSS |
| `RewardLootBenchmarkTests.cs` | airdrop loot — elapsed time per call |
| `BotBenchmarkTests.cs` | one bot's inventory — elapsed time per bot, payload projection timed separately |
| `RagfairBenchmarkTests.cs` | a dynamic flea offer pass — elapsed time per pass, views-override projection and forced publish timed separately |
| `RepeatableQuestBenchmarkTests.cs` | one repeatable quest of each type — elapsed time per quest, publish cold/warm and views-override projection timed separately |
| `ScavCaseBenchmarkTests.cs` | one scav case of each shipped recipe — elapsed time per call, publish cold/warm and the override projections timed separately |
| `ItemBaseClassBenchmarkTests.cs` | one bulk item base class cache build — elapsed time per hydrate |
| `RagfairLinkedItemBenchmarkTests.cs` | one ragfair linked item table build — elapsed time per build |
| `DbPublishSpikeTests.cs` | phase 0 state-ownership spike — full-DB publish envelope: per-root size and projection time; paired with `rust/spt-native/tests/phase0_publish_spike.rs` for parse time and RSS |
| `RagfairViewsEquivalenceTests.cs` | phase 1 flip #1 — writes the 3-root publish envelope and C#-built expected ragfair views; paired with `rust/spt-native/tests/phase1_ragfair_views.rs` for the derivation-equivalence check |
| `QuestViewsEquivalenceTests.cs` | phase 1 flip #2 — writes the 4-root publish envelope and C#-built expected quest views; paired with `rust/spt-native/tests/phase1_quest_views.rs` for the derivation-equivalence check |

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

`f6c40fa` — 2026-08-19, post resident-DB flip (phase 1 flip #4, ABI 25). Same fixture shape and
workload. The native arm now rides the resident DB: `DbPublisher.EnsureCurrent` publishes the
four roots once (absorbed in warmup — the locations root gained the three statics lifts for this
flip, ~19 MB serialized from each `LazyLoad.Value` so registered transformers apply) and every
timed pass sends `{epoch, varying}` only — the ~22 MB per-call items-view projection is gone
from eligible sends. `BuildVarying`/`BuildViewsOverride` replace the old whole-projection
`BuildCommonPayload`: the override is the ineligible caller's per-call cost at the pre-flip
shape, no part of the eligible pass (this fixture times no projection arm; it never did).
looseLoot still crosses per call, as the raw splice inside the varying block.

| Path | median | median (2nd run) | mean | min | max | alloc/run | GC gen0/1/2 |
|---|---|---|---|---|---|---|---|
| native (rust, resident db) | **327.82 ms** | 318.38 ms | 326.84 ms | 260.88 ms | 382.39 ms | 83.9 MB | 12/7/4 |
| legacy (C# 4.1.2) | 995.85 ms | 1009.40 ms | 988.39 ms | 850.17 ms | 1142.31 ms | 315.2 MB | 805/681/15 |

Speedup: **3.04x** (3.17x on the second invocation) against the pre-flip 2.21x — the win
widened, all of it on the native arm (461.92 → 327.82 ms; the legacy arm is code-identical and
its 1019.11 → 995.85 ms move is inside the noise bar). Managed allocation: native 104.8 →
83.9 MB/run, **3.76x** less than legacy (pre-flip 2.97x).

The statics made the publish dearer — the accepted per-*mutation* price of this flip. The forced
publish (the ragfair fixture's `publish (4 roots, forced)` arm, n=5 after 1 warmup, measured in
the same sitting) reads **730.08 ms** median (749.33 ms on the second invocation) against
flip #2's 471.64 ms: a ~+260 ms delta that is the statics' own serialize + copy + parse share —
~19 MB of `LazyLoad.Value` reads, in line with Phase 0's per-root rates (22.4 MiB of templates
cost 391.7 ms warm projection alone), not an anomaly. Every family's republish now pays it;
per-root dirty tracking is the named upgrade path if the per-mutation cost ever matters. Nothing
per-call moved for it: ragfair's own arms held (full pass native 520.95 ms against flip #1's
520.25 ms; regeneration 12.56 against 12.06 ms), while ragfair's publish-cold arm now reads
752.75 ms for republishing the statics-bearing envelope every run.

Peak working set, separate invocations per path, two of each:

| Path | process peak RSS | settled RSS | managed heap | alloc/run |
|---|---|---|---|---|
| native (rust, resident db) | 2183 / 2107 MB | 2141 / 2030 MB | 400 MB | 84.0 / 84.1 MB |
| legacy (C# 4.1.2) | 1689 / 1723 MB | 1464 / 1445 MB | 399 MB | 312.8 / 314.4 MB |

The accepted price is resident memory: the native process now peaks ~380–490 MB above legacy
(pre-flip: ~190–360 MB above), carrying the four published roots plus the statics lifts —
the shape Phase 0 priced (405.2 MiB `Value`-bound RSS delta, before the statics joined). Read
the gap, not the absolutes: legacy's own peak moved 1415/1356 → 1689/1723 MB on unchanged code,
so the sitting drifted too. The `.so` is 21.85 MB this sitting (was 20.29 MB), still unstripped.

## Airdrop loot

`06825b3` — 2026-08-17. One `LootGenerator.CreateRandomLoot` call, n=20 after 2 warmups.

| Path | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|
| **native (rust)** | **75.55 ms** | 80.97 ms | 75.83 ms | 62.11 ms | 90.44 ms |
| **legacy (C# 4.1.2)** | **15.05 ms** | 14.37 ms | 14.55 ms | 4.53 ms | 28.12 ms |

Speedup: **0.20x** (0.18x on the second invocation) — ~5x slower.

`f6c40fa` — 2026-08-19, post resident-DB flip (phase 1 flip #4, ABI 25). Same fixture shape and
workload. The native arm now rides the resident DB: an eligible send is `{epoch, varying}` —
the whole-items-view serialisation that was the cost of this path per call is gone from
eligible sends, surviving only as the ineligible `viewsOverride` arm. The reward
exports read the resident preset views (`defaultPresets` here; sealed's resident arm builds no
`presetsByTpl` at all any more); the six service-backed blacklists/sets ride the varying block.

| Path | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|
| **native (rust, resident db)** | **1.57 ms** | 1.62 ms | 1.73 ms | 1.41 ms | 2.54 ms |
| **legacy (C# 4.1.2)** | **19.69 ms** | 20.93 ms | 19.92 ms | 6.42 ms | 30.81 ms |

Speedup: **12.52x** (12.96x on the second invocation), against the pre-flip **0.20x** —
residency did not just close the ~5x deficit, it inverted it: the pre-flip native median
(75.55 ms) was almost entirely the per-call items-view projection and serialisation, and what
remains is a ~1.6 ms generation pass. The legacy arm is code-identical to pre-flip; its
15.05 → 19.69 ms move (min 6.42) is the sitting.

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

`ae325d8` — 2026-08-18, after the level fold (ABI 22). `BotBatchTests.WaveCostPerBot`, medians of 5
per arm, two invocations.

| wave | serial per-bot | `.AsParallel()` per-bot | batched (rayon) | batched vs parallel |
|---|---|---|---|---|
| 45 | 48.49 / 48.70 ms | 14.90 / 15.35 ms | **1.72 / 1.69 ms** | 8.68x / 9.08x |
| 20 | 48.10 / 48.94 ms | 14.87 / 14.18 ms | **2.53 / 2.59 ms** | 5.88x / 5.48x |
| 10 | 48.46 / 49.44 ms | 15.28 / 15.14 ms | **5.14 / 4.87 ms** | 2.97x / 3.11x |
| 5 | 47.36 / 49.00 ms | 16.38 / 16.09 ms | **9.54 / 9.77 ms** | 1.72x / 1.65x |
| 1 | 48.85 / 50.56 ms | 48.90 / 50.42 ms | **46.76 / 46.44 ms** | 1.05x / 1.09x |

All figures ms per bot. Request bytes per bot: 3.81 MiB single-bot, against 0.08 (wave 45), 0.19 (20),
0.38 (10), 0.76 (5), 3.81 (1) MiB batched.

Against the previous measurement (`aa733a7`, 2026-08-14): batched cost per bot fell from 5.73 to 1.72
ms at wave 45 and from 7.63 to 5.14 at wave 10, and per-bot request bytes fell from 0.30 to 0.08 MiB
(45), 0.40 → 0.19 (20), 0.58 → 0.38 (10), 0.94 → 0.76 (5). Read the batched column against the
*same-run* serial and parallel arms, not against the old table: both of those arms are unchanged code
and still drifted 9.5-31% between the two dates (serial 9.5-18%, parallel 13-31%).

The whole saving is wire volume. The batch's per-bot cost is `shared/N + slice`, and folding the
template, loot pools and handbook prices out of the slice and onto the shared block as per-level-band
variants left the slice at `botId` + `testSeed` + `details` — a few hundred bytes, small enough to
vanish into the MiB rounding above (wave 45's 0.08 MiB/bot is `3.81/45` to two decimals). So the
shared block is now effectively **100%** of the request, where before the fold it was the 95.7% that
`SharedBotViewsWire`'s doc comment used to quote. Wave 1 is unchanged *in what it builds*: one bot,
one segment, one copy of everything, the same bytes as before the fold. Its timing still moves run to
run like every other row here (42.72 → 46.76 ms across the two dates, against unchanged-code arms
that drifted as much) - the construction claim is not a claim that the number holds.

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

`0287c7e` — 2026-08-18, post resident-DB flip (phase 1 flip #1, ABI 22). Same fixture shape and
workload. The native arms now ride the resident DB: `DbPublisher.EnsureCurrent` publishes the
templates, traders and globals roots once (absorbed in warmup) and every timed pass sends the
varying block only. `BuildRequest` now times the C#-built `viewsOverride` — the ineligible
caller's per-call cost, no longer any part of the eligible pass. The publish cold/warm rows
replace the deleted slice cache's cold/warm rows: cold bumps `DatabaseMutationStamp` before every
run, so each pass republishes all three roots first.

| Scenario | Path | median | median (2nd run) | mean | min | max | offers | alloc/run |
|---|---|---|---|---|---|---|---|---|
| full pass | native (rust, resident db) | **520.25 ms** | 531.57 ms | 543.22 ms | 468.97 ms | 622.33 ms | 23,817 | 207.2 MB |
| full pass | legacy (C# 4.1.2) | 455.06 ms | 419.75 ms | 456.44 ms | 424.67 ms | 498.72 ms | 24,018 | 282.6 MB |
| full pass | `BuildRequest` (views override) | 15.87 ms | 14.18 ms | 21.73 ms | 12.88 ms | 41.29 ms | — | 7.1 MB |
| regeneration | native (rust, resident db) | **12.06 ms** | 11.75 ms | 13.52 ms | 10.56 ms | 18.02 ms | 879 | 5.3 MB |
| regeneration | legacy (C# 4.1.2) | 9.90 ms | 9.64 ms | 9.51 ms | 5.62 ms | 13.34 ms | 847 | 5.9 MB |
| regeneration | `BuildRequest` (views override) | 13.84 ms | 13.14 ms | 14.16 ms | 8.80 ms | 20.55 ms | — | 7.1 MB |
| publish (3 roots, forced) | `DbPublisher.ForcePublish` | 456.73 ms | 468.19 ms | 452.48 ms | 416.59 ms | 482.14 ms | — | — |
| regeneration | native, publish **cold** (stamp bumped per run) | 440.02 ms | 446.55 ms | 437.99 ms | 431.88 ms | 443.97 ms | 882 | 156.9 MB |
| regeneration | native, publish **warm** | **11.58 ms** | 11.44 ms | 13.59 ms | 10.35 ms | 19.59 ms | 857 | 5.3 MB |

Speedup: **0.87x** full pass (0.79x), **0.82x** regeneration (0.82x). Publish warm vs cold:
**37.99x** on the median (39.02x). Against the pre-flip `06825b3` numbers: pass 1 (full, native)
699.55 → 520.25 ms and pass 2 (regeneration, native) 14.19 → 12.06 ms — the per-call half no
longer builds or sends any view. The forced publish, 456.73 ms, is the whole per-*mutation* cost
(3-root projection + FFI copy + parse + view derivation; Phase 0's warm three-root figure was
432.4 ms); the cold arm's 440.02 ms median reading *below* it is run-to-run noise — the two arms'
ranges overlap — not generation being free. The resident warm arm reads 11.58 ms against the deleted slice cache's warm 10.59 ms —
the varying block now carries per call what the slice held resident (the spec § C# driver
carve-out fields, O(KB)); at ~1 ms on an 11 ms pass the fixture cannot fully resolve it, and it is the
accepted price of retiring the slice cache. Working set over the timed
phase: native +370 MB on the full pass (2nd run +390 MB), legacy +0 MB — same shape as pre-flip's
+387 MB.

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

`e7c2852` — 2026-08-18, post resident-DB flip (phase 1 flip #2, ABI 23). Same fixture shape and
workload. The native arms now ride the resident DB: `DbPublisher.EnsureCurrent` publishes the
templates, traders, globals and locations roots once (absorbed in warmup — the locations root,
`Base` + `AllExtracts` only, joined the envelope for this flip) and every timed pass sends the
varying block only. `BuildViewsOverride` times the C#-built `viewsOverride` — the ineligible
caller's per-call cost. The publish cold/warm rows replace the deleted slice cache's cold/warm
rows: cold bumps `DatabaseMutationStamp` before every run, so each pass republishes all four
roots first. The forced-publish row is the ragfair fixture's `DbPublisher.ForcePublish` arm
(n=5 after 1 warmup, measured in the same sitting; the publisher is family-agnostic, and its
envelope now carries four roots — the arm's console label still prints "3 roots").

| Type | Arm | median | median (2nd run) | mean | min | max | alloc/run |
|---|---|---|---|---|---|---|---|
| Elimination | legacy (C# 4.1.2) | 20.59 ms | 21.09 ms | 21.61 ms | 19.55 ms | 30.16 ms | 1.3 MB |
| Elimination | native, publish **cold** (stamp bumped per run) | 455.19 ms | 456.53 ms | 470.33 ms | 447.24 ms | 651.82 ms | 156.5 MB |
| Elimination | **native, publish warm** | **2.69 ms** | 2.94 ms | 2.96 ms | 2.50 ms | 4.97 ms | 0.1 MB |
| Completion | legacy (C# 4.1.2) | 21.48 ms | 21.91 ms | 30.12 ms | 19.32 ms | 126.53 ms | 3.4 MB |
| Completion | native, publish **cold** (stamp bumped per run) | 460.74 ms | 460.41 ms | 463.05 ms | 450.40 ms | 502.89 ms | 156.4 MB |
| Completion | **native, publish warm** | **4.81 ms** | 4.86 ms | 5.17 ms | 4.68 ms | 8.33 ms | 0.1 MB |
| Exploration | legacy (C# 4.1.2) | 3.57 ms | 3.62 ms | 4.00 ms | 3.02 ms | 6.46 ms | 1.3 MB |
| Exploration | native, publish **cold** (stamp bumped per run) | 457.53 ms | 459.56 ms | 460.28 ms | 448.42 ms | 499.02 ms | 156.4 MB |
| Exploration | **native, publish warm** | **2.53 ms** | 2.54 ms | 2.80 ms | 2.38 ms | 4.33 ms | 0.1 MB |
| Pickup | legacy (C# 4.1.2) | 3.06 ms | 3.82 ms | 4.16 ms | 2.72 ms | 13.86 ms | 1.3 MB |
| Pickup | native, publish **cold** (stamp bumped per run) | 458.06 ms | 458.67 ms | 459.90 ms | 446.93 ms | 496.98 ms | 156.4 MB |
| Pickup | **native, publish warm** | **2.36 ms** | 2.67 ms | 2.59 ms | 2.30 ms | 4.15 ms | 0.1 MB |
| — | `BuildViewsOverride` only | 11.79 ms | 11.81 ms | 13.82 ms | 7.31 ms | 32.19 ms | 6.6 MB |
| — | publish (4 roots, forced) — `DbPublisher.ForcePublish` | 471.64 ms | 471.03 ms | 480.43 ms | 442.43 ms | 527.53 ms | — |

Speedup, legacy against warm native (run 1 / run 2): Elimination **7.67x / 7.18x**, Completion
4.46x / 4.51x, Exploration **1.41x / 1.43x**, Pickup **1.30x / 1.43x**. The Completion legacy arm's
low mode carried its median this sitting (21.48 ms against the pre-flip bimodal 56.15/min 21.15),
so its speedup already reads off the low mode — no correction needed. Warm against the deleted
slice cache's warm (`06825b3`: 2.60 / 4.95 / 2.35 / 2.29 ms): 2.69 / 4.81 / 2.53 / 2.36 ms — every
delta is inside the ~10% noise bar, and the six service/config-backed fields the flip moved from
the invariant slice into the per-call varying block (`itemBlacklist`, `rewardItemBlacklist`,
`bossItems`, `seasonalItemTplBlacklist`, `repeatableQuestTemplateIds`, `locationIdMap` — O(KB))
do not resolve above it: the same accepted price flip #1 paid. What is gone outright is the slice
send: pre-flip a pass whose send carried the invariant slice paid ~47-90 ms and ~10.6 MB; post-flip
no DB-derived data crosses per call at all, and warm alloc/run holds at 0.1 MB. The cold arm's
~453-457 ms cold−warm delta is the whole per-*mutation* cost (4-root projection + FFI copy +
parse + both families' view derivation), not a per-send cost; the forced publish reads 471.64 ms against
0287c7e's 3-root 456.73 ms, the locations root's share sitting inside the two arms' overlap.
Measured in the same sitting, ragfair's own arms did not move for gaining the fourth root (full
pass native 493.28 ms against flip #1's 520.25 ms).

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

`cecdd5c` — 2026-08-19, post resident-DB flip (phase 1 flip #5, ABI 26). Same fixture shape and
workload. The native arms now ride the resident DB: `DbPublisher.EnsureCurrent` publishes the
five roots once (absorbed in warmup — the hideout root, `production.scavRecipes` only, joined
the envelope for this flip) and every timed pass sends `{epoch, varying}` only. The fixture
gained the publish cold/warm split the quest fixture established: cold bumps
`DatabaseMutationStamp` before every run, so each pass republishes all five roots first. `Build`
survives as the whole epoch-0 override request an ineligible (modded, untrusted) send pays per
call — the same composition it always measured — and `BuildViewsOverride` is timed on its own
as its database-views half.

| Recipe | End products (common/rare/superrare) | Arm | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|---|---|
| `6271093e…` moonshine | 0 / 1 / 3-5 | native, publish **warm** | 1.58 ms | 1.60 ms | 1.77 ms | 1.52 ms | 2.64 ms |
| `6271093e…` moonshine | 0 / 1 / 3-5 | legacy (C# 4.1.2) | 1.46 ms | 1.51 ms | 1.66 ms | 1.40 ms | 4.02 ms |
| `6271093e…` moonshine | 0 / 1 / 3-5 | native, publish **cold** (stamp bumped per run) | 733.68 ms | 736.03 ms | 739.59 ms | 712.26 ms | 791.14 ms |
| `62710a8c…` 15,000 ₽ | 1 / 1-3 / 0 | native, publish **warm** | 1.77 ms | 1.68 ms | 2.02 ms | 1.60 ms | 3.13 ms |
| `62710a8c…` 15,000 ₽ | 1 / 1-3 / 0 | legacy (C# 4.1.2) | 1.46 ms | 1.49 ms | 1.50 ms | 1.41 ms | 2.18 ms |
| `62710a8c…` 15,000 ₽ | 1 / 1-3 / 0 | native, publish **cold** (stamp bumped per run) | 737.83 ms | 738.10 ms | 744.92 ms | 714.55 ms | 794.49 ms |
| `62710974…` 2,500 ₽ | 1-2 / 0-1 / 0 | **native, publish warm** | **1.68 ms** | **1.66 ms** | 2.00 ms | 1.61 ms | 3.05 ms |
| `62710974…` 2,500 ₽ | 1-2 / 0-1 / 0 | **legacy (C# 4.1.2)** | **0.43 ms** | **0.42 ms** | 0.46 ms | 0.38 ms | 1.04 ms |
| `62710974…` 2,500 ₽ | 1-2 / 0-1 / 0 | native, publish **cold** (stamp bumped per run) | 744.80 ms | 743.57 ms | 749.01 ms | 717.88 ms | 809.29 ms |
| `62710a69…` 95,000 ₽ | 0 / 1-3 / 1-2 | **native, publish warm** | **1.74 ms** | **1.68 ms** | 2.19 ms | 1.60 ms | 4.50 ms |
| `62710a69…` 95,000 ₽ | 0 / 1-3 / 1-2 | **legacy (C# 4.1.2)** | **0.43 ms** | **0.43 ms** | 0.48 ms | 0.39 ms | 1.11 ms |
| `62710a69…` 95,000 ₽ | 0 / 1-3 / 1-2 | native, publish **cold** (stamp bumped per run) | 740.42 ms | 736.29 ms | 751.10 ms | 722.28 ms | 805.53 ms |
| `62710a0e…` intel folder | 0 / 2-4 / 2-3 | **native, publish warm** | **1.81 ms** | **1.88 ms** | 2.20 ms | 1.58 ms | 5.93 ms |
| `62710a0e…` intel folder | 0 / 2-4 / 2-3 | **legacy (C# 4.1.2)** | **0.46 ms** | **0.47 ms** | 0.85 ms | 0.43 ms | 7.37 ms |
| `62710a0e…` intel folder | 0 / 2-4 / 2-3 | native, publish **cold** (stamp bumped per run) | 740.86 ms | 741.30 ms | 754.32 ms | 724.95 ms | 806.96 ms |
| — | — | `Build` (request only) | 13.99 ms | 11.93 ms | 12.28 ms | 5.37 ms | 27.32 ms |
| — | — | `BuildViewsOverride` only | 6.78 ms | 6.41 ms | 10.91 ms | 4.98 ms | 37.80 ms |

Steady state (positions 3-5): **~1.7 ms native warm against ~0.44 ms legacy — ~0.25x, ~4x
slower**, against the pre-flip **0.011x, ~88x slower**. The warm path shed ~37.5 of its ~39 ms
(39.35 → 1.68 ms on the settled 2,500 ₽ recipe, **23.4x**, 23.5x on the second invocation):
the ~7.75 ms C# projection, its serialise and the native-side parse of the whole views bundle,
all gone from eligible sends — the claim the flip made, confirmed. What remains against legacy
is the whole ~1.7 ms pass — varying-block build and serialise, the FFI round trip and the
native generation, unsplit by this fixture — against ~0.44 ms of plain C#. The positional
gradient the baseline noted is now legacy-only: the warm arm reads flat ~1.6-1.9 ms across all
five recipes while legacy's first two positions still read ~1.5 ms against its settled
~0.44 ms. The cold arm's ~734-745 ms cold−warm delta is the whole per-*mutation* cost (5-root
projection + FFI copy + parse + every family's view derivation), flat across recipes — read
against flip #4's 730.08 ms forced publish, the hideout root's share (`production.scavRecipes`,
O(KB)) sits inside the noise. The projection arms are bimodal (spread 5-38 ms across both
invocations): `Build` 13.99 / 11.93 ms and `BuildViewsOverride` 6.78 / 6.41 ms against the old
`Build`'s 7.75 / 6.86 ms — the ineligible caller's per-call price held.

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

Post-flip (#3, resident DB) — `4f66860`, 2026-08-18. Same fixture, same shape (4,673 templates
in; 4,553 tpls / 20,218 ancestor ids, plus 120 root node ids out):

| Arm | median | mean | min | max |
|---|---|---|---|---|
| **native (rust, resident)** | **21.69 ms** | 22.53 ms | 17.49 ms | 29.64 ms |
| **legacy (C# 4.1.2)** | **6.97 ms** | 8.29 ms | 5.12 ms | 20.58 ms |
| `Build` (request only) | 0.28 ms | 0.47 ms | 0.24 ms | 3.56 ms |

Speedup **0.32x** (pre-flip 0.23x): eligible sends now ride the resident templates root — an
`{epoch}`-only wire, no per-send projection or whole-table request serialisation — while ineligible sends
carry the old projection as `viewsOverride` at the pre-flip cost. The fixture's two warmups
absorb the first send, so the publish-carrying first-send cost did not surface as a number of its
own here; the per-mutation four-root publish is the repeatable-quests section's ~471 ms figure.

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

Post-flip (#3, resident DB) — `4f66860`, 2026-08-18. Same fixture, same shape (4,673 templates
in; 4,673 tpls / 63,530 linked ids out), native measured first as in the pre-flip main table:

| Arm | median | mean | min | max |
|---|---|---|---|---|
| **native (rust, resident)** | **60.39 ms** | 56.66 ms | 37.33 ms | 70.53 ms |
| **legacy (C# 4.1.2)** | **21.49 ms** | 21.98 ms | 17.41 ms | 26.80 ms |
| `Build` (request only) | 5.18 ms | 7.55 ms | 2.93 ms | 18.48 ms |

Speedup **0.36x** as the fixture measures it (pre-flip 0.16x): eligible sends now ride the
resident templates root (`{epoch}`-only wire), ineligible sends carry the old projection as
`viewsOverride` at the pre-flip cost. The legacy arm is code-identical to pre-flip, so its move
(21.49 against 15.38 ms) is the sitting — read the native delta against that bar. The warmups
absorb the first send, so no distinct publish-carrying first-send number surfaced.

## Caveats

- **Spread is wide.** Treat differences under ~10% between runs as noise; no outlier rejection, no
  confidence intervals.
- **One map** for location loot: `bigmap`.
- **A mod changes what is measured.** `UseLegacyPath()` also returns true when any frozen 4.1.2
  protected member carries a live Harmony patch, so such a server runs legacy regardless of these
  numbers.
- **No historical series.** Figures cannot be diffed against an earlier commit without re-running it
  on the same machine.
