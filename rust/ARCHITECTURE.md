# Architecture — `rust/`

A Cargo workspace with two members. `spt-native` is the port, built as a `cdylib` (plus `rlib`, so the tests
can link it). It is a **port of C# server logic**, not a new subsystem: every module
stands in for a named `SPTarkov.Server.Core` file and is expected to produce byte-identical output. The C# side
of the boundary is `Libraries/SPTarkov.Server.Core/Native/` — `NativeMethods.cs` and `SptNative.cs`, plus the
per-family payload projections under `Bot/`, `Loot/`, `Ragfair/`, `RepeatableQuests/` — **except the log
pipeline**, whose P/Invoke lives in a different assembly: `Libraries/SPTarkov.Common/Native/NativeMethods.cs`
and `Common/Logger/SPTLoggerDispatcher.cs`.

`spectre-facade` has nothing to do with the port. It is a `dotnetdll` program that emits a facade
`Spectre.Console.Ansi.dll` exposing only `Spectre.Console.Color`, because the frozen 4.1.2 mod surface has that
type baked into `ISptLogger<T>`, `SptLogMessage`, `ClientLogRequest` and `Watermark.Draw`, and a compiled mod's
typeref can only be satisfied by an assembly of that name. ~520 lines, built by the `BuildSpectreFacade` target
in `SPTarkov.Common.csproj` — every build, but incrementally, so an unchanged facade is not re-emitted. Its own
header covers the fidelity gaps. Everything below is about `spt-native`.

Build coupling and cross-RID rules live in [CLAUDE.md](../CLAUDE.md); the boundary as seen from C# is in
[ARCHITECTURE.md](../ARCHITECTURE.md) under *Native Rust layer*. This file covers what's inside `spt-native`.

