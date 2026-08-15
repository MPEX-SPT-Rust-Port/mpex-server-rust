# Mod-Pool Enumeration-Order Projection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make native bot generation draw randomised mod slots in the same order as legacy C#, closing the PMC level-15+ parity divergence (RUST-ROADMAP.md roadmap item 5).

**Architecture:** C# projects the live `BotEquipmentModPoolService` pools' slot-name enumeration order onto the bot request as per-template lists of indices into the already-projected `slots` arrays (~27 KB). Rust's `derive_pool` — the single divergence point — reorders its `IndexMap` by that list, with database order as the total fallback. ABI bumps 10 → 11 in lockstep.

**Tech Stack:** C# (.NET 10, NUnit) + Rust (1.97.1, serde/indexmap). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-15-mod-pool-order-projection-design.md`

## Global Constraints

- Rust gate: `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings` — green at every commit.
- C# gate: `dotnet test` (Debug) green at every commit; `csharpier format .` before the final commit; C# style rules from CLAUDE.md (always brace bodies, no expression-bodied members, `_camelCase` private fields).
- `ABI_VERSION` (`rust/spt-native/src/lib.rs:8`) and `ExpectedAbiVersion` (`Libraries/SPTarkov.Server.Core/Native/SptNative.cs:52`) change **in the same commit** (Task 2). No other file carries the literal — the `ffi.rs` test asserts against `crate::ABI_VERSION`.
- Guideline 3 (RUST-ROADMAP.md): project per call, never cache. `BuildModPoolSlotOrder` queries the live service on every request build.
- The C# legacy path is the oracle; no change to any frozen 4.1.2 class surface. The only C# production-code changes are in `Native/Bot/BotPayloads.cs`, `Native/Bot/BotPayloadProjection.cs`, one argument at `Generators/Bot/BotInventoryGenerator.cs:216`, and one primary-constructor parameter + argument in `Generators/Bot/BotWaveBatcher.cs` (fork-added class, not frozen).
- Commit messages follow the repo's `type: lowercase summary` style and end with the Claude co-author trailer.

---

### Task 1: Pin the divergence — level-15+ parity cases that fail today

**Files:**
- Modify: `Testing/UnitTests/Tests/Generators/BotParityTests.cs`

**Interfaces:**
- Consumes: the fixture's existing `Generate`, `PreWarmLootCache`, `BuildCase`, `_seeds` members (all present at `BotParityTests.cs:36-304`).
- Produces: test method `TheSameSeedGeneratesEquivalentInventoryAtRandomisedLevels` and roles `"usec-level20"` / `"bear-level20"`, which Task 4 un-ignores.

- [ ] **Step 1: Add the two level-20 cases to `BuildCase`.** In the `details` switch (after the `"usec-at-night"` arm, `BotParityTests.cs:265-274`), add:

```csharp
            // Daytime twins of the nighttime case: levels inside the pmc randomisation buckets
            // (15+) that set RandomisedArmorSlots and RandomisedWeaponModSlots, which route the
            // armor and weapon mod pools through BotEquipmentModPoolService's enumeration order
            "usec-level20" => new BotGenerationDetails
            {
                Role = "pmcUSEC",
                RoleLowercase = "pmcusec",
                Side = "Usec",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotLevel = 20,
                IsPmc = true,
            },
            "bear-level20" => new BotGenerationDetails
            {
                Role = "pmcBEAR",
                RoleLowercase = "pmcbear",
                Side = "Bear",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotLevel = 20,
                IsPmc = true,
            },
```

In the `templateKey` switch directly below (`:290-295`), add two arms before the default:

```csharp
            "usec-level20" => "usec",
            "bear-level20" => "bear",
```

- [ ] **Step 2: Add the failing test.** After `TheSameSeedGeneratesEquivalentInventoryOnBothPaths` (`:75-94`), add the value source and the test — **without** the `[Ignore]` attribute yet:

