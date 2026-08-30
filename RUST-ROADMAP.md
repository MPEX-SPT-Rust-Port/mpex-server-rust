# Rust port — status and roadmap

What is ported to `rust/spt-native`, what is known-broken, and what comes next. Crate internals:
[rust/ARCHITECTURE.md](rust/ARCHITECTURE.md). The C# side of the boundary:
[ARCHITECTURE.md](ARCHITECTURE.md) § *Native Rust layer*. Measurements:
[BENCHMARK.md](BENCHMARK.md). History — the per-flip/phase decision ledgers and the PR ledger:
[RUST-LEDGER.md](RUST-LEDGER.md); decision numbers cited here resolve there.

Convention: this file cites **symbols** (`Type.Member`), never line numbers — line references churn
with every edit and go stale silently.

## Status

Ported and native by default: the loot family, the bot family and player scav generation, dynamic
ragfair offer generation, the repeatable-quest family, scav case rewards, the item base-class cache,
the ragfair linked-item table, map/raid setup (PMC wave splice included), achievement statistics and
weather generation. Each keeps its full 4.1.2 C# body as a **legacy path**, selected automatically
when a mod hooks it or manually via a config flag. The log pipeline and profile persistence are
ported with **no** legacy path; the crate owns the terminal outright (raw `Console.Write*`, prompts,
title, clear).

Forty-three C-ABI exports (`src/ffi.rs`) carry all of it, JSON in and JSON out — except the ragfair
response (framed MessagePack), `spt_db_load` and `spt_profile_load` (JSON header frame + file
bytes), and the log/console exports (raw fields or bytes). Current ABI 38. Since Phase 6b the
exports resolve from the `mpex-server` executable (rlib linkage; shipped Linux builds) or the
`spt_native` cdylib (dev builds, tests, Windows) — identical `[LibraryImport]` shape either way.

