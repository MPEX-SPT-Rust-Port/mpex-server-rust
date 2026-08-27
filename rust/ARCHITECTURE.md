# **rust** Architecture

## 1. Overview Summary

A Cargo workspace with three members. `spt-native` is the port and everything below is about it unless
noted: built as a `cdylib` (plus `rlib`, so the tests can link it), it is a **port of C# server logic**, not a
new subsystem — every module stands in for a named `SPTarkov.Server.Core` file and is expected to produce
byte-identical output. `spectre-facade` emits a stub assembly that holds a frozen mod ABI, and `mpex-server`
is the shipped launcher that hosts the CLR. Build coupling and cross-RID rules live in
[CLAUDE.md](../CLAUDE.md); the boundary as seen from C# is in [ARCHITECTURE.md](../ARCHITECTURE.md) under
*Native Rust layer*.

| Language | Lines of Code | File Count |
|-----------|-----------------|-----------|
| `Rust` (whole workspace) | `58,146` | `70` |
| ↳ `spt-native/src/` | `55,579` | `58` |
| ↳ `spt-native/tests/` | `1,953` | `9` |
| ↳ `spectre-facade` | `522` | `2` |
| ↳ `mpex-server` | `92` | `1` |

Inline tests included, `src/` splits: `bot/` ~32%, `loot/` ~23%, `ragfair/` ~15%, `quest/` ~12%,
`scav_case/` ~4%. `bot_equipment_mod_generator.rs` alone is 4.2k.

---

## 2. High Level Design

| Component | Responsibility | Interacts With |
|-----------|-----------------|------------------|
| `spt-native` | The port: generation families, resident DB, database verification, the log pipeline and the terminal. `cdylib` + `rlib` | `Libraries/SPTarkov.Server.Core/Native/` for everything but the log and console exports, which come from `Libraries/SPTarkov.Common/Native/NativeMethods.cs` |
| `spectre-facade` | Emits a facade `Spectre.Console.Ansi.dll` exposing only `Spectre.Console.Color` | Built by `BuildSpectreFacade` in `SPTarkov.Common.csproj`; referenced by five C# projects |
| `mpex-server` | Shipped launcher: hosts the CLR via netcorehost and `run_app`s the published server assembly, argv forwarded. Since Phase 6b it links `spt-native` as an rlib and re-exports its symbols | The published `SPT.Server` assembly; `spt-native`; `Containerfile.release`'s entrypoint |

`spectre-facade` has nothing to do with the port. It is a ~520-line `dotnetdll` program, needed because the
frozen 4.1.2 mod surface has `Spectre.Console.Color` baked into `ISptLogger<T>`, `SptLogMessage`,
`ClientLogRequest` and `Watermark.Draw`, and a compiled mod's typeref can only be satisfied by an assembly of
that name. Built on every build, but incrementally. Its own header covers the fidelity gaps.

**The split-brain rule, post-6b:** exactly one linkage path may be live *per process*. `mpex-server` links
`spt-native` as an rlib and, via `-Wl,--export-dynamic` in `.cargo/config.toml`, carries all 39
`#[unsafe(no_mangle)]` exports in its own `.dynsym`; the resident DB's statics therefore live in the executable.
The published Linux tree ships **no cdylib** (`ExcludeSptNativeFromPublish` in `SPTarkov.Server.csproj`), so
there is nothing for a second copy to come from. The cdylib is still built and still lives in `bin/`, which is
what `dotnet test` and the `SPT.Server` dev executable bind to — a different process, never this one. The C#
resolver enforces the order; see ARCHITECTURE.md § *Native Rust layer*.

What enforces the rule is the **absence of the file**, not a runtime assertion: `GetMainProgramHandle()` hands
back a `dlopen(NULL)` pseudo-handle, so nothing on the C# side can tell which copy a symbol came from. That
matters for exactly one case — running `rust/target/<profile>/mpex-server` against a `bin/` tree, where the
cdylib is present by design. A lost anchor there falls through to it and boots *silently* with the statics in
the wrong place; in a published tree the same mistake is a hard boot failure. The smoke covers the published
tree only.

Two things that follow, and that a reader will otherwise trip over:

- **The launcher must reference `spt_native` in its own source.** An rlib nothing references is discarded whole
  by the linker, taking all 39 exports with it, silently, at link time. `src/main.rs` carries a deliberate
  anchor call and `scripts/smoke-mpex-server.sh` checks the launcher still exports them — retention is
  all-or-nothing, so a nonzero count is the whole check and no export count needs maintaining. Any path
  reference suffices; the anchor is a call behind `black_box` only so that deleting it looks like a behaviour
  change rather than dead-code cleanup.
- **`.cargo/config.toml` is discovered from the working directory, not from `--manifest-path`.** Building with
  `--manifest-path rust/Cargo.toml` from the repo root silently drops every rustflag, including
  `--export-dynamic`, and yields zero exports for reasons unrelated to the anchor. Always `cd rust` first.

Windows is deliberately unchanged: an `.exe` has no export table without `/EXPORT:` args or a `.def` file, so
the publish exclusion is gated on the Linux target and a Windows launcher still resolves through the cdylib.

The C# side of the `spt-native` boundary is `Libraries/SPTarkov.Server.Core/Native/` — `NativeMethods.cs`,
`SptNative.cs` and the per-family payload projections under `BaseClass/`, `Bot/`, `Db/`, `Loot/`, `Ragfair/`,
`RepeatableQuests/`, `ScavCase/` — **except the log pipeline and the console**, whose P/Invoke lives in a
different assembly: `Libraries/SPTarkov.Common/Native/NativeMethods.cs`, with
`Common/Logger/SPTLoggerDispatcher.cs` and `Common/Native/SptConsole.cs` over it.

