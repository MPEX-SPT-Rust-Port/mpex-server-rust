# Rust port — status and roadmap

What is ported to `rust/spt-native`, what is known-broken, and what comes next. Crate internals are
[rust/ARCHITECTURE.md](rust/ARCHITECTURE.md); the C# side of the boundary is
[ARCHITECTURE.md](ARCHITECTURE.md) § *Native Rust layer*. Every measurement lives in
[BENCHMARK.md](BENCHMARK.md); no timings are repeated here.

## Status

Ported and native by default: the loot family, the bot family, dynamic ragfair offer generation, the
repeatable-quest family, scav case rewards, the item base-class cache build, the ragfair
linked-item table and map/raid setup. Each keeps its full 4.1.2 C# implementation as a **legacy
path**, selected
automatically when a mod hooks it or manually via a config flag. The log pipeline is ported too and
has no legacy path: `SPTLoggerDispatcher` hands every line to the crate, and the crate owns the
terminal outright — raw `Console.Write*`, prompts, title and clear all cross the boundary.

Thirty-nine C-ABI exports (`src/ffi.rs`) carry all of it, JSON in and JSON out — except the ragfair
response (a framed MessagePack envelope), `spt_db_load` and `spt_profile_load` (a JSON header frame
followed by the loaded file bytes), and the log and console exports (the fields of one line, or raw
bytes, directly). Current ABI 34.

Since Phase 6b those exports are reached two ways, one per process. A shipped Linux build resolves
them out of the `mpex-server` executable, which links the crate as an rlib; dev builds, the test run
and Windows resolve them from the `spt_native` cdylib. The call shape is identical either way —
`[LibraryImport]` against the name `spt_native`, with the resolver choosing the source.

Native is not uniformly faster. Loot and repeatable quests win; bots, reward loot, ragfair, scav
case, the base-class hydrate and the linked-item table are slower than the C# they replace, and
native stays their default anyway — each case is argued where it is measured, in
[BENCHMARK.md](BENCHMARK.md), and each has a force-legacy flag. Ragfair is the one family that set
itself a parity gate and **missed** it; the resident-DB flip narrowed the gap without closing it,
and every lever short of the remaining state-ownership phases is spent.

## Working

