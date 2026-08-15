# Mod-pool enumeration-order projection — design

**Date:** 2026-08-15
**Status:** approved in chat; this document is the binding spec
**Predecessors:** the bot family port (`2026-08-13-bot-family-port-design.md`) and the botwave
production wiring (`2026-08-14-botwave-production-wiring-design.md`). This is RUST-ROADMAP.md
roadmap item 5 and closes the first entry of its *Broken / known divergences* list. The port
playbook rules (RUST-ROADMAP.md § Guidelines) still bind: frozen 4.1.2 surface, verbatim legacy
path, Harmony-detection dispatch, project per call, lockstep FFI envelopes, full gate loop.

## 1. Goal and success criteria

PMC level-15+ bots diverge from legacy on both armor and weapon mod pools. The mechanism, traced
to the line: `BotEquipmentModPoolService`'s inner
`ConcurrentDictionary<string, HashSet<MongoId>>` (keyed by slot name) enumerates in hash-bucket
order, and both consumers freeze that order with `ToDictionary()` —
`BotInventoryGenerator.GetFilteredDynamicModsForItem` (`:901`) on the `RandomisedArmorSlots` path
and `BotEquipmentModGenerator.cs:739` on the `RandomisedWeaponModSlots` path. The draw loops
(`BotEquipmentModGenerator.cs:150` via a stable `OrderBy`, and `:553` via `SortModKeys`, whose
trailing `UnionWith` preserves the residual order) then roll RNG per slot in that order, so a
different slot order desynchronises every draw after the first. Rust's `derive_pool`
(`mod_pool_service.rs:123`) builds the same pools in database slot order — same membership,
different order. The inner `HashSet` values and the required-mods path are already
order-identical; **the slot-name order is the only seam.**

Success, measured on `dev` after the change:

1. New level-15+ `BotParityTests` cases (§5) — written first, failing on the ordering before the
   fix — pass byte-equal after it.
2. The existing eight level-1 parity cases and all other tests stay green: full `dotnet test`
   (Debug and Release), `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`.
3. One `BotBenchmarkTests` run confirms the request growth (~27 KB on ~4.6 MiB) moved nothing
   outside noise. No benchmark gate.
4. RUST-ROADMAP.md and `rust/ARCHITECTURE.md` updated (§6).

## 2. Approach: project the order, never the hash

Rejected alternatives: emulating .NET's string hashing and `ConcurrentDictionary` bucket layout in
Rust (fragile, version-coupled, already rejected by the roadmap), and reordering the projected
`slots` array itself (`GetRequiredModsForWeaponSlot` reads it in database order — reordering it
fixes one path by breaking another).

Instead the C# side tells Rust the order it will be held to. On net10.0 the order is deterministic
for a fixed insertion history (non-randomized ordinal string comparer, dotnet/runtime#81557), and
projecting it per call from the **live service** keeps guideline 3 intact — the service's lazy
singletons make the query a dictionary walk, and a runtime pool mutation or a patched service's
ordering is reflected on the next call automatically.

## 3. Wire change (ABI 10 → 11)

One new optional map on both bot request shapes — `GenerateBotInventoryRequest` and the batch
envelope's `SharedBotViews` (call-invariant, so the wave pays it once):

```
modPoolSlotOrder: { [tpl: MongoId]: int[] }
```

- One entry per template that appears in a mod pool **and** in the projected items view **and**
  whose pool has ≥ 2 slot names (696 templates on shipped data; order cannot matter below 2).
- The `int[]` lists the pool's slot names in the service's enumeration order, each name encoded as
  the index of its **first** occurrence in that template's already-projected `slots` array
  (~27 KB total; names would be ~68 KB and can drift from the slot data).
- Projection (in the bot payload projection, which gains a `BotEquipmentModPoolService`
  dependency): per projected template, query `GetModsForGearSlot(tpl)` and, if that yields no
  entries, `GetModsForWeaponSlot(tpl)`; enumerate the first non-empty result's keys. A template present in
  both pools has the same inner-dictionary construction history in each (same slots, same
  insertion sequence, same comparer), hence the same enumeration order — one map serves both.
- Both pools are built by the same `GeneratePool` walk; the projection never touches the outer
  dictionaries' order (they are only ever indexed by key).

`ABI_VERSION` (`lib.rs`) and `SptNative.ExpectedAbiVersion` bump to 11 together, plus the `ffi.rs`
abi test literal.

## 4. Rust change

`derive_pool` (`mod_pool_service.rs:123`) is the single divergence point; everything downstream
(`get_filtered_dynamic_mods_for_item`, `sort_mod_keys`, the draw loops) already preserves its
`IndexMap` order faithfully. It gains the order list as an input:

- Build the `IndexMap<String, IndexSet<String>>` in database slot order exactly as today.
- If the request carries an order list for the template: reorder the map to list the named slots
  first, in projected order (index → `slots[i].name`, first-occurrence rule matching §3), then any
  remaining entries in database order. Out-of-range indices are skipped. This makes the fallback
  total: no list, a partial list, or a stale list all yield a deterministic order and never a
  panic — and **no list at all reproduces today's behavior byte-for-byte**, which keeps the eight
  existing level-1 parity cases meaningful as regression pins.
- Membership stays derived in Rust from the slot data, as today. The projection carries order
  only.

`BotContext` (or the request structs in `bot/models.rs`) carries the map to the service's call
sites; both the single-bot and batch entry points thread it through.

## 5. Tests

Rust (`mod_pool_service.rs`):

- New: an order list reorders `derive_pool`'s output; a partial list front-loads the named slots
  and appends the rest in database order; an out-of-range index is skipped; no list keeps
  database order (the existing eight tests keep their database-order assertions and pin exactly
  that arm).

C# (`BotParityTests`):

- New cases at a level inside the `randomisedArmorSlots`/`randomisedWeaponModSlots` buckets
  (level ≥ 15; use the shipped pmc buckets) for `usec` and `bear`, two seeds each, deep-equal via
  the existing `LootJsonAssert`/`LootIdNormalizer` machinery. **Written first**; they must fail on
  mod-slot ordering against the unfixed native path, proving they exercise the divergence.
- `TheNighttimeRandomisationClampIsReplayedOnBothPaths` (`BotParityTests.cs:113`) currently
  declines to compare inventories, with a doc comment naming this divergence as the reason.
  Upgrade it to a full inventory comparison and delete that comment.
- The bot wire-pin fixture gains the new field on its round-trip request, mirroring how the other
  optional request members are pinned.

## 6. Documentation

- RUST-ROADMAP.md: remove the divergence from *Broken / known divergences*; mark roadmap item 5
  done with a one-line result. The *patches on collaborators* bullet keeps
  `BotEquipmentModPoolService` — content patches on it still do not reach the native path (§7).
- `rust/ARCHITECTURE.md`: `mod_pool_service.rs` row notes the projected order; the layout table's
  stale "`ABI_VERSION` (currently 8)" is corrected in passing to 11 while the bump edits that line.

## 7. Out of scope

- **Content patches on `BotEquipmentModPoolService`.** A patched `GetModsFor*Slot` changes
  membership, not just order; Rust still derives membership itself, so such patches still don't
  reach the native path. Documented, unchanged. Do not add the service to `_hookableMembers` —
  that would force every wave to legacy.
- Full-output golden tests for the loot and bot families beyond the new parity cases (roadmap
  item 6).
- Any change to the C# legacy draw order — the frozen 4.1.2 behavior is the oracle, not the
  patient.
