# Rust port — status and roadmap

What is ported to `rust/spt-native`, what is known-broken, and what comes next. For the crate's
internals see [rust/ARCHITECTURE.md](rust/ARCHITECTURE.md); for the C# side of the boundary see
[ARCHITECTURE.md](ARCHITECTURE.md) § *Native Rust layer*.

## Status

The loot family, the bot family, dynamic ragfair offer generation, the repeatable-quest family and
scav case rewards are ported and run natively by default. Every ported class keeps its full 4.1.2 C#
implementation as a **legacy path**, selected automatically when a mod hooks it or manually via a
config flag. The log pipeline is ported too, and has no legacy path: `SPTLoggerDispatcher` hands
every line to the crate.
Eighteen C-ABI exports (`src/ffi.rs`) carry all of it, JSON in and JSON out — except the ragfair
response, which is a framed MessagePack envelope, and `spt_log_emit`, which passes the fields of one
line directly (current ABI 17).

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
| A whole bot wave in one call — shared views on the wire once, rayon-parallel per bot, one `{result \| error}` envelope each | `BotWaveBatcher.TryGenerateWave`, from `BotController.GenerateBotWave` | `spt_generate_bot_inventory_batch` |
| A batch of dynamic flea offers (assort walk, pricing, barter schemes) | `RagfairOfferGenerator.GenerateDynamicOffers` | `spt_generate_dynamic_offers` |
| Repeatable quests (all four types + rewards) | `*QuestGenerator.Generate` | `spt_generate_repeatable_quest` |
| Scav case rewards | `ScavCaseRewardGenerator.Generate` | `spt_generate_scav_case_rewards` |
| The whole log pipeline — filters, level gates, per-target formatting, console + file sinks | `SPTLoggerDispatcher.Log` | `spt_logger_init`, `spt_log_emit`, `spt_logger_close` |
| Generator diagnostics, localised and logged natively as they happen | `DatabaseImporter` → `SptNative.SetServerLocales` | `spt_locales_set` |

Also working: mod-added fields on game data survive the round trip (`#[serde(flatten)] extra` maps
mirroring Ceciler's `[JsonExtensionData]`); native generator diagnostics render and log themselves through
the native pipeline; seeded-RNG parity at the primitive level (xoshiro256\*\*, twin known-answer tests both sides).

## Broken / known divergences

- **Bot generation is tens of times slower per bot** — ~51-54 ms native vs ~1.2-1.4 ms legacy, for
  both measured roles (the 88 ms/54 ms split in earlier figures is measurement order, not role). The
  whole items table (~9.7k objects) is projected and serialised per bot, and ~92% of the cost is that
  transport, not generation — Rust generation itself is ~2.9 ms. The batched wave path amortises the
  shared block across the wave: against the `.AsParallel()` per-bot loop production runs, a
  batch measures ~1.5x at the median wave of 10 and ~1.9-2.2x at `assault`'s wave of 45. The batch
  loop is rayon-parallel, but that bought nothing measurable on top of the single-threaded batch —
  the win is amortising the shared block, and transport stays single-threaded on the C# side.
- **Reward loot is ~3x slower** — ~53 ms native vs ~17 ms legacy per `CreateRandomLoot`; same
  per-call items-view projection with only 15-35 items to amortise it against.
- **Ragfair generation is 1.47x slower on the full pass** — 626 ms native vs 427 ms legacy, and
  7.3x on the expired-offer regeneration pass (75 ms vs 10 ms). Down from 3.0x after the
  native-parity effort (1550 → 626 ms, a 2.48x improvement on the native path), but the 1.25x
  parity gate was **missed** and every in-scope lever is spent — see the ragfair section of
  BENCHMARK.md. What is left is neither generation (~85 ms) nor the payload projection (~13 ms,
  ~2%): it is the C# side binding ~367k `Models` objects out of the response, plus GC. Absolute
  cost is small (once at startup, then per-expiry bursts) and native stays the default for family
  consistency; `RagfairConfig.ForceLegacyRagfairGeneration` is the opt-out.
- **Completion quests are ~4.9x faster on the warm native path** — ~5 ms native at the median,
  from ~13 ms and a 1.7x before the ancestor cache landed. They were **3.1x slower** (67.50 ms)
  until the base-class walk in `GetWhitelistedItemSelection` was measured: C# tests all 137
  whitelisted candidates per item as O(1) hits on `ItemBaseClassService`'s prebuilt parent map, and
  the port kept that shape while answering each call with a fresh parent-chain walk. See
  [BENCHMARK.md](BENCHMARK.md) § *What still costs* and § *What the Completion figures used to be*
  for the numbers. `QuestConfig.ForceLegacyRepeatableQuestGeneration` is the opt-out.
- **Completion carries ~2.4 ms the other quest types do not**, down from ~10 ms.
  `ItemBaseClassService`'s prebuilt map is ported (`loot/item_helper.rs`'s `ItemBaseClassCache`,
  built once per cached invariant slice), but the ~10 ms was never the parent-chain *walk* — it was
  the linear scan of the candidate list at each link, and the cache alone moved nothing. The drop
  came from switching the two Completion whitelist/blacklist sites to the set-probing form. The
  residual is presumed to be `is_valid_reward_item`'s pass over all 4,673 templates, inferred
  rather than measured; see [BENCHMARK.md](BENCHMARK.md) § *What still costs*.
