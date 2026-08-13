# Bot-family Native Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `BotEquipmentModGenerator`, `BotInventoryGenerator`, `BotWeaponGenerator`, and `BotLootGenerator` to `rust/spt-native` behind one `spt_generate_bot_inventory` call per bot, dual-dispatched from `BotInventoryGenerator.GenerateInventory` with the legacy C# retained as the executable oracle.

**Architecture:** One FFI cut at `BotGenerator.cs:284` (the only production call site of `GenerateInventory`; the whole four-class subtree runs inside it — verified complete call topology in the spec). Frozen 4.1.2 surfaces; widened hook detection (any patched member of the four classes except the dispatcher itself → legacy); `BotConfig.ForceLegacyBotGeneration` escape hatch; per-call payload projection, no snapshots; bug-for-bug parity including the live-`BotConfig` randomisation-clamp mutation and the `BotInventoryContainerService` grid replay.

**Tech Stack:** C# (.NET 10) + Rust (cdylib, serde/IndexMap/rand_xoshiro), NUnit, cargo test.

**Spec:** `docs/superpowers/specs/2026-08-13-bot-family-port-design.md` — read it first; every task below argues from it. The two prior ports are the pattern library: `Generators/Loot/LootGenerator.cs` (dispatch/ctor/seams), `rust/spt-native/src/loot/loot_generator.rs` (module style), `Testing/UnitTests/Tests/Generators/LootParityTests.cs`/`RewardLootParityTests.cs` (test style), `docs/superpowers/plans/2026-08-12-loot-generator-port.md` (this plan's ancestor).

## Global Constraints

- **Frozen 4.1.2 surface** (playbook rule 1): no signature, parameter-name, or visibility change to any existing public/protected member of the four ported classes or their DTOs. Legacy bodies retained verbatim as `XLegacy` private methods — zero edits inside.
- **No constructor changes anywhere** — `BotConfig` is already injected where dispatch lives; mag-gen detection uses reflection on the existing protected field (Task 14), not a ctor addition.
- **C# nomenclature in Rust casing** (CLAUDE.md): `GenerateModsForWeapon` → `generate_mods_for_weapon`; wire names pinned with serde renames, insertion-ordered maps (`IndexMap`) everywhere a C# `Dictionary` feeds enumeration or draws.
- **Bug-for-bug** (playbook rule 3): the quirk tables in each task are requirements, not bugs to fix. Intentional NREs become `LootError`s carrying an equivalent message; draw *order* and draw *count* must match the legacy path exactly under a shared seed.
- **RNG**: every Rust draw goes through `loot::random_util` primitives so `TestSeedGuard` covers it. Never draw eagerly where C# short-circuits (and vice versa).
- **FFI** (playbook rule 4): envelopes are internal contracts, shipped in lockstep; `ABI_VERSION` 4→5 exactly once (Task 12), `ExpectedAbiVersion` in the same commit.
- **Gate loop per task**: Rust tasks end `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`; C# tasks end `dotnet build server-csharp.slnx && dotnet test` (+ `csharpier format .` before commit). Final full gate in Task 16.
- **Commits**: one per task, message given in the task. `git add` only the task's files.

## File Structure

```
rust/spt-native/src/
  lib.rs                          # modify: `pub mod bot;`, ABI 4→5 (Task 12 only)
  ffi.rs                          # modify: 1 export + pin-test bump (Task 12)
  bot/
    mod.rs                        # new: pub mod models; pub(crate) mod …; BotContext
    models.rs                     # new: wire envelopes (Task 1, grown by later tasks)
    exhaustable_array.rs          # new (Task 3)
    durability_limits_helper.rs   # new (Task 4)
    repair_service.rs             # new (Task 4)
    bot_generator_helper.rs       # new (Task 5): Upd gen, incompat, container grids
    bot_weapon_generator_helper.rs# new (Task 6)
    mod_pool_service.rs           # new (Task 6): pool derivation from items view
    bot_equipment_mod_generator.rs# new (Tasks 7–8)
    inventory_mag_gen.rs          # new (Task 9)
    bot_weapon_generator.rs       # new (Task 9)
    bot_loot_generator.rs         # new (Task 10)
    bot_inventory_generator.rs    # new (Task 11): orchestrator = FFI entry body
  loot/random_util.rs             # modify (Task 2): 3 new primitives + generic keys
Libraries/SPTarkov.Server.Core/
  Models/Spt/Config/BotConfig.cs  # modify (Task 14): ForceLegacyBotGeneration property
  Native/NativeMethods.cs         # modify (Task 13): 1 LibraryImport
  Native/SptNative.cs             # modify (Task 13): wrapper + enum arm + ABI 5
  Native/Bot/BotPayloads.cs       # new (Task 13): C# envelopes
  Native/Bot/BotPayloadProjection.cs # new (Task 13): projection + replay helpers
  Generators/Bot/BotInventoryGenerator.cs # modify (Task 14): dispatch ONLY (legacy body untouched)
Testing/UnitTests/Tests/
  Utils/RandomSourceParityTests.cs  # modify (Task 2): twin KATs
  Generators/BotParityTests.cs      # new (Task 15)
  Generators/BotPathDispatchTests.cs# new (Task 15)
  Generators/BotHookLivenessTests.cs# new (Task 15)
  Generators/BotBenchmarkTests.cs   # new (Task 16)
```

C# reference sources (read-only oracles — never edited except where listed above):
`Generators/Bot/BotEquipmentModGenerator.cs`, `Generators/Bot/BotInventoryGenerator.cs`,
`Generators/Bot/BotWeaponGenerator.cs`, `Generators/Loot/BotLootGenerator.cs`,
`Helpers/BotGeneratorHelper.cs`, `Helpers/DurabilityLimitsHelper.cs`,
`Helpers/BotWeaponGeneratorHelper.cs`, `Services/Bot/BotEquipmentModPoolService.cs`,
`Services/Bot/BotInventoryContainerService.cs`, `Services/Bot/BotLootCacheService.cs`,
`Services/Commerce/RepairService.cs` (`AddBuff` only), the `IInventoryMagGen` implementations
(grep `: IInventoryMagGen` under `Generators/`), `Utils/Collections/ExhaustableArray.cs`,
`Utils/RandomUtil.cs`.

---

### Task 1: `bot/` module scaffold + wire models

**Files:**
- Create: `rust/spt-native/src/bot/mod.rs`, `rust/spt-native/src/bot/models.rs`
- Modify: `rust/spt-native/src/lib.rs` (add `pub mod bot;` — do NOT touch `ABI_VERSION`)
- Test: inline `#[cfg(test)]` at the bottom of `models.rs`

**Interfaces:**
- Produces: `GenerateBotInventoryRequest`, `BotInventoryResult`, `BotGenerationDetailsWire`, and the template/config wire types below. Later Rust tasks consume these; Task 13 mirrors them in C#.
- Consumes: `loot::models::ItemView`/`ItemsView` and the `Item` wire type — reuse via `use crate::loot::models::…`, do not duplicate.

- [ ] **Step 1: Write failing round-trip tests** (same style as `loot/models.rs` tests): deserialize `GenerateBotInventoryRequest` from a minimal literal JSON string covering every top-level field; serialize a `BotInventoryResult` and assert exact wire names (`inventory`, `diagnostics`, `containerGrids`, `randomisationClamps`); assert `testSeed: null` → `None`; assert an unknown key inside a template block lands in the `#[serde(flatten)] extra` map (round-trip preserved).

- [ ] **Step 2: Run** — `cd rust && cargo test -p spt-native bot::models` → FAIL (module missing).

- [ ] **Step 3: Implement.** Top-level envelope, spec §3 verbatim:

```rust
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBotInventoryRequest {
    pub bot_id: String,
    pub test_seed: Option<u64>,
    pub details: BotGenerationDetailsWire,
    pub template: BotTemplateWire,          // inventory + chances + generation blocks only
    pub generating_player_level: i32,       // hoisted live state
    pub is_night_time: bool,                // hoisted live state
    pub equipment_config: serde_json::Map<String, serde_json::Value>, // BotConfig.Equipment[role]; typed fields grow per-task
    pub item_spawn_limits: IndexMap<String, IndexMap<String, f64>>,
    pub wallet_loot: serde_json::Value,
    pub currency_stack_size: serde_json::Value,
    pub secure_container_ammo_stack_count: serde_json::Value,
    pub disable_loot_on_bot_types: Vec<String>,
    pub low_profile_gas_block_tpls: Vec<String>,
    pub loot_item_resource_randomization: serde_json::Value,
    pub pmc_config: serde_json::Value,
    pub repair_kit_weapon: serde_json::Value,
    pub equipment_blacklist: serde_json::Value,   // GetBotEquipmentBlacklist(role, level) result
    pub sight_whitelist: IndexMap<String, Vec<String>>,
    pub loot_pools: IndexMap<String, serde_json::Value>, // the 13 resolved BotLootCacheService pools
    pub item_presets: IndexMap<String, serde_json::Value>, // GlobalTable.ItemPresets
    pub default_presets_by_tpl: IndexMap<String, serde_json::Value>,
    pub presets_by_id: IndexMap<String, serde_json::Value>,
    pub config_blacklist: Vec<String>,      // ItemFilterService.GetBlacklistedItems()
    pub handbook_prices: IndexMap<String, f64>,
    pub items: crate::loot::models::ItemsView,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGenerationDetailsWire {
    pub role: String, pub role_lowercase: String, pub side: String,
    pub bot_level: i32, pub is_pmc: bool, pub is_player_scav: bool,
    pub game_version: String, pub location: Option<String>,
    pub bot_difficulty: String,
    pub clear_bot_container_cache_after_generation: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotInventoryResult {
    pub inventory: serde_json::Value,       // BotBaseInventory shape, built by Task 11
    pub diagnostics: Vec<crate::loot::models::Diagnostic>,
    pub container_grids: IndexMap<String, serde_json::Value>, // slot → grid state (Task 5 shapes it)
    pub randomisation_clamps: IndexMap<String, f64>,          // equipment-slot → clamped chance
}
```

`BotTemplateWire` = `{ inventory: BotTypeInventoryWire, chances: serde_json::Value, generation: serde_json::Value }`; `BotTypeInventoryWire` mirrors the fields `BotTypeInventory` exposes (`equipment: IndexMap<String, IndexMap<String, f64>>`, `mods`, `ammo`, `items` — check `Models/Eft/Common/Tables/BotType.cs` for exact member names and wire casing) with `#[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>` on every game-data type, per the loot models' convention. **Deliberate looseness:** blocks typed `serde_json::Value` here get real types in the task that reads them (each later Rust task's "model growth" step) — same pattern as the loot port's Task-1-then-grow approach. `mod.rs` declares `pub mod models;` only for now.

