# Bot-family Rust port — design

**Date:** 2026-08-13
**Stage:** `todo/TODO.md` candidates #2 + #3, ported together as one stage (user decision:
combined scope, so the FFI boundary is one call per bot from day one).
**Classes ported:** `Generators/Bot/BotEquipmentModGenerator.cs` (1916 lines),
`Generators/Bot/BotInventoryGenerator.cs` (783), `Generators/Bot/BotWeaponGenerator.cs` (882),
`Generators/Loot/BotLootGenerator.cs` (895).
**Stays C#:** `Generators/Bot/BotGenerator.cs` (thin orchestrator per `todo/TODO.md`), all
services the payload projects.

Follows the Porting playbook (ARCHITECTURE.md): frozen 4.1.2 public+protected surface, legacy
path retained verbatim as the executable oracle, dual dispatch with hook detection, per-call
payload projection (no snapshots), lockstep FFI envelopes, manual gate loop.

## 1. Boundary — one native call per bot

The call graph is a strict chain with no outside production callers:

```
BotController.cs:287 / PlayerScavGenerator.cs:79
  └─ BotGenerator.GenerateBot (BotGenerator.cs:190)
       └─ BotInventoryGenerator.GenerateInventory   ← BotGenerator.cs:284, ONLY call site
            ├─ GenerateAndAddEquipmentToBot → BotEquipmentModGenerator.GenerateModsForEquipment
            ├─ GenerateAndAddWeaponsToBot → BotWeaponGenerator.GenerateRandomWeapon
            │                               / AddExtraMagazinesToInventory
            └─ BotLootGenerator.GenerateLoot → AddLooseWeaponsToInventorySlot
                                                └─ BotWeaponGenerator.GenerateRandomWeapon
```

The native cut is **`BotInventoryGenerator.GenerateInventory`**. One new export,
`spt_generate_bot_inventory`, runs the entire subtree — equipment, weapon + magazines, loot,
and all `BotEquipmentModGenerator` recursion — in Rust. This covers 100% of production traffic
for all four classes (verified: `GenerateInventory` at `BotGenerator.cs:284` is the only
production call site of `BotInventoryGenerator`; `GenerateLoot`, `GenerateRandomWeapon`,
`AddExtraMagazinesToInventory` are called only from inside the subtree).

`BotGenerator` steps interleave cleanly: template mutations (`FilterBotEquipment`, Christmas
strip, Unheard pocket weights, `GameVersion` set) all land **before** line 284 and are
therefore captured by projection; dogtag and inventory-id rewrite run **after** and operate on
the returned item list, both paths alike.

Mods calling any of the four classes' public methods directly still execute the retained
legacy C# — behavior unchanged.

## 2. Dispatch and override contract

Dispatch lives in `GenerateInventory` only. The legacy path runs when **any** of:

1. **`BotConfig.ForceLegacyBotGeneration`** — new flag (`bot.json` →
   `forceLegacyBotGeneration`), the bot family's own escape hatch pre-agreed in
   ARCHITECTURE.md ("revisit at the bot-family port, which will want its own flag anyway").
   `BotConfig` is already a constructor parameter of the dispatching class, so **no
   constructor changes anywhere** — no additive-overload trick needed this time.