- **Exploration and Pickup quests are a wash** — a warm native call costs ~3.3 ms whatever it
  generates, and those two quests are ~3 ms of C# work, so native lands within a few tenths of a
  millisecond of legacy either way. The port pays on Elimination (4.7-6.3x) and Completion (~1.7x).
- **Scav case rewards are ~85x slower per call** — ~37.5 ms native against ~0.44 ms legacy, flat
  across the shipped recipes (the payload does not depend on the recipe), measured off the settled
  positions; the first two positions measured in a run have not settled on either arm. `Build()`
  alone is ~6.8 ms of that, and the rest is transport, serialisation and rebinding, not generation —
  the same items-view projection every family pays, against a 1-7 item output and a legacy arm that
  is sub-millisecond warm C#. It is a cold path — one call per finished scav case craft, behind a
  41-minute-to-5-hour timer — so it lands in the same acceptance class as reward loot: native stays
  the default, `ScavCaseConfig.ForceLegacyScavCaseGeneration` is the opt-out. See
  [BENCHMARK.md](BENCHMARK.md) § *Results — scav case rewards*.
- **An unknown scav case recipe id is an error natively, an NRE in legacy** — native returns
  `No scav case recipe found with id: <id>`; the C# body dereferences the missing recipe and throws
  at the same call site. A recipe whose `EndProducts` is missing any of the three ranges is
  *dropped* from the projection by `ScavCaseNativeRequestBuilder` (sending a null would fail the
  parse of the whole request), so asking for one gets that same "no recipe found" where legacy NREs.
  C# was never able to generate it either. Never fires on vanilla data.
- **An ammo pool with nothing in the rarity's price band is a message natively, an index throw in
  legacy** — both warn first, then native fails with
  `No cartridges found matching the price range for rarity: <rarity>` where the C# hands the empty
  sequence on and throws indexing it. Same divergence class as the recipe id above: the failure is
  the C#'s, only the text differs. Cannot fire on shipped data — all three rarity bands have ammo.
- **The native scav case path is fresher than legacy for runtime-added items** — C# fills
  `DbItemsCache`/`DbAmmoItemsCache` once per generator instance and refills them only when empty.
  The generator is transient, but its holder graph bottoms out in a singleton, so in practice one
  instance answers every craft for the life of the process; the native request rebuilds both pools
  per call. Same shape as the ragfair `AllowedFleaPriceItemsForBarter` divergence — in native's
  favour, but a divergence.
