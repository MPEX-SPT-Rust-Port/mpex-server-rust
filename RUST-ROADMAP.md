# Rust port — status and roadmap

What is ported to `rust/spt-native`, what is known-broken, and what comes next. For the crate's
internals see [rust/ARCHITECTURE.md](rust/ARCHITECTURE.md); for the C# side of the boundary see
[ARCHITECTURE.md](ARCHITECTURE.md) § *Native Rust layer*. Every measurement lives in
[BENCHMARK.md](BENCHMARK.md); no timings are repeated here.

## Status

The loot family, the bot family, dynamic ragfair offer generation, the repeatable-quest family, scav
case rewards and the item base-class cache build are ported and run natively by default. Every
ported class keeps its full 4.1.2 C# implementation as a **legacy path**, selected automatically
when a mod hooks it or manually via a config flag. The log pipeline is ported too, and has no legacy
path: `SPTLoggerDispatcher` hands every line to the crate.

Twenty-one C-ABI exports (`src/ffi.rs`) carry all of it, JSON in and JSON out — except the ragfair
response, which is a framed MessagePack envelope, and `spt_log_emit`, which passes the fields of one
line directly (current ABI 20).

Native is not uniformly faster. Loot and repeatable quests win; bots, reward loot, ragfair, scav case
and the base-class hydrate are slower than the C# they replace, and native stays their default
anyway — each case is argued where it is measured, in [BENCHMARK.md](BENCHMARK.md), and each has a
force-legacy flag for anyone who disagrees. Ragfair is the one family that set itself a parity gate
and **missed** it, with every in-scope lever spent.

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
| The whole log pipeline — filters, level gates, per-target formatting, console + file sinks | `SPTLoggerDispatcher.Log` | `spt_logger_init`, `spt_logger_reinit`, `spt_log_emit`, `spt_logger_close`, `spt_log_set_tap` |
| Generator diagnostics, localised and logged natively as they happen | `DatabaseImporter` → `SptNative.SetServerLocales` | `spt_locales_set` |

Also working: mod-added fields on game data survive the round trip (`#[serde(flatten)] extra` maps
mirroring Ceciler's `[JsonExtensionData]`); native generator diagnostics render and log themselves
through the native pipeline; seeded-RNG parity at the primitive level (xoshiro256\*\*, twin
known-answer tests both sides).

## Broken / known divergences

### Behaviour

- **`get_flea_prices_as_array` is O(offers × price table) if a mod enables barters** — it re-derives
  the whole filtered flea price list per barter offer. Dead on shipped data (`ragfair.json`
  `dynamic.barter.chancePercent` is `0`), but a mod raising that pays it on every barter offer of a
  full pass. Latent, not measured.
- **Patches on collaborators do not reach the native path** and do not flip to legacy — only the
  ported classes' own members are detected. Affected: `RandomUtil`, `ItemHelper`,
  `CounterTrackerHelper`, `BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper`, `BotEquipmentModPoolService`, `BotLootCacheService`,
  `WeightedRandomHelper`, `ItemFilterService`/`PresetHelper` predicates, `ICloner`. Ragfair adds
  `HandbookHelper`, `PaymentHelper`, `BotHelper`, `TraderHelper` and `SeasonalEventService`.
  Repeatable quests add `MathUtil`. Scav case adds `RagfairPriceService.GetStaticPriceForItem` and
  the `HideoutTable` recipe reads, and checks no collaborator *substitution* at all — see
  *Exceptions in force*.
- **Templates without `_props` read as "not in the db"** on the native *generator* paths — they are
  dropped from `itemsView`. Only bites mod-added props-less templates. The base-class hydrate is the
  exception: it projects the whole table, so a props-less template gets its cache entry as in C#.
- **`customMoneyTpls` are not projected to the ragfair native path** — offers priced in a mod-added
  currency go through the unrounded arm.
- **The native ragfair and scav case paths are fresher than legacy for runtime-added items** — C#
  caches `AllowedFleaPriceItemsForBarter` (ragfair) and `DbItemsCache`/`DbAmmoItemsCache` (scav
  case) once per generator instance and effectively never invalidates them; Rust re-derives per
  call. In native's favour, but a divergence.
- **The item base-class cache *key* differs** — legacy stores under `item.Id`
  (`ItemBaseClassService.cs:194,199`), native under the `templateTable.Items` dictionary key.
  Separable only by a mod inserting a template under a key ≠ its `_id`, where legacy is the broken
  arm. In native's favour.
- **Golden-test parity is normalised, not raw-byte.** Every family has a full-output golden gate
  (`LootParityTests`, `BotParityTests`, `RewardLootParityTests`, `RagfairParityTests`,
  `RepeatableQuestParityTests`, `ScavCaseParityTests`, `ItemBaseClassParityTests` — the last needs
  no normaliser, its walk being deterministic and compared for exact equality). The sanctioned gaps
  are minted `MongoId`s (outside the seeded stream on both sides, masked by the normaliser) and, for
  ragfair only, `intId` (a live C# counter) and `startTime`/`endTime` (one batch timestamp natively
  vs a per-offer clock in legacy, so only the spread is compared).
- **A failure crosses as a message for C# to throw with** — everything the run logged on the way
  down is already in the log; the error text itself stays the C# caller's to log (one-buffer
  contract). Since ABI 18 a panic crosses with its message too. The error line carries no category:
  it arrives as an exception, not a log line.