### `spt-native` module map

| Path | Role |
|---|---|
| `src/lib.rs` | Module roots and `ABI_VERSION` (currently 35; must equal `SptNative.ExpectedAbiVersion`) |
| `src/ffi.rs` | The C-ABI surface. The **only** module containing `unsafe` |
| `src/runtime.rs` | Process-wide multi-thread tokio runtime, `OnceLock`-built. Used only by `verify` and the fused load |
| `src/verify.rs` | Hashes `SPT_Data` with XXH3-128 and diffs it against `checks.dat`. `verify_collecting` is the same walk with a `want` predicate that whole-reads and returns matching files' bytes, so the fused load reads each file once |
| `src/bin/gen_checks.rs` | The bin that writes `checks.dat`, over `verify::generate`. Release builds only |
| `src/logger.rs` | The log pipeline: `sptLogger.json`, filters, level gates, per-target formatting — and the console sink |
| `src/log_sink.rs` | The file sink alone: where a formatted line lands on disk, with its rotation and archiving |
| `src/console.rs` | Terminal control: Windows console setup, title, clear, the stdin line-ending strip |
| `src/diag.rs` | The generator families' way into that pipeline: the locale table, the render, and `DiagSink` |
| `src/db.rs` | The resident DB: the epoch-versioned store of published roots (templates, traders, globals, locations, hideout, configs); a publish re-derives the ragfair, quest and bot views from the tables |
| `src/db/models.rs` | The publish envelope's wire types |
| `src/db/load.rs` | The fused startup load: one walk over `SPT_Data` hashes, reads and assembles the five roots into epoch 1, and collects the eager `database/` file bytes for the C# replica. A plain `async fn` — no CLR needed, so the post-6b exe can call it before booting the runtime |
| `src/loot/` | Location loot (static containers, loose loot) and reward loot (airdrops, cases, containers) |
| `src/bot/` | One bot's entire inventory: equipment, mods, weapons, magazines, loot |
| `src/ragfair/` | One batch of dynamic flea offers: the assort walk, pricing, barter schemes, the offers |
| `src/quest/` | One repeatable quest of any of the four types, its rewards, and the mutated quest-type pool |
| `src/scav_case/` | One scav case craft's rewards: the pools, the per-rarity picks, the money/ammo/preset arms |
| `src/raid/` | One scav raid's setup: the reduced raid time, the loot percents and the train-exit changes, plus the raid-start bot-hostility and scav-extract deltas. The one family that rides no resident DB |
| `src/base_class.rs` | The whole `ItemBaseClassService` cache in one call, over `loot/item_helper.rs`'s `ItemBaseClassCache` |
| `src/linked_items.rs` | The whole `RagfairLinkedItemService` table in one call: the bidirectional slot/chamber/cartridge walk plus the revolver camora-ammo edge case |
| `src/profile.rs` | The disk half of `SaveServer`: list, load, save, delete over `user/profiles/`, with `FileUtil.WriteFileAsync`'s temp-then-rename protocol. Stateless — the directory arrives in every request — and profile bytes are opaque, written and read verbatim. The live `SptProfile` graph, the MD5 dirty-check and `BackupService` all stay C# |

```
C# SptNative → spt_generate_* (JSON in)
  → serde into a request envelope from the family's models.rs
    (base_class.rs and linked_items.rs declare theirs inline instead)
  → catch_unwind( generator )
  → serde out the result, or the failure message, into an out-buffer
```

---

## 3. Low Level Design

### Toolchain and dependencies

Toolchain is pinned in `rust-toolchain.toml` (1.98, edition 2024) and `Cargo.lock` is committed.
Dependencies: `serde`/`serde_json` (with `preserve_order`, so untyped maps keep C# `Dictionary` insertion
order), `rmp-serde` (the ragfair MessagePack envelope), `indexmap`, `rand`/`rand_xoshiro`, `rayon`, `tokio`,
`walkdir`, `xxhash-rust`, `base64`, `regex-lite` (the `sptLogger.json` filter patterns — deliberately -lite, so
.NET-only syntax degrades to never-match), plus `tempfile` as its only dev-dependency. `.cargo/config.toml`
pins `-C target-cpu=x86-64-v3` on both x64 targets, so the built library will not run on pre-AVX2 hardware, and
the mold linker on Linux. Release is `opt-level = 3` with fat LTO, one codegen unit and stripped symbols; dev
trades that for build time — `opt-level = 1`, sixteen codegen units, line-tables-only debug info.

### FFI boundary (`ffi.rs`)