- **`get_flea_prices_as_array` is O(offers × price table) if a mod enables barters** — it re-derives
  the whole filtered flea price list per barter offer, with an ancestor-cache probe per entry.
  Dead on shipped data (`ragfair.json` `dynamic.barter.chancePercent` is `0`, so no barter offer is
  ever rolled), but a mod that raises that percentage pays it on every barter offer of a ~58k-offer
  pass. Legacy avoids it by caching the list in `AllowedFleaPriceItemsForBarter` — the same cache
  that makes legacy stale (next bullet). Latent, not measured.
- **The native ragfair path is fresher than legacy for runtime-added items** — the C#
  `AllowedFleaPriceItemsForBarter` cache is built once per generator instance and never invalidated;
  Rust re-derives it per call. A divergence in native's favour, but a divergence.
- **`customMoneyTpls` are not projected to the ragfair native path** — offers priced in a mod-added
  currency go through the unrounded arm.
- **Golden-test parity is normalised, not raw-byte** — every family now has a full-output golden
  gate (`LootParityTests` over all 13 loot-bearing maps, `BotParityTests` including level-20
  randomised buckets and the nighttime clamp, `RewardLootParityTests` over all five reward entry
  points, `RagfairParityTests`, `RepeatableQuestParityTests` over all four quest types,
  `ScavCaseParityTests` over every shipped recipe at two seeds), all seeded and byte-equal after id
  normalisation. The sanctioned gaps are minted `MongoId`s (outside the seeded stream on both sides —
  a repeatable quest mints ~12-25 of them, and a scav case mints from three sources: the reward
  roots, a preset's `ReplaceIDs`, and the cartridge children `ItemHelper.AddCartridgesToAmmoBox` adds
  (minted at `ItemHelper.cs:1516`); all masked by the normaliser) and, for ragfair only, `intId` (a
  live C# counter) and `startTime`/`endTime` (one batch timestamp natively vs a per-offer clock in
  legacy, so only the spread is compared).
- **Patches on collaborators do not reach the native path** and do not flip to legacy — only the
  ported classes' own members are detected. Affected: `RandomUtil`, `ItemHelper`,
  `CounterTrackerHelper`, `BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper`, `BotEquipmentModPoolService`, `BotLootCacheService`,
  `WeightedRandomHelper`, `ItemFilterService`/`PresetHelper` predicates, `ICloner`. Ragfair adds
  `HandbookHelper`, `PaymentHelper`, `BotHelper`, `TraderHelper` and `SeasonalEventService` to that
  list (its own four classes *are* detected — see *Exceptions in force*). Repeatable quests add
  `MathUtil`; their two folded-in collaborators, `RepeatableQuestHelper` and
  `RepeatableQuestRewardGenerator`, *are* detected — both their frozen members and a container
  substitution of either flip the calling generator to legacy. Scav case adds
  `RagfairPriceService.GetStaticPriceForItem` (already frozen *for ragfair*, but that scan flips
  ragfair, not this family) and the `HideoutTable` recipe reads; `RandomUtil`, `ItemHelper`,
  `PresetHelper`/`ItemFilterService`, `SeasonalEventService` and `ICloner` are already listed and
  apply to it too. Its seam checks no collaborator *substitution* at all — see
  *Exceptions in force*.
- **Templates without `_props` read as "not in the db"** on the native path — they are dropped from
  `itemsView`. Vanilla data always has `_props`; this only bites mod-added props-less templates.
- **Typed loose-loot path is slow** — ~1347 ms per raid start for `bigmap` vs ~345 ms raw, against
  929 ms for the C# it replaced. Any registered `LazyLoad` transformer (seasonal events, mods)
  forces it. Vanilla installs stay on the raw path.
- **A failure still crosses as a bare message** — everything the run logged on the way down is
  already in the log, emitted as it happened, but the error itself is the one thing the FFI hands
  back as text for C# to throw with (one-buffer contract). Localising and categorising that last
  line is what remains.
- **Hangs are mostly undiagnosable** — ported retry loops can spin exactly as 4.1.2 does, inside an
  FFI call with no managed stack trace. Generator diagnostics stream now, so a hang beside a
  diagnostic site shows its last line; a hang in a stretch with no diagnostic sites still shows
  nothing. Force legacy to get the managed stack back.
