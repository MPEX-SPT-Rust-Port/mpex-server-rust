# Ragfair native path to legacy parity — design

**Date:** 2026-08-14
**Status:** approved in chat; this document is the binding spec
**Predecessors:** the ragfair port (`2026-08-13-ragfair-port-design.md`) and its performance
diagnostics (`todo/RAGFAIR_DIAG_1.md`, `todo/RAGFAIR_DIAG_2.md`, both recorded 2026-08-14 at
`fce0b9a`, an ancestor of current `dev`). The port playbook rules (RUST-ROADMAP.md § Guidelines)
still bind: frozen 4.1.2 surface, verbatim legacy path, Harmony-detection dispatch, seeded-RNG
primitive parity, lockstep FFI envelopes, full gate loop. Guideline 5 explicitly permits envelope
format changes with an ABI bump.

## 1. Goal and success criteria

Close the ragfair full-pass gap: native 1550 ms vs legacy 517 ms measured 2026-08-14 (medians,
`RagfairBenchmarkTests`, Release, AMD Ryzen 5 5600H). **Parity is the requirement** — stage B is a
verified checkpoint on the way, not an exit ramp; stage C always runs.

Success, measured after stage C on the same machine in the same run:

1. Native full-pass median ≤ **1.25×** the same-run legacy full-pass median.
2. `RagfairParityTests` 19/19 byte-equal, unchanged.
3. Full `dotnet test` green; `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings` green.
4. `BENCHMARK.md` updated with the new figures and corrected methodology (§6).

If 1.25× is missed after stage C, the fallback lever is DIAG_2 §4.3 (build offer items directly
with fresh ids instead of clone-then-reparent, ~100 ms sequential) — in scope only in that case.

## 2. Stage A — parallelize the batch walk (Rust only, no ABI change)

`generate_dynamic_offers` (`rust/spt-native/src/ragfair/offer_generator.rs:402`) walks
`assort_items_to_process` sequentially — 710 ms of the 1550. Fan it out with rayon (already a
crate dependency, used by `generate_inventory_batch`) **only when `request.test_seed.is_none()`**:

- **Seeded runs stay sequential, byte-for-byte.** The seeded RNG is `thread_local`
  (`loot/random_util.rs`); the guard at `offer_generator.rs:405`
  (`request.test_seed.map(TestSeedGuard::install)`) keeps working exactly as today on the
  sequential path. All 19 `RagfairParityTests` cases set a seed, so they exercise the unchanged
  path. Production is already nondeterministic (legacy fans out one `Task.Factory.StartNew` per
  assort entry), so the unseeded parallel path breaks no promise.
- **Diagnostics move out of the shared `&mut RagfairContext`** to per-worker buffers, merged
  afterwards.
- **Per-worker `offers`/`rejected` merge in assort order**, and `int_id` is assigned during the
  merge so the counter stays sequential and output order is stable regardless of which worker
  finished first.

Expected: generation 710 → ~165 ms (DIAG_2 §4.1 measured a 12-thread scoped-thread prototype at
165 ms; rayon on the same 12 hardware threads is equivalent).

## 3. Stage B — framed response, direct deserialize (ABI 8 → 9)

Two independent costs die here: the single-threaded 400 ms C# response deserialize and the 90 ms
wire→DTO map.

**Envelope.** The response becomes a framed binary envelope, designed so stage C changes only the
payload encoding:

- header: envelope version, payload-encoding tag (`0` = JSON), offer count, then the non-offer
  sections (`rejectedCanSellTemplates`, diagnostics, counters) — small, encoding per the tag;
- body: one length-prefixed frame per offer, payload encoded per the tag.

Rust already knows each offer's byte length as it serializes, so framing is near-free.
`ABI_VERSION` (`rust/spt-native/src/lib.rs:8`) bumps 8 → 9 in lockstep with
`SptNative.ExpectedAbiVersion`.

**C# deserialize.** `Parallel.For` over raw frame spans,
`JsonSerializer.Deserialize<RagfairOffer>` per span — **straight into `RagfairOffer`**.
`RagfairOfferWire` and `ToRagfairOffer` are deleted; `RagfairOffer.User` drops `required` and
`Requirements` becomes a concrete `List`. Deserializing raw byte ranges is load-bearing:
DIAG_2 §4.2 measured `JsonElement.Deserialize` losing most of the win to internal re-serialization.

