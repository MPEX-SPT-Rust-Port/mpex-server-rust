# Benchmarks

Every benchmark measures a Rust port against the retained 4.1.2 C# implementation it replaced. They
live under `Testing/UnitTests/Tests/` as `[Explicit]` NUnit fixtures, so a plain `dotnet test` never
runs them. There are no `cargo bench` targets.

| Fixture | Measures |
|---|---|
| `LootBenchmarkTests.cs` | location loot — elapsed time per call, allocation, peak RSS |
| `RewardLootBenchmarkTests.cs` | airdrop loot — elapsed time per call |
| `BotBenchmarkTests.cs` | one bot's inventory — elapsed time per bot, the resident arm's payload projection timed separately |
| `RagfairBenchmarkTests.cs` | a dynamic flea offer pass — elapsed time per pass, views-override projection and forced publish timed separately |
| `RepeatableQuestBenchmarkTests.cs` | one repeatable quest of each type — elapsed time per quest, publish cold/warm and views-override projection timed separately |
| `ScavCaseBenchmarkTests.cs` | one scav case of each shipped recipe — elapsed time per call, publish cold/warm and the override projections timed separately |
| `ItemBaseClassBenchmarkTests.cs` | one bulk item base class cache build — elapsed time per hydrate |
| `RagfairLinkedItemBenchmarkTests.cs` | one ragfair linked item table build — elapsed time per build |
| `DbPublishSpikeTests.cs` | phase 0 state-ownership spike — full-DB publish envelope: per-root size and projection time; paired with `rust/spt-native/tests/phase0_publish_spike.rs` for parse time and RSS |
| `RagfairViewsEquivalenceTests.cs` | phase 1 flip #1 — writes the 3-root publish envelope and C#-built expected ragfair views; paired with `rust/spt-native/tests/phase1_ragfair_views.rs` for the derivation-equivalence check |
| `QuestViewsEquivalenceTests.cs` | phase 1 flip #2 — writes the 4-root publish envelope and C#-built expected quest views; paired with `rust/spt-native/tests/phase1_quest_views.rs` for the derivation-equivalence check |
| `DatabaseImportBenchmarkTests.cs` | phase 3 fused SPT_Data load — startup import: verify, native fused load and the managed replica walk timed separately on both `DatabaseImporter` arms |
| `DbPublishFixtureTests.cs` | phase 4 configs root — writes the projected configs root; paired with `rust/spt-native/tests/phase4_configs_root.rs` for the two-serializer parse check |

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

## Phase 2 — write barriers

Phase 2 branch — 2026-08-19. `Patches/Ceciler.WriteBarriers` prepends `WriteBarrier.Bump()` to the
property setters of every DB model type reachable from the five roots `DbPublisher` publishes, so a
mod writing game data dirties the resident DB with no hand-written call. Two costs to price: the
barriers fire on every non-`init` setter during `SPT_Data` deserialization (startup), and any
production path that writes into a published root on every pass buys a full five-root republish on
the next native call (steady state).

| measure | value |
|---|---|
| instrumented setters | 2891 |
| startup, base (`5c94484`), median of 6 | 8.95 s |
| startup, barriered, median of 6 | 9.37 s |
| startup delta | **+0.42 s** (bar: ~2 s) |

Startup is time from process start to the first `/health` answer, which is after `DatabaseImporter`
— everything below `GameCallbacks` runs before Kestrel binds. Two sets of three runs per
configuration, `SPT.Server` launched from its own Release output directory, base built into a
separate worktree:

```
                run 1   run 2   run 3  |  run 4   run 5   run 6   median
base (5c94484)  10.109   7.737   9.217 |   8.156   8.685   9.215     8.95
barriered        9.850   9.114   9.427 |   9.322   8.261   9.642     9.37
```

The delta is smaller than the run-to-run spread of either configuration (base alone covers
7.74–10.11 s), so it is well under the ~2 s bar. `DatabaseMutationStamp.BumpGlobal` keeps its
`Interlocked.Increment`; the planned fallbacks (a plain `_current++`, then an `Armed` flag set after
the initial import) are not needed and were not implemented.

Steady state is gated by `WriteBarrierChurnTests` — a publish must not dirty the stamp it just read,
a native response decode must not move it, and a settled ragfair pass must not force a republish. All
three pass on Release.

**Denylist entries added by the churn guard: none.** The two churning paths the guard and the
existing suite found share one cause, and it is not a database write:

| churn source | diagnosis | fix |
|---|---|---|
| `LocationLootGeneratorTests.GenerateLocationLootBeatsTheCSharpBaseline` — 1910 ms/call vs a 929.83 ms bar | every `GenerateStaticContainers` response deserialized fresh `SpawnpointTemplate`s, whose setters are barriered because the type is reachable from `LocationTable` | `WriteBarrier.Suppress()` around the decode in `SptNative.DecodeResult` |
| `RepeatableQuestPathDispatchTests.ASecondUnchangedPassSkipsTheRepublish` — one epoch per pass | every repeatable-quest response deserialized a fresh quest, whose condition types are barriered because they are reachable from `TemplateTable.Quests` | as above |

Deserializing a native response cannot reach an object a published root already points at, so those
writes convey no freshness — the same argument the publish projection's suppression scope rests on.
Denying the types instead would have meant denying the whole quest-template and loose-loot subgraph,
which is precisely the coverage the barriers exist for. With the suppression in place the loot perf
gate reads 328.59 ms mean over 10 runs (bar 929.83 ms) and the full Release suite is green.

**The suppression is itself a blind spot**, narrower than the six denylist entries it replaced but
not the absence of one: a genuine database write on a thread inside a decode callback's extent is
invisible to the barriers. Nothing does that today — `JsonSerializer.Deserialize` allocates the graph
it fills — and `WriteBarrierChurnTests.ANativeResponseDecodeDoesNotMoveTheStamp` pins the invariant
directly so narrowing the scope fails by name rather than as a perf regression. Task 7's ledger
records the scope, not "no blind spots".

**One confirmed churn path is deliberately left unmitigated: `LocationController.cs:44`**
(`mapBase.Loot = []`, inside the `client/locations` loop over every map). `LocationBase` is reachable
from `LocationTable`, so the setter is barriered: every `client/locations` request moves the stamp
once per map and taxes the next native call a full five-root republish (~733–745 ms, the forced-publish
figure under § Scav case rewards). Frequency is once per client menu load, not per raid, so it is a
latency spike on an already-slow request rather than steady-state churn — but it is real and no test
observes it, because the loot perf gate calls `LocationLootGenerator` directly and never enters the
controller. It is not deniable the way the decode was: it is a write *into* the live database, so
`LocationBase` would have to be denied outright (losing map-base coverage) or the controller taught
to build its response off a copy. Left for Task 6's risk assessment rather than fixed blind.

The plan's third named candidate, `TraderAssortHelper.cs:179-185` (three writes per trader resupply),
needs no guard: `ResetExpiredTrader` only runs when a trader's assorts have actually expired — hours
apart on the 5 s `IOnUpdate` tick — and a resupply genuinely changes data the resident views derive
from, so the republish it buys is a correct freshness signal, not churn. The plan's fourth,
`RagfairServerHelper.cs:61` / `RagfairOfferGenerator.cs:504` (`CanSellOnRagfair = false`), converges
because the flip is one-way; `ASteadyStateRagfairPassConvergesToNoRepublish` is the pin for that.

