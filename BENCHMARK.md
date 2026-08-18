# Benchmarks

Every benchmark in this repo measures a Rust port against the retained 4.1.2 C# implementation it
replaced. They live under `Testing/UnitTests/Tests/` as `[Explicit]` NUnit fixtures, so a plain
`dotnet test` never runs them:

| Fixture | Measures |
|---|---|
| `LootBenchmarkTests.cs` | location loot — elapsed time per call, allocation, peak RSS |
| `RewardLootBenchmarkTests.cs` | airdrop loot — elapsed time per call; numbers recorded in the reward-loot section of [ARCHITECTURE.md](ARCHITECTURE.md), not here |
| `BotBenchmarkTests.cs` | one bot's inventory — elapsed time per bot, with the payload projection timed separately |
| `RagfairBenchmarkTests.cs` | a dynamic flea offer pass — elapsed time per pass, with the payload projection timed separately |
| `RepeatableQuestBenchmarkTests.cs` | one repeatable quest of each type — elapsed time per quest, native measured with the invariant slice cold and warm, with the slice projection timed separately |
| `ScavCaseBenchmarkTests.cs` | one scav case of each shipped recipe — elapsed time per call, with the request projection timed separately |
| `ItemBaseClassBenchmarkTests.cs` | one bulk item base class cache build over the shipped items table — elapsed time per hydrate, with the request projection timed separately |
| `RagfairLinkedItemBenchmarkTests.cs` | one ragfair linked item table build over the shipped items table — elapsed time per build, with the request projection timed separately |

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

# one scav case per shipped recipe, both paths, plus the request projection on its own
dotnet test -c Release --filter "FullyQualifiedName~ScavCaseBenchmarkTests" \
  --logger "console;verbosity=detailed"

# one bulk base class cache build, both paths, plus the request projection on its own
dotnet test -c Release --filter "FullyQualifiedName~ItemBaseClassBenchmarkTests" \
  --logger "console;verbosity=detailed"

# one linked item table build, both paths, plus the request projection on its own
dotnet test -c Release --filter "FullyQualifiedName~RagfairLinkedItemBenchmarkTests" \
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

The native arm above is the raw path, which splices `looseLoot.json`'s bytes in unparsed. A
registered `LazyLoad` transformer (seasonal events, mods) forces the typed path instead, and that
one is the slowest of the three: **~1347 ms** per raid start for `bigmap`, against ~345 ms raw and
the 929 ms C# it replaced. Older figures, carried over from RUST-ROADMAP.md rather than re-measured
alongside the table above.

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

Recorded 2026-08-16 on `1819c0c` plus the single-walk base-class fix in
`quest/completion.rs`, and the working-tree fixture that produced them. Same machine as the
bot-generation and ragfair figures above, not the machine the location-loot figures came from.

