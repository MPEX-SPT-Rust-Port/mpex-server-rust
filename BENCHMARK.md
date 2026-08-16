# Benchmarks

Every benchmark in this repo measures a Rust port against the retained 4.1.2 C# implementation it
replaced. They live in `Testing/UnitTests/Tests/Generators/` as `[Explicit]` NUnit fixtures, so a
plain `dotnet test` never runs them:

| Fixture | Measures |
|---|---|
| `LootBenchmarkTests.cs` | location loot — elapsed time per call, allocation, peak RSS |
| `RewardLootBenchmarkTests.cs` | airdrop loot — elapsed time per call; numbers recorded in the reward-loot section of [ARCHITECTURE.md](ARCHITECTURE.md), not here |
| `BotBenchmarkTests.cs` | one bot's inventory — elapsed time per bot, with the payload projection timed separately |
| `RagfairBenchmarkTests.cs` | a dynamic flea offer pass — elapsed time per pass, with the payload projection timed separately |
| `RepeatableQuestBenchmarkTests.cs` | one repeatable quest of each type — elapsed time per quest, native measured with the invariant slice cold and warm, with the slice projection timed separately |

There are no `cargo bench` targets. `Containerfile.dev` mentions `cargo bench` as a toolchain
capability, not as a suite that exists.

## Running them

Release only — the cargo dev profile makes Debug numbers meaningless.

