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
too, and has no legacy path: `SPTLoggerDispatcher` hands every line to the crate.

Twenty-two C-ABI exports (`src/ffi.rs`) carry all of it, JSON in and JSON out — except the ragfair
response, which is a framed MessagePack envelope, and `spt_log_emit`, which passes the fields of one
line directly (current ABI 21).

Native is not uniformly faster. Loot and repeatable quests win; bots, reward loot, ragfair, scav
case, the base-class hydrate and the linked-item table are slower than the C# they replace, and
native stays their default anyway — each case is argued where it is measured, in
[BENCHMARK.md](BENCHMARK.md), and each has a force-legacy flag for anyone who disagrees. Ragfair is
the one family that set itself a parity gate and **missed** it, with every in-scope lever spent.

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
| The whole log pipeline — filters, level gates, per-target formatting, console + file sinks | `SPTLoggerDispatcher.Log` | `spt_logger_init`, `spt_logger_reinit`, `spt_log_emit`, `spt_logger_close`, `spt_log_set_tap` |
| Generator diagnostics, localised and logged natively as they happen | `DatabaseImporter` → `SptNative.SetServerLocales` | `spt_locales_set` |

Also working: mod-added fields on game data survive the round trip (`#[serde(flatten)] extra` maps
mirroring Ceciler's `[JsonExtensionData]`); native generator diagnostics render and log themselves
through the native pipeline; seeded-RNG parity at the primitive level (xoshiro256\*\*, twin
known-answer tests both sides).

## Broken / known divergences

### Behaviour

- **Patches on collaborators do not reach the native path** and do not flip to legacy — only the
  ported classes' own members are detected. Affected: `RandomUtil`, `ItemHelper`,
  `CounterTrackerHelper`, `BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper`, `BotEquipmentModPoolService`, `BotLootCacheService`,
  `WeightedRandomHelper`, `ItemFilterService`/`PresetHelper` predicates, `ICloner`, plus ragfair's
  `HandbookHelper`, `PaymentHelper`, `BotHelper`, `TraderHelper`, `SeasonalEventService`, quests'
  `MathUtil` and scav case's `RagfairPriceService.GetStaticPriceForItem` and `HideoutTable` reads.
- **Templates without `_props` read as "not in the db"** on the native *generator* paths — they are
  dropped from `itemsView`. Only bites mod-added props-less templates. The base-class hydrate
  projects the whole table and is unaffected.
- **`customMoneyTpls` are not projected to the ragfair native path** — offers priced in a mod-added
  currency go through the unrounded arm.
- **The native ragfair and scav case paths are fresher than legacy for runtime-added items** — C#
  caches `AllowedFleaPriceItemsForBarter` (ragfair) and `DbItemsCache`/`DbAmmoItemsCache` (scav
  case) per generator instance and effectively never invalidates; Rust re-derives per call.
- **The item base-class and linked-item cache *keys* differ** — legacy stores under `item.Id`
  (`ItemBaseClassService.cs:194,199`; `RagfairLinkedItemService.cs:200`), native under the
  `templateTable.Items` dictionary key. Separable only by a mod filing a template under a key ≠ its
  `_id`, where legacy is the broken arm (consumers resolve by dictionary key).
- **Golden-test parity is normalised, not raw-byte.** Every family has a full-output golden gate
  (`*ParityTests` in `Testing/UnitTests`). Sanctioned gaps: minted `MongoId`s, and for ragfair
  `intId` and `startTime`/`endTime` (one batch timestamp natively vs a per-offer clock in legacy).
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
- **Console output is asynchronous and drops on a full queue** — an 8192-line bounded channel to a
  writer thread. A hard crash loses what is queued; a deeper burst drops rather than blocks.
- **Excluded categories still pay the per-line marshaling cost** — filtering moved native-side, so
  every line crosses the boundary before it is dropped.
- **Filter regexes are regex-lite** — no lookarounds, no backreferences, ASCII-only character
  classes. A pattern that will not compile is reported to stderr once and then never matches.
- **A native logging failure has no C# fallback** — a failed `spt_logger_init` means no logging at
  all for the run, and the same for a config the C# parser tolerated but Rust rejects. The known
  cases are handled except the `type` tag of a `loggers` entry, case-sensitive on both sides.
- **The pipeline reads `sptLogger.json` once; runtime mutation needs an explicit reload** — mutating
  `SptLoggerConfiguration.Loggers` changes what `IsLogEnabled` answers but not what is written.
  `SPTLoggerDispatcher.ReloadConfiguration()` (additive, post-port) re-hands the object to
  `spt_logger_reinit`; a rejected reload leaves the running pipeline untouched.
- **Line terminators are always `\n` and dates always Gregorian, culture-independent.**
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

## Guidelines

1. **Frozen surface.** Preserve the ported class's entire 4.1.2 public *and protected* surface —
   constructor including parameter names, methods, DTOs. Keep the C# implementation verbatim as the
   legacy path; never delete it. Enforced by `dotnet apicompat` in the sibling `mpex-api-compat` repo.
2. **Override contract.** Detect Harmony patches on the frozen members (`Harmony.GetPatchInfo`) and
   route to legacy so hooks fire with baseline semantics. Add a `forceLegacy...` config flag as the
   escape hatch for hooks detection can't see.
3. **Resident DB epoch, publish on dirty.** DB-derived state lives resident on the Rust side:
   `DbPublisher` republishes every supported root when the global `DatabaseMutationStamp` has moved
   and stamps the returned epoch into each request. Only the varying block — per-call service and
   config state — and the optional `viewsOverride` remain per-call. Ineligible callers (mods loaded
   without `TrustNativeRequestCacheWithMods`, or `DisableNativeRequestCache`) send the C#-built
   view bundle as `viewsOverride` on every call at today's projection cost, never touching resident
   state. Full protocol: the epoch-protocol section and its 2026-08-18 amendments in
   docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md. Quests still run the older
   stamp-keyed invariant-slice cache until flip #2; see *Exceptions in force*.
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
`ItemBaseClassService` adds `ItemBaseClassNativeRequestBuilder` + `ItemConfig`,
`RagfairLinkedItemService` adds `RagfairLinkedItemNativeRequestBuilder` + `RagfairConfig`. The
container selects the overload; anything built through the frozen 4.1.2 constructor gets a null
builder and runs legacy unconditionally. Ragfair offer generation, `RepeatableQuestRewardGenerator`
and `RepeatableQuestHelper` needed no change at all.

**Config flags.** `LocationConfig.ForceLegacyLootGeneration` covers *both* loot generators — there
is no per-generator flag. Elsewhere: `BotConfig.ForceLegacyBotGeneration` and `ForcePerBotGeneration`,
`RagfairConfig.ForceLegacyRagfairGeneration` and `ForceLegacyRagfairLinkedItemBuild`,
`QuestConfig.ForceLegacyRepeatableQuestGeneration`,
`ScavCaseConfig.ForceLegacyScavCaseGeneration`, `ItemConfig.ForceLegacyItemBaseClassHydration`, plus
each cache's `TrustNativeRequestCacheWithMods` / `DisableNativeRequestCache`. Only
`forceLegacyLootGeneration` is serialised into a shipped `.json` (`location.json`); the rest exist
as C# defaults and a user who wants one adds it to the file.

**What flips to legacy.** Loot flips only on a *protected* member patch. Every other family flips on
a patch of any public/protected/protected-internal member of its frozen set, **except** the
dispatcher entry point itself — a patch there wraps whichever path runs, by design. Frozen sets:
bots, the four generator classes; ragfair, `RagfairOfferGenerator`, `RagfairPriceService`,
`RagfairServerHelper`, `RagfairAssortGenerator`; quests, the four `*QuestGenerator`s plus
`RepeatableQuestRewardGenerator` and `RepeatableQuestHelper`; scav case, base class and the
linked-item table, their own class only. A container-substituted subclass also flips — except scav
case, which checks no substitution at all. Bots additionally flip on an `InventoryMagGenComponents`
set that isn't exactly the four built-ins. `PickupQuestGenerator` contributes **zero** frozen
hookable members — its whole legacy body is inline in `Generate`.

**The bot wave batches before it iterates.** `BotController.GenerateBotWave` offers the wave to
`BotWaveBatcher.TryGenerateWave` first; the batcher returns null — and the unchanged per-bot path
runs — on `ForcePerBotGeneration`, on anything `BotInventoryGenerator.UseLegacyPath()` already
catches, on a patch of any frozen `BotGenerator`/`BotController` member except `GenerateBotWave`, on
a substituted `BotGenerator`, or on a wave that could write nighttime clamps (only the per-bot path
replays those). The response is one `{result | error}` envelope per bot in request order (ABI 8): a
failed bot is skipped with a Critical log and the rest of the wave still generates.

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
therefore put a server on the slow path without saying so.

**The ragfair batch walk is parallel only when unseeded.** An unseeded walk fans across rayon: a
forked `RagfairContext` per assort entry, merged back in assort order with `intId` reassigned during
the merge. A **seeded** walk stays sequential (the seeded RNG is `thread_local`) and every
`RagfairParityTests` case sets a seed, so parity rides the unchanged path. Production is unseeded on
both arms.

**The ragfair response is a framed MessagePack envelope, not a JSON buffer** — one length-prefixed
frame per offer behind a header frame (since ABI 10, encoding tag 1), deserialised with
`Parallel.For` straight out of the native buffer. Ragfair is the only export that uses it. Its batch
also takes **one timestamp** where legacy calls `TimeUtil.GetTimeStamp()` per offer.

**Ragfair reads the resident DB; quests still cache an invariant slice — the two departures from
per-call projection (guideline 3).** Both key freshness on the same `DatabaseMutationStamp`, a
monotonic counter bumped from `SeasonalEventService.UpdateGlobalEvents`, `ItemFilterService`'s
blacklist `Add*` methods, `CustomItemService`'s `Create*` methods and a guarded replay bump when
`CanSellOnRagfair` flips true→false. **Ragfair (flip #1, ABI 22):** `DbPublisher` publishes the
templates, traders and globals roots into the resident store (`rust/spt-native/src/db.rs`), which
derives the ragfair views at publish time; each request carries the epoch, and an epoch the store
does not hold returns `STATUS_STALE_EPOCH` (4), surfacing as `NativeStaleEpochException` and
self-healing with one `ForcePublish` + retry — a lost epoch costs one republish, never a wrong
result. **Quests:** the older stamp-keyed slice cache (`{invariantStamp, invariant?, varying}`,
quests ABI 14) runs unchanged until flip #2 and shares status 4's value. **A mod writing an
injected table's dictionaries directly is invisible to the stamp by design** — the eligibility gate
carries that weight instead: resident state (and the quest cache) is trusted only when no mods are
loaded, with `TrustNativeRequestCacheWithMods` as the opt-in and `DisableNativeRequestCache` as the
kill switch; ineligible ragfair callers send a per-call `viewsOverride` instead (guideline 3).
Every other payload in the crate is still projected per call.

**Flip #1 ledger.** (a) Helper-cache freshness: legacy's hydrate-once caches — `TraderHelper`'s
trader prices, `HandbookHelper`'s handbook price lookup, `PresetHelper`'s preset store and
default-preset maps — could serve stale values into a rebuilt slice; Rust re-derives every view
from the published roots on each publish, so the resident path is uniformly *fresher*, never
staler, after runtime mutations — favours correctness, recorded here rather than "fixed". The
practical edge: a resident send and a `viewsOverride` send can diverge after a runtime mutation,
because the override is still built through those hydrate-once caches. (b) pmc name lists stay
C#-projected in the varying block; flip #6 (bots
root resident) is the named revisit point. (c) Runtime *config* edits still bypass the stamp — the
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

**The ported 4.1.2 quirks are documented at their call sites**, as numbered `Quirk N` comments in
`rust/spt-native/src/quest/*.rs`, `src/scav_case/generator.rs`, `src/base_class.rs` and
`src/linked_items.rs`; grep case-insensitively for `quirk`. Some numbers have no Rust site because
the quirk lives on the C# side (the base-class hydrate never resetting `_rootNodeIds`, the
linked-item dispatcher's copy loop and no-lock rule, the request builder's null-`Filter`-group
projection) or on no code at all. The behaviour these preserve is deliberate; reverting one silently
diverges from C#. The bare `:N` line numbers in those comments are the 4.1.2 body the port was
written against, not the current file.

## Roadmap

1. Next candidates and their costing live in [todo/TODO.md](todo/TODO.md); with #1 and #2 landed,
   the unstarted front is the tier-1 completeness trio (#4-#6) and tier 2.
2. Convert `is_valid_reward_item`'s trader whitelist (`quest/reward_generator.rs:869`, a `Vec<&str>`
   of up to 14 candidates) to `ItemBaseClassCache::is_of_baseclasses_set` and measure whether 14 is
   long enough for the set form to pay. Narrow and unmeasured.