Toolchain is pinned in `rust-toolchain.toml` (1.97.1, edition 2024). Dependencies are:
`serde`/`serde_json` (with `preserve_order`, so untyped maps keep C# `Dictionary` insertion order),
`rmp-serde` (the ragfair MessagePack envelope), `indexmap`, `rand`/`rand_xoshiro`, `rayon`, `tokio`,
`walkdir`, `xxhash-rust`, `base64`, `regex-lite` (the `sptLogger.json` filter patterns — deliberately
-lite, so .NET-only syntax degrades to never-match) — plus `tempfile` as the only dev-dependency (the
`verify` FFI tests need a real directory). `spectre-facade` pulls in `dotnetdll` and nothing else.
`Cargo.lock` is committed.

`.cargo/config.toml` pins `-C target-cpu=x86-64-v3` on both x64 targets, so the built library will not run on
pre-AVX2 hardware, and the mold linker on Linux. Both workspace profiles use one codegen unit; release adds fat
LTO.

Roughly 48k lines across the 51 files of `src/`, inline tests included. `src/bot/` is ~35% of that and
`bot_equipment_mod_generator.rs` alone ~4.2k; `src/loot/` ~25%, `src/ragfair/` ~15%,
`src/quest/` ~12%, `src/scav_case/` ~4%.

## Layout

| Path | Role |
|---|---|
| `src/lib.rs` | Module roots and `ABI_VERSION` (currently 19; must equal `SptNative.ExpectedAbiVersion`) |
| `src/ffi.rs` | The C-ABI surface. The **only** module containing `unsafe` |
| `src/runtime.rs` | Process-wide multi-thread tokio runtime, `OnceLock`-built. Used only by `verify` |
| `src/verify.rs` | Hashes `SPT_Data` with XXH3-128 and diffs it against `checks.dat` |
| `src/bin/gen_checks.rs` | The bin that writes `checks.dat`, over `verify::generate`. Release builds only |
| `src/logger.rs` | The log pipeline: `sptLogger.json`, filters, level gates, per-target formatting — and the console sink |
| `src/log_sink.rs` | The file sink alone: where a formatted line lands on disk, with its rotation and archiving |
| `src/diag.rs` | The generator families' way into that pipeline: the locale table, the render, and `DiagSink` |
| `src/loot/` | Location loot (static containers, loose loot) and reward loot (airdrops, cases, containers) |
| `src/bot/` | One bot's entire inventory: equipment, mods, weapons, magazines, loot |
| `src/ragfair/` | One batch of dynamic flea offers: the assort walk, pricing, barter schemes, the offers |
| `src/quest/` | One repeatable quest of any of the four types, its rewards, and the mutated quest-type pool |
| `src/scav_case/` | One scav case craft's rewards: the reward and ammo pools, the per-rarity picks, the money/ammo/preset arms |
| `src/base_class.rs` | The whole `ItemBaseClassService` cache in one call, over `src/loot/item_helper.rs`'s `ItemBaseClassCache` |

## FFI boundary (`ffi.rs`)

Nineteen `extern "C"` exports. Two are trivial (`spt_native_abi_version`, `spt_buf_free`); twelve take a UTF-8
JSON generation request; `spt_verify_database` takes a UTF-8 directory path instead. All thirteen of those hand
back a heap buffer the caller releases with `spt_buf_free`. `spt_locales_set` takes the resolved server-locale
table as UTF-8 JSON and buffers a parse error, or panic text since ABI 18. The last three are the log pipeline
(`spt_logger_init`, `spt_log_emit`, `spt_logger_close`): `spt_logger_init` takes the raw `sptLogger.json`
bytes and hands back a buffer only on failure, `spt_log_emit` passes one line's fields directly rather
than a JSON document, and `spt_logger_close` takes nothing — see *The log pipeline* below.

```
C# SptNative → spt_generate_* (JSON in)
  → serde deserialize into a request envelope from loot/, bot/, ragfair/, quest/ or scav_case/models.rs
    (base_class.rs carries its own)
  → catch_unwind( generator )
  → serde serialize the result, or the failure message (LootError or panic text), into an out-buffer
```

- `run_generator_with` is the shared body of the twelve generation exports; nine reach it through
  `run_generator`, the JSON-response-plus-`LootError` wrapper. Ragfair, quest and scav case call it
  directly — ragfair to frame its response instead of emitting one JSON document, the other two for
  their own error types.
  `spt_verify_database` is separate because it blocks on the tokio runtime.
- Status codes: `STATUS_OK` 0, `STATUS_BAD_ARGS` 1 (null pointer, bad UTF-8, unparseable JSON),
  `STATUS_PANIC` 2 (the panic message is in the out-buffer since ABI 18), `STATUS_ERROR` 3,
  `STATUS_STALE_SLICE` 4 (ragfair and quest only — see below).
  **Quest and scav case are the exceptions to 2**: `quest/mod.rs` and `scav_case/mod.rs` catch the
  generator's panic themselves and report it as `STATUS_ERROR` 3 carrying the panic message, because
  those families port a C#-sanctioned throw as an `.expect` — a generation failure, not a library
  bug. The cost is that a real port bug in those families (an index panic in the Rust) also arrives
  as 3, indistinguishable from a sanctioned generation failure, instead of reaching `SptNative.cs`'s
  "this indicates a native library bug" wording. Deliberate: a sanctioned throw is a generation
  failure and must read as one.
- **Two requests have a cached half**, ragfair's and the repeatable quest's. Each arrives as
  `{invariantStamp, invariant?, varying}`; `src/ragfair/slice_cache.rs` and
  `src/quest/slice_cache.rs` each hold the last parsed invariant slice under the stamp it came with,
  in **separate** slots, so a repeat pass can omit it. A slice-less request whose stamp the cache does
  not hold returns `STATUS_STALE_SLICE` and the C# caller retries once with the slice included. Those
  two slices are the only request data the crate holds across calls; every other payload is still
  projected per call.
- **A buffer is written on failure too** — the parse error, the `LootError` message, or (since
  ABI 18) the panic text. Ownership is decided by the out-pointer being non-null, never by the
  status code. `spt_verify_database`'s free-on-success-only shape must not be copied into the
  generators.
- `catch_unwind` on every fallible path: a Rust panic never unwinds into the CLR.
- **Only the failure message crosses the buffer.** On the `LootError` path `run_generator` writes
  `error.message` and nothing else — but the run's diagnostics are already in the log, emitted as
  they happened through `diag::DiagSink`, so a failure no longer loses what led up to it. The error
  text itself is still the C# caller's to log.

Adding an export means bumping `ABI_VERSION` and `SptNative.ExpectedAbiVersion` together; a test in `ffi.rs`
asserts the constant so the bump can't be forgotten.

## `src/verify.rs`

`checks.dat` is base64-wrapped JSON (`Path`/`Hash` pairs) written at Release build time by `generate` in this
same module, via the `gen_checks` bin (`src/bin/gen_checks.rs`, invoked by the `PreBuildHashFile` target in
`SPTarkov.Server.Assets.csproj`). Three properties of `verify` are load-bearing:

- **Scope comes from the manifest, not the tree.** Only the top-level `SPT_Data` roots the manifest names
  (`configs/`, `database/`) are walked. The build relocates unhashed artifacts into the output `SPT_Data`
  (satellite assemblies, admin-panel `wwwroot/`) and `generate` leaves `images/` and `checks.dat` out
  deliberately — walking everything would fail on all of them.
- **The check runs both directions.** Disk files are hashed against the manifest, and manifest entries with no
  walked file are reported as `missing_from_disk`, so a deletion (or a symlink the walk skips) can't pass.
- **Empty manifest fails closed** rather than verifying nothing.

Hashing fans out over the runtime through a `JoinSet` capped at `MAX_CONCURRENT_HASHES` (32). The result is a
`VerifyReport { ok, failures, checked }`.

## The log pipeline (`src/logger.rs`, `src/log_sink.rs`)

The one ported family with **no legacy path**: C# no longer has log handlers at all. `spt_logger_init` parses
the raw `sptLogger.json` bytes once per process; `SPTLoggerDispatcher.Log` then hands each line's fields across
`spt_log_emit`, and the crate owns filter matching, the level gate, per-target format expansion and the sinks.

Init and close are **ref-counted**, not idempotent: a second init keeps the running pipeline and ignores the
new config, but it bumps the count, and the pipeline is only drained and torn down by the last of as many
`spt_logger_close` calls as there were successful inits. That is what lets the prepatcher's nested
`Program.Main` dispose its own container while the outer host keeps logging.

Two rules the C# side depends on: **logging never fails the server** — a bad config or a broken library is one
stderr notice and every later emit is a silent no-op — and **an emit never blocks on I/O**, because each file
gets a background writer thread fed over a bounded channel that drops lines rather than growing without limit.
`log_sink.rs` also owns rotation and the archive cap — one owner for both, so the cap is enforced at the
rotation rather than on a timer.

The generator families feed the same pipeline from the inside rather than over `spt_log_emit`. `diag.rs`
renders a locale-keyed diagnostic against the table C# pushes once over `spt_locales_set` after the database
import — a startup snapshot, and a missing table or key leaves the key itself as the text — then hands the
line to the logger through `ffi::emit_pipeline`. Those lines carry a process-local per-thread counter as
`%tid%` rather than a managed thread id, and the Rust thread name (usually empty) as `%tname%`.

## `src/loot/`

| Module | Stands in for | What it does |
|---|---|---|
| `location_loot_generator.rs` | `Generators/Loot/LocationLootGenerator.cs` | `generate_static_containers`, `generate_dynamic_loot` — a map's containers and loose spawn points |
| `loot_generator.rs` | `Generators/Loot/LootGenerator.cs` | `create_random_loot`, `create_forced_loot`, `get_sealed_weapon_case_loot`, `get_random_loot_container_loot` |
| `item_helper.rs` | `Helpers/Items/ItemHelper.cs`, `Extensions/ItemExtensions.cs` | Template lookups, base-class tests, item-tree cloning/re-iding, stack splitting, magazine/ammo-box filling. Also defines `LootContext` and `LootError` |
| `container_extensions.rs` | `Extensions/ContainerExtensions.cs` | 2D grid packing — slot search and marking, ported warts and all |
| `random_util.rs` | `Utils/RandomUtil.cs` | Every draw primitive, bug-for-bug. Also `TestSeedGuard` |
| `probability_object_array.rs` | `Utils/Collections/ProbabilityObjectArray.cs` | Weighted draws over a key pool |
| `mongo_id.rs` | `Models/Common/MongoId.cs` | 12-byte ObjectId generation, byte-for-byte identical layout |
| `math_util.rs` | `Utils/MathUtil.cs` | Linear interpolation and range mapping |
| `models.rs` | `Models/…` | Wire types (see *Conventions*) |

## `src/bot/`

`mod.rs` defines `BotContext<'a>` — the read-only views (items, presets, blacklists, durability config,
equipment filters…) one generation run borrows, plus the `DiagSink` its diagnostics emit through. It is the bot
family's analog of `loot::item_helper::LootContext`.

| Module | Stands in for | What it does |
|---|---|---|
| `bot_inventory_generator.rs` | `Generators/Bot/BotInventoryGenerator.cs` | `generate_inventory` — the orchestrator and the crate's bot entry point — plus `generate_inventory_batch`, one wave in one call over a rayon loop |
| `bot_equipment_mod_generator.rs` | `Generators/Bot/BotEquipmentModGenerator.cs` | Both mod halves (equipment, weapon), plus the one `BotWeaponModLimitService` method they call |
| `bot_generator_helper.rs` | `Helpers/Bot/BotGeneratorHelper.cs`, `BotInventoryContainerService.cs` | Per-item `Upd` blocks, compatibility probes, and the `ContainerGrids` occupancy state |
| `bot_loot_generator.rs` | `Generators/Loot/BotLootGenerator.cs` | Fills pockets/vest/backpack/secure from pools the C# caller resolved |
| `bot_weapon_generator.rs` | `Generators/Bot/BotWeaponGenerator.cs` | Pick, kit out, load a weapon; hand over spare magazines |
| `bot_weapon_generator_helper.rs` | `Helpers/Bot/BotWeaponGeneratorHelper.cs` | Magazine and bullet counts, magazine+ammo item pairs |
| `inventory_mag_gen.rs` | `Generators/Weapons/*` | The four `IInventoryMagGen` strategies, collapsed into one enum with a fixed dispatch order |
| `durability_limits_helper.rs` | `Helpers/Bot/DurabilityLimitsHelper.cs` | Weapon/armor durability rolls |
| `mod_pool_service.rs` | `Services/Bot/BotEquipmentModPoolService.cs` | Slot mod pools, derived per call instead of cached, drawn in the projected C# enumeration order (`modPoolSlotOrder`) |
| `repair_service.rs` | `Services/Commerce/RepairService.cs` | Only `AddBuff`, the one slice bot generation reaches |
| `exhaustable_array.rs` | `Utils/Collections/ExhaustableArray.cs` | Draw-without-replacement |
| `models.rs` | `Models/…` | Wire types |

## `src/ragfair/`

`mod.rs` defines `RagfairContext<'a>` — the items view, presets, handbook and flea price tables,
resolved trader prices and the `dynamic` config block one batch borrows, plus its `DiagSink`.
One native call generates a whole batch of offers, not one offer.

| Module | Stands in for | What it does |
|---|---|---|
| `offer_generator.rs` | `Generators/Ragfair/RagfairOfferGenerator.cs` | `generate_dynamic_offers` — the batch pass, the barter schemes, the offer object, condition randomisation and armor-plate removal |
| `assort_generator.rs` | `Generators/Ragfair/RagfairAssortGenerator.cs` | The assort walk: every flea-sold preset, then every sellable template, as (root + children) lists. Draws nothing |
| `price_service.rs` | `Services/Ragfair/RagfairPriceService.cs` | The pricing math one offer needs — flea/handbook/trader arms, preset rollups, the one biased price draw |
| `server_helper.rs` | `Helpers/Ragfair/RagfairServerHelper.cs` | Stack counts, offer counts, offer currency, item validity |
| `slice_cache.rs` | — | The parsed invariant slice, keyed by the caller's `DatabaseMutationStamp` (see *FFI boundary*) |
| `models.rs` | `Models/Spt/Config/RagfairConfig.cs`, `Models/…` | Config records and the request/response envelopes |

Two crate-internal facts:

- **The walk is parallel only when unseeded.** C# fans one `Task.Factory.StartNew` per assort entry;
  an unseeded batch here fans across rayon the same way (a forked context per entry, merged back in
  assort order with `intId` reassigned). A **seeded** batch stays sequential — the seeded RNG is
  thread-local, so fanning out would drop every worker onto entropy, and parity rides the sequential
  path byte-for-byte. Generation is no longer where the port loses to legacy (see `../BENCHMARK.md`).
- **`GetFleaPricesAsArray`'s cache is re-derived per call.** The C# `AllowedFleaPriceItemsForBarter`
  field is built once per generator instance and never invalidated; here it is rebuilt on every call,
  which makes the native path *fresher* than legacy for runtime-added items.

## `src/quest/`

`mod.rs` defines `QuestContext<'a>` (`:30`) — the items view, base-class cache, handbook/flea/preset price
tables, the reward and seasonal blacklists, boss items and spawns, extracts and location ids, the quest
templates and the levelled Completion white/blacklists, plus its `DiagSink`. Every view is borrowed off the
cached invariant slice. One native call generates **one** quest of one type.

`generate_repeatable_quest` (`:98`) stands in for the type switch of
`RepeatableQuestController.PickAndGenerateRandomRepeatableQuest` (`:390-397`): it takes the slice, installs the
seed guard, dispatches on the requested type, and returns
`QuestNativeResponse { quest, pool }` — the quest **and** the type pool the generator mutated on the way, which
rides back whether or not a quest came out. A `None` quest is a normal outcome (exhausted pool, or a generator
that gave up and logged why), not a failure.

| Module | Stands in for | What it does |
|---|---|---|
| `elimination.rs` | `Generators/RepeatableQuests/EliminationQuestGenerator.cs` | The kill-N-of-X quest |
| `completion.rs` | `Generators/RepeatableQuests/CompletionQuestGenerator.cs` | The hand-over-N-of-X quest |
| `exploration.rs` | `Generators/RepeatableQuests/ExplorationQuestGenerator.cs` | The survive-N-raids quest |
| `pickup.rs` | `Generators/RepeatableQuests/PickupQuestGenerator.cs` | The fetch-N-items-of-a-type quest. Reachable, but no shipped `quest.json` lists `Pickup` in its `types` |
| `reward_generator.rs` | `Generators/RepeatableQuests/RepeatableQuestRewardGenerator.cs` | The reward chain every type ends with: XP, money, GP coins, an optional weapon preset, items, trader standing, an optional skill point |
| `helper.rs` | `Helpers/Quest/RepeatableQuestHelper.cs` | The template clone/placeholder pass each generator opens with, and the level-band config lookups |
| `slice_cache.rs` | — | The quest invariant slice, in its own slot separate from ragfair's (see *FFI boundary*) |
| `models.rs` | `Models/Spt/Repeatable/…`, `Models/…` | Wire types |

This is the one family that ports a C#-sanctioned `throw` as a panic rather than an error value — see
*Conventions*.

## Conventions

These are what keep the port correct; break one and output silently diverges from C#.

- **Every ported module names its C# source in its `//!` header**, with the line range where the port is a
  slice of a larger file rather than the whole of it. (`lib.rs`, `ffi.rs`, `runtime.rs`, `verify.rs` and all four
  `mod.rs` files have no C# counterpart and no header.) Read that header before changing anything.
- **Deviations are marked `Deviation:`, at the scope they apply to** — module header, item doc, or a comment on
  the line itself. Grep that bare form: the bot family bolds it (`**Deviation:**`) and the loot family does not,
  and ragfair and quest currently record none. Only `bot_inventory_generator.rs` and `bot_weapon_generator.rs`
  also collect theirs under a module-level `# Deviations` heading; everywhere else there is no section to read.
- **RNG draw order is a contract.** The bot family states it up front: every generating module opens with an
  ordered list ("*RNG calls, in C# source order — the parity contract*"), including draws C# consumes and
  discards. The loot family documents each draw inline at its call site instead, against the C# line, and so do
  the draw primitives themselves (`random_util.rs`, `probability_object_array.rs`, `exhaustable_array.rs`). Adding,
  removing, or reordering a draw desynchronises the whole sequence, so a "harmless" early-out that skips a roll
  is a bug.
- **The generator families log for themselves, and have one rule for throws.** C# `ISptLogger` calls become
  `Diagnostic` values pushed onto the run's `diag::DiagSink`, which renders and emits each one into the log
  pipeline above as it happens, under the porting module's `CATEGORY` — the `typeof(T).FullName` of the C#
  class it stands in for, so one category per generator. (Tests swap in the sink's `Capture` variant and
  assert the diagnostics as data.) A C# *return-null-and-log* path is ported as a `Diagnostic` plus `None`, in every
  family. A C#-*sanctioned* `throw` is ported one of two ways, by family: loot, bot and ragfair return it as a
  `LootError` (so does an unguarded null deref they would have NRE'd on); the quest family panics at the throw
  site — `panic!` or `.expect` — and catches it at the family entry point (`quest/mod.rs:128`), which carries
  the message across as `STATUS_ERROR`. Scav case does both: a `ScavCaseError` return where the C# throw is
  reachable through a guard, a panic caught at its own entry point (`scav_case/mod.rs:52`) where the C# throws out
  of a dictionary index. Panicking is not unsafe here: every export runs inside `catch_unwind`
  (`ffi.rs:225`), so nothing unwinds past the FFI boundary either way.
- **Wire models come in five families** (`loot/models.rs`, `bot/models.rs`, `ragfair/models.rs`,
  `quest/models.rs`, `scav_case/models.rs`). DB/EFT models mirror C# records field-for-field, pinned
  to the exact `JsonPropertyName`, each with a `#[serde(flatten)] extra` map so
  mod-added fields survive the round trip — the counterpart to the `[JsonExtensionData]` that `Tools/Ceciler`
  injects. Request/response envelopes are a fresh contract and use plain camelCase with no passthrough map.
- **One C# RNG lifetime can span two native calls.** Each generator entry point (not `ffi.rs`) opens the run by
  mapping the request's optional seed through `TestSeedGuard::install`. `generate_dynamic_loot` is the exception: it uses
  `TestSeedGuard::resume`, which picks up the thread-local stream the preceding `generate_static_containers`
  parked under the same seed. C# installs one `SeededRandomSource` for the whole of `GenerateLocationLoot` and
  draws in both phases; the native side is entered once per phase, so a fresh `install` on the second would
  replay the first phase's values. The guard is RAII, so a panic can't leak a seeded stream onto a pooled thread.
- **Caches become per-call derivation.** C# DI singletons keyed by bot id or built once over the whole database
  (`BotEquipmentModPoolService`, `BotInventoryContainerService`) are recomputed per call or handed across the
  boundary by the caller. The unit is one bot, not one raid: the batch export hoists only the views every bot
  in the wave shares (`SharedBotViewsWire`) and still derives the rest per bot, each with its own seed guard.

## Tests

Almost all tests are inline `#[cfg(test)]` modules (~700 of them); the only `tests/` target is
`completion_whitelist_baseclass.rs`, the Completion whitelist shape guard. Three kinds:

- **Parity fixtures** — replay a C# scenario and assert the exact item list.
- **Seeded-RNG tests** — an optional seed on every request envelope (`testSeed`, spelled `seed` on the quest
  one) installs a `TestSeedGuard`, swapping thread entropy for a xoshiro256\*\* stream that is bit-identical to
  `Utils/RandomSource.cs`. Known-answer tests in `random_util.rs` pin it, and `RandomSourceParityTests.cs` pins
  the C# end.
- **FFI transport tests** — `ffi.rs` round-trips the exports through raw pointers; between them they cover
  success, parse failure, generation failure and null arguments. `spt_generate_bot_inventory_batch` is the one
  export with no transport test of its own.

The one exception is `tests/completion_whitelist_baseclass.rs`, a timing-and-equivalence guard for the
Completion whitelist filter's base-class lookups. It runs against the real shipped `items.json`, so it needs
`scripts/decompress-assets.sh` to have run.

Run with `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`.