The three pre-existing denylist entries and their reasons live in `WriteBarriersPatch._denied`
(`Item`, `BotBase`, `PmcDataRepeatableQuest` — all live per-request state).

## Phase 3 — Rust loads SPT_Data

`fdcd2b1` — 2026-08-19. Both `DatabaseImporter` arms, driven directly by the fixture so each half is
timed on its own: legacy hashes the tree (`SptNative.VerifyDatabaseAsync`) and then walks it a second
time off disk, native does one native walk (`SptNative.DbLoad(verify: true)`) that hashes, reads and
installs the five resident roots, after which the same reflection walk materializes `DatabaseTables`
from the returned buffers. 1 cold + 3 warm runs per arm, medians over the warm ones.

    dotnet test -c Release --filter "FullyQualifiedName~DatabaseImportBenchmarkTests" --logger "console;verbosity=detailed"

| measure | value |
|---|---|
| legacy verify | 96.8 ms |
| legacy import (disk walk) | 383.8 ms |
| legacy total | 480.6 ms |
| native fused load (verify + read + install) | 484.6 ms |
| native replica materialize (buffer-fed walk) | 451.1 ms |
| native total | 935.7 ms |
| returned buffers | 202 files, 49.4 MiB |
| native against legacy | **0.51x** — ~1.9x slower |

Three Release invocations, transcribed unedited into one layout (warm medians; the third reverses the
arm order, which is a one-line swap in the fixture, not an option it exposes):

```
measure                              cold ms  warm median ms     |  run 2          |  reversed
legacy verify                          122.0            96.8     |   95.9    85.2  |   88.6    82.9
legacy import (disk walk)              622.8           383.8     |  714.8   361.7  |  339.0   342.2
legacy total                           744.8           480.6     |  810.7   468.8  |  427.6   425.4
native fused load (verify+read)        747.1           484.6     |  756.0   475.9  |  486.6   463.4
native replica materialize             440.5           451.1     |  502.7   412.1  | 1190.7   398.8
native total                          1187.6           935.7     | 1258.7   888.1  | 1677.3   862.1
```

**The deliverable is the retired double-read, and it is not a speed-up.** 202 files / 49.4 MiB of
eager content now cross from Rust as buffers instead of being read once by the verifier and again by
`ImporterUtil`; the lazy files are excluded by design and stay disk-path `LazyLoad`s on both arms
(`locales/global/*` and `locations/*/looseLoot.json` are never read, `staticLoot.json` /
`staticContainers.json` are read for resident-root assembly but not returned). Measured at the
importer the flip **costs 419-455 ms** (480.6 → 935.7 ms; 468.8 → 888.1 on the repeat; 425.4 → 862.1
reversed), for two reasons this fixture separates:

- The fused load's ~380-391 ms over the bare verify (484.6 against 96.8; +390.7 and +380.5 on the
  other two invocations) is not a read. Buffer retention, the FFI copy and the five-root assembly,
  parse and derivation are all inside it, unsplit here — work the legacy arm does not do at import
  time at all.
- The buffer-fed walk is **50-68 ms slower** than the disk walk it replaces (451.1 against 383.8;
  +50.4 and +56.6 on the other two), not faster. With the page cache warm the second read it retires
  was nearly free, while the buffer plumbing is not.

Arm order does not explain either reading, but this sitting is noisier than the one this section
first pinned. Reversed (native measured first) **both** arms came in cheaper — legacy total 425.4 ms
against 480.6/468.8 measured first, native total 862.1 ms against 935.7/888.1 measured second — which
is a sitting-level shift, not a position effect: a position effect moves the two arms in opposite
directions. The ratio, which divides that shift out, holds at 0.51x / 0.53x / 0.49x across the three.
The reversed native cold materialize (1190.7 ms) is first-position JIT on the reflection walk, which
is why the cold column is reported and not used.

**The envelope preallocation in `fdcd2b1` is inside the measured native fused load and does not show
up.** Sizing `assemble_publish_envelope` with `Vec::with_capacity` instead of growing ~60 MB from
`Vec::new()` retires ~26 reallocations, and the fused load reads 484.6 / 475.9 / 463.4 ms against the
previous sitting's 480.5 / 463.2 / 476.6 at `ca7e3de` — the same band, no separation either way. The
untouched legacy arm moved by as much over the same two sittings (480.6 / 468.8 / 425.4 against
426.0 / 438.6 / 416.1), which is what says the difference is the sitting rather than the change. The
fix stands on its own terms (a ~120 MB cumulative `memcpy` is not worth keeping); it is not a
measured speed-up and nothing here should be read as one.

**Neither total is a startup total, and the gap the flip was built to close is not wired up.** Both
arms still pay the first `DbPublisher.EnsureCurrent` republish of all five roots — 730-745 ms, the
forced-publish figure under § Scav case rewards — because `EnsureCurrent` publishes whenever its own
`_currentEpoch` is 0 and nothing feeds `DbLoad`'s installed epoch into it (`DatabaseImporter`'s
`ponytail:` note says so in as many words). Estimated, not measured end-to-end: adding that
republish to each arm's total puts a startup at ≈1.21 s legacy against ≈1.67 s native — arithmetic
over two fixtures, since the 730-745 ms addend comes from `ScavCaseBenchmarkTests`' publish-cold arm
at a different commit and sitting, not from this one. No process-start-to-`/health` figure was taken
for this flip; Phase 2's startup rows are what a measured one would look like. The saving on offer —
dropping that first republish, since epoch 1 is already
resident and was assembled from the same bytes — is the measurement to retake once it exists.
`CoreConfig.ForceLegacyDatabaseImport` is the opt-out; `DatabaseLoadEquivalenceTests` is the gate that
both arms still produce identical tables.

Read against a warm page cache only: `DI`'s container build imports the database once before any
timed run, so "cold" here is JIT-cold, never disk-cold, and both arms' reads are served from memory.
A genuinely cold `SPT_Data` would favour the single-read arm by whatever the retired second read
costs off the disk — not measured, and not measurable from this fixture.

**Startup RSS was not re-measured on this branch.** Decision 1 in the Phase 3 ledger declines
loose-loot residency against a ~1 GB budget (405.2 MiB publish delta, § Phase 0), and this flip adds a
~60 MB transient envelope plus a full parse of it on top of the resident roots — no number was taken
for either, in this fixture or any other. The gap is recorded, not closed.

## Phase 4 — configs join the resident set

`ecf856b` — 2026-08-20. Every loaded config publishes as a sixth root keyed by its `Kind`, and six
families (scav case, ragfair, quest, reward loot, location loot, bots) now read their config data
off it instead of off the per-call varying block. Two prices to re-take: the publish, which gained
a root, and the bot wire, which lost the config half of its shared block.

    dotnet test -c Release --filter "FullyQualifiedName~ScavCaseBenchmarkTests" --logger "console;verbosity=detailed"

Two full invocations of that filter, medians and second-run medians, both ranges taken across the
five shipped recipes:

