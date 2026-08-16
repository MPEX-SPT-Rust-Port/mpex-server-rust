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
alongside it — sixteen always-default item members (−640 KB / −13.8% on `assault`, −13.3% on
`pmcUSEC`) and `slots[].required` (−49 KB), taking the `pmcUSEC` request the size guards measure to
~4.18 MB — worth ~4-6% on the per-bot path and ~0% batched, since the batch already divides the
block they shrink.

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

Recorded 2026-08-15 on `6d975e4` — the stamp-gated invariant-slice cache
(`.superpowers/sdd/2026-08-15-db-mutation-stamp-ragfair-slice-cache/`), re-measuring the figures the
native-parity effort left on `df07d54`. Same machine as the bot-generation figures above, **not**
the machine the location-loot figures came from.

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
`expiredOfferThreshold`), the shape
`RagfairServer.ProcessExpiredFleaOffers` hands over. `RagfairPayloadProjection.BuildRequest` is
timed on its own in a third phase per scenario. A fourth scenario times the regeneration pass with
the invariant-slice cache forced cold against the same pass left warm (see *The slice cache*).

**The flea is already full when the timing starts.** The test harness runs every `IOnLoad`, so
`RagfairCallbacks.OnLoadAsync` → `RagfairServer.Load()` has already generated a complete set of
dynamic offers into the holder before the fixture takes its baseline snapshot. The fixture removes
everything each run adds, outside the stopwatch, but deliberately preserves that pre-existing set —
so every timed run is a pass over an *already-populated* flea, on both paths. That is the state the
`ProcessExpiredFleaOffers` call runs in for real, and it is symmetric across paths, so the verdict
below is unaffected; it does change what the offer column means (see below).

### Wall clock

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

Speedup on median wall clock: **0.69x** on the full pass (**1.45x slower**) and **0.65x** on the
regeneration pass (**1.53x slower**, was 7.3x slower before the slice cache). Projection share of
the native median: **2.1%** (full pass), **84.9%** (regeneration) — the projection phase still
builds the whole slice every time it is timed, so on a warm regeneration pass it now measures work
the generation pass no longer does.

### The slice cache

The regeneration pass's cost was never generation, it was the 5.8 MB call-invariant request slice
being rebuilt and re-serialised for a pass that only regenerates 1,400 offers. Since `6d975e4` the
slice is sent only when `DatabaseMutationStamp` changed since the last send; the native side keeps
the parsed copy keyed by that stamp, and the send is varying-fields-only otherwise. A hit skips
the C# projection (both price maps over the whole items table, every preset, the items view), the
MessagePack serialise of the result, and the native parse of it.

| Regeneration pass | median | alloc/run |
|---|---|---|
| cache cold (stamp bumped before every run — the pre-`6d975e4` behaviour) | 77.94 ms | 17.6 MB |
| **cache warm (the shipped path)** | **11.46 ms** | **5.2 MB** |

**6.80x** on the median; warm is **14.7%** of cold, and warm allocates **0.30x** what cold does. The
`regeneration | native (rust)` row above (15.19 ms) and this warm row (11.46 ms) are the same
configuration measured in two different phases of the same run — the gap between them is run-to-run
spread on a ~12 ms pass, not a difference in what was measured.
The cold arm reproduces the old 74.59 ms baseline within noise, which is the check that the arms
differ only in the cache. The full pass is unchanged — its slice is 2.1% of the pass, so caching it
buys nothing there and the 670 ms figure is the same 626 ms measurement one run's spread later.

The offer column is **offers accepted into an already-full flea**, not offers the path produced.
The full pass *produces* ~58k offers — the framed response captured for the deserialize attribution
below held 58,137 frames — and `RagfairOfferHolder.AddOffer` drops a fake-player offer once that
template is at its cap, which is what leaves ~24k. The cap is re-rolled per call
(`RagfairServerHelper.GetOfferCountByBaseType` is a `RandomUtil.GetInt` over the configured range),
so a full flea still admits whatever a high roll leaves room for. That is also why 1,400 distinct
expired templates land as ~870 offers — the pre-filled holder, not anything about the run's own
interleaving. Against an empty holder the same input would land as ~1,400. The two paths land
within ~1% of each other on the full pass, so both are doing the same amount of work.

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
`RagfairOffer`/nickname instances in its type census. A production pass is unseeded and therefore
nondeterministic (offer counts are per-template `RandomUtil.GetInt` rolls), so these are three
captures of the same workload varying ~0.7% run to run, not three measurements of one number.

