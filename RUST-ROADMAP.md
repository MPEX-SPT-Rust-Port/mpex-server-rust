# Rust port — status and roadmap

What is ported to `rust/spt-native`, what is known-broken, and what comes next. For the crate's
internals see [rust/ARCHITECTURE.md](rust/ARCHITECTURE.md); for the C# side of the boundary see
[ARCHITECTURE.md](ARCHITECTURE.md) § *Native Rust layer*.

## Status

The loot family, the bot family and dynamic ragfair offer generation are ported and run natively by
default. Every ported class keeps its full 4.1.2 C# implementation as a **legacy path**, selected
automatically when a mod hooks it or manually via a config flag. Twelve C-ABI exports (`src/ffi.rs`)
carry it, JSON in and JSON out — except the ragfair response, which is a framed MessagePack
envelope (current ABI 11) the C# side parses in parallel.

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

- **Randomised weapon-mod spawn rolls can desync native from legacy** — one side spawns a randomised
  weapon mod the other skips (e.g. `mod_mount_000` on the AK-74N), an RNG-stream desync in the
  randomised-mod draw path (suspect: the randomised `mod_magazine` draw-count path). Pre-existing —
  it was masked by the armor-plate ordering divergence (roadmap item 5) until that was fixed, and at
  HEAD the failing seed set is a strict subset of the pre-fix commit's. Only affects roles that set
  `randomisedArmorSlots`/`randomisedWeaponModSlots` (shipped: `pmc`, buckets 1-3, levels 15-100).
  Pinned by the `[Ignore]`d `BotParityTests.TheRemainingWeaponModSpawnDesyncIsPinned` (usec/bear at
  level 20, seed 42). Workaround: `ForceLegacyBotGeneration`.
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
- **`get_flea_prices_as_array` is O(offers × price table) if a mod enables barters** — it re-derives
  the whole filtered flea price list per barter offer, with an `is_of_baseclasses` walk per entry.
  Dead on shipped data (`ragfair.json` `dynamic.barter.chancePercent` is `0`, so no barter offer is
  ever rolled), but a mod that raises that percentage pays it on every barter offer of a ~58k-offer
  pass. Legacy avoids it by caching the list in `AllowedFleaPriceItemsForBarter` — the same cache
  that makes legacy stale (next bullet). Latent, not measured.
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
  frame per offer behind a header frame (since ABI **10**, encoding tag 1; current ABI is 11), which
  C# deserialises with `Parallel.For` over the frames straight out of the native buffer — no
  whole-response JSON document is ever materialised. Only the ragfair response uses it; every other
  export is still JSON in / JSON out.
- **One timestamp per ragfair batch**, where legacy calls `TimeUtil.GetTimeStamp()` as each offer is
  built. `startTime` is therefore uniform across a native batch, and `endTime` is that timestamp
  plus the same per-offer random spread legacy draws.
- **One piece of state is replayed after the ragfair call**: `rejectedCanSellTemplates` sets
  `CanSellOnRagfair = false` on the live template table. The `AddOffer` insert loop, the holder's
  per-template cap and the `OfferCounter` advance all stay C#.

## Roadmap

1. ~~Mod-compatibility test gaps~~ — done, `ModCompatibilityTests` (`todo/TODO-TESTING.md`).
2. ~~Ragfair offer + price generation~~ — done, `.superpowers/sdd/2026-08-13-ragfair-port/`
   (`todo/TODO.md` #4). Native by default, 19/19 parity, still slower than legacy — see item 4.
3. ~~Shared invalidation-aware items-view cache~~ — **retracted.** There is no database mutation
   stamp, and mods mutate the database at runtime, so a cache cannot know when it is stale;
   rebuilding the projection per call is the only correct choice until such a stamp exists
   (guideline 3). It would also only have covered the ~20% build phase of the bot payload cost, and
   ~1% of a ragfair full pass. **One exception, measured after the fact:** the ragfair
   *regeneration* pass spends ~60% of its time building and serialising the 5.8 MB call-invariant
   request slice, so it is the one caller where the cache would pay — see item 8, which still
   requires the stamp first.
4. ~~The two ragfair performance levers~~ — **done, gate missed.** Both shipped: the batch walk
   fans across rayon when unseeded, and the response is a framed MessagePack envelope (ABI 10)
   deserialised in parallel, plus a one-pass fresh-id offer copy. Full pass **1550 → 626 ms**
   (2.48x), regeneration ~103 → 75 ms, and native allocation now under legacy's (219 vs 283 MB).
   The 1.25x parity target was **missed at 1.47x** and the in-scope levers are exhausted; the
   residual is C#-side binding of ~367k `Models` objects out of the response, not generation
   (~85 ms) and not the projection (~13 ms). Figures and attribution: BENCHMARK.md § *Results —
   ragfair offer generation*, effort record `.superpowers/sdd/2026-08-14-ragfair-native-parity/`.
5. ~~`ConcurrentDictionary` mod-pool ordering divergence~~ — done: the C# enumeration order
   crosses the FFI as per-template slot indices (`modPoolSlotOrder`, ABI 11), and Rust's
   `derive_pool` draws in that order with database order as the total fallback. Level-15+ parity
   pinned by `BotParityTests` (usec/bear at level 20, seed 1337, plus the nighttime case now
   comparing full inventories at that seed). Seed 42 still fails on an unrelated, pre-existing
   weapon-mod spawn desync — first *Broken* bullet above. Spec:
   `docs/superpowers/specs/2026-08-15-mod-pool-order-projection-design.md`.
6. Full-output golden tests (same seed, bit-identical output vs the legacy path as oracle) for the
   loot and bot families; ragfair already has them (`RagfairParityTests`).
7. `checks.dat` generate path in Rust (`todo/TODO.md` #12) — detached quick win, drops
   `PostBuild.cs`'s `System.IO.Hashing` NuGet dependency.
8. **Database mutation stamp, then a cached serialized request slice** — the sanctioned way to
   reopen the request-side cache item 3 retracted, and the only lever left on the ragfair
   regeneration pass (~60% of it is building and serialising a 5.8 MB slice that does not change
   between calls). **The stamp is the precondition, and it is the hard half**: a counter every
   mod-facing database mutation path bumps, so a cache can tell staleness. Do not cache first and
   add the stamp later — that is exactly the incorrect cache guideline 3 forbids.
9. Later candidates, in `todo/TODO.md` order: repeatable quests, scav case rewards, weather, fence
   assorts, raid-time adjustment, ragfair linked-item table.