2. **Harmony patch detected** (`Harmony.GetPatchInfo`) on any frozen member of any of the
   four classes **except `GenerateInventory` itself**. This is the user-approved *widened*
   hookable set: `BotEquipmentModGenerator` exposes 22 public helpers and only 4 protected
   members, so the loot port's protected-only detection would cover almost nothing. The
   hookable set is: all declared public and protected methods of the four classes minus the
   one dispatching entry point (protected *fields* like `InventoryMagGenComponents` cannot
   be patched — the DI-set check in item 4 covers that surface). Patches on
   `GenerateInventory` itself run *around* the native body — prefix and postfix fire on the
   dispatcher with real arguments and results, whichever path runs underneath (same contract
   as the loot generators' public entry points).
3. **`TypePriority` replacement detected**: any of the three injected sibling generators
   (`BotEquipmentModGenerator`, `BotWeaponGenerator`, `BotLootGenerator`) resolves to a
   runtime type other than the built-in concrete class. The native path would silently bypass
   a mod's subclass otherwise.
4. **Foreign `IInventoryMagGen`**: the DI-resolved `IEnumerable<IInventoryMagGen>` contains
   anything beyond the four built-in implementations (mods can register magazine strategies;
   Rust hardcodes the four built-ins).

Test seams copied from the loot generators: `internal LootGenerationPath LastPathTaken` and
`internal ulong? NativeTestSeed` on `BotInventoryGenerator`.

Checks 2–4 are computed per call (patch state and DI state are cheap to read; the loot port
already does per-call `GetPatchInfo`).

## 3. Payload

Envelopes are internal contracts (playbook rule 4): `Native/Bot/BotPayloads.cs` ↔
`rust/spt-native/src/bot/models.rs`, member for member, `#[serde(flatten)] extra` on
game-data types, shipped in lockstep. `ABI_VERSION` 4→5.

### Input (projected per call — no snapshots, playbook rule 2)

- `botId`, `testSeed?: u64`.
- `botGenerationDetails` flattened to scalars: `Role`, `RoleLowercase`, `Side`, `BotLevel`,
  `IsPmc`, `IsPlayerScav`, `GameVersion`, `Location`, `BotDifficulty`,
  `ClearBotContainerCacheAfterGeneration`.
- The already-cloned-and-filtered `BotType` template — **only** its `BotInventory`
  (equipment/mods/ammo pools), `BotChances`, and `BotGeneration` blocks (the
  appearance/experience/name blocks are not read below the boundary; on-disk templates are
  281–653 KB, the projected slice is well under half).
- **Hoisted live-state scalars**, resolved in C# before the call so the native side stays
  pure: `generatingPlayerLevel: int` (from `profileHelper.GetPmcProfile(sessionId)?.Info?.
  Level ?? 1` — the only save-server read in the subtree) and `isNightTime: bool` (from
  `weatherHelper.IsNightTime(raidConfig…)`, resolving the
  `ProfileActivityService.GetProfileActivityRaidData(sessionId)` read). `sessionId` therefore
  never crosses the FFI; it stays in the frozen C# signatures.
- Config slices: `BotConfig.Equipment[role]` (`EquipmentFilters` incl. plate weighting,
  forceStock, weaponSlotIdsToMakeRequired, `Randomisation`), `BotConfig.ItemSpawnLimits`,
  `WalletLoot`, `CurrencyStackSize`, `SecureContainerAmmoStackCount`, `DisableLootOnBotTypes`,
  `BotRolesWithDogTags`, `LowProfileGasBlockTpls`, `LootItemResourceRandomization`;
  `PmcConfig` (ForceArmband, LootSettings, LootItemLimitsRub, ForceHealingItemsIntoSecure,
  LooseWeapon*, AddSecureContainerLootFromBotConfig, WeaponHasEnhancementChancePercent);
  `RepairConfig.RepairKit.Weapon`.
- Collaborator projections:
  - `BotEquipmentFilterService.GetBotEquipmentBlacklist(role, level)` result and
    `GetBotWeaponSightWhitelist` map.
  - The 13 resolved `BotLootCacheService` pools for this role, post price-filter. This
    deliberately snapshots the ragfair-dynamic-priced PMC weights **as the legacy path sees
    them** (the cache is lazily hydrated once per role from `PMCLootGenerator` +
    `RagfairPriceService` and cloned on read today) — ragfair pricing is not ported.
  - `ItemFilterService.GetBlacklistedItems()` (existing reward-loot projection reused).
  - `PresetHelper` default-preset map (existing projection reused) plus the two
    by-id preset lookups `GetPreset(id)` needs.
  - `GlobalTable.ItemPresets` for the weapon-preset fallback.
  - A handbook price map (`HandbookHelper.GetTemplatePrice`) restricted to tpls present in
    the projected loot pools, for the rouble-budget check.
  - The shared **items view** (`PayloadProjection`, same as both loot ports).
  - `BotEquipmentModPoolService` is **not** marshaled: its gear/weapon mod pools are pure
    derivations of the item table, so Rust derives them from the items view on demand
    (replacing the C# lazy cache). `ResetWeaponPool()`/cache state therefore cannot desync
    the native path — it re-derives from live-projected data every call.

### Output (round-trips everything the C# subtree mutates)

- The generated `BotBaseInventory` (items + the six container ids).
- Diagnostics array, replayed through the existing `PayloadProjection.ReplayDiagnostics`.
- Final container-occupancy grids. When `ClearBotContainerCacheAfterGeneration == false`
  (player-scav path), C# writes them back into `BotInventoryContainerService`, because
  `PlayerScavGenerator.cs:151-188` reads those grids **after** generation returns. When the
  flag is true, the dispatcher clears/ignores them exactly as the legacy path's
  `ClearCache(botId)` does.
- The clamped per-slot `Randomisation.EquipmentMods` values. **Bug-for-bug requirement:**
  `BotInventoryGenerator.cs:204` writes the night-time clamp into the *live*
  `BotConfig.Equipment[role].Randomisation` object (`BotHelper.GetBotRandomizationDetails`
  returns the config reference, not a clone), so the clamps accumulate across bots and raids.
  The dispatcher replays the returned values into the same live object so both paths corrupt
  config identically.

Note on mutable in/out arguments inside the subtree (`ModSpawnChances`, `ModPool`,
`ConflictingItemTpls`, `WeaponStats`, `ModLimits` counters): with the boundary at
`GenerateInventory`, all of these are interior to the native call and never cross the FFI —
they were the reason a per-method boundary for #2 alone would have been so expensive.

## 4. Rust side

New module family `rust/spt-native/src/bot/`, C# nomenclature in Rust casing (CLAUDE.md
style rule):

- `bot_inventory_generator.rs`, `bot_weapon_generator.rs`, `bot_loot_generator.rs`,
  `bot_equipment_mod_generator.rs` — the four ported classes.
- `bot_generator_helper.rs` — `GenerateExtraPropertiesForItem` (durability/resource `Upd`
  rolls, light/laser/NVG toggles; RNG-heavy, must be in-stream),
  `IsItemIncompatibleWithCurrentItems`, `AddItemWithChildrenToEquipmentSlot` +
  container-occupancy grids (Rust owns the grids for the duration of the call).
- `durability_limits_helper.rs`; `repair_service.rs` (`AddBuff` only);
  `bot_weapon_generator_helper.rs` (`GetRandomizedMagazineCount`, `GetRandomizedBulletCount`,
  `CreateMagazineWithAmmo`, `MagazineIsCylinderRelated`).
- `inventory_mag_gen.rs` — the four built-in `IInventoryMagGen` processors, dispatched in the
  same priority order `MagGenSetUp` produces.
- `exhaustable_array.rs` — draw-without-replacement twin preserving C#'s
  `LinkedList` removal order and `GetInt(0, count-1)` indexing.
- Mod-pool derivation (`BotEquipmentModPoolService`'s `GetModsForGearSlot`/
  `GetModsForWeaponSlot`/`GetRequiredModsForWeaponSlot`/`GetCompatibleModsForWeaponSlot`
  equivalents) computed from the items view.
- Wallet-loot grid placement (`GetContainerSlotMap`/`CanPlaceItemsInContainer`/
  `PlaceItemInContainer` equivalents; `container_extensions.rs` has related grid logic to
  build on).

Reused as-is from the loot family: `item_helper` (`get_item`, `is_of_baseclass(es)`,
`add_cartridges_to_ammo_box`, `fill_magazine_with_*`, `add_child_slot_items`, `replace_ids`,
`split_stack`, `set_found_in_raid`), `mongo_id`, `random_util` (+`TestSeedGuard`),
`probability_object_array` (not used by this family, but no change needed), the items-view
model, and the `run_generator` FFI shim.

New `random_util` primitives, each pinned by twin KATs (C# `RandomSourceParityTests`-style +
Rust KAT):

- `roll_chance` — `RandomUtil.RollChance` (RandomUtil.cs:498) is an **inclusive 1–100 roll,
  deliberately distinct from `get_chance_100`** (exclusive 1–99). Do not alias.
- `get_percent_of_value` (used by resource randomization and `AddBuff`).
- `reduce_value_by_percent` (armor durability paths).
- Key-generalized `get_weighted_value`: call sites draw string-, double-, and MongoId-keyed
  dictionaries (`BotLootGenerator.cs:97` draws numeric count keys; `:625` draws a string
  stack count then parses it). Insertion order must be preserved per key type — the existing
  `IndexMap<String, f64>` twin generalizes or gains per-call-site conversions, whichever
  keeps the draw sequence bit-identical.

FFI: one export (`spt_generate_bot_inventory`, ~12 lines through `run_generator`), one
`[LibraryImport]` line, one `LootExport`-style enum member + switch arm, `ABI_VERSION` 4→5
with the `ffi.rs` pin test and `ExpectedAbiVersion` updated in lockstep.

## 5. Parity — bug-for-bug

Legacy path is the oracle (playbook rule 3). The catalogued quirks the port must replicate,
by class:

**BotEquipmentModGenerator** — intentional NRE at `:270` (`modTpl.Value` deref when pool
empty + slot Required); double-negative flow `:268/:275`; **disabled `SlotBlocked` branch**
`:1281` (commented out on purpose — `SlotBlocked` only reachable via
`IsItemIncompatibleWithCurrentItems`); `blockedAttemptCount` reset-before-break `:1243/:1280`
with `Math.Round(count * 0.75)` and `>` comparison; `ModSlotCanHoldMuzzleDevices` **ignores
its `modsParentId` parameter** `:847` (unlike its scope twin `:808`); `ShouldForceSubStockSlots`
NRE on roles missing from `botConfig.Equipment` `:662`; lazy-LINQ re-enumeration in
`FilterPlateModsForSlotByLevel` (`:363-:448`, repeated db lookups, `ArmorClass.Value` NRE on
props-less templates); post-draw armor-level clamp `:353` (draw consumed even when clamped);
3-attempt plate-class wraparound loop `:386-:444`; inverted gas-block filters between the
optic/iron-sight branches `:1063/:1072` (deliberate); cylinder magazines bypass recursion via
`FillCamora`, which clones **one** ammo tpl into every slot including non-camora slots
`:1800-:1813`; ammo containers short-circuit to `request.AmmoTpl` **except `mod_magazine`**
`:1028`; the `OrderBy` front/back-plate sort is stable so residual order is dictionary
insertion order `:115-:130`; unguarded `Filters.First()` `:1204`.

**BotInventoryGenerator** — `RootEquipmentPool?.Count != 0` is true for a *null* pool, then
`:516` NREs; the `while (!found)` loop `:518` only checks `attempts > maxAttempts` in the
incompatible branch, so a fully-broken pool loops until pool exhaustion `:520-:541`;
`FilterRigsToThoseWithoutProtection`'s only call site passes `allowEmptyResult = true`, so a
bot with zero unarmored rigs gets an emptied vest pool feeding the previous quirk;
`GetDesiredWeaponsForBot` `:718-:730` — primary rolled first, second-primary short-circuited
by `shouldSpawnPrimary &&` (roll *not consumed* when false), holster short-circuited the
other way; template/chance mutations (`wornItemChances["TacticalVest"] = 100` `:394`,
`Armband` `:223`, armband pool clear `:219`) hit the per-bot clone — but the `:204`
randomisation clamp hits **live config** (see §3 output).

**BotWeaponGenerator** — `IsWeaponValid` returns inside the `foreach` `:412`, validating only
the first item with required slots; intentional NRE surfaces:
`InventoryMagGenComponents.FirstOrDefault(…).Process(…)` `:464/:506`, `ubglMod.Template`
`:492`, three unguarded `DefAmmo.Value` reads + `cartridgePool[magazineCaliberData]` indexer
in `GetWeightedCompatibleAmmo` `:630/:651/:656/:669`, and the `Chambers` chain `:212-:213`
(`Any()` on possibly-null, then `.FirstOrDefault().Properties.Filters.FirstOrDefault().Filter`).

**BotLootGenerator** — the **eleven weighted draws `:97-:107` are consumed before the
`DisableLootOnBotTypes` zeroing `:110-:116`** (rolls burned even when discarded);
`ItemHasReachedSpawnLimit`'s escape hatch `if (currentLimitCount > currentLimitCount * 10)`
`:789` is dead, loop bounded only by pool exhaustion (`i--` + `pool.Remove` `:503-:505`);
vest branch checks only slot presence `:317` while backpack also checks count > 0 `:273`;
secure container: hardcoded 50 items, `totalValueLimitRub = -1` disables the budget check
(`> 0` at `:600`); grenades never in backpack `:261`; `RandomiseAmmoStackSize` **ignores its
`isPmc` parameter** `:842` (frozen vestigial signature); `RandomiseMoneyStackSize` unguarded
indexer `currencyWeights[moneyItem.Template]` `:829`; `AddLootFromPool` mutates the caller's
pool `:503` (safe only because the cache clones on read — the projection must feed Rust an
owned copy the same way).

Draw-order alignment: candidate pools preserve C# enumeration order (insertion-ordered maps,
per the RNG-parity stage); one seeded stream per call. Items-view limitation carried over
from the loot ports: props-less templates are absent from the view — documented, plus the
`ArmorClass` NRE point above makes them a legacy-path crash anyway.