Working set over the timed phase: native grows +443 MB on the full pass against the legacy path's
+0 MB; peak RSS is process-wide and cumulative, so only the growth figures are comparable, and only
within one invocation.

### Checkpoint series — what the native-parity effort bought

Each row is one `RagfairBenchmarkTests` invocation, medians, same machine, native and legacy from
the **same run**. Ratio is native / legacy on the full pass; above 1.00x means native is slower.

| Stage | Commit | Full pass native | Full pass legacy | Ratio | Regen native | Regen legacy | Alloc/run native |
|---|---|---|---|---|---|---|---|
| Baseline | `7fab74d` | 1550 ms | 517 ms | 3.00x | ~103 ms | ~12.8 ms | not recorded |
| A — rayon batch fan-out | `5fefd61` | 884.67 ms | 454.37 ms | 1.95x | 85.42 ms | 9.87 ms | 182.9 MB |
| B — framed response, ABI 9 | `357c7f3` | 676.12 ms | 443.22 ms | 1.53x | 75.00 ms | 10.50 ms | 208.4 MB |
| C — MessagePack payloads, ABI 10 | `d076b39` | 719.39 ms | 441.66 ms | 1.63x | 75.67 ms | 10.78 ms | 358.3 MB |
| C — reader allocation remediation | `a2acb6f` | 663.95 ms | 457.44 ms | 1.45x | 76.14 ms | 10.51 ms | 219.9 MB |
| One-pass fresh-id offer items | `df07d54` | 625.78 ms | 426.57 ms | **1.47x** | 74.59 ms | 10.25 ms | 219.3 MB |
| Stamp-gated slice cache | `6d975e4` | 670.63 ms | 460.29 ms | **1.45x** | **15.19 ms** | 9.92 ms | 206.6 MB |

**Stage A** moved generation, not the pass: the `CreateOffersFromAssort` timer inside the native
full pass fell from ~710 ms to 91–103 ms across the run's samples, while the full pass only fell
1550 → 885 ms. Everything after that is transport work.

**Stage C regressed on arrival** — 676.12 → 719.39 ms with native alloc/run up 208.4 → 358.3 MB
(+72%) at a flat offer count, reproduced back-to-back. The cost was in the C# reader, not the
codec: a `ToArray` copy per frame, a fresh `Utf8JsonWriter` pair per transcoded value, a reflection
`GetValue` per item, and ~2M `ReadString` map-key allocations per pass. Removing all four
(`a2acb6f`) brought it to 663.95 ms / 219.9 MB — msgpack then beat the stage B JSON frames it
replaced, by 12 ms, at +11.5 MB.

**The one-pass fresh-id change is real but small.** Full-pass medians cannot resolve it: run-to-run
spread on the native median is ±25 ms, and an A/B with the change stashed and unstashed read
1.46x/1.46x before against 1.47x/1.53x after. The finer instrument is the `CreateOffersFromAssort`
timer, which measures only the leg the change touches — median 92 → 85 ms, **~8% off a ~92 ms
generation leg**, which is ~1% of a transport-bound pass.

### The 1.25x parity gate — MISSED

The effort's success criterion was a native full-pass median at or under **1.25x** the same-run
legacy median. The final state is **1.47x** (625.78 / 426.57; the bar was 533.2 ms), and the
preceding checkpoint was 1.45x. **The gate is missed and every in-scope lever is spent.**

What was achieved instead: the full pass went **1550 → 626 ms, a 2.48x improvement** on the native
path, and native allocation fell below legacy's (219 MB vs 283 MB). Native is now within ~200 ms of
legacy rather than a second behind it, but it is still behind it.

Where the residual sits, measured rather than assumed:

- **Not generation.** The Rust half is ~85 ms of a ~626 ms pass.
- **Not the payload projection.** `BuildRequest` is 13 ms, ~2% of the full pass.
- **Not converter taxes.** Timed at their real per-pass instance counts against the shipped
  `Parallel.For` deserialize (~305 ms, 41.4 MB / 58,137 frames): the Ceciler-injected
  `[JsonExtensionData]` dictionary is **3.4 ms**, `MongoId`'s ctor validation over 425,550 ids is
  **11.1 ms**, and `string.Intern` on 58,558 nicknames is **15.1 ms** — ~30 ms all together, 15x
  under the 50 ms bar that would have justified changing the injected property. (The 3.4 ms is the
  dictionaries' allocation cost measured in isolation, not System.Text.Json's metadata overhead for
  them.)