```bash
scripts/decompress-assets.sh    # once, if SPT_Data/database/locations/*/looseLoot.json is missing

# head-to-head elapsed time per call and allocation
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

# one quest per type, legacy against native with the slice cold and warm, plus the slice on its own
dotnet test -c Release --filter "FullyQualifiedName~RepeatableQuestBenchmarkTests" \
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
runs — collecting inside the phase would distort the per-call elapsed time the fixture exists to
measure. Median is the headline figure; mean, min and max are reported alongside because run-to-run
spread is wide (see Caveats).

**Allocation.** `GC.GetTotalAllocatedBytes(precise: true)` across the phase, divided by run count.
This is managed allocation only. Whatever the native side allocates on the Rust heap never reaches
the GC, so on the native path this figure does not include it — peak RSS is the only number that
sees both sides.

**Peak RSS needs its own process.** `Environment.WorkingSet` is process-wide, and the managed heap
does not hand pages back to the OS. Whichever path runs second inherits everything the first left
resident, so its absolute peak reflects both paths combined. That is why `NativePeakWorkingSet` and
`LegacyPeakWorkingSet` are separate tests meant to be run one per `dotnet test` invocation and
compared only against each other. Inside the head-to-head test the comparable figure is
`WorkingSetGrowthMb` (growth over that phase), not the absolute peak.

**Binary sizes** are reported from `AppContext.BaseDirectory` at the end of the head-to-head run.
The legacy C# path still ships inside `SPTarkov.Server.Core.dll` — it is the frozen mod contract —
so the native library is additive on disk.

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

### Elapsed time per call and allocation

| Path | median | mean | min | max | alloc/run | GC gen0/1/2 |
|---|---|---|---|---|---|---|
| native (rust) | **333.23 ms** | 341.91 ms | 308.92 ms | 423.43 ms | 100.5 MB | 11/10/6 |
| legacy (C# 4.1.2) | 871.17 ms | 864.28 ms | 712.36 ms | 1005.25 ms | 315.1 MB | 147/141/16 |

Speedup on median elapsed time per call: **2.61x**. Managed allocation: **3.14x** less.

### Peak working set

Separate `dotnet test` invocations, as above.

| Path | process peak RSS | settled RSS | managed heap | alloc/run |
|---|---|---|---|---|
| native (rust) | 1456 MB | 1445 MB | 327 MB | 100.2 MB |
| legacy (C# 4.1.2) | 1185 MB | 941 MB | 382 MB | 314.3 MB |

Native peak RSS is ~270 MB higher than legacy's. Native's settled RSS barely drops after a forced
collection because those pages are the Rust heap, invisible to the GC; the legacy path's settled RSS
falls by ~240 MB after the same collection.

### Binary sizes

| | |
|---|---|
| `libspt_native.so` | 1.95 MB |
| `SPTarkov.Server.Core.dll` | 5.43 MB |

## Results — bot generation

Recorded 2026-08-13 on `94fe128` plus the working-tree fix to the projection phase's timed region.
Different machine from the location-loot figures above — the two result sets are not comparable to
each other.

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
is timed on its own in a third phase, with the same hoisting so the clone is not counted twice.

| Role | Path | median | mean | min | max |
|---|---|---|---|---|---|
| assault | native (rust) | **88.04 ms** | 84.06 ms | 48.62 ms | 119.37 ms |
| assault | legacy (C# 4.1.2) | 1.36 ms | 2.18 ms | 0.59 ms | 12.99 ms |
| assault | `BuildRequest` only | 9.89 ms | 9.88 ms | 5.20 ms | 25.14 ms |
| usec | native (rust) | **54.43 ms** | 55.47 ms | 47.49 ms | 71.12 ms |
| usec | legacy (C# 4.1.2) | 1.20 ms | 1.59 ms | 0.82 ms | 5.22 ms |
| usec | `BuildRequest` only | 5.09 ms | 5.41 ms | 4.83 ms | 8.14 ms |

Speedup on median elapsed time per bot: **0.02x** for both roles. Projection share of the native median:
**11.2%** (assault), **9.4%** (usec).

**Measurement order.** Whichever role is timed first pays roughly 40 ms of extra serialise time
against a still-growing heap; reversing the order swaps the two figures. Steady state is ~51-54 ms
per bot for both roles.

A phase-by-phase split of a ~51 ms bot: `BuildRequest` 10.3 ms, C# serialise 21.6 ms, Rust
deserialise 14.9 ms, Rust generation 2.9 ms, FFI plus result deserialise ~1.4 ms. The native path
hands the whole items table across the boundary per bot: 4,673 `ItemView`s plus ~5,000 nested
slot/grid/cartridge/chamber views (~9.7k objects), and every global preset. Payload transport is
~92% of the per-bot cost, generation ~5.7%.

`BotWaveBatcher` sends the shared views once per wave instead of once per bot, so a wave of N
divides most of the request between N bots. Against the `.AsParallel()` per-bot loop
(`BotController.GenerateBotWave`, the baseline production runs), a single-threaded batch measured
**~1.7x** at wave 10 (12.5 → 7.3 ms/bot) and **~2.4-2.5x** at wave 45 (13.2 → 5.2-5.5 ms/bot).
Under rayon the batch loop measures **1.9-2.2x** at wave 45 and **~1.5x** at wave 10.
`BotBatchTests.WaveCostPerBot` reports the serial, parallel and batched arms together. Two payload
trims: sixteen always-default item members (−640 KB / −13.8% on `assault`, −13.3% on `pmcUSEC`) and
`slots[].required` (−49 KB), taking the `pmcUSEC` request to ~4.18 MB — ~4-6% on the per-bot path,
~0% batched (the batch already divides the block they shrink).

Bot-specific caveats, on top of the general ones below:

- **Two roles, one level, one difficulty.** `assault` and level-1 `pmcUSEC`, `normal`, `standard`
  game version. A boss or a high-level PMC is not measured.
- **No allocation or RSS figures.** Only elapsed time per bot. The native path allocates ~9.7k view objects
  per bot on the managed heap before serialising them; this is unmeasured here.
- **The legacy figures include a warm `BotLootCacheService`.** Hydration happens during the warmup
  runs. A cold first bot of a role costs more on both paths.

### Batched wave, rayon batch loop — measured

Recorded 2026-08-14 on `aa733a7`, same machine and environment table as the bot-generation figures
above (AMD Ryzen 5 5600H, 6C/12T; Release; .NET SDK 10.0.110; rustc 1.97.1). Source:
`BotBatchTests.WaveCostPerBot` (`[Explicit]`, `dotnet test -c Release --filter "Name=WaveCostPerBot"`),
medians of 5 runs per arm per wave. Two independent invocations of the whole test are reported.

| wave | serial per-bot | `.AsParallel()` per-bot | batched (rayon) | batched vs parallel | pre-rayon batched |
|---|---|---|---|---|---|
| 45 | 44.10 / 43.16 ms | 12.33 / 11.98 ms | **5.73 / 6.22 ms** | 2.15x / 1.92x | 5.2-5.5 ms |
| 20 | 43.94 / 42.83 ms | 12.72 / 12.26 ms | **6.02 / 5.95 ms** | 2.11x / 2.06x | not recorded |
| 10 | 43.11 / 42.67 ms | 11.67 / 11.85 ms | **7.63 / 7.85 ms** | 1.53x / 1.51x | 7.3 ms |
| 5 | 42.48 / 42.71 ms | 13.89 / 13.62 ms | **11.83 / 11.97 ms** | 1.17x / 1.14x | not recorded |
| 1 | 41.42 / 41.46 ms | 43.29 / 43.25 ms | **42.72 / 41.81 ms** | 1.01x / 1.03x | not recorded |

All figures are ms per bot. Request bytes per bot over the same waves: 3.78 MiB single-bot, against
0.30 (wave 45), 0.40 (20), 0.58 (10), 0.94 (5), 3.78 (1) MiB batched.

Rayon's batched figures (5.73/6.22 ms/bot at wave 45, 7.63/7.85 at wave 10) sit slightly above the
pre-rayon single-threaded batch (5.2-5.5 ms, 7.3 ms) at the same waves.

## Results — ragfair offer generation

Recorded 2026-08-15 on `6d975e4` — the stamp-gated invariant-slice cache
(`.superpowers/sdd/2026-08-15-db-mutation-stamp-ragfair-slice-cache/`), re-measuring the figures the
native-parity effort left on `df07d54`. Same machine as the bot-generation figures above, not the
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
every sellable template gets its offers — ~58k offers with full item trees, of which the holder's
per-template cap accepts ~24k. *Regeneration pass*: 1,400 cloned single-item lists (the configured
`expiredOfferThreshold`), the shape `RagfairServer.ProcessExpiredFleaOffers` hands over.
`RagfairPayloadProjection.BuildRequest` is timed on its own in a third phase per scenario. A fourth
scenario times the regeneration pass with the invariant-slice cache forced cold against the same
pass left warm (see *The slice cache*).

**Pre-populated flea.** The test harness runs every `IOnLoad`, so `RagfairCallbacks.OnLoadAsync` →
`RagfairServer.Load()` has already generated a complete set of dynamic offers into the holder before
the fixture takes its baseline snapshot. The fixture removes everything each run adds, outside the
stopwatch, but preserves that pre-existing set, so every timed run is a pass over an
already-populated flea, on both paths. This is symmetric across paths; it affects what the offer
column means (see below).

### Elapsed time per pass

| Scenario | Path | median | mean | min | max | offers accepted | alloc/run |
|---|---|---|---|---|---|---|---|
| full pass | native (rust) | **670.63 ms** | 645.77 ms | 556.87 ms | 739.75 ms | 24,005 | 206.6 MB |
| full pass | legacy (C# 4.1.2) | 460.29 ms | 471.43 ms | 432.00 ms | 540.32 ms | 24,276 | 283.3 MB |
| full pass | `BuildRequest` only | 14.08 ms | 13.59 ms | 9.77 ms | 15.68 ms | — | 7.1 MB |
| regeneration | native (rust) | **15.19 ms** | 15.31 ms | 10.91 ms | 21.54 ms | 867 | 5.2 MB |
| regeneration | legacy (C# 4.1.2) | 9.92 ms | 9.60 ms | 5.46 ms | 13.86 ms | 859 | 5.9 MB |
| regeneration | `BuildRequest` only | 12.90 ms | 13.79 ms | 8.44 ms | 18.70 ms | — | 7.1 MB |
| regeneration | native, cache **cold** | 77.94 ms | 75.72 ms | 66.87 ms | 82.32 ms | 876 | 17.6 MB |
| regeneration | native, cache **warm** | **11.46 ms** | 13.09 ms | 10.08 ms | 16.85 ms | 863 | 5.2 MB |

Speedup on median elapsed time per pass: **0.69x** on the full pass (**1.45x slower**) and **0.65x** on the
regeneration pass (**1.53x slower**, was 7.3x slower before the slice cache). Projection share of
the native median: **2.1%** (full pass), **84.9%** (regeneration) — the projection phase builds the
whole slice every time it is timed, so on a warm regeneration pass it measures work the generation
pass no longer does.

### The slice cache

The regeneration pass's cost was the 5.8 MB call-invariant request slice being rebuilt and
re-serialised for a pass that only regenerates 1,400 offers. Since `6d975e4` the slice is sent only
when `DatabaseMutationStamp` changed since the last send; the native side keeps the parsed copy keyed
by that stamp, and the send is varying-fields-only otherwise. A hit skips the C# projection (both
price maps over the whole items table, every preset, the items view), the MessagePack serialise of
the result, and the native parse of it.

| Regeneration pass | median | alloc/run |
|---|---|---|
| cache cold (stamp bumped before every run — the pre-`6d975e4` behaviour) | 77.94 ms | 17.6 MB |
| **cache warm (the shipped path)** | **11.46 ms** | **5.2 MB** |

**6.80x** on the median; warm is **14.7%** of cold, and warm allocates **0.30x** what cold does. The
`regeneration | native (rust)` row above (15.19 ms) and this warm row (11.46 ms) measure the same
configuration in two different phases of the same run; the difference is run-to-run spread on a
~12 ms pass. The cold arm reproduces the old 74.59 ms baseline within noise. The full pass is
unchanged — its slice is 2.1% of the pass — and its 670 ms figure is the same 626 ms measurement one
run's spread later.

The offer column is **offers accepted into an already-full flea**, not offers the path produced. The
full pass *produces* ~58k offers — the framed response captured for the deserialize attribution below
held 58,137 frames — and `RagfairOfferHolder.AddOffer` drops a fake-player offer once that template
is at its cap, which is what leaves ~24k. The cap is re-rolled per call
(`RagfairServerHelper.GetOfferCountByBaseType` is a `RandomUtil.GetInt` over the configured range).
1,400 distinct expired templates land as ~870 offers because of the pre-filled holder; against an
empty holder the same input would land as ~1,400. The two paths land within ~1% of each other on the
full pass.

Wire volume, both encodings measured off the same generated batch (a captured 5.8 MB request run
straight through `generate_dynamic_offers`, 57,842 offers, framed both ways by the crate's own
serializers):

| Envelope | Bytes |
|---|---|
| stage B — framed JSON (ABI 9) | 43.2 MB |
| **stage C — framed MessagePack (ABI 10), the shipped path** | **35.4 MB** |

MessagePack is **0.82x** the JSON wire, ~18% smaller. The **41.4 MB** figure quoted elsewhere for
this response is the *stage B* capture taken through the C# path for the deserialize attribution
(58,137 frames); it and the 43.2 MB above are the same encoding measured on two different passes.

Offer counts differ between captures — 57,842 here, 58,137 in the attribution capture, 58,558
`RagfairOffer`/nickname instances in its type census. A production pass is unseeded (offer counts are
per-template `RandomUtil.GetInt` rolls), so these are three captures of the same workload varying
~0.7% run to run.

Working set over the timed phase: native grows +443 MB on the full pass against the legacy path's
+0 MB; peak RSS is process-wide and cumulative, so only the growth figures are comparable, and only
within one invocation.

### Checkpoint series — native-parity effort

Each row is one `RagfairBenchmarkTests` invocation, medians, same machine, native and legacy from the
same run. Ratio is native / legacy on the full pass; above 1.00x means native is slower.

| Stage | Commit | Full pass native | Full pass legacy | Ratio | Regen native | Regen legacy | Alloc/run native |
|---|---|---|---|---|---|---|---|
| Baseline | `7fab74d` | 1550 ms | 517 ms | 3.00x | ~103 ms | ~12.8 ms | not recorded |
| A — rayon batch fan-out | `5fefd61` | 884.67 ms | 454.37 ms | 1.95x | 85.42 ms | 9.87 ms | 182.9 MB |
| B — framed response, ABI 9 | `357c7f3` | 676.12 ms | 443.22 ms | 1.53x | 75.00 ms | 10.50 ms | 208.4 MB |
| C — MessagePack payloads, ABI 10 | `d076b39` | 719.39 ms | 441.66 ms | 1.63x | 75.67 ms | 10.78 ms | 358.3 MB |
| C — reader allocation remediation | `a2acb6f` | 663.95 ms | 457.44 ms | 1.45x | 76.14 ms | 10.51 ms | 219.9 MB |
| One-pass fresh-id offer items | `df07d54` | 625.78 ms | 426.57 ms | **1.47x** | 74.59 ms | 10.25 ms | 219.3 MB |
| Stamp-gated slice cache | `6d975e4` | 670.63 ms | 460.29 ms | **1.45x** | **15.19 ms** | 9.92 ms | 206.6 MB |

**Stage A:** the `CreateOffersFromAssort` timer inside the native full pass fell from ~710 ms to
91–103 ms across the run's samples; the full pass fell 1550 → 885 ms.

**Stage C:** 676.12 → 719.39 ms, native alloc/run 208.4 → 358.3 MB (+72%) at a flat offer count,
reproduced back-to-back. Traced to the C# reader: a `ToArray` copy per frame, a fresh
`Utf8JsonWriter` pair per transcoded value, a reflection `GetValue` per item, and ~2M `ReadString`
map-key allocations per pass. Removing all four (`a2acb6f`): 663.95 ms / 219.9 MB.

**One-pass fresh-id change:** full-pass medians have ±25 ms run-to-run spread; an A/B with the change
stashed and unstashed read 1.46x/1.46x before against 1.47x/1.53x after. The `CreateOffersFromAssort`
timer, which measures only the leg the change touches: median 92 → 85 ms.

### Full-pass residual breakdown

Final checkpoint ratio: **1.47x** (625.78 / 426.57 ms); preceding checkpoint: 1.45x.

Full pass native: 1550 → 626 ms across the checkpoint series (2.48x). Native allocation: 219 MB vs
legacy's 283 MB.

Phase breakdown of the residual, measured at real per-pass counts:

- Rust generation: ~85 ms of the ~626 ms pass.
- `BuildRequest`: 13 ms (~2%).
- Converter taxes against the shipped `Parallel.For` deserialize (~305 ms, 41.4 MB / 58,137 frames):
  Ceciler-injected `[JsonExtensionData]` dictionary 3.4 ms, `MongoId` ctor validation over 425,550
  ids 11.1 ms, `string.Intern` on 58,558 nicknames 15.1 ms — ~30 ms combined. (The 3.4 ms is the
  dictionaries' allocation cost measured in isolation, not System.Text.Json's metadata overhead for
  them.)
- Remaining ~275 ms: System.Text.Json binding of 366,851 `Models` instances (`Item` 83,292, `Upd`
  60,325, `RagfairOffer`/`OfferRequirement`/`RagfairOfferUser` 58,558 each, `UpdRepairable` 43,300,
  tail 4,260). Legacy has no equivalent leg — it never crosses a wire.

**GC mode.** `<ServerGarbageCollection>true</ServerGarbageCollection>` is set on
`SPTarkov.Server.csproj` only, not on `UnitTests.csproj`; at ~220 MB per pass the fixture's parallel
deserialize is GC-throttled — single-threaded deserialize (268.6 ms) beat the `Parallel.For` one
(304.8 ms) in the attribution run. Same commit and machine with `DOTNET_gcServer=1`:

```
full pass native (rust)     median=558.95 ms  min=417.63  alloc/run=209.1 MB
full pass legacy (C# 4.1.2) median=329.51 ms  min=300.32  alloc/run=282.6 MB
                            ratio 1.70x (vs 1.53x under workstation GC at the same commit)
```

Server GC takes ~117 ms off native and ~113 ms off legacy. The checkpoint series above was measured
in the fixture's own (workstation) GC mode, for consistency with the baseline it was compared
against.

Regeneration pass: 74.59 ms → 11.46 ms warm via the slice cache (see *The slice cache* above); ~60%
of the pre-cache cost was building and serialising the 5.8 MB call-invariant request slice, against
~1% of a full pass.

Ragfair-specific caveats, on top of the general ones below:

- **Both paths are parallel.** `GenerateDynamicOffersLegacy` fans one `Task.Factory.StartNew` per
  assort entry across the thread pool; since `5fefd61` the native side fans the unseeded batch walk
  across rayon (seeded runs stay sequential for parity), and the C# side deserialises the response
  frames with `Parallel.For`. Both paths use the 12 threads.
- **Every timed run works on a pre-filled flea** (see Workload). Both paths pay the holder's
  per-template cap checks against a full `_fakePlayerOffers` index.
- **n=5 after 1 warmup**, not the 20 the other fixtures use — a full pass is seconds, not
  milliseconds. Native full pass spread is 614-872 ms, ±25 ms run-to-run on the median; changes
  under ~5% of the pass cannot be resolved here — use the `CreateOffersFromAssort` timer the native
  path logs instead.
- **The plain native arms run with the cache warm.** Nothing bumps the stamp between runs, so after
  the cold arm every timed pass is a cache hit.
- **Regeneration input is fixed.** The same 1,400 single-item lists every run, sampled from the head
  of the assort rather than from offers that actually expired.
- **First-call effects are warmed away.** One warmup per phase covers JIT, the native library load
  and the assort generation's first pass.
- **Native is measured first in each scenario**, so the legacy phase inherits a warmer process and a
  heap already grown to the native path's high-water mark.

## Results — repeatable quest generation

Recorded 2026-08-16 on `b0a3e27` plus the working-tree fixture that produced them. Same machine as
the bot-generation and ragfair figures above, not the machine the location-loot figures came from.

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |
| Configuration | Release, n=20 after 2 warmups, per arm per quest type |

**Workload.** One `Generate` call per quest type — `Elimination`, `Completion`, `Exploration`,
`Pickup` — on the live shipped database, at the midpoint of the second shipped level band for that
type, for the first trader whitelisted for it, unseeded the way production runs. The
`QuestTypePool` the controller builds is rebuilt per run outside the stopwatch, because the
generators consume the pool they are handed.

**Three arms per type**, all asserted rather than assumed — the fixture checks `LastPathTaken` and
`RepeatableQuestNativeRequestBuilder.LastSendIncludedSlice` before it reports a number:

- **legacy** — `QuestConfig.ForceLegacyRepeatableQuestGeneration`, the retained 4.1.2 C# path.
- **native, slice cold** — `QuestConfig.DisableNativeRequestCache`, so every send carries the whole
  invariant slice. This is what a server with mods loaded pays: `CacheEligible()` is false whenever
  any mod is loaded unless the user set `TrustNativeRequestCacheWithMods`.
- **native, slice warm** — the stamp is unchanged and the cache is eligible, so the send carries the
  varying fields only and the native side reuses its parsed slice. This is what a stock server pays.

A fourth phase times `BuildInvariantSlice()` on its own.

### Elapsed time per quest

Two full invocations of the fixture; the second median is the error bar on the first.

| Type | Arm | median | median (2nd run) | mean | min | max | alloc/run |
|---|---|---|---|---|---|---|---|
| Elimination | legacy (C# 4.1.2) | 15.60 ms | 16.69 ms | 16.11 ms | 14.60 ms | 21.30 ms | 1.3 MB |
| Elimination | native, slice cold | 87.11 ms | 88.19 ms | 81.91 ms | 43.76 ms | 106.46 ms | 10.9 MB |
| Elimination | **native, slice warm** | **3.38 ms** | 3.38 ms | 3.41 ms | 3.30 ms | 4.17 ms | 0.1 MB |
| Completion | legacy (C# 4.1.2) | 21.64 ms | 21.83 ms | 27.30 ms | 20.24 ms | 89.78 ms | 3.4 MB |
| Completion | native, slice cold | 113.91 ms | 112.82 ms | 115.36 ms | 104.67 ms | 143.02 ms | 10.6 MB |
| Completion | **native, slice warm** | **67.50 ms** | 68.99 ms | 66.96 ms | 61.75 ms | 70.24 ms | 0.1 MB |
| Exploration | legacy (C# 4.1.2) | 3.13 ms | 3.11 ms | 3.40 ms | 2.98 ms | 5.13 ms | 1.2 MB |
| Exploration | native, slice cold | 46.10 ms | 46.75 ms | 47.30 ms | 41.83 ms | 54.83 ms | 10.7 MB |
| Exploration | **native, slice warm** | **3.31 ms** | 3.31 ms | 3.39 ms | 3.26 ms | 4.21 ms | 0.1 MB |
| Pickup | legacy (C# 4.1.2) | 2.91 ms | 2.81 ms | 3.31 ms | 2.75 ms | 7.31 ms | 1.2 MB |
| Pickup | native, slice cold | 45.38 ms | 45.70 ms | 47.45 ms | 40.94 ms | 57.05 ms | 10.7 MB |
| Pickup | **native, slice warm** | **3.20 ms** | 3.20 ms | 3.26 ms | 3.14 ms | 4.00 ms | 0.0 MB |
| — | `BuildInvariantSlice` only | 9.50 ms | 9.75 ms | 11.09 ms | 6.51 ms | 21.66 ms | 6.8 MB |

Speedup on median elapsed time per quest, legacy against the warm native path — the pairing a stock
server runs:

| Type | speedup (run 1 / run 2) | other data points |
|---|---|---|
| Elimination | **4.62x / 4.95x** | third invocation: 6.24x |
| Completion | **0.32x / 0.32x** | |
| Exploration | 0.94x / 0.94x | |
| Pickup | 0.91x / 0.88x | |

A warm native call costs ~3.2 ms regardless of quest type. Exploration and Pickup generate ~2.9 ms of
C# work in the legacy path; Elimination generates 15-17 ms; Completion generates 20-22 ms.

### The slice, and what a C#-side memo could buy

Cost of sending the invariant slice, cold median minus warm median, per send:

| Type | cold − warm (run 1 / run 2) |
|---|---|
| Elimination | 83.73 / 84.82 ms — inflated, see caveats |
| Completion | 46.41 / 43.83 ms |
| Exploration | 42.78 / 43.44 ms |
| Pickup | 42.18 / 42.49 ms |

A full send costs ~43 ms and ~10.6 MB of managed allocation per call, the same figure across quest
types.

Of that ~43 ms, `BuildInvariantSlice()` is **9.50 / 9.75 ms** (6.8 MB of the 10.6 MB): both price
maps over the whole items table, the items view, every default weapon preset, and the boss spawns and
extracts of every location. The remaining ~33 ms and ~3.8 MB is the serialise of the built slice plus
the native side's parse of it — the request is serialised whole, invariant and varying together.

A stamp-keyed C#-side memo of the built slice would remove `BuildInvariantSlice()`'s ~10 ms out of
the ~43 ms full-send cost, for servers ineligible for the native cache (mods loaded without
`TrustNativeRequestCacheWithMods`). A stock server pays none of this.

### Modded-server cold-path ratios

Cold (slice sent every call) against legacy: Completion 5.2x slower, Elimination ~2.9x slower (using
the corrected ~46 ms cold figure, not the 87 ms median in the table), Exploration and Pickup ~15x
slower.

Completion's warm call is 67.5 ms against legacy's 21.6 ms; its cold-minus-warm gap (~44 ms) matches
the other types', so the difference sits inside the native call after the slice is already parsed.

Repeatable-quest-specific caveats, on top of the general ones below:

- **The Elimination cold arm reads ~40 ms high, and it is measurement order.** It is the first
  native phase in the process; its per-run timings sit at 78-107 ms and fall to 60 and 45 ms on the
  last two runs, and its min (43.8 ms) matches the steady cold median of the other three types. Read
  Elimination's cold cost as ~46 ms like the rest.
- **The Completion legacy arm's mean is not its cost.** Its first three timed runs are 96, 64 and 54
  ms against a steady ~21 ms. The median (21.6 ms) is unaffected.
- **The Elimination legacy median moves between invocations** — 15.60, 16.69 and 21.25 ms over three
  runs, each tight within itself (±3 ms). The warm native arm does not move at all (3.38 / 3.38 /
  3.40 ms), so the Elimination speedup is a **4.6-6.2x** band, not just the 4.62x the first table row
  reads.
- **One band, one trader, unseeded.** The midpoint of the second shipped level band and the first
  whitelisted trader per type. An unseeded run draws a different quest every time; the spread columns
  include that variation.
- **No RSS figures**, and the warm arms' allocation rounds to 0.0-0.1 MB, so treat those as "under
  100 KB" rather than as measurements. `alloc/run` also includes each iteration's `BuildPool()` —
  itself under ~100 KB, and it cancels out of the cold-minus-warm delta.
- **Workstation GC**, as with the ragfair fixture: `<ServerGarbageCollection>` is set on
  `SPTarkov.Server.csproj`, not on `UnitTests.csproj`.

## Caveats

- **Elapsed-time spread is wide** — native min-to-max is 309–423 ms on an otherwise idle machine.
  Treat differences under ~10% between runs as noise; the fixture does no outlier rejection and
  reports no confidence interval.
- **One map.** `bigmap` only. Maps differ enormously in loot volume.
- **A mod changes what is measured.** `UseLegacyPath()` also returns true when any frozen 4.1.2
  protected member carries a live Harmony patch. With such a mod loaded the server runs the legacy
  path in production regardless of these numbers.
- **No historical series.** Nothing records past results, so these figures cannot be diffed against
  an earlier commit without re-running that commit on the same machine.