- [ ] **Step 4: Run** — `cargo test -p spt-native bot::models` → PASS.
- [ ] **Step 5: Gates + commit** — fmt/clippy clean; `git add rust/ && git commit -m "feat: add bot-family wire models to spt-native"`

### Task 2: RNG primitives — `roll_chance`, `get_percent_of_value`, `reduce_value_by_percent`, generic `get_weighted_value`

**Files:**
- Modify: `rust/spt-native/src/loot/random_util.rs`
- Modify: `Testing/UnitTests/Tests/Utils/RandomSourceParityTests.cs` (twin KATs)

**Interfaces:**
- Produces: `pub fn roll_chance(chance: f64) -> bool`, `pub fn get_percent_of_value(percent: f64, value: f64, to_fixed: i32) -> f64`, `pub fn reduce_value_by_percent(value: f64, percent: f64) -> f64`, and `pub fn get_weighted_value<K: Clone + Eq + std::hash::Hash>(map: &IndexMap<K, f64>) -> Result<K, LootError>` (generalizing the existing string-keyed fn; keep a thin `get_weighted_value_str` alias if call sites elsewhere break, but prefer updating them — they're all in this repo).

- [ ] **Step 1: Read the C# sources.** `Utils/RandomUtil.cs`: `RollChance` at `:498` — an **inclusive 1–100 scaled roll** (`GetInt(1, 100*scale)/(1*scale) <= chance`), deliberately different from `GetChance100` (exclusive 1–99, `:144-149`, already ported). Copy the exact scale arithmetic. `GetPercentOfValue` and `ReduceValueByPercent` — read both bodies, mirror rounding (`toFixed` semantics) exactly. For the generic map draw, the existing Rust `get_weighted_value` body is the algorithm; only the key type generalizes. Insertion order is the draw order — `IndexMap` non-negotiable.
- [ ] **Step 2: Rust KATs (failing).** Existing KAT pattern (seeded `TestSeedGuard`, pin printed values after first run): `roll_chance` under seed 42 for chances `[0.0, 50.0, 100.0]` ×3 draws each — assert exact bool sequence AND that `roll_chance(100.0)` still consumes a draw; `get_percent_of_value(15.0, 200.0, 2)`-style pure-math cases (no RNG) with exact expected values from running the C# in Step 4; `reduce_value_by_percent` likewise; generic `get_weighted_value` over an `IndexMap<i64, f64>` and an `IndexMap<String, f64>` with identical weights/order → identical index sequence.
- [ ] **Step 3: Implement + pin** — run with `-- --nocapture`, pin values, re-run → PASS.
- [ ] **Step 4: C# twin KATs** in `RandomSourceParityTests.cs`, same fixture style as the existing `GetWeightedValue` twins: `SeededRandomSource(42)` into `RandomUtil`, call `RollChance`/`GetPercentOfValue`/`ReduceValueByPercent` and `WeightedRandomHelper.GetWeightedValue` over a `Dictionary<double, double>` and a `Dictionary<string, double>` with the same literals as Step 2; assert the pinned values match.
- [ ] **Step 5: Run both** — `cargo test -p spt-native random_util` and `dotnet test --filter "FullyQualifiedName~RandomSourceParityTests"` → PASS.
- [ ] **Step 6: Gates + commit** — `feat: add roll_chance and percent primitives with twin KATs`

### Task 3: `exhaustable_array.rs`

**Files:**
- Create: `rust/spt-native/src/bot/exhaustable_array.rs` (+ `pub(crate) mod` line in `bot/mod.rs`)

**Interfaces:**
- Produces: `pub struct ExhaustableArray<T: Clone> { … }` with `pub fn new(pool: Vec<T>) -> Self`, `pub fn get_random_value(&mut self) -> Option<T>`, `pub fn get_first_value(&mut self) -> Option<T>`, `pub fn has_values(&self) -> bool` — whatever the C# exposes, mirrored.

- [ ] **Step 1: Read the C# source** — `Utils/Collections/ExhaustableArray.cs` (`:24-35` is the draw): `GetRandomValue` = `randomUtil.GetInt(0, count-1)`, `ElementAt(index)` on a `LinkedList`, remove, `cloner.Clone` the element. Mirror the index arithmetic and removal exactly (a `Vec<T>` with `remove(index)` reproduces LinkedList `ElementAt`+remove order); Rust owns its data so the clone is `T: Clone`.
- [ ] **Step 2: Failing tests** — seeded: pool `[10,20,30,40,50]` under seed 42 → assert the exact drawn sequence (pin after first run) and that the sequence *exhausts* (5 draws → 5 unique elements, 6th → `None`); `has_values` flips; empty pool → `None` without consuming a draw (verify against C# behavior first — if C# draws even on empty, replicate that).
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port ExhaustableArray draw-without-replacement to spt-native`

### Task 4: `durability_limits_helper.rs` + `repair_service.rs::add_buff`

**Files:**
- Create: `rust/spt-native/src/bot/durability_limits_helper.rs`, `rust/spt-native/src/bot/repair_service.rs`

**Interfaces:**
- Produces: `pub fn get_randomized_max_weapon_durability(...)`, `get_randomized_current_weapon_durability(...)`, `get_randomized_armor_durability(...)` (exact set/names from the C# — `GetRandomizedMax…`/`…Current…` at `DurabilityLimitsHelper.cs:135/:142/:193/:205`), taking the bot role + the config slices they read; `pub fn add_buff(repair_kit_weapon_cfg: &…, weapon_root: &mut Item) -> Result<(), LootError>` mirroring `RepairService.AddBuff` (`RepairService.cs:506-521`: two `GetWeightedValue` draws, two `GetDouble`, one `GetPercentOfValue` — **draw order is the contract**).
- Consumes: Task 2 primitives.

- [ ] **Step 1: Read the C# sources fully** — `Helpers/DurabilityLimitsHelper.cs` (all public methods reachable from `BotGeneratorHelper.GenerateExtraPropertiesForItem` — trace which are called for weapons vs armor vs mods, and which `BotConfig.Durability` per-role config each reads) and `RepairService.AddBuff`. List, in a module doc-comment, each RNG call in C# source order — this list is the parity checklist.
- [ ] **Step 2: Failing seeded tests** — for each fn: seed 42, a hand-built config slice (per-role max/min deltas), assert exact pinned outputs across 3 consecutive calls (pins the draw count too). `add_buff`: a weapon item with `Upd.Repairable`, a two-entry buff-type weight map + a rate map → assert the exact `Upd.Buff` shape C# writes (`BuffType`, `Value`, `ThresholdDurability` — read the C# for exact fields and rounding).
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port durability limits and repair buff rolls to spt-native`

### Task 5: `bot_generator_helper.rs` — `Upd` generation, incompatibility, container grids

**Files:**
- Create: `rust/spt-native/src/bot/bot_generator_helper.rs`
- Modify: `rust/spt-native/src/bot/models.rs` (typed `LootItemResourceRandomization`, the container-grid wire shape)

**Interfaces:**
- Produces:
  - `pub fn generate_extra_properties_for_item(ctx: &BotContext, item_template: &ItemView, bot_role: &str) -> Option<Upd>` — mirrors `BotGeneratorHelper.GenerateExtraPropertiesForItem` (`Helpers/BotGeneratorHelper.cs`, RNG sites `:101` `GetArrayValue(WeapFireType)`, `:132/:140/:152/:160` `GetChance100`×4, `:182/:193/:196` resource randomization via `GetChance100`/`GetPercentOfValue`/`GetDouble`) including night-raid light/laser toggles (reads `ctx.is_night_time`) and durability via Task 4.
  - `pub fn is_item_incompatible_with_current_items(items_so_far: &[Item], tpl: &str, equipment_slot: &str, ctx: &BotContext) -> IncompatibilityResult` (mirror the C# return type `ChooseRandomCompatibleModResult`-style `{ incompatible: bool, found_slot_id: Option<String>, reason: String }` — read the C# for exact shape).
  - `pub struct ContainerGrids` — per-bot occupancy grids owned by the Rust call: `pub fn add_empty_container(&mut self, slot: &str, container_details: …)`, `pub fn add_item_with_children_to_equipment_slot(&mut self, allowed_slots: &[String], root_item: Item, children: Vec<Item>, inventory: &mut Vec<Item>, ctx: &BotContext) -> ItemAddedResult` mirroring `BotGeneratorHelper.AddItemWithChildrenToEquipmentSlot` + `BotInventoryContainerService` (`AddEmptyContainerToBot`, the `int[,]` grid walk in `Services/Bot/BotInventoryContainerService.cs`). `ItemAddedResult` mirrors the C# enum (`SUCCESS`/`NO_SPACE`/`NO_CONTAINERS`/… — read the enum source). Serialization of final grid state → `BotInventoryResult.container_grids` (shape: slot name → `{ containerId, width, height, occupied: Vec<Vec<bool>> }` or whatever losslessly reproduces the C# `ContainerDetails` — Task 14 must be able to rebuild the service's state from it).
  - `pub struct BotContext<'a>` in `bot/mod.rs`: borrows `items: &ItemsView` + the request's config slices — the analog of `loot::LootContext`.
- Consumes: Tasks 2, 4.

- [ ] **Step 1: Read the C# sources fully** — `Helpers/BotGeneratorHelper.cs` (whole file: `GenerateExtraPropertiesForItem`, `IsItemIncompatibleWithCurrentItems` incl. the `BlocksX` property probes, `AddItemWithChildrenToEquipmentSlot`, `GetBotEquipmentRole`) and `Services/Bot/BotInventoryContainerService.cs` (grid layout, placement scan order — row-major vs column-major matters for parity). Doc-comment the RNG order list.
- [ ] **Step 2: Failing tests** — (a) seeded `generate_extra_properties_for_item` for: a weapon template (fire-mode + durability draws), an armor template, a resource item (med/food) with `LootItemResourceRandomization` for the role, a light/laser mod under `is_night_time` true vs false → pin exact `Upd`s and draw counts; (b) `is_item_incompatible…`: a `ConflictingItems` pair → incompatible with the C# reason string; a `BlocksEarpiece` probe case; compatible case; (c) grids: a 2×2 container, place a 1×1 then a 2×1 item → occupancy and found-slot coordinates pinned; full container → `NO_SPACE`; two allowed slots → first-fit order matches C# slot iteration order.
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port bot item property generation and container grids to spt-native`

### Task 6: `bot_weapon_generator_helper.rs` + `mod_pool_service.rs`

**Files:**
- Create: `rust/spt-native/src/bot/bot_weapon_generator_helper.rs`, `rust/spt-native/src/bot/mod_pool_service.rs`

**Interfaces:**
- Produces:
  - `pub fn get_randomized_magazine_count(mag_counts: &IndexMap<String, f64>) -> Result<f64, LootError>` (`BotWeaponGeneratorHelper.cs:70`, one `GetWeightedValue`), `pub fn get_randomized_bullet_count(…)` (`:26`), `pub fn create_magazine_with_ammo(…) -> Vec<Item>`, `pub fn magazine_is_cylinder_related(name: &str) -> bool` (name-contains check).
  - `mod_pool_service`: `pub fn get_mods_for_gear_slot(ctx, tpl) -> IndexMap<String, IndexSet<String>>`, `get_mods_for_weapon_slot`, `get_required_mods_for_weapon_slot`, `get_compatible_mods_for_weapon_slot` — pure derivations from the items view replicating what `Services/Bot/BotEquipmentModPoolService.cs` lazily caches (slot-filter walks over the template's `Properties.Slots`); **no caching in Rust** — spec §3 fixes this as re-derive-per-call (that is what makes `ResetWeaponPool()` irrelevant natively).
- Consumes: Tasks 1, 2; `loot::item_helper` (`get_item`, `fill_magazine_with_*` — already ported).

- [ ] **Step 1: Read the C# sources fully** — `Helpers/BotWeaponGeneratorHelper.cs`, `Services/Bot/BotEquipmentModPoolService.cs` (`GeneratePool` walk: which slot names it includes/excludes for gear vs weapon, required-mod detection). Doc-comment RNG order.
- [ ] **Step 2: Failing tests** — seeded magazine/bullet count pins; `create_magazine_with_ammo` → mag root + cartridge child shape/ids/stack counts pinned; derivation tests against a 4-template mini items-view fixture (weapon with two slots, one `Required`, filters; a plate carrier with plate slots) → assert derived pools equal the C# service's output for the same fixture (compute expected by hand from the fixture, not by running C#).
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port weapon-gen helpers and mod pool derivation to spt-native`

### Task 7: `bot_equipment_mod_generator.rs` — equipment-mod path

**Files:**
- Create: `rust/spt-native/src/bot/bot_equipment_mod_generator.rs`
- Modify: `bot/models.rs` (typed `EquipmentFilters` fields this path reads: plate weighting, `RandomisationDetails`)

**Interfaces:**
- Produces: `pub fn generate_mods_for_equipment(ctx: &BotContext, grids: &mut ContainerGrids, equipment: &mut Vec<Item>, parent_id: &str, parent_template: &ItemView, settings: &mut GenerateEquipmentPropertiesWire, specific_blacklist: &…, should_force_spawn: bool) -> Result<(), LootError>` plus its internal helpers `filter_plate_mods_for_slot_by_level`, `get_min_max_armor_plate_class`, `get_default_preset_armor_slot` — Rust-private, C# names.
- Consumes: Tasks 2, 5, 6; `loot::random_util::get_weighted_value`.

**C# mapping — `Generators/Bot/BotEquipmentModGenerator.cs`:**

| Rust fn | C# lines | Parity requirements (each is an assertion target) |
|---|---|---|
| `generate_mods_for_equipment` | 97–313 | Recursion at `:300` per added mod with its own pool; `OrderBy` front_plate/back_plate/other at `:115-130` — **stable sort, residual order = map insertion order**; `ShouldModBeSpawned` gate; forced-spawn override; `IsRemovablePlateSlot` (6-name set, `:199/:326`); the `:268/:275` double-negative flow and the `:270` **intentional NRE** (pool empty + slot Required → `LootError` naming the null deref) |
| `filter_plate_mods_for_slot_by_level` | 315–457 | Lazy-LINQ re-enumeration `:363-448` — `platesFromDb` re-computed at `:366/:381/:399/:448` (no RNG inside, so a materialized Vec is output-equivalent; keep the repeated *filtering* semantics); `ArmorClass.Value` NRE on props-less templates → `LootError`; weighted draw at `:350` **always consumed**, clamp at `:353` applied after; 3-attempt plate-class wraparound `:386-444` with wrap to `Min` |
| `get_min_max_armor_plate_class` | 458–489 | protected static; straight port |
| `get_default_preset_armor_slot` | 490–502 | uses `default_presets_by_tpl` projection |

- [ ] **Step 1: Read the C# lines above in full before writing anything.** Also `Models/Spt/Bots/GenerateEquipmentProperties.cs` for the settings shape (`ModPool`, `SpawnChances.EquipmentModsChances`, `BotData{Role,Level,EquipmentRole}`, `BotEquipmentConfig`, `RandomisationDetails`) — grow `models.rs` with a typed `GenerateEquipmentPropertiesWire`.
- [ ] **Step 2: Failing tests** — module fixture (mini items-view: a plate carrier with front/back/side plate slots, plates of classes 2–6, a headwear item with a `mod_nvg` slot chain, a required-slot item with an empty pool): (a) seeded full run over the carrier → pinned item tree (ids normalized), plate classes obey the weighted-then-clamped draw; (b) plate filter with `maxArmorLevel` clamping → draw still consumed (assert via next-draw value); (c) empty pool + Required slot → `LootError`; (d) forceSpawn path; (e) recursion: the NVG chain produces grandchildren.
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port equipment mod generation to spt-native`

### Task 8: `bot_equipment_mod_generator.rs` — weapon-mod path

**Files:**
- Modify: `rust/spt-native/src/bot/bot_equipment_mod_generator.rs`, `bot/models.rs` (`GenerateWeaponRequestWire`, `BotModLimitsWire`)

**Interfaces:**
- Produces: `pub fn generate_mods_for_weapon(ctx: &BotContext, request: &mut GenerateWeaponRequestWire) -> Result<(), LootError>` — mutates `request.weapon`, `mod_spawn_chances`, `mod_pool`, `conflicting_item_tpls`, `weapon_stats`, `mod_limits` in place (all interior to the FFI call; `BotModLimitsWire` counters mirror `BotWeaponModLimitService.WeaponModHasReachedLimit`'s mutation).
- Consumes: Tasks 2, 3, 5, 6, 7's fixture style.

**C# mapping — `BotEquipmentModGenerator.cs` (weapon path):**

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `generate_mods_for_weapon` | 503–775 | Sort via `sort_mod_keys` (`:858`); per-slot flow `ShouldModBeSpawned` (`:989`, `RollChance` at `:1003`) → `ChooseModToPutIntoSlot` (`:1021`); spawn-chance mutations `:622/:637/:643-644/:657/:666`; `conflicting_item_tpls` insert `:694`; `weapon_stats` writes `:674/:678/:683`; mod-limit counter mutation via `weapon_mod_has_reached_limit` (port inline, ~120 lines incl. baseclass checks); recursion `:761`; **cylinder branch `:697-706` → `fill_camora`**; `mod_pool` writes `:723/:735` |
| `should_force_sub_stock_slots` | 776–789 | `:662`-adjacent NRE when role missing from equipment config — replicate as `LootError` |
| `mod_is_front_or_rear_sight` / `mod_slot_can_hold_scope` / `mod_slot_can_hold_muzzle_devices` | 790–857 | `:847` **ignores `modsParentId`** — port the ignored parameter as-is with a `// parity: parameter unused in C#` comment |
| `sort_mod_keys` | 858–958 | exact ordering table |
| `get_mod_item_slot_from_db_template` / `get_mod_pool_for_slot` / `get_mod_pool_for_default_slot` / `get_filtered_mod_pool` | 959–988, 1321–1448 | default-preset branch via `get_matching_preset`/`get_matching_mod_from_preset` (`:1449-1488`, uses `presets_by_id` + `default_presets_by_tpl`) |
| `choose_mod_to_put_into_slot` | 1021–1149 | `:1028` ammo-container short-circuit to `request.ammo_tpl` **except `mod_magazine`**; `:1035` null-propagated vs `:1099` unguarded `parent_slot.required` deref; **`:1281` `SlotBlocked` never set (disabled on purpose)** — only `is_item_incompatible…` can set it |
| `get_filtered_magazine_pool_by_capacity` | 1150–1181 | straight port |
| `get_compatible_weapon_mod_tpl_for_slot_from_pool` / `get_compatible_mod_from_pool` | 1182–1309 | `ExhaustableArray` draws (`:1231/:1246`); `blocked_attempt_count` reset-before-break `:1243/:1280`; `max_blocked_attempts = round(count * 0.75)` with `>` |
| `create_mod_item` | 1510–1526 | calls `generate_extra_properties_for_item` (Task 5) — **in-stream RNG position** |
| `get_random_mod_tpl_from_item_db` | 1540–1569 | `ExhaustableArray` draw `:1546/:1550` |
| `is_mod_valid_for_slot` | 1570–1630 | error-text diagnostics via `push_diagnostic` (loot pattern) |
| `add_compatible_mods_for_provided_mod` / `get_dynamic_mod_pool` / `filter_mods_by_blacklist` | 1631–1734 | `:1660-1662` mod-pool mutation; `:1674` uses `get_compatible_mods_for_weapon_slot` derivation (Task 6); `:1701` blacklist filter reads `equipment_blacklist` projection |
| `fill_camora` / `merge_camora_pools` | 1735–1834 | picks **one** ammo tpl and clones into **every** `Properties.Slots` entry incl. non-camora (`:1800-1813`); fresh child ids `:1803` |
| `filter_sights_by_weapon_type` | 1835–1916 | sight whitelist projection; `:1861/:1874/:1889-1890` baseclass checks; `:1887` `is_item_in_db` = `get_item().is_some()` |

- [ ] **Step 1: Read the C# lines above in full.** Grow `models.rs`: `GenerateWeaponRequestWire` mirroring `Models/Spt/Bots/GenerateWeaponRequest.cs` (`weapon: Vec<Item>`, `mod_pool`, `weapon_id`, `parent_template` tpl, `mod_spawn_chances: IndexMap<String, f64>`, `ammo_tpl`, `bot_data`, `mod_limits`, `weapon_stats`, `conflicting_item_tpls: IndexSet<String>`).
- [ ] **Step 2: Failing tests** — extend the fixture with an M4-style weapon (receiver→scope mount→sight chain, stock chain for `should_force_sub_stock_slots`, magazine + caliber ammo, a revolver with camora slots): (a) seeded full `generate_mods_for_weapon` → pinned normalized tree; (b) camora path: one ammo tpl in every slot, fresh ids; (c) mod-limit: third scope in pool with limit 2 → skipped, counter state pinned; (d) muzzle-device branch with the `:847` ignored-parameter behavior locked by a test that passes a wrong parent id and still succeeds; (e) sight whitelist filters by weapon baseclass; (f) `mod_spawn_chances` mutated to 100 for required-slot names (`:643` area) visible in the returned request.
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port weapon mod generation to spt-native`

### Task 9: `inventory_mag_gen.rs` + `bot_weapon_generator.rs`

**Files:**
- Create: `rust/spt-native/src/bot/inventory_mag_gen.rs`, `rust/spt-native/src/bot/bot_weapon_generator.rs`
- Modify: `bot/models.rs` (`GenerateWeaponResultWire`, `GenerationDataWire` weights)

**Interfaces:**
- Produces: `pub fn generate_random_weapon(ctx, grids, equipment_slot: &str, bot_template_inventory, details, weapon_parent_id, mod_chances) -> Result<Option<GenerateWeaponResultWire>, LootError>`; `pub fn generate_weapon_by_tpl(…)`; `pub fn add_extra_magazines_to_inventory(ctx, grids, generated_weapon, mag_weights, inventory, bot_role) -> Result<(), LootError>`; `pub fn pick_weighted_weapon_template_from_pool(…)`; mag-gen dispatch `fn process_mag_gen(kind_order: …)` replicating the four built-ins in `MagGenSetUp` priority order.
- Consumes: Tasks 2–8; `loot::item_helper::{add_cartridges_to_ammo_box, fill_magazine_with_*}`.

**C# mapping — `Generators/Bot/BotWeaponGenerator.cs`:**

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `generate_random_weapon` / `pick_weighted_weapon_template_from_pool` | 64–112 | weighted draw `:99` |
| `generate_weapon_by_tpl` | 113–252 | preset fallback via `item_presets` (`:337` area via `get_preset_weapon_mods` `:321`); `RepairService.AddBuff` gate `:154` (`GetChance100` vs `WeaponHasEnhancementChancePercent`) then Task 4 `add_buff`; the `:212-213` **intentional NRE chain** (`Chambers` `Any()` on possibly-null then unguarded `.FirstOrDefault().Properties.Filters.FirstOrDefault().Filter`) → `LootError`; `is_weapon_valid` `:370-426` — **`return true` inside the `foreach` at `:412`: only the FIRST item with required slots is validated, preserve** |
| `add_cartridge_to_chamber` / `construct_weapon_base_list` | 253–320 | straight port |
| `get_weighted_compatible_ammo` | 596–680 | weighted draw `:673`; unguarded `DefAmmo.Value` `:630/:651/:669` + `cartridge_pool[caliber]` indexer `:656` → `LootError`s |
| `get_compatible_cartridges_*` / `get_weapon_caliber` | 681–762 | straight port |
| `add_extra_magazines_to_inventory` | 427–482 | `get_randomized_magazine_count` (Task 6); mag-gen dispatch — `InventoryMagGenComponents.FirstOrDefault(CanHandle).Process` **unguarded** `:464` → `LootError` when none handles |
| `add_ubgl_grenades_to_bot_inventory` | 483–519 | `ubglMod.Template` after `FirstOrDefault` `:492` → `LootError` |
| `add_ammo_to_secure_container` | 520–550 | uses `secure_container_ammo_stack_count` |
| `get_magazine_template_from_weapon_template` | 551–595 | straight port |
| `fill_existing_magazines` / `fill_ubgl` / `add_or_update_magazines_child_with_ammo` / `fill_camoras_with_ammo` | 763–882 | reuse `loot::item_helper` fill fns — verify their semantics match these call sites before reusing (they were ported for ammo boxes; magazine fill draws `GetInt` min 0.25×max — read `ItemHelper.FillMagazineWithCartridge` usage here) |

The four `IInventoryMagGen` implementations: find them (`grep -rn ": IInventoryMagGen" Libraries/`), port each `CanHandleInventoryMagGen` + `Process` with its RNG (`BarrelInventoryMagGen` `GetInt(3,6)`/`GetInt(min,max)` at `:31/:35`; `ExternalInventoryMagGen`'s `GetArrayValue` fallback at `:200`; the internal-magazine and UBGL ones per source). Dispatch order = `MagGenSetUp`'s `GetPriority()` sort (`BotWeaponGenerator.cs:47-52`) — pin the order in a test.

- [ ] **Step 1: Read the C# sources above in full.** Doc-comment the RNG order for one full `generate_random_weapon` run.
- [ ] **Step 2: Failing tests** — fixture grows a full weapon platform: (a) seeded `generate_weapon_by_tpl` → pinned normalized weapon tree with chambered round, filled magazine; (b) enhancement roll at 100% → `Upd.Buff` present; at 0% → absent, **draw still consumed**; (c) `is_weapon_valid` quirk: second item missing a required slot still passes (asserted deliberately); (d) mag-gen dispatch: internal-magazine weapon routes to the internal gen, external to external, none-can-handle → `LootError`; (e) `add_extra_magazines_to_inventory` places mags via Task 5 grids, `NO_SPACE` stops cleanly (read the C# for the stop semantics first); (f) UBGL path.
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port bot weapon generation and magazine strategies to spt-native`

### Task 10: `bot_loot_generator.rs`

**Files:**
- Create: `rust/spt-native/src/bot/bot_loot_generator.rs`
- Modify: `bot/models.rs` (typed loot-pool entries, `ItemSpawnLimitSettingsWire`)

**Interfaces:**
- Produces: `pub fn generate_loot(ctx: &BotContext, grids: &mut ContainerGrids, inventory: &mut …, details: &BotGenerationDetailsWire) -> Result<(), LootError>` + internals mirroring the C# names (`add_loot_from_pool`, `create_wallet_loot`, `add_required_child_items_to_parent`, `add_loose_weapons_to_inventory_slot`, `randomise_money_stack_size`, `randomise_ammo_stack_size`, `get_item_spawn_limits_for_bot_type`, `get_matching_id_from_spawn_limits`, `item_has_reached_spawn_limit`, `add_forced_medical_items_to_pmc_secure`, `get_available_containers_bot_can_store_items_in`, `get_single_item_loot_price_limits`).
- Consumes: Tasks 2, 5, 9 (`generate_random_weapon` for loose weapons); `handbook_prices` projection.

**C# mapping — `Generators/Loot/BotLootGenerator.cs`:**

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `generate_loot` | 68–385 | **eleven weighted draws `:97-107` consumed BEFORE the `DisableLootOnBotTypes` zeroing `:110-116`** — draw first, zero after, exactly; backpack branch checks count>0 `:273` but vest branch checks only slot presence `:317` (asymmetry preserved); grenades never in backpack `:261`; secure container: 50 items hardcoded, `totalValueLimitRub = -1` `:376/:380` disables the `> 0` budget check `:600`; forced pscav meds `add_forced_medical_items_to_pmc_secure` `:429-460`; loose-weapon chance `:276` |
| `add_loot_from_pool` | 461–615 | pool mutation `:503` (`pool.Remove` + `i--` `:505` — iterate accordingly); `item_has_reached_spawn_limit` `:755-806` — **dead escape hatch `:789` (`count > count*10` always false), dead `return false` `:806`**; budget via `handbook_prices` `:602` with the `> 0` gate; placement via Task 5 grids, `NO_SPACE` → `break` `:589` |
| `create_wallet_loot` | 616–648 | weighted string stack-count draw then parse `:625/:631` (**generic `get_weighted_value` over string keys, insertion order**); grid placement of currency inside the wallet (2×2 walk — `InventoryHelper.GetContainerSlotMap`/`CanPlaceItemsInContainer`/`PlaceItemInContainer` equivalents; implement here or in Task 5's grid module, whichever keeps one grid implementation) |
| `add_required_child_items_to_parent` | 649–682 | uses `add_child_slot_items` (already ported) |
| `add_loose_weapons_to_inventory_slot` | 683–754 | `GetInt` `:699`, `GetArrayValue` `:693`; calls Task 9 `generate_random_weapon` |
| `randomise_money_stack_size` | 821–841 | **unguarded `currency_weights[money_tpl]` indexer `:829`** → `LootError` when missing |
| `randomise_ammo_stack_size` | 842–855 | **`is_pmc` parameter ignored `:842-848`** — keep the parameter, ignore it, comment `// parity: parameter unused in C#` |
| `get_item_spawn_limits_for_bot_type` / `get_matching_id_from_spawn_limits` | 856–905 | current-limits clone vs global reference `:47-57` semantics — Rust owns both copies, mirror outputs |

- [ ] **Step 1: Read the C# source in full**, plus `Services/Bot/BotLootCacheService.cs` far enough to enumerate the **13 `LootCacheType` pool reads** `GenerateLoot` performs (type + args per call site) — that list is Task 13's projection contract; write it into this module's doc-comment now.
- [ ] **Step 2: Failing tests** — fixture with pools (special/healing/drugs/food/drink/currency/stims/grenades/backpack/pocket/vest/secure): (a) seeded full `generate_loot` for a PMC-shaped details → pinned normalized items incl. dogtag-free inventory (dogtags are outside the boundary); (b) `DisableLootOnBotTypes` role → 11 draws still consumed (pin the next draw); (c) spawn-limit: item over its limit re-rolled, dead escape hatch asserted by a limit-1 pool of one tpl looping to pool exhaustion not explosion; (d) wallet loot: seeded → pinned currency stacks inside wallet grid; (e) money stack with missing weight key → `LootError`; (f) loose weapon in backpack with mags.
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port bot loot generation to spt-native`

### Task 11: `bot_inventory_generator.rs` — the orchestrator

**Files:**
- Create: `rust/spt-native/src/bot/bot_inventory_generator.rs`
- Modify: `bot/models.rs` (finalize `BotInventoryResult.inventory` as a typed `BotBaseInventoryWire`: `items: Vec<Item>` + the six container-root ids — mirror `Models/Eft/Common/Tables/BotBaseInventory` member names/casing)

**Interfaces:**
- Produces: `pub fn generate_inventory(request: GenerateBotInventoryRequest) -> Result<BotInventoryResult, LootError>` — the fn Task 12 exports. Installs `TestSeedGuard` from `request.test_seed` (plain `install` — single entry point, same as reward loot).
- Consumes: everything above.

**C# mapping — `Generators/Bot/BotInventoryGenerator.cs`:**

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `generate_inventory` | 80–125 | order: base → equipment → weapons → loot; `ClearCache(botId)` at `:116` becomes "emit grids in the result only when `!clear_bot_container_cache_after_generation`" (Task 14 replays) |
| `generate_inventory_base` | 126–167 | fresh MongoIds for the six roots |
| `generate_and_add_equipment_to_bot` | 168–425 | slot loop `:234` minus 4 excluded slots + 6 explicit calls (`:266/:293/:314/:335/:356/:397`); armband forcing `:219-223`; **the `:204` randomisation clamp** — apply it AND record into `result.randomisation_clamps` (slot → clamped value) for the C# replay; `wornItemChances` overrides `:394/:223`; `get_pocket_pool_by_game_edition` `:426-441` reads `details.game_version`; night-time gate reads `request.is_night_time` (C# `:195`); player level reads `request.generating_player_level` (C# `:228`, used 7×) |
| `filter_rigs_to_those_with_protection` / `…without_protection` | 442–494 | **overwrite the template's TacticalVest pool `:458/:487`**; the only call site passes `allowEmptyResult = true` `:388` → empty vest pool allowed, feeding the `:510` quirk |
| `generate_equipment` | 495–636 | `RootEquipmentPool?.Count != 0` **true for null pool** then `:516` NRE → `LootError`; `while (!found)` `:518` checks `attempts > maxAttempts` only in the incompatible branch — a draw hitting the `!dbResult.Key` branch `:528-541` never checks it (loop bounded by pool exhaustion `:520`); weighted draw `:525`; `RootEquipmentPool.Remove` `:537/:559`; `ModPool` write `:596`; incompat check via Task 5; `add_empty_container` `:622` |
| `get_filtered_dynamic_mods_for_item` | 637–680 | uses Task 6 `get_mods_for_gear_slot` |
| `generate_and_add_weapons_to_bot` / `get_desired_weapons_for_bot` / `add_weapon_and_magazines_to_inventory` | 681–777 | **`:718-730` short-circuit draw skipping: primary rolled once; second-primary roll NOT consumed when `shouldSpawnPrimary` false; holster roll NOT consumed when primary spawns** — replicate the exact `&&`/`||` short-circuits; weapon calls → Task 9; extra mags `:768` |

- [ ] **Step 1: Read the C# source in full.**
- [ ] **Step 2: Failing tests** — the module fixture becomes a mini bot template (equipment pools for 6 slots, weapon pools, chances, generation weights): (a) seeded end-to-end `generate_inventory` → pinned normalized full inventory; (b) night-time run → clamps present in `randomisation_clamps` and applied to subsequent slots within the same call; (c) `get_desired_weapons_for_bot` draw-skip: seeds chosen so primary=false → assert holster roll used the *second* stream value not the third; (d) pscav flag false → `container_grids` populated in result, true → empty; (e) armored-rig template + `FilterRigsToThoseWithoutProtection` → empty vest pool → the `:510`-quirk `LootError`.
- [ ] **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Gates + commit** — `feat: port bot inventory orchestration to spt-native`

### Task 12: FFI export + ABI bump

**Files:**
- Modify: `rust/spt-native/src/ffi.rs`, `rust/spt-native/src/lib.rs` (`ABI_VERSION` 4→5), `Libraries/SPTarkov.Server.Core/Native/SptNative.cs` (`ExpectedAbiVersion` 4→5 in the SAME commit)

**Interfaces:**
- Produces: `spt_generate_bot_inventory(req_ptr: *const u8, req_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32`

- [ ] **Step 1: Failing FFI tests** — mirror the existing export tests in `ffi.rs`: happy path (minimal valid request JSON from Task 11's fixture → `STATUS_OK` + parseable `BotInventoryResult`); invalid JSON → `STATUS_BAD_ARGS` with serde message; a `LootError`-triggering request (empty required pool per Task 11 test e) → `STATUS_ERROR`. Update the ABI pin test 4→5.
- [ ] **Step 2: Implement** — the ~4-line `run_generator` delegation, pattern-copied from `spt_create_random_loot`. Bump both version constants.
- [ ] **Step 3: Run** — full Rust gate suite + `dotnet build server-csharp.slnx` + `dotnet test --filter "FullyQualifiedName~SptNativeVerifyTests"` (proves lockstep).
- [ ] **Step 4: Commit** — `feat: expose bot inventory generation over C ABI and bump ABI to 5`

### Task 13: C# payloads + wrapper + projection

**Files:**
- Create: `Libraries/SPTarkov.Server.Core/Native/Bot/BotPayloads.cs`, `Libraries/SPTarkov.Server.Core/Native/Bot/BotPayloadProjection.cs`
- Modify: `Native/NativeMethods.cs` (1 `[LibraryImport]`), `Native/SptNative.cs` (wrapper + export-enum arm)
- Test: extend `Testing/UnitTests/Tests/Generators/` with a wire test inside Task 15's file later; here a minimal `SptNativeBotWireTests.cs`

**Interfaces:**
- Consumes: Task 1's wire names (member-for-member; run the Task 1 round-trip JSON through the C# serializer in the test to prove it).
- Produces: `internal record GenerateBotInventoryRequest(...)` etc. in `BotPayloads.cs`; `SptNative.GenerateBotInventory(GenerateBotInventoryRequest request) -> BotInventoryResult`; `internal static class BotPayloadProjection` with `BuildRequest(...)` assembling the full payload: reuses `PayloadProjection.BuildItemsView` + `ReplayDiagnostics` (shared with the loot ports — extend `ItemView` fields only if a bot-path read needs one that's missing; check against Tasks 5–11's `ItemView` field usage), projects the spec-§3 list: template blocks, details scalars, hoisted `generatingPlayerLevel` + `isNightTime`, config slices, `GetBotEquipmentBlacklist`/`GetBotWeaponSightWhitelist`, the **13 loot-pool reads exactly as doc-commented in Task 10 Step 1** (call `botLootCacheService.GetLootFromCache` with identical args — hydration side effects then match legacy), `ItemPresets`, preset maps, `GetBlacklistedItems()`, handbook price map over pool tpls.

- [ ] **Step 1: Failing wire test** — build a tiny but valid request via `BotPayloadProjection.BuildRequest` against the live test database (`DI.GetInstance()` singletons, assault role, level 1, `TestSeed = 42`), call `SptNative.GenerateBotInventory`, assert: deserializes, `Inventory.Items` non-empty, every id parses via `new MongoId(id)`, `RandomisationClamps` non-null. This pins the whole C#↔Rust wire.
- [ ] **Step 2: Run** → FAIL (types missing). **Step 3: Implement.** **Step 4: Run** → PASS + `dotnet test --filter "FullyQualifiedName~DependencyInjectionValidationTests"` green.
- [ ] **Step 5: csharpier + commit** — `feat: C# payloads and projection for native bot generation`

### Task 14: dispatch cutover in `BotInventoryGenerator`

**Files:**
- Modify: `Generators/Bot/BotInventoryGenerator.cs` (dispatch only — the 4.1.2 bodies move verbatim to private `GenerateInventoryLegacy` etc. with zero interior edits), `Models/Spt/Config/BotConfig.cs` (`public bool ForceLegacyBotGeneration { get; set; }` — absent JSON key deserializes false, same as `ForceLegacyLootGeneration`), `Services/Bot/BotInventoryContainerService.cs` (one `internal` restore method)

**Interfaces:**
- Consumes: Task 13's `BotPayloadProjection.BuildRequest` / `SptNative.GenerateBotInventory`.
- Produces: the seams Task 15 tests: `internal LootGenerationPath LastPathTaken { get; private set; }`, `internal ulong? NativeTestSeed { get; set; }` (copy the loot generators' declarations verbatim); `internal static readonly` hookable-member set.

- [ ] **Step 1: Build the widened hookable set.** Pattern-copy `LocationLootGenerator.cs:71-91`'s `_hookableMembers` reflection, but over **all four types** (`BotInventoryGenerator`, `BotEquipmentModGenerator`, `BotWeaponGenerator`, `BotLootGenerator`), `DeclaredOnly`, filter `IsPublic || IsFamily || IsFamilyOrAssembly` (catches `protected internal AddLootFromPool`), methods only (skip ctors/property accessors/`DesiredWeapons` DTO), **minus `BotInventoryGenerator.GenerateInventory` itself**. Static methods included (`GetMinMaxArmorPlateClass`, `GetAmmoContainers`, `MagGenSetUp` — Harmony patches statics).
- [ ] **Step 2: `UseLegacyPath()`** — copy the loot pattern, four conditions in order (cheap first): (1) `botConfig.ForceLegacyBotGeneration`; (2) `Harmony.GetPatchInfo` over the hookable set; (3) TypePriority: `botEquipmentModGenerator.GetType() != typeof(BotEquipmentModGenerator) || botWeaponGenerator.GetType() != typeof(BotWeaponGenerator) || botLootGenerator.GetType() != typeof(BotLootGenerator)` (the injected instances); (4) mag-gen set: read `BotWeaponGenerator`'s `protected readonly InventoryMagGenComponents` field via cached reflection (`typeof(BotWeaponGenerator).GetField("InventoryMagGenComponents", BindingFlags.Instance | BindingFlags.NonPublic)`), compare each element's `GetType()` against the pinned set of the four built-in implementations (enumerate them: `grep -rn ": IInventoryMagGen" Libraries/` and pin the four types in a static array).
- [ ] **Step 3: Convert `GenerateInventory` to the dispatcher.** Rename current body → `GenerateInventoryLegacy` (private, verbatim). New body: `if (UseLegacyPath()) { LastPathTaken = Legacy; return GenerateInventoryLegacy(...); }` else project (Task 13), call native, then replay: (a) `ReplayDiagnostics`; (b) deserialize `Inventory` to `BotBaseInventory` (the loot ports' deserialize-into-frozen-DTO pattern); (c) **grids**: when `!botGenerationDetails.ClearBotContainerCacheAfterGeneration`, rebuild `BotInventoryContainerService` state for this `botId` from `result.ContainerGrids` via the new `internal` restore method (shape from Task 5 — must satisfy `PlayerScavGenerator.cs:151-188`'s reads); when true, nothing (native never wrote the service); (d) **randomisation clamps**: for each entry, write into the live object `botHelper.GetBotRandomizationDetails(botGenerationDetails.BotLevel, botConfig.Equipment[role])`.`EquipmentMods[slot]` — the exact object legacy mutates at `:204` (verify the nighttime-only condition against `:195-210` and replay under the same condition). Native failure → throw with the native message (loot-port convention), no silent fallback.
- [ ] **Step 4: Run** — `dotnet build server-csharp.slnx && dotnet test` → all green (existing `BotWeaponGeneratorTests`/`BotGeneratorHelperTests` enter below the boundary and still run legacy code; anything driving `GenerateInventory` now runs native by default).
- [ ] **Step 5: csharpier + commit** — `feat: route bot inventory generation through spt-native behind the dual path`

### Task 15: parity, dispatch, hook-liveness, replay tests

**Files:**
- Create: `Testing/UnitTests/Tests/Generators/BotParityTests.cs`, `BotPathDispatchTests.cs`, `BotHookLivenessTests.cs`

**Interfaces:**
- Consumes: `LastPathTaken`/`NativeTestSeed` seams, `LootIdNormalizer` (reuse as-is), the `SaveServer.CreateProfile(new Info { ProfileId = sessionId })` stub pattern from `BotWeaponGeneratorTests`.

- [ ] **Step 1: Parity tests.** `[TestFixture] [NonParallelizable]`. Roles `["assault", "usec", "bear", "assault-as-playerscav"]` × seeds `[42, 1337]`. Per case: build `BotGenerationDetails` + cloned template from the live `BotTable` mirroring what `BotGenerator.GenerateBot` passes at `:284` (clone via `ICloner`, run `BotEquipmentFilterService.FilterBotEquipment` first — mirror `BotGenerator.cs:205` so both paths see the filtered template). **Pre-warm `BotLootCacheService` per role with one unseeded legacy generation before the seeded runs** — cache hydration must not consume seeded draws on either path. Then: native run (`NativeTestSeed = seed`), legacy run (`ForceLegacyBotGeneration = true` + `SeededRandomSource(seed)` into `_randomUtil.RandomSource`, exactly as `LootParityTests.Generate` does; restore in `finally`), assert `LastPathTaken` per path, both inventories non-empty, `LootIdNormalizer`-normalized JSON equality of the full `BotBaseInventory`. Also assert post-call `BotConfig` randomisation state is byte-equal between paths (serialize `botConfig.Equipment[role].Randomisation` after each, compare) — the clamp-replay check; save/restore the config object in `finally`.
- [ ] **Step 2: Dispatch tests.** (a) default → `LastPathTaken == Native`; (b) `ForceLegacyBotGeneration = true` → Legacy (restore in `finally`); (c) TypePriority: construct a `BotInventoryGenerator` manually passing a trivial `class TestBotLootGeneratorSubclass : BotLootGenerator` instance (same ctor args from DI) → Legacy; (d) mag-gen: construct passing a `BotWeaponGenerator` whose `InventoryMagGenComponents` includes a stub `IInventoryMagGen` (subclass or a hand-rolled impl) → Legacy — if the field can't be injected without DI, register the stub in an isolated `ServiceCollection` per the `ModCompatibilityTests` recipe instead; (e) grids: pscav-shaped details (`ClearBotContainerCacheAfterGeneration = false`) on the native path → `BotInventoryContainerService` state for that botId is non-empty and grid-consistent with the returned inventory (then clear it in `finally`, as `PlayerScavGenerator.cs:96` would).
- [ ] **Step 3: Hook-liveness tests.** Pattern-copy `LootHookLivenessTests.cs`: one live Harmony patch per class — `BotEquipmentModGenerator.SortModKeys` (public helper — proves the *widened* set), `BotWeaponGenerator.PickWeightedWeaponTemplateFromPool`, `BotLootGenerator.RandomiseMoneyStackSize`, `BotInventoryGenerator.GenerateEquipment` — each flips `LastPathTaken` to Legacy, unpatch in `finally`. Plus: a patch on `GenerateInventory` itself does NOT flip the path but its prefix/postfix observably fire around the native body.
- [ ] **Step 4: Run** — `dotnet test --filter "FullyQualifiedName~Bot"` → PASS. Any parity mismatch is a porting bug: normalize both JSONs, diff, locate the first diverging item/draw, fix in Rust with Tasks 7–11's quirk tables as the checklist, re-run.
- [ ] **Step 5: Full suite** — `dotnet test` green.
- [ ] **Step 6: csharpier + commit** — `test: seeded parity and dispatch coverage for bot generation`

### Task 16: benchmark, docs, final gates

**Files:**
- Create: `Testing/UnitTests/Tests/Generators/BotBenchmarkTests.cs`
- Modify: `ARCHITECTURE.md`, `todo/TODO.md`, `BENCHMARK.md`

- [ ] **Step 1: Benchmark** — pattern-copy `RewardLootBenchmarkTests.cs`: n=20 per path per role (assault + usec), Release-mode guidance in the header comment, report median ms/bot native vs legacy and the projection share (time `BuildRequest` separately). Record numbers in `BENCHMARK.md`.
- [ ] **Step 2: ARCHITECTURE.md** — new "Bot generation" section after the reward-loot one, covering (spec §8 verbatim where it lists limitations): the one-call-per-bot boundary at `GenerateInventory`; the four dispatch conditions incl. the **widened hookable set** (public helpers count — contrast with the loot ports' protected-only rule and say why); `ForceLegacyBotGeneration` (no ctor change — `BotConfig` was already injected); the two replays (container grids for pscav, live-config randomisation clamps); collaborator-patch limitations (`BotGeneratorHelper`, `DurabilityLimitsHelper`, `RepairService.AddBuff`, `BotWeaponGeneratorHelper`, `BotEquipmentModPoolService` incl. `ResetWeaponPool` no-op note, `BotLootCacheService`, `WeightedRandomHelper`, `ItemHelper` twins, `ICloner`); foreign `IInventoryMagGen` → automatic legacy; props-less templates; FFI-hang diagnosability note; benchmark numbers + the sanctioned items-view-cache follow-up if they're poor. Update the "Remaining agreed order" tail: bot family done; ragfair next per `todo/TODO.md`.
- [ ] **Step 3: todo/TODO.md** — mark #2 and #3 done (strike-through + pointer to the spec date, matching #1's format).
- [ ] **Step 4: Final gate loop (playbook rule 5)** —
  `dotnet build server-csharp.slnx -c Release` →
  `mpex-api-compat/ci/check-api-compat.sh` (all six assemblies clean — expected additions: `BotConfig.ForceLegacyBotGeneration` property only; everything else internal) →
  `dotnet test` →
  `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings` →
  `csharpier format .` →
  `graphify update .`
- [ ] **Step 5: Commit** — `docs: record the native bot generation boundary and widened hook contract`

---

## Self-review notes (already applied)

- **Spec coverage:** §1 boundary → Tasks 11/14; §2 dispatch → Task 14 (all four conditions), seams → Tasks 14/15; §3 payload → Tasks 1/13, grids+clamp replays → Tasks 5/11/14/15; §4 Rust modules → Tasks 3–11, primitives → Task 2, FFI → Task 12; §5 quirk catalog → distributed into Tasks 7–11 mapping tables (every spec quirk appears in exactly one table); §6 testing → Tasks 15/16 (+ per-task Rust tests); §7 perf → Task 16 benchmark + docs; §8 limitations → Task 16 docs; §9 gate → Task 16; §10 phasing → task order.
- **Loot-cache RNG hazard:** projection hydrates `BotLootCacheService` at a different time than legacy does; Task 15 pre-warms the cache before seeded runs so neither path spends seeded draws on hydration. If parity still diverges on first-run-per-role, hydration itself consumes RNG — pre-warm in `BuildRequest` callers is the fix, not a parity-test workaround; escalate to a plan revision if hit.
- **`sessionId` never crosses the FFI** (spec §3): `generatingPlayerLevel`/`isNightTime` hoisting in Task 13; frozen signatures keep the parameter.
- **Type consistency check done:** `BotContext` (Task 5) is the ctx type used in Tasks 6–11; `ContainerGrids` (Task 5) is the grids type in Tasks 8–11; `GenerateWeaponResultWire` produced in Task 9 consumed in Task 10's loose-weapon path; wire names of Task 1 are asserted against C# in Task 13 Step 1.
