# Database mutation stamp + Rust-resident ragfair slice cache

**Date:** 2026-08-15
**Status:** Approved
**Roadmap:** RUST-ROADMAP.md item 1 (database mutation stamp, then a cached request slice)

## Problem

The ragfair *regeneration* pass (`RagfairServer.ProcessExpiredFleaOffers` →
`RagfairOfferGenerator.GenerateDynamicOffers`, fired on the ~10 s tick once ≥1400 offers expire)
spends ~60% of its ~75 ms median rebuilding and serialising a ~5.8 MB request slice that is
call-invariant, then pays the Rust-side JSON parse of that slice again. Guideline 3 (project per
call, never cache) forbids caching it without a staleness signal, because mods mutate the database
at runtime.

### Premise correction to the roadmap item

The roadmap assumed a `DatabaseService`/`DatabaseServer` facade whose mutation paths a counter
could instrument. **Neither exists in this fork.** The database is ten DI singletons
(`TemplateTable`, `GlobalTable`, `BotTable`, …) registered directly in
`SPTarkov.Server/Helpers/ProgramHelpers.cs:59-68`; server code and mods inject the table object
and write into its dictionaries with no method call in between. There is no chokepoint, so no
stamp can see a direct dictionary write by a mod. What *can* be seen:

- **Server-internal post-first-pass mutators of the slice's inputs — exactly two:**
  `SeasonalEventService.UpdateGlobalEvents` (reachable at any moment via the force-event chat
  commands) and the native ragfair path itself, which writes `CanSellOnRagfair = false` back into
  the live template table each call (`RagfairOfferGenerator.cs:402-412`). Everything else
  (`PostDbLoadService`, `RagfairPriceService.Load`) runs before the first flea pass.
- **Mod-facing service chokepoints:** `CustomItemService` (writes items, handbook and prices) and
  `ItemFilterService.AddItemToBlacklistCache` / `AddItemToLootableBlacklistCache`.
- **Invisible forever:** a mod writing a table dictionary directly. Handled by the eligibility
  gate below, never by the stamp.

## Design

### 1. `DatabaseMutationStamp` (C#)

A new `[Injectable]` singleton in `Libraries/SPTarkov.Server.Core`: `long Current` read via
`Interlocked.Read`, `void Bump()` via `Interlocked.Increment`. One-line bump sites:

| Site | Note |
|---|---|
| `SeasonalEventService.UpdateGlobalEvents` | Covers startup enable and runtime chat-command force. |
| `ItemFilterService.AddItemToBlacklistCache` + lootable sibling | Mod-only surface, but a real chokepoint. |
| `CustomItemService` item creation | The common mod route; hits items, handbook and prices at once. |
| `CanSellOnRagfair` replay in `RagfairOfferGenerator` | **Guarded:** bump only when a write flips a value from `true`. Unguarded, re-reported already-`false` templates would bump every pass and the cache would never hit. Guarded, it converges: first pass rejects and bumps, second rebuilds with `false` baked in, later passes hit. |

Frozen classes (`SeasonalEventService`, `ItemFilterService`, `CustomItemService`,
`RagfairOfferGenerator`) receive the stamp via an **additive constructor overload** — the
`LootGenerator` precedent: the frozen 4.1.2 constructor stays verbatim, the container selects the
wider overload, apicompat stays green.

### 2. Wire protocol (ABI 12 → 13)

`GenerateDynamicOffersRequest` splits into:

```
{ invariantStamp: u64, invariant?: InvariantSlice, varying: VaryingFields }
```

- `varying` — the four per-call fields: `expiredOffers`, `timestamp`, `offerCounterStart`,
  `testSeed`.
- `invariant` — everything else (items view, handbook/trader prices, presets, flea prices,
  blacklists, seasonal state, pmc names, `dynamic` config), now optional.

Rust keeps one process-lifetime cache slot: `static Mutex<Option<(u64, InvariantSlice)>>` (the
caller is a single sequential C# singleton; contention is nil).

- `invariant` present → parse, store under `invariantStamp`, use.
- `invariant` absent → use the cached slice when the stamp matches; otherwise return a distinct
  `CacheMiss` error code. C# retries **once** with the full slice. C# tracks Rust's state
  correctly in-process, so the retry is a self-healing backstop, not a normal path.

On a hit, C# builds and serialises only the tiny varying object: the ~14 ms `BuildRequest`, the
~5.8 MB serialise and the Rust-side parse all disappear. On a miss, cost is exactly today's.

### 3. Eligibility gate (soundness)

C# sends slice-less requests only when `loadedMods.Count == 0`. With mods present every call is a
full send — runtime mod mutations stay visible per guideline 3 and behaviour is byte-identical to
today. Two additive `RagfairConfig` properties:

- `TrustNativeRequestCacheWithMods` — **opt-in** for modded installs whose mods don't write
  tables mid-run (the instrumented chokepoints cover the common routes; direct dictionary writes
  are the documented residual risk);
- `DisableNativeRequestCache` — **kill switch** forcing full sends everywhere
  (`ForceLegacy*`-style escape hatch; also for tests that mutate fixture data between calls).

### 4. Testing

- **C#:** same-seed run twice → the second run hits the cache and produces byte-identical offers;
  a bump between runs (forced seasonal event) forces a full send whose output reflects the change;
  replay-guard bump/no-bump; mods-present always full-sends; a `CacheMiss` reply triggers exactly
  one retry.
- **Rust:** unit tests for store / lookup / stamp-mismatch.
- **Existing gates:** all 19 seeded `RagfairParityTests`, full `dotnet test`,
  `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`,
  `mpex-api-compat/ci/check-api-compat.sh`.
- **Benchmark:** re-measure the regeneration pass; update BENCHMARK.md, RUST-ROADMAP.md and
  ARCHITECTURE.md (orientation level only).

## Deliberate scope cuts

- One global counter, no per-table granularity — a false invalidation costs one full send.
- No caching for the bot or reward-loot paths yet; the stamp makes them possible later.
- No attempt to observe direct dictionary writes by mods — impossible without wrapping ten table
  singletons; that is what the eligibility gate is for.