Supersedes the figures taken on `b0a3e27`, where Completion's warm call read 67.50 ms against a
legacy 21.64 ms — 0.32x, the one native quest arm slower than the C# it replaced. See
[What the Completion figures used to be](#what-the-completion-figures-used-to-be).

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
| Elimination | legacy (C# 4.1.2) | 16.67 ms | 21.39 ms | 17.42 ms | 15.20 ms | 22.24 ms | 1.3 MB |
| Elimination | native, slice cold | 91.32 ms | 92.95 ms | 88.35 ms | 45.21 ms | 107.36 ms | 10.9 MB |
| Elimination | **native, slice warm** | **3.52 ms** | 3.38 ms | 4.17 ms | 3.35 ms | 6.55 ms | 0.1 MB |
| Completion | legacy (C# 4.1.2) | 23.31 ms | 22.72 ms | 40.87 ms | 19.93 ms | 100.59 ms | 3.4 MB |
| Completion | native, slice cold | 57.50 ms | 57.54 ms | 59.59 ms | 50.31 ms | 86.84 ms | 10.6 MB |
| Completion | **native, slice warm** | **12.80 ms** | 13.57 ms | 12.88 ms | 11.98 ms | 14.18 ms | 0.1 MB |
| Exploration | legacy (C# 4.1.2) | 3.14 ms | 3.37 ms | 3.43 ms | 3.01 ms | 6.30 ms | 1.3 MB |
| Exploration | native, slice cold | 47.92 ms | 47.06 ms | 48.36 ms | 42.48 ms | 56.06 ms | 10.7 MB |
| Exploration | **native, slice warm** | **3.38 ms** | 3.26 ms | 3.52 ms | 3.27 ms | 4.42 ms | 0.1 MB |
| Pickup | legacy (C# 4.1.2) | 2.94 ms | 3.10 ms | 3.65 ms | 2.78 ms | 9.90 ms | 1.3 MB |
| Pickup | native, slice cold | 46.84 ms | 45.23 ms | 48.71 ms | 42.24 ms | 61.07 ms | 10.7 MB |
| Pickup | **native, slice warm** | **3.32 ms** | 3.16 ms | 3.35 ms | 3.17 ms | 4.16 ms | 0.0 MB |
| — | `BuildInvariantSlice` only | 10.74 ms | 10.00 ms | 11.99 ms | 6.62 ms | 19.91 ms | 6.8 MB |

Speedup on median elapsed time per quest, legacy against the warm native path — the pairing a stock
server runs:

| Type | speedup (run 1 / run 2) | other data points |
|---|---|---|
| Elimination | **4.74x / 6.33x** | earlier invocations: 4.62x, 4.95x, 6.24x |
| Completion | **1.82x / 1.67x** | |
| Exploration | 0.93x / 1.03x | |
| Pickup | 0.89x / 0.98x | |

A warm native call costs ~3.3 ms for Elimination, Exploration and Pickup alike — the work is the
FFI round trip, not the quest. Completion was the exception at ~13 ms, because it is the only type
that filters the whole item table per call; it has since come down to ~5 ms, see
[What still costs](#what-still-costs).

Exploration and Pickup generate ~3 ms of C# work in the legacy path, so native cannot beat them and
lands at parity; Elimination generates 15-21 ms and Completion 20-23 ms, which is where the wins
come from.

### The slice, and what a C#-side memo could buy

Cost of sending the invariant slice, cold median minus warm median, per send:

| Type | cold − warm (run 1 / run 2) |
|---|---|
| Elimination | 87.80 / 89.58 ms — inflated, see caveats |
| Completion | 44.70 / 43.97 ms |
| Exploration | 44.55 / 43.80 ms |
| Pickup | 43.52 / 42.07 ms |

A full send costs ~43 ms and ~10.6 MB of managed allocation per call, the same figure across quest
types.

Of that ~43 ms, `BuildInvariantSlice()` is **10.74 / 10.00 ms** (6.8 MB of the 10.6 MB): both price
maps over the whole items table, the items view, every default weapon preset, and the boss spawns and
extracts of every location. The remaining ~33 ms and ~3.8 MB is the serialise of the built slice plus
the native side's parse of it — the request is serialised whole, invariant and varying together.

A stamp-keyed C#-side memo of the built slice would remove `BuildInvariantSlice()`'s ~10 ms out of
the ~43 ms full-send cost, for servers ineligible for the native cache (mods loaded without
`TrustNativeRequestCacheWithMods`). A stock server pays none of this.

### What still costs

Completion's warm call was ~13 ms against the other three types' ~3.3 ms. The ~10 ms difference was
`GetItemsToRetrievePool` (`:125-155`), which runs `IsValidRewardItem` over all 4,673 templates, and
`GetWhitelistedItemSelection` (`:188-214`), which tests every survivor against a 137-entry
whitelist. C# answers each base-class test from `ItemBaseClassService`'s ancestor set, precomputed
once at startup and O(1) per lookup; `item_helper::is_of_baseclasses` walked the parent chain live
on every call.

Recorded 2026-08-16 on `8963a41`, same machine and configuration as the table above. Two full
invocations, warm medians:

| Type | `1819c0c` | `52a27e0` ancestor cache | `8963a41` set probe |
|---|---|---|---|
| Elimination | 3.52 / 3.38 ms | 2.64 / 2.63 ms | 2.62 / 2.70 ms |
| **Completion** | **12.80 / 13.57 ms** | **11.74 / 13.50 ms** | **5.02 / 4.75 ms** |
| Exploration | 3.38 / 3.26 ms | 2.34 / 2.47 ms | 2.42 / 2.33 ms |
| Pickup | 3.32 / 3.16 ms | 2.27 / 2.38 ms | 2.35 / 2.28 ms |

Completion's speedup over legacy is **4.89x** on the second invocation, from 1.82x / 1.67x. The
first invocation reads 11.30x only because its legacy arm's early runs contaminated the median as
well as the mean (56.77 ms against a 19.94 ms min); see the caveat below.

Read Completion against the same-session floor, not across sessions: the other three types are
~25-30% cheaper in both later sessions than in the `1819c0c` one, which is process and machine state
— they make almost no base-class calls. Completion against that floor is **5.0 vs 2.4 ms**, from
**12.8 vs 3.4**. Cold, legacy and `BuildInvariantSlice` medians and every alloc/run figure stayed
within the spread of the table above.

**The flattened ancestor cache alone did not move it.** `52a27e0` built
`ItemBaseClassService`'s map once per cached invariant slice and answered all 19 quest and ragfair
call sites from it, and Completion warm stayed at 11.74 / 13.50 ms. The cache removed the parent
chain *walk* — about four `IndexMap` probes per item — which was never the dominant term. What
remained was `is_of_baseclasses`' linear scan of the candidate list at each link: with a 137-entry
whitelist that is `chain_len × 137` string comparisons per item, unchanged by any amount of caching.
Measured in isolation on the shipped table by
`tests/completion_whitelist_baseclass.rs`, which reports all three formulations: **67.6 ms** for a
walk per candidate, **10.1 ms** for one walk with a slice scan, **1.3 ms** with the candidates in a
set. `8963a41` added `ItemBaseClassCache::is_of_baseclasses_set` and used it at the two Completion
whitelist/blacklist sites, where the candidate list is already a `HashSet` and every other caller
passes one to fourteen ids. Same enumeration direction as the slice form — the short ancestor chain
probes the candidates, not the C# `Overlaps` direction — so the answers are identical.

`FOLLOWUPS.md` expected the cache to "subsume both effects". It did not: the two are independent,
and the scan was the whole of it. The cache is what makes the chain lookup O(1) and is kept on that
basis, but on its own it bought nothing measurable at any current call site.

A residual ~2.5 ms over the other three types remains, presumed to be `GetItemsToRetrievePool`'s
pass over all 4,673 templates now that the whitelist filter no longer dominates it. That
attribution is inferred from what is left, not measured — the same kind of claim that proved wrong
above, so treat it as a starting point rather than a finding.

Bot and ragfair were re-measured on `8963a41` and neither moved. Ragfair matches on every arm: full
pass 630.90 ms native / 440.85 legacy, regeneration 14.49 / 10.41, cache cold 80.92 against warm
9.91, and every alloc/run figure unchanged to within 0.1 MB of the section above. Bot per-bot
medians are 84.05 ms (`assault`) and 53.99 ms (`usec`) against the recorded 88.04 and 54.43. Bot's `BuildRequest` arm
read 15.23 / 15.58 ms against the recorded 9.89 / 5.09, which is not within spread — it is the
payload projection, untouched by this change and unmeasured since, so read it as this session's
figure rather than a regression. Both results are what was expected: bot and loot deliberately keep
the walk, and the bot path is ~92% payload transport regardless.

### What the Completion figures used to be

On `b0a3e27` the same fixture read **67.50 / 68.99 ms** warm and 113.91 ms cold, a 0.32x speedup —
the only native quest arm slower than the C# path it replaced.

`GetWhitelistedItemSelection` (`:365-371`) tests every whitelisted candidate against every item in
the pool. C# affords that shape because each `IsOfBaseclass` is an O(1) cache hit; the Rust port
kept the shape but answered each call with a fresh parent-chain walk, so the filter restarted a full
walk for each of the 137 whitelisted candidates, per item, to keep 135 items out of 4,673. Measured
in isolation against the real table: **66.9 ms**, against 9.85 ms for one walk testing every
candidate at each link.

Testing all candidates in a single walk took the warm call to ~13 ms and the cold call to ~57.5 ms.
The blacklist twin (`:222-246`) had the same shape and was changed with it, though the shipped
config sets `useBlacklist: false`, so it cost nothing in these figures.

Guarded by `quest::completion::tests::the_whitelist_filter_walks_each_item_chain_once`, which pins
the production filter against a one-walk reference measured in the same process, and by
`tests/completion_whitelist_baseclass.rs`.

### Modded-server cold-path ratios

Cold (slice sent every call) against legacy: Completion ~2.5x slower, Elimination ~2.8x slower
(using the corrected ~45 ms cold figure, not the 91 ms median in the table), Exploration and Pickup
~15x slower.

Repeatable-quest-specific caveats, on top of the general ones below:

- **The Elimination cold arm reads ~45 ms high, and it is measurement order.** It is the first
  native phase in the process; its per-run timings start at 100+ ms and fall away, and its min
  (45.21 / 44.15 ms) matches the steady cold median of the other three types. Read Elimination's
  cold cost as ~45 ms like the rest.
- **The Completion legacy arm's mean is not its cost.** Its early timed runs reach 98-101 ms against
  a steady ~22 ms, which drags the mean to ~41 ms. The median (23.31 / 22.72 ms) is usually
  unaffected, but not reliably: one `8963a41` invocation had enough slow early runs to drag the
  median itself to 56.77 ms against a 19.94 ms min. Check the min before trusting a Completion
  legacy median.
- **The Elimination legacy median moves between invocations** — 16.67 and 21.39 ms here, 15.60,
  16.69 and 21.25 ms on `b0a3e27`, each tight within itself (±3 ms). The warm native arm barely
  moves (3.52 / 3.38 ms), so the Elimination speedup is a **4.7-6.3x** band, not the single figure
  the first table row reads.
- **One band, one trader, unseeded.** The midpoint of the second shipped level band and the first
  whitelisted trader per type. An unseeded run draws a different quest every time; the spread columns
  include that variation.
- **No RSS figures**, and the warm arms' allocation rounds to 0.0-0.1 MB, so treat those as "under
  100 KB" rather than as measurements. `alloc/run` also includes each iteration's `BuildPool()` —
  itself under ~100 KB, and it cancels out of the cold-minus-warm delta.
- **Workstation GC**, as with the ragfair fixture: `<ServerGarbageCollection>` is set on
  `SPTarkov.Server.csproj`, not on `UnitTests.csproj`.

## Results — scav case rewards

Recorded 2026-08-17 on `d31a000` plus the working-tree fixture that produced them. Same machine as
the bot-generation, ragfair and repeatable-quest figures above, not the machine the location-loot
figures came from.

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |
| Configuration | Release, n=20 after 2 warmups, per arm per recipe |

**The design expected native to lose here, and it does.** Scav case rewards are a cold path — one
`Generate` call per finished craft, behind a 41-minute-to-5-hour hideout timer — and the call
produces 1 to 7 reward groups. The native path projects and serialises the whole items view plus a
static price for every tpl in it on each call, with that handful of output items to amortise it
against; the reward-loot port is the comparison class (~53 ms native vs ~17 ms legacy, 15-35 items).
No parity gate was promised on this port and none is claimed.

**Workload.** One `ScavCaseRewardGenerator.Generate(recipeId)` call per shipped recipe — all five in
`hideout/production.json`'s `scavRecipes` — on the live shipped database, unseeded the way
production runs. Between them the recipes cover an empty rarity, a fixed count and a ranged count.

**Two arms per recipe**, asserted rather than assumed — the fixture checks `LastPathTaken` before it
reports a number:

- **native** — the default path.
- **legacy** — `ScavCaseConfig.ForceLegacyScavCaseGeneration`, the retained 4.1.2 C# path.

A third phase times `ScavCaseNativeRequestBuilder.Build()` on its own, on the first recipe — the
request is identical for every recipe bar its `RecipeId`.

Each arm is measured off a generator built for that arm and warmed twice first. The legacy path
caches its two item pools on the instance, and those warmups build them: that is the state a
production generator answers from. `ScavCaseRewardGenerator` is transient in DI, but the graph
holding it hangs off the singleton `HttpServer`, so one instance serves every craft for the life of
the process and pays the pool build once.

### Elapsed time per call

Two full invocations of the fixture; the second median is the error bar on the first. Recipes are in
the order the shipped table lists them, which is the order they are measured in — the first two
positions have not settled, on both arms, so only positions 3-5 are steady-state figures. See below.

| Recipe | End products (common/rare/superrare) | Arm | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|---|---|
| `6271093e…` moonshine | 0 / 1 / 3-5 | native (rust) | 74.08 ms | 74.11 ms | 74.55 ms | 62.71 ms | 89.55 ms |
| `6271093e…` moonshine | 0 / 1 / 3-5 | legacy (C# 4.1.2) | 1.64 ms | 1.93 ms | 2.10 ms | 1.80 ms | 3.83 ms |
| `62710a8c…` 15,000 ₽ | 1 / 1-3 / 0 | native (rust) | 49.73 ms | 41.17 ms | 47.10 ms | 34.97 ms | 74.22 ms |
| `62710a8c…` 15,000 ₽ | 1 / 1-3 / 0 | legacy (C# 4.1.2) | 0.77 ms | 0.78 ms | 0.83 ms | 0.71 ms | 1.59 ms |
| `62710974…` 2,500 ₽ | 1-2 / 0-1 / 0 | **native (rust)** | **36.88 ms** | **37.50 ms** | 39.63 ms | 34.29 ms | 53.81 ms |
| `62710974…` 2,500 ₽ | 1-2 / 0-1 / 0 | **legacy (C# 4.1.2)** | **0.41 ms** | **0.41 ms** | 0.43 ms | 0.38 ms | 0.81 ms |
| `62710a69…` 95,000 ₽ | 0 / 1-3 / 1-2 | **native (rust)** | **37.38 ms** | **37.82 ms** | 39.61 ms | 33.91 ms | 54.87 ms |
| `62710a69…` 95,000 ₽ | 0 / 1-3 / 1-2 | **legacy (C# 4.1.2)** | **0.44 ms** | **0.44 ms** | 0.46 ms | 0.40 ms | 0.81 ms |
| `62710a0e…` intel folder | 0 / 2-4 / 2-3 | **native (rust)** | **37.59 ms** | **37.32 ms** | 39.72 ms | 34.37 ms | 52.57 ms |
| `62710a0e…` intel folder | 0 / 2-4 / 2-3 | **legacy (C# 4.1.2)** | **0.45 ms** | **0.45 ms** | 0.51 ms | 0.40 ms | 1.20 ms |
| — | — | `Build` (request only) | 6.69 ms | 6.84 ms | 9.26 ms | 4.46 ms | 17.80 ms |

Steady state is taken off `62710974…`, `62710a69…` and `62710a0e…` — the three measured in positions
3-5. The first two positions are still settling on both arms and are excluded; see below.
**~37.5 ms native against ~0.44 ms legacy — native is ~85x slower per call**, and the ratio is flat
across those three because the cost is not the recipe. It is the widest native-versus-legacy gap
in this file, and it was the expected outcome rather than a regression found afterwards.

Where it goes: `Build()` alone is **6.7 / 6.8 ms** — the items view, a static price per tpl in it,
every default preset, the blacklists and the recipe table. The remaining ~31 ms is the serialise of
that request, the native side's parse of it, generation itself, and binding the response back into
`Models` objects. Same shape as the bot path, where the equivalent split measured ~92% transport.

The legacy path has nothing comparable to pay: it filters the item table once per generator instance
into `DbItemsCache`/`DbAmmoItemsCache`, then a call is three price-range filters over that cached
list plus 1-7 draws. Sub-millisecond is what a warm instance costs; a cold one pays the pool build
first, which this fixture excludes from both arms by warming up.

### The first two positions measured have not settled

`6271093e…`, measured first, reads 74 ms native and ~1.8 ms legacy in both invocations — stable, so
not spread, and its min (62.71 ms) never falls to the others' floor. `62710a8c…`, measured second,
is elevated too and by less: 49.73 / 41.17 ms native against the settled ~37.5, and 0.77 / 0.78 ms
legacy against the settled ~0.44. It is a gradient over the first two positions, not a single bad
phase — two warmups do not settle the process.

It is measurement order, not the recipe. Running the same fixture with the recipe list reversed
reproduces the gradient positionally — inflated first row, part-settled second row — and leaves
`6271093e…` and `62710a8c…` at ordinary figures once they are measured late:

| Recipe (reversed order) | native median | legacy median |
|---|---|---|
| `62710a0e…` (measured first) | 76.54 ms | 1.67 ms |
| `62710a69…` (measured second) | 40.55 ms | 0.76 ms |
| `62710974…` | 38.56 ms | 0.56 ms |
| `62710a8c…` | 37.63 ms | 0.44 ms |
| `6271093e…` (measured last) | **38.71 ms** | **0.46 ms** |

Position for position, that is the same gradient: ~76 / ~1.7 ms first, ~40 / ~0.76 ms second, then
settled. Read the first two recipe rows of the main table — `6271093e…` and `62710a8c…`, both arms —
as ~37.5 ms native / ~0.44 ms legacy like the rest, which is what they read when measured late. The
fixture keeps the two-warmup methodology of the other fixtures in this file rather than tuning it
away; the same artifact is documented on the repeatable-quest Elimination cold arm above.

### Why native stays the default anyway

One call per finished craft, against production times of 41 minutes (`62710974…`) to 5h20m
(`62710a0e…`). The absolute cost is ~37 ms once per craft on a path a player reaches a few times a
day — nothing a player or a raid loop can observe. Native stays the default for family consistency
with the other ported generators, and `ScavCaseConfig.ForceLegacyScavCaseGeneration` is the opt-out
for anyone who disagrees.

Scav-case-specific caveats, on top of the general ones below:

- **No allocation or RSS figures.** This fixture times elapsed wall clock only.
- **Unseeded.** Every timed run draws different rewards, and a draw that lands on a weapon or armor
  template costs a preset clone that a plain item does not. The min/max columns include that.
- **`Build()`'s mean is not its cost.** 9.26 ms against a 6.84 ms median and a 4.46 ms min, in both
  invocations. Read the median; the phase is skewed by a few slow runs and no cause was measured.
- **Workstation GC**, as with the ragfair and quest fixtures.

## Results — item base class cache

Recorded 2026-08-17 on `526704c` plus the working-tree fixture that produced them. Same machine as
the bot-generation, ragfair, repeatable-quest and scav-case figures above, not the machine the
location-loot figures came from.

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |
| Configuration | Release, n=20 after 2 warmups, per arm |

**Native loses this one, by ~3.5x.** The build is a deterministic walk over the shipped items table,
and the C# it replaces is a tight loop over a dictionary the process already holds: no filtering, no
draws, no clone. There is nothing on the native side that the walk itself can win back against a
round trip, and this benchmark exists to put a number on the loss rather than to claim one. It is a
startup path — see *Why native stays the default anyway* below.

**Workload.** One `ItemBaseClassService.HydrateItemBaseClassCache()` call over the live shipped
items table: 4,673 templates in, a cache of 4,553 tpls carrying 20,218 ancestor ids plus 120 root
node ids out. This is the whole workload the service has; nothing else about it was ported.

**Two arms**, asserted rather than assumed — the fixture checks `LastPathTaken` before it reports a
number:

- **native** — the additive constructor, which wires the request builder. What the container builds.
- **legacy** — the frozen 4.1.2 constructor, which has no native seam and so hydrates legacy
  unconditionally. Selected by construction rather than by `ItemConfig.ForceLegacyItemBaseClassHydration`,
  so the fixture never touches the shared config; the dispatcher reaches the same body either way.

**A service instance per run**, built outside the stopwatch. That is the production shape — one
hydrate on a fresh singleton — and it keeps the legacy arm honest, since its dictionary is built
from nothing every time.

A third phase times `ItemBaseClassNativeRequestBuilder.Build()` on its own.

### Elapsed time per hydrate

Two full invocations of the fixture. The recorded figures are the second invocation's; the first
invocation's median is the error bar on it.

| Arm | median | median (1st run) | mean | min | max |
|---|---|---|---|---|---|
| **native (rust)** | **29.19 ms** | 29.60 ms | 29.22 ms | 20.35 ms | 44.92 ms |
| **legacy (C# 4.1.2)** | **8.19 ms** | 8.80 ms | 10.32 ms | 7.42 ms | 23.97 ms |
| `Build` (request only) | 0.34 ms | 0.29 ms | 0.37 ms | 0.29 ms | 0.62 ms |

Speedup on median elapsed time per hydrate: **0.28x** (0.30x on the first invocation) — native is
**~3.4-3.6x slower**. An earlier pair of invocations of the same fixture, before the payload-shape
line was added to it, read 29.45 / 30.52 ms native against 7.52 / 7.42 ms legacy: the same result.

**It is not the request projection.** `Build()` is **0.34 ms**, ~1% of the native median — the
cheapest request any port in this file builds, because it is two fields per template, `_parent` and
`_type`, and nothing else. Every other port in this file pays 5-14 ms here; this one does not, and it
loses anyway. The remaining ~29 ms is the serialise of that request, the native side's parse of it,
the walk, and binding the response back into `Dictionary<MongoId, HashSet<MongoId>>`. The response is
the bigger payload of the two: 20,218 ancestor ids across 4,553 sets against the request's 4,673
two-field entries, and every one of those ids comes back through `MongoId`'s validating constructor.

The legacy path has nothing comparable to pay. It walks each template's parent chain in-process
against a dictionary it already has, adding straight into the cache — 8.2 ms for the whole table,
~1.8 µs per template.

**Measurement order does not move this.** Reversing the two arms reads 27.65 ms native against
8.56 ms legacy (0.31x), inside the spread of the table above. The first-position inflation
documented on the quest and scav-case fixtures does not appear here on either arm.

### Why native stays the default anyway

`PostDbLoadService.PerformPostDbLoadActions` calls hydrate exactly once, after mods have loaded, and
a mod that adds items may call it again explicitly. So the measured loss is **~21 ms added to server
startup**, once, on a path no player and no raid loop ever reaches. Native stays the default for
family consistency with the other ported services, and
`ItemConfig.ForceLegacyItemBaseClassHydration` is the opt-out for anyone who disagrees.

Base-class-specific caveats, on top of the general ones below:

- **No allocation or RSS figures.** This fixture times elapsed wall clock only. Native allocates the
  4,673-entry request view and the whole response map per call, on the managed heap, and neither is
  measured here.
- **Only the bulk build is ported.** `AddItemToCache`, the per-item fallback `ItemHasBaseClass` uses
  for a tpl the bulk build missed, is unchanged C# on both paths and is not measured.
- **The legacy arm's mean is not its cost.** 10.32 ms against an 8.19 ms median and a 7.42 ms min:
  a few slow early runs, the same artifact the other fixtures document. Read the median.
- **Workstation GC**, as with the ragfair, quest and scav-case fixtures.

## Results — ragfair linked item table

Recorded 2026-08-17 on `dd96eb1` plus the working-tree fixture that produced them. Same machine as
the bot-generation, ragfair, repeatable-quest, scav-case and item-base-class figures above, not the
machine the location-loot figures came from.

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 |
| Configuration | Release, n=20 after 2 warmups, per arm |

**Native loses this one too, and by more than the base class port did.** The build is a
deterministic walk over the shipped items table, and the C# it replaces walks a dictionary the
process already holds — no filtering, no draws, no clone. The design said so up front (the port
design's *Transport expectation*: "native will likely lose the end-to-end benchmark"), and it does.
It is a once-per-process lazy build — see *Why native stays the default anyway* below.

**Workload.** One linked item table build over the live shipped items table: 4,673 templates in, a
table of 4,673 tpls carrying 63,530 linked ids out. `BuildLinkedItemTable` is protected, so the
timed call is `GetLinkedItems(tpl)` on a fresh instance — the cache miss triggers the build, exactly
the way production reaches it. The miss and the indexer read that follows are both a dictionary
probe against a full-table walk. This is the whole workload the service has; nothing else about it
was ported.

**Two arms**, asserted rather than assumed — the fixture checks `LastPathTaken` before it reports a
number:

- **native** — the additive constructor, which wires the request builder. What the container builds.
- **legacy** — the frozen 4.1.2 constructor, which has no native seam and so builds legacy
  unconditionally. Selected by construction rather than by
  `RagfairConfig.ForceLegacyRagfairLinkedItemBuild`, so the fixture never touches the shared config;
  the dispatcher reaches the same body either way.

**A service instance per run**, built outside the stopwatch — not an optimisation but a requirement.
The final copy loop is `Dictionary.Add` (quirk 1, ported verbatim on both paths), so a second build
on a warm instance throws; each run must build its own. That is also the production shape, since the
service is a singleton that builds once.

A third phase times `RagfairLinkedItemNativeRequestBuilder.Build()` on its own.

### Elapsed time per build

Two full invocations of the fixture. The recorded figures are the second invocation's; the first
invocation's median is the error bar on it.

| Arm | median | median (1st run) | mean | min | max |
|---|---|---|---|---|---|
| **native (rust)** | **92.53 ms** | 92.91 ms | 87.12 ms | 53.43 ms | 111.09 ms |
| **legacy (C# 4.1.2)** | **14.65 ms** | 15.19 ms | 15.07 ms | 10.66 ms | 19.52 ms |
| `Build` (request only) | 3.85 ms | 3.97 ms | 7.23 ms | 2.73 ms | 19.14 ms |

Speedup on median elapsed time per build: **0.16x** (0.16x on the first invocation) — native is
**~6.3x slower** as the fixture measures it. Both arms move with measurement order, though, and
position for position the loss is **~3.1-5.4x**; see below.

**It is not the request projection.** `Build()` is **3.85 ms**, ~4% of the native median. The
remaining ~89 ms is the serialise of that request, the native side's parse of it, the walk, and
binding the response back into `Dictionary<MongoId, HashSet<MongoId>>`. Both legs are id-heavy: the
request is 4,673 templates carrying 40,761 slot, chamber and cartridge filter ids, and the response
is 63,530 linked ids across 4,673 sets, every one of them arriving through `MongoId`'s validating
constructor. At 24 bytes per id that is ~1 MB out and ~1.5 MB back before any envelope — arithmetic
off the counts the fixture prints, not a measured wire figure.

The response keys exactly the 4,673 tpls the items table holds. The build's reverse edges (an id in
some template's filter gets that template added to *its* set) key nothing the table did not already
have, so the "may key entries the items table never held" allowance in `RagfairLinkedItemResult` is
unused by the shipped database.

The legacy path has nothing comparable to pay. It walks the same table in-process and unions the
filter ids straight into the sets it is building — 14.7 ms for the whole table, ~3.1 µs per
template.

### Measurement order moves both arms

Unlike the base-class fixture, where reversing the arms changed nothing, here whichever arm is
measured **first** pays for it, on both paths:

| Arm | measured first | measured second |
|---|---|---|
| native (rust) | 92.53 / 92.91 ms | 74.25 / 81.20 ms |
| legacy (C# 4.1.2) | 30.12 / 29.74 ms | 14.65 / 15.19 ms |

Legacy doubles — 30 ms in first position against 14.7 ms in second — which is what makes the
headline ratio order-sensitive. Reading the two arms in the same position: **~3.1x** slower
(both first) or **~5.1-5.4x** slower (both second). The shipped fixture's 6.3x pairs an inflated
native arm with a settled legacy one and is the pessimistic end of that band; the design's ~3.5x
expectation, taken from the base-class port, sits at the optimistic end. No cause was measured;
each arm allocates a full table's worth of sets, so a heap already grown by the other arm is the
obvious suspect, but that is a guess, not a finding.

### Why native stays the default anyway

The build is lazy and single-shot: the first `GetLinkedItems` miss builds the whole table, and every
call after it is a dictionary hit. There are three call sites — `RagfairHelper`'s linked flea search
and two in `LootGenerator`'s weapon reward loot — so one request in the life of the process pays it.
The measured loss is **~60-66 ms added to whichever request arrives first** (native minus legacy,
taken position for position), once, and nothing afterwards. Native stays the default for family
consistency with the other ported services, and `RagfairConfig.ForceLegacyRagfairLinkedItemBuild` is
the opt-out for anyone who disagrees.

Linked-item-specific caveats, on top of the general ones below:

- **No allocation or RSS figures.** This fixture times elapsed wall clock only. Native allocates the
  4,673-entry request view and the whole response table per call, on the managed heap, and neither
  is measured here.
- **Order effects are not warmed away.** Two warmups per arm, the same methodology as the other
  fixtures in this file, and they do not settle it — see above. Read the band, not the single ratio.
- **`Build()`'s mean is not its cost.** 7.23 ms against a 3.85 ms median and a 2.73 ms min, in both
  invocations: a few slow runs, the same artifact the scav-case fixture documents. Read the median.
- **The timed call includes `GetLinkedItems`' miss and indexer read**, on both arms — two dictionary
  probes against a ~15-90 ms build.
- **Workstation GC**, as with the ragfair, quest, scav-case and base-class fixtures.

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