- **Hangs are mostly undiagnosable** — ported retry loops can spin exactly as 4.1.2 does, inside an
  FFI call with no managed stack trace. Generator diagnostics stream, so a hang beside a diagnostic
  site shows its last line; elsewhere, nothing. Force legacy to get the managed stack back.

The next four cannot fire on shipped data, and the parity gates would catch them if they started to.
They are recorded because a mod could reach them:

- **Unknown scav case recipe id** — native returns "no recipe found"; legacy NREs at the same call
  site. A recipe missing any of the three `EndProducts` ranges is dropped by
  `ScavCaseNativeRequestBuilder` and gets the same message; C# could never generate it either.
- **An ammo pool with nothing in the rarity's price band** — native fails with a message, legacy
  throws indexing the empty sequence. All three shipped rarity bands have ammo.
- **A parentless Item-type template or a cyclic parent chain on the hydrate** — C# stores
  `{ MongoId.Empty }` where native leaves `{}`, and recurses forever on a cycle where native breaks
  at the first repeated parent. No shipped Item-type template is parentless and the chains are
  acyclic.
- **The native `_type` test is ASCII-only** — `eq_ignore_ascii_case` against C#'s
  `StringComparison.OrdinalIgnoreCase`, so a `_type` matching `"Item"` only under non-ASCII case
  folding diverges. Every shipped `_type` is `"Item"` or `"Node"`.

### Logging

- **Generator lines carry one category per generator** — `typeof(T).FullName` of the C# class each
  Rust module ports, where the replay era logged the whole bot family through
  `ISptLogger<BotInventoryGenerator>`. A `sptLogger.json` filter written against that class now
  matches far fewer lines.
- **Generator lines use a different `%tid%` space** — a process-local counter handed out per thread
  in first-emit order, not the managed thread id. `%tname%` is the Rust thread name, usually empty.
  `%date%` is the moment of emission, where replayed lines were stamped at the end of the call.
- **Generator locale text is a startup snapshot** — `DatabaseImporter` pushes resolved server locales
  once (`spt_locales_set`), so a mod mutating them later no longer changes generator line text. A
  failed push is one stderr notice and every generator line falls back to its locale key.
- **Parallel generator lines interleave** — ragfair and bot rayon workers emit as they run, so lines
  no longer arrive grouped per bot or per assort entry.