- **Generator lines carry one category per generator** — `typeof(T).FullName` of the C# class each
  Rust module ports, where the replay era logged the whole bot family through
  `ISptLogger<BotInventoryGenerator>`. A custom `sptLogger.json` filter written against that class
  now matches far fewer lines.
- **Generator lines use a different `%tid%` space** — a small process-local counter handed out per
  thread in first-emit order, not the managed thread id C# lines carry, and `%tname%` is the Rust
  thread name, usually empty. `%date%` is the moment of emission, where replayed lines were all
  stamped at the end of the native call.
- **Generator locale text is a startup snapshot** — `DatabaseImporter` pushes the resolved server
  locales once (`spt_locales_set`), so a mod mutating them later no longer changes what a generator
  line says. A failed push is one stderr notice and every generator line falls back to its locale key.
- **Parallel generator lines interleave** — the ragfair and bot rayon workers emit as they run, so
  lines no longer arrive grouped per bot or per assort entry. Each takes the global logger lock,
  which is fine at diagnostic rates.
- **Console output is now asynchronous and drops on a full queue** — the native pipeline hands each
  line to a writer thread behind an 8192-line bounded channel (file sinks always did; the console
  does now too). A hard crash can lose whatever is still queued, and a burst deeper than the queue
  drops lines rather than blocking the caller.
- **Filter regexes are regex-lite** — no lookarounds, no backreferences, ASCII-only character
  classes, against .NET's fuller `Regex`. A pattern that will not compile is reported to stderr once
  at startup and then never matches.
- **A native logging failure has no C# fallback** — the managed handlers are gone, so a failed
  `spt_logger_init` means one stderr notice and no logging at all for the run. A config the C#
  parser would have tolerated but Rust rejects fails the same way; the known cases (a UTF-8 BOM, and
  case-insensitive `logLevel`/filter `type`/`matchingType` values) are handled, the `type` tag of a
  `loggers` entry stays case-sensitive on both sides.
- **The pipeline snapshots `sptLogger.json` at startup** — mutating
  `SptLoggerConfiguration.Loggers` at runtime (a mod adding or retargeting a logger) no longer
  changes what is written, while `IsLogEnabled` still reads the mutated list, so the two can
  disagree. A re-init export is future work.
- **Excluded categories still pay the per-line marshaling cost** — filtering moved native-side, so
  every line is encoded, crosses the FFI boundary and takes the pipeline mutex before it is dropped.
- **Line terminators are always `\n` and dates always Gregorian, culture-independent** — the C#
  handlers used `Environment.NewLine` and `CurrentCulture`, so a Windows log file loses its `\r` and
  a non-Gregorian locale no longer shows in `%date%`.

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

- **`LootGenerator` took a constructor overload**, not a signature change — the frozen 11-parameter
  4.1.2 constructor stays, a 12th-parameter overload adding `LocationConfig` is what the container
  selects. Additive only.
- **One flag for the whole loot family.** `LocationConfig.ForceLegacyLootGeneration` forces both loot
  generators; there is no per-generator flag. Bots have their own,
  `BotConfig.ForceLegacyBotGeneration`.
- **Bot generation freezes a wider hook set.** Loot flips to legacy only on a *protected* member
  patch; bots flip on any public/protected/protected-internal member of all four classes except
  `GenerateInventory` itself — because only `GenerateInventory` is a dispatcher, the rest are never
  called natively and a patch on one would silently do nothing.
- **Bots also flip to legacy on non-patch conditions**: a subclass of `BotEquipmentModGenerator` /
  `BotWeaponGenerator` / `BotLootGenerator` from the container, or an `InventoryMagGenComponents`
  set that isn't exactly the four built-ins.
