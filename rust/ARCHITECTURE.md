# Architecture — `rust/`

A Cargo workspace with one crate, `spt-native`, built as a `cdylib` (plus `rlib`, so the tests can link it).
It is a **port of C# server logic**, not a new subsystem: every module stands in for a named
`SPTarkov.Server.Core` file and is expected to produce byte-identical output. The C# side of the boundary is
`Libraries/SPTarkov.Server.Core/Native/` (`NativeMethods.cs`, `SptNative.cs`).

Build coupling and cross-RID rules live in [CLAUDE.md](../CLAUDE.md); the boundary as seen from C# is in
[ARCHITECTURE.md](../ARCHITECTURE.md) under *Native Rust layer*. This file covers what's inside the crate.

Toolchain is pinned in `rust-toolchain.toml` (1.97.1, edition 2024). Dependencies are:
`serde`/`serde_json` (with `preserve_order`, so untyped maps keep C# `Dictionary` insertion order),
`rmp-serde` (the ragfair MessagePack envelope), `indexmap`, `rand`/`rand_xoshiro`, `rayon`, `tokio`,
`walkdir`, `xxhash-rust`, `base64` — plus `tempfile` as the only dev-dependency (the `verify` FFI
tests need a real directory). `Cargo.lock` is committed.

Roughly 35k lines across 32 files, tests included. `src/bot/` is ~45% of that and
`bot_equipment_mod_generator.rs` alone ~4.2k; `src/loot/` ~31%, `src/ragfair/` ~20%.

## Layout

| Path | Role |
|---|---|
| `src/lib.rs` | Module roots and `ABI_VERSION` (currently 12; must equal `SptNative.ExpectedAbiVersion`) |
| `src/ffi.rs` | The C-ABI surface. The **only** module containing `unsafe` |
| `src/runtime.rs` | Process-wide multi-thread tokio runtime, `OnceLock`-built. Used only by `verify` |
| `src/verify.rs` | Hashes `SPT_Data` with XXH3-128 and diffs it against `checks.dat` |
| `src/loot/` | Location loot (static containers, loose loot) and reward loot (airdrops, cases, containers) |
| `src/bot/` | One bot's entire inventory: equipment, mods, weapons, magazines, loot |
| `src/ragfair/` | One batch of dynamic flea offers: the assort walk, pricing, barter schemes, the offers |

## FFI boundary (`ffi.rs`)

Twelve `extern "C"` exports. Two are trivial (`spt_native_abi_version`, `spt_buf_free`); the other ten take a
UTF-8 JSON request and hand back a heap buffer the caller releases with `spt_buf_free`.

```
C# SptNative → spt_generate_* (JSON in)
  → serde deserialize into a request envelope from loot/, bot/ or ragfair/models.rs
  → catch_unwind( generator )
  → serde serialize the result, or the LootError message, into an out-buffer
```

- `run_generator` is the shared body of the nine generation exports. `spt_verify_database` is separate
  because it blocks on the tokio runtime.
- Status codes: `STATUS_OK` 0, `STATUS_BAD_ARGS` 1 (null pointer, bad UTF-8, unparseable JSON),
  `STATUS_PANIC` 2, `STATUS_ERROR` 3.
- **A buffer is written on failure too** — the parse error or the `LootError` message. Ownership is decided by
  the out-pointer being non-null, never by the status code. `spt_verify_database`'s free-on-success-only shape
  must not be copied into the generators.
- `catch_unwind` on every fallible path: a Rust panic never unwinds into the CLR.
- **Diagnostics do not survive a failure.** On the `LootError` path `run_generator` writes only
  `error.message`; everything the run had accumulated in `LootContext`/`BotContext`/`RagfairContext`
  is dropped, so a failed call tells the C# caller nothing about what it logged on the way down.

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
| `models.rs` | `Models/…` | Wire types (see *Conventions*) |

## `src/bot/`

`mod.rs` defines `BotContext<'a>` — the read-only views (items, presets, blacklists, durability config,
equipment filters…) one generation run borrows, plus the `Diagnostic` list it accumulates. It is the bot
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
resolved trader prices and the `dynamic` config block one batch borrows, plus its `Diagnostic` list.
One native call generates a whole batch of offers, not one offer.

| Module | Stands in for | What it does |
|---|---|---|
| `offer_generator.rs` | `Generators/Ragfair/RagfairOfferGenerator.cs` | `generate_dynamic_offers` — the batch pass, the barter schemes, the offer object, condition randomisation and armor-plate removal |
| `assort_generator.rs` | `Generators/Ragfair/RagfairAssortGenerator.cs` | The assort walk: every flea-sold preset, then every sellable template, as (root + children) lists. Draws nothing |
| `price_service.rs` | `Services/Ragfair/RagfairPriceService.cs` | The pricing math one offer needs — flea/handbook/trader arms, preset rollups, the one biased price draw |
| `server_helper.rs` | `Helpers/Ragfair/RagfairServerHelper.cs` | Stack counts, offer counts, offer currency, item validity |
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

## Conventions

These are what keep the port correct; break one and output silently diverges from C#.

- **Every ported module names its C# source in its `//!` header**, with the line range where the port is a
  slice of a larger file rather than the whole of it. (`lib.rs`, `ffi.rs`, `runtime.rs`, `verify.rs` and the two
  `mod.rs` files have no C# counterpart and no header.) Read that header before changing anything.
- **Deviations are marked `**Deviation:**`, at the scope they apply to** — module header, item doc, or a comment
  on the line itself. Only `bot_inventory_generator.rs` and `bot_weapon_generator.rs` also collect theirs under a
  module-level `# Deviations` heading; everywhere else, grep for the marker rather than expecting a section.
- **RNG draw order is a contract.** The bot family states it up front: every module that draws opens with an
  ordered list ("*RNG calls, in C# source order — the parity contract*"), including draws C# consumes and
  discards. The loot family documents each draw inline at its call site instead, against the C# line. Adding,
  removing, or reordering a draw desynchronises the whole sequence, so a "harmless" early-out that skips a roll
  is a bug.
- **No logging, no throwing.** C# `ISptLogger` calls become `Diagnostic` values the caller replays through its
  own logger; C# exceptions (and unguarded null derefs) become `LootError`, returned rather than panicked.
- **Wire models come in three families** (`loot/models.rs`, `bot/models.rs`, `ragfair/models.rs`). DB/EFT
  models mirror C# records field-for-field, pinned to the exact `JsonPropertyName`, each with a
  `#[serde(flatten)] extra` map so
  mod-added fields survive the round trip — the counterpart to the `[JsonExtensionData]` that `Tools/Ceciler`
  injects. Request/response envelopes are a fresh contract and use plain camelCase with no passthrough map.
- **One C# RNG lifetime can span two native calls.** Each generator entry point (not `ffi.rs`) opens the run
  with `test_seed.map(TestSeedGuard::install)`. `generate_dynamic_loot` is the exception: it uses
  `TestSeedGuard::resume`, which picks up the thread-local stream the preceding `generate_static_containers`
  parked under the same seed. C# installs one `SeededRandomSource` for the whole of `GenerateLocationLoot` and
  draws in both phases; the native side is entered once per phase, so a fresh `install` on the second would
  replay the first phase's values. The guard is RAII, so a panic can't leak a seeded stream onto a pooled thread.
- **Caches become per-call derivation.** C# DI singletons keyed by bot id or built once over the whole database
  (`BotEquipmentModPoolService`, `BotInventoryContainerService`) are recomputed per call or handed across the
  boundary by the caller, since one native call generates one bot.

## Tests

All tests are inline `#[cfg(test)]` modules (~565 of them); there is no `tests/` integration suite. Three kinds:

- **Parity fixtures** — replay a C# scenario and assert the exact item list.
- **Seeded-RNG tests** — a `testSeed` field on every request envelope installs a `TestSeedGuard`, swapping
  thread entropy for a xoshiro256\*\* stream that is bit-identical to `Utils/RandomSource.cs`. Known-answer
  tests in `random_util.rs` pin it, and `RandomSourceParityTests.cs` pins the C# end.
- **FFI transport tests** — `ffi.rs` round-trips each export through raw pointers, covering success, parse
  failure, generation failure, and null arguments.

Run with `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`.