RNG stream note: `BotGenerator` draws from the shared stream before (health, skills, voice,
appearance) and after (dogtag) the boundary. Parity tests therefore seed **at the boundary**
(legacy: install `SeededRandomSource` immediately before calling `GenerateInventory`;
native: `NativeTestSeed`), exactly as the loot parity tests already do.

Thread-safety: `BotController` generates bots with `AsParallel` (`BotController.cs:257-260`).
The FFI call is synchronous on the calling thread, so the thread-local test-seed override
survives; `BotWeaponGenerator` is a DI singleton but gains no mutable state; loot-cache
hydration stays on the C# side of projection under its existing lock.

## 6. Testing

Mirror the loot suite in `Testing/UnitTests/Tests/Generators/`:

- **Parity** (`BotParityTests`): same seed → `LootIdNormalizer`-normalized JSON equality of
  the full `BotBaseInventory` between native and legacy, across roles (assault, usec/bear
  PMC, player scav) and ≥2 seeds, inputs projected from the live database. Fail-fast asserts
  on `LastPathTaken` and non-empty inventories before comparing. Stub profile via
  `SaveServer.CreateProfile` (the `BotWeaponGeneratorTests` pattern) for the pmc-level read.
- **Dispatch** (`BotPathDispatchTests`): native by default; legacy on
  `ForceLegacyBotGeneration`; legacy on `TypePriority` subclass of each sibling generator;
  legacy on a foreign `IInventoryMagGen` registration.