- **A wave is one native call before it is many.** `BotController.GenerateBotWave` offers the wave to
  `BotWaveBatcher.TryGenerateWave` first; the batcher returns null — and the unchanged per-bot path
  runs — on `BotConfig.ForcePerBotGeneration`, on anything `BotInventoryGenerator.UseLegacyPath()`
  already catches, on a live patch of any frozen member of `BotGenerator`/`BotController` except
  `GenerateBotWave`, on a container-substituted `BotGenerator`, or on a wave that could write
  nighttime clamps (night raid **and** the role's equipment config carries
  `NighttimeChanges.EquipmentModsModifiers` in some randomisation band) — that clamp is the cross-bot
  feedback loop below, and only the per-bot path replays it. A `BotController` subclass built on the
  frozen 14-parameter constructor gets a null batcher and never batches. The batch response is one
  `{result | error}` envelope per bot in request order (ABI **8**): a failed bot is skipped with a
  Critical log and the rest of the wave still generates.
- **Two pieces of state are replayed after the bot call** because Rust keeps them to itself:
  container grid occupancy (`RestoreContainerGrids`) and nighttime mod-chance clamps
  (`ReplayRandomisationClamps`, a cross-bot feedback loop through `BotEquipmentFilterService`).
- **The reward-loot blacklist crosses as two collections** — `configBlacklist` for the reward pool,
  `globalBlacklist` for sealed-container filters. They differ once a mod calls
  `AddItemToBlacklistCache` at runtime; collapsing them would change behaviour.
- **Loose loot has two input paths.** Null `dynamicLootDist` splices `looseLoot.json`'s raw bytes in
  unparsed (faster, more faithful); a registered `LazyLoad` transformer forces the typed path.
- **Ragfair freezes four classes and takes no constructor change.** Same shape as bots: a patch on
  any public/protected/protected-internal member of `RagfairOfferGenerator`, `RagfairPriceService`,
  `RagfairServerHelper` or `RagfairAssortGenerator` flips to legacy, except `GenerateDynamicOffers`
  itself, the dispatcher. A container-substituted subclass of any of the three collaborators flips
  too. `RagfairConfig.ForceLegacyRagfairGeneration` is the flag, and `RagfairConfig` was already an
  injected parameter, so nothing about the constructor moved.
- **The ragfair batch walk is parallel only when unseeded.** An unseeded walk fans across rayon
  (`1.12.0`): a forked `RagfairContext` per assort entry, results and diagnostics merged back in
  assort order with `intId` reassigned during the merge, so the counter is sequential regardless of
  which worker finished first. A **seeded** walk stays sequential — the seeded RNG is
  `thread_local`, and all 19 `RagfairParityTests` cases set a seed, so parity rides the unchanged
  path byte-for-byte. Production is unseeded on both paths (legacy fans one `Task.Factory.StartNew`
  per assort entry), so the parallel arm breaks no promise. Same rayon rules as the bot batch: no
  effect on `PARKED_RNG`, whose only consumer is the loot dynamic entry point, which never runs on
  a rayon worker.
- **The ragfair response is a framed MessagePack envelope, not a JSON buffer.** One length-prefixed
  frame per offer behind a header frame (since ABI **10**, encoding tag 1; current ABI is 17), which
  C# deserialises with `Parallel.For` over the frames straight out of the native buffer — no
  whole-response JSON document is ever materialised. Only the ragfair response uses it; every other
  export is still JSON in / JSON out.
- **One timestamp per ragfair batch**, where legacy calls `TimeUtil.GetTimeStamp()` as each offer is
  built. `startTime` is therefore uniform across a native batch, and `endTime` is that timestamp
  plus the same per-offer random spread legacy draws.
- **One piece of state is replayed after the ragfair call**: `rejectedCanSellTemplates` sets
  `CanSellOnRagfair = false` on the live template table. The `AddOffer` insert loop, the holder's
  per-template cap and the `OfferCounter` advance all stay C#.