- **It is C#-side response binding plus GC.** The pass materialises **366,851** `Models` instances
  (`Item` 83,292, `Upd` 60,325, `RagfairOffer`/`OfferRequirement`/`RagfairOfferUser` 58,558 each,
  `UpdRepairable` 43,300, tail 4,260). After the ~30 ms of taxes above, the remaining ~275 ms is
  System.Text.Json binding those objects, which no converter-level change reaches. Legacy has no
  equivalent leg at all — it never crosses a wire. (That ~305 ms leg is larger than the ~199 ms
  native is behind legacy; the two do not subtract, because the rest of the native pass is cheaper
  than the C# work legacy does in its place. Removing the leg entirely would put native ahead.)

**The fixture runs workstation GC; the shipped server does not.**
`<ServerGarbageCollection>true</ServerGarbageCollection>` is set on `SPTarkov.Server.csproj` only,
not on `UnitTests.csproj`, so at ~220 MB per pass the fixture's parallel deserialize is
GC-throttled — single-threaded deserialize (268.6 ms) actually beat the `Parallel.For` one
(304.8 ms) in the attribution run. Re-running the whole benchmark with `DOTNET_gcServer=1` on the
same commit and machine moves **both** sides, so the verdict does not change:

```
full pass native (rust)     median=558.95 ms  min=417.63  alloc/run=209.1 MB
full pass legacy (C# 4.1.2) median=329.51 ms  min=300.32  alloc/run=282.6 MB
                            ratio 1.70x (vs 1.53x under workstation GC at the same commit)
```

Server GC takes ~117 ms off native and ~113 ms off legacy — the ratio is unchanged to slightly
worse while both absolute numbers improve. Read the shipped server's real numbers off the server-GC
figures and the *comparison* off either; the gate was evaluated in the fixture's own GC mode, for
consistency with the baseline it was set against.

**The regeneration pass was a different problem, and it is fixed.** ~60% of it was building and
serialising the 5.8 MB call-invariant request slice, against ~1% of a full pass — the one place
where a request-side cache would pay. `DatabaseMutationStamp` and the native slice cache took it
from 74.59 ms to **11.46 ms** warm; see *The slice cache* above.

Ragfair-specific caveats, on top of the general ones below:

- **The legacy path is parallel, and so is the native path now.** `GenerateDynamicOffersLegacy`
  fans one `Task.Factory.StartNew` per assort entry across the thread pool; since `5fefd61` the
  native side fans the unseeded batch walk across rayon (seeded runs stay sequential for parity),
  and the C# side deserialises the response frames with `Parallel.For`. Both paths are using the
  12 threads.
- **Every timed run works on a pre-filled flea** (see Workload). Both paths pay the holder's
  per-template cap checks against a full `_fakePlayerOffers` index, so the absolute numbers include
  work an empty-flea pass would not do — but symmetrically, and this is the state the real
  regeneration pass runs in.
- **n=5 after 1 warmup**, not the 20 the other fixtures use — a full pass is seconds, not
  milliseconds. Spread is now wide relative to the differences being measured (native full pass
  614-872 ms, and ±25 ms run-to-run on the median), so a change worth less than ~5% of the pass
  cannot be resolved here — use the `CreateOffersFromAssort` timer the native path logs instead.
- **The plain native arms run with the cache warm.** Nothing bumps the stamp between runs, so after
  the cold arm every timed pass is a cache hit — which is what a real server does between mutations.
  The cold arm exists to price the miss; do not read it as the default.
- **Regeneration input is fixed.** The same 1,400 single-item lists every run, sampled from the head
  of the assort rather than from offers that actually expired. Real expired offers skew towards
  fast-selling templates and can carry item trees.
- **First-call effects are warmed away.** One warmup per phase covers JIT, the native library load
  and the assort generation's first pass. The server's real first flea pass at startup costs more on
  both paths.
- **Native is measured first in each scenario**, so the legacy phase inherits a warmer process (and
  a heap already grown to the native path's high-water mark). That biases mildly *against* native —
  it does not account for the remaining 1.47x, but the gap is not smaller than measured.

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