| measure | value (run 1) | value (run 2) |
|---|---|---|
| publish **cold** (stamp bumped per run) | 736.7 – 748.2 ms | 742.0 – 749.4 ms |
| publish cost per send (cold − warm) | 735.2 – 746.4 ms | 740.5 – 747.5 ms |
| five-root baseline, same fixture (`cecdd5c` / `9011794`) | 733.7 – 744.8 ms / 732.9 – 745.1 ms | 736.0 – 743.6 ms |
| native, publish **warm** | 1.55 – 1.78 ms | 1.54 – 1.93 ms |
| legacy (C# 4.1.2) | 0.41 – 1.53 ms | 0.42 – 1.39 ms |
| `Build` (request only) | 14.00 ms | 13.49 ms |
| `BuildViewsOverride` only | 6.45 ms | 6.46 ms |

**The sixth root is free at this fixture's resolution.** The plan budgeted roughly Phase 0's configs
row (67.7 ms warm projection, 0.7 MiB) on top of the 730–745 ms five-root publish; what the cold arm
actually reads is 736.7–748.2 ms against flip #6's nearer 732.9–745.1 — **+3.1 to +3.8 ms** on the
range endpoints, inside a per-recipe spread of 719–811 ms. So the spike's configs row does not
predict the marginal price of the root in a real publish: 67.7 ms warm for 0.7 MiB, against globals'
7.3 ms for the same 0.7 MiB, is a 9x gap at equal size that this publish does not reproduce.
The gap is the spike fixture, not the projection: `DbPublishSpikeTests.cs:57-78` calls
`ConfigLoader.Initialize(...)` *inside* the timed closure, so both its cold and warm configs figures
pay a full 28-file disk read plus deserialize that no other root's closure pays.
Nothing per-call moved for the root either: the warm arm holds at
1.54–1.93 ms against flip #5's 1.58–1.88 ms.

The bot wire has no committed fixture that reports bytes — `BotPayloadSizeTests` pins the *override*
arm's budget by design — so it was measured with a throwaway Release probe over
`BotPayloadProjection.BuildRequest` / `BuildSharedVarying`, serialised through
`JsonUtil.JsonSerializerOptionsNoIndent`, one `pmcUSEC` bot and one level band, as the flip-6 figures
were. The probe warms twice before reading (see the artifact note below).

| measure | value | pre-phase (`9011794`) |
|---|---|---|
| eligible single-bot request | 451,223 B (0.43 MiB) | 589,344 B (0.56 MiB) |
| views-override single-bot request | 4,152,813 B (3.96 MiB) | 4,208,129 B (4.01 MiB) |
| override against eligible | 9.20x | 7.1x |
| `shared` block, whole | 66,293 B | 204,414 B (implied) |
| `shared.equipment` | 39,811 B | same projection, unchanged |
| `shared.modPoolSlotOrder` | 26,428 B | 26,428 B |

| wave | resident per-bot | pre-phase resident per-bot | resident per send | views-override per-bot |
|---|---|---|---|---|
| 45 | **10,272 B (0.010 MiB)** | 13,341 B | 462,235 B | 92,529 B |
| 20 | **22,802 B (0.022 MiB)** | 29,707 B | 456,035 B | 207,881 B |
| 10 | **45,356 B (0.043 MiB)** | 59,167 B | 453,555 B | 415,514 B |
| 5 | **90,463 B (0.086 MiB)** | 118,087 B | 452,315 B | 830,781 B |
| 1 | **451,323 B (0.430 MiB)** | 589,444 B | 451,323 B | 4,152,913 B |

**The eligible wire shed 138,121 bytes (134.9 KiB) per send, a flat 23.0–23.4% at every wave size,
and that number is the config half exactly.** The block lost fourteen of its twenty members — twelve
config slices to the resident configs root, plus `equipmentBlacklist` and
`weaponModEquipmentBlacklist`, which the native side now selects out of `equipment` itself — while
the two sized members that survive are built by projections the flip did not touch
(`modPoolSlotOrder` reads 26,428 B on both sides of it; `equipment`'s expression is character for
character what `9011794` had). So the shared block's 204,414 → 66,293 B is the config half leaving
and nothing else. At wave 45 an eligible server now crosses 10 KB per bot where it crossed 13 KB
before the flip and 94 KB before residency.

**What dominates the remaining wire is the caller's own block, not either varying member.** On a
wave-45 send `templateVariants` — the per-level-band filtered template plus its loot pools — is
384,685 B, **83.2%** of the 462,235-byte request, with the 45 bot slices at 11,161 B behind it. The
plan predicted `modPoolSlotOrder` would be left dominant; it is third. Among the members that are
genuinely varying process state, `equipment` (39,811 B) leads `modPoolSlotOrder` (26,428 B) — and led
it at `9011794` as well, off the same projection expression, so the flip-6 block's "single largest
member still crossing per call" was already wrong when written:
`BotConfig.Equipment` was ruled to stay on the wire because `ReplayRandomisationClamps` writes the
nighttime mod chances back into it after every send, so the largest config-shaped member is the one
that did not move. Carving a varying `EquipmentMods` member out of an otherwise resident
`Equipment` is the named upgrade path, and it is ledger material, not a measurement.

**The ineligible arm did not move.** 4,152,813 B against the pre-phase 4,208,129 B is the same wire
by construction — the config half moved from `shared` into `viewsOverride`, not off the send, so the
ineligible total should be flat and is. The gap measures 55,316 B and the first-call artifact below
measures 54,178 B (423,285 − 369,107) — the two match to within ~1.2 KB, which puts the pre-phase
override arm in the unwarmed regime and its own resident arm in the warmed one. That is a
reading of two numbers, not a re-measurement of `9011794`. The override/eligible ratio rising
7.1x → 9.20x is entirely the eligible arm shrinking either way.

**Measurement note: the first `BuildRequest` of a process answers a template 54,178 B larger than
every later one** (423,285 B against a settled 369,107 B, reproduced across three passes in each of
two probe runs). It is not in the filtered clone — the template measures 368,927 B immediately after
`FilterBotEquipment` and 423,285 B by the time `BuildTemplateView` runs inside the same
`BuildRequest` — and it is present at `9011794` too on the arithmetic above, so nothing here suggests
Phase 4 introduced it; mechanism not chased. It is worth 12% of an unwarmed single-bot reading, so
any re-take of this line must warm first. The pre-phase resident figures were in the settled regime
(their single-bot 589,344 B and wave-1 589,444 B differ by the same 100 bytes as this sitting's
451,223 and 451,323), which is what makes the two comparable.

## Phase 5 — profile persistence

`b1f579a` — 2026-08-20. `SaveServer`'s disk boundary moved behind `spt_profile_*`. There is no
fixture: none predates this phase, and the spec's stated value is strategic (unblocking 6b). But
Decision 11 pre-committed to taking the cheap number rather than arguing the cost away, so
`SaveProfileAsync`'s own returned milliseconds were read before and after, on a **26.50 MB synthetic
profile**, 6 runs per pass and two passes per state:

| state | median | range |
|---|---|---|
| before (`fileUtil.WriteFileAsync`) | ~161 ms | 155 – 186 ms |
| after (native `spt_profile_save`) | ~192 ms | 187 – 217 ms |

**This is a regression and it is real, not noise: ~30–40 ms, about 20% slower, and the two states'
ranges do not overlap across any of the four passes.** The harness was throwaway and is not
committed; the profile is synthetic because no player profile of meaningful size exists in this
environment, so the number sizes the effect rather than pinning it.

The accounting, stated exactly, because the obvious version of it is wrong. The pre-phase path did
**not** stream: `fileUtil.WriteFileAsync(filePath, jsonProfile, ct)` took the `string` overload
(`Utils/FileUtil.cs:103-107`), which does one `Encoding.UTF8.GetBytes` into a full-size `byte[]` and
hands that to a single `fs.WriteAsync`. So peak was already `jsonProfile` (UTF-16) plus one
full-size UTF-8 buffer. The `MemoryStream` **replaces** that buffer (it is the same full-size UTF-8
bytes plus ~128 bytes of envelope); it is not an extra one. Two genuinely new costs remain — "costs"
and not "allocations", because the second turns out to be a pooled buffer — and the second was not
anticipated by the plan:

- `profile.rs`'s `SaveRequest.profile` is an **owned** `Box<RawValue>` (`profile.rs:175`), so serde
  scan-skips the profile and then copies all 26.5 MB into it. This is the one extra full-size copy
  at peak.
- `Utf8JsonWriter.WriteRawValue(string)` (`SptNative.cs:633` — `profileJson` is a `string`, `:614`,
  so it is the string overload, which delegates to the char-span one) transcodes through a
  `chars × 3` scratch buffer rented from `ArrayPool<byte>.Shared`. **That pool does serve buffers
  this large**, so the steady-state cost is the UTF-8 encode pass, not a per-save allocation.

On top of those, the serde parse-scan of the whole request buffer and a `Task.Run` hop are work the
write path did not do before, neither of which allocates a full copy.

**The scratch buffer is cheaper than an earlier draft of this section claimed, and the correction is
worth stating because the wrong version is intuitive.** Measured on .NET 10.0.10 against an 8 MB
payload: `ArrayPool<byte>.Shared` returns the *same array* after rent → return → rent at 1, 4, 16,
80, 128 and 512 MB, so there is no ~1 MB pooling cliff — that ceiling belongs to
`ArrayPool<T>.Create()`'s `ConfigurableArrayPool` (`DefaultMaxArrayLength = 1024*1024`), which the
same probe shows pooling at 1 MB and *not* at 2 MB. And the allocation is a cold-start effect, not a
per-call one: the first save on a thread allocates ~6.2x the char count, every later save on that
thread ~2.0x, the difference being the scratch getting rented rather than allocated. (Those two
absolutes are harness-inclusive — the probe measured the whole `ProfileSave` wrapper, so they count
the `MemoryStream` request buffer alongside the scratch, and are not `WriteRawValue`'s share alone.
The ~4.2x *difference* between them is what isolates the scratch.) What keeps any
of it live is an accident this prose previously never stated — `ProfileSaveAsync` hops through
`Task.Run` (`SptNative.cs:611`), and the shared pool's fast path is a per-thread TLS slot, so a save
landing on a threadpool thread with a cold cache does pay the full first-call price.

**The ruling: the regression ships.** Implementing the remedy now would make `spt_profile_save` the
first export off the shared `run_generator_with` ladder, at the tail of a phase whose whole value is
mechanical parity. The **framed-request alternative is re-opened as a named follow-up** instead —
frame the save request the way the load response is framed, and/or hand the wrapper UTF-8 bytes so
`WriteRawValue`'s `ReadOnlySpan<byte>` overload skips the transcode. The MD5 dirty-check gates all of
this to profiles that actually changed.

**The load side was not separately timed and no claim is made about its latency — but that covers
timing, not allocation, and the allocation is not nothing.** Where the save path's new buffer
replaced an old one, the load path's are pure addition: `DeserializeFromFileAsync`
(`Utils/JsonUtil.cs:102-113`) opened a `FileStream` with `bufferSize: 4096` and streamed it into
`JsonSerializer.DeserializeAsync`, so no full-size buffer of the profile ever existed. The native
path materialises three, all transient:

- `fs::read` (`profile.rs:133`) reads the whole file into a `Vec`;
- `encode_load_frame` (`profile.rs:154-165`) copies those bytes into a second, exactly-sized `Vec`
  for the frame — deliberately, so `write_buffer`'s `into_boxed_slice` does not realloc;
- `ParseProfileFrame`'s `span[at..].ToArray()` (`SptNative.cs:598`) copies them a third time onto the
  managed heap, because the native buffer is freed as soon as the wrapper returns.

On the same 26.50 MB profile that is ~80 MB of churn per load against approximately zero before. At
most two are live at once — the read buffer and the frame during `encode_load_frame`, then the frame
and the managed array during `ToArray` — so the concurrent peak is ~53 MB, not ~80 MB. Two of the
three are native, so `GC.GetTotalAllocatedBytes` would not see them; none of this is on the
save-side follow-up's path.

## Load-epoch seeding — the first publish, skipped

`c7af978` — 2026-08-26. On a modless boot `DatabaseImporter` follows the native load with a
configs-only publish and hands `DbPublisher` the resulting epoch, so the first `EnsureCurrent`
returns without publishing. **Full-boot publishes go 2 → 1**, not 1 → 0: `AdjustLocationBotValues`
bumps the stamp later in `PostDbLoadService`, so the publish `RagfairCallbacks` forces stays.

Nothing committed measures this. `DatabaseImportBenchmarkTests` times the load walk and never the
publisher, and `ScavCaseBenchmarkTests`' 730–745 ms `ForcePublish` arm is a *warm, in-process*
publish — a scale reference, not a boot measurement. So this section is a boot harness rather than a
fixture: `dotnet publish -c Release` into three kept trees, `./mpex-server` booted from each in turn,
`/health` polled at 100 ms and the console watched for `Server has started, happy playing`. Console
logging was raised to `Debug` with a `[%time%]` prefix in all three trees, identically, for the phase
timeline. Six rounds, arms **interleaved within each round** so page-cache and thermal drift hit all
three equally — an un-interleaved first pass had the pre-change tree written to disk last and read
about a second faster for it, which inverted the headline.

| arm | tree |
|---|---|
| **seeded** | this branch, `c7af978`, native import |
| **pre-change** | `a250382`, the merge base with `dev`, native import |
| **legacy** | `c7af978` with `"forceLegacyDatabaseImport": true` added to the published `SPT_Data/configs/core.json` — and `checks.dat` regenerated over that tree (`cargo run --bin gen_checks <dir>/SPT_Data`), because Release verification hashes `configs/` too and rejects the edited file otherwise |

### Boot wall time, six runs per arm

| arm | to `/health` | range | to `Server has started` | range |
|---|---|---|---|---|
| seeded | 7173 ms | 6532 – 7390 | **10,676 ms** | 9931 – 10,892 |
| pre-change | 6754 ms | 6220 – 7181 | 11,537 ms | 10,955 – 12,020 |
| legacy | 6426 ms | 4949 – 6440 | 11,480 ms | 10,099 – 11,698 |

Medians; every run is in the raw list below.

    seeded      health  6532 6637 7068 7278 7285 7390   started   9931 10140 10572 10779 10887 10892
    pre-change  health  6220 6330 6753 6755 7175 7181   started  10955 10961 11483 11591 12009 12020
    legacy      health  4949 5793 6426 6427 6429 6440   started  10099 11047 11477 11484 11485 11698

**Read the `Server has started` column, not the `/health` one.** `/health` is a minimal API that
answers as soon as Kestrel is bound, and on the pre-change arm Kestrel binds *inside* the first
`EnsureCurrent`: in every pre-change run `/health` answered ~2 s before `PostDbLoadService` finished.
That column therefore measures everything up to the publish and none of the publish, so the seed's
**+419 ms** there is its own load-time cost carrying none of its saving. Against `Server has started`
the seed is **−861 ms against pre-change (−7.5 %) and −804 ms against legacy**.

### Where it moves, from the console timeline

Medians over the same six runs per arm:

| phase | seeded | pre-change | legacy |
|---|---|---|---|
| `Database import took …` | 3398 ms | 2915 ms | 1460 ms |
| `startup callbacks` → `Generating flea offers` | **2776 ms** | 5062 ms | 6404 ms |
| `Generating flea offers` → `Server has started` | 3072 ms | 2192 ms | 2238 ms |

Three movements, and two of them are costs:

- **+483 ms on the import line** — the load-time configs publish, which the importer runs before it
  stops its own stopwatch. **`Database import took Nms` is no longer comparable to any pre-phase
  figure in this file**: on a modless boot it now contains a publish. Compare it to the pre-change
  arm here; never to § Phase 3.
- **−2286 ms in the `PostDbLoadService` block** — the skipped first `EnsureCurrent`. Three times the
  730–745 ms the warm fixture reads, because a boot publish is the process's *first*: cold JIT
  through `DbPayloadProjection`, cold allocator, cold native parse.
- **+880 ms in the flea-offer block** — the saving is not free. Skipping the first publish promotes
  the `RagfairCallbacks` publish to the process's first full six-root publish, so it inherits the
  cold-start cost the skipped one used to absorb. Net of all three: **−923 ms**, which is the −861 ms
  wall-clock delta to within run-to-run noise.

**Legacy and pre-change land in the same place** — 11,480 against 11,537 ms — which is § Phase 3's
regression restated at boot scale: the native import costs 1455 ms more than the C# one and hands
1342 ms of it back in the `PostDbLoadService` block, because whichever arm touches the resident DB
first pays the parse and the derive. The seed is the first change that takes that work off the boot
instead of moving it.

### The deterministic half

Wall clock is noisy; the log lines are not. Every seeded Release boot above prints, in order:

    Resident database seeded at epoch 2; a modless boot skips the first publish.
    Load-time seed consumed at epoch 2; first publish skipped.

and never `Load-time seed voided:` — in every seeded Release boot taken for this section (the six
tabulated runs and the discarded un-interleaved pass) the stamp did not move between the importer's
read and the first `EnsureCurrent`. Only Release boots carry that evidence: when these runs were
taken a Debug publish had no Ceciler write barriers, so a Debug smoke boot could not have printed
the voided line no matter what wrote to the database — vacuous as proof. Since corrected on this
branch: `IsPublish` is defined for every publish, and a build without barriers never seeds at all,
so wherever the seeded/consumed pair appears the tripwire behind it is live. Epoch 2 is the load's epoch-1
five-root install plus the configs-only publish. A voided line means some pre-`GameCallbacks` write
moved the stamp and the first publish came back; it is the line to grep after any change to
`PostDbLoadService` or to a startup `IOnLoad`.

Machine as § Machine. Two trees were built — seeded and pre-change — both with the pinned
`rustc 1.98` and .NET SDK 10.0.110; the legacy arm is a copy of the seeded publish with the config flag
flipped, so it runs the seeded arm's binaries and differs only in `core.json` and `checks.dat`.

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

Every figure in this file is from one machine. Each section names its own commit and sitting; only
within a section are figures directly comparable.

| | |
|---|---|
| CPU | AMD Ryzen 5 5600H (6C/12T) |
| RAM | 23 GB |
| OS | Linux 7.1.8-200.fc44.x86_64 (Fedora 44) |
| .NET SDK | 10.0.110 |
| rustc | 1.97.1 for every section below; 1.98 is the pin from `b563367` on, and separates from none of them (§ Toolchain) |

Earlier location-loot figures in this file were taken on a Ryzen 7 5800X3D and have been replaced;
its 2.61x is not comparable to the 2.21x below.

## Toolchain — 1.97.1 against 1.98

`b563367` — 2026-08-26. `rust-toolchain.toml` moved 1.97.1 → 1.98. Taken as a paired A/B rather than
by re-reading the sections below, because those were measured at `06825b3` through `875b2c9`: a
straight re-run would fold a week of commits into the toolchain column. Same commit, same working
tree, same `Cargo.lock`, same machine, one sitting; two passes per fixture per toolchain, medians
below. The control was produced by flipping the channel back, deleting `libspt_native.so` and
rebuilding — verified as `rustc 1.97.1 (8bab26f4f)` against `rustc 1.98.0 (88d9e12ae)`.

| fixture / arm | 1.97.1 (pass 1 / 2) | 1.98 (pass 1 / 2) |
|---|---|---|
| location loot, native | 325.22 / 328.11 ms | 366.28 / 331.25 ms |
| location loot, legacy | 995.15 / 1005.01 ms | 1016.27 / 1022.10 ms |
| airdrop loot, native | 1.36 / 1.42 ms | 1.41 / 1.39 ms |
| bot assault, native | 3.44 / 4.29 ms | 4.54 / 3.44 ms |
| bot usec, native | 5.27 / 5.27 ms | 5.33 / 5.39 ms |
| ragfair full pass, native | 489.05 / 504.56 ms | 488.34 / 475.86 ms |
| ragfair publish (forced) | 751.80 / 755.80 ms | 756.81 / 744.96 ms |
| repeatable quest, elimination warm | 2.50 / 2.45 ms | 2.42 / 2.41 ms |
| scav case warm (`6271093e…`) | 1.47 / 1.84 ms | 1.51 / 1.62 ms |
| item base class, native | 12.34 / 12.71 ms | 12.69 / 12.38 ms |
| ragfair linked item, native | 39.62 / 42.26 ms | 41.50 / 41.50 ms |
| db import, native total (warm) | 883.7 ms | 887.3 ms |
| loot peak RSS, native | 2512 / 2455 MB | 2473 / 2549 MB |
| `libspt_native.so` | 4.28 MB | 4.22 MB |

**No arm separates.** Every 1.98 median lands inside the 1.97.1 pair's own pass-to-pass spread, in
both directions — the bump is not a speed-up and not a regression at this fixture set's resolution.
The one row that looks like a move is location loot's 366.28 ms, and it is the outlier of its own
pair (331.25 ms on the repeat, against 325.22 / 328.11 control); the legacy arm, which cannot be
touched by a rustc bump, drifted +11 to +17 ms over the same two sittings, which is the noise bar to
read the native column against. The only measured difference is 60 KB off the stripped `.so`.

**Two artifacts this pairing exposed, neither caused by the bump.** Bot `BuildRequest` alternates
bimodally between ~0.23 ms and ~0.84 ms (assault) — and it flips *between passes within each
toolchain* (1.97.1 read 0.24 then 0.84; 1.98 read 0.83 then 0.23), so § Mod-pool ownership's 0.23 ms
is the low mode of a two-mode reading, not a settled median. Cause not chased. Second, the ragfair
regeneration pass reports a projection share above 100% (123.4%), which is the fixture's own
arithmetic over two arms it times separately, not a measurement.

**Not re-run under either toolchain:** Phase 2's startup rows (they need a base-commit worktree
build) and Phase 5's profile-save rows (that harness was throwaway and never committed). Both stay
pinned to their own commits.

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

