# Rust port — status and roadmap

What is ported to `rust/spt-native`, what is known-broken, and what comes next. For the crate's
internals see [rust/ARCHITECTURE.md](rust/ARCHITECTURE.md); for the C# side of the boundary see
[ARCHITECTURE.md](ARCHITECTURE.md) § *Native Rust layer*.

## Status

The loot family, the bot family and dynamic ragfair offer generation are ported and run natively by
default. Every ported class keeps its full 4.1.2 C# implementation as a **legacy path**, selected
automatically when a mod hooks it or manually via a config flag. Twelve C-ABI exports (`src/ffi.rs`)
carry it, all JSON in / JSON out.

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

Also working: mod-added fields on game data survive the round trip (`#[serde(flatten)] extra` maps
mirroring Ceciler's `[JsonExtensionData]`); native log lines are replayed through the C# logger;
seeded-RNG parity at the primitive level (xoshiro256\*\*, twin known-answer tests both sides).

## Broken / known divergences

- **PMC level 15+ bots diverge from legacy** — both armor and weapon mod pools. Cause:
  `BotEquipmentModPoolService`'s `ConcurrentDictionary` enumerates in hash-bucket order, Rust derives
  the pool in database slot order, so slots fill in a different order and every draw after the first
  differs. Only affects roles that set `randomisedArmorSlots`/`randomisedWeaponModSlots` (shipped:
  `pmc`, buckets 1-3, levels 15-100). Workaround: `ForceLegacyBotGeneration`.
- **Bot generation is tens of times slower per bot** — ~51-54 ms native vs ~1.2-1.4 ms legacy, for
  both measured roles (the 88 ms/54 ms split in earlier figures is measurement order, not role). The
  whole items table (~9.7k objects) is projected and serialised per bot, and ~92% of the cost is that
  transport, not generation — Rust generation itself is ~2.9 ms. The batched wave path amortises the
  shared block across the wave: against the `.AsParallel()` per-bot loop production runs, a
  single-threaded batch is worth ~1.7x at the median wave of 10 and ~2.4-2.5x at `assault`'s wave of
  45, and the batch loop is now rayon-parallel on top of that.
- **Reward loot is ~3x slower** — ~53 ms native vs ~17 ms legacy per `CreateRandomLoot`; same
  per-call items-view projection with only 15-35 items to amortise it against.
- **Ragfair generation is 3.4x slower on the full pass** — 1485 ms native vs 437 ms legacy, and
  8.8x on the expired-offer regeneration pass (95 ms vs 11 ms). Two roughly equal halves: ~713 ms of
  single-threaded Rust generation against legacy's 12-thread fan-out, and ~772 ms of wrapper —
  request serialisation, the FFI crossing and deserialising ~24k offers with full item trees. The
  payload projection is only 1% of the full pass, so trimming or caching it is *not* the lever
  here. Absolute cost is small (once at startup, then per-expiry bursts) and native stays the
  default for family consistency; `RagfairConfig.ForceLegacyRagfairGeneration` is the opt-out.
- **The native ragfair path is fresher than legacy for runtime-added items** — the C#
  `AllowedFleaPriceItemsForBarter` cache is built once per generator instance and never invalidated;
  Rust re-derives it per call. A divergence in native's favour, but a divergence.
- **`customMoneyTpls` are not projected to the ragfair native path** — offers priced in a mod-added
  currency go through the unrounded arm.
- **No RNG sequence parity between paths** — same seed, different output. The shared RNG pins the
  draw primitives, not the draw order. Full-output golden tests are still pending. Ragfair is the
  one exception: `RagfairParityTests` holds six item classes × two seeds byte-equal after id
  normalisation, plus forced barter and pack cases. Its sanctioned gaps are `intId` (a live C#
  counter), minted `MongoId`s (outside the seeded stream on both sides) and `startTime`/`endTime`
  (one batch timestamp natively vs a per-offer clock in legacy, so only the spread is compared).
- **Patches on collaborators do not reach the native path** and do not flip to legacy — only the
  ported classes' own members are detected. Affected: `RandomUtil`, `ItemHelper`,
  `CounterTrackerHelper`, `BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper`, `BotEquipmentModPoolService`, `BotLootCacheService`,
  `WeightedRandomHelper`, `ItemFilterService`/`PresetHelper` predicates, `ICloner`. Ragfair adds
  `HandbookHelper`, `PaymentHelper`, `BotHelper`, `TraderHelper` and `SeasonalEventService` to that
  list (its own four classes *are* detected — see *Exceptions in force*).
- **Templates without `_props` read as "not in the db"** on the native path — they are dropped from
  `itemsView`. Vanilla data always has `_props`; this only bites mod-added props-less templates.
- **Typed loose-loot path is slow** — ~1347 ms per raid start for `bigmap` vs ~345 ms raw, against
  929 ms for the C# it replaced. Any registered `LazyLoad` transformer (seasonal events, mods)
  forces it. Vanilla installs stay on the raw path.
- **Failures lose their diagnostics** — on error C# throws with the native message only; log lines
  collected before the failure are dropped (one-buffer FFI contract).
- **Hangs are undiagnosable** — ported retry loops can spin exactly as 4.1.2 does, but inside an FFI
  call with no managed stack trace. Force legacy to get it back.
- **Native bot logs collapse to one category** — all four generators log through
  `ISptLogger<BotInventoryGenerator>`.

## Guidelines

1. **Frozen surface.** Preserve the ported class's entire 4.1.2 public *and protected* surface —
   constructor including parameter names, methods, DTOs. Keep the C# implementation verbatim as the
   legacy path; never delete it. Enforced by `dotnet apicompat` in the sibling `mpex-api-compat` repo.
2. **Override contract.** Detect Harmony patches on the frozen members (`Harmony.GetPatchInfo`) and
   route to legacy so hooks fire with baseline semantics. Add a `forceLegacy...` config flag as the
   escape hatch for hooks detection can't see.
3. **Project per call, never cache.** Payloads are rebuilt from the live database, configs and
   services on every call — that is what keeps runtime mod mutations visible. Accept the cost.
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
   → `dotnet test` → `csharpier format .`.

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
- **The ragfair batch walk is sequential.** `GenerateDynamicOffersLegacy` fans one
  `Task.Factory.StartNew` per assort entry; the native side walks the batch on the calling thread.
  Deliberate, and the larger half of the performance loss above. Rayon (`1.12.0`) is in the crate,
  but only `generate_inventory_batch` uses it: `bots.into_par_iter()`, with a per-bot `TestSeedGuard`
  so seeded output stays deterministic per bot regardless of which worker takes it, and no effect on
  `PARKED_RNG` — the only consumer of a park is the loot dynamic entry point, which never runs on a
  rayon worker.
- **One timestamp per ragfair batch**, where legacy calls `TimeUtil.GetTimeStamp()` as each offer is
  built. `startTime` is therefore uniform across a native batch, and `endTime` is that timestamp
  plus the same per-offer random spread legacy draws.
- **One piece of state is replayed after the ragfair call**: `rejectedCanSellTemplates` sets
  `CanSellOnRagfair = false` on the live template table. The `AddOffer` insert loop, the holder's
  per-template cap and the `OfferCounter` advance all stay C#.

## Roadmap

1. ~~Mod-compatibility test gaps~~ — done, `ModCompatibilityTests` (`todo/TODO-TESTING.md`).
2. ~~Ragfair offer + price generation~~ — done, `.superpowers/sdd/2026-08-13-ragfair-port/`
   (`todo/TODO.md` #4). Native by default, 19/19 parity, but slower than legacy — see items 4a/4b.
3. ~~Shared invalidation-aware items-view cache~~ — **retracted.** There is no database mutation
   stamp, and mods mutate the database at runtime, so a cache cannot know when it is stale;
   rebuilding the projection per call is the only correct choice until such a stamp exists
   (guideline 3). It would also only have covered the ~20% build phase of the bot payload cost, and
   ~1% of a ragfair full pass.
4. The two ragfair performance levers, neither of which is item 3:
   - **a. Fan the batch walk out with rayon** — attacks the ~713 ms single-threaded generation half,
     against legacy's 12-thread ~405 ms.
   - **b. Slim the response** — the other ~772 ms is serialising, crossing and deserialising ~24k
     offers with full item trees. Both together are projected at ~1.6-1.8x, i.e. still short of the
     legacy path; ship them only if the startup and per-expiry bursts start to matter.
5. `ConcurrentDictionary` mod-pool ordering divergence — project the C# enumeration order across the
   FFI per item rather than emulating .NET's hash.
6. Full-output golden tests (same seed, bit-identical output vs the legacy path as oracle) for the
   loot and bot families; ragfair already has them (`RagfairParityTests`).
7. `checks.dat` generate path in Rust (`todo/TODO.md` #12) — detached quick win, drops
   `PostBuild.cs`'s `System.IO.Hashing` NuGet dependency.
8. Later candidates, in `todo/TODO.md` order: repeatable quests, scav case rewards, weather, fence
   assorts, raid-time adjustment, ragfair linked-item table.