- **The ragfair invariant slice is cached natively — the one exception to guideline 3.**
  `DatabaseMutationStamp` is a monotonic counter bumped from four instrumented mutation paths:
  `SeasonalEventService.UpdateGlobalEvents`, `ItemFilterService`'s two blacklist `Add*` methods,
  `CustomItemService`'s two `Create*` methods, plus a guarded replay bump in
  `RagfairOfferGenerator` when `CanSellOnRagfair` actually flips true→false. **A mod writing an
  injected table's dictionaries directly is invisible to the stamp by design** — instrumenting
  every write is not possible, so the eligibility gate carries that weight instead: the cache is
  used only when no mods are loaded, with `RagfairConfig.TrustNativeRequestCacheWithMods` as the
  opt-in for mod setups known not to mutate, and `RagfairConfig.DisableNativeRequestCache` as the
  kill switch. Every other payload in the crate is still projected per call.
- **The ragfair request is `{invariantStamp, invariant?, varying}` (since ABI 13).** The invariant half is
  sent only when the stamp moved; the native side keeps the parsed slice keyed by that stamp. A
  slice-less request whose stamp the cache does not hold returns `STATUS_STALE_SLICE` (4), which
  surfaces as `NativeStaleSliceException` and self-heals with one full-send retry — so a lost cache
  costs one pass's projection, never a wrong result. Warm regeneration pass: 11.46 ms against
  77.94 ms cold ([BENCHMARK.md](BENCHMARK.md) § *The slice cache*).
- **Known ceiling: runtime config edits are slice inputs the stamp does not watch.**
  `ragfairConfig.Dynamic` rides in the invariant slice, and nothing bumps the stamp when a config
  object is mutated. No production path writes it at runtime today; test fixtures that mutate
  config bump the stamp manually. Instrument the config objects if that ever changes.
- **Repeatable quests freeze six classes and dispatch from four.** The frozen set is the four
  dispatching generators — `EliminationQuestGenerator`, `CompletionQuestGenerator`,
  `ExplorationQuestGenerator`, `PickupQuestGenerator` — plus `RepeatableQuestRewardGenerator` and
  `RepeatableQuestHelper`, the two collaborators the native path folds in. A live patch on any
  public/protected/protected-internal member of any of the six flips to legacy, **except the four
  `Generate` methods**: each is a dispatcher now, so a patch on one wraps whichever path runs and
  does *not* force legacy. A container substitution — a subclass of the generator itself, of
  `RepeatableQuestHelper` or of `RepeatableQuestRewardGenerator` — flips too, and the check runs per
  generator instance at call time. `PickupQuestGenerator` contributes **zero** frozen hookable
  members: its whole legacy body is inline in `Generate`, so nothing of its own is patchable.
- **The four generators took constructor overloads**, not signature changes — the frozen 4.1.2
  constructors stay, and the container selects an overload adding `QuestConfig` and
  `RepeatableQuestNativeRequestBuilder` on each. Additive only, and a generator built through the
  frozen constructor has no native seam and runs legacy unconditionally.
  `RepeatableQuestRewardGenerator` and `RepeatableQuestHelper` are folded into the native path by
  their callers rather than dispatching themselves, so neither needed a new constructor: both keep
  their frozen 4.1.2 ones untouched.
- **Three `QuestConfig` flags, C# defaults only.** `ForceLegacyRepeatableQuestGeneration`,
  `TrustNativeRequestCacheWithMods` and `DisableNativeRequestCache` are not serialised into
  `quest.json` — same as the ragfair flags, they exist as defaults on the config object and a user
  who wants one adds it to the file.