| Feature | Entry point | Native export |
|---|---|---|
| `SPT_Data` hash verification (XXH3-128, parallel) | `DatabaseImporter` → `SptNative` | `spt_verify_database` |
| Database load (SPT_Data → resident roots + C# replica buffers) | `DatabaseImporter.LoadDatabaseAsync` | `spt_db_load` |
| Static container loot | `LocationLootGenerator.GenerateStaticContainers` | `spt_generate_static_containers` |
| Loose loot spawn points | `LocationLootGenerator.GenerateDynamicLoot` | `spt_generate_dynamic_loot` |
| Airdrop loot | `LootGenerator.CreateRandomLoot` / `CreateForcedLoot` | `spt_create_random_loot`, `spt_create_forced_loot` |
| Sealed weapon cases | `LootGenerator.GetSealedWeaponCaseLoot` | `spt_get_sealed_weapon_case_loot` |
| Reward containers | `LootGenerator.GetRandomLootContainerLoot` | `spt_get_random_loot_container_loot` |
| Whole bot inventory (equipment, mods, weapons, loot) | `BotInventoryGenerator.GenerateInventory` | `spt_generate_bot_inventory` |
| A whole bot wave in one call | `BotWaveBatcher.TryGenerateWave`, from `BotController.GenerateBotWave` | `spt_generate_bot_inventory_batch` |
| A batch of dynamic flea offers (assort walk, pricing, barter schemes) | `RagfairOfferGenerator.GenerateDynamicOffers` | `spt_generate_dynamic_offers` |
| Repeatable quests (all four types + rewards) | `*QuestGenerator.Generate` | `spt_generate_repeatable_quest` |
| Scav case rewards | `ScavCaseRewardGenerator.Generate` | `spt_generate_scav_case_rewards` |
| Item base-class cache hydrate | `ItemBaseClassService.HydrateItemBaseClassCache` | `spt_build_item_base_class_cache` |
| Ragfair linked-item table | `RagfairLinkedItemService.BuildLinkedItemTable` | `spt_build_ragfair_linked_item_table` |
| Resident DB publish — the templates, traders, globals, locations and hideout roots, the configs root (all 28, keyed by `Kind`), plus the ragfair, quest and bot views derived from the tables | `DbPublisher.EnsureCurrent` / `ForcePublish` | `spt_db_publish` |
| The whole log pipeline — filters, level gates, per-target formatting, console + file sinks | `SPTLoggerDispatcher.Log` | `spt_logger_init`, `spt_logger_reinit`, `spt_log_emit`, `spt_logger_close`, `spt_log_set_tap` |
| The terminal — raw `Console.Write*` (redirected into the pipeline), prompts, title, clear | `NativeConsoleWriter.Install`, `SptConsole` | `spt_console_write`, `spt_console_read_line`, `spt_console_set_title`, `spt_console_clear` |
| The `IsLogEnabled` gate and the line a mod `ILogHandler` renders | `SPTLoggerDispatcher.IsLogEnabled`, `BaseLogHandler.FormatMessage` | `spt_log_enabled`, `spt_log_format` |
| Generator diagnostics, localised and logged natively as they happen | `DatabaseImporter` → `SptNative.SetServerLocales` | `spt_locales_set` |
| Profile persistence — every live listing, read, write (temp-then-rename) and delete under `user/profiles/`; serialization, the MD5 dirty-check and `BackupService` stay C# | `SaveServer.LoadAsync` / `LoadProfileAsync` / `SaveProfileAsync` / `RemoveProfile` | `spt_profile_list`, `spt_profile_load`, `spt_profile_save`, `spt_profile_delete` |
| One scav raid's time adjustment — the weighted reduction draw, the loot percents and the train-exit changes | `RaidTimeAdjustmentService.GetRaidAdjustments` | `spt_get_raid_adjustments` |
| Map setup for a shortened raid — exit retiming, wave drop/retime, PMC spawn pruning and offset; the loot multipliers are the carve-out and stay C# | `RaidTimeAdjustmentService.MakeAdjustmentsToMap` | `spt_make_adjustments_to_map` |
| Raid start — per-role bot hostility merges and the scav extract append | `LocationLifecycleService.AdjustBotHostilitySettings` / `AdjustExtracts` | `spt_adjust_bot_hostility_settings`, `spt_adjust_extracts` |

Also working: mod-added fields on game data survive the round trip (`#[serde(flatten)] extra` maps
mirroring Ceciler's `[JsonExtensionData]`); native generator diagnostics render and log themselves
through the native pipeline; seeded-RNG parity at the primitive level (xoshiro256\*\*, twin
known-answer tests both sides).

**`rust/` is a two-crate workspace, and the second crate is not a port.** `rust/spectre-facade` is a
build-time generator: it emits a facade `Spectre.Console.Ansi.dll` (via `dotnetdll`) carrying just
`Spectre.Console.Color`, because SPT dropped the real dependency while the frozen 4.1.2 mod surface
still bakes that type into `ISptLogger<T>`, `SptLogMessage`, `ClientLogRequest` and `Watermark.Draw`
— a compiled mod's typeref names the *defining* assembly, and in Spectre.Console 0.57.2 `Color` lives
in `Spectre.Console.Ansi`. Two consequences: **`SPTarkov.Common` needs the Rust toolchain too**, not
just `Core` (its `BuildSpectreFacade` target shells out to `cargo`), and the `<Reference>` is not
transitive, so each of the five projects naming `Color` carries its own. The colours are inert;
cosmetic gaps are `FromInt32` returning Default instead of the xterm palette entry and the inherited
`ValueType.ToString()`. Scope is `Color` only — mods calling `AnsiConsole`, `Markup` or `Style` still
break.

## Broken / known divergences

### Behaviour

- **Patches on collaborators do not reach the native path** and do not flip to legacy — only the
  ported classes' own members are detected. Affected: `RandomUtil`, `ItemHelper`,
  `CounterTrackerHelper`, `BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper`, `BotLootCacheService`, `WeightedRandomHelper`,
  `ItemFilterService`/`PresetHelper` predicates, `ICloner`, plus ragfair's `HandbookHelper`,
  `PaymentHelper`, `BotHelper`, `TraderHelper`, `SeasonalEventService`, quests' `MathUtil` and scav
  case's `RagfairPriceService.GetStaticPriceForItem` and `HideoutTable` reads. Two exceptions:
  `SeasonalEventService.ChristmasEventEnabled` and `RemoveChristmasItemsFromBotInventory` are
  detected **member-scoped** by the bot wave batcher only (it re-times them per level band, so a
  patch there de-batches the wave to the per-bot path where the strip runs in C#); and
  `BotEquipmentModPoolService` is detected **whole-type** by `BotInventoryGenerator` since ABI 32,
  Rust owning the pools outright, with its two `protected` pool-property getters (`GearModPool`,
  `WeaponModPool`) re-admitted explicitly because `IsSpecialName` filters them out of the
  `_hookableMembers` sweep. Invisible in principle: a constructor patch (`GetMethods` never returns
  constructors — the same exclusion as the four generator types) and plain runtime calls into the
  public surface (`ResetWeaponPool()`, or mutating the live collections `GetModsFor*Slot` returns).
  `forceLegacy` is the standing escape hatch.
- **The native and legacy bot paths draw mod slots in different orders at randomised levels.** Since
  ABI 32 the native pool enumerates the template's own `Properties.Slots`; legacy enumerates
  `BotEquipmentModPoolService`'s `ConcurrentDictionary`, sized from `Environment.ProcessorCount`
  (measured moving at 13 real slot names between 8 and 16 cores). The two arms produce different —
  not wrong — bots for one seed, and only the native order is machine-independent. **No cross-arm
  case at any level covers this, and none ever could again**: a different draw order means different
  RNG consumption, so no order-insensitive comparison would pass. The level-1 matrix is untouched
  (`TheSameSeedGeneratesEquivalentInventoryOnBothPaths`, 4 roles × 2 seeds) but reaches the module
  only through `get_required_mods_for_weapon_slot`, which reads the template directly and never
  `derive_pool` — the three pool-building call sites (`bot_inventory_generator:970`,
  `bot_equipment_mod_generator:1020` and `:1574`) each sit behind a randomisation gate level 1 never
  trips. **The exact-output coverage the randomised-level matrix carried is gone on both arms, not
  moved.** Its replacement, `BotParityTests.TheNativePathGeneratesAtRandomisedLevels`, is a smoke
  case over the same 44 cases (2 roles × 22 seeds): generation completes, native ran rather than
  falling back, inventory is non-trivial — nothing about *which* items came out. The nighttime
  clamp's effect on the inventory is therefore uncovered on both arms, though
  `TheNighttimeRandomisationClampIsReplayedOnBothPaths` still pins the clamp write. The native draw
  is covered on its own by the Rust-side golden below. A patch on the pool service declines to
  legacy, where the machine-dependent order still applies.
- **Native bot output is not reproducible across processes**, so no *C#-side* golden can pin it —
  pre-existing and wider than the mod pool. `MongoId.GetHashCode` (`Models/Common/MongoId.cs:325`) is
  `HashCode.Combine`, which .NET seeds per process, so every `Dictionary<MongoId, …>` the bot
  projection serialises enumerates in a process-random order and the seeded native draw walks it. Two
  back-to-back runs of one isolated fixture produced inventories differing in item **count** (69 →
  68), so no normaliser absorbs it. **A Rust-side golden does hold:** `flip6_bots_resident.rs` drives
  both bot exports through the FFI in its own process off a synthetic DB, and `src/bot/` has no
  equivalent hazard (no `HashMap`; its four `HashSet`s are membership-tested, never iterated;
  everything the draw walks is an `IndexMap`/`IndexSet`). `RESIDENT_BATCH_GOLDEN` pins the exact bytes
  of a three-bot batch at fixed seeds — both PMC level bands and the preset fallback — and held
  across five processes and both build profiles. It also reaches `derive_pool` through all three
  gated routes (`get_compatible_mods_for_weapon_slot`, `get_mods_for_weapon_slot`,
  `get_mods_for_gear_slot`, each confirmed by a panic probe), and pins ordering: a randomised mount's
  two derived sub-slots make the key order observable, a two-candidate `mod_foregrip` makes the inner
  set order observable through the seeded pick. The C#-side fix — sorting the projection's
  `MongoId`-keyed dictionaries before serialising — would work and is deliberately not taken: it
  changes the draw order on **every** native path and so alters generated bots server-wide, which is
  a live-wire behaviour change owing its own spec and parity gate, not a test repair. Deferring is
  safe because bots are random by design and no consumer asks two processes to agree.
- **Templates without `_props` read as "not in the db"** on the native *generator* paths — they are
  dropped from `itemsView`. Only bites mod-added props-less templates. The base-class hydrate
  projects the whole table and is unaffected.
- **The native ragfair and scav case paths are fresher than legacy for runtime-added items** — C#
  caches `AllowedFleaPriceItemsForBarter` (ragfair) and `DbItemsCache`/`DbAmmoItemsCache` (scav case)
  per generator instance and effectively never invalidates; Rust re-derives per call (override sends
  only since flips #1/#5 — the eligible path is fresh-per-publish).
- **Container mutations of a table are invisible to the resident-DB families.** Since Phase 2 the
  Ceciler-injected write barriers (`Patches/Ceciler.WriteBarriers`) bump the stamp from every
  non-`init` property setter reachable from the published roots — the five tables plus, since Phase
  4, the 28 configs, so 33 roots — walking property types *and* base types (`TradersTable` declares
  nothing and reaches `Trader` only through its `Dictionary<MongoId, Trader>` base). What stays
  invisible:
  - `Add`/`Remove`/indexer-set on a table or config collection, at root level or one below
    (`trader.Assort.Items`, `handbook.Items`);
  - array element writes and reflection-driven writes;
  - the setters of the four denied live-per-request types — `Item`, `BotBase`,
    `PmcDataRepeatableQuest` (so a write to a trader assort `Item`'s `Upd` bumps nothing) and, since
    Phase 4, `GenerationData`, reachable through `EquipmentFilters.Randomisation[].Generation` and
    `KarmaLevel.ItemLimits` but written per bot by `BotWeaponGenerator` and
    `PlayerScavGenerator.AdjustItemWeights`. Neither field is read from the resident root today, but
    unread is not un-stale;
  - the setters of open-generic model types, never barriered by design — `MinMax<T>`'s three, so a
    mod editing a location's `Limit`/`MinMaxBot` bands writes nothing the stamp sees, and since Phase
    4 the same hole reaches config bands that *are* read resident (`ScavCaseConfig`'s
    `RewardItemValueRangeRub`, `AmmoRewards.AmmoRewardValueRangeRub` and the per-rarity `MinMax<int>`
    counts under `MoneyRewards` — the scav case's whole price and money band set);
  - anything behind an `object?`-typed property the walk cannot follow
    (`TemplateSide.EquipmentBuilds`/`WeaponBuilds`);
  - a genuine database write inside a native-response decode callback's extent —
    `SptNative.DecodeResult` holds a `WriteBarrier.Suppress()` scope across the decode because
    deserializing a response into DB-shaped model types was the real churn source. Nothing does that
    today and `WriteBarrierChurnTests` pins the invariant. The publish's own suppression scope spans
    `DbPayloadProjection`'s `LazyLoad.Value` reads, so a mod-registered transformer writing into a
    published root *other* than the value it transforms is suppressed too (both shipped `StaticLoot`
    transformers write only inside the transformed graph).

  **The config half is the wider half.** Config bodies are mostly collections, and the resident ones
  are the exposure: `ItemConfig.Blacklist`/`RewardItemBlacklist`/`BossItems`,
  `LocationConfig.LooseLootBlacklist`, `SeasonalEventConfig.ChristmasContainerIds`,
  `BotConfig.ItemSpawnLimits`/`CurrencyStackSize`. Where a table root's stale window needs a mod to
  reach for a container, a config root's opens for most runtime config edits short of a plain
  property set. Root-level tracking collections were evaluated and declined — 4 of ~90 mutation sites
  and none of the post-startup ones on a published root, against 17+ apicompat suppressions and a
  public `ProfileFixerService` signature break. Two mod-reachable container writes were closed by
  hand instead (`RagfairPriceService.ReplaceFleaBasePrices`, `CustomQuestService.CreateQuest`), each
  as an additive constructor overload, bringing hand-written bump sites to eight. The barriers also
  over-announce in two correctness-safe places: `UserBuild.Id`/`Name` are reached through
  `EquipmentBuild` ancestry from the profile templates (a profile build save dirties the stamp), and
  `LocationController.cs:44` (`mapBase.Loot = []`) is a genuine table write that costs a full
  six-root republish once per map per `client/locations` request — accepted, not mitigated
  (BENCHMARK.md § Phase 2).
- **Write barriers exist on Release and publish builds only.** Ceciler does not run in Debug, so
  `WriteBarrier.Installed` is false there and `ResidentDbDispatch.Eligible` refuses to honour
  `TrustNativeRequestCacheWithMods` — a Debug server with mods loaded always sends the views
  override. The trust flag must not vouch for barriers that were never injected.
- **Native database load requires comment-free JSON in the resident-root files** — templates,
  traders, globals, locations and hideout are parsed by `serde_json` inside `spt_db_load`, which
  rejects the comments the pure-C# reader tolerated (`JsonUtil` sets `ReadCommentHandling =
  JsonCommentHandling.Skip`; trailing commas are rejected on both paths). Non-resident files keep
  that tolerance — their bytes cross as buffers and are still deserialized C#-side. Shipped data
  passes (`rust/spt-native/tests/phase3_db_load.rs` is the gate), so this bites only a hand-edited or
  mod-shipped `database/`, for which `forceLegacyDatabaseImport` is the escape hatch.
- **A `database/` file whose entire body is the JSON `null` literal throws on the native path** —
  `ImporterUtil.DeserializeFileAsync` guards the buffer deserialize with a `?? throw`, where the disk
  path leaves the property null and carries on. Unreachable on shipped data (all 289
  `database/*.json` scanned); `forceLegacyDatabaseImport` covers a hand-edited tree.
- **The item base-class and linked-item cache *keys* differ** — legacy stores under `item.Id`
  (`ItemBaseClassService.cs:194,199`; `RagfairLinkedItemService.cs:200`), native under the
  `templateTable.Items` dictionary key. Separable only by a mod filing a template under a key ≠ its
  `_id`, where legacy is the broken arm (consumers resolve by dictionary key).
- **Golden-test parity is normalised, not raw-byte.** Every family has a full-output golden gate
  (`*ParityTests` in `Testing/UnitTests`). Sanctioned gaps: minted `MongoId`s, and for ragfair
  `intId` and `startTime`/`endTime` (one batch timestamp natively vs a per-offer clock in legacy).
- **The resident-DB views-equivalence gate is manual, not part of `dotnet test`.** Proving a
  natively-derived view matches the C#-built override over the real database is a two-step harness:
  an `[Explicit]` NUnit fixture writes the roots envelope and expected views
  (`RagfairViewsEquivalenceTests`, `QuestViewsEquivalenceTests`), then an `#[ignore]`d Rust test
  compares them (`rust/spt-native/tests/phase1_{ragfair,quest}_views.rs`). **Flip #6 took the other
  route and it is the one to copy:**
  `BotResidentDbTests.AResidentSendAndAnOverrideSendProduceIdenticalBotsFieldForField` sends the same
  seeded wave twice — resident, then with the C#-built `viewsOverride` — and compares as normalised
  JSON. It is a plain `[Test]`, so it runs in `dotnet test`, and it caught a real derivation
  divergence (flip #6 decision 2) the view-by-view harness would have had to be told to look for.
- **A Harmony patch on `FileUtil.WriteFileAsync`/`DeleteFile` no longer sees profile I/O** — since
  Phase 5 `SaveServer`'s disk boundary is `spt_profile_*`. Patches on `SaveServer`'s own members
  still fire, and `BackupService`'s copies still go through C#. **No escape hatch** (Phase 5 decision
  6): the two arms produce identical bytes and the write protocol is a step-for-step port, so a
  revert is a plain `git revert` with no stranded state — a second write path would rot untested.
- **Profile save and load cancellation is best-effort-before, never mid-flight** — a token is checked
  before the native call and cannot interrupt a started write. Atomicity is unchanged
  (`FileUtil.WriteFileAsync` was already temp-then-rename, `Utils/FileUtil.cs:113`); what changed is
  that a cancellation arriving after the call begins now lets the save complete. I/O failures on save,
  load, list and delete throw `InvalidOperationException` out of `SptNative.DecodeResult` where they
  used to throw `IOException`-family types — nothing in the tree catches those specifically, so this
  bites a mod with `catch (IOException)` around a profile operation.
- **A `user/profiles/` that cannot be `stat`ped now aborts startup instead of loading zero
  profiles** — `spt_profile_list` raises on any non-`NotFound` `stat` failure, and `SaveServer.LoadAsync`
  is an `IOnLoad`, so the throw stops the server with `Critical exception, stopping server...` plus
  path and errno. The reachable cause is a `user/profiles/` that lost `+x`: .NET's Unix enumerator
  answers `Directory.GetFiles` from `d_type` and never `stat`s, then `File.Exists` returned `false`
  for each child — so 4.1.2 invited the player to create a new profile beside intact ones.
  Deliberate: a loud stop on a `chmod`-recoverable condition beats a silent one presenting as data
  loss.
- **`LoadAsync` sees profiles in sorted order**, where `Directory.GetFiles` order was
  filesystem-dependent. Strictly more deterministic; nothing downstream reads the order.
- **A failure crosses as a message for C# to throw with** — never as a log line, so it carries no
  category. Since ABI 18 a panic crosses with its message too.
- **Hangs are mostly undiagnosable** — ported retry loops can spin exactly as 4.1.2 does, inside an
  FFI call with no managed stack trace. Force legacy to get the managed stack back.
- **`get_flea_prices_as_array` is O(offers × price table) if a mod enables barters.** Dead on shipped
  data (`ragfair.json` `dynamic.barter.chancePercent` is `0`). Latent, not measured.

Unreachable on shipped data, recorded because a mod could reach them: an unknown scav case recipe id
or an ammo pool empty in its rarity band (native returns a message, legacy NREs); a parentless or
cyclic parent chain on the hydrate (native terminates, C# stores `{ MongoId.Empty }` or recurses);
five malformed filter shapes on the linked-item walk (native skips, legacy throws on four and
silently drops camora ammo on the fifth); and the native `_type` test being `eq_ignore_ascii_case`
against C#'s `OrdinalIgnoreCase`. The parity gates would catch any of them.

### Logging

- **Generator lines carry one category per generator** — `typeof(T).FullName` of the C# class each
  Rust module ports, where the replay era logged the whole bot family through
  `ISptLogger<BotInventoryGenerator>`. A `sptLogger.json` filter against that class matches far fewer
  lines now.
- **Generator lines use a different `%tid%` space** — a process-local counter in first-emit order,
  not the managed thread id. `%tname%` is the Rust thread name, usually empty. `%date%` is the moment
  of emission, where replayed lines were stamped at the end of the call.
- **Generator locale text is a startup snapshot** — `DatabaseImporter` pushes resolved server locales
  once (`spt_locales_set`); a mod mutating them later no longer changes generator line text. A failed
  push falls every generator line back to its locale key.
- **Parallel generator lines interleave** — ragfair and bot rayon workers emit as they run, so lines
  no longer arrive grouped per bot or per assort entry.
- **Console output is asynchronous.** One 8192-slot bounded channel to a writer thread carries both
  rendered log lines and the raw `Console.Write*` bytes redirected into it. A burst of log lines
  deeper than the queue drops; raw bytes, and the drain `spt_console_read_line` does first, block
  instead, so a prompt is never lost. The ceiling: a terminal that stops draining stalls managed
  `Console.Write` behind it and makes `spt_logger_close` wait on the writer thread. Shutdown is
  carved out — `spt_logger_close` takes the pipeline out from under the lock before joining, so a
  `Console.Write` racing teardown writes straight to stdout and can interleave with the draining
  backlog. A hard crash still loses what is queued.
- **Excluded categories still pay the per-line marshaling cost** — filtering moved native-side, so
  every line crosses the boundary before it is dropped.
- **Filter regexes are regex-lite** — no lookarounds, no backreferences, ASCII-only character
  classes. A pattern that will not compile is reported to stderr once and then never matches.
- **A native logging failure has no C# fallback** — a failed `spt_logger_init` means no logging at
  all for the run, and the same for a config the C# parser tolerated but Rust rejects. The known
  cases are handled except the `type` tag of a `loggers` entry, case-sensitive on both sides.
- **The pipeline reads `sptLogger.json` once; runtime mutation needs an explicit reload** — mutating
  `SptLoggerConfiguration.Loggers` changes neither what is written nor, since the gate moved to
  `spt_log_enabled`, what `IsLogEnabled` answers; both read the *applied* configuration.
  `SPTLoggerDispatcher.ReloadConfiguration()` (additive, post-port) re-hands the object to
  `spt_logger_reinit`; a rejected reload leaves the running pipeline untouched. With no pipeline to
  ask, `IsLogEnabled` falls back to the C# object.
- **Line terminators are always `\n` and dates always Gregorian, culture-independent** — including
  `BaseLogHandler.FormatMessage` now that it renders through `spt_log_format`. The deleted C#
  `LogTime.ToString("yyyy-MM-dd")` / `("HH:mm:ss.fff")` used `CurrentCulture`, so under fi-FI, th-TH
  or ar-SA a mod handler stamped a different timestamp from the pipeline line beside it. Nothing pins
  the process culture; this makes the two agree.
- **File rotation was redesigned, not ported** — ZLogger rolled ascending (`spt.1.log` was the *next*
  file); the native sink cascades, so `.1` is the *most recent* archive and `spt.log` only ever holds
  the current run. Anyone comparing `spt.N.log` across the upgrade reads it backwards.
- **Lowering `maxRollingFiles` does not sweep the old high indices** — 10 → 3 strands
  `spt.3.log`..`spt.9.log` until deleted by hand (the `ponytail:` note in `log_sink.rs`'s `cascade`).
- **Mod `ILogHandler` routing goes through a hybrid tap.** The dispatcher fans C#-originated lines out
  to handlers; `spt_log_set_tap`'s callback delivers Rust-originated generator lines as rendered text
  with no `Exception` object. Registration changed shape: resolve `SPTLoggerDispatcher` from DI and
  call `RegisterHandler` (additive, post-port) — a constructor-injected handler set is always empty.
- **The fatal-error pause is "Press enter to exit..." (`spt_console_read_line`)** — the old
  `Console.ReadKey(true)` single keypress is gone; raw-mode stdin was not worth the platform console
  code. The two mod-load-failure pauses (`Program.cs:136` and `:147`) still read with no prompt at
  all: pre-existing.
- **`BaseLogHandler.FormatMessage` renders through `spt_log_format`** — a format containing bare `{`
  or `}` now renders literally instead of throwing out of `CompositeFormat.Parse`, a positional hole
  like `{0}` renders literally too, and a handler that cannot reach the native side degrades to the
  unformatted message rather than throwing.
- **A `GetCompiledFormat` override no longer reaches `FormatMessage`** — `BaseLogHandler` hands the
  reference's raw `Format` string to `spt_log_format`. The method still compiles and caches for
  direct callers; both shipped reference types are `sealed`, so reaching the override meant
  constructing the subclass in code.
- **`Console.Clear()`'s `IsOutputRedirected` guard became a Rust-side tty check** — clear and title
  are ANSI/OSC escapes queued behind the console sink, with VT enabled on Windows at
  `spt_logger_init` (where the Windows title is still a console API call).
- **A mod calling `Console.SetOut`/`SetError` — or setting `Console.OutputEncoding`, which does the
  same internally — silently un-wraps `NativeConsoleWriter`** and reverts that process to raw writes
  racing the pipeline on fd 1 (and, on Windows, the mojibake the UTF-8 codepage setup fixed), with
  zero test failures. There is no mechanical guard; the only in-tree warning is a comment in
  `Program.StartServer`.
- **Non-string `Console.WriteLine(value)` overloads can tear** — `TextWriter` decomposes them into
  `Write(value); WriteLine();`, two queue messages, so a log line from another thread can land
  between the value and its newline. Every in-tree call site uses the string overloads; the failure
  is cosmetic interleaving.

## Guidelines

1. **Frozen surface.** Preserve the ported class's entire 4.1.2 public *and protected* surface —
   constructor including parameter names, methods, DTOs. Enforced by `dotnet apicompat` in the
   sibling `mpex-api-compat` repo. The *surface* is frozen unconditionally; the *body* is not. Keep
   the C# implementation as the legacy path **only where Rust cannot reliably replace it**, which
   holds when either condition fails: (a) both arms produce identical observable output, so deleting
   the C# body strands nothing and a revert is clean, and (b) nothing mod-visible needs that body as
   guideline 2's patch-routing target. Phase 5's profile persistence dropped legacy (identical bytes,
   not a hookable algorithm); Phase 3's database import kept `ForceLegacyDatabaseImport` (different
   artifacts). Every generator family fails (b). A family that keeps no legacy path argues it in its
   ledger.
   Two apicompat gotchas: **post-baseline types are exempt as additions, not by visibility** — the
   whole `Native/` tree post-dates 4.1.2, and so do types outside it (`BotWaveBatcher` **lost a
   constructor parameter** at ABI 32 and came back clean, exit 0, no CP0002, invoking the tool
   directly against the frozen baseline DLLs). The rule is the type's age, not its directory. And
   **`mpex-api-compat/ci/check-api-compat.sh` resolves its tool manifest from the current working
   directory**, so anywhere cwd does not persist between shell invocations the script reports every
   assembly as failed — which looks exactly like a real break and actually means no analysis ran.
   Invoke the tool directly when you cannot guarantee cwd.
2. **Override contract.** Detect Harmony patches on the frozen members (`Harmony.GetPatchInfo`) and
   route to legacy so hooks fire with baseline semantics. Add a `forceLegacy...` config flag as the
   escape hatch for hooks detection can't see. A port that kept no legacy path under guideline 1 has
   no routing target, so it carries neither — the log pipeline and profile persistence are the two
   shipped examples.
3. **Resident DB epoch, publish on dirty.** DB-derived state lives resident on the Rust side:
   `DbPublisher` republishes every supported root when the global `DatabaseMutationStamp` has moved
   and stamps the returned epoch into each request. The published set is **six roots** since Phase 4
   — templates, traders, globals, locations, hideout, plus a `configs` root carrying all 28 loaded
   configs keyed by each one's `Kind`. Since Phase 2 the stamp is moved primarily by the
   Ceciler-injected write barriers, with eight hand-written bump sites for container writes barriers
   cannot see. Only the varying block — per-call **service** state plus whatever the caller itself
   selected — and the optional `viewsOverride` remain per-call. For the bot family the service-state
   half is now vacuous: ABI 32 took the last cached service state off `SharedBotVarying`, leaving
   `generatingPlayerLevel` and `isNightTime` (live reads), `equipment` (held off by a runtime writer)
   and the caller-selected `levelGeneration` / `templateVariants`.
   Ineligible callers — mods loaded where `TrustNativeRequestCacheWithMods` does not hold (defaults
   **on** since Phase 2, counts only where `WriteBarrier.Installed`), or anyone with
   `DisableNativeRequestCache` — send the C#-built view bundle as `viewsOverride` on every call,
   never touching resident state. Full protocol: the epoch-protocol section of
   `docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md`. Thirteen of the seventeen
   generation exports ride it (flips #1-#6) — the raid four carry no epoch at all — both slice
   caches are gone, and the eligibility rule plus the stale-epoch self-heal live once in
   `Native/Db/ResidentDbDispatch`; a family's own
   `ResidentDbEligible()` is a one-line wrapper.
4. **RNG parity.** Both sides draw through the shared xoshiro256\*\* source behind test-only seams
   (`Utils/RandomSource.cs` / `random_util.rs`), pinned by twin known-answer tests. Production C#
   randomness stays bit-for-bit unchanged.
5. **FFI envelopes are internal.** Request/response types are a C#↔Rust contract shipped in lockstep
   — change them freely, bump `spt_native_abi_version` and `SptNative.ExpectedAbiVersion` together.
   The assertion in `ffi.rs`'s `abi_version_export_matches_crate_const` is the third site and must
   move with them. No third-party consumer of the cdylib is supported.
6. **Ports keep an `[Injectable]` entry point.** A static wrapper like `SptNative` is only acceptable
   for startup-internal subsystems mods never touch. Anything patchable calls Rust from inside a
   resolved service.
7. **Gate loop** (no CI in this fork): `dotnet build -c Release` →
   `mpex-api-compat/ci/check-api-compat.sh` → `dotnet test` → `csharpier format .` → `cd rust &&
   cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`. Run
   `dotnet tool restore --tool-manifest <mpex-api-compat>/.config/dotnet-tools.json` first and invoke
   the script with cwd *inside* `mpex-api-compat` — see guideline 1's second gotcha.

### Exceptions in force

**Constructors.** Every family took an additive overload, never a signature change: `LootGenerator`
adds `LocationConfig` (extended in place — it post-dates the baseline — with the loaded-mod list +
`DbPublisher` in flip #4, when `LocationLootGenerator` took an additive overload for the same pair),
the four quest generators add `QuestConfig` + `RepeatableQuestNativeRequestBuilder`,
`ScavCaseRewardGenerator` adds `ScavCaseNativeRequestBuilder` plus a further overload for the loot
pair, `ItemBaseClassService` adds `ItemBaseClassNativeRequestBuilder` + `ItemConfig`,
`RagfairLinkedItemService` adds `RagfairLinkedItemNativeRequestBuilder` + `RagfairConfig`,
`RaidTimeAdjustmentService` and `LocationLifecycleService` each add `RaidNativeRequestBuilder`, and
`BotInventoryGenerator` adds the loaded-mod list + `DbPublisher`, chaining its frozen 4.1.2 primary
constructor. The container selects the overload; anything built through the frozen constructor gets a
null builder — or a null `DbPublisher`, which `ResidentDbDispatch.Eligible` answers `false` to — and
runs legacy or the override arm unconditionally. `BotWaveBatcher` post-dates the baseline, so its
primary constructor simply took the pair. Ragfair offer generation, `RepeatableQuestRewardGenerator`
and `RepeatableQuestHelper` needed no change.

**Config flags.** `LocationConfig.ForceLegacyLootGeneration` covers *both* loot generators — there is
no per-generator flag. Elsewhere: `BotConfig.ForceLegacyBotGeneration` and `ForcePerBotGeneration`,
`RagfairConfig.ForceLegacyRagfairGeneration` and `ForceLegacyRagfairLinkedItemBuild`,
`QuestConfig.ForceLegacyRepeatableQuestGeneration`, `ScavCaseConfig.ForceLegacyScavCaseGeneration`,
`ItemConfig.ForceLegacyItemBaseClassHydration`, `LocationConfig.ForceLegacyRaidAdjustments` (covering
all four raid exports across both services, on the `ForceLegacyLootGeneration` precedent), and
`CoreConfig.ForceLegacyDatabaseImport` — the one
flag that is not a generation path. Plus `TrustNativeRequestCacheWithMods` /
`DisableNativeRequestCache`, the resident-DB eligibility gate, on `RagfairConfig`, `QuestConfig`,
`ItemConfig`, `LocationConfig` (covering both loot generators), `ScavCaseConfig` and `BotConfig`
(covering both bot exports); the linked-item table reads `RagfairConfig`'s pair. Only
`forceLegacyLootGeneration` is serialised into a shipped `.json` (`location.json`); the rest are C#
defaults a user adds to the file to change — and since Phase 2 `TrustNativeRequestCacheWithMods`
defaults **on** in all six, so it is the flag a user adds to turn *off*.

**What flips to legacy.** Loot flips only on a *protected* member patch. Every other family flips on
a patch of any public/protected/protected-internal member of its frozen set, **except** the
dispatcher entry point itself — a patch there wraps whichever path runs, by design. Frozen sets:
bots, the four generator classes; ragfair, `RagfairOfferGenerator`, `RagfairPriceService`,
`RagfairServerHelper`, `RagfairAssortGenerator`; quests, the four `*QuestGenerator`s plus
`RepeatableQuestRewardGenerator` and `RepeatableQuestHelper`; scav case, base class and the
linked-item table, their own class only. Map/raid setup is the one **family-wide** set that spans two
services and is **member-scoped** on one of them: six methods — `RaidTimeAdjustmentService`'s
`GetMapSettings`, `AdjustWaves`, `AdjustPMCSpawns` and `GetExitAdjustments`, plus
`LocationLifecycleService`'s `AdjustExtracts` and `AdjustBotHostilitySettings` — and a patch on any one
of them declines all four exports at once. Whole-type on `LocationLifecycleService` would de-native the
family on a patch to any of its 23 methods, `StartLocalRaidAsync` chief among them (member-scoped
precedent: the seasonal pair above). `AdjustLootMultipliers` is deliberately outside it, the carve-out.
A container-substituted subclass also flips — **except
loot**, whose `UseLegacyPath` ends at the patch scan, so a `LootGenerator`/`LocationLootGenerator`
subclass at a higher `TypePriority` still runs native. Bots additionally flip on an
`InventoryMagGenComponents` set that isn't exactly the four built-ins. `PickupQuestGenerator`
contributes **zero** frozen hookable members — its whole legacy body is inline in `Generate`.

**The bot wave batches before it iterates.** `BotController.GenerateBotWave` offers the wave to
`BotWaveBatcher.TryGenerateWave` first; the batcher returns null — and the unchanged per-bot path
runs — on `ForcePerBotGeneration`, on anything `BotInventoryGenerator.UseLegacyPath()` already
catches, on a patch of any frozen `BotGenerator`/`BotLevelGenerator`/`BotEquipmentFilterService`/
`BotController` member except `GenerateBotWave`, on a substituted `BotGenerator`, `BotLevelGenerator`
or `BotEquipmentFilterService`, or on a wave that could write nighttime clamps (only the per-bot path
replays those). The response is one `{result | error}` envelope per bot in request order (ABI 8): a
failed bot is skipped with a Critical log and the rest of the wave still generates.

**The wave's level draw is native and its template ships per level band, not per bot** (ABI 22).
`BotLevelGenerator.GenerateBotLevel` + `ChooseBotLevel` are ported to `bot/level_generator.rs` with
**no new export** — the draw is the first act of each bot's rayon task, ahead of every other seeded
draw, and the drawn `level`/`exp` ride back for the caller to write into
`details.BotLevel`/`Info.Level`/`Info.Experience` before `CacheBot` reads them.
`GetRelativePmcBotLevelRange` stays C#-side (wave-constant inputs), shipped as `levelGeneration` —
PMC waves only; non-PMC takes constant level 1 and draws nothing, which keeps non-PMC seeded pins
byte-identical. Because every level-dependent *pre-call* step is a pure band lookup that draws
nothing — `FilterBotEquipment` (whose `Clothing` weighting adjustment also reshapes the appearance
and voice pools) and the `LootItemLimitsRub` price bands — the batcher splits the range at those
bands' edges and runs the **unchanged C#** filter, seasonal strip, blacklist strip and pool hydration
once per band, shipping one `templateVariants` entry per band. Segments are typically 1-3 (up to ~8
for a full 1..79 range) and always exactly one `[1..1]` for a non-PMC or playerscav wave. The per-bot
slice collapses to `botId` + `testSeed` + `details`; the whole request is
`{epoch, viewsOverride?, shared, bots[]}` (single-bot: `{epoch, viewsOverride?, shared, bot,
template, lootPools}`) at 13,341 bytes per bot at wave 45 against the override arm's 93,758. The
voice and appearance *draws* move **after** the call, onto the band the drawn level lands in. The
decline set grows two member-scoped seasonal-strip entries; pool and price hydration
(`BotLootCacheService.GetLootFromCache`, 12 calls, and `HandbookHelper.GetTemplatePrice`) also run
once per band and are deliberately **not** in it, because economy mods patch them constantly and
declining would de-batch most modded servers. Divergences: **none intended.** One fidelity note —
`AddAdditionalPocketLootWeightsForUnheardBot` applies to the cloned variant template with an `if let`
where C# dereferences `PocketLoot` unguarded, so a template with no `pocketLoot` block NREs on the
per-bot path and is a no-op here (documented at the port site). A PMC batch bot gains 1-2 draws at
the head of its stream by construction, so a PMC seeded pin repinning is expected and is *not* a
divergence; a changed **non-PMC** pin would be a bug.

**State replayed after a native call**, because Rust keeps it to itself: bot container grid occupancy
(`RestoreContainerGrids`) and nighttime mod-chance clamps (`ReplayRandomisationClamps`); ragfair's
`rejectedCanSellTemplates`, which sets `CanSellOnRagfair = false` on the live template table. The
quest `QuestTypePool` round-trips and is copied *into* the caller's instance (`CopyPoolInto`), not
swapped — the controller keeps reading that instance, so reference identity has to survive.

**The reward-loot blacklist is two collections, and since Phase 4 only one of them crosses** —
`configBlacklist` for the reward pool, `globalBlacklist` for sealed-container filters. They differ
once a mod calls `AddItemToBlacklistCache` at runtime; collapsing them would change behaviour.
`configBlacklist` is `ItemConfig.Blacklist` and now comes off the resident `spt-item` stem, so it
crosses only on the override arm; `globalBlacklist` is `ItemFilterService`'s mutable cache — service
state, not config — and still rides every send on both arms.

**Loose loot has two input paths.** Null `dynamicLootDist` splices `looseLoot.json`'s raw bytes in
unparsed (faster, more faithful); a registered `LazyLoad` transformer (seasonal events, mods) forces
the typed path, which is slower than both the raw path and the C# it replaced — so a mod can put a
server on the slow path without saying so. Since flip #4 the fork lives inside the request's varying
block (`varying.looseLoot`) on both arms. Loose loot is the one loot input that never went resident:
raw bytes resident would cost 549 MiB RSS (BENCHMARK.md § Phase 0). Phase 3 was the named revisit
point and declined it again for the same number; resident paths plus an on-demand read is the upgrade
if it is ever wanted.

**The ragfair batch walk is parallel only when unseeded.** An unseeded walk fans across rayon: a
forked `RagfairContext` per assort entry, merged back in assort order with `intId` reassigned during
the merge. A **seeded** walk stays sequential (the seeded RNG is `thread_local`) and every
`RagfairParityTests` case sets a seed, so parity rides the unchanged path. Production is unseeded on
both arms.

**The ragfair response is a framed MessagePack envelope, not a JSON buffer** — one length-prefixed
frame per offer behind a header frame (since ABI 10, encoding tag 1), deserialised with
`Parallel.For` straight out of the native buffer. Ragfair is the only export that uses it. Its batch
also takes **one timestamp** where legacy calls `TimeUtil.GetTimeStamp()` per offer.

**Every ported generation family reads the resident DB.** All key freshness on the same
`DatabaseMutationStamp`, a monotonic counter moved by the Ceciler setter barriers over 33 roots plus
eight hand-written sites for container writes: `SeasonalEventService.UpdateGlobalEvents`,
`ItemFilterService`'s two blacklist `Add*` methods, `CustomItemService`'s two `Create*` methods,
`CustomQuestService.CreateQuest`, `RagfairPriceService.ReplaceFleaBasePrices` and a guarded replay
bump when `CanSellOnRagfair` flips true→false. Requests arrive as `{epoch, viewsOverride?, varying}`;
an epoch the store does not hold returns `STATUS_STALE_EPOCH` (4), surfacing as
`NativeStaleEpochException` and self-healing with one `ForcePublish` + retry — a lost epoch costs one
republish, never a wrong result. Per flip:

| Flip | ABI | What went resident |
|---|---|---|
| #1 ragfair, #2 quests | 22, 23 | The templates, traders, globals and locations roots; both families' views derive at publish. |
| #3 base-class hydrate + linked-item table | 24 | No varying block, no new roots — walk inputs derive from the templates root at request time (props-less drop, first-filter-group-only). |
| #4 loot (six exports) | 25 | The three statics (`staticLoot`, `staticContainers`, `statics`, ~19 MB) as typed lifts on the locations root, serialized at publish from each `LazyLoad.Value`; preset views ride `RagfairDbViews.default_presets_by_tpl_key`. looseLoot stays a per-call splice, `staticAmmoDist` stays a parameter. |
| #5 scav case | 26 | A new `hideout` root — `production.scavRecipes` only. Recipe views derive at request time; `itemsView`/`staticPrices`/`defaultPresetsByTpl` borrow the ragfair views. |
| #6 bots (both exports) | 27 | No new root. `BotDbViews` derives at publish from templates + globals, embedding `RagfairDbViews` by `Arc` and adding only `defaultPresetIdsByTpl` and `expTable`. |
| Phase 4 configs | 30 | The sixth root: all 28 configs by `Kind`. Six families resolve their blocks per call — quest's `repeatableQuestTemplateIds`/`locationIdMap` off `spt-quest` and `rewardItemBlacklist`/`bossItems` off `spt-item`; reward loot's four `ItemConfig` sets off `spt-item`; ragfair's `dynamic` off `spt-ragfair`, `configBlacklist` off `spt-item`, `customMoneyTpls` off `spt-inventory`; location loot's `config` off `spt-location` and christmas container ids off `spt-seasonalevents`; scav case's whole block off `spt-scavcase`; eleven of the bot family's twelve lifts off `spt-bot`/`spt-pmc`/`spt-repair`. |

What still rides the varying block is service state plus inputs the caller selected or C# must
resolve per call. A mod writing an injected table's dictionaries directly is still invisible to the
stamp — its *scalar* writes are not, since Phase 2, so `TrustNativeRequestCacheWithMods` defaults
**on** and a modded Release server rides resident state. `DisableNativeRequestCache` remains the kill
switch; the container gap is the *Broken* ledger's container-mutations bullet. Ineligible callers
send a per-call `viewsOverride` with `epoch: 0` — a documented wire contract, not runtime-enforced.

### Ledgers

Each flip and phase landed with its own plan, ABI bump, goldens passing unchanged, and BENCHMARK.md
re-measured before the next started. Decision numbers are stable — cross-references elsewhere in the
tree cite them.

**Flip #1 — ragfair (ABI 22).**
- Freshness: legacy's hydrate-once caches (`TraderHelper` trader prices, `HandbookHelper` price
  lookup, `PresetHelper` preset store and default-preset maps) could serve stale values into a
  rebuilt slice; Rust re-derives every view per publish, so the resident path is uniformly *fresher*.
  The practical edge: a resident send and a `viewsOverride` send can diverge after a runtime
  mutation, because the override is still built through those caches.
- (b) pmc name lists stay C#-projected in the varying block. Flip #6 was the named revisit point and
  declined it (decision 1); Phase 4 declined it again (decision 10).
- (c) Runtime *config* edits bypassed the stamp — **closed by Phase 4** for scalar edits. Collection
  edits remain, as the *Broken* ledger's container bullet.
- (d) An `_items: []` preset added at runtime now aborts the publish loudly, naming the preset
  (`views.rs`'s `build_preset_cache`), where the old slice path tolerated it. Stricter, deliberately.
- `Native/` delta: +214/−70 across 7 files. `Db/` (`DbPublisher` + `DbPayloadProjection`, 105 lines)
  is new shared infrastructure every later flip reuses.

**Flip #2 — repeatable quests (ABI 23).**
- Freshness: pre-flip the quest slice was rebuilt from live tables on every send that carried it;
  post-flip an *un-stamped* table mutation is invisible until the next stamped publish.
- The quest views share `items`/`handbookPrices`/`fleaPrices` with the ragfair views through one
  `Arc`; `defaultWeaponPresets`, `defaultPresetOrItemPrices`, the repeatableQuests lifts and the two
  location maps are quest-own derivations at publish.
- The `locations` root is `Base` + `AllExtracts` only, keyed by the locations' `JsonPropertyName`
  strings (e.g. `factory4_day`) and domain-bounded by `LocationTable.GetDictionary()`; a null
  `AllExtracts` ships as `[]`.
- `Native/` delta: +175/−103 across 5 files.

**Flip #3 — base-class hydrate + linked-item table (ABI 24).**
- Freshness: an eligible hydrate — including `GetLinkedItems`' rebuild on a cache miss mid-run
  (`RagfairLinkedItemService.cs:126-133`) — reads last-published state. Override sends still project
  live tables per call.
- The walk-input equivalence handshake (`OneShotViewsEquivalenceTests` / `flip3_oneshot_views.rs`)
  ran green over the full real database: 4,553 base-class chains and 4,673 linked-item sets identical.
- `Native/` delta: +207/−1 across 5 files — growth, because the one-shots' whole pre-flip payload
  *was* the projection, which survives as the ineligible arm.

**Flip #4 — loot, six exports (ABI 25).**
- Freshness: statics refresh on publish, not per call, so a transformer registered *after* the last
  stamped publish (registering one bumps no stamp) is invisible to eligible sends until the next bump.
  The kill switches cover it.
- looseLoot stays per-call on both arms — 549 MiB RSS on top of the measured 405.2 MiB publish delta,
  for a payload read once per raid start that already rides a zero-copy `WriteRawValue` splice.
  Residency was deferred to Phase 3, where it was declined outright on the same number.
  `staticAmmoDist` is permanently varying: it is a parameter of the frozen public signatures.
  `GetDefaultPresetsByTplKey`'s duplicate-first-item-tpl case now aborts the publish loudly naming the
  culprit preset, where pre-flip C# threw `ArgumentException` per forced-loot call. One saving landed
  as a side effect: sealed's resident arm no longer builds `presetsByTpl` at all.
- `Native/` delta: +201/−51 across 5 files.

**Flip #5 — scav case (ABI 26).**
- Freshness, a direction flip: pre-flip the native scav case re-derived item and price pools per call,
  so it was *fresher than legacy* for runtime-added items; post-flip the eligible path is
  fresh-per-publish. Presets moved the same way flip #4's did.
- The `hideout` root carries `production.scavRecipes` only, at the real table path's
  `JsonPropertyName`s. No view derives from it at publish — the recipe views derive at request time,
  preserving the C# skip-a-recipe-missing-`endProducts`-or-a-band semantics bug-for-bug, where a
  publish-time derive would have to abort loudly. The raw root pins the capitalized
  `Common`/`Rare`/`Superrare` wire names (`HideoutProduction.cs`); the request-time derivation maps
  them onto the existing lowercase `ScavRecipeView` — zero generator-algorithm change.
- Eligibility + branch + stale-retry mirror `LootGenerator` exactly. The pre-flip hydration sweep
  found readers only and no lazy writer into `Production.ScavRecipes`, so no `DbPublisher` pre-touch
  carve-out was needed.
- `Native/` delta: +110/−30 across 5 files.

**Flip #6 — bots, both exports (ABI 27).** Closed Phase 1.
- Freshness: the database half — items view, `itemPresets`, `defaultPresetIdsByTpl`, exp table — is
  last-published state. Handbook prices carry one mod-only edge: the eligible arm reads
  `RagfairDbViews.handbookPrices`, keyed off the **items table**, where the override arm calls
  `HandbookHelper.GetTemplatePrice` per drawn tpl — so a tpl priced in the handbook but absent from
  the items table prices at its handbook value on the override arm and at 0 on the resident one.
  Unreachable for generatable loot.
- **1, no bots root.** Its only consumer would be the pmc name lists: the bot family reads no bot
  templates resident, so the root would carry 5.7 MiB and ~94.6 ms of warm projection on every
  publish (~13% of the measured ~735 ms) to serve two name lists. And
  `GatherPmcNamesOfLength` filters on a config value, so the derivation could not go resident before
  Phase 4 anyway. Phase 4 declined it a second time (decision 10).
- **2, `modPoolSlotOrder` is not a view.** The plan had it deriving into `BotDbViews`; Task 5's
  field-for-field resident-vs-override identity test caught the divergence, and the root cause is not
  port drift — the C# order is the live `BotEquipmentModPoolService`'s `ConcurrentDictionary`
  enumeration order, process-local and not a function of the database. The Rust derivation was
  deleted and the field moved into `SharedBotVarying` at 26,428 bytes per send, then left the wire
  entirely at ABI 32 when Rust took the pools — the member was deleted rather than re-homed.
- **3, `BotDbViews` as built**: `{ragfair: Arc<RagfairDbViews>, defaultPresetIdsByTpl, expTable}`.
  The two bot-own members are a re-key of `defaultPresetsByTpl` to each preset's own id
  (`ToDefaultPresetIds`) and `globals.config.exp.level.expTable[].exp`, lifted out of
  `BotWaveBatcher`. The derivation is total and `Result`-shaped so a future hard failure aborts the
  publish.
- **4, the handbook-price union stays the override arm's shape.** `BuildViewsOverride` prices the
  union of every loot pool the send can draw from (one cache single-bot, one per level band batched)
  rather than the whole handbook — collision-safe, since a tpl in two pools resolves to the same
  `GetTemplatePrice`. The eligible arm reads the resident items-keyed map and needs no union.
- **5, one envelope for both exports**, one `resolve_bot_views` resolver returning `LootEpochError`,
  so `STATUS_STALE_EPOCH` and the self-heal behave identically on both. `SharedBotViewsWire` was
  renamed `SharedBotVaryingWire`.
- **6, the two-arm dispatch block stays copied — evaluated and declined.** The fifth-copy rule targets
  *identical* blocks; across the 11 sites the block takes ~6 distinct shapes (per-export
  `ViewsOverride` expressions, a `bool viewsOverride` parameter, a `.Result` unwrap, early-return
  one-shots with no varying block, bots' mutate-the-request form). A shared helper would be generic
  over the request type, take two builder closures and a delegate for each site's private-set
  `LastSendIncludedViewsOverride` — a 5-parameter abstraction replacing a 12-line `if/else` whose only
  duplicated line is the flag assignment. Commit 1 extracted the part that *was* identical into
  `ResidentDbDispatch`. Revisit only if a future flip makes the arms converge.
- `Native/` delta: **+227/−285 across 7 files** — the program's **first genuine shrink**.
  `BotPayloads.cs` alone is +88/−151 (wire types collapsed onto the shared envelope) and the seven
  copied dispatch blocks became `Native/Db/ResidentDbDispatch` (38 lines). Both flip-#5 review
  carryovers are discharged here; the stale-epoch retry now has a scav case self-heal test
  (`ScavCaseResidentDbTests`).

**Phase 3 — fused database load (ABI 29).** `spt_db_load` fuses the `checks.dat` hash walk with
reading `database/`: one walk hashes (when verifying — Debug ships no `checks.dat`), reads and
installs the five resident roots as epoch 1, then hands the eager file bytes back for `ImporterUtil`'s
reflection walk. `CoreConfig.ForceLegacyDatabaseImport` restores the pure-C# arm.

**Measured, the flip is a regression.** At the importer, **935.7 ms against legacy's 480.6 ms — 0.51x,
+419–455 ms** (BENCHMARK.md § Phase 3), with 202 files / 49.4 MiB of eager content crossing as
buffers. The deliverable is the retired startup double-read, but against a warm page cache that read
was nearly free while the buffer plumbing is not: the buffer-fed walk is 50–68 ms *slower* than the
disk walk it replaces (451.1 vs 383.8 ms), and the fused load costs ~380–391 ms over the bare verify
(484.6 vs 96.8 ms) for buffer retention, the FFI copy and five-root assembly, parse and derivation.

**The startup win is not wired up.** `DbPublisher.EnsureCurrent` still republishes every root
whenever `_currentEpoch` is 0, and nothing feeds `DbLoad`'s installed epoch into it, so both arms pay
a 730–745 ms forced publish. Feeding that epoch through buys less than it looks: `EnsureCurrent`
republishes when `_currentEpoch == 0` **or** `_lastPublishedStamp != stamp` (`DbPublisher.cs:46`),
and on Release the barriers move the stamp during `PostDbLoadService` before that first
`EnsureCurrent` ever runs. *(Corrected by the load-epoch seed follow-up: the mover is exactly one
write, `coreConfig.ServerStartTime` at `PostDbLoadService.cs:53-56`, which precedes the first
`EnsureCurrent` — `HydrateItemBaseClassCache`, five lines down. `AdjustLocationBotValues`
(`PostDbLoadService.cs:627`) bumps the stamp too, through `LocationBase`'s Ceciler-injected setters,
but it runs at `:95`, after that first publish, so it was never in the pre-publish window.)*
**Phase 4 moved the goalposts further:** `spt_db_load` stays `database/`-scoped, so epoch 1
installs five of the six published roots and carries no `configs` root — skipping the first
`EnsureCurrent` on the strength of epoch 1 would leave every config-reading family without one.
*(Also corrected: an absent root is not a resolve failure. A family whose view resolver finds no
configs root answers `STATUS_STALE_EPOCH` — status 4 — which the C# side already self-heals with
`ForcePublish` + one retry, so the cost of getting this wrong would have been a silent extra
republish, not a fault.)*
The follow-up now has to publish configs at load time from the live C# objects, not from
`configs/*.json` (the values-not-keys trap), or keep the first republish. *(Delivered 2026-08-26 —
see the load-epoch seeding ledger below.)*

- Freshness: **none at generation time.** Epoch 1 is boot-validation only, always superseded by the
  first `EnsureCurrent` republish, so no generation path ever reads it. *(Corrected by the
  load-epoch seed follow-up: on a modless boot the first `EnsureCurrent` now consumes the seed
  instead of republishing, so epoch 1 plus the configs-only publish — epoch 2 — is exactly what
  every eligible family reads until the `RagfairCallbacks` settle publish.)*
- **1, loose-loot residency declined.** 549 MiB on top of the measured 405.2 MiB publish RSS delta is
  954.2 MiB, leaving no headroom under the ~1 GB line Phase 0's RSS gate drew. The spec's
  byte-serving export was **not built**. `locations/*/looseLoot.json` and `locales/global/*` are
  classified never-read and stay disk-path `LazyLoad`s on both arms;
  `staticLoot.json`/`staticContainers.json` are read for root assembly but not returned.
- **2, per-file buffer handoff, not per-root.** The C# reflection walk stays and remains the
  file→property mapping authority; Rust owns file→wire for the five resident roots only, keyed
  `database/…` on both sides and consumed inside `DeserializeFileAsync`. Reproducing
  `LoadRecursiveAsync`'s mapping semantics exactly was this phase's named risk, and keeping the C#
  walk neutralizes it structurally. The one duplicated semantic — importer skip lists and lazy
  patterns — fails benign both ways: an extra returned file is ignored, a missing one falls back to a
  disk read.
- **3, epoch-1 assembly is validated by parse + derive + the real-tree integration test**
  (`rust/spt-native/tests/phase3_db_load.rs`), not a C#-envelope equivalence harness. **The hazard
  that hides, for whoever wires the load-time epoch through:** `classify` (`load.rs:74`) and
  `LOCATION_MEMBERS` (`load.rs:18`) are a second, independent file→wire mapping duplicating
  `DbPayloadProjection`, and **nothing gates it** — `DatabaseLoadEquivalenceTests` compares
  `DatabaseTables`, never the resident roots. What makes it safe today is only that epoch 1 is
  superseded by the very republish the follow-up exists to remove. Gate it against a
  `DbPayloadProjection` publish before, not after. *(Delivered with the load-epoch seed:
  `ResidentRootEquivalenceTests` is that gate — always-on, five roots, digest compare over the
  typed lift surface via `spt_db_resident_digest`.)*
- **4, the equivalence golden is permanent.** `DatabaseLoadEquivalenceTests` compares the
  legacy-built and native-built `DatabaseTables` root by root and pins that the fused load returns a
  file under every root it compares (`ImporterUtilPreloadedTests` covers consumption). Plain `[Test]`,
  so it runs in `dotnet test`.
- `Native/` delta: +132/−1 across 2 files, almost all additive — one `[LibraryImport]` entry, the
  `DbLoad` wrapper and the framed-response parser with its three internal DTOs. The single deletion is
  the `ExpectedAbiVersion` constant. The phase's other edits land in `ImporterUtil`, `JsonUtil` and
  `DatabaseImporter`.

**Phase 4 — the configs root (ABI 30).** All 28 loaded configs publish as a sixth root keyed by
`Kind`, and six families read their config data off it. No new exports, no new derived views,
`spt_db_load` untouched.

- Freshness, and it is a real cost. For a *scalar* config write it is a wash — the Ceciler walk covers
  all 28 config types (33 roots), so a property set moves the stamp and the next call republishes. For
  a *collection* mutation it is a straight loss: `Add`/`Remove`/indexer-set on `ItemConfig.Blacklist`,
  `LocationConfig.LooseLootBlacklist`, `BotConfig.ItemSpawnLimits` and their kind was read fresh on
  every send before and is now invisible until some other stamped write lands. Config bodies are
  mostly collections, so the window is wider than the table roots', and these are values a family
  genuinely reads. The kill switches restore per-call freshness; root-level tracking collections
  remain the sanctioned remedy, declined on the same arithmetic as Phase 2.
- **1, wire keys are the `kind` strings** — read from each config's own `Kind` while iterating the
  injected dictionary, not C# type names and not file stems. No reflection needed.
- **2, all 28 publish; Rust lifts only the ten stems it reads** (`spt-item`, `spt-scavcase`,
  `spt-ragfair`, `spt-inventory`, `spt-quest`, `spt-location`, `spt-seasonalevents`, `spt-bot`,
  `spt-pmc`, `spt-repair`); everything else rides the flatten map full-fidelity. The root measured
  free.
- **3, configs arrive by `spt_db_publish` only.** Raw `configs/*.json` bytes are not the live objects
  (C# record defaults, `PostDbLoadService` fixups, mod edits), so assembling a root from disk walks
  into the values-not-keys trap.
- **4, consumption is per-call resident reads** (the scav-case recipe-view precedent): no field joined
  `ResidentDb`'s derived views, no derivation gate changed. One functional exception — ragfair gained
  `customMoneyTpls` off `spt-inventory`, retiring the divergence where offers priced in a mod-added
  currency took the unrounded arm.
- **5, the ineligible arm keeps its cost.** Each family's `viewsOverride` bundle gained the config
  block its varying half used to carry. Measured flat: a single-bot override send is 4,152,813 B
  against the pre-phase 4,208,129 B.
- **6, the barrier extension is 28 root FQNs, not a namespace sweep.** The `_denied` list gained
  `GenerationData` (four denied types now) and gained name validation, so a drifted entry fails the
  build instead of silently barriering nothing.
- **7, `Option<Lift>` is the strictness contract at the stem boundary.** An absent stem is `None` —
  the root parses and the family's per-call resolve fails loudly naming the stem. A present-but-
  malformed stem fails the whole publish parse (`STATUS_BAD_ARGS`) — and not for one call only:
  `DbPublisher.PublishLocked` never reaches `_lastPublishedStamp = stamp` when the publish throws, so
  every later `EnsureCurrent()` re-attempts and throws again, from outside `ResidentDbDispatch.Send`'s
  try, and every eligible native call 500s until the config is fixed. Reachable only through a mod
  nulling a `required` member with trust on. Three lifts deliberately break the rule: `spt-item`'s
  four sets and `spt-inventory`'s `customMoneyTpls` stay `#[serde(default)]` despite being C#
  `required`, and `spt-pmc` parses as the soft `PmcConfigWire`; `phase4_configs_root.rs` pins the soft
  members' wire names.
- **8, caller-selected config stays varying** — quest's `repeatableConfig`, loot's
  `containerSettings`/`rewardDetails`, bots' `levelGeneration`.
- **9, the bot equipment blacklists moved to native selection.** The per-(role, level)
  `FirstOrDefault` over `BotConfig.Equipment[role].Blacklist` became a Rust lookup, pinning the
  deliberate `level ?? 0` divergence between the two lists; selection is not a draw, so it is
  RNG-neutral. Both members left the wire entirely on both arms.
- **10, the pmc name lists stay varying** — `GatherPmcNamesOfLength` still reads the bot *table*,
  which has no root. A names-only mini-root is the standing upgrade if the varying cost ever measures.
  The same answer covers every `SeasonalEventService` / `ItemBlacklistCache` /
  `LootableItemBlacklistCache` / `RagfairLinkedItemService` / `GetMoneyTpls`-backed field: those are
  **service state, not config**, and no phase currently owns them.
- **11, `customMoneyTpls` is the one projection divergence fixed**; every other stays bug-for-bug.
- **12, family order** was scav case → ragfair → quest → reward loot → location loot → bots.

Three rulings amended the plan during execution. **The two loot multipliers stay per-call** —
`RaidTimeAdjustmentService.AdjustLootMultipliers` scales `LocationConfig.StaticLootMultiplier` and
`LooseLootMultiplier` **in place** through the dictionary indexer for a shortened scav raid and puts
them back after generation, so no property setter fires and a resident snapshot would hand the raid
unadjusted PMC-density loot. Both ride the varying block as C#-resolved per-location scalars on both
arms and land unread in `LocationConfigLift.extra`;
`LootResidentDbTests.AnInPlaceLootMultiplierAdjustmentReachesAResidentSend` pins it.
**`BotConfig.Equipment` stays varying** — the phase's largest planned lift, declined for a worse
version of the same reason: `BotInventoryGenerator.ReplayRandomisationClamps` writes the nighttime
mod-chance clamps back into `Equipment[role].Randomisation[band].EquipmentMods` through the indexer
after *every* native single-bot send, and that write is a deliberate cross-bot feedback loop the next
bot's C# prelude reads (`BotEquipmentFilterService.cs:63`). A published copy would freeze at the
on-disk values and diverge from bot 2 of a nighttime raid on. Eleven of twelve planned bot lifts
landed; the upgrade path is roadmap item 3. **`ItemConfigLift.blacklist` is a `HashSet<String>`, not
the plan's `IndexSet`** — the override wire mirrors C#'s `HashSet`, so both arms read one shape and
there is no iteration site to observe an order. Zero `[Test]` bodies, assertions, seeds or normalizers
were edited anywhere in the phase.

`Native/` delta: **+289/−210 across 13 files** — mostly members moving from a varying record into the
family's `viewsOverride` record in the same file, each with a doc line naming its resident equivalent.
`Db/DbPayloadProjection.cs` (+17/−3) is the whole configs-root writer;
`Bot/BotPayloadProjection.cs` (+31/−50) is the only genuine shrink, where the two blacklist
projections were deleted in favour of native selection. The wire did shrink: **−138,121 B per
eligible bot send, a flat 23.0–23.4% at every wave size**, down to 10,272 B per bot at wave 45. The
sixth root cost **+3.1 to +3.8 ms** of cold publish — inside a 719–811 ms per-recipe spread, against
a budgeted ~67.7 ms. What now dominates an eligible bot send is the caller's own `templateVariants`
at **83.2%** of the request.

**Phase 5 — profile persistence (ABI 31).** Four `spt_profile_*` exports own `user/profiles/`' live
listing, reads, writes and deletes. No resident state, no new root, no legacy path and no config
flag: the profiles directory arrives in every request and profile bytes are opaque.

Freshness is unchanged — profile bytes were never resident and every load and save still hits disk
through the same MD5 gate. What moved is *failure* visibility and I/O posture:

- **Mid-write cancellation no longer exists** (decision 7). A started write always completes.
  Atomicity is unaffected either way.
- **I/O failures throw a different type** (decision 8). `ProfileError{BadArgs,Io}` crosses as
  `STATUS_BAD_ARGS`/`STATUS_ERROR` and `DecodeResult` raises `InvalidOperationException`, where
  `FileUtil.WriteFileAsync` and `JsonUtil.DeserializeFromFileAsync` raised `IOException`-family types.
  `RemoveProfile` changed the same way; throw-vs-no-throw and the `bool` return are unchanged — a
  missing file is still `false` and still just logged.
- **Profile I/O is no longer on async file handles.** Both `useAsync: true` sites became a blocking
  syscall on a `Task.Run` threadpool thread. `SaveAsync` and `LoadAsync` loop sequentially, so exactly
  one thread is parked at a time — a property of those loops, not parity, and a future concurrent
  caller would not inherit it.
- **A `default`/empty `MongoId` now throws** (decision 9), where the old body silently probed
  `user/profiles/.json`. Unreachable in-tree, but not for the tidy reason: `LoadProfileAsync` applies
  **no** id check of its own (`SaveServer.cs:198-199`), so for loads the native gate genuinely is the
  first thing an empty id meets and the protection is entirely in the callers — `LoadAsync`
  pre-filters on `MongoId.IsValidMongoId`, `LauncherV2Controller.cs:156` passes a freshly minted id,
  and `CreateProfileService.cs:239-244` receives its id from the session (its first statement,
  `saveServer.GetProfile(sessionId)`, throws on `IsEmpty`). `SaveProfileAsync` never reaches Rust
  with one: `IsProfileInvalidOrUnloadable` returns `false` for an absent key
  (`SaveServer.cs:331-343`), so an empty id passes that guard, takes the save lock, and dies on
  `profiles[sessionID]` (`SaveServer.cs:282`) exactly as before. The `!sessionId.IsEmpty` guard at
  `LauncherV2Controller.cs:95` is on `RemoveProfile`, not the load/save pair; other disk-reaching
  callers are `CreateProfileService.cs:239-244`, `GameCallbacks.cs:70`, `PrestigeController.cs:98`
  and `LocationLifecycleService.cs:500,719`.
- **Profile listing is sorted**, where `Directory.GetFiles` order was filesystem-dependent.
- **UTF-8 BOM handling is now explicit.** The `FileStream` deserialize skipped a BOM for free; the
  `ReadOnlySpan<byte>` overload does not, so `profile.rs::load` strips it (reusing
  `db/load.rs::strip_bom`). Net behaviour is unchanged — that is the point — but the guard is now
  load-bearing code, and deleting it silently sends hand-edited BOM'd profiles down the
  `-corrupt.json` + backup-rollback arm.

One further change landed as its own commit (`e7d3a4b`) ahead of the native swap: **autosave failure
isolation changed shape.** `SaveAsync` now catches per profile (rethrowing on cancellation) and
writes `saveMd5` *after* the write instead of before. Together: a failed write no longer marks that
profile version as persisted, and one unwritable profile no longer aborts the rest of the tick.
Before, the second property held only by accident, through the poisoned hash — shipping the reorder
alone would have converted a per-version loss into an unbounded multi-profile autosave outage, which
is why the two halves are one commit. `SaveAsyncSurvivesOneUnwritableProfile` is the pin.

**Listing semantics, corrected against the plan's own text.** Decision 5 said `backups/`,
`-corrupt.json` and stray `.bak` files "are excluded by the same C# lines that exclude them today" —
right about the files, wrong about the directory. `-corrupt.json` and `.bak` do reach C# and are
dropped by the extension filter and the `MongoId.IsValidMongoId` stem gate. `backups/` is a
**directory**: `profile.rs::list` keeps only entries whose `fs::metadata` says `is_file()`, so it
never reaches C#. The false premise matters because it would later justify "simplifying away" C#
filters that are in fact the only thing excluding the two file cases. `fs::metadata(entry.path())` is
used and not `entry.metadata()` because only the free function follows symlinks — `DirEntry::metadata`
is `lstat` on Unix and would classify a symlink-to-a-profile as neither file nor directory.
Following-then-`is_file()` matches `Directory.GetFiles` on the two cases that matter (measured on
.NET 10.0.10) but is **not exact**; the source is the accurate account.

A **dangling** symlink is still skipped (`list_skips_a_dangling_symlink`) — a real divergence, since
`GetFiles` returns the link, but inert: a dangling `{id}.json` link passes both C# filters, and what
saves us is `load`'s own `NotFound` arm answering `found: false` regardless. A **denied** `stat` is
now raised, and this is why the code changed: `readdir` needs only `+r` while `stat`ping a child needs
`+x`, and .NET answers file-vs-directory from `d_type`, so on a `user/profiles/` that lost `+x`
`GetFiles` returns every profile while every `fs::metadata` fails `EACCES`. Swallowing that reported
an empty directory and `LoadAsync` offered to create a new profile beside intact files.
`list_raises_an_unreadable_entry` pins the fix and self-guards (root bypasses the search bit).
**This is an improvement over the pre-phase C#, not a parity restoration:** `File.Exists` also
returns `false` on `EACCES`, and both `LoadProfileAsync`'s guard and `DeserializeFromFileAsync`'s
short-circuit (`JsonUtil.cs:104-107`) are `File.Exists` — so the pre-phase path enumerated every
profile and then silently loaded none, the same zero-profile presentation one stage later.

Decisions: **1, Rust is stateless and `dir` rides every request** — no module static; residency waits
on the profile-model port (`todo/TODO.md` #19). **2, serialization stays C#** — Rust is a
byte-faithful passthrough (`RawValue` on save, raw frame bytes on load), so on-disk format is
byte-identical and the MD5 dirty-check is unaffected. **3, the MD5 dirty-check and per-session save
locks stay C#.** **4, `BackupService` stays C#**: Rust owns live-file writes, deletes and the
load-time listing; C# keeps the read-only probes, the corrupt-copy, the backup copy loop and the
restore copy. The only writer overlap is restore-during-load, already serialized inside
`LoadProfileAsync`'s recovery arm. **5, all four exports take the standard envelope shape**
(`{"schema":1,"dir":…}`, plus `id` on three); not-found rides the load frame header
(`{"found":false}`) and no new status code was added — every filter stays in C# verbatim, so there is
zero filter-parity risk. **6, no legacy path and no `forceLegacy` flag** (the `SPTLoggerDispatcher`
precedent). **7, cancellation before the native call only.** **8, the error surface is
`ProfileError{BadArgs,Io}`**, message naming the path and the OS error. **9, Rust guards the id** —
24 ASCII hex chars, mirroring `MongoId.IsValidMongoId` (`Extensions/MongoIdExtensions.cs:52-68`).
This is the path-traversal guard at the trust boundary and is non-negotiable even though C# always
passes a typed `MongoId`. **10, the `DbPublisher._currentEpoch == 0` unconditional republish is
declined again** — independent of the profile disk boundary and still blocked on the values-not-keys
mapping gate; the discussion is the load-time-epoch follow-up in the Phase 3 ledger. **11, no
benchmark fixture, but the free number was taken.** **12, plain synchronous `std::fs` on the calling
thread** — single-file ops need no tokio, and C# keeps its async posture through `Task.Run`.

`Native/` + `SaveServer` delta: **+266/−18 across 3 files** — `NativeMethods.cs` +12/−0,
`SptNative.cs` +226/−1 (four wrappers, the `ProfileLoadResult` record, the frame parser; the single
deletion is the ABI constant) and `SaveServer.cs` +28/−17.

**The measurement, because decision 11 pre-committed to it.** `SaveProfileAsync`'s returned
milliseconds on a **26.50 MB synthetic profile**, 6 runs per pass and two passes per state: **~161 ms
median (155–186) before, ~192 ms median (187–217) after — about 20% slower, and the ranges do not
overlap across any of the four passes.** A real regression, recorded as one. The profile is synthetic
and the harness throwaway, so the figure sizes the effect rather than pinning it.

Attribution, and the naive version is wrong: the pre-phase path did **not** stream.
`fileUtil.WriteFileAsync(filePath, jsonProfile, ct)` took the `string` overload
(`Utils/FileUtil.cs:103-107`), one `Encoding.UTF8.GetBytes` into a full-size `byte[]` given to a
single `fs.WriteAsync` — so peak was already `jsonProfile` (UTF-16) plus one full-size UTF-8 buffer,
and the `MemoryStream` **replaces** that buffer rather than adding one. The two real new costs are
`profile.rs`'s **owned** `pub profile: Box<RawValue>` (`profile.rs:175`), so serde scan-skips the
profile and then copies all 26.5 MB — the one extra full-size copy at peak — and
`Utf8JsonWriter.WriteRawValue(string)` (`SptNative.cs:633`), which transcodes through a `chars × 3`
scratch buffer rented from `ArrayPool<byte>.Shared`. That second cost is **weaker than it looks**:
the shared pool *does* serve buffers this large (measured on .NET 10.0.10 by reference identity at 1,
4, 16, 80, 128 and 512 MB — the ~1 MB cliff belongs to `ArrayPool<T>.Create()`'s
`ConfigurableArrayPool`, not `Shared`), so there is no guaranteed ~3x allocation per save: the first
save on a thread allocates ~6.2x the char count and every later one on that thread ~2.0x (both
harness-inclusive; the ~4.2x *difference* isolates the scratch). What keeps it real is that
`ProfileSaveAsync` hops through `Task.Run` and the pool's fast path is a per-thread TLS slot, so a
save landing on a cold threadpool thread pays the first-call price. In steady state the honest cost is
the UTF-8 encode pass, not an allocation.

**The ruling is that the regression ships**: the remedy would make `spt_profile_save` the first export
off the shared `run_generator_with` ladder, at the tail of a phase whose entire value is mechanical
parity. It is re-opened as roadmap item 4. **The load side was not timed, but its allocations are pure
addition** where the save path's replaced an old buffer: `DeserializeFromFileAsync` streamed with
`bufferSize: 4096` so no full-size buffer existed, where the native path materialises three transient
ones — `fs::read` (`profile.rs:133`), `encode_load_frame` (`profile.rs:154-165`) copying into a second
exactly-sized `Vec` so `into_boxed_slice` does not realloc, and `ParseProfileFrame`'s
`span[at..].ToArray()` (`SptNative.cs:598`) copying onto the managed heap because the native buffer is
freed as soon as the wrapper returns. On the same 26.50 MB profile that is ~80 MB of churn per load
against approximately zero before; at most two are live at once, so concurrent peak is ~53 MB. Two of
the three are native, so `GC.GetTotalAllocatedBytes` would not see them, and none of it is on the
save-side follow-up's path.

**Phase 6a — `mpex-server` bootstrap (landed 2026-08-18, no ABI change).** An `mpex-server` bin crate
hosts the CLR via `netcorehost`; `run_app` is shipped by publish and is the release container's
entrypoint, with `scripts/smoke-mpex-server.sh` as its e2e check. `mpex-server.exe` ships from the
same wiring but has never been executed on Windows.

**Phase 6b — rlib linkage flip (landed 2026-08-21, no ABI bump).** The resident DB's statics now live
in the executable: `mpex-server` links `spt-native` as an rlib and is linked with
`-Wl,--export-dynamic`, so all 39 exports sit in its own `.dynsym`, and the two
`SetDllImportResolver` callbacks try `NativeLibrary.GetMainProgramHandle()` before the cdylib. The
published Linux tree therefore ships no cdylib and `SPT.Server.Linux` is no longer a working
direct-run fallback there.

It is **not** the design the spec described. The planned shape — `initialize_for_runtime_config` +
`get_delegate_loader_for_assembly` + an `[UnmanagedCallersOnly] Init(HostVTable*)` in a shim
assembly, with `DllImport` replaced by a 34-slot vtable and an ABI bump — was written out in full,
reviewed twice, and replaced: `run_app`, `Program.Main`, `[LibraryImport]` and the ABI all stay, and
the change is ~85 lines. Five spec overrides and the declined `Build.props` order flip (nothing
forces it: `mpex-server` links a sibling crate, not `SPT.Server.dll`) are in the ledger. Carried
forward:

- **Windows exports.** An `.exe` has no export table without `/EXPORT:` args or a `.def` file, so the
  cdylib exclusion is Linux-gated and Windows behaviour is unchanged — which also still means never
  executed, and `Build.props:31` still maps no `win-x64` triple.
- **The one-linkage-path-per-process rule is enforced by publish layout, not structurally.** The
  published tree has no cdylib, so a lost export anchor is a loud boot failure there; a `bin/` tree
  keeps one for `dotnet test`, so the same mistake under a locally-built launcher boots silently with
  the statics in the cdylib. Nothing at runtime can distinguish the two —
  `GetMainProgramHandle()` is a `dlopen(NULL)` pseudo-handle.
- **The launcher arm has no end-to-end gate outside `scripts/smoke-mpex-server.sh`,** and this fork
  has no CI to run it. `dotnet test` always takes the cdylib arm; `DllImportResolverTests` pins only
  that the test host correctly declines the launcher one.

**Mod-pool ownership (ABI 32, landed 2026-08-25).** `BotPayloadProjection.BuildModPoolSlotOrder` is
gone. Flip #6 decision 2 had shown the order could never go resident — it is the live
`BotEquipmentModPoolService`'s `ConcurrentDictionary` enumeration order, process-local — so the exit
taken was the other one Phase 2's write barriers made safe: **own the pools rather than observe
them.** The native pool now enumerates the template's own `Properties.Slots`, and the 26,428 bytes
per send left the wire on both arms. This is a **deletion** on the C# side rather than a port: pool
*contents* were derived natively from the bot port onward (`mod_pool_service.rs`), and only the
*ordering* was ever observed from C#. `BuildRequest` fell **5.19 → 0.23 ms** (assault, BENCHMARK.md
§ Mod-pool ownership). What it bought: the C# order was sized from `Environment.ProcessorCount`, so
it was never machine-independent, and the native draw order is host-independent now. What it cost:
the two arms draw in different orders at randomised levels, and the exact-output coverage there is
gone on both — booked in the *Broken* ledger, together with the process-nondeterminism finding that
made a C#-side golden unimplementable. `BotEquipmentModPoolService` gained a whole-type decline
entry (guideline 2), Rust no longer consulting it.

**Load-epoch seeding (ABI 33, landed 2026-08-26).** The Phase 3 follow-up, delivered: on a modless boot
the first `EnsureCurrent` no longer publishes. `DatabaseImporter.LoadDatabaseAsync(seedResidentDb:)` is
opt-in and `Program.cs` passes true only when `loadedMods.Count == 0`; it follows the native load's
five-root epoch-1 install with a **configs-only publish built from the live C# config objects** — never
`configs/*.json`, the values-not-keys trap — reaching epoch 2, and records `(epoch, stamp)` in the static
`DbLoadSeed`. `DbPublisher.EnsureCurrent` consumes that seed once: it forces the same `HandbookHelper`
hydration a first publish would have forced, under the same `WriteBarrier.Suppress()`, then either logs
`Load-time seed consumed at epoch N; first publish skipped.` and starts from that epoch, or logs
`Load-time seed voided: …` and republishes because the stamp moved in the window. Two changes close that
window. `ItemConfig.HandbookPriceOverride` now rides the `spt_db_load` request, so the resident handbook
carries the merged prices C#'s lazy hydration produces — an **envelope-only merge**: the raw handbook
bytes are restored before `files` is handed back, so the C# reflection walk still parses the shipped
file. And `PostDbLoadService`'s `coreConfig.ServerStartTime` write, the one stamp mover that precedes the
first `EnsureCurrent`, is suppressed; the carve-out leaves the resident `spt-core` entry stale by exactly
that field until the next real republish, which is safe only because nothing native lifts `spt-core`
(lift the suppression the day a consumer appears). The gate is `ResidentRootEquivalenceTests`: the
load-installed roots against a `DbPayloadProjection` publish of the same tree, compared through
`spt_db_resident_digest` (ABI 33's new export) as canonical post-parse digests of the **typed lift
surface** — `extra` maps excluded, because envelope text legitimately differs there in member order,
number formatting, explicit nulls and Debug-build model coverage. What it buys is **publishes 2 → 1**,
not 1 → 0: `AdjustLocationBotValues` still bumps the stamp before `RagfairCallbacks` generates offers, so
that second publish stays, by design. Measured at the boot, that is **−861 ms to `Server has started`
against the merge base, −7.5%** — the skipped publish is worth −2286 ms in the `PostDbLoadService`
block and gives 483 ms back on the import line and 880 ms back to the now-cold `RagfairCallbacks`
publish (BENCHMARK.md § Load-epoch seeding, which also explains why boot-to-`/health` reads this as a
*regression*: `/health` answers before the publish it skips). The `Database import took Nms` line now
contains a publish on a modless boot and is no longer comparable to any pre-phase figure.

**Map/raid setup (ABI 34, landed 2026-08-27).** The whole of `RaidTimeAdjustmentService`'s algorithm
plus `LocationLifecycleService`'s two `LocationBase` passes moved to `src/raid/`, behind four exports:
`spt_get_raid_adjustments`, `spt_make_adjustments_to_map`, `spt_adjust_bot_hostility_settings` and
`spt_adjust_extracts`. `Native/` grew 909 lines across four files — `Native/Raid/RaidPayloads.cs`
(525), `Native/Raid/RaidNativeRequestBuilder.cs` (297), plus the `[LibraryImport]` and wrapper entries
in `NativeMethods.cs` and `SptNative.cs`. The two services keep their full 4.1.2 bodies.

- **Deltas cross, not objects, and that is what the aliasing forced.** Each export takes a small
  C#-projected request carrying only the members the algorithm reads and answers with keep-index
  lists, per-index field updates, append selections and warning flags; a thin C# applier mutates the
  *original* objects in legacy order. Three live-object channels thread through what looks like a
  clone-only mutation — `AdjustPMCSpawns` offsets `.Time` on live `PmcConfig.CustomPmcWaves` instances
  the PMC splice appended by reference (a permanent config mutation that compounds across shortened
  raids — an upstream bug, preserved), `AdjustBotHostilitySettings` appends live `ChancedEnemy`
  instances into the clone, and `AdjustExtracts` assigns a deferred `Exits.Union` over live
  `AllExtractsExit` instances. A whole-`LocationBase` round trip would sever all three, so it would
  need a replay block anyway — at which point the object crossing pays for nothing, while costing a
  29-record model mirror and megabytes per raid start (at `LLS:211`/`:213` the clone already carries
  the generated loot). Deltas keep the reference identity structurally: the applier writes the very
  objects legacy wrote, and the `Exits.Union` statement is kept verbatim. Precedent is the repo's own —
  the bot level draw riding back for the caller to write, `ReplayRandomisationClamps`, the quest
  `QuestTypePool`'s `CopyPoolInto`.
- **The carve-out is `AdjustLootMultipliers`,** which runs C#-side on **both** arms and is excluded
  from the frozen set: it writes the live `LocationConfig` multiplier dictionaries through the indexer,
  and `GenerateLocationLoot` reads them a few lines later when it builds *its* native request. Phase 4
  ruled this exact write is why the multipliers stay per-call in the loot family's varying block. A
  patch on it therefore does **not** decline the raid family to legacy, and fires on the native arm —
  pinned by `RaidAdjustmentHookLivenessTests`.
- **No resident state, no new root, no epoch** — Phase 5's precedent. Option C was costed and declined:
  `EscapeTimeLimit` and `Exits` sit untyped in `LocationBaseView::extra`, `scavRaidTimeSettings` untyped
  in `LocationConfigLift::extra`, and `SurvivedSecondsRequirement` has no globals lift at all, so going
  resident means new lifts, a changed digest surface and the whole eligibility/trust/stale-heal
  machinery — to amortize a ~2 KB payload on a menu-frequency endpoint, while *adding* a stale window
  per-call projection does not have. **It is the named upgrade path** if a later phase wants those lifts
  for other reasons; it is owned by no phase today. Consequence: the four exports carry no `epoch` and
  no `viewsOverride`, `TrustNativeRequestCacheWithMods`/`DisableNativeRequestCache` do not apply, and
  `FfiFailure` has a single arm — raid never returns `STATUS_STALE_EPOCH`.
- **ABI 34 has four constant sites, not three.** `lib.rs:19`, `SptNative.ExpectedAbiVersion`
  (`SptNative.cs:124`) and the `ffi.rs` tripwire assert (`:1699`) are the lockstep three; this family
  added a fourth in prose at `rust/ARCHITECTURE.md:79` ("currently 34"). Any renumber — the
  parallel-branch collision rule, whichever of two same-number bumps lands second — is four sites plus
  a docs grep for the stale number, not three.
- **No standing benchmark fixture**, by ruling rather than omission: menu and raid-start frequency has
  no throughput to win, so Phase 5's decision 11 applies — the free number off the parity run is
  recorded (BENCHMARK.md § Map/raid setup) and no `[Explicit]` harness is added.
- **Booked divergences — all exception-type changes, no behavioural one.** One is reachable on shipped
  data without a mod: `labyrinth` is in `LocationTable` but absent from `scavRaidTimeSettings.maps`, so
  its missing-key `KeyNotFoundException` becomes an `InvalidOperationException` naming the map (whether
  the shipped client flow can queue a scav there is unverified). The rest need a mod-shaped config: null
  `AlwaysEnemies` with additional enemy types (NRE → error) and a non-numeric `ReductionPercentWeights`
  key (`FormatException` → error). `MakeAdjustmentsToMap`'s error paths apply no deltas where legacy
  left a partially-mutated, then-abandoned clone — unobservable, the multiplier side effect being
  identical on both arms.
- **`AdjustBotHostilitySettings` Quirk-10 error path applies no deltas where legacy left earlier roles
  applied.** Unobservable: the clone half is abandoned identically, and the one surviving mutation — the
  duplicate-`Role` merge write onto the live `PmcConfig` `ChancedEnemy` (LLS:316-324) — is idempotent
  and value-independent (LLS:310 clears and the loop recomputes first := last every run);
  `pmcConfig.HostilitySettings` has no other reader. Reachable only with mod config. The native error
  message *text* is invented — legacy NREs with no message — so the spec books the type change, not the
  text.
- **Null `ExitChanges` on a session-parked `RaidChanges`** would NRE in legacy at RTAS:62 and fails JSON
  parse natively (`Vec` rejects null) → BAD_ARGS → `InvalidOperationException`. Unreachable: RTAS:215
  always writes `[]`. It is booked because the request DTO carries the **real** `RaidChanges` record
  rather than a mirror — a required-`List` mirror could not hold the null the divergence depends on.
- **Two log lines are dropped and booked.** The train-disable debug line (RTAS:351) — its
  `mostPossibleTimeRemainingAfterDeparture` operand is a per-exit intermediate the wire does not carry —
  and the negative-weight warning inside `GetWeightedValue` (WeightedRandomHelper.cs:80), which fires
  mid-draw where the applier cannot see it and which the Rust twin already drops. Neither is
  load-bearing. Every other message is re-emitted verbatim C#-side from the delta fields, under the same
  `IsLogEnabled` guards; only *timing* moves (after the call, not during).
- **Two builder touch-order fidelity notes.** The hostility builder no longer dereferences
  `BotLocationModifier` when the config loop would run zero iterations — legacy no-ops there, so the
  early deref was a *new* exception rather than a booked type change, and the fidelity bar books only
  type changes. The extracts builder's early `GetLocation` **is** booked unobservable: the `TryGetValue`
  chain cannot throw, and the identical string is already dereferenced 82 lines earlier.
- **Quirk 4's "a `None` time seeds the offset" sub-state is unreachable** and the spec and plan still
  assert it — recorded here so nobody re-litigates it. The offset filter's pmc set is a strict subset of
  the keep filter's, and the keep filter admits a pmc spawn only with `Some(time) > start`, so every
  spawn that reaches the offset has a time. The dead legs are ported and commented anyway, per the
  fidelity bar; their reachable halves are tested.

**The ported 4.1.2 quirks are documented at their call sites** as numbered `Quirk N` comments in
`rust/spt-native/src/quest/*.rs`, `src/scav_case/generator.rs`, `src/base_class.rs`,
`src/linked_items.rs`, `src/loot/container_extensions.rs` and `src/raid/*.rs`; grep
case-insensitively for `quirk`,
which also turns up unnumbered ones in the bot, loot and ragfair modules. Some numbers have no Rust
site because the quirk lives on the C# side or on no code at all. The behaviour these preserve is
deliberate; reverting one silently diverges from C#. The bare `:N` line numbers in those comments are
the 4.1.2 body the port was written against, not the current file.

## Roadmap

State-ownership Phases 1 through 6b are complete (ledgers above). Open work:

1. **Port queue** — candidates and their costing live in [todo/TODO.md](todo/TODO.md); the unstarted
   front is tier 2. The two axes are independent: a flip re-homes data for something already ported,
   a TODO item ports something new.
2. **Convert `is_valid_reward_item`'s trader whitelist** (`quest/reward_generator.rs:869`, a
   `Vec<&str>` of up to 14 candidates) to `ItemBaseClassCache::is_of_baseclasses_set` and measure
   whether 14 is long enough for the set form to pay. Narrow and unmeasured.
3. **Split `BotConfig.Equipment`** (named by Phase 4, not delivered with it). Lift `equipment` onto
   the resident configs root and keep one varying member carrying just the live role+band
   `EquipmentMods` that `ReplayRandomisationClamps` writes, gated by a second-bot nighttime regression
   test. At 39,811 bytes it is now the largest — and only — member of genuinely varying process state
   on a bot send.
4. **Frame the profile save request** (named by Phase 5, not delivered with it). The removable costs,
   in the order worth chasing: the owned `Box<RawValue>` copy in `profile.rs` (a genuine extra
   full-size copy at peak), and `Utf8JsonWriter.WriteRawValue(string)`'s `chars × 3` transcode
   scratch — a full UTF-8 encode pass on every save, though *not* a guaranteed allocation. Handing the
   wrapper UTF-8 bytes takes the `ReadOnlySpan<byte>` overload and buys the encode pass; framing the
   request the way the load response is framed removes the `RawValue` copy. The price is that
   `spt_profile_save` becomes the first export off the shared `run_generator_with` ladder, which is
   exactly why Phase 5 declined to do it inline.
