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

Twenty-nine C-ABI exports (`src/ffi.rs`) carry all of it, JSON in and JSON out — except the ragfair
response, which is a framed MessagePack envelope, and the log and console exports, which pass the
fields of one line, or raw bytes, directly (current ABI 28).

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
| Resident DB publish — the templates, traders, globals, locations and hideout roots, plus the ragfair, quest and bot views derived from them | `DbPublisher.EnsureCurrent` / `ForcePublish` | `spt_db_publish` |
| The whole log pipeline — filters, level gates, per-target formatting, console + file sinks | `SPTLoggerDispatcher.Log` | `spt_logger_init`, `spt_logger_reinit`, `spt_log_emit`, `spt_logger_close`, `spt_log_set_tap` |
| The terminal — raw `Console.Write*` (redirected into the pipeline), prompts, title, clear | `NativeConsoleWriter.Install`, `SptConsole` | `spt_console_write`, `spt_console_read_line`, `spt_console_set_title`, `spt_console_clear` |
| The `IsLogEnabled` gate and the line a mod `ILogHandler` renders | `SPTLoggerDispatcher.IsLogEnabled`, `BaseLogHandler.FormatMessage` | `spt_log_enabled`, `spt_log_format` |
| Generator diagnostics, localised and logged natively as they happen | `DatabaseImporter` → `SptNative.SetServerLocales` | `spt_locales_set` |

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
  `BotWeaponGeneratorHelper`, `BotEquipmentModPoolService`, `BotLootCacheService`,
  `WeightedRandomHelper`, `ItemFilterService`/`PresetHelper` predicates, `ICloner`, plus ragfair's
  `HandbookHelper`, `PaymentHelper`, `BotHelper`, `TraderHelper`, `SeasonalEventService`, quests'
  `MathUtil` and scav case's `RagfairPriceService.GetStaticPriceForItem` and `HideoutTable` reads.
  One partial exception: `SeasonalEventService.ChristmasEventEnabled` and
  `RemoveChristmasItemsFromBotInventory` *are* detected — member-scoped, by the bot wave batcher
  only, because the batch re-times them per level band. A patch there de-batches the wave to the
  per-bot path, where the strip runs in C# and the patch takes effect; every other use of the type,
  ragfair's included, stays undetected.
- **Templates without `_props` read as "not in the db"** on the native *generator* paths — they are
  dropped from `itemsView`. Only bites mod-added props-less templates. The base-class hydrate
  projects the whole table and is unaffected.
- **`customMoneyTpls` are not projected to the ragfair native path** — offers priced in a mod-added
  currency go through the unrounded arm.
- **The native ragfair and scav case paths are fresher than legacy for runtime-added items** — C#
  caches `AllowedFleaPriceItemsForBarter` (ragfair) and `DbItemsCache`/`DbAmmoItemsCache` (scav
  case) per generator instance and effectively never invalidates; Rust re-derives per call
  (override sends only since flips #1/#5 — the eligible path is fresh-per-publish; see the flip
  ledgers).