- **The quest invariant slice is cached natively too, in its own slot.** Same terms as ragfair's and
  the same `DatabaseMutationStamp` key, but a **separate** native cache
  (`src/quest/slice_cache.rs`): the two families project different slices and move independently.
  Same eligibility gate — used only when no mods are loaded, with
  `QuestConfig.TrustNativeRequestCacheWithMods` as the opt-in and `QuestConfig.DisableNativeRequestCache`
  as the kill switch — and the **same ceiling**: a config edit without a table write does not bump
  the stamp. A modded (ineligible) server therefore full-sends every call, ~43 ms and ~10.6 MB of
  managed allocation per quest, the same figure for every quest type
  ([BENCHMARK.md](BENCHMARK.md) § *The slice, and what a C#-side memo could buy*).
- **The quest request is `{invariantStamp, invariant?, varying}` (since ABI 14)**, the same shape and the
  same status codes as ragfair's: `0` OK, `3` ERROR, `4` `STATUS_STALE_SLICE` — which surfaces as
  `NativeStaleSliceException` and self-heals with one full-send retry.
- **The `QuestTypePool` round-trips.** The generators consume the pool they are handed, so the
  mutated pool comes back in the response and is copied *into* the caller's instance
  (`CopyPoolInto`) rather than replacing it — the controller holds that instance and keeps reading
  it after the call, so reference identity has to survive.
- **The ported 4.1.2 quirks are documented at their call sites**, as numbered `Quirk N` comments in
  `rust/spt-native/src/quest/*.rs` (`reward_generator.rs` bolds them `**Quirk N, ported verbatim:**`;
  `elimination.rs` uses the plain form, and `helper.rs:161` carries an unnumbered
  `Ported quirk, not a typo`). `src/scav_case/generator.rs` numbers its own the same way. Grep
  case-insensitively for `quirk` under `src/quest/` and `src/scav_case/` to find all of them; the
  behaviour they preserve is deliberate and reverting one silently diverges from C#. The bare `:N`
  line numbers in those comments — quirks and ordinary citations alike — are the 4.1.2 body the port
  was written against, not the current file: where a native seam was inserted above the retained
  legacy body, the C# line has since moved down by the size of that seam.
- **Scav case took a constructor overload and freezes one class, its own.** The frozen 12-parameter
  4.1.2 constructor stays (as the primary constructor); the container selects an additive
  13-parameter overload adding `ScavCaseNativeRequestBuilder`. Additive only, and a generator built
  through the frozen constructor gets a null builder and runs legacy unconditionally. The hookable
  set is every declared public/protected member of `ScavCaseRewardGenerator` **except `Generate`**,
  the dispatcher — 12 members; a live patch on any of them, or a subclass of the generator from the
  container, flips to legacy. Same shape as the quest generators.
- **One `ScavCaseConfig` flag, C# default only.** `ForceLegacyScavCaseGeneration` is not serialised
  into `scavcase.json` — same as the ragfair and quest flags, it exists as a default on the config
  object and a user who wants it adds it to the file.
- **The scav case seam does not check for substituted collaborators**, unlike the quest one.
  `ScavCaseNativeRequestBuilder` projects what `PresetHelper`, `RagfairPriceService`,
  `ItemFilterService` and `SeasonalEventService` answer, and `ItemHelper`'s share of the work
  (`IsOfBaseclass`, `AddCartridgesToAmmoBox`) is ported natively rather than projected at all — so a
  container substitution of any of the five is silently ignored rather than flipping to legacy. The
  collaborator-patch divergence above, with substitution folded in.
- **Scav case caches nothing — guideline 3's default holds.** Every call reprojects the items view,
  the static price table, the default presets, the blacklists and the recipe table. The ragfair and
  quest invariant slices are still the only two exceptions in the crate.

## Roadmap

1. Later candidates, in `todo/TODO.md` order: weather, fence assorts, raid-time adjustment, ragfair
   linked-item table.
2. Re-scope the logging port's phase 3. Live emission made "`STATUS_ERROR` carries the run's
   accumulated diagnostics" moot; what is left is the error envelope itself — the message and its
   localisation.
3. Convert `is_valid_reward_item`'s trader whitelist to the set form and measure.
   `quest/reward_generator.rs` builds it as a `Vec<&str>` of up to 14 candidates, consumed 4,673
   times per Completion pass; `ItemBaseClassCache::is_of_baseclasses_set` is the cheaper shape once
   a candidate list is long enough, and 14 is the point where that is worth checking rather than
   assuming. Narrow and unmeasured — the "~10 ms" attribution this item used to carry was disproved
   (see *Broken / known divergences*, Completion).