- **Hook liveness** (`BotHookLivenessTests`): a live Harmony patch on a member of *each of
  the four classes* flips dispatch to legacy; a patch on `GenerateInventory` itself fires
  around the native body.
- **Container-grid replay**: player-scav path — grids readable from
  `BotInventoryContainerService` after a native call with
  `ClearBotContainerCacheAfterGeneration == false`.
- **Config-mutation replay**: the night-time randomisation clamp lands in live `BotConfig`
  identically on both paths (save/restore in `finally`).
- **KATs**: `roll_chance`, `get_percent_of_value`, `reduce_value_by_percent`, generalized
  `get_weighted_value` — twin known-answer tests on both sides.
- **Benchmark** (`BotBenchmarkTests`): per-bot native vs legacy wall time, mirroring
  `LootBenchmarkTests`, to measure the projection cost (§7).
- `[NonParallelizable]` + save/restore wherever global singletons (config flag, RNG seams,
  container service, loot cache) are touched, as `LootParityTests` does.
- Existing `BotWeaponGeneratorTests` / `BotGeneratorHelperTests` keep passing untouched —
  they enter below the dispatch boundary and exercise the retained legacy code.

## 7. Performance stance

Per-call projection, no snapshots (playbook rule 2). One call per bot instead of the 4–9 a
#2-only port would have made; the items view (~50 ms/projection measured in the reward-loot
port) is the dominant cost at ~30–60 bots per wave, C#-parallel. The benchmark decides:
if wave generation time is unacceptable, the **sanctioned follow-up** is the
invalidation-aware items-view cache (or raw-JSON passthrough) already named in
ARCHITECTURE.md — a separate stage, not smuggled into this one. `ForceLegacyBotGeneration`
restores legacy performance and C# diagnosability meanwhile. Like the reward-loot port, a
measured regression with the escape hatch documented is an acceptable interim outcome; an
unmeasured one is not.