**Converter tax.** The MongoId converter + Ceciler-injected `[JsonExtensionData]` dictionary cost
~129 ms across ~83k `Item`/`Upd` instances (DIAG_1). The extension-data *mechanism* stays — `Item`
needs it for mod fields to round-trip — but the converter read path gets a measured fix (faster
MongoId parse; avoid the extension-dictionary allocation when the payload has no unknown fields,
if achievable without changing round-trip semantics). This lever is measure-first: profile, fix,
re-measure; no semantic change to what round-trips.

Expected: response leg ~490 → ~100–170 ms (DIAG_2 §4.2: 71–232 ms parallel under server GC —
which production uses — 224–265 ms under workstation GC).

## 4. Stage C — MessagePack payloads in the same frames (ABI 9 → 10)

The payload-encoding tag flips to `1` = MessagePack; the frame structure from stage B is
untouched, so stage B's parallel-deserialize machinery carries over.

- Rust: `rmp-serde` **pinned at 1.3.1** serializes each offer (and the header sections) instead of
  `serde_json`. Field-name (map) encoding, not tuple encoding, so mod-facing extension fields
  survive and the two sides stay structurally self-describing.
- C#: `MessagePack` (MessagePack-CSharp) with a custom `MongoId` formatter and extension-data
  handling equivalent to the JSON path. One new NuGet dependency; contract config lives next to
  the ragfair native code, not on the model types (Models attributes stay STJ-only).
- The **request leg stays JSON** — it is ~4% of the full pass; its real cost is the regeneration
  pass, which is §7's follow-up plan, and keeping one leg JSON preserves a debuggable path for
  request dumps (`todo/ragfair-request.json` workflow).

Projected full pass after A+B+C: ~500–600 ms ≈ parity (DIAG_1 projection for 4a + b3).

## 5. Unchanged surfaces

- Frozen-class Harmony hook-liveness dispatch and `RagfairConfig.ForceLegacyRagfairGeneration`.
- The legacy path, verbatim.
- `rejectedCanSellTemplates` replay onto the live template table.
- The whole insert side: `AddOffer` loop, holder per-template cap, `OfferCounter` — all C#.
- One-timestamp-per-batch semantics.
- Request projection (`RagfairPayloadProjection.BuildRequest`) and request encoding.

## 6. Verification and benchmark protocol

Per stage, in order, all in Release:

1. `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
2. `dotnet test` — the full suite, not a filter. `RagfairParityTests` must stay 19/19 byte-equal;
   `SptNativeRagfairWireTests` is updated for the envelope in stages B and C;
   `RagfairPathDispatchTests`/`RagfairHookLivenessTests`/`ModCompatibilityTests` unchanged and green.
3. `dotnet test -c Release --filter "FullyQualifiedName~RagfairBenchmarkTests" --logger
   "console;verbosity=detailed"` — record native and legacy medians from the **same run**, full
   pass and regeneration pass.

Docs updated at the end regardless of outcome (DIAG_2 §7):

- `BENCHMARK.md`: new figures; the full pass produces ~58k offers / ~41 MB on the wire (not
  ~24k — that is the post-cap accepted count); the fixture runs workstation GC while the shipped
  server sets `ServerGarbageCollection=true`.
- `RUST-ROADMAP.md`: item 4 closed with results; the items-view-cache statement gets the
  regeneration-pass exception (worth ~1% of a full pass but ~60% of a regeneration pass); the
  latent `get_flea_prices_as_array` hazard (dead at `barter.chancePercent: 0`, O(offers ×
  price-table) if a mod raises it) joins the known-divergences list.

## 7. Out of scope, recorded as follow-up

The regeneration pass spends ~60% building and serializing the 5.8 MB call-invariant request
slice. The sanctioned fix — a database mutation stamp plus a cached serialized request slice —
reverses the roadmap's item-3 retraction *only if* the stamp exists first, which is database-layer
surgery touching every mod-facing mutation path. This plan does not build it. A new roadmap entry
records the finding and the stamp precondition so it isn't lost.

Also out of scope unless §1's fallback triggers: the per-offer deep-clone elimination
(DIAG_2 §4.3, 244 ms sequential, materially less once stage A lands).

## 8. Execution notes

- Implementation runs via Opus 5 subagents.
- Do not use per-offer `Instant::now()` timers on this host (~2 µs per call, no vDSO fast path);
  attribute generation-half costs by ablation (DIAG_2 §2).
- The diagnostic fixtures in `todo/ragfair-diagnostics/` (`RagfairWrapperBreakdownTests.cs` et
  al.) are available for stage-level attribution; they need `<AllowUnsafeBlocks>true</...>` in
  `Testing/UnitTests/UnitTests.csproj` and must not be committed enabled.
