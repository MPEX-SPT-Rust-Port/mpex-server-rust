# Rust port — status and roadmap

What is ported to `rust/spt-native`, what is known-broken, and what comes next. For the crate's
internals see [rust/ARCHITECTURE.md](rust/ARCHITECTURE.md); for the C# side of the boundary see
[ARCHITECTURE.md](ARCHITECTURE.md) § *Native Rust layer*. Every measurement lives in
[BENCHMARK.md](BENCHMARK.md); no timings are repeated here.

## Status

The loot family, the bot family, dynamic ragfair offer generation, the repeatable-quest family, scav
case rewards, the item base-class cache build and the ragfair linked-item table are ported and run
natively by default. Every ported class keeps its full 4.1.2 C# implementation as a **legacy path**,
selected automatically when a mod hooks it or manually via a config flag. The log pipeline is ported
too, and has no legacy path: `SPTLoggerDispatcher` hands every line to the crate, and the crate owns
the terminal outright — raw `Console.Write*`, prompts, title and clear all cross the boundary.

Thirty-four C-ABI exports (`src/ffi.rs`) carry all of it, JSON in and JSON out — except the ragfair
response, which is a framed MessagePack envelope, `spt_db_load` and `spt_profile_load`, whose
responses are a JSON header frame followed by the loaded file bytes, and the log and console
exports, which pass the fields of one line, or raw bytes, directly (current ABI 32).

Since Phase 6b those exports are reached two ways, one per process. A shipped Linux build resolves
them out of the `mpex-server` executable itself, which links the crate as an rlib; dev builds, the
test run and Windows resolve them from the `spt_native` cdylib. The call shape is identical either
way — `[LibraryImport]` against the name `spt_native`, with the resolver choosing the source.

Native is not uniformly faster. Loot and repeatable quests win; bots, reward loot, ragfair, scav
case, the base-class hydrate and the linked-item table are slower than the C# they replace, and
native stays their default anyway — each case is argued where it is measured, in
[BENCHMARK.md](BENCHMARK.md), and each has a force-legacy flag for anyone who disagrees. Ragfair is
the one family that set itself a parity gate and **missed** it; the resident-DB flip narrowed the
gap without closing it, and every lever short of the remaining state-ownership phases is spent.

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