- **Console output is asynchronous and drops on a full queue** — each line goes to a writer thread
  behind an 8192-line bounded channel. A hard crash loses what is queued; a burst deeper than the
  queue drops lines rather than blocking.
- **Excluded categories still pay the per-line marshaling cost** — filtering moved native-side, so
  every line is encoded and crosses the boundary before it is dropped.
- **Filter regexes are regex-lite** — no lookarounds, no backreferences, ASCII-only character
  classes. A pattern that will not compile is reported to stderr once and then never matches.
- **A native logging failure has no C# fallback** — a failed `spt_logger_init` means one stderr
  notice and no logging at all for the run. A config the C# parser tolerated but Rust rejects fails
  the same way; the known cases (UTF-8 BOM, case-insensitive `logLevel`/filter `type`/`matchingType`)
  are handled, but the `type` tag of a `loggers` entry stays case-sensitive on both sides.
- **The pipeline reads `sptLogger.json` once; runtime mutation needs an explicit reload** — mutating
  `SptLoggerConfiguration.Loggers` alone changes what `IsLogEnabled` answers but not what is
  written. `SPTLoggerDispatcher.ReloadConfiguration()` (additive, post-port) re-hands the object to
  `spt_logger_reinit`; a reload the native parser rejects leaves the running pipeline untouched.
  Mutate `Loggers` before the server serves traffic, or accept the enumeration race — unguarded,
  exactly as pre-port.
- **Line terminators are always `\n` and dates always Gregorian, culture-independent** — a Windows
  log file loses its `\r` and a non-Gregorian locale no longer shows in `%date%`.
- **File rotation was redesigned, not ported** — ZLogger rolled with an ascending sequence
  (`spt.1.log` was the *next* file); the native sink cascades, so `.1` is always the *most recent*
  archive and `spt.log` only ever holds the current run. Anyone comparing `spt.N.log` across the
  upgrade reads the sequence in the opposite order.