Thirty-nine `extern "C"` exports: two trivial (`spt_native_abi_version`, `spt_buf_free`), seventeen taking a
UTF-8 JSON generation request (the newest four are the raid-setup family's, described after this list),
four taking a profile-persistence request (`{schema, dir}`, plus `id`
on all but `spt_profile_list` and the profile text on `spt_profile_save` — see *`src/profile.rs`*;
`spt_profile_load` returns a framed byte response, `[u32-LE header length][{"found":bool}][file
bytes]`),
`spt_verify_database` taking a directory path, `spt_db_publish` taking the
resident-DB publish envelope, `spt_db_load` taking the fused-load request
(`{schema, dir, verify, handbookPriceOverride?}` — see the bullet below) and
returning a framed byte response — a length-prefixed JSON header naming the verify report, the installed
epoch and each returned file's path and length, followed by the file bodies back to back —
`spt_db_resident_digest` taking no request at all (out-buffer only) and answering
`{"epoch":N,"roots":{"templates":"<16-hex>",…}}`, one canonical digest per resident root over the *typed
lift surface* — the roots serialize in digest mode, which skips the named `extra` overflow maps, so whatever
rides one is invisible to it, and the two arms' extras are *known* to differ (explicit nulls, number
forms, Debug-build model coverage, the projection's narrower hideout root), so a divergence living
entirely in extras passes the gate by design; the dictionary roots' bare flatten maps (`TradersRoot`, `LocationsRoot`) are
*not* overflow, they are the payload, and they are digested —
with absent roots omitted and `{"epoch":0,"roots":{}}` before the first publish; the digests are
test support for the load/projection equivalence gate and no wire contract, so compare two calls within
one process, never across builds or machines —
`spt_locales_set` taking the resolved server-locale table as JSON, and eleven
for the log pipeline and the terminal it owns (`spt_logger_init`, `spt_logger_reinit`, `spt_log_emit`,
`spt_logger_close`, `spt_log_set_tap`, `spt_log_enabled`, `spt_log_format`, `spt_console_write`,
`spt_console_read_line`, `spt_console_set_title`, `spt_console_clear` — see *The log pipeline*).
The four raid-setup exports, then. The first, `spt_get_raid_adjustments`, takes one scav raid's time
adjustment, JSON in and JSON out (`{applied, chosenReductionPercent, mapSettingsMissingValue, raidChanges}`,
whose `raidChanges` is the real `RaidChanges` record and whose `exitChanges` entries carry
`ExtractChange`'s PascalCase names), and **names no resident-DB epoch** because every config and location
member it reads is projected into the request. The second, `spt_make_adjustments_to_map`, takes
`spt_get_raid_adjustments`' own `raidChanges` back (`exitChanges`
PascalCase again, inbound this time) alongside the map's exit names, wave times and boss spawns, and
answers with **deltas, not a mutated map** — `{escapeTimeLimit, exitUpdates, aborted, abortedExitName,
mapSettingsMissingValue, waveAdjustments?}`. Every `index` in that response addresses the request's own
projection order, which is what lets the C# side apply the deltas to the live `LocationBase` objects it
built the request from. It draws nothing, so it carries no `testSeed`, and it names no epoch either.
The family's raid-start pair, `spt_adjust_bot_hostility_settings` and `spt_adjust_extracts`, are two more of
them, and deltas again: the first answers `{entries}` — **one entry per config role, in the request map's
insertion order**, `{role, matchedIndex?, addAlwaysEnemies, runChancedEnemiesLoop, setAlwaysFriends?,
bearEnemyChance?, usecEnemyChance?, savageEnemyChance?, savagePlayerBehaviour?}`, a null `matchedIndex`
meaning the role matched no location entry — one ordered list, because the C# applier walks it once and so
keeps legacy's warn/apply interleaving. The second answers `{warnUnknownMap, appendExtractIndices}`, indices
into the request's own extract projection. Neither carries the map or location name: both warnings are
re-emitted C#-side, from the live objects the request was projected from. Like their siblings they name no
epoch, and they draw nothing, so neither carries a `testSeed`. The
twenty-five generation/verify/publish/load/digest/profile exports hand back a heap buffer on success, which the
caller releases with `spt_buf_free`; so do `spt_console_read_line` and `spt_log_format`.

- `spt_db_load`'s optional `handbookPriceOverride` member carries `ItemConfig.HandbookPriceOverride` —
  `{"<itemId>":{"parentId":"…","price":N},…}`, in document order. Rust merges it into
  `database/templates/handbook.json` the way `HandbookHelper.HydrateHandbookCache` does (upsert by `Id`,
  missing entries appended at the end) so the epoch-1 handbook equals a published one. **The merge is visible
  to the publish envelope only**: the framed response still hands back the raw disk bytes, so the C# replica
  and the equivalence gate hydrate from an unmerged file. The member is absent when no CLR is alive to supply
  live config values (post-6b pre-load), and `LoadRequest` carries no `deny_unknown_fields` — an old library
  paired with a new caller would silently ignore it, so the ABI lockstep assert, not the parser, is what
  prevents that pairing.
- `run_generator_with` is the shared body of every generation export, `spt_db_publish`, `spt_db_load` and
  the four `spt_profile_*` —
  parse, `catch_unwind`,
  encode — so a new export is a thin wrapper over it, generic in its error type and response encoding.
  `spt_db_load` is its own encoder (the framed byte response) and blocks on the tokio runtime inside its
  generator fn. Two stand apart from the shared body entirely: `spt_verify_database`, because it blocks on
  the runtime around a hand-written wrapper, and `spt_db_resident_digest`, because it takes no request to
  parse — just a null check, a `catch_unwind` and a `write_buffer`.
- Status codes: `STATUS_OK` 0, `STATUS_BAD_ARGS` 1, `STATUS_PANIC` 2, `STATUS_ERROR` 3, `STATUS_STALE_EPOCH` 4
  (every generation export since flip #6 **except the four raid ones**). **Quest, scav case and raid never
  return 2**: they catch the generator's panic themselves and report it as 3 carrying the message. Quest and
  scav case do it because they port a C#-sanctioned throw as a panic — a generation failure, not a library
  bug; raid returns both of its sanctioned throws as a `RaidError` and keeps the `catch_unwind` as a
  backstop. The cost is that a real port bug in those three also arrives as 3, indistinguishable from a
  sanctioned failure. Deliberate. Raid never returns 4 either: it rides no resident DB, so its `FfiFailure`
  impl has a single arm.
- **Every family but raid rides the resident DB (Phase 1 complete at flip #6, ABI 27): ragfair, the repeatable
  quest, the two startup one-shots (base-class cache, linked-item table), the loot pair — location loot and
  reward loot — the scav case, and the bot family's two exports.** `spt_db_publish` (called by C#'s
  `DbPublisher` whenever `DatabaseMutationStamp` has moved) makes six roots resident in `db.rs` — templates,
  traders, globals, locations, hideout and (since Phase 4) configs — and derives the ragfair, quest and bot
  views off the five tables, in that
  order, because each gates on the previous one's output. Every root is optional — an absent one keeps the
  resident copy — and the epoch bumps on full and partial publishes alike; a bad schema or a failed
  derivation aborts before the swap, so the previous resident DB survives, and an ungated view set is left
  `None`, which the families reading it answer as a stale epoch. A rider's request then names an epoch and
  borrows those views instead of carrying them; only what genuinely varies per call still crosses the
  boundary. The resident roots are the only request data held across calls. Two traps: an epoch the store
  does not hold returns `STATUS_STALE_EPOCH`, and the C# caller force-publishes and retries once; an
  ineligible caller (mods loaded without trust, or the kill switch) instead sends the views inline with
  `epoch: 0`, a wire contract that is documented, not runtime-enforced. Which views each family borrows, and
  why loose loot and `staticAmmoDist` deliberately stayed per-call, is in RUST-ROADMAP.md's flip ledgers.
- **The `configs` root is keyed by kind string, and no view derives from it.** All 28 loaded configs arrive
  under their own `Kind` (`"spt-item"`, `"spt-bot"`, …) rather than a type or file name, and families read
  them per call the way scav case reads its recipes. Only the stems some family actually reads are lifted
  into typed structs (`ConfigsRoot` in `db/models.rs`); the rest ride the flatten map, and so does any
  member of a lifted stem nobody reads. **The strictness contract to know before adding a stem:** each is an
  `Option<Lift>`, so an *absent* stem still parses and the reading family fails its per-call resolve loudly
  naming the stem, while a *malformed* one fails the whole publish (`STATUS_BAD_ARGS`) and leaves the
  previous resident DB standing — and keeps failing, because `DbPublisher.PublishLocked` never advances
  `_lastPublishedStamp` on a throw, so every later `EnsureCurrent()` retries and throws from outside
  `ResidentDbDispatch.Send`'s try, and every eligible native call 500s until the config is fixed. A member
  is strict exactly when the C# member is `required`, with three deliberate exceptions: `spt-item`, whose four
  sets stay `#[serde(default)]` so the five families sharing the stem can keep publishing partial ones in
  their fixtures (`ItemConfigLift`'s doc has the trade); `spt-inventory`'s `customMoneyTpls`, where an absent
  member is a valid empty set; and the whole `spt-pmc` stem, which parses as the override wire's soft
  `PmcConfigWire` (its doc has the trade). The soft members' wire names are pinned by the hand-run
  `phase4_configs_root.rs`, since a drifted name on a soft member parses fine and silently reads empty. Which config members deliberately stayed per-call
  — the ones a C# writer mutates in place through an indexer, where no write barrier fires, or that the
  caller itself selects — is in RUST-ROADMAP.md's Phase 4 ledger. Phase 4 added **no** export.
- **A buffer is written on failure too** — the parse error, the `LootError` message, or the panic text.
  Ownership is decided by the out-pointer being non-null, never by the status code. `spt_verify_database`'s
  free-on-success-only shape must not be copied into the generators.
- `catch_unwind` on every fallible path: a Rust panic never unwinds into the CLR.
- **Only the failure message crosses the buffer.** The run's diagnostics are already in the log, emitted as
  they happened through `diag::DiagSink`; the error text itself is the C# caller's to log.

Adding an export means bumping `ABI_VERSION` and `SptNative.ExpectedAbiVersion` together; a test in `ffi.rs`
asserts the constant so the bump can't be forgotten.

### `src/verify.rs`

`checks.dat` is a path/hash manifest written at Release build time by `generate` in this same module, via the
`gen_checks` bin (invoked by `PreBuildHashFile` in `SPTarkov.Server.csproj`). Hashing fans out over the
tokio runtime under a concurrency cap. Three properties are load-bearing:

- **Scope comes from the manifest, not the tree.** Only the top-level `SPT_Data` roots the manifest names
  (`configs/`, `database/`) are walked; the build relocates unhashed artifacts into the output `SPT_Data` and
  `generate` leaves `images/` and `checks.dat` out deliberately, so walking everything would fail.
- **The check runs both directions.** Manifest entries with no walked file are reported as `missing_from_disk`,
  so a deletion (or a symlink the walk skips) can't pass.
- **Empty manifest fails closed** rather than verifying nothing.

### The log pipeline (`src/logger.rs`, `src/log_sink.rs`)

The one ported family with **no legacy path**: C# no longer ships a log handler of its own — `ILogHandler`
and `BaseLogHandler` survive only for mods to implement, and nothing built-in does. `spt_logger_init` parses
the raw `sptLogger.json` bytes once per process; `SPTLoggerDispatcher.Log` then hands each line's fields across
`spt_log_emit`, and the crate owns filter matching, the level gate, per-target format expansion and the sinks.

The pipeline also owns the terminal itself. C# installs a forwarding `TextWriter` over `Console.Out` and
`Console.Error` (`NativeConsoleWriter.Install`, called from `Program.Main` right after logger init), so raw
`Console.Write*` sites forward to `spt_console_write`: stdout bytes travel the same queue as rendered log
lines, stderr bytes write directly. Terminal control is native too — `spt_console_set_title` and
`spt_console_clear` send tty-gated escapes through that queue (the Windows title is a console API call
instead, and the Windows console setup — UTF-8 codepage plus VT — happens in `spt_logger_init`), and
`spt_console_read_line` drains the queue before reading stdin, so a prompt written just before is on screen.
Two exports answer rather than write: `spt_log_enabled` serves the `IsLogEnabled` gate from the applied
configuration, and `spt_log_format` renders `BaseLogHandler.FormatMessage`'s line for mod handlers. Before
init, after close, or with no console target configured the bytes still reach the terminal, written straight
to stdout on the Rust side; only an unloadable library falls the write back through the C# writer's captured
original, which is what the dispatcher's stderr fallbacks-of-last-resort need.

- **Init and close are ref-counted, not idempotent.** A second init keeps the running pipeline and ignores the
  new config but bumps the count; teardown needs as many `spt_logger_close` calls as there were successful
  inits. That is what lets the prepatcher's nested `Program.Main` dispose its own container while the outer
  host keeps logging.
- **Logging never fails the server** — a bad config or a broken library is one stderr notice and every later
  emit is a silent no-op — and **an emit never blocks on I/O**: each file gets a background writer thread fed
  over a bounded channel that drops lines rather than growing without limit. `log_sink.rs` owns rotation and
  the archive cap together, so the cap is enforced at the rotation rather than on a timer.
- **The console queue is one FIFO carrying two disciplines.** Rendered log lines are offered and dropped when
  the 8192-slot channel is full; raw `Console.Write` bytes and the pre-read drain block until there is room,
  because a lost prompt is worse than a lost log line. The ceiling that buys, stated rather than engineered
  away: a terminal that stops draining blocks the managed `Console.Write` behind it, and `spt_logger_close`
  (from `Program.Main`'s `finally`) waits on the writer thread until it does drain.
- **`spt_logger_reinit` swaps the config in place** without touching the init count — C# calls it from
  `SPTLoggerDispatcher.ReloadConfiguration()` after a mod mutates `SptLoggerConfiguration.Loggers`. A
  same-path target reopens in append mode rather than cascading the archives again.
- **Mod-facing `ILogHandler`s are fed from two legs**: the dispatcher fans C#-originated lines out itself, and
  `spt_log_set_tap`'s callback delivers the Rust-originated ones.
- **The generator families feed the pipeline from the inside**, not over `spt_log_emit`. `diag.rs` renders a
  locale-keyed diagnostic against the table C# pushes once over `spt_locales_set` after the database import —
  a startup snapshot, so a missing table or key leaves the key itself as the text — then hands the line
  straight to the logger.

### `src/loot/`

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

Since flip #4 both generators resolve their DB-derived views off the resident DB (see *FFI boundary*); the
family has no `views.rs` of its own, borrowing `ragfair/views.rs`'s instead. Since Phase 4 its config
inputs come off the resident `configs` root too — **except the two loot multipliers**, which a C#
service rescales in place per scav raid and which therefore stay per-call; the service-backed fields
ride each request as before.

### `src/bot/`

`mod.rs` defines `BotContext<'a>` — the read-only views one generation run borrows, plus its `DiagSink`. The
bot family's analog of `LootContext`. It also owns `BotViews`, the two-arm enum every DB-derived read goes
through, and `resolve_bot_views`, shared by both exports — see *FFI boundary*.

| Module | Stands in for | What it does |
|---|---|---|
| `bot_inventory_generator.rs` | `Generators/Bot/BotInventoryGenerator.cs` | `generate_inventory` — the orchestrator and the crate's bot entry point — plus `generate_inventory_batch`, one wave in one call over a rayon loop |
| `level_generator.rs` | `Generators/Bot/BotLevelGenerator.cs` | `generate_bot_level` — the batch path's level/exp draw. Only `GenerateBotLevel` + `ChooseBotLevel`; `GetRelativePmcBotLevelRange` stays C#-side as hoisted wave state |
| `bot_equipment_mod_generator.rs` | `Generators/Bot/BotEquipmentModGenerator.cs` | Both mod halves (equipment, weapon), plus the one `BotWeaponModLimitService` method they call |
| `bot_generator_helper.rs` | `Helpers/Bot/BotGeneratorHelper.cs`, `BotInventoryContainerService.cs` | Per-item `Upd` blocks, compatibility probes, and the `ContainerGrids` occupancy state |
| `bot_loot_generator.rs` | `Generators/Loot/BotLootGenerator.cs` | Fills pockets/vest/backpack/secure from pools the C# caller resolved |
| `bot_weapon_generator.rs` | `Generators/Bot/BotWeaponGenerator.cs` | Pick, kit out, load a weapon; hand over spare magazines |
| `bot_weapon_generator_helper.rs` | `Helpers/Bot/BotWeaponGeneratorHelper.cs` | Magazine and bullet counts, magazine+ammo item pairs |
| `inventory_mag_gen.rs` | `Generators/Weapons/*` | The four `IInventoryMagGen` strategies, collapsed into one enum with a fixed dispatch order |
| `durability_limits_helper.rs` | `Helpers/Bot/DurabilityLimitsHelper.cs` | Weapon/armor durability rolls |
| `mod_pool_service.rs` | `Services/Bot/BotEquipmentModPoolService.cs` | Slot mod pools, derived per call instead of cached, drawn in the template's own database slot order — owned natively since ABI 32 |
| `repair_service.rs` | `Services/Commerce/RepairService.cs` | Only `AddBuff`, the one slice bot generation reaches |
| `exhaustable_array.rs` | `Utils/Collections/ExhaustableArray.cs` | Draw-without-replacement |
| `views.rs` | — | Publish-time derivation of `BotDbViews` from the resident roots in `src/db.rs`, embedding `RagfairDbViews` by `Arc` (see *FFI boundary*) |
| `models.rs` | `Models/…` | Wire types |

### `src/ragfair/`

`mod.rs` defines `RagfairContext<'a>` — the read-only views and config one batch borrows, plus its
`DiagSink`. One native call generates a whole batch of offers, not one offer.

| Module | Stands in for | What it does |
|---|---|---|
| `offer_generator.rs` | `Generators/Ragfair/RagfairOfferGenerator.cs` | `generate_dynamic_offers` — the batch pass, the barter schemes, the offer object, condition randomisation and armor-plate removal |
| `assort_generator.rs` | `Generators/Ragfair/RagfairAssortGenerator.cs` | The assort walk: every flea-sold preset, then every sellable template, as (root + children) lists. Draws nothing |
| `price_service.rs` | `Services/Ragfair/RagfairPriceService.cs` | The pricing math one offer needs — flea/handbook/trader arms, preset rollups, the one biased price draw |
| `server_helper.rs` | `Helpers/Ragfair/RagfairServerHelper.cs` | Stack counts, offer counts, offer currency, item validity |
| `views.rs` | — | Publish-time derivation of the ragfair views from the resident roots in `src/db.rs` (see *FFI boundary*) |
| `models.rs` | `Models/Spt/Config/RagfairConfig.cs`, `Models/…` | Config records and the request/response envelopes |

Two crate-internal facts:

- **The walk is parallel only when unseeded.** C# fans one `Task.Factory.StartNew` per assort entry; an
  unseeded batch here fans across rayon the same way (a forked context per entry, merged back in assort order
  with `intId` reassigned). A **seeded** batch stays sequential — the seeded RNG is thread-local, so fanning
  out would drop every worker onto entropy, and parity rides the sequential path byte-for-byte.
- **`GetFleaPricesAsArray`'s cache is re-derived per call.** The C# `AllowedFleaPriceItemsForBarter` field is
  built once per generator instance and never invalidated; here it is rebuilt on every call, which makes the
  native path *fresher* than legacy for runtime-added items.

### `src/quest/`

`mod.rs` defines `QuestContext<'a>` — the read-only views one quest borrows, plus its `DiagSink`. Every
DB-derived view comes off the resident DB and, since Phase 4, so does every config-backed one (see *FFI
boundary*); the service-backed fields and the caller's chosen `repeatableConfig` ride
the request. One native call generates **one** quest of one type.

`generate_repeatable_quest` stands in for the type switch of
`RepeatableQuestController.PickAndGenerateRandomRepeatableQuest`: it resolves the views, installs the seed
guard, and dispatches on the requested type. Two things to know before touching it — the mutated quest-type
pool rides back alongside the quest whether or not one was generated, and a `None` quest is a normal outcome
(exhausted pool, or a generator that gave up and logged why), not a failure.

| Module | Stands in for | What it does |
|---|---|---|
| `elimination.rs` | `Generators/RepeatableQuests/EliminationQuestGenerator.cs` | The kill-N-of-X quest |
| `completion.rs` | `Generators/RepeatableQuests/CompletionQuestGenerator.cs` | The hand-over-N-of-X quest |
| `exploration.rs` | `Generators/RepeatableQuests/ExplorationQuestGenerator.cs` | The survive-N-raids quest |
| `pickup.rs` | `Generators/RepeatableQuests/PickupQuestGenerator.cs` | The fetch-N-items-of-a-type quest. Reachable, but no shipped `quest.json` lists `Pickup` in its `types` |
| `reward_generator.rs` | `Generators/RepeatableQuests/RepeatableQuestRewardGenerator.cs` | The reward chain every type ends with: XP, money, GP coins, an optional weapon preset, items, trader standing, an optional skill point |
| `helper.rs` | `Helpers/Quest/RepeatableQuestHelper.cs` | The template clone/placeholder pass each generator opens with, and the level-band config lookups |
| `views.rs` | — | Publish-time derivation of the quest views from the resident roots in `src/db.rs`, sharing the items/price views with `ragfair/views.rs` through one `Arc` (see *FFI boundary*) |
| `models.rs` | `Models/Spt/Repeatable/…`, `Models/…` | Wire types |

### `src/scav_case/`

Since flip #5 this family rides the resident DB like the loot pair, `mod.rs` resolving the views and
deriving its recipe views from the hideout root per request. It is the one family with **no context
type** — a craft reads few enough members that the borrowed struct the others use would earn nothing —
and the one whose `mod.rs` carries a `//!` header, because it holds the entry point. Read that header
first: it states the family's citation convention, and the bare `` `:N` `` line numbers do **not** point
where they look like they do in today's C# file. One native call generates **one** craft's rewards.

| Module | Stands in for | What it does |
|---|---|---|
| `mod.rs` | `Generators/ScavCaseRewardGenerator.cs` (entry) | `generate_scav_case_rewards` — resolves the views, installs the seed guard, and catches the generator's panics so a C#-sanctioned throw comes back as an error rather than `STATUS_PANIC` (see *Conventions*) |
| `generator.rs` | `Generators/ScavCaseRewardGenerator.cs` | The craft: the reward pool (rebuilt per request, not cached on an instance), the per-rarity counts and price bands, the picks, and the money/ammo/preset arms |
| `models.rs` | — | Request/response envelopes only; the DB/EFT types they carry are `loot::models`' |

### `src/raid/`

The one family that rides **no** resident DB: every config and location member it reads is projected into the
request, so a call names no epoch. Like `scav_case`, its `mod.rs` holds the entry point and carries a `//!`
header — read it first, because it states the family's citation convention: a bare `` `:N` `` is a line of
`Services/InRaid/RaidTimeAdjustmentService.cs`, and an `LLS:N` (`raid_start.rs` only) one of
`Services/InRaid/LocationLifecycleService.cs`. One native call adjusts **one** raid.

| Module | Stands in for | What it does |
|---|---|---|
| `mod.rs` | `Services/InRaid/RaidTimeAdjustmentService.cs`, `Services/InRaid/LocationLifecycleService.cs` (entries) | `get_raid_adjustments`, `make_adjustments_to_map`, `adjust_bot_hostility_settings` and `adjust_extracts` — all four `catch_unwind` the pass, so a panic arrives as an error message rather than `STATUS_PANIC` (see *Conventions*); only the first installs the seed guard, because only the first draws |
| `adjustments.rs` | `Services/InRaid/RaidTimeAdjustmentService.cs` (`:35-193`, `:201-374`) | `GetRaidAdjustments` with `GetMapSettings` and `GetExitAdjustments` — the chance roll, the weighted reduction percent, the loot-percent floors, and the per-train-exit disable-or-reduce walk — plus `MakeAdjustmentsToMap` with `AdjustWaves` and `AdjustPMCSpawns`: the exit-update walk and its unmatched-name abort, the twice-applied wave reduction, and the pmc keep-and-offset passes. `AdjustLootMultipliers` is *not* here: it rewrites the live `LocationConfig` in place and stays C# on both arms, the family's one decline-set carve-out |
| `raid_start.rs` | `Services/InRaid/LocationLifecycleService.cs` (`LLS:251-275`, `LLS:281-363`) | `AdjustExtracts` and `AdjustBotHostilitySettings` — the scav-side/unknown-map gates and the ignore-case scav-extract filter, and the per-config-role location match with the ops each match earns. The two loops that write through live references stay C#-side: the chanced-enemy probe-as-you-fill and the `Exits.Union` |
| `models.rs` | `Models/Spt/Location/RaidChanges.cs` (both time exports' inner half) | Wire types — a fresh contract, mirrored member-for-member C#-side |

### Conventions

These are what keep the port correct; break one and output silently diverges from C#.

- **Every ported module names its C# source in its `//!` header**, with a line range where the port is a slice
  of a larger file. Read that header before changing anything. (The modules with no C# counterpart — the
  infrastructure, `db.rs`, and every family `mod.rs` but `scav_case`'s and `raid`'s — say so or carry no
  header at all.)
- **Deviations are marked `Deviation:`, at the scope they apply to** — module header, item doc, or the line
  itself. Grep the bare form; the bolding is inconsistent between families.
- **RNG draw order is a contract.** The bot family states it up front: its generator modules open with an
  ordered list of their draws in C# source order, including the ones C# consumes and discards. The loot
  family, the two-function `level_generator.rs`, and the draw primitives themselves document each draw at
  its call site against the C# line instead. Adding, removing or reordering a draw desynchronises the whole
  sequence, so a "harmless" early-out that skips a roll is a bug.
- **The generator families log for themselves, and have one rule for throws.** C# `ISptLogger` calls become
  `Diagnostic` values pushed onto the run's `diag::DiagSink`, rendered and emitted as they happen under the
  module's `CATEGORY` — the `typeof(T).FullName` of the C# class it stands in for. (Tests swap in the sink's
  `Capture` variant and assert diagnostics as data.) A C# *return-null-and-log* path is ported as a
  `Diagnostic` plus `None`, in every family. A C#-*sanctioned* `throw` is ported one of two ways: loot, bot and
  ragfair return it as a `LootError` (so does an unguarded null deref they would have NRE'd on); quest panics
  at the throw site and catches it at the family entry point. Scav case does both — a `ScavCaseError` where the
  C# throw is reachable through a guard, a caught panic where the C# throws out of a dictionary index.
  Panicking is safe here: every export runs inside `catch_unwind`, so nothing unwinds past the boundary.
- **Wire models live in a `models.rs` per generator directory**, plus `db/models.rs` for the publish roots.
  (`base_class.rs` and `linked_items.rs` declare their one request/response pair inline instead.) The rule
  when adding one: a model mirroring a C# record must carry a `#[serde(flatten)]` catch-all, or mod-added
  fields silently vanish on the round trip — it is the counterpart to the `[JsonExtensionData]` that
  `Tools/Ceciler` injects C#-side. Request/response envelopes are a fresh contract and need no such map.
- **One C# RNG lifetime can span two native calls.** Each generator entry point (not `ffi.rs`) opens the run by
  mapping the request's optional seed through `TestSeedGuard::install`. `generate_dynamic_loot` is the
  exception: it uses `TestSeedGuard::resume`, picking up the thread-local stream the preceding
  `generate_static_containers` parked under the same seed, because C# installs one `SeededRandomSource` for the
  whole of `GenerateLocationLoot` and draws in both phases. The guard is RAII, so a panic can't leak a seeded
  stream onto a pooled thread.
- **Caches become per-call derivation.** C# DI singletons keyed by bot id or built over the whole database
  (`BotEquipmentModPoolService`, `BotInventoryContainerService`) are recomputed per call or handed across the
  boundary by the caller. The unit is one bot, not one raid: the batch export resolves the database views
  once per call, hoists the config every bot in the wave shares, and still derives the rest per bot, each
  with its own seed guard.
- **The batch export is one wave per call, and the shared half is keyed by level band.** Everything the
  wave's bots have in common — templates and loot pools — rides once, one entry per level band rather than
  per bot; each bot's rayon task draws its own level *first* (matching where the C# prelude does it), then
  picks the band covering it. Non-PMC waves draw nothing there, so their seeded streams are unchanged. The
  single-bot export shares the same envelope and shared block but is untouched by all this: it keeps C#
  level generation and C# filtering, sending its pre-filtered template and loot pools at the top level. A
  failed bot carries its message in its own reply slot rather than failing the export — the wave survives
  it, the way `BotController.TryGenerateSingleBot` skips a bot with one Critical log — so the batch export
  never returns `STATUS_ERROR`, only bad-args, a stale epoch or a panic.

### Tests

Almost all tests are inline `#[cfg(test)]` modules (~770 of them). Three kinds:

- **Parity fixtures** — replay a C# scenario and assert the exact item list.
- **Seeded-RNG tests** — an optional seed on every request envelope (`testSeed`, spelled `seed` on the quest
  one) installs a `TestSeedGuard`, swapping thread entropy for a xoshiro256\*\* stream bit-identical to
  `Utils/RandomSource.cs`. Known-answer tests in `random_util.rs` pin it; `RandomSourceParityTests.cs` pins the
  C# end.
- **FFI transport tests** — `ffi.rs` round-trips the exports through raw pointers, covering success, parse
  failure, generation failure and null arguments. `spt_generate_bot_inventory_batch` is the one export with no
  transport test of its own. The four `spt_profile_*` are covered here too — junk envelope, wrong schema,
  traversal id, null args.

`profile.rs`'s own tests each work in a `tempfile::TempDir` and touch no resident state, so unlike the
resident-DB tests they need no `DB_TEST_LOCK` and run fully parallel.

Ten `tests/` targets, in three groups. `completion_whitelist_baseclass.rs` and `phase3_db_load.rs` run
against the real shipped tree, so both need `scripts/decompress-assets.sh` to have run: the first guards the
Completion whitelist filter's base-class lookups against `items.json`, the second runs the fused load over
`SPT_Data` and asserts the installed epoch, the five resident roots and their derived views, which files do
and do not ride the handoff, and that a returned buffer is byte-exact against disk — it is also the gate on
the resident roots being comment-free JSON.
`phase1_ragfair_views.rs`, `phase1_quest_views.rs`,
`flip3_oneshot_views.rs`, `phase0_publish_spike.rs` and `phase4_configs_root.rs` are `#[ignore]`d halves of
C#-paired harnesses —
each replays a fixture its twin under `Testing/UnitTests` wrote to `$TMPDIR`, so running one alone proves
nothing. The Phase 4 one is the only crossing of `ConfigLoader`'s bespoke serializer options and the shared
`JsonUtil` ones the publish writes with, so it is the gate on that pair, not on any generator.
`flip4_loot_resident.rs`, `flip5_scavcase_resident.rs` and `flip6_bots_resident.rs` are
self-contained: each
publishes a minimal DB and proves a resident-epoch send generates identically to the same data sent inline.

Run with `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`. That is
workspace-wide, so it also picks up the other two members' handful of inline tests — `mpex-server`'s three
over the server-assembly probe, `spectre-facade`'s two.

---

## 4. Integration Points

| External System | Integration Type | Notes |
|-------------------|-------------------|-------|
| `SPTarkov.Server.Core` | Sync FFI, C ABI | `Native/NativeMethods.cs` + `SptNative.cs` and the per-family projections. Thirty-nine exports; `ABI_VERSION` must equal `SptNative.ExpectedAbiVersion` |
| `SPTarkov.Common` | Sync FFI, C ABI | The eleven log and console exports plus `spt_buf_free`, from a second `Native/NativeMethods.cs` — Common cannot reference Core |
| `SPT_Data/` on disk | Batch, async over tokio | `spt_verify_database` hashes `configs/` + `database/` with XXH3-128; `spt_db_load` (since ABI 29) does that walk *and* reads `database/` in one pass, installing the five database roots — never the configs root, which only `spt_db_publish` builds — and handing the eager bytes back to `DatabaseImporter`; `gen_checks` writes `checks.dat` on Release builds |
| `user/profiles/` on disk | Blocking read/write per call | `spt_profile_*` (since Phase 5) own every live listing, read, write and delete; the directory arrives in each request. `BackupService` (C#) still copies and restores beside them |
| `sptLogger.json` | Config bytes over FFI | Parsed once per process by `spt_logger_init`; filters use `regex-lite`, so .NET-only syntax degrades to never-match |
| Log files on disk | Async, background thread per sink | `log_sink.rs`: bounded channel that drops rather than grows; rotation and archive cap enforced together |
| .NET CLR (`mpex-server`) | Process host | netcorehost `run_app`s the published server assembly with argv forwarded; libnethost comes from NuGet at build time via the `nethost-download` feature |
| MSBuild | Build-time shell-out | `BuildSptNative` (Core) and `BuildSpectreFacade` (Common) invoke `cargo`, so **`cargo` on `PATH` is a hard build dependency** |

---

# Relationship to Other Framework Components

| Component | Responsibility |
|-----------|-----------------|
| [root `ARCHITECTURE.md`](../ARCHITECTURE.md) | The boundary as seen from C#, under *Native Rust layer* |
| [`Libraries/SPTarkov.Server.Core/ARCHITECTURE.md`](../Libraries/SPTarkov.Server.Core/ARCHITECTURE.md) | `Native/`, the thirteen dual-path classes, the `ForceLegacy*` config flags |
| [`Libraries/ARCHITECTURE.md`](../Libraries/ARCHITECTURE.md) | `SPTarkov.Common`'s logging front end and the Spectre facade reference chain |
| [`CLAUDE.md`](../CLAUDE.md) | Build coupling, cross-RID rules, the ABI-bump requirement |
| [`RUST-ROADMAP.md`](../RUST-ROADMAP.md) | Port status, exceptions in force, known divergences, the flip ledgers cited above |
| [`BENCHMARK.md`](../BENCHMARK.md) | Native vs legacy timings — every measurement lives there |