That last sentence has since been settled and this paragraph's premise no longer holds at HEAD:
`[profile.release]` now sets `debug = false` and `strip = true` (`line-tables-only` moved to
`[profile.dev]`), and the same library builds at 4.22 MB (§ Toolchain). The 20.29 MB above and the
21.85 MB below are correct for their own commits and are left as measured.

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

`ecf856b` — 2026-08-20, post configs flip (phase 4, ABI 30). Not re-timed: `LocationConfig` now
resolves per location off the resident configs root, while the two loot multipliers stay on the
varying block because the caller picks them per call.

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

`ecf856b` — 2026-08-20, post configs flip (phase 4, ABI 30). Not re-timed: the reward exports read
the `ItemConfig` sets off the resident configs root, so the config half of the varying block is gone
and only the service half rides on.

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

`9011794` — 2026-08-19, post resident-DB flip (phase 1 flip #6, ABI 27). Same fixture shape and
workload. The native arm now rides the resident DB: `DbPublisher.EnsureCurrent` publishes the five
roots once (absorbed in warmup — this flip added no root) and every timed call sends
`{epoch, shared, bot, template, lootPools}`. What left the wire is the whole database half — the
items table, the `ItemPresets` map, the default-preset ids and the loot pools' handbook prices —
which now lives on the resident `BotDbViews`: the ragfair views by `Arc`, plus two bot-own
derivations at publish (`defaultPresetIdsByTpl`, `expTable`). What still crosses per call is the
shared varying block (config, blacklists, the equipment filters, and the mod-pool slot order —
live `BotEquipmentModPoolService` state, not a database view, so it rides both arms), the bot's
slice, and the caller's pre-filtered `template`/`lootPools`. `BuildRequest` therefore times the
*resident* arm's projection now — the ineligible arm's extra cost is `BuildViewsOverride` on top of
it.

| Role | Path | median | median (2nd run) | mean | min | max |
|---|---|---|---|---|---|---|
| assault | native (rust, resident db) | **13.19 ms** | 13.07 ms | 14.04 ms | 12.57 ms | 18.14 ms |
| assault | legacy (C# 4.1.2) | 2.19 ms | 2.47 ms | 2.69 ms | 0.66 ms | 8.07 ms |
| assault | `BuildRequest` (resident arm) | 6.06 ms | 6.01 ms | 6.16 ms | 4.47 ms | 9.33 ms |
| usec | native (rust, resident db) | **14.24 ms** | 15.41 ms | 14.14 ms | 9.72 ms | 20.00 ms |
| usec | legacy (C# 4.1.2) | 1.38 ms | 1.50 ms | 2.61 ms | 0.99 ms | 17.56 ms |
| usec | `BuildRequest` (resident arm) | 7.69 ms | 8.03 ms | 8.47 ms | 3.47 ms | 20.51 ms |

Speedup: **0.17x** assault (0.19x), **0.10x** usec (0.10x), against the pre-flip **0.02x** for both.
Projection share of the native median: **46.0%** (assault), **54.0%** (usec); 46.0% / 52.1% on the
second invocation. Against the pre-flip `06825b3` medians the native arm shed 77.1 ms of 90.32
(assault, **6.85x**) and 41.7 ms of 55.98 (usec, **3.93x**) — the flip's claim, and the largest
*absolute* per-call saving Phase 1 produced (scav case's 23.4x is the bigger ratio, off a 39 ms
base). The projection itself fell 17.46 → 6.06 ms (assault, 2.88x)
and 15.82 → 7.69 ms (usec, 2.06x); it is now roughly half the native call rather than a fifth,
because the part that shrank is the part it used to dominate. The remaining ~13-14 ms is the
varying block's build and serialise, the template and loot-pool projection, the FFI round trip and
the native generation, unsplit by this fixture — bots stay slower than the ~1.4-2.5 ms of legacy
C#, and `BotConfig.ForceLegacyBotGeneration` remains the opt-out. Legacy drifted up against its own
pre-flip reading (1.99 → 2.19 assault, 1.29 → 1.38 usec) on unchanged code, which is the noise bar
these medians are read against.

Wire volume, one `pmcUSEC` bot: **589,344 bytes (0.56 MiB)** on the eligible arm against
**4,208,129 bytes (4.01 MiB)** with the views override — **7.1x**. Of what remains, the mod-pool
slot order was 26,428 bytes (25.8 KiB) — called the single largest member still crossing per call
here, which was wrong when written and is retracted two paragraphs down, and off the wire entirely
since ABI 32 (§ Mod-pool ownership). (Measured once off
`BotPayloadProjection.BuildRequest(...)` serialised with and without
`BuildViewsOverride`; no committed fixture reports it — `BotPayloadSizeTests` pins the *override*
arm's budget by design, since that is the wire a regression would inflate.)

Forced publish, the first eligible send's cost, never averaged into the medians above: **732.90 –
745.08 ms** median across the five recipes of `ScavCaseBenchmarkTests`' publish-cold arm (cold −
warm: 731.30 – 743.23 ms), against flip #5's 733.68 – 744.80 ms on the same fixture. Unchanged
within noise: this flip added no root, and `BotDbViews`' derivation is an `IndexMap` re-key plus one
`Select` over the exp table. No bot fixture has a publish arm — both bot fixtures absorb the publish
in warmup — so the number is read off the scav case fixture, which republishes all five roots per
timed run.

`ecf856b` — 2026-08-20, post configs flip (phase 4, ABI 30). Not re-timed, but re-weighed: twelve
config slices and the two equipment blacklists left the shared varying block for the resident configs
root, taking 138,121 bytes off every eligible send (§ Phase 4), while `BotConfig.Equipment` stays on
the wire because `ReplayRandomisationClamps` writes into it after every send — which also corrects
the block above: `modPoolSlotOrder` was never the largest member crossing per call, since `equipment`
measures 39,811 B off a projection expression that is character-identical at `9011794`, so it led
there too and the Phase 4 reading is not a new lead.

### Mod-pool ownership (ABI 32)

`875b2c9` — 2026-08-22, after Rust took the mod pools outright. `BotBenchmarkTests`, assault,
`BuildRequest` only, n=20 after 2 warmups. The "before" half is a **fresh reading taken at
`07ecc91`**, not the `9011794` table's 6.06 ms: the pair below is one machine, one day apart, one
command, and is comparable to itself rather than to the blocks above.

```
assault BuildRequest only    n=20  mean=5.30 ms  median=5.19 ms  min=3.82 ms  max=7.17 ms   (07ecc91, before)
assault BuildRequest only    n=20  mean=0.27 ms  median=0.23 ms  min=0.21 ms  max=0.41 ms   (875b2c9, after)
```

**5.19 → 0.23 ms, 22.6x.** One run of n=20 per arm on each date, not a repeated invocation like the
tables above. The two ranges do not overlap — the after-max (0.41 ms) is an order of magnitude below
the before-min (3.82 ms) — so unlike most deltas in this file this one is not a reading of noise.
The control is the native arm rather than an unchanged-code one: it fell 9.14 → 3.47 ms over the
same pair of runs, a 5.67 ms drop against `BuildRequest`'s 4.96 ms, and the ~0.7 ms balance is the
serialise/deserialise of the 26,428 B that left the wire with it. A host that had simply got faster
would not produce that arithmetic. What left `BuildRequest` is `BuildModPoolSlotOrder`: a walk of
the whole `ItemHelper.TemplateTable.Items` table, one `GetModsForGearSlot` per tpl plus a
`GetModsForWeaponSlot` behind an empty-gear-pool check.

**This is the single-bot path only.** `BotWaveBatcher` calls `BotPayloadProjection.BuildSharedVarying`
directly and never `BuildRequest` (`BotWaveBatcher.cs:460`), so the batch arm's saving — the same
walk, once per wave instead of once per bot — is not in this number, and no fixture reports it.

Wire, by arithmetic rather than a fresh probe: `shared.modPoolSlotOrder` measured 26,428 B (the
Phase 4 member table above) and the member no longer exists, so 26,428 B leaves the shared varying
block per send. A send is one FFI call, so that is per bot on the single-bot path and per wave on
the batch one.

The time share of `BuildModPoolSlotOrder` had never been measured separately from the rest of
`BuildRequest`, so the before/after pair above is the first decomposition of that projection into a
member and a remainder — and the remainder is 0.23 ms, which is the more useful half of the result.

### Equipment split (ABI 34)

`f3586ac` — 2026-08-26, after `BotConfig.Equipment` went resident. What stays on the wire is the
`liveEquipmentMods` overlay: role → band → `EquipmentMods`, the only cells a barrier-invisible
runtime writer touches and Rust reads.

**Wire, by arithmetic rather than a fresh request probe.** `shared.equipment` measured **39,811 B**
and the member no longer exists on the varying block, so that is what leaves an eligible send. The
"before" figure is the `shared.equipment` row of § Phase 4's member table — **that table is
pre-mod-pool, so its request totals no longer stand, but the row does**, because the projection
expression behind it never changed between the two readings. The overlay that replaced it measures
**777 B** off shipped `bot.json`: five bands across two roles
(`pmc` 4, `exusec` 1), the only roles whose `randomisation` list carries a non-null `equipmentMods`.

**Net: 39,811 − 777 = 39,034 B off every eligible send** — per bot on the single-bot path, per wave
on the batch one, since a send is one FFI call. The override arm nets approximately zero and is very
slightly worse: the member moves from `shared` into `viewsOverride` inside the same request and the
overlay is added on top, so an ineligible send grows by those 777 B.

Measured once with a throwaway Release probe over `SptNativeBotWireTests`' request (it builds both
arms off the live database), serialised through `JsonUtil.JsonSerializerOptionsNoIndent` — the same
route the flip-6 and Phase 4 byte figures took, because no committed fixture reports bytes. That
probe also read the relocated member at 39,631 B on the override arm, 0.5% under the Phase 4 row,
which is the cross-check on using that row as the "before".

**None of this is observable in `BotPayloadSizeTests`, by design.** Both of its fixtures build the
override arm, which is the wire a regression would inflate — and that is exactly the arm where the
member only changes address. `BatchAmortisesTheSharedBlock`'s `< single/9` bound is unaffected in
kind — both sides of that comparison move by the same 777 B — and stays as written, as does
`RequestStaysUnderTheWireBudget`'s 4,300,000 B ceiling.

**`BuildRequest` wall time — unchanged by this split, which the pair shows only after
decomposition.** Same fixture and command as the mod-pool pair above (`BotBenchmarkTests`,
`[Explicit]`, Release, n=20 after 2 warmups), before at `a8535ba`, after at `f3586ac`:

```
assault BuildRequest only    n=20  mean=0.29 ms  median=0.24 ms  min=0.20 ms  max=0.44 ms   (a8535ba, before)
assault BuildRequest only    n=20  mean=0.95 ms  median=0.84 ms  min=0.76 ms  max=1.62 ms   (f3586ac, after)
usec BuildRequest only       n=20  mean=0.13 ms  median=0.12 ms  min=0.09 ms  max=0.38 ms   (a8535ba, before)
usec BuildRequest only       n=20  mean=0.14 ms  median=0.13 ms  min=0.11 ms  max=0.21 ms   (f3586ac, after)
```

usec is flat. Assault reads 3.4x worse and reproduced (0.81 ms median on a second invocation), so it
is not noise — but it is not this change either. A decomposition probe in the same Release build
timed the parts of assault's `BuildRequest`: `BuildSharedVarying`, the only method the split touched,
at **0.01 ms** (both roles), against `BuildLootPools` at **0.63 ms of a 0.66 ms total**. That probe
was a throwaway edit and is **not committed**, so those part-times are not reproducible from the
tree; the durable evidence is diff scope — `git diff a8535ba..HEAD` touches `BotPayloadProjection`
in three hunks (two methods: `BuildSharedVarying`, whose docblock splits into a hunk of its own, and
`BuildViewsOverride`) and nothing else on the C# request path, while `BuildLootPools`,
`BotLootCacheService` and the rest of that path are byte-identical across the two commits. Read the
assault pair as loot-cache and host state, not as a projection regression. Note also what
`BuildRequest` never contained: the 39,811 B member's *serialisation* cost is paid in the wrapper's
serialise step, which this fixture does not time, so the wire saving does not show up here at all.
The `native (rust)` series the same invocation prints was **not read** for this change, so the
split's one new native-side cost — `resolve_equipment` cloning the full 59-role equipment graph once
per single-bot call, including the override arm where the merge is provably a no-op — is unmeasured;
a `Cow` for the empty-overlay case is the booked follow-up (PR #20's carryover comment) if it ever
shows up.

**Publish delta: expected ~zero, and only its shape was confirmed.** The configs root already carried
the equipment JSON at every publish — it landed in `BotConfigLift.extra`, parsed but unread — so the
new cost is a typed parse of bytes that were already crossing and already being parsed into a map. No
publish arm was re-timed for this change.

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

`9011794` — 2026-08-19, post resident-DB flip (phase 1 flip #6, ABI 27). Same fixture shape and
workload. **All three arms still send the views override** (`epoch: 0`): what `WaveCostPerBot`
exists to measure is batch-vs-per-bot equivalence, not residency, so it holds the database half
constant on both sides of the comparison — the resident/override equivalence gate is
`BotResidentDbTests`. Read this table as the *ineligible* (modded, untrusted) server's wave cost,
and the single-bot block above as the eligible one.

| wave | serial per-bot | `.AsParallel()` per-bot | batched (rayon) | batched vs parallel |
|---|---|---|---|---|
| 45 | 46.34 / 46.50 ms | 13.97 / 14.33 ms | **1.85 / 1.65 ms** | 7.55x / 8.70x |
| 20 | 47.03 / 47.49 ms | 13.55 / 12.96 ms | **3.72 / 2.86 ms** | 3.64x / 4.53x |
| 10 | 46.44 / 46.52 ms | 13.08 / 12.94 ms | **5.33 / 5.10 ms** | 2.45x / 2.54x |
| 5 | 46.47 / 46.22 ms | 14.06 / 14.46 ms | **10.34 / 10.68 ms** | 1.36x / 1.35x |
| 1 | 44.31 / 41.63 ms | 43.91 / 45.01 ms | **49.13 / 48.52 ms** | 0.89x / 0.93x |

All figures ms per bot; request bytes per bot unchanged at 3.81 MiB single-bot against 0.08 (wave
45), 0.19 (20), 0.38 (10), 0.76 (5) MiB batched — the override arm's wire did not move, because
every member the flip touched was renamed or re-parented, not added or dropped: the database half
became `viewsOverride`, the rest became `SharedBotVarying`, and `modPoolSlotOrder` went with it (it
left the wire entirely at ABI 32 — § Mod-pool ownership).
The timings are **not** flatly "within spread", and the direction is worth recording: against
`ae325d8` the two unchanged-code arms fell in **10 of 10** cells each (serial to 46-47 ms from
48-50, parallel to 13-14 from 15-16) while the batched arm — the only one this flip reshaped — rose
in **9 of 10**, by +4-9% in seven of them. Most of that is inside the 9.5-31% drift the arms above
show anyway, and the one cell that is not — wave 20's first invocation at 3.72 ms, +44-47% on the
pre-flip pair — is contradicted by its own repeat (2.86 ms, +10.4%), so a single outlier against a
30% intra-day spread is the likeliest reading. But a consistent small rise on the one arm that
changed has a plausible mechanism: the override arm's payload gained a level of nesting (its
database half moved from the request's top level down into `viewsOverride`, the rest into `shared`),
which is native-side deserialization cost, not generation. Unresolved at this fixture's resolution;
re-read it before the next reshape of this wire rather than treating it as settled noise.

The eligible wave's wire is the number this table cannot show, measured off the same projection as
the single-bot figure above (`pmcUSEC`, one level band):

| wave | resident per-bot | views-override per-bot |
|---|---|---|
| 45 | **13,341 B (0.013 MiB)** | 93,758 B (0.089 MiB) |
| 20 | **29,707 B (0.028 MiB)** | 210,647 B (0.201 MiB) |
| 10 | **59,167 B (0.056 MiB)** | 421,046 B (0.402 MiB) |
| 5 | **118,087 B (0.113 MiB)** | 841,844 B (0.803 MiB) |
| 1 | **589,444 B (0.562 MiB)** | 4,208,229 B (4.013 MiB) |

A flat **7.0-7.1x** across every wave size, which is what "the shared block is effectively 100% of
the request" implies: the flip shrank that block, and the slice was already noise. At wave 45 an
eligible server crosses 13 KB per bot where the pre-flip wire crossed 94 KB.

`ecf856b` — 2026-08-20, post configs flip (phase 4, ABI 30). The three timed arms are unaffected —
they all send the views override, whose wire did not move — but the eligible column above is
superseded by § Phase 4's, which reads 10,272 B at wave 45 against the 13,341 B here.

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

`ecf856b` — 2026-08-20, post configs flip (phase 4, ABI 30). Not re-timed: `RagfairConfig.Dynamic`
and the config blacklist now come off the resident configs root — which also retired the
`customMoneyTpls` divergence — and the service half of the varying block rides on.

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

`ecf856b` — 2026-08-20, post configs flip (phase 4, ABI 30). Not re-timed: the `QuestConfig` maps and
the `ItemConfig` sets went to the resident configs root, while the caller-selected `repeatableConfig`
stays on the varying block.

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

`ecf856b` — 2026-08-20, post configs flip (phase 4, ABI 30). Re-timed as the phase's publish arm
(§ Phase 4): the whole `ScavCaseConfig` block moved to the resident configs root, the warm arm holds
at 1.54–1.93 ms, and the publish now carries six roots at 736.7–749.4 ms.

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

## Map/raid setup

`55fbbd8` — 2026-08-27. **There is no fixture and there will not be one.** Phase 5's decision 11 is
the precedent: take the cheap number rather than argue the cost away, but do not build an
`[Explicit]` harness for a family that runs at menu and raid-start frequency — once per raid-time
query, once per raid start — where no throughput exists to win. So the figures below are the free
ones the parity run already prints, not a measurement.

Two Release invocations of `dotnet test -c Release --filter "FullyQualifiedName~RaidAdjustment"`,
72 tests, **11.60 s and 11.81 s** total wall — most of which is the shared database-load fixture the
suite builds once. Per-test, as the `verbosity=detailed` invocation reports them:

**The run predates the family's fifth export.** `spt_apply_pmc_wave_changes` landed at ABI 36 and is
deliberately unmeasured — TODO.md #4 is a completeness-only port, so the ruling below (no fixture)
covers it too. The four names in the round-trip row are the four that existed at `55fbbd8`, not the
family as it stands.

| Case | reads |
|---|---|
| `AScavRequestMatchesOnBothPaths` — both arms of `GetRaidAdjustments` on a shipped map plus a whole-map compare | **< 1 ms** (3 of 4 cases; lighthouse at seed 42 read 2 ms) |
| `*RoundTripsThroughTheRealLibrary` — one request build + one FFI crossing + one response decode, per export | **< 1 ms** (`spt_get_raid_adjustments`, `spt_adjust_extracts`), **1 ms** (`spt_make_adjustments_to_map`, `spt_adjust_bot_hostility_settings`) |
| the map-shaped parity cases (`MapAdjustmentsMatchOnBothPaths` and siblings) | 8 – 13 ms |

**That 8–13 ms band is the harness, not the call.** Every map-shaped case clones the whole
`LocationBase` twice — once per arm — and serializes both clones to compare them, and the control for
it is already in the table: `ANonScavSideIsANoOpOnBothPaths` reads **10 ms** for a case where
*neither* arm adjusts anything at all.

**No throughput claim is made, and no speedup figure exists.** Nothing in this run separates the two
arms — the parity cases time both together and are dominated by the compare, and NUnit's per-test
resolution bottoms out at "< 1 ms", which is where the interesting calls sit. The number these
figures support is the only one the family needs: a raid's setup crossing costs on the order of a
millisecond, against a raid.

## Caveats

- **Spread is wide.** Treat differences under ~10% between runs as noise; no outlier rejection, no
  confidence intervals.
- **One map** for location loot: `bigmap`.
- **A mod changes what is measured.** `UseLegacyPath()` also returns true when any frozen 4.1.2
  protected member carries a live Harmony patch, so such a server runs legacy regardless of these
  numbers.
- **No historical series.** Figures cannot be diffed against an earlier commit without re-running it
  on the same machine.