- **Container mutations of a table are invisible to the resident-DB families** — since Phase 2 the
  Ceciler-injected write barriers (`Patches/Ceciler.WriteBarriers`) bump the stamp from every
  non-`init` property setter reachable from the five published roots — a walk over property types
  *and* base types, since `TradersTable` declares nothing and reaches `Trader` only through its
  `Dictionary<MongoId, Trader>` base — so a mod's scalar writes reach the resident DB without a
  hand-written bump. What stays invisible: a mod calling `Add`/`Remove`/indexer-set on a table
  collection (root-level or one below — `trader.Assort.Items`, `handbook.Items`), array element
  writes, reflection-driven writes, the setters of the three denied live-per-request types (`Item`,
  `BotBase`, `PmcDataRepeatableQuest` — so a write to a trader assort `Item`'s `Upd` bumps nothing;
  the churn guard added no fourth entry), the setters of open-generic model types, which are never
  barriered by design (`MinMax<T>`'s three, so a mod editing a location's `Limit`/`MinMaxBot` bands
  writes nothing the stamp sees), anything behind an `object?`-typed property the walk cannot follow
  (`TemplateSide.EquipmentBuilds`/`WeaponBuilds`), and a genuine database write performed inside a
  native-response decode callback's extent — `SptNative.DecodeResult` holds a
  `WriteBarrier.Suppress()` scope across the decode, because deserializing a response into
  DB-shaped model types (a repeatable quest's condition subgraph, `SpawnpointTemplate`) was the real
  churn source. Nothing does that today and `WriteBarrierChurnTests` pins the invariant by name. The
  publish's own suppression scope has the same shape and one mod-reachable edge: it spans
  `DbPayloadProjection`'s `LazyLoad.Value` reads, so a mod-registered transformer that writes into a
  published root *other* than the value it transforms is suppressed too (both shipped `StaticLoot`
  transformers write only inside the transformed graph).
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
  full five-root republish — once per client menu load, accepted rather than mitigated
  (BENCHMARK.md § Phase 2).
- **Write barriers exist on Release and publish builds only.** Ceciler does not run in Debug, so
  `WriteBarrier.Installed` is false there and `ResidentDbDispatch.Eligible` refuses to honour
  `TrustNativeRequestCacheWithMods` — a Debug server with mods loaded always sends the views
  override. Deliberate: the trust flag must not vouch for barriers that were never injected.
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
  or `}` now renders literally instead of throwing out of `CompositeFormat.Parse`, and a handler that
  cannot reach the native side degrades to the unformatted message rather than throwing.
- **A `GetCompiledFormat` override no longer reaches `FormatMessage`** — `BaseLogHandler` hands the
  reference's raw `Format` string to `spt_log_format`, so a mod subclassing `BaseSptLoggerReference`
  to rewrite the compiled format changes nothing about what a handler renders. The method still
  compiles and caches for anyone calling it directly; both shipped reference types are `sealed`, so
  reaching the override at all meant constructing the subclass in code.
- **`Console.Clear()`'s `IsOutputRedirected` guard became a Rust-side tty check** — clear and title
  are ANSI/OSC escapes queued behind the console sink, with VT enabled on Windows at
  `spt_logger_init` (where the Windows title is still a console API call, not an escape).

## Guidelines

1. **Frozen surface.** Preserve the ported class's entire 4.1.2 public *and protected* surface —
   constructor including parameter names, methods, DTOs. Keep the C# implementation verbatim as the
   legacy path; never delete it. Enforced by `dotnet apicompat` in the sibling `mpex-api-compat` repo.
   **Why the `Native/` payload reshapes never flag it:** the whole tree post-dates the 4.1.2
   baseline, so its members are *additions*, which apicompat does not report — not because they are
   hidden — plenty of them are public (`Native/Loot/LootPayloads.cs`,
   `Native/BaseClass/ItemBaseClassPayloads.cs`, `DbPublisher`). A future flip reshaping one of those
   should expect a clean run for that reason, not from a visibility rule that does not hold.
2. **Override contract.** Detect Harmony patches on the frozen members (`Harmony.GetPatchInfo`) and
   route to legacy so hooks fire with baseline semantics. Add a `forceLegacy...` config flag as the
   escape hatch for hooks detection can't see.
3. **Resident DB epoch, publish on dirty.** DB-derived state lives resident on the Rust side:
   `DbPublisher` republishes every supported root when the global `DatabaseMutationStamp` has moved
   and stamps the returned epoch into each request. Since Phase 2 the stamp is moved primarily by the
   Ceciler-injected write barriers on the model setters reachable from the published roots, with
   eight hand-written bump sites left for the container writes barriers cannot see. Only the varying
   block — per-call service and config state — and the optional `viewsOverride` remain per-call.
   Ineligible callers — mods loaded where `TrustNativeRequestCacheWithMods` does not hold (it defaults
   **on** since Phase 2, and counts only where `WriteBarrier.Installed`, i.e. Release and publish
   builds), or anyone with `DisableNativeRequestCache` — send the C#-built
   view bundle as `viewsOverride` on every call at today's projection cost, never touching resident
   state. Full protocol: the epoch-protocol section of
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
   No third-party consumer of the cdylib is supported.
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
`ScavCaseConfig.ForceLegacyScavCaseGeneration`, `ItemConfig.ForceLegacyItemBaseClassHydration`, plus
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

**The reward-loot blacklist crosses as two collections** — `configBlacklist` for the reward pool,
`globalBlacklist` for sealed-container filters. They differ once a mod calls
`AddItemToBlacklistCache` at runtime; collapsing them would change behaviour.

**Loose loot has two input paths.** Null `dynamicLootDist` splices `looseLoot.json`'s raw bytes in
unparsed (faster, more faithful); a registered `LazyLoad` transformer (seasonal events, mods) forces
the typed path instead, which is slower than both the raw path and the C# it replaced. A mod can
therefore put a server on the slow path without saying so. Since flip #4 the fork lives inside the
request's varying block (`varying.looseLoot`) on both the resident and the override arm — loose
loot is the one loot input that never went resident, by decision: raw bytes resident would cost
549 MiB RSS (BENCHMARK.md § Phase 0), so the per-call splice survives until Phase 3
(`spt_db_load`), where Rust holds the per-map bytes resident from disk for free.

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
reachable from the five published roots, plus eight hand-written sites for container writes the
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
`defaultPresetsByTpl` borrow the already-resident ragfair views. Six quest service/config-backed fields (`itemBlacklist`, `rewardItemBlacklist`,
`bossItems`, `seasonalItemTplBlacklist`, `repeatableQuestTemplateIds`, `locationIdMap`) ride the
varying block per call until Phase 4 — the same carve-out ragfair's config-derived fields
took, and loot's carve-out set (`config`, `seasonal`, `lootableItemBlacklist`, `moneyTpls`, the
six reward blacklists/sets, sealed's mod-extendable `linkedItems`) rides the same way, as does
scav case's (`recipeId`, `config`, `inactiveSeasonalItems`, `globalBlacklist`,
`rewardItemBlacklist`, `bossItems`). **Flip #6 (ABI 27) put both bot exports — the single bot and
the batched wave — on the same envelope**, closing Phase 1: no new root, `BotDbViews` deriving at
publish from the templates and globals roots once the ragfair views are resident (it embeds
`RagfairDbViews` by `Arc`, the quest views' precedent, and adds only `defaultPresetIdsByTpl` and
`expTable`). The pre-flip `SharedBotViewsWire` split in two — the database half became
`viewsOverride` on the ineligible arm, the rest was renamed `SharedBotVarying` — and the bot
family's varying carve-out is `modPoolSlotOrder` plus the config and service-backed blocks
(`equipment`, `bosses`, `durability`, the two equipment blacklists, `configBlacklist`, the
`pmcConfig`/`repairKitWeapon` lifts). Each bot's `template` and `lootPools` stay caller-supplied
on both arms — the batch ships them per level band — because the filtered template is the caller's
own product, not a database view.
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
lists stay varying until Phase 4, because the filter reads a config value and the root's ~94.6 ms
of per-publish projection would serve nothing else. (c) Runtime *config* edits still bypass the
stamp — the
pre-flip ceiling, unchanged, and Phase 4 closes it — but ragfair's config-derived fields now flow
through the varying block on every call, which narrows the ceiling for ragfair. (d) An
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
`WriteRawValue` splice; Phase 3 (`spt_db_load`) makes the per-map bytes resident from disk for
free, so residency is deferred there (this plan's call, per the spec's decision register —
overriding the spec's older raw-bytes-resident flip-order prose). `staticAmmoDist` is
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
The varying carve-out (`recipeId`, `config`, the four service-backed sets) rides every send until
Phases 2/4, the standing carve-out every flip has taken. The pre-flip hydration sweep found readers
only (`ScavCaseRewardGenerator`, `HideoutController`) and no lazy writer into
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
closed as answered. **2, `modPoolSlotOrder` is not a view.** The plan had it deriving
into `BotDbViews` at publish; Task 5's field-for-field resident-vs-override identity test caught
the divergence the plan itself ranked first, and the root cause is not port drift: the C# order is
the enumeration order of the live `BotEquipmentModPoolService`'s `ConcurrentDictionary`, which is
process-local (bucket layout, `ProcessorCount`-dependent growth) and not a function of the database
at all. The Rust derivation was deleted and the field moved into `SharedBotVarying`/
`SharedBotVaryingWire`, riding the per-call varying block on **both** arms at 26,428 bytes
(BENCHMARK.md) under the spec's standing service-backed carve-out — the same class as ragfair's
config-derived fields and quest's six service-backed sets, and the same Phases 2/4 exit. It is now
the largest single member still crossing per bot. **3, `BotDbViews` as built**:
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

1. **Phases 1 and 2 of the state-ownership program are complete; Phase 3 is the active front.** Phase 1 of
   `docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md` moved every generation
   export onto the epoch protocol, one family per flip — each its own plan, own ABI bump,
   goldens passing *unchanged*, BENCHMARK.md re-measured before the next started. Landed: #1
   ragfair, #2 repeatable quests, #3 base-class hydrate + linked-item table, #4 loot (statics
   resident on the locations root; looseLoot deliberately stays a per-call splice — the flip #4
   ledger has the 549 MiB number, Phase 3 the residency point), #5 scav case (2026-08-19,
   ABI 26; recipes resident on a new `hideout` root), #6 bots (2026-08-19, ABI 27; no new root,
   `SharedBotViewsWire` split into `viewsOverride` + `SharedBotVarying`). All thirteen generation
   exports read the resident DB, both slice caches are gone and all six slower-than-C# families
   have been re-measured; bots was the biggest win, as predicted — 90.32 → 13.19 ms per assault
   bot and a 7.1x smaller wire (BENCHMARK.md).
   **Phase 2 landed 2026-08-19** (no ABI change, still 27): `Patches/Ceciler.WriteBarriers` injects a
   stamp bump into every non-`init` model setter reachable from the five published roots, and
   `TrustNativeRequestCacheWithMods` now defaults **on** in all six configs, gated on
   `WriteBarrier.Installed` so a Debug build — which Ceciler never rewrites — still forces the views
   override with mods loaded. A modded Release server rides the resident DB; the container gap and the
   rest of the residual blind spots are the *Broken* ledger's container-mutations bullet, the startup
   and churn numbers are BENCHMARK.md § Phase 2.
   Next is Phase 3 (Rust loads `SPT_Data`), Phase 4
   (configs join the resident set, closing the runtime-config ceiling flip #1's ledger records),
   Phase 5 (profile persistence) and Phase 6 (process inversion: an `mpex-server` bin crate hosts
   the CLR via `netcorehost`, making Rust the executable). Phase 6a — the `run_app` bootstrap (`rust/mpex-server`,
   shipped by publish and the release container's entrypoint; `scripts/smoke-mpex-server.sh` is
   its e2e check) — landed 2026-08-18 (`mpex-server.exe` ships from the same wiring but has never
   been executed on Windows); 6b (the delegate-loader shim flip, where the resident DB's
   statics move into the exe and `SptNative.cs`'s `DllImport` layer dissolves into a vtable of
   the existing exports) waits on Phases 3 and 5.
2. Port candidates and their costing live in [todo/TODO.md](todo/TODO.md); with #1-#6
   landed, the unstarted front is tier 2. The two axes
   are independent — a flip re-homes data for something already ported, a TODO item ports
   something new.
3. Convert `is_valid_reward_item`'s trader whitelist (`quest/reward_generator.rs:869`, a `Vec<&str>`
   of up to 14 candidates) to `ItemBaseClassCache::is_of_baseclasses_set` and measure whether 14 is
   long enough for the set form to pay. Narrow and unmeasured.
4. **Enabled by Phase 2, not delivered with it: `BotPayloadProjection.BuildModPoolSlotOrder`.**
   With the bot database half
   resident (flip #6), this is the dominant irreducible per-call C# cost left on the single-bot
   resident path — a full items-table walk with two `BotEquipmentModPoolService` lookups per tpl,
   ~6 ms of the measured 6.06 ms `BuildRequest` (BENCHMARK.md, assault) — and the one remaining
   member that structurally *cannot* go resident under the current service design: the flip #6
   ledger's decision 2 shows the order is the live service's `ConcurrentDictionary` enumeration
   order, process-local and not a function of the database. Phase 2's write barriers are what made
   the alternative safe — own the pools rather than observe them, so the order is Rust's own and the
   member leaves the wire entirely — but the work itself is unstarted. Until then it rides both arms
   at 26,428 bytes per send.