- **Lowering `maxRollingFiles` does not sweep the old high indices** — a change from 10 to 3 strands
  `spt.3.log`..`spt.9.log` until deleted by hand (the `ponytail:` note in `log_sink.rs`'s `cascade`).
- **Mod `ILogHandler` routing goes through a hybrid tap.** The dispatcher fans C#-originated lines
  out to handlers (original message and `Exception` object, per-reference filters and level);
  `spt_log_set_tap`'s callback delivers Rust-originated generator lines, which arrive as rendered
  text with no `Exception` object and the native `%tid%` counter. Registration changed shape:
  resolve `SPTLoggerDispatcher` from DI and call `RegisterHandler` (additive, post-port), because
  `AddSptLogger` builds the dispatcher from its own service collection and a constructor-injected
  handler set is therefore always empty in a real run.

## Guidelines

1. **Frozen surface.** Preserve the ported class's entire 4.1.2 public *and protected* surface —
   constructor including parameter names, methods, DTOs. Keep the C# implementation verbatim as the
   legacy path; never delete it. Enforced by `dotnet apicompat` in the sibling `mpex-api-compat` repo.
2. **Override contract.** Detect Harmony patches on the frozen members (`Harmony.GetPatchInfo`) and
   route to legacy so hooks fire with baseline semantics. Add a `forceLegacy...` config flag as the
   escape hatch for hooks detection can't see.
3. **Project per call, never cache.** Payloads are rebuilt from the live database, configs and
   services on every call — that is what keeps runtime mod mutations visible. Accept the cost. Two
   exceptions are in force, the ragfair and repeatable-quest invariant slices; see *Exceptions in
   force* for their terms.
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
adds `LocationConfig`, the four quest generators add `QuestConfig` +
`RepeatableQuestNativeRequestBuilder`, `ScavCaseRewardGenerator` adds `ScavCaseNativeRequestBuilder`,
`ItemBaseClassService` adds `ItemBaseClassNativeRequestBuilder` + `ItemConfig`. The
container selects the overload; anything built through the frozen 4.1.2 constructor gets a null
builder and runs legacy unconditionally. Ragfair needed no change — `RagfairConfig` was already
injected. `RepeatableQuestRewardGenerator` and `RepeatableQuestHelper` are folded in by their
callers, so both keep their frozen constructors untouched.

**Config flags.** `LocationConfig.ForceLegacyLootGeneration` covers *both* loot generators — there
is no per-generator flag. Elsewhere: `BotConfig.ForceLegacyBotGeneration` and `ForcePerBotGeneration`,
`RagfairConfig.ForceLegacyRagfairGeneration`, `QuestConfig.ForceLegacyRepeatableQuestGeneration`,
`ScavCaseConfig.ForceLegacyScavCaseGeneration`, `ItemConfig.ForceLegacyItemBaseClassHydration`, plus
each cache's `TrustNativeRequestCacheWithMods` / `DisableNativeRequestCache`. Only
`forceLegacyLootGeneration` is serialised into a shipped `.json` (`location.json`); the rest exist
as C# defaults and a user who wants one adds it to the file.

**What flips to legacy.** Loot flips only on a *protected* member patch. Every other family flips on
a patch of any public/protected/protected-internal member of its frozen set, **except** the
dispatcher entry point itself — a patch there wraps whichever path runs, by design. Frozen sets:
bots, the four generator classes; ragfair, `RagfairOfferGenerator`, `RagfairPriceService`,
`RagfairServerHelper`, `RagfairAssortGenerator`; quests, the four `*QuestGenerator`s plus
`RepeatableQuestRewardGenerator` and `RepeatableQuestHelper`; scav case and base class, their own
class only. A container-substituted subclass also flips — except for scav case, which checks no
substitution at all, so swapping `PresetHelper`, `RagfairPriceService`, `ItemFilterService`,
`SeasonalEventService` or `ItemHelper` under it is silently ignored. Bots additionally flip on an
`InventoryMagGenComponents` set that isn't exactly the four built-ins. `PickupQuestGenerator`
contributes **zero** frozen hookable members — its whole legacy body is inline in `Generate`.

**The bot wave batches before it iterates.** `BotController.GenerateBotWave` offers the wave to
`BotWaveBatcher.TryGenerateWave` first; the batcher returns null — and the unchanged per-bot path
runs — on `ForcePerBotGeneration`, on anything `BotInventoryGenerator.UseLegacyPath()` already
catches, on a patch of any frozen `BotGenerator`/`BotController` member except `GenerateBotWave`, on
a substituted `BotGenerator`, or on a wave that could write nighttime clamps (night raid **and** the
role's equipment config carries `NighttimeChanges.EquipmentModsModifiers`) — only the per-bot path
replays those. A `BotController` subclass on the frozen 14-parameter constructor gets a null batcher.
The response is one `{result | error}` envelope per bot in request order (ABI 8): a failed bot is
skipped with a Critical log and the rest of the wave still generates.

**State replayed after a native call**, because Rust keeps it to itself: bot container grid occupancy
(`RestoreContainerGrids`) and nighttime mod-chance clamps (`ReplayRandomisationClamps`, a cross-bot
feedback loop through `BotEquipmentFilterService`); ragfair's `rejectedCanSellTemplates`, which sets
`CanSellOnRagfair = false` on the live template table. The quest `QuestTypePool` round-trips and is
copied *into* the caller's instance (`CopyPoolInto`), not swapped — the controller keeps reading that
instance, so reference identity has to survive.

**The reward-loot blacklist crosses as two collections** — `configBlacklist` for the reward pool,
`globalBlacklist` for sealed-container filters. They differ once a mod calls
`AddItemToBlacklistCache` at runtime; collapsing them would change behaviour.

**Loose loot has two input paths.** Null `dynamicLootDist` splices `looseLoot.json`'s raw bytes in
unparsed (faster, more faithful); a registered `LazyLoad` transformer (seasonal events, mods) forces
the typed path instead, which is slower than both the raw path and the C# it replaced. A mod can
therefore put a server on the slow path without saying so.

**The ragfair batch walk is parallel only when unseeded.** An unseeded walk fans across rayon
(`1.12.0`): a forked `RagfairContext` per assort entry, results merged back in assort order with
`intId` reassigned during the merge. A **seeded** walk stays sequential — the seeded RNG is
`thread_local` — and all `RagfairParityTests` cases set a seed, so parity rides the unchanged path.
Production is unseeded on both arms, so the parallel path breaks no promise.

**The ragfair response is a framed MessagePack envelope, not a JSON buffer** — one length-prefixed
frame per offer behind a header frame (since ABI 10, encoding tag 1), deserialised with
`Parallel.For` straight out of the native buffer, so no whole-response JSON document is ever
materialised. Ragfair is the only export that uses it. Its batch also takes **one timestamp**, where
legacy calls `TimeUtil.GetTimeStamp()` per offer, so `startTime` is uniform across a batch.

**Ragfair and quests each cache an invariant slice natively — the only two exceptions to guideline
3.** Separate native caches (`src/quest/slice_cache.rs` for quests) keyed on the same
`DatabaseMutationStamp`, a monotonic counter bumped from `SeasonalEventService.UpdateGlobalEvents`,
`ItemFilterService`'s two blacklist `Add*` methods, `CustomItemService`'s two `Create*` methods and a
guarded replay bump when `CanSellOnRagfair` flips true→false. The request is
`{invariantStamp, invariant?, varying}` (ragfair since ABI 13, quests since ABI 14); a slice-less
request whose stamp the cache does not hold returns `STATUS_STALE_SLICE` (4), surfacing as
`NativeStaleSliceException` and self-healing with one full-send retry, so a lost cache costs one
pass's projection and never a wrong result. **A mod writing an injected table's dictionaries
directly is invisible to the stamp by design** — the eligibility gate carries that weight instead:
the cache is used only when no mods are loaded, with `TrustNativeRequestCacheWithMods` as the opt-in
and `DisableNativeRequestCache` as the kill switch. A modded server therefore full-sends every call.
**Known ceiling:** runtime *config* edits are slice inputs the stamp does not watch. No production
path writes config at runtime today; test fixtures bump the stamp manually. Instrument the config
objects if that changes. Every other payload in the crate, scav case and the base-class hydrate
included, is still projected per call.

**The ported 4.1.2 quirks are documented at their call sites**, as numbered `Quirk N` comments in
`rust/spt-native/src/quest/*.rs`, `src/scav_case/generator.rs` and `src/base_class.rs`
(`quest/helper.rs:169` carries an unnumbered one). Grep case-insensitively for `quirk` in those
paths — that turns up base-class quirks 2, 3 and 5 only: quirk 1 is on the C# side (hydrate resets
the cache dictionary but never `_rootNodeIds`, so the native arm unions the response's root ids into
the existing set rather than replacing it) and quirk 4 has no code site, its quirk being an
unreachable error path. The behaviour these preserve is deliberate; reverting one silently diverges
from C#. The bare `:N` line numbers in those comments are the 4.1.2 body the port was written
against, not the current file — where a native seam was inserted above a retained legacy body, the
C# line has since moved down.

## Roadmap

1. Next candidates and their costing live in [todo/TODO.md](todo/TODO.md); `RagfairLinkedItemService`
   is the first unstarted item.
2. Convert `is_valid_reward_item`'s trader whitelist to the set form and measure.
   `quest/reward_generator.rs` builds it as a `Vec<&str>` of up to 14 candidates, consumed 4,673
   times per Completion pass; `ItemBaseClassCache::is_of_baseclasses_set` is the cheaper shape once
   a candidate list is long enough, and 14 is the point where that is worth checking rather than
   assuming. Narrow and unmeasured.