Also working: mod-added fields on game data survive the round trip (`#[serde(flatten)] extra` maps
mirroring Ceciler's `[JsonExtensionData]`); native generator diagnostics render and log themselves
through the native pipeline; seeded-RNG parity at the primitive level (xoshiro256\*\*, twin
known-answer tests both sides).

**`rust/` is a two-crate workspace, and the second crate is not a port.** `rust/spectre-facade` is
a build-time generator: it emits a facade `Spectre.Console.Ansi.dll` (via `dotnetdll`) carrying
just `Spectre.Console.Color`, because SPT dropped the real dependency while the frozen 4.1.2 mod
surface still bakes that type into `ISptLogger<T>`, `SptLogMessage`, `ClientLogRequest` and
`Watermark.Draw` — a compiled mod's typeref names the *defining* assembly, so only an assembly of
that name satisfies it (in Spectre.Console 0.57.2 `Color` lives in `Spectre.Console.Ansi`, not
`Spectre.Console`). It is guideline 1 paid in a second language rather than anything ported. Two
consequences: **`SPTarkov.Common` needs the Rust toolchain too**, not just `Core` — its
`BuildSpectreFacade` target shells out to `cargo` — and the `<Reference>` is not transitive, so
each of the five projects naming `Color` carries its own. The colours are inert (the logger prints
plain text); known cosmetic gaps are `FromInt32` returning Default instead of the xterm palette
entry and the inherited `ValueType.ToString()`. Scope is `Color` only — mods that called
`AnsiConsole`, `Markup` or `Style` still break, that surface never being SPT's contract.

## Broken / known divergences

### Behaviour

- **Patches on collaborators do not reach the native path** and do not flip to legacy — only the
  ported classes' own members are detected. Affected: `RandomUtil`, `ItemHelper`,
  `CounterTrackerHelper`, `BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper`, `BotLootCacheService`, `WeightedRandomHelper`,
  `ItemFilterService`/`PresetHelper` predicates, `ICloner`, plus ragfair's
  `HandbookHelper`, `PaymentHelper`, `BotHelper`, `TraderHelper`, `SeasonalEventService`, quests'
  `MathUtil` and scav case's `RagfairPriceService.GetStaticPriceForItem` and `HideoutTable` reads.
  Two partial exceptions. `SeasonalEventService.ChristmasEventEnabled` and
  `RemoveChristmasItemsFromBotInventory` *are* detected — member-scoped, by the bot wave batcher
  only, because the batch re-times them per level band. A patch there de-batches the wave to the
  per-bot path, where the strip runs in C# and the patch takes effect; every other use of the type,
  ragfair's included, stays undetected. And `BotEquipmentModPoolService` is detected **whole-type**
  by `BotInventoryGenerator` since ABI 32 — seven of its eight methods build a pool or read one and
  the eighth, `ResetWeaponPool`, clears one, and Rust owns the pools outright now, so a patch on any
  of them can only take effect on the legacy path and routes there. Narrowed, not closed: the type's
  two `protected` pool properties (`GearModPool`, `WeaponModPool`) stay undetected, because
  `_hookableMembers` filters on `!method.IsSpecialName` and property accessors are `IsSpecialName`.
  Those properties are the backing state the getters read — exactly where a mod would inject pool
  contents — so the residual is the one shape of patch that most wants detecting.
- **The native and legacy bot paths draw mod slots in different orders at randomised levels.** Since
  ABI 32 the native pool enumerates in the template's own `Properties.Slots` order; legacy
  enumerates `BotEquipmentModPoolService`'s `ConcurrentDictionary`, whose bucket count is sized from
  `Environment.ProcessorCount` (measured moving at 13 real slot names between 8 and 16 cores). So
  the two arms produce different — not wrong — bots for one seed, and only the native side's
  *order* is machine-independent. Nothing cross-arm covers the mod pool **at randomised levels** now,
  and nothing covers native there on its own either: a different draw order means different RNG
  consumption, so no order-insensitive comparison would pass, and the C#-side golden that was to
  replace the cross-arm assertion is unimplementable for the reason in the next entry. (The level-1
  matrix is untouched — `TheSameSeedGeneratesEquivalentInventoryOnBothPaths` still deep-compares
  whole inventories cross-arm over 4 roles × 2 seeds. Those cases do still reach the module at level
  1, but only through `get_required_mods_for_weapon_slot`, which reads the template's slots directly
  and never `derive_pool` — the three call sites that *do* build a pool (`bot_inventory_generator`
  `:970`, `bot_equipment_mod_generator` `:1020` and `:1574`) each sit behind a randomisation gate
  the level-1 roles never trigger. So **no cross-arm case at any level** covers the ordering this
  change moved. That required-mods method is the same one `BotHookLivenessTests` patches, for the
  same reason.) **The exact-output coverage the randomised-level matrix carried is therefore gone on
  both arms, not moved.** In its place `BotParityTests.TheNativePathGeneratesAtRandomisedLevels` is
  a smoke case over the same 44 cases (2 roles × 22 seeds), and says so in its own comments: it
  asserts that generation completes, that the native path ran rather than falling back to legacy,
  and that the inventory is non-trivial — nothing about *which* items came out. The nighttime
  clamp's *effect on the inventory* is uncovered on both arms as a result, though
  `TheNighttimeRandomisationClampIsReplayedOnBothPaths` still pins the clamp write itself, which is
  what that case was built for. A patch on the pool service declines to legacy (guideline 2), which
  is where the machine-dependent order still applies.
- **Native bot output is not reproducible across processes**, so no *C#-side* golden can pin it —
  pre-existing, wider than the mod pool, and surfaced by the ABI 32 work rather than caused by it.
  `MongoId.GetHashCode` (`Models/Common/MongoId.cs:325`) is `HashCode.Combine`, which .NET seeds
  from a per-process random value, so every `Dictionary<MongoId, …>` the bot projection serialises
  enumerates in a process-random order and the seeded native draw walks that order. Two back-to-back
  runs of one isolated fixture produced inventories differing in item **count** (69 → 68), not
  merely in ordering, so no normaliser absorbs it. ABI 32 does not change this and did not cause it:
  `MongoId.cs` is untouched by that work, and the cross-arm tests that were immune were immune only
  because they compared native to legacy *inside a single process*. **The limit is C#-side only, and
  a Rust-side golden does hold:** `flip6_bots_resident.rs` drives both bot exports through the FFI
  in its own process off a synthetic DB, and `src/bot/` has no equivalent hazard — no `HashMap`
  anywhere in it, its four `HashSet`s membership-tested rather than iterated, everything the draw
  walks an `IndexMap`/`IndexSet`. Its `RESIDENT_BATCH_GOLDEN` pins the exact bytes of a three-bot
  batch at fixed seeds — both PMC level bands and the preset fallback — and held across four
  separate processes and both build profiles. **It does not, however, reach `derive_pool`**, the
  ordering this change moved: both routes there are gated on a populated
  `EquipmentFilters.randomisation`, that fixture supplies a `blacklist` band only, and its one drawn
  weapon slot has a single candidate (confirmed by making `derive_pool` panic unconditionally — the
  test still passed). So the golden is end-to-end drift detection over the bot pipeline, not an
  ordering pin. **The named upgrade:** give the fixture a `randomisation` band with a
  multi-candidate randomised slot and the same golden covers the ordering too — viable precisely
  because the spike proved Rust-side goldens reproduce across processes where a C#-side one cannot.
  The C#-side fix — sorting the projection's `MongoId`-keyed dictionaries before serialising — would
  work and is deliberately not taken here, because it changes the draw order on **every** native
  path and so alters generated bots server-wide: a live-wire behaviour change owing its own spec and
  parity gate, not a test repair. Deferring is safe because this is a testability limit rather than
  a production defect — bots are random by design and no consumer asks two processes to agree.
- **Templates without `_props` read as "not in the db"** on the native *generator* paths — they are
  dropped from `itemsView`. Only bites mod-added props-less templates. The base-class hydrate
  projects the whole table and is unaffected.
- **The native ragfair and scav case paths are fresher than legacy for runtime-added items** — C#
  caches `AllowedFleaPriceItemsForBarter` (ragfair) and `DbItemsCache`/`DbAmmoItemsCache` (scav
  case) per generator instance and effectively never invalidates; Rust re-derives per call
  (override sends only since flips #1/#5 — the eligible path is fresh-per-publish; see the flip
  ledgers).
- **Container mutations of a table are invisible to the resident-DB families** — since Phase 2 the
  Ceciler-injected write barriers (`Patches/Ceciler.WriteBarriers`) bump the stamp from every
  non-`init` property setter reachable from the published roots — the five tables plus, since
  Phase 4, the 28 configs, so 33 roots in all — a walk over property types
  *and* base types, since `TradersTable` declares nothing and reaches `Trader` only through its
  `Dictionary<MongoId, Trader>` base — so a mod's scalar writes reach the resident DB without a
  hand-written bump. What stays invisible: a mod calling `Add`/`Remove`/indexer-set on a table
  collection (root-level or one below — `trader.Assort.Items`, `handbook.Items`) or, since Phase 4,
  on a **config** collection, array element
  writes, reflection-driven writes, the setters of the four denied live-per-request types (`Item`,
  `BotBase`, `PmcDataRepeatableQuest` — so a write to a trader assort `Item`'s `Upd` bumps nothing —
  and, since Phase 4, `GenerationData`, which the configs root made reachable through
  `EquipmentFilters.Randomisation[].Generation` and `KarmaLevel.ItemLimits` but which
  `BotWeaponGenerator` and `PlayerScavGenerator.AdjustItemWeights` write per bot generated, so a mod
  writing `botConfig.Equipment["pmc"].Randomisation[i].Generation["healing"].Weights` or
  `playerScavConfig.KarmaLevel["-7"].ItemLimits["healing"].Whitelist` bumps nothing and the resident
  configs root — which ships whole `BotConfig`/`PlayerScavConfig` — keeps the pre-write values until
  the next stamped write; neither field is read from the resident root today, but unread is not
  un-stale, and the hole opens the moment one is), the setters of
  open-generic model types, which are never barriered by design (`MinMax<T>`'s three, so a mod
  editing a location's `Limit`/`MinMaxBot` bands writes nothing the stamp sees, and since Phase 4
  the same hole reaches config-side bands that *are* read resident: `ScavCaseConfig`'s
  `RewardItemValueRangeRub`, `AmmoRewards.AmmoRewardValueRangeRub` and the per-rarity `MinMax<int>`
  counts under `MoneyRewards` are the scav case's whole price and money band set), anything behind an
  `object?`-typed property the walk cannot follow
  (`TemplateSide.EquipmentBuilds`/`WeaponBuilds`), and a genuine database write performed inside a
  native-response decode callback's extent — `SptNative.DecodeResult` holds a
  `WriteBarrier.Suppress()` scope across the decode, because deserializing a response into
  DB-shaped model types (a repeatable quest's condition subgraph, `SpawnpointTemplate`) was the real
  churn source. Nothing does that today and `WriteBarrierChurnTests` pins the invariant by name. The
  publish's own suppression scope has the same shape and one mod-reachable edge: it spans
  `DbPayloadProjection`'s `LazyLoad.Value` reads, so a mod-registered transformer that writes into a
  published root *other* than the value it transforms is suppressed too (both shipped `StaticLoot`
  transformers write only inside the transformed graph).
  **The config half of that list is the wider half.** Config bodies are mostly collections, and the
  resident ones are the exposure: `ItemConfig.Blacklist`/`RewardItemBlacklist`/`BossItems`,
  `LocationConfig.LooseLootBlacklist`, `SeasonalEventConfig.ChristmasContainerIds`,
  `BotConfig.ItemSpawnLimits`/`CurrencyStackSize`. So where a
  table root's stale window needs a mod to reach for a container, a config root's opens for most
  runtime config edits short of a plain property set — and unlike much of the table case, these are
  values a family reads off the resident root today.
  Root-level tracking collections were evaluated and declined — they would have caught 4 of ~90
  mutation sites in the tree and none of the post-startup ones on a published root, against 17+
  apicompat suppressions and a public `ProfileFixerService` signature break. Two mod-reachable
  container writes that bumped nothing were closed by hand instead
  (`RagfairPriceService.ReplaceFleaBasePrices`, `CustomQuestService.CreateQuest`), each as an
  additive constructor overload — both services turned out to be on the frozen 4.1.2 set — which
  brings the hand-written bump sites to eight. The kill switches cover the remainder. The barriers
  also over-announce in two known places, both correctness-safe: `UserBuild.Id`/`Name` are reached
  through `EquipmentBuild` ancestry from the profile templates, so a profile build save dirties the
  stamp (cold path, one extra republish), and `LocationController.cs:44` (`mapBase.Loot = []`, once
  per map per `client/locations` request) is a genuine table write that taxes the next native call a
  full republish of every root — six of them since Phase 4 — once per client menu load, accepted
  rather than mitigated (BENCHMARK.md § Phase 2).
- **Write barriers exist on Release and publish builds only.** Ceciler does not run in Debug, so
  `WriteBarrier.Installed` is false there and `ResidentDbDispatch.Eligible` refuses to honour
  `TrustNativeRequestCacheWithMods` — a Debug server with mods loaded always sends the views
  override. Deliberate: the trust flag must not vouch for barriers that were never injected.
- **Native database load requires comment-free JSON in the resident-root files** — templates,
  traders, globals, locations and hideout are parsed by `serde_json` inside `spt_db_load`, which
  rejects the comments the pure-C# reader tolerated (`JsonUtil` sets
  `ReadCommentHandling = JsonCommentHandling.Skip` and no trailing-comma knob — trailing commas are
  rejected on both paths, `AllowTrailingCommas` appearing nowhere in the tree). Non-resident files keep that tolerance: their
  bytes cross as buffers and are still deserialized C#-side on exactly the same options as before.
  Shipped data passes — `rust/spt-native/tests/phase3_db_load.rs` runs the fused load over the real
  tree and is the gate — so this bites only a hand-edited or mod-shipped `database/`, for which
  `forceLegacyDatabaseImport` is the escape hatch.
- **A `database/` file whose entire body is the JSON `null` literal throws on the native path** —
  `ImporterUtil.DeserializeFileAsync` guards the buffer deserialize with a `?? throw`, where the
  pure-C# disk path leaves the property null and carries on. Unreachable on shipped data: all 289
  `database/*.json` files were scanned and none has a bare `null` body. `forceLegacyDatabaseImport`
  covers a hand-edited tree here too.
- **The item base-class and linked-item cache *keys* differ** — legacy stores under `item.Id`
  (`ItemBaseClassService.cs:194,199`; `RagfairLinkedItemService.cs:200`), native under the
  `templateTable.Items` dictionary key. Separable only by a mod filing a template under a key ≠ its
  `_id`, where legacy is the broken arm (consumers resolve by dictionary key).
- **Golden-test parity is normalised, not raw-byte.** Every family has a full-output golden gate
  (`*ParityTests` in `Testing/UnitTests`). Sanctioned gaps: minted `MongoId`s, and for ragfair
  `intId` and `startTime`/`endTime` (one batch timestamp natively vs a per-offer clock in legacy).
- **The resident-DB views-equivalence gate is manual, not part of `dotnet test`.** Proving a
  natively-derived view matches the C#-built override over the real database is a two-step
  harness: an `[Explicit]` NUnit fixture writes the roots envelope and the expected views
  (`RagfairViewsEquivalenceTests`, `QuestViewsEquivalenceTests`), then an `#[ignore]`d Rust
  integration test compares them (`rust/spt-native/tests/phase1_{ragfair,quest}_views.rs`).
  Neither half runs in the gate loop; a flip that silently changes a derivation is caught by the
  parity goldens, not by this. **Flip #6 took the other route** and it is the one to copy:
  `BotResidentDbTests.AResidentSendAndAnOverrideSendProduceIdenticalBotsFieldForField` sends the
  same seeded wave twice — once off the resident DB, once with the C#-built `viewsOverride` — and
  compares the generated bots as normalised JSON. It is a plain `[Test]`, so it *does* run in
  `dotnet test`, and it caught a real derivation divergence (the flip #6 ledger's decision 2)
  that the view-by-view harness would have had to be told to look for.
- **A Harmony patch on `FileUtil.WriteFileAsync`/`DeleteFile` no longer sees profile I/O** —
  since Phase 5 `SaveServer`'s disk boundary is `spt_profile_*`, so a mod intercepting profile
  writes or deletes through `FileUtil` never fires. Patches on `SaveServer`'s own members still
  fire — its signatures are frozen and unchanged by this phase — and `BackupService`'s copies still
  go through C#. **No escape hatch**: Decision 6 shipped no legacy path and no `forceLegacy` flag.
  Not the `SPTLoggerDispatcher` precedent — a logger flip loses log lines and this one could lose
  player saves, so the precedent does not carry the blast radius. The reason it holds anyway is that
  Phase 3 kept `CoreConfig.ForceLegacyDatabaseImport` because its two arms produce *different*
  artifacts (the fused load does assembly, parse and derivation work the legacy arm does not), while
  Phase 5's arms produce the same bytes: on-disk format is byte-identical and the write protocol is a
  step-for-step port, so a revert is a plain `git revert` with no stranded state and no migration. A
  second write path here would rot untested, and a rotted fallback is worse than none. Bites only
  mods that hook profile I/O at the `FileUtil` layer.
- **Profile save and load cancellation is best-effort-before, never mid-flight** — a token is
  checked before the native call and cannot interrupt a started write, where `WriteAsync` could be
  cancelled mid-file. Atomicity is not what changed: `FileUtil.WriteFileAsync` was already
  temp-then-rename (`Utils/FileUtil.cs:113`), so neither arm can leave a truncated live file. What
  changed is the outcome of a cancellation that arrives after the call begins — it used to abandon
  the save, and now the save completes. Alongside it, I/O failures on the profile paths — save, load,
  list and delete alike — throw `InvalidOperationException` out of `SptNative.DecodeResult` where
  they used to throw `IOException`-family types; nothing in the tree catches those specifically, so
  this bites a mod with a `catch (IOException)` around a profile operation. No escape hatch, same
  reason.
- **A `user/profiles/` that cannot be `stat`ped now aborts startup instead of loading zero
  profiles** — `spt_profile_list` raises on any non-`NotFound` `stat` failure, and `SaveServer.LoadAsync`
  is an `IOnLoad`, so the throw reaches `SPTStartupHostedService.ExecuteAsync`'s outer catch and stops
  the server with `Critical exception, stopping server...` plus the path and errno. The most reachable
  cause is a `user/profiles/` that has lost `+x`: 4.1.2 C# booted normally there, because .NET's Unix
  enumerator answers `Directory.GetFiles` from `getdents64`'s `d_type` and never `stat`s a regular
  file, and then `File.Exists` on each child returned `false` — so the player was invited to create a
  new profile beside intact ones. **Deliberate**: a loud stop on a `chmod`-recoverable condition beats
  a silent one that presents as data loss. The blast radius is a startup failure, which is why it is
  here and not only in the Phase 5 ledger.
- **`LoadAsync` sees profiles in sorted order**, where `Directory.GetFiles` order was
  filesystem-dependent. Strictly more deterministic; nothing downstream reads the order.
- **A failure crosses as a message for C# to throw with** — never as a log line, so it carries no
  category. Since ABI 18 a panic crosses with its message too.
- **Hangs are mostly undiagnosable** — ported retry loops can spin exactly as 4.1.2 does, inside an
  FFI call with no managed stack trace. Force legacy to get the managed stack back.
- **`get_flea_prices_as_array` is O(offers × price table) if a mod enables barters.** Dead on
  shipped data (`ragfair.json` `dynamic.barter.chancePercent` is `0`). Latent, not measured.

Unreachable on shipped data, recorded because a mod could reach them: an unknown scav case recipe id
or an ammo pool empty in its rarity band (native returns a message, legacy NREs); a parentless or
cyclic parent chain on the hydrate (native terminates, C# stores `{ MongoId.Empty }` or recurses);
five malformed filter shapes on the linked-item walk (native skips, legacy throws on four and
silently drops camora ammo on the fifth); and the native `_type` test being
`eq_ignore_ascii_case` against C#'s `OrdinalIgnoreCase`. The parity gates would catch any of them.

### Logging

- **Generator lines carry one category per generator** — `typeof(T).FullName` of the C# class each
  Rust module ports, where the replay era logged the whole bot family through
  `ISptLogger<BotInventoryGenerator>`. A `sptLogger.json` filter against that class matches far
  fewer lines now.
- **Generator lines use a different `%tid%` space** — a process-local counter in first-emit order,
  not the managed thread id. `%tname%` is the Rust thread name, usually empty. `%date%` is the
  moment of emission, where replayed lines were stamped at the end of the call.
- **Generator locale text is a startup snapshot** — `DatabaseImporter` pushes resolved server locales
  once (`spt_locales_set`); a mod mutating them later no longer changes generator line text. A
  failed push falls every generator line back to its locale key.
- **Parallel generator lines interleave** — ragfair and bot rayon workers emit as they run, so lines
  no longer arrive grouped per bot or per assort entry.
- **Console output is asynchronous, and the two things riding its queue behave differently** — one
  8192-slot bounded channel to a writer thread carries both rendered log lines and the raw
  `Console.Write*` bytes redirected into it. A burst of log lines deeper than the queue drops; raw
  bytes, and the drain a `spt_console_read_line` does first, block instead, so a prompt is never
  lost. The ceiling that buys: a terminal that stops draining stalls managed `Console.Write` behind
  it and makes `spt_logger_close` wait on the writer thread. Shutdown is carved out of that shared
  order: `spt_logger_close` takes the pipeline out from under the lock before joining the writer
  thread, so a `Console.Write` racing the teardown writes straight to stdout and can interleave with
  the backlog still draining. A hard crash still loses what is queued.
- **Excluded categories still pay the per-line marshaling cost** — filtering moved native-side, so
  every line crosses the boundary before it is dropped.
- **Filter regexes are regex-lite** — no lookarounds, no backreferences, ASCII-only character
  classes. A pattern that will not compile is reported to stderr once and then never matches.
- **A native logging failure has no C# fallback** — a failed `spt_logger_init` means no logging at
  all for the run, and the same for a config the C# parser tolerated but Rust rejects. The known
  cases are handled except the `type` tag of a `loggers` entry, case-sensitive on both sides.
- **The pipeline reads `sptLogger.json` once; runtime mutation needs an explicit reload** — mutating
  `SptLoggerConfiguration.Loggers` changes neither what is written nor, since the gate moved to
  `spt_log_enabled`, what `IsLogEnabled` answers: both now read the *applied* configuration, where
  the C# object used to answer immediately and could disagree with what got written.
  `SPTLoggerDispatcher.ReloadConfiguration()` (additive, post-port) re-hands the object to
  `spt_logger_reinit`; a rejected reload leaves the running pipeline untouched. With no pipeline to
  ask — before init, after close, no library — `IsLogEnabled` still falls back to the C# object.
- **Line terminators are always `\n` and dates always Gregorian, culture-independent** — including
  `BaseLogHandler.FormatMessage` now that it renders through `spt_log_format`. The deleted C#
  `LogTime.ToString("yyyy-MM-dd")` / `("HH:mm:ss.fff")` used `CurrentCulture`, so under a culture
  with a non-`:` time separator (fi-FI) or a non-Gregorian default calendar (th-TH, ar-SA) a mod
  handler stamped a different timestamp from the pipeline line beside it. Nothing pins the process
  culture; this makes the two agree.
- **File rotation was redesigned, not ported** — ZLogger rolled ascending (`spt.1.log` was the
  *next* file); the native sink cascades, so `.1` is the *most recent* archive and `spt.log` only
  ever holds the current run. Anyone comparing `spt.N.log` across the upgrade reads it backwards.
- **Lowering `maxRollingFiles` does not sweep the old high indices** — 10 → 3 strands
  `spt.3.log`..`spt.9.log` until deleted by hand (the `ponytail:` note in `log_sink.rs`'s `cascade`).
- **Mod `ILogHandler` routing goes through a hybrid tap.** The dispatcher fans C#-originated lines
  out to handlers; `spt_log_set_tap`'s callback delivers Rust-originated generator lines as rendered
  text with no `Exception` object. Registration changed shape: resolve `SPTLoggerDispatcher` from DI
  and call `RegisterHandler` (additive, post-port) — a constructor-injected handler set is always
  empty in a real run.
- **The fatal-error pause is "Press enter to exit..." (`spt_console_read_line`)** — the old
  `Console.ReadKey(true)` single keypress is gone; raw-mode stdin was not worth the platform console
  code. The two mod-load-failure pauses (`Program.cs:136` and `:147`) still read with no prompt at
  all: pre-existing, and only conspicuous now that their two siblings print one.
- **`BaseLogHandler.FormatMessage` renders through `spt_log_format`** — a format containing bare `{`
  or `}` now renders literally instead of throwing out of `CompositeFormat.Parse`, a positional hole
  like `{0}` (previously expanded as the date argument) renders literally too, and a handler that
  cannot reach the native side degrades to the unformatted message rather than throwing.
- **A `GetCompiledFormat` override no longer reaches `FormatMessage`** — `BaseLogHandler` hands the
  reference's raw `Format` string to `spt_log_format`, so a mod subclassing `BaseSptLoggerReference`
  to rewrite the compiled format changes nothing about what a handler renders. The method still
  compiles and caches for anyone calling it directly; both shipped reference types are `sealed`, so
  reaching the override at all meant constructing the subclass in code.
- **`Console.Clear()`'s `IsOutputRedirected` guard became a Rust-side tty check** — clear and title
  are ANSI/OSC escapes queued behind the console sink, with VT enabled on Windows at
  `spt_logger_init` (where the Windows title is still a console API call, not an escape).
- **A mod calling `Console.SetOut`/`SetError` — or setting `Console.OutputEncoding`, which does the
  same internally — silently un-wraps `NativeConsoleWriter`** and reverts that process to raw writes
  racing the pipeline on fd 1 (and, on Windows, the mojibake the UTF-8 codepage setup fixed), with
  zero test failures. Before this port such a call was harmless. There is no mechanical guard; the
  only in-tree warning is a comment in `Program.StartServer`.
- **Non-string `Console.WriteLine(value)` overloads can tear** — `TextWriter` decomposes them into
  `Write(value); WriteLine();`, two queue messages, so a log line from another thread can land
  between the value and its newline. Every in-tree call site uses the string overloads; only mod
  output can hit it, and the failure is cosmetic interleaving.

## Guidelines

1. **Frozen surface.** Preserve the ported class's entire 4.1.2 public *and protected* surface —
   constructor including parameter names, methods, DTOs. Enforced by `dotnet apicompat` in the
   sibling `mpex-api-compat` repo. The *surface* is frozen unconditionally; the *body* is not.
   Keep the C# implementation as the legacy path **only where Rust cannot reliably replace it**,
   which holds when either condition fails: (a) both arms produce identical observable output, so
   deleting the C# body strands nothing and a revert is clean, and (b) nothing mod-visible needs
   that body as guideline 2's patch-routing target. Both shipped precedents read this way: Phase 5's
   profile persistence dropped legacy because both arms emit identical bytes and the disk boundary
   is not a hookable algorithm, while Phase 3's database import kept `ForceLegacyDatabaseImport`
   because its two arms produce different artifacts. Every generator family fails (b) — legacy is
   what a detected patch routes to — so this changes nothing for the six of them. A family that
   keeps no legacy path argues it in its ledger.
   **Why the `Native/` payload reshapes never flag it:** the whole tree post-dates the 4.1.2
   baseline, so its members are *additions*, which apicompat does not report — not because they are
   hidden — plenty of them are public (`Native/Loot/LootPayloads.cs`,
   `Native/BaseClass/ItemBaseClassPayloads.cs`, `DbPublisher`). A future flip reshaping one of those
   should expect a clean run for that reason, not from a visibility rule that does not hold.
   The same exemption reaches post-baseline types outside `Native/`, spot-checked at ABI 32:
   `BotWaveBatcher` (`Generators/Bot/`) **lost a constructor parameter** and came back clean —
   `APICompat ran successfully without finding any breaking changes.`, exit 0, no CP0002. That was
   the apicompat tool invoked **directly**, for the single `SPTarkov.Server.Core` assembly against
   the sibling repo's frozen 4.1.2 baseline DLLs; it is not a gate pass and says nothing about the
   other assemblies. The rule is the type's age against the baseline, not the directory it sits in.
   **Second gotcha, worth more than the exit code:** `mpex-api-compat/ci/check-api-compat.sh`
   resolves its local dotnet tool manifest from the *current working directory*, so anywhere cwd
   does not persist between shell invocations (an agent session, most CI shims) the script reports
   every assembly as failed. That looks exactly like the known baseline failure and actually means
   no analysis ran at all — invoke the tool directly when you cannot guarantee cwd.
2. **Override contract.** Detect Harmony patches on the frozen members (`Harmony.GetPatchInfo`) and
   route to legacy so hooks fire with baseline semantics. Add a `forceLegacy...` config flag as the
   escape hatch for hooks detection can't see. A port that kept no legacy path under guideline 1 has
   no routing target, so it carries neither detection nor a flag — the log pipeline and profile
   persistence are the two shipped examples.
3. **Resident DB epoch, publish on dirty.** DB-derived state lives resident on the Rust side:
   `DbPublisher` republishes every supported root when the global `DatabaseMutationStamp` has moved
   and stamps the returned epoch into each request. The published set is **six roots** since Phase 4
   — the templates, traders, globals, locations and hideout tables, plus a `configs` root carrying
   all 28 loaded configs keyed by each one's own `Kind` string. Since Phase 2 the stamp is moved
   primarily by the
   Ceciler-injected write barriers on the model setters reachable from the published roots, with
   eight hand-written bump sites left for the container writes barriers cannot see. Only the varying
   block — per-call **service** state, plus whatever the caller itself selected — and the optional
   `viewsOverride` remain per-call; config state left the varying block in Phase 4. For the bot
   family the "service state" half of that clause is now **vacuous**: ABI 32 took the last cached
   service state off `SharedBotVarying`, leaving `generatingPlayerLevel` and `isNightTime` (live
   `ProfileHelper` / `WeatherHelper` reads, resolved per call by definition), `equipment` (held off
   the resident DB by a runtime writer, not by being a service's cache) and the caller-selected
   `levelGeneration` / `templateVariants`. The other families' service-backed fields (decision 10
   below) are untouched.
   Ineligible callers — mods loaded where `TrustNativeRequestCacheWithMods` does not hold (it defaults
   **on** since Phase 2, and counts only where `WriteBarrier.Installed`, i.e. Release and publish
   builds), or anyone with `DisableNativeRequestCache` — send the C#-built
   view bundle as `viewsOverride` on every call at today's projection cost, never touching resident
   state; since Phase 4 that bundle carries the family's config block too, so the ineligible wire is
   the same size it was. Full protocol: the epoch-protocol section of
   docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md. Ragfair (flip #1), the
   repeatable quests (flip #2), the two startup one-shots — the base-class hydrate and the
   ragfair linked-item table (flip #3) — the loot family — location and reward loot
   (flip #4) — the scav case (flip #5) and the bot family — both exports (flip #6) — ride it
   today. That is **Phase 1 of the state-ownership program complete**: all thirteen generation
   exports read the resident DB, both slice caches are gone, and every family's per-call payload is
   its varying block plus whatever the frozen C# signature forces (a `staticAmmoDist` parameter,
   the caller's filtered bot template). The eligibility rule and the stale-epoch self-heal live once, in
   `Native/Db/ResidentDbDispatch`; a family's own `ResidentDbEligible()` is a one-line wrapper that
   sources the flag pair from its config record.
4. **RNG parity.** Both sides draw through the shared xoshiro256\*\* source behind test-only seams
   (`Utils/RandomSource.cs` / `random_util.rs`), pinned by twin known-answer tests. Production C#
   randomness stays bit-for-bit unchanged.
5. **FFI envelopes are internal.** Request/response types are a C#↔Rust contract shipped in lockstep
   — change them freely, bump `spt_native_abi_version` and `SptNative.ExpectedAbiVersion` together.
   The in-tree assertion in `ffi.rs`'s `abi_version_export_matches_crate_const` is the third site and
   must move with them. No third-party consumer of the cdylib is supported.
6. **Ports keep an `[Injectable]` entry point.** A static wrapper like `SptNative` is only acceptable
   for startup-internal subsystems mods never touch. Anything patchable calls Rust from inside a
   resolved service.
7. **Gate loop** (no CI in this fork): `dotnet build -c Release` → `mpex-api-compat/ci/check-api-compat.sh`
   → `dotnet test` → `csharpier format .` → `cd rust && cargo test && cargo fmt --check &&
   cargo clippy --all-targets -- -D warnings`.
   **Gotcha:** run `dotnet tool restore --tool-manifest <mpex-api-compat>/.config/dotnet-tools.json`
   first and invoke the script with the working directory *inside* `mpex-api-compat` — a missing
   `apicompat` tool falsely reports "API COMPATIBILITY BROKEN".

### Exceptions in force

**Constructors.** Every family took an additive overload, never a signature change: `LootGenerator`
adds `LocationConfig` (extended in place — it post-dates the baseline — with the loaded-mod list
+ `DbPublisher` in flip #4, when `LocationLootGenerator` took an additive overload adding the
same pair), the four quest generators add `QuestConfig` +
`RepeatableQuestNativeRequestBuilder`, `ScavCaseRewardGenerator` adds `ScavCaseNativeRequestBuilder`
(and, since flip #5, a further overload adding the loaded-mod list + `DbPublisher` — the pair loot
took, chained through the earlier one),
`ItemBaseClassService` adds `ItemBaseClassNativeRequestBuilder` + `ItemConfig`,
`RagfairLinkedItemService` adds `RagfairLinkedItemNativeRequestBuilder` + `RagfairConfig`, and
(flip #6) `BotInventoryGenerator` adds the loaded-mod list + `DbPublisher`, chaining its frozen
4.1.2 primary constructor. The
container selects the overload; anything built through the frozen 4.1.2 constructor gets a null
builder — or, on the resident-DB families, a null `DbPublisher`, which `ResidentDbDispatch.Eligible`
answers `false` to — and runs legacy or the override arm unconditionally. `BotWaveBatcher` needed
no overload: it post-dates the baseline, so its primary constructor simply took the pair. Ragfair
offer generation, `RepeatableQuestRewardGenerator` and `RepeatableQuestHelper` needed no change at
all.

**Config flags.** `LocationConfig.ForceLegacyLootGeneration` covers *both* loot generators — there
is no per-generator flag. Elsewhere: `BotConfig.ForceLegacyBotGeneration` and `ForcePerBotGeneration`,
`RagfairConfig.ForceLegacyRagfairGeneration` and `ForceLegacyRagfairLinkedItemBuild`,
`QuestConfig.ForceLegacyRepeatableQuestGeneration`,
`ScavCaseConfig.ForceLegacyScavCaseGeneration`, `ItemConfig.ForceLegacyItemBaseClassHydration`,
(since Phase 3) `CoreConfig.ForceLegacyDatabaseImport` — the one flag that is not a generation
path, restoring the pure-C# verify-then-walk database import — plus
`TrustNativeRequestCacheWithMods` / `DisableNativeRequestCache`, which carry the resident-DB
eligibility gate and exist on `RagfairConfig`, `QuestConfig`, (since flip #3) `ItemConfig`,
(since flip #4) `LocationConfig`, whose pair covers both loot generators, (since flip #5)
`ScavCaseConfig` and (since flip #6) `BotConfig`, whose pair covers both bot exports;
the linked-item table reads `RagfairConfig`'s pair rather than gaining its own. Only
`forceLegacyLootGeneration` is serialised into a shipped `.json` (`location.json`); the rest exist
as C# defaults and a user who wants to change one adds it to the file — note that since Phase 2
`TrustNativeRequestCacheWithMods` defaults **on** in all six configs, so it is the flag a user adds
to turn *off*.

**What flips to legacy.** Loot flips only on a *protected* member patch. Every other family flips on
a patch of any public/protected/protected-internal member of its frozen set, **except** the
dispatcher entry point itself — a patch there wraps whichever path runs, by design. Frozen sets:
bots, the four generator classes; ragfair, `RagfairOfferGenerator`, `RagfairPriceService`,
`RagfairServerHelper`, `RagfairAssortGenerator`; quests, the four `*QuestGenerator`s plus
`RepeatableQuestRewardGenerator` and `RepeatableQuestHelper`; scav case, base class and the
linked-item table, their own class only. A container-substituted subclass also flips — **except
loot**, whose `UseLegacyPath` ends at the patch scan and checks no substitution at all, so a
`LootGenerator`/`LocationLootGenerator` subclass registered at a higher `TypePriority` still runs
native. The families with a one-class frozen set (scav case, base class, linked-item table) check
substitution of that class; the multi-class ones check their collaborators too. Bots additionally
flip on an `InventoryMagGenComponents` set that isn't exactly the four built-ins. `PickupQuestGenerator` contributes **zero** frozen
hookable members — its whole legacy body is inline in `Generate`.

**The bot wave batches before it iterates.** `BotController.GenerateBotWave` offers the wave to
`BotWaveBatcher.TryGenerateWave` first; the batcher returns null — and the unchanged per-bot path
runs — on `ForcePerBotGeneration`, on anything `BotInventoryGenerator.UseLegacyPath()` already
catches, on a patch of any frozen `BotGenerator`/`BotLevelGenerator`/`BotEquipmentFilterService`/
`BotController` member except `GenerateBotWave`, on a substituted `BotGenerator`,
`BotLevelGenerator` or `BotEquipmentFilterService`, or on a wave that could write nighttime clamps
(only the per-bot path replays those). The response is one `{result | error}` envelope per bot in
request order (ABI 8): a failed bot is skipped with a Critical log and the rest of the wave still
generates.

**The wave's level draw is native and its template ships per level band, not per bot** (ABI 22).
`BotLevelGenerator.GenerateBotLevel` + `ChooseBotLevel` are ported to `bot/level_generator.rs` with
**no new export** — the draw is the first act of each bot's rayon task, ahead of every other seeded
draw, exactly where the C# prelude does it, and the drawn `level`/`exp` ride back on the envelope
for the caller to write into `details.BotLevel`/`Info.Level`/`Info.Experience` before `CacheBot`
reads them. `GetRelativePmcBotLevelRange` stays C#-side: its inputs are wave-constant, so the
batcher calls it once and ships the range as `levelGeneration` — PMC waves only;
non-PMC takes the constant level 1 and draws nothing, which is what keeps non-PMC seeded pins
byte-identical). Because every level-dependent *pre-call* step is a *pure band lookup* that draws
nothing — `FilterBotEquipment` (whose `Clothing` weighting adjustment is also what reshapes the
appearance and voice pools, so they are not a separate lookup) and the `LootItemLimitsRub` price
bands — the batcher splits the range at those bands' edges and runs the **unchanged C#** filter,
seasonal strip, blacklist strip and pool hydration once per band — shipping one `templateVariants`
entry per band instead of one filtered template per bot. Segments are typically 1-3 (up to ~8 for a
full 1..79 range on shipped config), and always exactly one `[1..1]` for a non-PMC or playerscav
wave. The per-bot slice collapses to `botId` + `testSeed` + `details`. Since flip #6 the whole
request is `{epoch, viewsOverride?, shared, bots[]}` (single-bot: `{epoch, viewsOverride?, shared,
bot, template, lootPools}`): the database half is resident and only the varying `shared` block, the
slices and the caller's filtered templates cross per call — 13,341 bytes per bot at wave 45 against
the override arm's 93,758 (BENCHMARK.md). The voice and appearance
*draws* move **after** the call, onto the band the drawn level lands in. The decline set grows two
member-scoped entries for the seasonal strip (`SeasonalEventService.ChristmasEventEnabled` and
`RemoveChristmasItemsFromBotInventory`, which now run per band) — member-scoped rather than
whole-type so a seasonal mod patching unrelated event surface does not de-batch the server. One
deliberate carve-out from "decline whenever a mod could observe the difference": pool and price
hydration — `BotLootCacheService.GetLootFromCache` (12 calls) and `HandbookHelper.GetTemplatePrice`
also run once per band and are **not** in the decline set, because economy mods patch them constantly
and declining there would de-batch most modded servers. Divergences: **none intended.** The one
fidelity note is `AddAdditionalPocketLootWeightsForUnheardBot`, which the native side applies to the
cloned variant template with an `if let` where C# dereferences `PocketLoot` unguarded — a
template with no `pocketLoot` block NREs on the per-bot path and is a no-op here (documented at the
port site in `bot_inventory_generator.rs`). A PMC batch bot gains 1-2 draws at the head of its
stream by construction, so a PMC seeded pin repinning is expected and is *not* a divergence — none
had to be, as it happens: every batch-vs-per-bot fixture is non-PMC, and no pinned value in the
suite moved. A changed **non-PMC** pin would be a bug. Measurements in
[BENCHMARK.md](BENCHMARK.md).

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
the typed path instead, which is slower than both the raw path and the C# it replaced. A mod can
therefore put a server on the slow path without saying so. Since flip #4 the fork lives inside the
request's varying block (`varying.looseLoot`) on both the resident and the override arm — loose
loot is the one loot input that never went resident, by decision: raw bytes resident would cost
549 MiB RSS (BENCHMARK.md § Phase 0), so the per-call splice survives — Phase 3 was the named
revisit point and **declined it again** for the same number (the Phase 3 ledger's decision 1);
resident paths plus an on-demand read is the upgrade if it is ever wanted.

**The ragfair batch walk is parallel only when unseeded.** An unseeded walk fans across rayon: a
forked `RagfairContext` per assort entry, merged back in assort order with `intId` reassigned during
the merge. A **seeded** walk stays sequential (the seeded RNG is `thread_local`) and every
`RagfairParityTests` case sets a seed, so parity rides the unchanged path. Production is unseeded on
both arms.

**The ragfair response is a framed MessagePack envelope, not a JSON buffer** — one length-prefixed
frame per offer behind a header frame (since ABI 10, encoding tag 1), deserialised with
`Parallel.For` straight out of the native buffer. Ragfair is the only export that uses it. Its batch
also takes **one timestamp** where legacy calls `TimeUtil.GetTimeStamp()` per offer.

**Every ported generation family reads the resident DB — the departures from per-call projection
(guideline 3).** All key freshness on the same `DatabaseMutationStamp`, a
monotonic counter moved since Phase 2 by the Ceciler-injected setter barriers over every model type
reachable from the published roots — five tables, and since Phase 4 the 28 configs as well — plus
eight hand-written sites for container writes the
barriers cannot see: `SeasonalEventService.UpdateGlobalEvents`, `ItemFilterService`'s two blacklist
`Add*` methods, `CustomItemService`'s two `Create*` methods, `CustomQuestService.CreateQuest`,
`RagfairPriceService.ReplaceFleaBasePrices` and a guarded replay bump when
`CanSellOnRagfair` flips true→false. **Ragfair (flip #1, ABI 22) and quests (flip #2, ABI 23)
share one protocol:** `DbPublisher` publishes the templates, traders, globals and locations roots
into the resident store (`rust/spt-native/src/db.rs`), which derives both families' views at
publish time; each request arrives as `{epoch, viewsOverride?, varying}`, and an epoch the store
does not hold returns `STATUS_STALE_EPOCH` (4), surfacing as `NativeStaleEpochException` and
self-healing with one `ForcePublish` + retry — a lost epoch costs one republish, never a wrong
result. **Flip #3 (ABI 24) put both startup one-shots — the base-class hydrate and the ragfair
linked-item table — on the same protocol**: no varying block, no new roots; their resident walk
inputs derive from the templates root at request time, deliberately not from the ragfair items
view (props-less drop, first-filter-group-only). **Flip #4 (ABI 25) put all six loot exports —
location loot's two and reward loot's four — on the same envelope**: the three statics
(`staticLoot`, `staticContainers`, `statics`, ~19 MB across all maps) became typed lifts on the
existing locations-root entries, serialized at publish from each `LazyLoad.Value` so registered
transformers are applied; the preset views ride `RagfairDbViews`, which gained its one flip-#4
derivation, `default_presets_by_tpl_key` (forced loot). looseLoot stays a per-call splice and
`staticAmmoDist` stays a parameter — the flip #4 ledger has both decisions. **Flip #5 (ABI 26)
put the scav case export on the same envelope**: a new `hideout` root — `production.scavRecipes`
only, the locations-root partial-projection precedent — went resident; the recipe views derive
from it at request time rather than at publish (flip-#3 precedent, preserving the C#
skip-a-malformed-recipe semantics bug-for-bug), and `itemsView`/`staticPrices`/
`defaultPresetsByTpl` borrow the already-resident ragfair views. **Phase 4 (ABI 30) added the sixth
root and closed the config half of every family's carve-out**: all 28 configs publish keyed by their
`Kind`, and the six config-reading families resolve their blocks off the root per call — quest's
`repeatableQuestTemplateIds`/`locationIdMap` off `spt-quest` and its `rewardItemBlacklist`/`bossItems`
off `spt-item`; reward loot's four `ItemConfig` sets off `spt-item`; ragfair's `dynamic` off
`spt-ragfair`, `configBlacklist` off `spt-item` and `customMoneyTpls` off `spt-inventory`; location
loot's `config` off `spt-location` and its christmas container ids off `spt-seasonalevents`; scav
case's whole config block off `spt-scavcase`; and eleven of the bot family's twelve config lifts off
`spt-bot`/`spt-pmc`/`spt-repair`. What still rides the varying block is **service state**, plus the
handful of inputs the caller itself selected or C# has to resolve per call — the Phase 4 ledger has
the residual set and why each one has no resident home. **Flip #6 (ABI 27) put both bot exports —
the single bot and the batched wave — on the same envelope**, closing Phase 1: no new root,
`BotDbViews` deriving at publish from the templates and globals roots once the ragfair views are
resident (it embeds `RagfairDbViews` by `Arc`, the quest views' precedent, and adds only `defaultPresetIdsByTpl` and
`expTable`). The pre-flip `SharedBotViewsWire` split in two — the database half became
`viewsOverride` on the ineligible arm, the rest was renamed `SharedBotVarying` — and the bot
family's varying carve-out was `modPoolSlotOrder` plus the config and service-backed blocks
(`equipment`, `bosses`, `durability`, the two equipment blacklists, `configBlacklist`, the
`pmcConfig`/`repairKitWeapon` lifts); Phase 4 cut that to `modPoolSlotOrder` and `equipment` alone,
the two equipment blacklists having left the wire entirely, and ABI 32 cut it again to `equipment`
alone when Rust took the mod pools. Each bot's `template` and `lootPools`
stay caller-supplied on both arms — the batch ships them per level band — because the filtered
template is the caller's own product, not a database view.
**A mod writing an injected table's dictionaries directly is still invisible to the stamp** — its
*scalar* writes no longer are, since Phase 2's write barriers, so the eligibility gate no longer
carries that weight: `TrustNativeRequestCacheWithMods` defaults **on**, a modded server rides
resident state, and the flag is honoured only where `WriteBarrier.Installed` — Ceciler runs on
Release and publish, never Debug. `DisableNativeRequestCache` remains the kill switch, and the
container gap plus the rest of the barriers' blind spots are the *Broken* ledger's
container-mutations bullet; ineligible callers send a per-call `viewsOverride` with `epoch: 0`
instead — a documented wire contract, not runtime-enforced (guideline 3). The rule itself and the one
stale-epoch retry are `Native/Db/ResidentDbDispatch`'s two methods, which flip #6 extracted from
the seven copies that had accumulated; every family calls them now, across nine sites.

**Flip #1 ledger.** (a) Helper-cache freshness: legacy's hydrate-once caches — `TraderHelper`'s
trader prices, `HandbookHelper`'s handbook price lookup, `PresetHelper`'s preset store and
default-preset maps — could serve stale values into a rebuilt slice; Rust re-derives every view
from the published roots on each publish, so the resident path is uniformly *fresher*, never
staler, after runtime mutations — favours correctness, recorded here rather than "fixed". The
practical edge: a resident send and a `viewsOverride` send can diverge after a runtime mutation,
because the override is still built through those hydrate-once caches. (b) pmc name lists stay
C#-projected in the varying block; flip #6 (bots
root resident) is the named revisit point — **closed by flip #6's decision 1: no bots root**, the
lists stay varying, deferred to Phase 4, because the filter reads a config value and the root's
~94.6 ms of per-publish projection would serve nothing else. **Phase 4 answered that deferral
declined again** (its decision 10): the configs root supplies the config half, but
`GatherPmcNamesOfLength` still reads the bot *table*, which has no root, so residency would need the
root flip #6 refused. A names-only mini-root is the standing upgrade if the varying cost ever
measures. (c) Runtime *config* edits still bypass the
stamp — the pre-flip ceiling, **closed by Phase 4** for scalar edits: the configs root is published
and the write barriers walk all 28 config types, so a runtime property set on a config now moves the
stamp like a table write. What is left of the ceiling is collection edits, which is the *Broken*
ledger's container-mutations bullet, not a ragfair-specific hole. (d) An
`_items: []` preset added at runtime by a trusted mod (stamp bumped) now aborts the publish loudly
on every eligible pass, naming the preset (`views.rs`'s `build_preset_cache`), where the old slice
path tolerated it — the startup `PresetCache` never saw it and the live `itemPresets` view carried
it harmlessly; stricter and louder, deliberately. Net `Native/`
delta for the flip (`git diff --stat da556e7..0287c7e -- Libraries/SPTarkov.Server.Core/Native/`):
+214/−70 lines across 7 files — the invariant half of the ragfair builder is gone, and `Db/`
(`DbPublisher` + `DbPayloadProjection`, 105 lines) is new shared infrastructure every later flip
reuses.

**Flip #2 ledger.** (a) Freshness delta: pre-flip, the quest slice was rebuilt from the live
tables whenever a send included it, so every send that carried the slice saw all mutations up to
that moment; post-flip an *un-stamped* table mutation is invisible to the quest path until the
next stamped publish. Stamped mutations are unchanged — the next publish picks them up, exactly
as before. (b) The quest views share `items`/`handbookPrices`/`fleaPrices` with the ragfair views
through one `Arc` (identical C# helper semantics, independently verified);
`defaultWeaponPresets`, `defaultPresetOrItemPrices`, the repeatableQuests lifts and the two
location maps are quest-own derivations at publish. (c) The `locations` root added for this flip
is `Base` + `AllExtracts` only (`AllExtracts` is a sibling of `Base` on the C# `Location`), keyed
by the locations' `JsonPropertyName` strings (e.g. `factory4_day`) and domain-bounded by
`LocationTable.GetDictionary()`; a null `AllExtracts` ships as `[]`. Net `Native/` delta for the
flip (`git diff --stat 1360a28..e7c2852 -- Libraries/SPTarkov.Server.Core/Native/`): +175/−103
lines across 5 files — the quest builder's stamp machinery and invariant-slice half are gone,
replaced by the four-root publish through the `Db/` infrastructure flip #1 built.

**Flip #3 ledger.** (a) Freshness delta: pre-flip, every hydrate/rebuild projected the live
tables at call time; post-flip an eligible hydrate — including `GetLinkedItems`' rebuild on a
cache miss mid-run (`RagfairLinkedItemService.cs:126-133`) — reads last-published state, so an
un-stamped table mutation is invisible to it until the next stamped publish. Override sends are
unchanged: the ineligible arm still projects the live tables per call. (b) The walk-input
equivalence handshake (`OneShotViewsEquivalenceTests` / `flip3_oneshot_views.rs`) ran green over
the full real database: 4,553 base-class chains and 4,673 linked-item sets identical between the
resident-derived and C#-built override inputs. (c) Net `Native/` delta for the flip
(`git diff --stat 80ce2ae..4f66860 -- Libraries/SPTarkov.Server.Core/Native/`): +207/−1 lines
across 5 files — growth, not the usual shrinkage, because the one-shots' whole pre-flip payload
*was* the projection, which survives intact as the ineligible `viewsOverride` arm; what was added
is each builder's eligibility gate + epoch request assembly and the additive internal wrappers in
`SptNative.cs`.

**Flip #4 ledger.** (a) Freshness delta: statics now refresh on publish, not per call — the
publish serializes each `LazyLoad.Value`, so a transformer registered *after* the last stamped
publish (registering one bumps no stamp) is invisible to eligible sends until the next stamp
bump, where pre-flip every call read `LazyLoad.Value` fresh; the kill switches
(`DisableNativeRequestCache`, `ForceLegacyLootGeneration`) cover it. (b) looseLoot stays
per-call on both arms — resident raw bytes would cost 549 MiB RSS on top of the measured
405.2 MiB publish delta, for a payload read once per raid start that already rides a zero-copy
`WriteRawValue` splice; residency was deferred to Phase 3 (this plan's call, per the spec's
decision register — overriding the spec's older raw-bytes-resident flip-order prose), **where it
was declined outright** on the same number — the Phase 3 ledger's decision 1. `staticAmmoDist` is
permanently varying: it is a parameter of the frozen public `GenerateStaticContainers`/
`GenerateDynamicLoot` signatures, so the resident DB must never stand in for it. And the
`GetDefaultPresetsByTplKey` duplicate-first-item-tpl case now aborts the publish loudly naming
the culprit preset, where pre-flip C# threw `ArgumentException` per forced-loot call —
spec-sanctioned strictness, same shape as flip #1's `_items: []` abort. One per-call saving
landed as a side effect: sealed's resident arm no longer builds `presetsByTpl` at all.
(c) Net `Native/` delta for the flip
(`git diff --stat be23393..f6c40fa -- Libraries/SPTarkov.Server.Core/Native/`): +201/−51 lines
across 5 files — growth like flip #3, because loot's whole pre-flip payload *was* the per-call
projection, which survives intact as the ineligible `viewsOverride` arm; what was added is the
six `{epoch, viewsOverride?, varying}` envelopes with their per-export varying records and the
statics projection in `DbPayloadProjection`.

**Flip #5 ledger.** (a) Freshness delta — a direction flip: pre-flip the native scav case
re-derived its item and price pools per call, so it was *fresher than legacy* for runtime-added
items (legacy's `DbItemsCache`/`DbAmmoItemsCache` effectively never invalidate — the bullet
under *Broken / known divergences*); post-flip the eligible path reads last-published state,
fresh-per-publish, and the kill switches (`DisableNativeRequestCache`, mods loaded without trust)
restore per-call freshness. Presets moved the same way flip #4's did: the eligible path reads the
globals-derived `defaultPresetsByTpl` instead of `PresetHelper` per call. (b) The `hideout` root
added for this flip carries `production.scavRecipes` only (the locations-root partial-projection
precedent), at the real table path's `JsonPropertyName`s; no view derives from it at publish —
the recipe views derive at request time, a filter over a handful of recipes that preserves the C#
skip-a-recipe-missing-`endProducts`-or-a-band semantics bug-for-bug, where a publish-time derive
would have to abort loudly. The raw root pins the capitalized `Common`/`Rare`/`Superrare` wire
names (`HideoutProduction.cs`); the request-time derivation maps them onto the existing lowercase
`ScavRecipeView` — zero generator-algorithm change. Eligibility + branch + stale-retry sit in
`ScavCaseRewardGenerator` mirroring `LootGenerator` exactly (nullable additive `DbPublisher?` +
loaded-mod list via a new DI ctor chaining the old one, the frozen 4.1.2 ctor untouched; private
`ResidentDbEligible`, where loot's is internal — the benchmark seam here needs only `Build`).
The varying carve-out at the flip was `recipeId`, `config` and four sets — two of them service-backed
(`inactiveSeasonalItems`, `globalBlacklist`), two config-backed (`rewardItemBlacklist`, `bossItems`),
a distinction the flip did not need to draw and Phase 4 did: it took `config` and the two
config-backed lists resident, leaving `recipeId` (caller-selected) and the two service-backed sets.
The pre-flip hydration sweep found
readers only (`ScavCaseRewardGenerator`, `HideoutController`) and no lazy writer into
`Production.ScavRecipes`, so no `DbPublisher` pre-touch carve-out was needed. (c) Net `Native/`
delta for the flip (`git diff --stat a9a224f..ed0144d -- Libraries/SPTarkov.Server.Core/Native/`):
+110/−30 lines across 5 files — growth like flips #3/#4, because the pre-flip payload *was* the
per-call projection, which survives intact: `BuildViewsOverride()` is the ineligible arm, and
`Build(recipeId, testSeed)` survives as a three-line epoch-0 composition for the public
benchmark seam; what was added is the `{epoch, viewsOverride?, varying}` split and the hideout
projection in `DbPayloadProjection`.

**Flip #6 ledger.** (a) Freshness delta: pre-flip every bot send projected the live tables, so an
eligible bot read every mutation up to that moment; post-flip its database half — the items view,
`itemPresets`, `defaultPresetIdsByTpl` and the exp table — is last-published state, so an
un-stamped table mutation is invisible to it until the next stamped publish. The flip #4/#5 class
exactly; the kill switches (`DisableNativeRequestCache`, mods loaded without trust,
`ForceLegacyBotGeneration`) restore per-call freshness. Handbook prices moved the same way and
carry one reachable-only-by-a-mod edge: the eligible arm reads `RagfairDbViews.handbookPrices`,
which is keyed off the **items table**, where the override arm calls
`HandbookHelper.GetTemplatePrice` per drawn tpl — so a tpl priced in the handbook but absent from
the items table prices at its handbook value on the override arm and at 0 on the resident one.
Unreachable for generatable loot: a tpl that is not in the items table cannot be in a
`BotLootCache` pool to begin with. **The slot-order freshness question the plan expected here does
not arise** — see decision 2: it never became a view. (b) Decisions. **1, no bots root** —
flip #1's ledger (b) named this flip as the revisit point for the pmc name lists, and the answer is
no. They are the only consumer a `bots` root would have: the bot family reads no bot templates
resident (the wave's filtered `template` is the C# caller's own product, shipped per level band),
so the root would carry 5.7 MiB and ~94.6 ms of warm projection on *every* publish — ~13% on the
measured ~735 ms — to serve two name lists on ragfair's varying block. And it would not finish the
job either way: `GatherPmcNamesOfLength` filters on `botConfig.BotNameLengthLimit`, a config value,
so the derivation cannot go resident before Phase 4. Deferred there, with flip #1's revisit note
closed as answered — and **Phase 4 declined it a second time** (its decision 10): the config half is
resident now, but the names still come off the bot table, so the deferral only ever moved half the
blocker and the other half is the root this flip refused. **2, `modPoolSlotOrder` is not a view.**
The plan had it deriving
into `BotDbViews` at publish; Task 5's field-for-field resident-vs-override identity test caught
the divergence the plan itself ranked first, and the root cause is not port drift: the C# order is
the enumeration order of the live `BotEquipmentModPoolService`'s `ConcurrentDictionary`, which is
process-local (bucket layout, `ProcessorCount`-dependent growth) and not a function of the database
at all. The Rust derivation was deleted and the field moved into `SharedBotVarying`/
`SharedBotVaryingWire`, where it rode the per-call varying block on **both** arms at 26,428 bytes
(BENCHMARK.md) under the spec's standing service-backed carve-out — but *not* the same class as
ragfair's config-derived fields or quest's config-backed sets, which Phase 4 took resident while
this one had no resident home at all (roadmap item 4 was its only exit, and took it at ABI 32: the
member left the wire rather than being re-homed). The claim that followed here
— that it is the largest single member still crossing per bot — was wrong when written and stayed
wrong: `equipment` is 39,811 B against its 26,428 B, off a projection neither flip touched, and both
are dwarfed by the caller's own `templateVariants` (BENCHMARK.md § Phase 4).
**3, `BotDbViews` as built**:
`{ragfair: Arc<RagfairDbViews>, defaultPresetIdsByTpl, expTable}` — the items and preset views ride
in through the shared `Arc` rather than a second derivation (the `QuestDbViews` embedding), and the
two bot-own members are a re-key of `defaultPresetsByTpl` to each preset's own id
(`ToDefaultPresetIds`) and `globals.config.exp.level.expTable[].exp`, lifted out of `BotWaveBatcher`
where the batch used to project it per wave. The derivation is total and `Result`-shaped so a
future hard failure aborts the publish the way ragfair's does. **4, the handbook-price union
stays the override arm's shape.** `BuildViewsOverride` prices the union of every loot pool the
send can draw from (one cache single-bot, one per level band batched) rather than the whole
handbook, which is collision-safe because a tpl in two pools resolves to the same
`GetTemplatePrice`; the eligible arm reads the resident items-keyed map instead and needs no union
at all. **5, one envelope for both exports.** Single-bot and batch now share
`{epoch, viewsOverride?, …}` and one `resolve_bot_views` resolver returning `LootEpochError`, so
`STATUS_STALE_EPOCH` and the one-shot self-heal behave identically on both; `SharedBotViewsWire`
was renamed `SharedBotVaryingWire` to stop the name claiming it carries views. **6, the two-arm
dispatch block stays copied — evaluated and declined.** The eligible/ineligible `if/else` that
follows `ResidentDbDispatch.Eligible` is now at its 5th/6th copy, so the fifth-copy rule was applied
and the answer is no: that rule targets *identical* blocks, and across the 11 sites the block takes
~6 distinct shapes — per-export `ViewsOverride` expressions, a `bool viewsOverride` parameter, a
`.Result` unwrap, early-return one-shots with no varying block at all, and bots' mutate-the-request
form. A shared helper would have to be generic over the request type, take two builder closures, and
take a delegate to set each site's private-set `LastSendIncludedViewsOverride` — a 5-parameter
abstraction replacing a 12-line `if/else` whose only genuinely duplicated line is the flag
assignment. Commit 1 already extracted the part that *was* identical (the eligibility rule and the
stale-epoch retry) into `ResidentDbDispatch`; what is left is shape, not duplication. Revisit only if
a future flip makes the arms converge. (c) Net `Native/`
delta for the flip (`git diff --stat 3dc37e1..9011794 -- Libraries/SPTarkov.Server.Core/Native/`):
**+227/−285 lines across 7 files** — the program's **first genuine shrink**. Flips #3-#5 grew
because their whole pre-flip payload survived as the `viewsOverride` arm; bots shrank anyway
because two things landed at once. `BotPayloads.cs` alone is +88/−151 — the wire types collapsed
onto the shared envelope — and the seven copied eligibility/dispatch blocks became
`Native/Db/ResidentDbDispatch` (38 lines, +38/−0), of which the three inside `Native/` gave back
+11/−18, +11/−18 and +10/−18 (`ItemBaseClassNativeRequestBuilder`,
`RagfairLinkedItemNativeRequestBuilder`, `RepeatableQuestNativeRequestBuilder`); the other four
copies live in `Generators/` and shrank outside this path. Both flip-#5 review carryovers are
discharged there: the dispatch-block extraction was the fifth copy triggering it, and the
stale-epoch retry that had never been exercised end to end now has a scav case self-heal test
(`ScavCaseResidentDbTests`).

**Phase 3 ledger.** `spt_db_load` (ABI 29) fuses the `checks.dat` hash walk with reading
`database/`: one walk hashes (when verifying — Debug ships no `checks.dat` and asks for none),
reads and installs the five resident roots as epoch 1, then hands the
eager file bytes back for `ImporterUtil`'s reflection walk to materialize `DatabaseTables` from.
`CoreConfig.ForceLegacyDatabaseImport` restores the pure-C# verify-then-walk arm.
**Measured, the flip is a regression, not a speed-up.** At the importer it reads **935.7 ms against
legacy's 480.6 ms — 0.51x, roughly 1.9x slower, +419–455 ms** (BENCHMARK.md § Phase 3), with
202 files / 49.4 MiB of eager content crossing as buffers. The deliverable is the retired startup
double-read, and against a warm page cache the read it retires was nearly free while the buffer
plumbing is not: the buffer-fed walk is 50–68 ms *slower* than the disk walk it replaces (451.1
against 383.8 ms), and the fused load costs ~380–391 ms over the bare verify (484.6 against
96.8 ms) for buffer retention, the FFI copy and the five-root assembly, parse and derivation —
work the legacy arm does not do at import time at all. Neither figure is a startup total, and the
gap the flip was built to close is **not wired up**: `DbPublisher.EnsureCurrent` still republishes
every root — six of them since Phase 4 — whenever its own `_currentEpoch` is 0, and nothing feeds
`DbLoad`'s installed epoch
into it, so both arms pay a 730–745 ms forced publish that epoch 1 already did the work for.
Feeding that epoch through is the named follow-up, and it buys less than it looks:
`EnsureCurrent` republishes when `_currentEpoch == 0` **or** `_lastPublishedStamp != stamp`
(`DbPublisher.cs:40`), and on Release the write barriers move the stamp during `PostDbLoadService`
— `AdjustLocationBotValues` writes `map.Base.BotMax`/`BotStart` for every `maxBotCap` entry that
resolves to a map (12 of the 13 shipped entries; `"default"` has no `LocationTable` property, and
`PostDbLoadService.cs:624-627` skips what does not resolve), and `LocationBase`'s setters bump
under Ceciler (`WriteBarrierCoverageTests.AWriteIntoTheLocationsRootBumps`) — so wiring epoch 1
in would skip that first republish only in the case where nothing dirtied the tables between the
load and the first eligible call. It is **Phase 4/5 work, not a Phase 3 defect** — decision 3 below
is why it was deliberately left out of this phase. **Phase 4 did not wire it either, and moved the
goalposts:** `spt_db_load` stays `database/`-scoped by that phase's decision 3, so epoch 1 now
installs five of the six published roots and carries no `configs` root at all. Skipping the first
`EnsureCurrent` on the strength of epoch 1 would hand every config-reading family a resolve failure,
so the follow-up now has to publish configs at load time — from the live C# objects, not from
`configs/*.json`, which is the values-not-keys trap that decision named — or keep the first
republish.

(a) Freshness delta: **none at generation time.** `DbPublisher` is untouched, and epoch 1 is
boot-validation only — always superseded by the first `EnsureCurrent` republish, so no generation
path ever reads it. The legacy import path survives behind `forceLegacyDatabaseImport`.

(b) Decisions. **1, loose-loot residency declined.** The spec's Phase 3 prose had Rust hold the
per-map raw bytes resident and serve them to the C# `LazyLoad` over a small export; 549 MiB on top
of the measured 405.2 MiB publish RSS delta (BENCHMARK.md § Phase 0) is 954.2 MiB, which leaves no
headroom under the ~1 GB line Phase 0's RSS gate drew, so the per-call splice stays and that
byte-serving export was **not built**.
`locations/*/looseLoot.json` and `locales/global/*` are classified never-read by the fused walk and
stay disk-path `LazyLoad`s on both arms; `staticLoot.json`/`staticContainers.json` are read for
resident-root assembly but not returned. Resident *paths* plus an on-demand read is the named
upgrade. **2, per-file buffer handoff, not per-root.** The spec said "per-root raw JSON buffers"
*replacing* `ImporterUtil.LoadRecursiveAsync`'s tree walk; as built the C# reflection walk stays and
remains the file→property mapping authority, and Rust owns the file→wire mapping for the five
resident roots only — the returned map is keyed `database/…` on both sides and consumed inside
`DeserializeFileAsync`. Reproducing `LoadRecursiveAsync`'s mapping semantics exactly was this
phase's named risk (a mismatch is a startup-data bug, not a perf bug), and keeping the C# walk
neutralizes it structurally rather than by testing for it. The one semantic Rust duplicates from C#
— the importer skip lists and the lazy patterns — fails benign in both drift directions: an extra
returned file is ignored dead weight, a missing one falls back to a disk read inside
`DeserializeFileAsync`. **3, epoch-1 assembly is validated by parse + derive + the real-tree
integration test** (`rust/spt-native/tests/phase3_db_load.rs`), not by a C#-envelope equivalence
harness. It can be, because epoch 1 is generation-invisible until 6b: the first `EnsureCurrent`
republish supersedes it, which is also what guarantees generation runs off post-`PostDbLoadService`
state rather than off raw disk bytes. That is the ceiling this flip accepts, recorded here — and it
is why the regression above cannot be closed by simply skipping the republish. **The hazard that
ceiling hides, for whoever wires the load-time epoch through:** `classify` (`load.rs:74`) and
`LOCATION_MEMBERS` (`load.rs:18`) are a second, independent file→wire mapping duplicating
`DbPayloadProjection`, and **nothing gates it** — `DatabaseLoadEquivalenceTests` compares
`DatabaseTables`, never the resident roots. What makes that safe today is only that epoch 1 is
superseded by the first `EnsureCurrent` republish, which is exactly the republish the follow-up
exists to remove; whoever removes it inherits a mapping that may have been diverging unobserved
since this phase landed. Gate it against a `DbPayloadProjection` publish before, not after.
**4, the equivalence golden is permanent, not a one-shot spike.** `DatabaseLoadEquivalenceTests`
compares the legacy-built and native-built `DatabaseTables` root by root and pins that the fused load
still returns a file under every root it compares (that `ImporterUtil` then consumes the map is
`ImporterUtilPreloadedTests`); it is a plain `[Test]`, so it runs in `dotnet test` — flip #6's route,
not the manual two-step harness. (c) Net `Native/` delta for the phase, the literal
`git diff --stat e265396..193bdac -- Libraries/SPTarkov.Server.Core/Native/`:

```
 .../SPTarkov.Server.Core/Native/NativeMethods.cs   |   3 +
 Libraries/SPTarkov.Server.Core/Native/SptNative.cs | 130 ++++++++++++++++++++-
 2 files changed, 132 insertions(+), 1 deletion(-)
```

Growth, and almost all of it additive: one `[LibraryImport]` entry, the `DbLoad` wrapper and the
framed-response parser with its three internal DTOs. The single deleted line is the
`ExpectedAbiVersion` constant, 28 → 29 — no existing `Native/` code path was replaced, and the
phase's other edits land outside `Native/`, in `ImporterUtil`, `JsonUtil` and `DatabaseImporter`.

**Phase 4 ledger.** The `configs` root (ABI 30) publishes all 28 loaded configs as a sixth root,
keyed by each one's `Kind` string, and six families — scav case, ragfair, quest, reward loot,
location loot, bots — read their config data off it instead of out of their per-call varying block.
No new exports, no new derived views, `spt_db_load` untouched.

(a) Freshness delta, and it is a real cost. Config data used to be projected by C# on **every**
send, so it was live by construction; now it is last-published state on the eligible arm. For a
*scalar* config write that is a wash — Phase 4 extended the Ceciler walk to all 28 config types
(33 roots in all), so a runtime property set moves the stamp and the next call republishes: still
correct, one republish dearer. For a *collection* mutation it is a straight loss. A mod calling
`Add`/`Remove`/indexer-set on `ItemConfig.Blacklist`, `LocationConfig.LooseLootBlacklist`,
`BotConfig.ItemSpawnLimits` and their kind was read fresh on every send before this phase and is
now invisible until some other stamped write happens to land. Config bodies are mostly collections,
so the window is wider than the table roots' equivalent, and unlike most of the table case these are
values a family genuinely reads. The kill switches — `DisableNativeRequestCache`, mods loaded
without trust, each family's `forceLegacy…` — restore per-call freshness, and root-level tracking
collections remain the sanctioned remedy: the spec named them, Phase 2 evaluated and declined them
on 4-of-~90 coverage against 17+ apicompat suppressions, and nothing here changes that arithmetic.
The eight hand-written bump sites are unchanged.

(b) Decisions. **1, wire keys are the `kind` strings** — read from each config's own `Kind` while
iterating the injected dictionary, not C# type names (the Phase 0 spike's choice) and not file
stems. The kind is the config's `JsonPropertyName`-pinned wire identity and needs no reflection.
**2, all 28 publish; Rust lifts only the ten stems it reads** (`spt-item`, `spt-scavcase`,
`spt-ragfair`, `spt-inventory`, `spt-quest`, `spt-location`, `spt-seasonalevents`, `spt-bot`,
`spt-pmc`, `spt-repair`) — everything else rides the root's flatten map full-fidelity. Per-type
curation would be maintenance for nothing; the root measured free (see (c)). **3, configs arrive by
`spt_db_publish` only.** `spt_db_load` stays `database/`-scoped: raw `configs/*.json` bytes are not
the live objects (C#-side record defaults, `PostDbLoadService` fixups, mod edits), so assembling a
root from disk walks straight into the values-not-keys trap. Epoch-1 config residency was
deliberately not built — the Phase 3 ledger has what that leaves the load-time-epoch follow-up.
**4, consumption is per-call resident reads** (the scav-case recipe-view precedent): no field joined
`ResidentDb`'s derived views, no derivation gate changed. One functional exception — ragfair's
money-tpl handling gained `customMoneyTpls` off `spt-inventory`, retiring the *Broken* ledger's
divergence where offers priced in a mod-added currency took the unrounded arm. An absent stem means
an empty set, exactly today's behaviour, and shipped `inventory.json` has no custom moneys, so the
goldens held. **5, the ineligible arm keeps its cost.** Each family's `viewsOverride` bundle gained
the config block its varying half used to carry — the same values, still C#-built per call — and
the varying structs shed the moved fields on both arms. Measured flat: a single-bot override send is
4,152,813 B against the pre-phase 4,208,129 B. **6, the barrier extension is 28 root FQNs, not a
namespace sweep** — appending the config types to `_publishedRoots` and letting reachability do the
rest; the spec's `Models/Spt/Config/*` sweep is rejected by the patch's own doc. The `_denied` list
gained `GenerationData`, so there are **four** denied types now, and gained name validation so a
drifted entry fails the build instead of silently barriering nothing. **7, `Option<Lift>` is the
strictness contract at the stem boundary.** An absent stem is `None` — the root parses, and the
family's per-call resolve fails loudly naming the stem. A present-but-malformed stem fails the whole
publish parse (`STATUS_BAD_ARGS`), previous resident DB intact — but *not* for one call only:
`DbPublisher.PublishLocked` never reaches `_lastPublishedStamp = stamp` when the publish throws, so
every later `EnsureCurrent()` re-attempts and throws again, from outside `ResidentDbDispatch.Send`'s
try, and every eligible native call 500s until the config is fixed. Reachable only through a mod
nulling a `required` member with `TrustNativeRequestCacheWithMods` on; the shipped projection cannot
produce it. Three lifts deliberately break the rule: `spt-item`'s four sets and `spt-inventory`'s
`customMoneyTpls` stay `#[serde(default)]` despite being C# `required`, and the `spt-pmc` stem
parses as the override wire's soft `PmcConfigWire` — the reasoning is in `ItemConfigLift`'s,
`InventoryConfigLift`'s, and `PmcConfigWire`'s docs, and the hand-run `phase4_configs_root.rs`
pins the soft members' wire names against the projected dump.
**8, caller-selected config stays varying.** Quest's `repeatableConfig` — the caller picks which
`QuestConfig.RepeatableQuests[i]`
applies — keeps riding both arms, flip #6's precedent for caller-supplied products; same for loot's
`containerSettings`/`rewardDetails` and bots' `levelGeneration`. **9, the bot equipment blacklists
moved to native selection.** The per-(role, level) `FirstOrDefault` over
`BotConfig.Equipment[role].Blacklist` became a Rust lookup, pinning the deliberate `level ?? 0`
divergence between the two lists; selection is not a draw, so it is RNG-neutral. Both members left
the wire **entirely, on both arms** — the selection reads the `equipment` the varying block still
carries, so neither needed a resident home. **10, the pmc name lists stay varying — flip #1's
revisit answered declined a second time.** Config residency removes only half the blocker:
`GatherPmcNamesOfLength` still reads the bot *table*, which has no root, and flip #6 already priced
one at 5.7 MiB and ~94.6 ms per publish to serve two lists. A names-only mini-root is the standing
upgrade if the varying cost ever measures. The same answer covered `modPoolSlotOrder` — until ABI 32
took a third route out and deleted the member rather than re-homing it — and still covers every
`SeasonalEventService` / `ItemBlacklistCache` / `LootableItemBlacklistCache` /
`RagfairLinkedItemService` / `GetMoneyTpls`-backed field: those are **service state, not config**,
and no phase currently owns them. The carve-out paragraph above was rewritten to say so — its
"Phases 2/4 exit" wording over-promised. **11, `customMoneyTpls` is the one projection divergence
fixed**; every other stays bug-for-bug. **12, family order** was scav case → ragfair → quest →
reward loot → location loot → bots: cleanest first to set the pattern, largest last.

Three rulings amended that plan during execution. **The two loot multipliers stay per-call** — the
plan's disposition table had them going resident and it was wrong.
`RaidTimeAdjustmentService.AdjustLootMultipliers` scales `LocationConfig.StaticLootMultiplier` and
`LooseLootMultiplier` **in place**, through the dictionary indexer, for a shortened scav raid and
puts them back after generation; no property setter fires, so the barriers never see it and a
resident snapshot would hand the raid unadjusted PMC-density loot. Both ride the varying block as
C#-resolved per-location scalars on both arms and land unread in `LocationConfigLift.extra`;
`LootResidentDbTests.AnInPlaceLootMultiplierAdjustmentReachesAResidentSend` pins it.
**`BotConfig.Equipment` stays varying** — the phase's largest planned lift, declined for the same
class of reason and a
worse one: `BotInventoryGenerator.ReplayRandomisationClamps` writes the nighttime mod-chance clamps
back into `Equipment[role].Randomisation[band].EquipmentMods` through the indexer after *every*
native single-bot send, and that write is a deliberate cross-bot feedback loop the next bot's C#
prelude reads (`BotEquipmentFilterService.cs:63`). A published copy would freeze at the config's
on-disk values and diverge from bot 2 of a nighttime raid on. Eleven of the twelve planned bot lifts
landed. The **named upgrade path** is to lift `equipment` resident and carry one varying member
holding just the live role+band `EquipmentMods`, gated by a second-bot nighttime regression test —
worth doing, because at 39,811 B `equipment` is the largest member of genuinely varying process
state on a bot send — it led `modPoolSlotOrder`'s 26,428 B when this was written and is now the only
one of the two left, ABI 32 having taken the mod-pool member off the wire (the caller-supplied
`templateVariants` is bigger than both, but it is not state anyone owns resident).
**`ItemConfigLift.blacklist` is a `HashSet<String>`, not the plan's
`IndexSet`** — the override wire mirrors C#'s `HashSet`, so both arms read one shape, and there is
no iteration site to observe an order. Test hygiene, for the record: zero `[Test]` bodies,
assertions, seeds or normalizers were edited anywhere in the phase. The two parity fixtures that
write configs they now publish keep the resident arm fresh by bumping `_databaseMutationStamp` in
their private helpers — `ScavCaseParityTests` gained the pattern, `RagfairParityTests` already had
it.

(c) Net `Native/` delta for the phase
(`git diff --stat 9222c49..e6d4ea1 -- Libraries/SPTarkov.Server.Core/Native/`): **+289/−210 lines
across 13 files.** Near-flat, and the shape says why: most of it is members moving from a varying
record into the family's `viewsOverride` record in the same file, each arriving with a doc line
naming its resident equivalent — a rename with commentary, not new machinery.
`Db/DbPayloadProjection.cs` (+17/−3) is the whole configs-root writer, `SptNative.cs` (+1/−1) is
the ABI constant, and `Bot/BotPayloadProjection.cs` (+31/−50) is the only genuine shrink, where the
two blacklist projections were deleted in favour of native selection. The wire, unlike the source,
did shrink: **−138,121 B per eligible bot send, a flat 23.0–23.4% at every wave size**, down to
10,272 B per bot at wave 45. The sixth root cost **+3.1 to +3.8 ms** of cold publish against flip
#6's five-root baseline — inside a 719–811 ms per-recipe spread, so free at the fixture's
resolution, against a budgeted ~67.7 ms. What now dominates an eligible bot send is not a config
member or a varying one but the caller's own `templateVariants` at **83.2%** of the request.
BENCHMARK.md § Phase 4 has all of it.

**Phase 5 ledger.** Four `spt_profile_*` exports (ABI 31) own `user/profiles/`' live listing, reads,
writes and deletes, so `SaveServer`'s disk boundary is native end to end. No resident state, no new
root, no legacy path and no config flag: the profiles directory arrives in every request and profile
bytes are opaque, written and read verbatim.

(a) Freshness delta: **none.** Profile bytes were never resident, and every load and save still hits
the disk through the same MD5 gate. What moved is *failure* visibility and I/O posture, in six
places.

- **Mid-write cancellation no longer exists** (Decision 7). A token is honoured before the native
  call and never inside it, so a started write always completes, where `WriteAsync` could be
  cancelled mid-file. Atomicity is unaffected either way — `FileUtil.WriteFileAsync` was already
  temp-then-rename — so what changed is only whether a late cancellation abandons the save or lets
  it finish.
- **I/O failures throw a different type** (Decision 8). `ProfileError{BadArgs,Io}` crosses as
  `STATUS_BAD_ARGS`/`STATUS_ERROR` and `DecodeResult` raises `InvalidOperationException`, where
  `FileUtil.WriteFileAsync` and `JsonUtil.DeserializeFromFileAsync` raised `IOException`-family
  types. No caller catches those specifically. **`RemoveProfile` changed the same way**:
  `FileUtil.DeleteFile` let `File.Delete` throw `IOException`/`UnauthorizedAccessException`;
  `SptNative.ProfileDelete` routes failures through `DecodeResult` into
  `InvalidOperationException`. Throw-vs-no-throw and the `bool` return semantics are unchanged — a
  missing file is still `false` and still just logged.
- **Profile I/O is no longer on async file handles.** `FileUtil.WriteFileAsync` opened its stream
  `useAsync: true` (`Utils/FileUtil.cs:127`) and `DeserializeFromFileAsync` read with `useAsync:
  true` (`Utils/JsonUtil.cs:109`); behind the FFI it is a blocking syscall on a `Task.Run`
  threadpool thread. `SaveAsync` and `LoadAsync` both loop sequentially, so exactly one thread is
  parked at a time and there is no starvation risk — but that is a property of those two loops, not
  parity, and a future concurrent caller would not inherit it.
- **A `default`/empty `MongoId` now throws** (Decision 9). `MongoId.ToString()` returns
  `string.Empty` for one, which fails Rust's 24-hex-char id gate; the old body silently probed
  `user/profiles/.json` and answered. Unreachable in-tree, but not for the tidy reason it is
  tempting to write down. `LoadProfileAsync` applies **no** id check of its own — it goes straight
  to `SptNative.ProfileLoadAsync` (`SaveServer.cs:198-199`), so for loads the native gate genuinely
  is the first thing an empty id meets, and the protection is entirely in the callers:
  `LoadAsync` pre-filters on `MongoId.IsValidMongoId`, `LauncherV2Controller.cs:156` passes a
  freshly minted `new MongoId()` (`:142`), and `CreateProfileService.cs:239-244` receives its id
  from the session and cannot be reached with an empty one — its first statement,
  `saveServer.GetProfile(sessionId)` (`:52`), throws on `IsEmpty` (`SaveServer.cs:103-106`).
  `SaveProfileAsync` never reaches Rust with one at all: `IsProfileInvalidOrUnloadable`
  returns `false` for an absent key (`SaveServer.cs:331-343`, it only returns `true` when the
  lookup *succeeds* and the flag is set), so an empty id passes that guard, takes the save lock,
  and then dies on `profiles[sessionID]` (`SaveServer.cs:282`) exactly as it did before this phase.
  Note for a future reader: the `!sessionId.IsEmpty` guard at `LauncherV2Controller.cs:95` is on the
  `RemoveProfile` call, not on the load/save pair, and there are more disk-reaching callers than
  that one — `CreateProfileService.cs:239-244`, `GameCallbacks.cs:70`, `PrestigeController.cs:98`
  and `LocationLifecycleService.cs:500,719`.
- **Profile listing is sorted**, where `Directory.GetFiles` order was filesystem-dependent. Load
  order is now deterministic where it previously was not — a strict improvement, but a change.
- **UTF-8 BOM handling is now explicit, not incidental.** The `FileStream` deserialize skipped a BOM
  for free; the `ReadOnlySpan<byte>` overload does not, so `profile.rs::load` strips it (reusing
  `db/load.rs::strip_bom`). Net player-visible behaviour is unchanged — that is the point — but the
  guard is now load-bearing code rather than a property of the .NET overload, and deleting it
  silently sends hand-edited BOM'd profiles down the `-corrupt.json` + backup-rollback arm.

One further behaviour change, landed as its own commit (`e7d3a4b`) ahead of the native swap:
**autosave failure isolation changed shape.** `SaveAsync` now catches per profile (rethrowing on
cancellation), and `saveMd5` is written *after* the write instead of before. Together: a failed
write no longer marks that profile version as persisted, and one unwritable profile no longer
aborts the remaining profiles for the tick. Before this phase the second property held only by
accident, through the poisoned hash. Shipping the reorder alone would have converted a per-version
loss into an unbounded multi-profile autosave outage, which is why the two halves are one commit;
`SaveAsyncSurvivesOneUnwritableProfile` is the pin.

**One correction to the plan's own text, recorded so it is not propagated.** Decision 5 says
`backups/`, `-corrupt.json` and stray `.bak` files "are excluded by the same C# lines that exclude
them today". That is right about the files and wrong about the directory. `-corrupt.json` and `.bak`
do reach C# and are dropped by the unchanged extension filter and the `MongoId.IsValidMongoId` stem
gate in `LoadAsync`. `backups/` is a **directory**: `profile.rs::list` keeps only entries whose
`fs::metadata` says `is_file()`, so it never reaches C# at all. The false premise matters because it
would later justify "simplifying away" C# filters that are in fact the only thing excluding the two
file cases. On the same listing: `fs::metadata(entry.path())` is used and not `entry.metadata()`,
because only the free function follows symlinks — `DirEntry::metadata` is `lstat` on Unix and would
classify a symlink-to-a-profile as neither file nor directory. Following-then-`is_file()` matches
`Directory.GetFiles` on the two cases that matter — measured on .NET 10.0.10, `GetFiles` returns a
symlink to a file and `GetDirectories`, not `GetFiles`, claims a symlink to a directory — but it is
**not exact**, and the source is the accurate account here, not this paragraph's earlier wording.
Two `stat` failures fall outside that match, and `list` now treats them differently — the
`unwrap_or(false)` that swallowed both is gone.

A **dangling** symlink is still skipped, and `list_skips_a_dangling_symlink` pins it. It is a real
divergence — `GetFiles` returns the link — but an inert one, and not for the reason an earlier draft
of this ledger gave: a dangling `{id}.json` link passes *both* C# filters, so the stem gate does not
save us. What does is `load`'s own `NotFound` arm, which answers `found: false` for that link
regardless; skipping it at the listing just reaches the same outcome a step earlier.

A **denied** `stat` is now raised instead. This is the larger divergence and the reason the code
changed. `readdir` needs only `+r` on a directory while `stat`ping a child needs `+x`, and .NET's
Unix enumerator answers file-vs-directory from `d_type` without `stat`ping a regular file — so on a
`user/profiles/` that has lost `+x`, `GetFiles` returns every profile while every `fs::metadata` here
fails `EACCES` (measured on .NET 10: `Directory.GetFiles` lists the file, `File.Exists` on that same
child is `false`). Swallowing that reported an empty directory, and `LoadAsync` came up with zero
profiles and offered to create a new one beside intact files. `list_raises_an_unreadable_entry`
pins the fix; it self-guards, because root bypasses the search bit and the arm is unreachable there.

**This is an improvement over the pre-phase C# too, not a parity restoration, and the distinction is
worth keeping straight.** `GetFiles` listing the profiles is where the pre-phase advantage ended:
`File.Exists` also returns `false` on `EACCES`, and both the guard in `LoadProfileAsync` and
`DeserializeFromFileAsync`'s own short-circuit (`JsonUtil.cs:104-107`,
`if (!File.Exists(file)) return default;`) are `File.Exists`. So the pre-phase path enumerated every
profile and then silently loaded none of them — the same zero-profile presentation, one stage later.
Raising the error is better than either state that preceded it.

**One known-stale cite left in place, deliberately.** `profile.rs`'s `save` doc comment comes with a
Windows warning — the `File` handle must drop before `fs::rename`, or `MoveFileExW` fails with a
sharing violation — and backs it with `RUST-ROADMAP.md:974`, which no longer points at the
"`mpex-server.exe` ships but has never been executed" sentence it was written against; this ledger's
own insertions moved it. It was left unfixed rather than edit production Rust in the phase's docs
commit. Same drift class as the Decision 10 pointer in (b), and the same remedy applies whenever that
file is next touched: cite the section, not the line. The warning itself is correct and load-bearing
— hoisting the handle out of the chain still passes every test on Linux, which is the only platform
this repo runs today.

(b) Decisions. **1, Rust is stateless and `dir` rides every request** — the `LoadRequest::dir`
pattern, no module static. The spec's "per-profile id↔resident-copy namespace" is established by the
wire contract (id-keyed exports), not by Rust-side state; residency waits on the profile-model port
(`todo/TODO.md` #19). **2, serialization stays C#** — `jsonUtil.Serialize` /
`Deserialize<JsonObject>` and `ProfileMigrationService` are untouched, and Rust is a byte-faithful
passthrough: `RawValue` on save, raw frame bytes on load. The on-disk format is byte-identical, so
hand-edited and shared profiles keep working and the MD5 dirty-check is unaffected. **3, the MD5
dirty-check and the per-session save locks stay C#** — identical skip-unchanged semantics,
`SaveProfileAsync`'s `Task<long>` unchanged in meaning. **4, `BackupService` stays C#, with the
coexistence rule written down**: Rust owns live-file writes, deletes and the load-time listing; C#
keeps the read-only probes (`RemoveProfile`'s final `FileExists`), the corrupt-copy, the backup copy
loop and the restore copy. The only writer overlap is restore-during-load, already serialized inside
`LoadProfileAsync`'s recovery arm. **5, all four exports take the standard envelope shape**
(`{"schema":1,"dir":…}`, plus `id` on three), not-found rides the load frame header
(`{"found":false}`) and **no new status code was added**; every filter stays in C# verbatim, so
there is zero filter-parity risk. **6, no legacy path and no `forceLegacy` flag** — the
`SPTLoggerDispatcher` precedent. The mod-visible consequence is a *Broken* ledger bullet, not a kill
switch. **7, cancellation is honoured before the native call only** (the `VerifyDatabaseAsync`
posture). **8, the error surface is `ProfileError{BadArgs,Io}`**, message naming the path and the OS
error. **9, Rust guards the id** — 24 ASCII hex chars, mirroring `MongoId.IsValidMongoId`
(`Extensions/MongoIdExtensions.cs:52-68`, which is where the rule lives; the static on `MongoId` is
a one-line delegate to it). This is the path-traversal guard at the trust boundary and is
non-negotiable even though C# always passes a typed `MongoId`; an id that passes cannot contain a
separator, a dot, or a parent reference. **10, the `DbPublisher._currentEpoch == 0` unconditional
republish is declined again** — it is independent of the profile disk boundary and still blocked on
the values-not-keys mapping gate. It stays open and re-filed for its own change; the discussion is
the **load-time-epoch follow-up in the Phase 3 ledger** above, under *Exceptions in force*. (Cited
by section and not by line: an earlier draft of this ledger pointed at `RUST-ROADMAP.md:720-727`,
which this very commit's Broken-ledger insertion pushed off target. Intra-file cites in this
document name their section, because the line numbers move every phase.) **11, no benchmark fixture, but the free number was taken** — and it came
back a regression; see (d). **12, plain synchronous `std::fs` on the calling thread** — single-file
ops need no tokio, and C# keeps its async posture through `Task.Run`.

(c) Net `Native/` + `SaveServer` delta for the phase
(`git diff --stat 159bf3d..b1f579a -- Libraries/SPTarkov.Server.Core/Native/
Libraries/SPTarkov.Server.Core/Servers/SaveServer.cs`): **+266/−18 across 3 files** —
`Native/NativeMethods.cs` +12/−0 (the four `[LibraryImport]` entries),
`Native/SptNative.cs` +226/−1 (the four wrappers, the `ProfileLoadResult` record, the frame parser;
the single deletion is the `ExpectedAbiVersion` constant, 30 → 31) and
`Servers/SaveServer.cs` +28/−17. Growth, and almost all of it additive: the boundary gained a
family and nothing in `Native/` was replaced. `SaveServer.cs` is the one file that both grew and
shrank, and roughly in balance — the `DirectoryExists`/`CreateDirectory` pair went away and the
per-profile autosave `try`/`catch` came in. `RemoveProfile`'s `Path.Combine`/`FileExists` probe
stays, per (b) Decision 4.

(d) The measurement, because Decision 11 pre-committed to it. `SaveProfileAsync`'s returned
milliseconds on a **26.50 MB synthetic profile**, 6 runs per pass and two passes per state:
**~161 ms median (155–186) before, ~192 ms median (187–217) after — about 20% slower, and the
ranges do not overlap across any of the four passes.** That is a real regression, not noise, and it
is recorded as one rather than argued away. The profile is synthetic (no player profile of
meaningful size exists in this environment) and the harness was throwaway, so the figure sizes the
effect rather than pinning it. Attribution, and the naive version of it is wrong: the pre-phase path
did **not** stream. `fileUtil.WriteFileAsync(filePath, jsonProfile, ct)` took the `string` overload
(`Utils/FileUtil.cs:103-107`), which does one `Encoding.UTF8.GetBytes` into a full-size `byte[]` and
gives that to a single `fs.WriteAsync`, so peak was already `jsonProfile` (UTF-16) plus one
full-size UTF-8 buffer. The `MemoryStream` **replaces** that buffer — same bytes plus ~128 of
envelope — and is not an extra one. The two real new costs are — "costs" and not "allocations",
because the second turns out to be a pooled buffer — `profile.rs`'s
`pub profile: Box<RawValue>` (`profile.rs:175`), which is **owned**, so serde scan-skips the profile
and then copies all 26.5 MB — the one extra full-size copy at peak — and, unanticipated by the plan,
`Utf8JsonWriter.WriteRawValue(string)` (`SptNative.cs:633`; `profileJson` is a `string`, `:614`, so
it is the string overload delegating to the char-span one), which transcodes through a `chars × 3`
scratch buffer rented from `ArrayPool<byte>.Shared`. That second cost is **much weaker than an
earlier draft of this ledger claimed**, and the claim is corrected here rather than quietly dropped:
the shared pool *does* serve buffers this large — measured on .NET 10.0.10 by reference identity at
1, 4, 16, 80, 128 and 512 MB — so there is no ~1 MB pooling cliff (that ceiling is
`ArrayPool<T>.Create()`'s `ConfigurableArrayPool`, which the same probe shows pooling at 1 MB and
not at 2 MB), and there is no guaranteed ~3x allocation per save: the first save on a thread
allocates ~6.2x the char count and every later one on that thread ~2.0x (both absolutes are
harness-inclusive — the probe measured the whole `ProfileSave` wrapper, so they count the
`MemoryStream` request buffer alongside the scratch and are not `WriteRawValue`'s share alone; the
~4.2x *difference* is what isolates the scratch). What keeps any of it real
is that `ProfileSaveAsync` hops through `Task.Run` (`SptNative.cs:611`) and the pool's fast path is
a per-thread TLS slot, so a save landing on a cold threadpool thread pays the first-call price. In
steady state the honest cost is the UTF-8 encode pass, not an allocation. The serde
parse-scan of the whole request buffer and that `Task.Run` hop are new work too, neither allocating a
full copy. **The ruling is that the regression
ships**: the remedy would make `spt_profile_save` the first export off the shared
`run_generator_with` ladder, at the tail of a phase whose entire value is mechanical parity. The
framed-request alternative is **re-opened as a named follow-up** (Roadmap item 6) rather than
implemented here — frame the save request the way the load response is framed, and/or hand the
wrapper UTF-8 bytes so `WriteRawValue`'s `ReadOnlySpan<byte>` overload skips the transcode.
BENCHMARK.md § Phase 5 has the table. **The load side was not separately timed and no claim is made
about its latency — but that covers timing, not allocation.** Where the save path's new buffer
replaced an old one, the load path's are pure addition: `DeserializeFromFileAsync`
(`Utils/JsonUtil.cs:102-113`) opened a `FileStream` with `bufferSize: 4096` and streamed it into
`JsonSerializer.DeserializeAsync`, so no full-size buffer of the profile ever existed, where the
native path materialises three transient ones — `fs::read` (`profile.rs:133`) into a `Vec`,
`encode_load_frame` (`profile.rs:154-165`) copying those bytes into a second exactly-sized `Vec` so
`write_buffer`'s `into_boxed_slice` does not realloc, and `ParseProfileFrame`'s `span[at..].ToArray()`
(`SptNative.cs:598`) copying them a third time onto the managed heap because the native buffer is
freed as soon as the wrapper returns. On the same 26.50 MB profile that is ~80 MB of churn per load
against approximately zero before; at most two are live at once, so the concurrent peak is ~53 MB.
Two of the three are native, so `GC.GetTotalAllocatedBytes` would not see them, and none of it is on
the save-side follow-up's path.

**The ported 4.1.2 quirks are documented at their call sites**, as numbered `Quirk N` comments in
`rust/spt-native/src/quest/*.rs`, `src/scav_case/generator.rs`, `src/base_class.rs`,
`src/linked_items.rs` and `src/loot/container_extensions.rs`; grep case-insensitively for `quirk`,
which also turns up unnumbered ones in the bot, loot and ragfair modules. Some numbers have no
Rust site because the quirk lives on the C# side (the base-class hydrate never resetting
`_rootNodeIds`, the
linked-item dispatcher's copy loop and no-lock rule, the request builder's null-`Filter`-group
projection) or on no code at all. The behaviour these preserve is deliberate; reverting one silently
diverges from C#. The bare `:N` line numbers in those comments are the 4.1.2 body the port was
written against, not the current file.

## Roadmap

1. **Phases 1 through 5 of the state-ownership program are complete; Phase 6b is the next front.** Phase 1 of
   `docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md` moved every generation
   export onto the epoch protocol, one family per flip — each its own plan, own ABI bump,
   goldens passing *unchanged*, BENCHMARK.md re-measured before the next started. Landed: #1
   ragfair, #2 repeatable quests, #3 base-class hydrate + linked-item table, #4 loot (statics
   resident on the locations root; looseLoot deliberately stays a per-call splice — the flip #4
   ledger has the 549 MiB number, and Phase 3 declined residency again), #5 scav case (2026-08-19,
   ABI 26; recipes resident on a new `hideout` root), #6 bots (2026-08-19, ABI 27; no new root,
   `SharedBotViewsWire` split into `viewsOverride` + `SharedBotVarying`). All thirteen generation
   exports read the resident DB, both slice caches are gone and all six slower-than-C# families
   have been re-measured; bots was the biggest win, as predicted — 90.32 → 13.19 ms per assault
   bot and a 7.1x smaller wire (BENCHMARK.md).
   **Phase 2 landed 2026-08-19** (no ABI change, still 27): `Patches/Ceciler.WriteBarriers` injects a
   stamp bump into every non-`init` model setter reachable from the five published roots (33 since
   Phase 4), and
   `TrustNativeRequestCacheWithMods` now defaults **on** in all six configs, gated on
   `WriteBarrier.Installed` so a Debug build — which Ceciler never rewrites — still forces the views
   override with mods loaded. A modded Release server rides the resident DB; the container gap and the
   rest of the residual blind spots are the *Broken* ledger's container-mutations bullet, the startup
   and churn numbers are BENCHMARK.md § Phase 2.
   **Phase 3 landed 2026-08-19 (ABI 29):** `spt_db_load` fuses the hash walk with reading
   `database/`, installs the five resident roots as epoch 1 and hands the eager file bytes to
   `ImporterUtil`; `CoreConfig.ForceLegacyDatabaseImport` is the opt-out and
   `DatabaseLoadEquivalenceTests` the permanent gate. It is a **measured regression at the
   importer** — 0.51x, +419–455 ms — and that cost is real rather than a wiring oversight: the
   fused load does assembly, parse and derivation work at import time that the legacy arm does not,
   and the buffer-fed walk is slower than the disk walk it retires. Separately, the *startup* win
   the flip was built for is still unwired — nothing feeds `DbLoad`'s installed epoch to
   `DbPublisher`, so **both** arms pay the first `EnsureCurrent` republish and it cancels out of the
   comparison above. The Phase 3 ledger has the numbers and the follow-up, BENCHMARK.md § Phase 3
   the measurement. Loose-loot residency was
   declined a second time, so the spec's `LazyLoad` byte-serving export does not exist.
   **Phase 4 landed 2026-08-20 (ABI 30):** all 28 configs publish as a sixth root keyed by `Kind`,
   the Ceciler walk covers 33 roots, and six families read their config data resident — which closes
   the runtime-config ceiling flip #1's ledger recorded for scalar writes and leaves config
   *collection* writes as the residual, on the same footing as the table roots'. `BotConfig.Equipment`
   and the two loot multipliers were ruled to stay varying (in-place indexer writes the barriers
   cannot see); the pmc name lists were declined a second time. The Phase 4 ledger has the decisions
   and BENCHMARK.md § Phase 4 the numbers — the root is free, the eligible bot wire is 23% smaller.
   **Phase 5 landed 2026-08-20 (ABI 31):** four `spt_profile_*` exports own `user/profiles/`' live
   listing, reads, writes and deletes, and `SaveServer`'s disk boundary is native end to end.
   Serialization, the MD5 dirty-check and `BackupService` stay C#; there is no legacy path and no
   config flag. It is a **measured save-side regression** — ~161 → ~192 ms on a 26.5 MB profile, about
   20% — shipped deliberately, with the framed-request remedy named as a follow-up (item 6 below).
   The Phase 5 ledger has the decisions and BENCHMARK.md § Phase 5 the number.
   Next is Phase 6 (process inversion: an `mpex-server` bin crate hosts
   the CLR via `netcorehost`, making Rust the executable). Phase 6a — the `run_app` bootstrap (`rust/mpex-server`,
   shipped by publish and the release container's entrypoint; `scripts/smoke-mpex-server.sh` is
   its e2e check) — landed 2026-08-18 (`mpex-server.exe` ships from the same wiring but has never
   been executed on Windows).
   **Phase 6b landed 2026-08-21 (no ABI bump).** The resident DB's statics now live in the
   executable: `mpex-server` links `spt-native` as an rlib and is linked with
   `-Wl,--export-dynamic`, so all 34 exports sit in its own `.dynsym`, and the two
   `SetDllImportResolver` callbacks try `NativeLibrary.GetMainProgramHandle()` before the cdylib.
   The published Linux tree therefore ships no cdylib and `SPT.Server.Linux` is no longer a working
   direct-run fallback there.
   It is **not** the design the spec described. The planned shape — `initialize_for_runtime_config`
   + `get_delegate_loader_for_assembly` + an `[UnmanagedCallersOnly] Init(HostVTable*)` in a shim
   assembly, with `DllImport` replaced by a 34-slot vtable and an ABI bump to 32 — was written out
   in full, reviewed twice, and replaced: `run_app`, `Program.Main`, `[LibraryImport]` and ABI 31
   all stay, and the change is ~85 lines. Five spec overrides, the declined `Build.props` order flip
   (nothing forces it: `mpex-server` links a sibling crate, not `SPT.Server.dll`) and the reasoning
   are in the Phase 6b ledger. Carried forward:
   **Windows exports.** An `.exe` has no export table without `/EXPORT:` args or a `.def` file, so
   the cdylib exclusion is Linux-gated and Windows behaviour is unchanged — which also still means
   never executed, and `Build.props:31` still maps no `win-x64` triple.
   **The one-linkage-path-per-process rule is enforced by publish layout, not structurally.** The
   published tree has no cdylib, so a lost export anchor is a loud boot failure there; a `bin/` tree
   keeps one for `dotnet test`, so the same mistake under a locally-built launcher falls through and
   boots silently with the statics in the cdylib. Nothing at runtime can distinguish the two —
   `GetMainProgramHandle()` is a `dlopen(NULL)` pseudo-handle.
   **The launcher arm has no end-to-end gate outside `scripts/smoke-mpex-server.sh`,** and this fork
   has no CI to run it. `dotnet test` always takes the cdylib arm, so the suite says nothing about
   the launcher one; `DllImportResolverTests` pins only that the test host correctly declines it.
2. Port candidates and their costing live in [todo/TODO.md](todo/TODO.md); with #1-#6
   landed, the unstarted front is tier 2. The two axes
   are independent — a flip re-homes data for something already ported, a TODO item ports
   something new.
3. Convert `is_valid_reward_item`'s trader whitelist (`quest/reward_generator.rs:869`, a `Vec<&str>`
   of up to 14 candidates) to `ItemBaseClassCache::is_of_baseclasses_set` and measure whether 14 is
   long enough for the set form to pay. Narrow and unmeasured.
4. **Delivered at ABI 32: `BotPayloadProjection.BuildModPoolSlotOrder` is gone.** The member could
   never go resident — flip #6's decision 2 showed the order is the live
   `BotEquipmentModPoolService`'s `ConcurrentDictionary` enumeration order, process-local and not a
   function of the database — so the exit taken was the other one Phase 2's write barriers made
   safe: **own the pools rather than observe them.** The native pool now enumerates the template's
   own `Properties.Slots`, the order is Rust's own, and the 26,428 bytes per send left the wire
   entirely on both arms. This item's "own the pools" framing was already half-true when written:
   pool *contents* were derived natively from the bot port onward (`mod_pool_service.rs`, 2026-08-13) and
   only the *ordering* was ever observed from C#, which is why the change is a **deletion** on the C#
   side rather than a port. `BuildRequest` fell **5.19 → 0.23 ms** (assault, BENCHMARK.md § Mod-pool
   ownership); the "~6 ms of the measured 6.06 ms" this item used to claim was an estimate and never
   a measurement. What it bought beyond the wire and the time: the C# order was sized from
   `Environment.ProcessorCount` (13 real slot names moved between 8 and 16 cores), so it was never
   machine-independent; the native draw **order** is host-independent now. The *output* still is not
   reproducible across processes, for a reason older than this change (see the *Broken* ledger's
   second bot entry). What it cost:
   the two arms draw in different orders at randomised levels, and the exact-output coverage there is
   gone on **both** — booked in the *Broken* ledger, together with the process-nondeterminism finding
   that made a native-only golden unimplementable. `BotEquipmentModPoolService` gained a whole-type
   decline entry (guideline 2), Rust no longer consulting it.
5. **Named by Phase 4, not delivered with it: split `BotConfig.Equipment`.** The Phase 4 ledger's
   third amendment — lift `equipment` onto the resident configs root and keep one varying member
   carrying just the live role+band `EquipmentMods` that `ReplayRandomisationClamps` writes, gated by
   a second-bot nighttime regression test. Now the **largest** member of genuinely varying process
   state on a bot send and the only one left: 39,811 bytes, with item 4's 26,428 off the wire
   (BENCHMARK.md § Phase 4).
6. **Named by Phase 5, not delivered with it: frame the profile save request.** Phase 5 measured
   `spt_profile_save` ~20% slower than the `FileUtil.WriteFileAsync` it replaced (~161 → ~192 ms on a
   26.5 MB profile) and shipped it anyway. The removable costs, in the order they are worth chasing:
   the owned `Box<RawValue>` copy in `profile.rs` — a genuine extra full-size copy at peak — and
   `Utf8JsonWriter.WriteRawValue(string)`'s `chars × 3` transcode scratch, which is a **full UTF-8
   encode pass** on every save but, contrary to an earlier draft of this item, is *not* a guaranteed
   allocation: `ArrayPool<byte>.Shared` pools at that size (measured; there is no ~1 MB cliff), so it
   allocates only on a threadpool thread whose pool cache is cold. Handing the wrapper UTF-8 bytes so
   the `ReadOnlySpan<byte>` overload is taken still buys the encode pass; the allocation half of that
   argument is much weaker than it was written. Framing the save request the way the load response is
   framed is the other half and removes the `RawValue` copy. The price is that `spt_profile_save`
   becomes the first export off the shared `run_generator_with` ladder, which is exactly why Phase 5
   declined to do it inline. BENCHMARK.md § Phase 5 has the measurement and the pool probe.