## 8. Mod-facing limitations (to document in ARCHITECTURE.md)

Same shape as the loot ports' section:

- Patches on collaborators the projection replaces never run on the native path:
  `BotGeneratorHelper.*`, `DurabilityLimitsHelper.*`, `RepairService.AddBuff`,
  `BotWeaponGeneratorHelper.*`, `BotEquipmentModPoolService.*`, `BotLootCacheService.
  GetLootFromCache`, `WeightedRandomHelper.GetWeightedValue`, `ItemHelper` twins, `ICloner`.
  Escape hatches: the flag, or a patch on any frozen member of the four ported classes
  (widened set, §2).
- `BotEquipmentModPoolService.ResetWeaponPool()` is a no-op for the native path (Rust
  re-derives pools from the live-projected item table per call — data mutations are honoured
  *better* than the C# cache, but patches on the service are not).
- Mods registering extra `IInventoryMagGen` implementations get the legacy path
  automatically (detection §2.4).
- Props-less mod-added templates: absent from the items view, same documented limitation as
  the loot ports.
- Native-path hangs (the `GenerateEquipment` retry loop can spin on a broken pool exactly as
  4.1.2 does) sit inside an FFI call without a managed stack trace; the flag restores
  diagnosability.

## 9. Gate (playbook rule 5)

`dotnet build server-csharp.slnx -c Release` → `mpex-api-compat/ci/check-api-compat.sh`
(all six assemblies clean — the flag is a new config property, hook-detection is internal,
no signature changes at all this time) → `dotnet test` → `cd rust && cargo test &&
cargo fmt --check && cargo clippy --all-targets -- -D warnings` → `csharpier format .` →
`graphify update .` → ARCHITECTURE.md bot-family section (boundary, widened hook contract,
flag, config-mutation replay, limitations, benchmark numbers) + `todo/TODO.md` #2/#3 marked
done.

## 10. Implementation phasing (for the plan)

Sized ~4.5k C# lines ported, est. 8–10k lines of Rust. Each phase lands green
(cargo + dotnet tests) before the next:

1. Rust primitives: `roll_chance`, `get_percent_of_value`, `reduce_value_by_percent`,
   generalized `get_weighted_value`, `exhaustable_array` — with twin KATs.
2. Helper modules: `durability_limits_helper`, `bot_generator_helper` (Upd generation,
   incompatibility, grids), `repair_service::add_buff`, `bot_weapon_generator_helper`,
   mod-pool derivation, wallet grid placement.
3. `bot_equipment_mod_generator.rs` (largest single port).
4. `bot_weapon_generator.rs` + `inventory_mag_gen.rs`.
5. `bot_loot_generator.rs`.
6. `bot_inventory_generator.rs` + payload models both sides + FFI export + ABI bump.
7. C# dispatch (flag, widened hook detection, TypePriority + mag-gen checks, projection,
   output replay) + full test suite.
8. Benchmark, docs, gate loop.