Native is not uniformly faster. Loot and repeatable quests win; bots, reward loot, ragfair, scav
case, the base-class hydrate and the linked-item table are slower than the C# they replace and stay
default anyway — each case argued where it is measured, in BENCHMARK.md, each with a force-legacy
flag. Ragfair set itself a parity gate and **missed** it; every lever short of further
state-ownership work is spent.

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
| Whole bot inventory | `BotInventoryGenerator.GenerateInventory` | `spt_generate_bot_inventory` |
| A whole bot wave in one call | `BotWaveBatcher.TryGenerateWave` | `spt_generate_bot_inventory_batch` |
| Player scav generation (karma chances/blacklist, inventory, additional loot) | `PlayerScavGenerator.Generate` | `spt_generate_player_scav` |
| Dynamic flea offers (assort walk, pricing, barters) | `RagfairOfferGenerator.GenerateDynamicOffers` | `spt_generate_dynamic_offers` |
| Repeatable quests (all four types + rewards) | `*QuestGenerator.Generate` | `spt_generate_repeatable_quest` |
| Scav case rewards | `ScavCaseRewardGenerator.Generate` | `spt_generate_scav_case_rewards` |
| Item base-class cache hydrate | `ItemBaseClassService.HydrateItemBaseClassCache` | `spt_build_item_base_class_cache` |
| Ragfair linked-item table | `RagfairLinkedItemService.BuildLinkedItemTable` | `spt_build_ragfair_linked_item_table` |
| Resident DB publish (6 roots + derived views) | `DbPublisher.EnsureCurrent` / `ForcePublish` | `spt_db_publish` |
| The whole log pipeline (filters, gates, formatting, sinks) | `SPTLoggerDispatcher.Log` | `spt_logger_init`, `spt_logger_reinit`, `spt_log_emit`, `spt_logger_close`, `spt_log_set_tap` |
| The terminal (redirected `Console.Write*`, prompts, title, clear) | `NativeConsoleWriter.Install`, `SptConsole` | `spt_console_write`, `spt_console_read_line`, `spt_console_set_title`, `spt_console_clear` |
| `IsLogEnabled` gate; mod `ILogHandler` line rendering | `SPTLoggerDispatcher.IsLogEnabled`, `BaseLogHandler.FormatMessage` | `spt_log_enabled`, `spt_log_format` |
| Generator diagnostics, localised natively | `DatabaseImporter` → `SptNative.SetServerLocales` | `spt_locales_set` |
| Profile persistence (list/read/write/delete under `user/profiles/`; serialization, MD5 dirty-check, `BackupService` stay C#) | `SaveServer.LoadAsync` / `LoadProfileAsync` / `SaveProfileAsync` / `RemoveProfile` | `spt_profile_list`, `spt_profile_load`, `spt_profile_save`, `spt_profile_delete` |
| Scav raid time adjustment | `RaidTimeAdjustmentService.GetRaidAdjustments` | `spt_get_raid_adjustments` |
| Map setup for a shortened raid (loot multipliers stay C#) | `RaidTimeAdjustmentService.MakeAdjustmentsToMap` | `spt_make_adjustments_to_map` |
| Raid start: hostility merges, scav extract append | `LocationLifecycleService.AdjustBotHostilitySettings` / `AdjustExtracts` | `spt_adjust_bot_hostility_settings`, `spt_adjust_extracts` |
| The PMC wave splice | `PmcWaveGenerator.ApplyWaveChangesToMap` | `spt_apply_pmc_wave_changes` |
| Achievement completion percentages | `AchievementController.GetAchievementStatics` | `spt_get_achievement_statistics` |
| One weather roll | `WeatherGenerator.GenerateWeather` | `spt_generate_weather` |

Also working: mod-added fields survive the round trip (`#[serde(flatten)] extra` maps mirroring
Ceciler's `[JsonExtensionData]`); seeded-RNG parity at the primitive level (xoshiro256\*\*, twin
known-answer tests both sides).

**`rust/spectre-facade`, the second workspace crate, is not a port** — a build-time generator
emitting a facade `Spectre.Console.Ansi.dll` (via `dotnetdll`) carrying just
`Spectre.Console.Color`, which the frozen 4.1.2 mod surface bakes into `ISptLogger<T>`,
`SptLogMessage`, `ClientLogRequest` and `Watermark.Draw`. Consequences: **`SPTarkov.Common` needs
the Rust toolchain too** (its `BuildSpectreFacade` target shells out to cargo), and the
`<Reference>` is non-transitive, so each of the five projects naming `Color` carries its own.
Cosmetic gaps: `FromInt32` returns Default, `ToString()` is inherited. Scope is `Color` only — mods
calling `AnsiConsole`, `Markup` or `Style` still break.

## Broken / known divergences

One bullet per divergence: symptom, who it bites, escape hatch. Deep forensics live in the ledger
entry cited.

### Behaviour

- **Patches on collaborators do not reach the native path** and do not flip to legacy — only the
  ported classes' own members are detected. Affected: `RandomUtil`, `ItemHelper`,
  `CounterTrackerHelper`, `BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper`, `BotLootCacheService`, `WeightedRandomHelper`,
  `ItemFilterService`/`PresetHelper` predicates, `ICloner`, ragfair's `HandbookHelper`,
  `PaymentHelper`, `BotHelper`, `TraderHelper`, `SeasonalEventService`, quests' `MathUtil`, scav
  case's `RagfairPriceService.GetStaticPriceForItem` and `HideoutTable` reads. Exceptions:
  `SeasonalEventService.ChristmasEventEnabled`/`RemoveChristmasItemsFromBotInventory` are detected
  member-scoped by the wave batcher; `BotEquipmentModPoolService` is detected whole-type since
  ABI 32. Invisible in principle: constructor patches, and plain runtime calls into the public
  surface. `forceLegacy` is the standing escape hatch. Player scav generation adds
  `RandomUtil.GetChance100` and `BotGeneratorHelper.GenerateExtraPropertiesForItem` /
  `AddItemWithChildrenToEquipmentSlot` to the bypassed set — all three live only in the legacy
  additional-loot pass the native arm replaces. Two qualifications, neither a new bypass:
  `ItemFilterService.IsLootableItemBlacklisted` is still called for real on the native arm, through
  `BotGenerator.RemoveBlacklistedLootFromBotTemplateInternal` (its only caller on this path, run
  C#-side on both arms so patches fire), leaving only the pre-existing family-wide hole for per-item
  predicate patches on native generation draws; and `AdjustItemWeights` likewise runs C#-side on
  both arms, but natively against a clone of the loot template's `BotGeneration` rather than the
  merged base template, so a postfix reading its arguments sees a different receiver per arm without
  anything flipping.
- **The native and legacy bot paths draw mod slots in different orders at randomised levels**
  (since ABI 32) — different, not wrong, bots for one seed; only the native order is
  machine-independent. No cross-arm case can ever cover it (different draw order = different RNG
  consumption). Exact-output coverage at randomised levels is gone on both arms; the replacement is
  a smoke case (`BotParityTests.TheNativePathGeneratesAtRandomisedLevels`) plus the Rust-side
  golden below. → ledger: mod-pool ownership.
- **A null `Appearance.Head`/`Hands`/`Voice` map fails the whole batch request** where legacy NRE'd
  one bot. Those members carry `ArrayToObjectFactoryConverter`, so a null map serialises as `[]`,
  which the Rust request deserialise rejects — one bad template kills the wave instead of one bot.
  Mod-data-only: all 57 shipped bot type files were scanned and none has such a member. Escape
  hatches: `ForcePerBotGeneration` or `ForceLegacyBotGeneration`. → ledger: item-20 port.
- **A drawn body tpl absent from `templates.customization` throws on the legacy path and draws
  hands natively** — the `bodyTpl → handsTpl` derive view simply has no entry, so the batch arm
  falls through to the ordinary weighted hands draw where `SetBotAppearance`'s C# dictionary
  indexer raises `KeyNotFoundException`. Mod-reachable only (shipped bodies all resolve).
  → ledger: item-20 port.
- **An unknown dogtag side, or a side with no `default` band, errors the bot natively where legacy
  NREs** — `GetDogtagTplByGameVersionAndSide` ignores both `TryGetValue` results, so an unknown
  side dereferences a null `gameVersionWeights` and a missing `default` hands
  `GetWeightedValue(null)` a null list; Rust returns a per-bot error envelope instead (the bot is
  skipped with a Critical log, the wave survives). The **game-version** key itself is *not* a
  divergence: both arms fall back to `default` on a miss, identically. Reachable only by a mod
  editing `pmcConfig.DogtagSettings` or adding a non-PMC role to `BotConfig.BotRolesWithDogTags`.
  Note shipped `pmc.json` has no `standard` band at all, so `default` is already the live path for
  most PMCs on **both** arms. → ledger: item-20 port.
- **The exp-reward difficulty fallback logs nothing on the batch arm** — legacy writes a Debug line
  when a bot's difficulty is missing from `experience.reward` and it falls back to `normal`; the
  native arm takes the same band silently. Diagnostics only, no behaviour change.
  → ledger: item-20 port.
- **The player scav's additional-loot pass draws from a different stream on each arm** — legacy
  rolls `LootItemsToAddChancePercent` C#-side after the bot is built; natively the roll continues the
  Rust stream inside the same call. Same seed, different items; cross-arm field-for-field parity
  holds only with that dictionary empty (`PlayerScavParityTests` empties it and covers the loot with
  per-arm cases). The pass also moves *earlier* in `Inventory.Items`: additional loot now lands
  before anything `GenerateBotFinish` appends, inert on shipped data (`botRolesWithDogTags` carries
  no scav role) but a real order divergence for a mod that adds one. Escape hatch:
  `ForceLegacyPlayerScavGeneration`.
- **Native bot output is not reproducible across processes** — `MongoId.GetHashCode` is
  per-process-seeded, so every `Dictionary<MongoId, …>` the projection serialises enumerates
  process-randomly. No C#-side golden can pin bot output; the Rust-side
  `RESIDENT_BATCH_GOLDEN` (`flip6_bots_resident.rs`) does, exactly. The C#-side fix (sort the keyed
  dictionaries) is declined: server-wide behaviour change owing its own spec and parity gate.
  → ledger: mod-pool ownership.
- **Templates without `_props` read as "not in the db"** on the native generator paths (dropped
  from `itemsView`). Bites mod-added props-less templates only; the base-class hydrate is
  unaffected.
- **Native ragfair and scav case are fresher than legacy for runtime-added items** — C# caches per
  generator instance and never invalidates; Rust re-derives per publish (or per call on override
  sends).
- **Container mutations of a table or config are invisible to the resident-DB stamp.** The
  Ceciler write barriers cover property setters over 33 roots; what stays invisible:
  `Add`/`Remove`/indexer-set on collections; array and reflection writes; the four denied
  live-per-request types (`Item`, `BotBase`, `PmcDataRepeatableQuest`, `GenerationData`); open
  generics — `MinMax<T>` is never barriered, which reaches location limit bands, scav case's whole
  price/money band set, and the equipment graph's four `LevelRange`s (where an edited band's
  overlay entry additionally **drops silently**, killing that band's nighttime clamp feedback);
  `object?`-typed properties; and writes inside `WriteBarrier.Suppress()` scopes. **Config
  collections are the wider half** (`ItemConfig.Blacklist`/`RewardItemBlacklist`/`BossItems`,
  `LocationConfig.LooseLootBlacklist`, `SeasonalEventConfig.ChristmasContainerIds`,
  `BotConfig.ItemSpawnLimits`/`CurrencyStackSize`). **Since ABI 34 the whole `BotConfig.Equipment`
  graph is in the window too** (previously per-call fresh); a runtime-registered role is missing
  resident-side (equipment phase declines with a diagnostic, weapon path errors the bot); the one
  carve-out is `Randomisation[band].EquipmentMods`, riding the `liveEquipmentMods` overlay fresh on
  both arms. Root-level tracking collections were evaluated and declined; two mod-reachable
  container writes are closed by hand (`RagfairPriceService.ReplaceFleaBasePrices`,
  `CustomQuestService.CreateQuest`), bringing hand-written bump sites to eight. Known
  over-announcements: `UserBuild.Id`/`Name` (profile build saves dirty the stamp) and
  `LocationController`'s `mapBase.Loot = []` reset (one full republish per map per
  `client/locations` request — accepted). Escape hatches: `DisableNativeRequestCache` (per-call
  freshness), or `TrustNativeRequestCacheWithMods` off. → ledger: Phase 2, Phase 4, equipment
  split.
- **W3: one UNHEARD PMC permanently widens pocket-loot weights for every later PMC in the
  process.** `BotEquipmentFilterService.AdjustGenerationChances` aliases config `GenerationData`
  into the cloned template by reference and
  `BotGenerator.AddAdditionalPocketLootWeightsForUnheardBot` writes through it into the live
  config. Pre-existing, stamp-invisible, never read via resident state (the polluted cells cross
  fresh on the template wire); confined to PMC bands 0–1 on shipped data; path-dependent since the
  batch flip (a batched UNHEARD leaves the config clean). Root fix (break the alias) declined —
  server-wide change owing its own spec. → ledger: equipment split.
- **W5: loot-cache hydration writes back into the live bot config through the same alias** —
  `BotLootCacheService.GetGenerationWeights` returns the aliased whitelist, so empty-but-present
  shipped whitelists gain every matching tpl. Same disposition as W3. → ledger: equipment split.
- **Write barriers exist on Release and publish builds only** — Ceciler does not run in Debug, so
  a Debug server with mods always sends the views override; the trust flag is refused there.
- **Native database load requires comment-free JSON in the five resident-root files** (serde_json
  rejects comments `JsonUtil` skipped; trailing commas rejected both paths). Non-resident files
  keep the tolerance. Shipped data passes (`phase3_db_load.rs` gates it);
  `forceLegacyDatabaseImport` for hand-edited or mod-shipped trees. Related: a `database/` file
  whose whole body is `null` throws natively where the disk path carried on — unreachable shipped,
  same escape hatch.
- **The base-class and linked-item cache keys differ** — legacy stores under `item.Id`, native
  under the `templateTable.Items` dictionary key. Separable only by a mod filing a template under a
  key ≠ its `_id`, where legacy is the broken arm.
- **Golden-test parity is normalised, not raw-byte** (`*ParityTests`). Sanctioned gaps: minted
  `MongoId`s; ragfair `intId` and `startTime`/`endTime` (one batch timestamp vs per-offer clock).
- **The ragfair/quest views-equivalence gates are manual** (`[Explicit]` fixture + `#[ignore]`d
  Rust test), not part of `dotnet test`. Flip #6's
  `AResidentSendAndAnOverrideSendProduceIdenticalBotsFieldForField` is the in-test-run pattern to
  copy — it caught a real divergence the manual harness would have missed.
- **A Harmony patch on `FileUtil.WriteFileAsync`/`DeleteFile` no longer sees profile I/O** (the
  disk boundary is `spt_profile_*` since Phase 5). Patches on `SaveServer`'s own members still
  fire; `BackupService` copies stay C#. **No escape hatch**: both arms produce identical bytes, so
  a revert is a plain `git revert`.
- **Profile save/load cancellation is best-effort-before, never mid-flight**; atomicity unchanged
  (temp-then-rename). I/O failures throw `InvalidOperationException` where they threw
  `IOException`-family types — bites a mod with `catch (IOException)` around profile ops.
- **A `user/profiles/` that cannot be `stat`ped aborts startup loudly** instead of silently loading
  zero profiles (the 4.1.2 behaviour, which invited creating a new profile beside intact ones).
  Deliberate: a `chmod`-recoverable stop beats silent data-loss presentation.
- **Profile listing and load order are sorted** (was filesystem-dependent). Strictly more
  deterministic; nothing reads the order.
- **A native failure crosses as a message for C# to throw with** — no log category; panics carry
  their message (since ABI 18). **Hangs are mostly undiagnosable** — ported retry loops can spin
  inside an FFI call with no managed stack; force legacy to get the stack back.
- **`get_flea_prices_as_array` is O(offers × price table) if a mod enables barters** — dead on
  shipped data, latent, unmeasured.

Unreachable on shipped data, recorded because a mod could reach them: unknown scav recipe id or
empty ammo rarity band (native message, legacy NRE); parentless or cyclic hydrate chains (native
terminates, C# stores `{ MongoId.Empty }` or recurses); five malformed linked-item filter shapes
(native skips, legacy throws or drops camora ammo); the native `_type` test is
`eq_ignore_ascii_case` vs C#'s `OrdinalIgnoreCase`. The parity gates would catch any of them.

### Logging

- One category per generator (`typeof(T).FullName` of the ported class) — a filter written against
  the old whole-family category matches far fewer lines.
- `%tid%` is a process-local counter in first-emit order; `%tname%` is the Rust thread name
  (usually empty); `%date%` is the moment of emission, not end-of-call.
- Generator locale text is a startup snapshot (`spt_locales_set`); later locale mutations don't
  change it; a failed push falls lines back to their locale key.
- Parallel generator lines interleave (rayon workers emit as they run).
- Console output is asynchronous: one 8192-slot channel to a writer thread. Log-line bursts past
  the queue **drop**; raw bytes (and the drain before `spt_console_read_line`) **block**, so a
  prompt is never lost. A terminal that stops draining stalls managed `Console.Write` and makes
  `spt_logger_close` wait; shutdown is carved out (racing writes go straight to stdout). A hard
  crash loses the queue.
- Excluded categories still pay per-line marshaling (filtering is native-side).
- Filter regexes are regex-lite: no lookarounds, no backreferences, ASCII classes. An uncompilable
  pattern reports to stderr once and never matches.
- A failed `spt_logger_init` — or a config C# tolerated but Rust rejects — means no logging for the
  run. The `type` tag of a `loggers` entry is case-sensitive both sides.
- Config is read once; runtime mutation needs `SPTLoggerDispatcher.ReloadConfiguration()`
  (additive, post-port). `IsLogEnabled` answers from the *applied* configuration.
- Line terminators are always `\n`, dates always Gregorian and culture-independent — including
  `BaseLogHandler.FormatMessage`, so mod-handler timestamps now agree with the pipeline under any
  process culture.
- Rotation cascades: `.1` is the *most recent* archive and `spt.log` only holds the current run —
  the reverse of ZLogger's ascending roll. Lowering `maxRollingFiles` strands old high indices
  until deleted by hand (the `ponytail:` note in `log_sink.rs`).
- Mod `ILogHandler` routing is a hybrid tap: C#-originated lines fan out normally; Rust-originated
  lines arrive rendered with no `Exception` object. Register via
  `SPTLoggerDispatcher.RegisterHandler` from DI — constructor-injected handler sets are always
  empty.
- The fatal-error pause is "Press enter to exit..." (`spt_console_read_line`), not `ReadKey`. The
  two mod-load-failure pauses in `Program` still read with no prompt (pre-existing).
- `BaseLogHandler.FormatMessage` renders through `spt_log_format`: bare `{`/`}` and positional
  holes render literally instead of throwing; an unreachable native side degrades to the
  unformatted message. A `GetCompiledFormat` override no longer reaches it (both shipped reference
  types are `sealed`).
- `Console.Clear()`'s redirect guard became a Rust-side tty check; clear and title are queued
  ANSI/OSC escapes (VT enabled on Windows at init).
- A mod calling `Console.SetOut`/`SetError` — or setting `Console.OutputEncoding` — **silently
  un-wraps `NativeConsoleWriter`**, reverting to raw writes racing the pipeline, with zero test
  failures. No mechanical guard; the only warning is a comment in `Program.StartServer`.
- Non-string `Console.WriteLine(value)` overloads can tear (decomposed into two queue messages) —
  cosmetic; every in-tree call site uses the string overloads.

## Guidelines

1. **Frozen surface.** Preserve the ported class's entire 4.1.2 public *and protected* surface —
   constructors (parameter names included), methods, DTOs. Enforced by `dotnet apicompat` in the
   sibling `mpex-api-compat` repo. The surface is frozen unconditionally; the *body* is not: keep
   the C# implementation as legacy only where Rust cannot reliably replace it, i.e. where either
   (a) the two arms' observable output differs (deleting C# would strand something) or (b)
   something mod-visible needs the body as guideline 2's patch-routing target. Profile persistence
   dropped legacy (identical bytes); database import kept a flag (different artifacts); every
   generator family fails (b). A family keeping no legacy path argues it in its ledger entry.
   Gotchas: **post-baseline types are exempt as additions, not by visibility** (the rule is the
   type's age, not its directory — `BotWaveBatcher` lost a constructor parameter and passed clean);
   and `mpex-api-compat/ci/check-api-compat.sh` resolves its tool manifest from cwd — anywhere cwd
   does not persist, every assembly reports failed, which means no analysis ran. Invoke the tool
   directly when cwd is not guaranteed.
2. **Override contract.** Detect Harmony patches on the frozen members (`Harmony.GetPatchInfo`) and
   route to legacy so hooks fire with baseline semantics; add a `forceLegacy...` flag for hooks
   detection can't see. A port with no legacy path has no routing target and carries neither (log
   pipeline, profile persistence).
3. **Resident DB epoch, publish on dirty.** DB-derived state lives Rust-side; `DbPublisher`
   republishes the six roots when `DatabaseMutationStamp` moves and stamps the epoch into each
   request. Only the varying block (per-call service state + caller-selected inputs) and the
   optional `viewsOverride` are per-call. Ineligible callers — mods without trust, or
   `DisableNativeRequestCache` — send the C#-built views as `viewsOverride` every call. Eligibility
   and the stale-epoch self-heal live once in `Native/Db/ResidentDbDispatch`. Details below under
   *Exceptions in force*; full protocol in
   `docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md`.
4. **RNG parity.** Both sides draw through the shared xoshiro256\*\* source behind test-only seams
   (`Utils/RandomSource.cs` / `random_util.rs`), pinned by twin known-answer tests. Production C#
   randomness stays bit-for-bit unchanged.
5. **FFI envelopes are internal.** A C#↔Rust contract shipped in lockstep — change freely, bump
   `spt_native_abi_version` and `SptNative.ExpectedAbiVersion` together, and move the `ffi.rs`
   `abi_version_export_matches_crate_const` assertion with them. No third-party cdylib consumer is
   supported.
6. **Ports keep an `[Injectable]` entry point.** Static wrappers like `SptNative` only for
   startup-internal subsystems mods never touch; anything patchable calls Rust from a resolved
   service.
7. **Gate loop** (no CI): `dotnet build -c Release` → `mpex-api-compat/ci/check-api-compat.sh` →
   `dotnet test` → `csharpier format .` → `cd rust && cargo test && cargo fmt --check && cargo
   clippy --all-targets -- -D warnings`. Run `dotnet tool restore` against the mpex-api-compat
   manifest first and invoke the script with cwd inside that repo (guideline 1's gotcha).

### Recurring traps and practice

Each learned at review or debugging cost.

- **Parallel ABI bumps auto-merge silently.** Two branches bumping N→N+1 merge with no textual
  conflict. Resolve to max+1 and renumber all **five** sites: `lib.rs`,
  `SptNative.ExpectedAbiVersion`, the `ffi.rs` tripwire, `rust/ARCHITECTURE.md`'s "currently N",
  and this file's "Current ABI N". Happened four times. Post-merge grep is confirmation-only —
  ledger entries keep historical numbers legitimately.
- **Export counts are a second silent-merge surface, worse because nothing asserts them.** The
  count sits in prose across `ARCHITECTURE.md`, this file and `rust/ARCHITECTURE.md`; parallel
  branches write different numbers on adjacent lines and git merges both cleanly. Re-derive, never
  sum branch deltas: `grep -c '#\[unsafe(no_mangle)\]' rust/spt-native/src/ffi.rs` is ground truth;
  the two subset counts in `rust/ARCHITECTURE.md` move with it.
- **apicompat wants a Release candidate against `server/baseline-dlls`.** A Debug candidate reports
  ~50 false CP0002 errors (Ceciler only rewrites on Release); passing `server` as baseline dies in
  the same banner as a real break. The gate has been red on every branch since the Assets-project
  removal (the script still hardcodes `SPTarkov.Server.Assets`) — read per-assembly results, not
  the exit code.
- **The span JSON overload does not eat a BOM.** `DeserializeAsync<T>(Stream)` consumes a UTF-8
  BOM; `Deserialize(ReadOnlySpan<byte>, …)` throws. Every port moving a file read behind the FFI
  hits this — strip Rust-side with `db/load.rs`'s `strip_bom` (don't write a second one), pin with
  tests both sides.
- **Residency dispositions start from a writer sweep**, not from "is this a config value?". Grep
  production code for runtime writers of every candidate member — especially indexer/collection
  mutations no setter barrier sees — before lifting state. Two Phase 4 members were
  mis-dispositioned by skipping this.
- **The override contract covers call cadence, not just call existence.** A port that batches or
  re-times C# service calls must sweep every re-timed call and decide per call: member-scoped
  decline, or a documented carve-out (a whole-type decline on a broad service silently de-batches
  modded servers).
- **Out-of-enum config keys are decline holes.** `EftEnumConverter` parses undefined numeric enum
  values, so an `Enum.GetValues`-keyed native table is reachable by a config edit alone. Decline to
  legacy before the native arm runs (weather's `HasOutOfEnumPresetKey` is the precedent).
- **Write port specs from as-built code, not prior specs' prose.** Prior specs describe intent that
  implementation silently corrected; read the merged precedent files before pinning a convention.
- **A C# property declared non-nullable is not one, and every fixture hand-setting it hides that.**
  `BotGenerationDetails.GameVersion` is only assigned on PMC paths, so non-PMC native bot sends
  crossed a null into a non-defaulted Rust `String` and were rejected outright — invisible because
  every bot fixture in the suite set the field by hand. Projection code defaults it at the seam now
  (`BotPayloadProjection.BuildBotSlice`). When a projection reads a member, check what writes it in
  production, not what the fixtures write.
- **`[Injectable]` registers transient, so a test seam set on a resolved instance may be set on the
  wrong object.** Seeding `NativeTestSeed` on a generator the container just handed you does not
  reach the nested collaborator the object under test actually holds — resolve the nested instance
  off the object graph instead (`PlayerScavParityTests.NestedBotInventoryGenerator`).
- **Never link `spt-native` as both rlib and cdylib in one build** — two resident DBs and
  split-brain epochs. Publish layout, not structure, enforces single linkage today (Phase 6b
  ledger).

### Exceptions in force

**Constructors.** Every family took an additive overload, never a signature change. The container
selects the overload; anything built through the frozen constructor gets a null builder — or a null
`DbPublisher`, which `ResidentDbDispatch.Eligible` answers `false` to — and runs legacy or the
override arm unconditionally.

| Class | Overload adds |
|---|---|
| `LootGenerator` | `LocationConfig`; extended in place (post-baseline) with loaded-mod list + `DbPublisher` at flip #4 |
| `LocationLootGenerator` | loaded-mod list + `DbPublisher` (flip #4) |
| the four `*QuestGenerator`s | `QuestConfig` + `RepeatableQuestNativeRequestBuilder` |
| `ScavCaseRewardGenerator` | `ScavCaseNativeRequestBuilder`; a further overload for loaded-mod list + `DbPublisher` |
| `ItemBaseClassService` | `ItemBaseClassNativeRequestBuilder` + `ItemConfig` |
| `RagfairLinkedItemService` | `RagfairLinkedItemNativeRequestBuilder` + `RagfairConfig` |
| `RaidTimeAdjustmentService`, `LocationLifecycleService`, `PmcWaveGenerator` | `RaidNativeRequestBuilder` |
| `AchievementController` | `AchievementNativeRequestBuilder` |
| `WeatherGenerator` | `WeatherNativeRequestBuilder` |
| `BotInventoryGenerator` | loaded-mod list + `DbPublisher`, chaining the frozen primary |
| `PlayerScavGenerator` | `PlayerScavNativeRequestBuilder` + `BotInventoryGenerator` + `SeasonalEventService` + loaded-mod list + `DbPublisher`, chaining the frozen primary (the seasonal service is not a frozen parameter — the native arm's christmas strip needs it) |

`BotWaveBatcher` post-dates the baseline (primary constructor took the pair directly). Ragfair
offer generation, `RepeatableQuestRewardGenerator` and `RepeatableQuestHelper` needed no change.

**Config flags.** All C# defaults a user adds to the file to change, except
`forceLegacyLootGeneration` (serialised in shipped `location.json`) and
`TrustNativeRequestCacheWithMods` (defaults **on** — the flag a user adds to turn *off*).

| Flag | Home | Covers |
|---|---|---|
| `ForceLegacyLootGeneration` | `LocationConfig` | both loot generators (no per-generator flag) |
| `ForceLegacyBotGeneration`, `ForcePerBotGeneration` | `BotConfig` | bot family; batch opt-out |
| `ForceLegacyPlayerScavGeneration` | `PlayerScavConfig` | player scav |
| `ForceLegacyRagfairGeneration` | `RagfairConfig` | offer generation |
| `ForceLegacyRagfairLinkedItemBuild` | `RagfairConfig` | linked-item table |
| `ForceLegacyRepeatableQuestGeneration` | `QuestConfig` | quest family |
| `ForceLegacyScavCaseGeneration` | `ScavCaseConfig` | scav case |
| `ForceLegacyItemBaseClassHydration` | `ItemConfig` | base-class hydrate |
| `ForceLegacyRaidAdjustments` | `LocationConfig` | all five raid exports across three classes |
| `ForceLegacyAchievementStatistics` | `CoreConfig` | achievement statistics |
| `ForceLegacyWeatherGeneration` | `WeatherConfig` | weather |
| `ForceLegacyDatabaseImport` | `CoreConfig` | database load (not a generation path) |
| `TrustNativeRequestCacheWithMods` / `DisableNativeRequestCache` | `RagfairConfig`, `QuestConfig`, `ItemConfig`, `LocationConfig`, `ScavCaseConfig`, `BotConfig`, `PlayerScavConfig` | resident-DB eligibility (linked-item table reads `RagfairConfig`'s pair) |

**What flips to legacy.** General rule: a patch on any public/protected/protected-internal member
of the family's frozen set flips it — except the dispatcher entry point itself, which wraps
whichever path runs. A container-substituted subclass also flips, **except loot**, whose
`UseLegacyPath` ends at the patch scan.

| Family | Frozen set and specials |
|---|---|
| Loot | `LootGenerator` + `LocationLootGenerator`, **protected members only**; no subclass check |
| Bots | the four generator classes; also flips when the `InventoryMagGenComponents` set ≠ the four built-ins |
| Player scav | **nine members, member-scoped, across three classes** — `PlayerScavGenerator`'s `AddAdditionalLootToPlayerScavContainers`, `ConstructBotBaseTemplate`, `AdjustBotTemplateWithKarmaSpecificSettings`, `AdjustEquipmentWeights`, `AdjustWeaponModWeights`, `BlacklistEquipment`; `BotGenerator.GeneratePlayerScav` + `GenerateBot` (the native arm inlines their orchestration into the internal shell `GeneratePlayerScavNative`); and `BotInventoryGenerator.GenerateInventory`, which the bot family deliberately leaves out of its own set but the native pscav arm bypasses entirely. Chains `BotInventoryGenerator.UseLegacyPath()`, so anything de-nativing bot inventory de-natives the player scav with it, and checks **three** subclasses (`PlayerScavGenerator`, `BotInventoryGenerator` and `BotGenerator`). Outside the set, C#-side on both arms: `Generate` (the dispatcher), `AdjustItemWeights` and `GetKarmaLimitValuesByKey` (their output feeds C#-side loot-pool hydration), `GetScavStats`, `GetScavLevel`, `GetScavExperience`, `SetScavCooldownTimer` |
| Ragfair | `RagfairOfferGenerator`, `RagfairPriceService`, `RagfairServerHelper`, `RagfairAssortGenerator` |
| Quests | the four `*QuestGenerator`s + `RepeatableQuestRewardGenerator` + `RepeatableQuestHelper`; `PickupQuestGenerator` contributes zero members (its legacy body is inline in `Generate`) |
| Scav case, base class, linked items | own class only |
| Weather | **seventeen members across five classes** — `WeatherGenerator`'s `GetWeatherPresetWeightsBySeason`, `GenerateWeatherByPreset`, `GetWeatherWeightsByPreset`, `GetRaidTemperature`, `SetCurrentDateTime`; `Generate` + `CanHandle` on each of `SunnyPreset`/`CloudyPreset`/`RainyPreset`; `AbstractWeatherPreset`'s six draw helpers — because the native arm reimplements every one of those bodies. Also flips when the injected `IWeatherPreset` concrete-type set ≠ the three built-ins (substitution and patching are caught by different checks) |
| Map/raid setup | **family-wide and member-scoped**: `RaidTimeAdjustmentService`'s `GetMapSettings`, `AdjustWaves`, `AdjustPMCSpawns`, `GetExitAdjustments` + `LocationLifecycleService`'s `AdjustExtracts`, `AdjustBotHostilitySettings`, `IsSide`. One patch declines all five exports. `AdjustLootMultipliers` is deliberately outside (the carve-out — it runs C#-side on both arms); `ApplyWaveChangesToMap` joined at ABI 36 adding zero members. Member-scoping avoids de-nativing on `LocationLifecycleService`'s other 23 methods |
| Achievements | **no frozen-member scan at all** — nothing hookable is bypassed; flag + null builder + subclass check are the whole rule |

**The bot wave batches before it iterates.** `BotController.GenerateBotWave` offers the wave to
`BotWaveBatcher.TryGenerateWave` first; the batcher returns null (per-bot path runs) on
`ForcePerBotGeneration`, anything `UseLegacyPath()` catches, a patch of any frozen
`BotGenerator`/`BotLevelGenerator`/`BotEquipmentFilterService`/`BotController` member except
`GenerateBotWave`, a substituted instance of those three, or a wave that could write nighttime
clamps (only the per-bot path replays them). Response: one `{result | error}` envelope per bot in
request order; a failed bot is skipped with a Critical log.

**The wave's level draw is native and its template ships per level band** (ABI 22).
`GenerateBotLevel`/`ChooseBotLevel` are ported with no new export — the draw is the first act of
each bot's rayon task, and the drawn `level`/`exp` ride back for the caller to write before
`CacheBot` reads them. `GetRelativePmcBotLevelRange` stays C# (`levelGeneration`, PMC waves only;
non-PMC takes constant level 1 and draws nothing, keeping non-PMC seeded pins byte-identical).
Every level-dependent pre-call step is a pure band lookup, so the batcher splits the range at band
edges and runs the unchanged C# filter/strip/hydration once per band, shipping one
`templateVariants` entry per band (1-3 typical; always one `[1..1]` for non-PMC). Since ABI 38 the
prelude draws — exp-reward-for-kill, voice, health, skills, the PMC game-version/member-category
block, appearance — and `GenerateBotFinish`'s dogtag run natively inside the batch call too, in the
C# prelude's order between the level draw and the inventory, off the drawn band. Naming
(`BotNameService`, blocked on the cross-wave `UsedNameCache`) and the sim-pscav cluster stay C# on
every arm. Pool and price hydration
(`BotLootCacheService.GetLootFromCache`, `HandbookHelper.GetTemplatePrice`) run once per band and
are deliberately **not** in the decline set — economy mods patch them constantly and declining
would de-batch most modded servers. One fidelity note: `AddAdditionalPocketLootWeightsForUnheardBot`
applies with an `if let` where C# dereferences unguarded, so a template with no `pocketLoot` block
NREs per-bot and no-ops batched. Since ABI 38 **every** batch bot gains the prelude draws ahead of
its inventory, so the level/exp literals are the only cross-ABI-stable pins on this path — nothing
precedes the level draw, and a moved `(level, exp)` literal is a bug. Inventory pins repin at
ABI 38 for every role, PMC and non-PMC alike (ledger: item-20 port).

**State replayed after a native call** (Rust keeps it to itself): bot container grid occupancy
(`RestoreContainerGrids`) and nighttime clamps (`ReplayRandomisationClamps`); ragfair's
`rejectedCanSellTemplates` (sets `CanSellOnRagfair = false` on the live table); the quest
`QuestTypePool`, copied into the caller's instance (`CopyPoolInto`) so reference identity survives.

**The reward-loot blacklist is two collections and only one crosses** — `configBlacklist`
(`ItemConfig.Blacklist`, resident off `spt-item`) for the reward pool; `globalBlacklist`
(`ItemFilterService`'s mutable cache — service state) for sealed-container filters, riding every
send on both arms. They differ once a mod calls `AddItemToBlacklistCache` at runtime.

**Loose loot has two input paths.** Null `dynamicLootDist` splices `looseLoot.json`'s raw bytes
unparsed; a registered `LazyLoad` transformer (seasonal events, mods) forces the typed path, slower
than both the raw path and the C# it replaced — a mod can put a server on the slow path without
saying so. Loose loot never went resident: raw bytes resident would cost 549 MiB RSS (declined
twice; resident paths + on-demand read is the upgrade if ever wanted).

**The ragfair batch walk is parallel only when unseeded** (seeded RNG is `thread_local`; every
parity case seeds, so parity rides the sequential path; production is unseeded on both arms). Its
response is the one framed-MessagePack envelope, deserialised with `Parallel.For`, and takes one
batch timestamp where legacy calls `TimeUtil.GetTimeStamp()` per offer.

**Every ported generation family reads the resident DB**, keyed on `DatabaseMutationStamp` (Ceciler
barriers over 33 roots + eight hand-written bump sites: `SeasonalEventService.UpdateGlobalEvents`,
`ItemFilterService`'s two blacklist `Add*`, `CustomItemService`'s two `Create*`,
`CustomQuestService.CreateQuest`, `RagfairPriceService.ReplaceFleaBasePrices`, a guarded replay bump
on `CanSellOnRagfair` true→false). Requests are `{epoch, viewsOverride?, varying}`; an unknown
epoch returns `STATUS_STALE_EPOCH`, surfacing as `NativeStaleEpochException` and self-healing with
one `ForcePublish` + retry. Fourteen of the twenty-one generation exports ride the epoch (flips
#1–#6, plus player scav on the bot views); the raid five, achievement statistics and weather carry no
epoch at all. Ineligible callers send a
per-call `viewsOverride` with `epoch: 0` (documented wire contract, not runtime-enforced). Per
flip:

| Flip | ABI | What went resident |
|---|---|---|
| #1 ragfair, #2 quests | 22, 23 | templates, traders, globals, locations roots; both families' views derive at publish |
| #3 base-class + linked items | 24 | no varying block, no new roots — walk inputs derive from the templates root at request time |
| #4 loot (six exports) | 25 | the three statics (~19 MB) as typed lifts on the locations root; looseLoot stays a per-call splice, `staticAmmoDist` stays a parameter |
| #5 scav case | 26 | the `hideout` root (`production.scavRecipes` only); recipe views derive at request time |
| #6 bots (both exports) | 27 | no new root; `BotDbViews` derives at publish (embeds `RagfairDbViews`, adds `defaultPresetIdsByTpl` + `expTable`) |
| Phase 4 configs | 30 | the sixth root: all 28 configs by `Kind`; Rust lifts the ten stems it reads (`spt-item`, `spt-scavcase`, `spt-ragfair`, `spt-inventory`, `spt-quest`, `spt-location`, `spt-seasonalevents`, `spt-bot`, `spt-pmc`, `spt-repair`) |

For the bot family the service-state half of the varying block is now nearly vacuous (ABI 32/34):
`generatingPlayerLevel` and `isNightTime` (live reads), `liveEquipmentMods` (the one
barrier-invisible cell kept off the root), and the caller-selected `levelGeneration` /
`templateVariants`.

**Ported 4.1.2 quirks are documented at their call sites** as `Quirk N` comments — grep
case-insensitively for `quirk` under `rust/spt-native/src/`. The behaviour they preserve is
deliberate; reverting one silently diverges from C#. Bare `:N` line numbers in those comments are
the 4.1.2 body the port was written against, not the current file. Some numbers have no Rust site
(the quirk lives C#-side or on no code at all).

## Roadmap

State-ownership Phases 1 through 6b are complete ([RUST-LEDGER.md](RUST-LEDGER.md)). Open work:

1. **Port queue** — candidates and costing in [todo/TODO.md](todo/TODO.md); the unstarted front is
   tier 2. The axes are independent: a flip re-homes data for something already ported, a TODO item
   ports something new.
2. **Convert `is_valid_reward_item`'s trader whitelist** (`quest/reward_generator.rs`, a
   `Vec<&str>` of up to 14 candidates) to `ItemBaseClassCache::is_of_baseclasses_set` and measure
   whether 14 is long enough for the set form to pay. Narrow and unmeasured.

<!-- Item 3, "split BotConfig.Equipment", was delivered at ABI 34 (ledger § Equipment split). The
list is deliberately not renumbered: the Phase 5 ledger cites item 4 by number. This comment also
keeps the renderer numbering the item below as 4. -->

4. **Frame the profile save request** (named by Phase 5, not delivered with it). Removable costs in
   order worth chasing: the owned `Box<RawValue>` copy in `profile.rs` (a genuine extra full-size
   copy at peak), and `Utf8JsonWriter.WriteRawValue(string)`'s `chars × 3` transcode scratch.
   Handing the wrapper UTF-8 bytes takes the `ReadOnlySpan<byte>` overload; framing the request
   like the load response removes the `RawValue` copy. The price: `spt_profile_save` becomes the
   first export off the shared `run_generator_with` ladder — exactly why Phase 5 declined it
   inline.
5. **Pin or vendor libnethost.** `rust/mpex-server` builds with netcorehost's `nethost-download`
   feature — newest apphost pack on nuget.org, prereleases included, no pin, no checksum, not
   covered by `cargo build --locked` — so the shipped launcher's C library floats across clean
   builds. Do it per target before the first tagged release image.
6. **A real Windows run.** The native console and launcher arm are structurally reviewed but have
   never executed on Windows (export-table and RID-triple gaps: Phase 6b ledger).

<!-- Item 7, "player scav test-coverage follow-ups" (PR #23 final review), was delivered by
PR #24 with no ABI bump. All four bullets landed: `PlayerScavGenerator.UseLegacyPath`
now de-natives on a substituted `BotGenerator` too (two of the nine frozen members live there) and
its subclass-check comment no longer describes virtual dispatch C# does not permit;
`SptNativePlayerScavWireTests` pins `GeneratePlayerScavRequest`/`KarmaSettingsView` (the C# fixture
for Rust's `KarmaSettingsWire`) and
`BotPathDispatchTests` drops the masking `GameVersion = "standard"`; `PlayerScavResidentDbTests`
compares a resident send against the real `PlayerScavNativeRequestBuilder.BuildViewsOverride`
projection field-for-field; `PlayerScavHookLivenessTests` resolves its cross-type members as
`MethodBase` identities that throw on a stale name; and the smaller items are covered — non-zero
`Modifiers.Mod` through `AdjustWeaponModWeights` cross-arm, a positive control beside the
seed-fragile parity negatives, and per-arm pins on the shipped 3–27 additional-loot band that
`GetChance100` had never seen. The pscav response no longer echoes `container_grids` back across
FFI: it rides the existing `clearBotContainerCacheAfterGeneration` wire flag, so `RESIDENT_GOLDEN`
moved (548C92D6… → 78B74F37…) while the ABI did not. -->