```csharp
    private static readonly string[] _randomisedRoles = ["usec-level20", "bear-level20"];

    /// <summary>
    /// The level-1 cases above sit below the pmc randomisation buckets, so they never route a mod
    /// pool through BotEquipmentModPoolService. These two do - level 20 selects buckets that set
    /// RandomisedArmorSlots and RandomisedWeaponModSlots - which is exactly the enumeration-order
    /// seam the modPoolSlotOrder projection exists for.
    /// </summary>
    [Test]
    public void TheSameSeedGeneratesEquivalentInventoryAtRandomisedLevels(
        [ValueSource(nameof(_randomisedRoles))] string role,
        [ValueSource(nameof(_seeds))] ulong seed
    )
    {
        PreWarmLootCache(role);

        var native = Generate(role, seed, forceLegacy: false, LootGenerationPath.Native);
        var legacy = Generate(role, seed, forceLegacy: true, LootGenerationPath.Legacy);

        LootJsonAssert.AssertEqual(legacy.Inventory, native.Inventory, $"role={role}", seed);
    }
```

- [ ] **Step 3: Run to verify all four cases FAIL on the ordering.**

Run: `dotnet test --filter "FullyQualifiedName~BotParityTests.TheSameSeedGeneratesEquivalentInventoryAtRandomisedLevels"`
Expected: 4 FAIL, each with a `LootJsonAssert` inventory diff. **Record where the first diff sits** (it should be in equipped armor plates or weapon mods — the randomised-slot output). If any case fails for a reason that is plainly *not* inventory content (an exception, a path-dispatch assert), STOP and report — that is a different bug, not this divergence.

- [ ] **Step 4: Ignore the cases so the tree stays green.** Add directly above the `[Test]` attribute:

```csharp
    [Ignore("fails until the mod-pool enumeration order is projected - docs/superpowers/specs/2026-08-15-mod-pool-order-projection-design.md")]
```

- [ ] **Step 5: Run to verify green.**

Run: `dotnet test --filter "FullyQualifiedName~BotParityTests"`
Expected: existing 9 pass, 4 skipped, 0 fail.

- [ ] **Step 6: Commit**

```bash
git add Testing/UnitTests/Tests/Generators/BotParityTests.cs
git commit -m "test: pin the level-15+ mod-pool ordering divergence (ignored until projected)"
```

---

### Task 2: Rust — order-aware `derive_pool`, wire structs, ABI 11

**Files:**
- Modify: `rust/spt-native/src/bot/mod_pool_service.rs` (derive_pool, its two pool callers, tests)
- Modify: `rust/spt-native/src/bot/mod.rs` (BotContext field, `NO_MOD_POOL_ORDER` static)
- Modify: `rust/spt-native/src/bot/models.rs` (field on `GenerateBotInventoryRequest` ~`:637` and `SharedBotViewsWire` ~`:669`)
- Modify: `rust/spt-native/src/bot/bot_inventory_generator.rs` (thread the field through `generate_inventory`'s repack ~`:166-224`, `generate_one`'s destructure ~`:277-303`, and the `BotContext` literal at `:312`)
- Modify: `rust/spt-native/src/lib.rs:8` (`ABI_VERSION` 10 → 11)
- Modify: `Libraries/SPTarkov.Server.Core/Native/SptNative.cs:52` (`ExpectedAbiVersion` 10 → 11)
- Modify: the six other test-fixture `BotContext { ... }` literals (`bot_equipment_mod_generator.rs` ×2, `bot_generator_helper.rs`, `bot_loot_generator.rs`, `bot_weapon_generator.rs`, `bot_weapon_generator_helper.rs`, `inventory_mag_gen.rs` — let the compiler's missing-field errors drive)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: wire field `modPoolSlotOrder` (`IndexMap<String, Vec<usize>>`, `#[serde(default)]`) on both request envelopes — the exact name/type Task 3's C# `[JsonPropertyName("modPoolSlotOrder")] Dictionary<MongoId, List<int>>` serialises to; `BotContext.mod_pool_slot_order: &'a IndexMap<String, Vec<usize>>`; `#[cfg(test)] pub(crate) static NO_MOD_POOL_ORDER` in `bot/mod.rs`.

- [ ] **Step 1: Write the failing tests** in `mod_pool_service.rs`'s test module. First give the `Fixture` an order map: add field `order: IndexMap<String, Vec<usize>>` (initialise `IndexMap::new()` in `Fixture::new`), and in `Fixture::ctx` add `mod_pool_slot_order: &self.order,` to the `BotContext` literal. Then add the tests:

```rust
    /// WEAPON_TPL's pool in database order is [mod_magazine, mod_scope] (indices 0 and 1 of its
    /// slots; index 2, mod_stock, has an empty filter and is not in the pool).
    #[test]
    fn a_projected_order_reorders_the_pool() {
        let mut fixture = Fixture::new();
        fixture.order.insert(WEAPON_TPL.to_owned(), vec![1, 0]);

        let keys: Vec<String> = get_mods_for_weapon_slot(&fixture.ctx(), WEAPON_TPL)
            .keys()
            .cloned()
            .collect();

        assert_eq!(keys, ["mod_scope", "mod_magazine"]);
    }

    #[test]
    fn a_partial_order_front_loads_the_named_slots_and_appends_the_rest_in_database_order() {
        let mut fixture = Fixture::new();
        fixture.order.insert(WEAPON_TPL.to_owned(), vec![1]);

        let keys: Vec<String> = get_mods_for_weapon_slot(&fixture.ctx(), WEAPON_TPL)
            .keys()
            .cloned()
            .collect();

        assert_eq!(keys, ["mod_scope", "mod_magazine"]);
    }

    /// 9 is out of range and 2 is the empty-filter slot the pool never held — both are skipped
    /// rather than panicking, so a stale projection degrades to a deterministic order.
    #[test]
    fn out_of_range_and_poolless_indices_are_skipped() {
        let mut fixture = Fixture::new();
        fixture.order.insert(WEAPON_TPL.to_owned(), vec![9, 2, 1, 0]);

        let keys: Vec<String> = get_mods_for_weapon_slot(&fixture.ctx(), WEAPON_TPL)
            .keys()
            .cloned()
            .collect();

        assert_eq!(keys, ["mod_scope", "mod_magazine"]);
    }

    /// No entry for the tpl means database order — the pre-projection behavior, byte for byte,
    /// which is what an old caller without the field still gets.
    #[test]
    fn no_order_keeps_database_order() {
        let fixture = Fixture::new();

        let keys: Vec<String> = get_mods_for_weapon_slot(&fixture.ctx(), WEAPON_TPL)
            .keys()
            .cloned()
            .collect();

        assert_eq!(keys, ["mod_magazine", "mod_scope"]);
    }

    /// The gear pool consults the same projected order.
    #[test]
    fn the_gear_pool_reorders_too() {
        let mut fixture = Fixture::new();
        fixture.order.insert(PLATE_CARRIER_TPL.to_owned(), vec![1, 0]);

        let keys: Vec<String> = get_mods_for_gear_slot(&fixture.ctx(), PLATE_CARRIER_TPL)
            .keys()
            .cloned()
            .collect();

        assert_eq!(keys, ["back_plate", "front_plate"]);
    }
```

- [ ] **Step 2: Make it compile enough to fail.** Add to `BotContext` in `bot/mod.rs` (after `secure_container_ammo_stack_count`):

```rust
    /// `modPoolSlotOrder` — the C# `BotEquipmentModPoolService` pools' slot-name enumeration
    /// order per template, as indices into that template's `slots`. Only order crosses the wire;
    /// membership is still derived by [`crate::bot::mod_pool_service`]. Missing entry = database
    /// order.
    pub mod_pool_slot_order: &'a IndexMap<String, Vec<usize>>,
```

and the test stand-in next to the other `NO_*` statics:

```rust
#[cfg(test)]
pub(crate) static NO_MOD_POOL_ORDER: std::sync::LazyLock<IndexMap<String, Vec<usize>>> =
    std::sync::LazyLock::new(IndexMap::new);
```

Fix every `missing field` compile error the new `BotContext` field raises: the six other test
fixtures get `mod_pool_slot_order: &crate::bot::NO_MOD_POOL_ORDER,`. The production literal
(`bot_inventory_generator.rs:312`) cannot use that static (`#[cfg(test)]`), so for this
intermediate step declare `let no_order = IndexMap::new();` above it and pass
`mod_pool_slot_order: &no_order,` — Step 6 replaces both lines with the wire field.

- [ ] **Step 3: Run to verify the new tests fail.**

Run: `cd rust && cargo test mod_pool_service`
Expected: `a_projected_order_reorders_the_pool`, `a_partial_order_...`, `out_of_range_...`, `the_gear_pool_reorders_too` FAIL (order still database); `no_order_keeps_database_order` and the existing eight PASS.

- [ ] **Step 4: Implement.** In `mod_pool_service.rs`, change `derive_pool` to:

```rust
/// The per-item half of `GeneratePool` (`:53-119`): each slot with a non-empty first filter becomes
/// an entry keyed by the slot name. Slots sharing a name merge, as C#'s `GetOrAdd` does.
///
/// `slot_order` is the C# service's enumeration order, projected as indices into `slots`
/// (`modPoolSlotOrder`). Entries it names come first, in its order; anything it does not name
/// keeps database order behind them — so no list, a partial list and a stale list all yield a
/// deterministic pool, and no list at all is the pre-projection behavior byte for byte.
fn derive_pool(
    items: &IndexMap<String, ItemView>,
    item_tpl: &str,
    slot_order: Option<&Vec<usize>>,
) -> IndexMap<String, IndexSet<String>> {
    let slots = get_item(items, item_tpl)
        .and_then(|item| item.slots.as_deref())
        .unwrap_or_default();

    let mut pool: IndexMap<String, IndexSet<String>> = IndexMap::new();

    for slot in slots {
        // No mod items in whitelist, skip
        let compatible_mods = slot.filter.as_deref().unwrap_or_default();
        if compatible_mods.is_empty() {
            continue;
        }

        pool.entry(slot.name.clone().unwrap_or_default())
            .or_default()
            .extend(compatible_mods.iter().cloned());
    }

    let Some(order) = slot_order else {
        return pool;
    };

    let mut reordered: IndexMap<String, IndexSet<String>> = IndexMap::with_capacity(pool.len());
    for &index in order {
        let Some(name) = slots.get(index).and_then(|slot| slot.name.as_deref()) else {
            continue;
        };
        if let Some(entry) = pool.shift_remove(name) {
            reordered.insert(name.to_owned(), entry);
        }
    }
    reordered.extend(pool);

    reordered
}
```

(`shift_remove`, not `swap_remove` — the residue must keep database order.) Update the two callers:

```rust
    derive_pool(ctx.items, item_tpl, ctx.mod_pool_slot_order.get(item_tpl))
```

in both `get_mods_for_gear_slot` and `get_mods_for_weapon_slot`. `get_required_mods_for_weapon_slot` is untouched — C# reads `Properties.Slots` directly in database order there, and the existing test pins it.

- [ ] **Step 5: Run to verify the new tests pass.**

Run: `cd rust && cargo test mod_pool_service`
Expected: all PASS.

- [ ] **Step 6: Wire the field.** In `bot/models.rs`, add to **both** `GenerateBotInventoryRequest` (after `items`, ~`:637`) and `SharedBotViewsWire` (after `items`, ~`:669`):

```rust
    /// The C# `BotEquipmentModPoolService` pools' slot-name enumeration order per template, as
    /// indices into that template's projected `slots` array. `#[serde(default)]` so an absent
    /// field means database order — today's behavior.
    #[serde(default)]
    pub mod_pool_slot_order: IndexMap<String, Vec<usize>>,
```

In `bot_inventory_generator.rs`:
1. `generate_inventory` (`:163`): add `mod_pool_slot_order,` to the `GenerateBotInventoryRequest` destructuring and to the `SharedBotViewsWire { ... }` literal it repacks into.
2. `generate_one` (`:270`): add `mod_pool_slot_order,` to the `SharedBotViewsWire` destructuring (`:277-303`) and replace Step 2's temporary local with `mod_pool_slot_order,` in the `BotContext` literal (`:312`), deleting the `let no_order` line.

Bump `rust/spt-native/src/lib.rs:8` to `pub const ABI_VERSION: u32 = 11;` and
`Libraries/SPTarkov.Server.Core/Native/SptNative.cs:52` to `private const uint ExpectedAbiVersion = 11;`.

- [ ] **Step 7: Full Rust gate.**

Run: `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 8: C# suite against the new library** (C# does not send the field yet; `serde(default)` must make that invisible).

Run: `dotnet test`
Expected: green (Task 1's four cases still skipped).

- [ ] **Step 9: Commit**

```bash
git add rust/spt-native/src/bot/mod_pool_service.rs rust/spt-native/src/bot/mod.rs \
        rust/spt-native/src/bot/models.rs rust/spt-native/src/bot/bot_inventory_generator.rs \
        rust/spt-native/src/lib.rs Libraries/SPTarkov.Server.Core/Native/SptNative.cs
git commit -m "feat: order-aware mod-pool derivation behind modPoolSlotOrder (ABI 11)"
```

---

### Task 3: C# — project the enumeration order onto the request

**Files:**
- Modify: `Libraries/SPTarkov.Server.Core/Native/Bot/BotPayloads.cs` (member on `GenerateBotInventoryRequest` and `SharedBotViews`, both after their `Items` member)
- Modify: `Libraries/SPTarkov.Server.Core/Native/Bot/BotPayloadProjection.cs` (`BuildModPoolSlotOrder` helper; parameter + member init in `BuildRequest` and `BuildSharedViews`)
- Modify: `Libraries/SPTarkov.Server.Core/Generators/Bot/BotInventoryGenerator.cs:216` (pass `botEquipmentModPoolService` — already a primary-constructor parameter at `:58`)
- Modify: `Libraries/SPTarkov.Server.Core/Generators/Bot/BotWaveBatcher.cs` (add `BotEquipmentModPoolService botEquipmentModPoolService` to the primary constructor at `:36`; pass it at the `BuildSharedViews` call `:255`)
- Modify: `Testing/UnitTests/Tests/Generators/SptNativeBotWireTests.cs:48` (`di.GetService<BotEquipmentModPoolService>()` argument + new pin test)
- Modify: `Testing/UnitTests/Tests/Generators/BotBatchTests.cs:222,251` and `Testing/UnitTests/Tests/Generators/BotPayloadSizeTests.cs:247,302` (same new argument)

**Interfaces:**
- Consumes: Task 2's wire field `modPoolSlotOrder: IndexMap<String, Vec<usize>>` (serde default).
- Produces: `[JsonPropertyName("modPoolSlotOrder")] public required Dictionary<MongoId, List<int>> ModPoolSlotOrder` on both records; `BuildRequest`/`BuildSharedViews` each gain a `BotEquipmentModPoolService botEquipmentModPoolService` parameter **directly after `botEquipmentFilterService`**.

- [ ] **Step 1: Write the failing pin test** in `SptNativeBotWireTests.cs` (after `ItemViewCarriesTheBotProjections`):

```csharp
    /// <summary>
    /// The mod-pool slot order rides the request so the native side can draw randomised mod slots
    /// in <c>BotEquipmentModPoolService</c>'s enumeration order. Indices point into the template's
    /// projected <c>slots</c> array, so each must be in range and unique.
    /// </summary>
    [Test]
    public void ModPoolSlotOrderIsProjectedAsSlotIndices()
    {
        // AK-74N again: a weapon whose pool holds several named slots, so it must carry an order
        var weaponTpl = new MongoId("5644bd2b4bdc2d3b4c8b4572");

        Assert.That(_request.ModPoolSlotOrder, Is.Not.Empty);
        Assert.That(_request.ModPoolSlotOrder.ContainsKey(weaponTpl), Is.True);

        var indices = _request.ModPoolSlotOrder[weaponTpl];
        var slotCount = _request.Items[weaponTpl].Slots!.Count;
        Assert.That(indices, Has.Count.GreaterThanOrEqualTo(2));
        Assert.That(indices, Is.Unique);
        Assert.That(indices, Is.All.InRange(0, slotCount - 1));
    }
```

- [ ] **Step 2: Run to verify it fails to compile** (`ModPoolSlotOrder` does not exist).

Run: `dotnet test --filter "FullyQualifiedName~SptNativeBotWireTests"`
Expected: build error naming `ModPoolSlotOrder`.

- [ ] **Step 3: Implement.** In `BotPayloads.cs`, add to **both** `GenerateBotInventoryRequest` and `SharedBotViews`, directly after each record's `Items` member:

```csharp
    /// <summary>
    /// <c>BotEquipmentModPoolService</c>'s pools' slot-name enumeration order per template, as
    /// indices into that template's <see cref="ItemView.Slots"/>. Only templates whose pool holds
    /// two or more slot names are listed - order cannot matter below two. Membership stays derived
    /// on the native side; this carries order alone.
    /// </summary>
    [JsonPropertyName("modPoolSlotOrder")]
    public required Dictionary<MongoId, List<int>> ModPoolSlotOrder { get; set; }
```

In `BotPayloadProjection.cs`, add the parameter `BotEquipmentModPoolService botEquipmentModPoolService`
directly after `BotEquipmentFilterService botEquipmentFilterService` in **both** `BuildRequest` and
`BuildSharedViews`, add to both return literals (next to `Items`):

```csharp
            ModPoolSlotOrder = BuildModPoolSlotOrder(botEquipmentModPoolService, itemHelper.TemplateTable.Items),
```

and add the helper (next to `BuildHandbookPrices`):

```csharp
    /// <summary>
    /// The slot-name enumeration order of <c>BotEquipmentModPoolService</c>'s pools, per template,
    /// as indices into the template's slots (the projected <c>slots</c> array is a 1:1
    /// <c>Select</c> of <c>Properties.Slots</c>, so the indices line up). Both consumers freeze
    /// the ConcurrentDictionary's order with <c>ToDictionary</c> before the draw loops walk it, so
    /// enumerating the dictionary here reads exactly the order the native side must draw in. A
    /// template present in both pools has the same inner-dictionary construction history in each -
    /// same slots, same insertion sequence, same comparer - so one map serves both.
    /// </summary>
    private static Dictionary<MongoId, List<int>> BuildModPoolSlotOrder(
        BotEquipmentModPoolService botEquipmentModPoolService,
        Dictionary<MongoId, TemplateItem> templates
    )
    {
        var order = new Dictionary<MongoId, List<int>>();

        foreach (var (tpl, template) in templates)
        {
            var slots = template.Properties?.Slots;
            if (slots is null || slots.Count < 2)
            {
                continue;
            }

            var pool = botEquipmentModPoolService.GetModsForGearSlot(tpl);
            if (pool.IsEmpty)
            {
                pool = botEquipmentModPoolService.GetModsForWeaponSlot(tpl);
            }
            if (pool.Count < 2)
            {
                continue;
            }

            var indices = new List<int>(pool.Count);
            foreach (var (slotName, _) in pool)
            {
                // First occurrence, matching the GetOrAdd merge of same-named slots
                for (var index = 0; index < slots.Count; index++)
                {
                    if (slots[index].Name == slotName)
                    {
                        indices.Add(index);
                        break;
                    }
                }
            }

            order[tpl] = indices;
        }

        return order;
    }
```

(If `Properties.Slots` is not a `List<Slot>` with `Count`/indexer, adapt the loop to the actual
collection type — the enumeration-then-index shape is what matters, not the accessor.) Add the
`using SPTarkov.Server.Core.Services.Bot;` directive if not already present (it is — `:13`).

Then update every caller with the new argument in the matching position:
- `BotInventoryGenerator.cs:216` → `botEquipmentModPoolService,` (the `:58` primary-ctor parameter)
- `BotWaveBatcher.cs` → add `BotEquipmentModPoolService botEquipmentModPoolService,` to the primary constructor and pass it at `:255`
- `SptNativeBotWireTests.cs:48`, `BotBatchTests.cs:222`, `BotBatchTests.cs:251`, `BotPayloadSizeTests.cs:247`, `BotPayloadSizeTests.cs:302` → `di.GetService<BotEquipmentModPoolService>(),`

- [ ] **Step 4: Run the wire tests.**

Run: `dotnet test --filter "FullyQualifiedName~SptNativeBotWireTests"`
Expected: all PASS, including `ModPoolSlotOrderIsProjectedAsSlotIndices`.

- [ ] **Step 5: Full C# suite** (DI validation catches the `BotWaveBatcher` constructor change; the batch and payload-size fixtures exercise `BuildSharedViews`).

Run: `dotnet test`
Expected: green (Task 1's four cases still skipped).

- [ ] **Step 6: Commit**

```bash
git add Libraries/SPTarkov.Server.Core/Native/Bot/BotPayloads.cs \
        Libraries/SPTarkov.Server.Core/Native/Bot/BotPayloadProjection.cs \
        Libraries/SPTarkov.Server.Core/Generators/Bot/BotInventoryGenerator.cs \
        Libraries/SPTarkov.Server.Core/Generators/Bot/BotWaveBatcher.cs \
        Testing/UnitTests/Tests/Generators/SptNativeBotWireTests.cs \
        Testing/UnitTests/Tests/Generators/BotBatchTests.cs \
        Testing/UnitTests/Tests/Generators/BotPayloadSizeTests.cs
git commit -m "feat: project the mod-pool enumeration order onto the bot request"
```

---

### Task 4: Prove parity — un-ignore the cases, upgrade the nighttime test

**Files:**
- Modify: `Testing/UnitTests/Tests/Generators/BotParityTests.cs`

**Interfaces:**
- Consumes: Task 1's ignored test + Task 2/3's fix.
- Produces: the level-15+ parity gate, green.

- [ ] **Step 1: Remove the `[Ignore(...)]` attribute** from `TheSameSeedGeneratesEquivalentInventoryAtRandomisedLevels`.

- [ ] **Step 2: Run — the four cases must now pass.**

Run: `dotnet test --filter "FullyQualifiedName~BotParityTests.TheSameSeedGeneratesEquivalentInventoryAtRandomisedLevels"`
Expected: 4 PASS. If any still fails, compare the diff against the one recorded in Task 1 Step 3: the same location means the projection or reorder is wrong (debug before proceeding — do NOT re-ignore); a *different* location means the ordering fix worked and uncovered a second, distinct divergence — STOP and report it with both diffs.

- [ ] **Step 3: Upgrade the nighttime case.** In `TheNighttimeRandomisationClampIsReplayedOnBothPaths`:
1. In its doc comment, delete the second paragraph ("It deliberately does not compare the two inventories. ... See the \"Bot generation\" limitations in ARCHITECTURE.md.") and replace it with:

```csharp
    /// With the mod-pool enumeration order projected (modPoolSlotOrder), the inventories compare
    /// too - this case is the only one that covers the nighttime clamp path at a randomised level.
```

2. Add the inventory comparison directly after the existing clamp asserts (`:141-142`):

```csharp
            LootJsonAssert.AssertEqual(legacy.Inventory, native.Inventory, "role=usec-at-night", seed);
```

- [ ] **Step 4: Run the whole fixture.**

Run: `dotnet test --filter "FullyQualifiedName~BotParityTests"`
Expected: 13 PASS, 0 skipped, 0 fail.

- [ ] **Step 5: Full suites, both configurations.**

Run: `dotnet test` then `cd rust && cargo test`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add Testing/UnitTests/Tests/Generators/BotParityTests.cs
git commit -m "test: level-15+ bot parity holds; nighttime case compares inventories"
```

---

### Task 5: Documentation, benchmark sanity, full gates

**Files:**
- Modify: `RUST-ROADMAP.md`
- Modify: `rust/ARCHITECTURE.md`
- Modify: `ARCHITECTURE.md` (root — only if it names the mod-pool ordering divergence; keep edits orientation-level per the repo convention)

- [ ] **Step 1: RUST-ROADMAP.md.**
1. Delete the first *Broken / known divergences* bullet ("**PMC level 15+ bots diverge from legacy** ...").
2. Replace roadmap item 5 with:

```markdown
5. ~~`ConcurrentDictionary` mod-pool ordering divergence~~ — done: the C# enumeration order
   crosses the FFI as per-template slot indices (`modPoolSlotOrder`, ABI 11), and Rust's
   `derive_pool` draws in that order with database order as the total fallback. Level-15+ parity
   pinned by `BotParityTests` (usec/bear at level 20, two seeds, plus the nighttime case now
   comparing inventories). Spec: `docs/superpowers/specs/2026-08-15-mod-pool-order-projection-design.md`.
```

3. Update ABI mentions that state the *current* version: the Status paragraph's "(ABI 10)" → "(ABI 11)". Leave historical phrasings ("introduced at ABI 10") alone; the *Exceptions in force* ragfair-envelope bullet's "(ABI **10**, encoding tag 1)" becomes "(since ABI **10**, encoding tag 1; current ABI is 11)".
4. In the *patches on collaborators* Broken bullet, `BotEquipmentModPoolService` stays listed — content patches on it still do not reach the native path (spec §7).

- [ ] **Step 2: rust/ARCHITECTURE.md.**
1. Layout table `src/lib.rs` row: "(currently 8; must equal ...)" → "(currently 11; must equal ...)".
2. `src/bot/` table `mod_pool_service.rs` row: "Slot mod pools, derived per call instead of cached" → "Slot mod pools, derived per call instead of cached, drawn in the projected C# enumeration order (`modPoolSlotOrder`)".

- [ ] **Step 3: root ARCHITECTURE.md.** `grep -n "ModPool\|mod pool\|enumeration order" ARCHITECTURE.md` — if the *Native Rust layer* limitations mention the ordering divergence, rewrite that line to say the order is projected since ABI 11 (one line, orientation-level). If nothing matches, skip.

- [ ] **Step 4: Benchmark sanity (spec §1.3 — no gate).**

Run: `dotnet test -c Release --filter "FullyQualifiedName~BotBenchmarkTests" --logger "console;verbosity=detailed"`
Expected: medians within noise of the pre-change run (native ~47-75 ms/bot, whichever role runs first paying more — that split is measurement order, not role). Record the medians for the final report. If native moved >10% against the same-run legacy, STOP and report before committing.

- [ ] **Step 5: Full gate loop.**

Run, in order:
1. `csharpier format .` — commit any churn separately as `style: csharpier`
2. `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
3. `dotnet test` (Debug) and `dotnet test -c Release` — the Release-only extension-data tests must run, not skip
4. `../mpex-api-compat/ci/check-api-compat.sh` if the sibling repo is checked out (no frozen surface changed, so this must pass untouched)
5. `graphify update .`

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add RUST-ROADMAP.md rust/ARCHITECTURE.md ARCHITECTURE.md
git commit -m "docs: record the mod-pool order projection; divergence closed at ABI 11"
```

(Drop root `ARCHITECTURE.md` from the add if Step 3 made no change.)
