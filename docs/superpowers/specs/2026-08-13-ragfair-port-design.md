# Ragfair offer + price generation Rust port — design

**Date:** 2026-08-13
**Status:** approved in chat; this document is the binding spec
**Predecessors:** the loot-family and bot-family ports (`2026-08-12-loot-generator-port`,
`2026-08-13-bot-family-port`). Their playbook rules (RUST-ROADMAP.md § Guidelines) bind this
port too: frozen 4.1.2 surface, verbatim legacy path, Harmony-detection dispatch, project per
call never cache, seeded-RNG primitive parity, lockstep FFI envelopes, `[Injectable]` entry
point, full gate loop.

## 1. Boundary — one native call per batch pass

The FFI crossing is **exactly one method**: `RagfairOfferGenerator.GenerateDynamicOffers`
(`Generators/Ragfair/RagfairOfferGenerator.cs:293`), as a new export
`spt_generate_dynamic_offers`. It has only two callers — `RagfairServer.Load()`
(`Servers/RagfairServer.cs:29`, startup full pass) and
`RagfairServer.ProcessExpiredFleaOffers()` (`:79`, regeneration once ≥1,400 offers expire) —
and carries all the volume: ~4.6k assort templates × 7–30 offers each, tens of thousands of
offers per full pass, each with a deep clone, re-id, condition/durability randomisation and a
full price computation.

Ports as private Rust (reachable only from the dispatcher): `CreateOffersFromAssort` (:332),
`CreateSingleOfferForItem` (:427), `RemoveBannedPlatesFromPreset` (:381), `RemoveArmorPlates`
(:508), `RandomiseOfferItemUpdProperties` (:641), `RandomiseItemCondition` (:694),
`RandomiseWeaponDurability` (:783), `RandomiseArmorDurabilityValues` (:804),
`AddMissingConditions` (:835), `CreateBarterBarterScheme` (:885), `CreateCurrencyBarterScheme`
(:963), `GetFleaPricesAsArray` (:933), plus the assort walk from
`RagfairAssortGenerator.GenerateRagfairAssortItems` (`Generators/Ragfair/RagfairAssortGenerator.cs:45`)
and the pricing math from `RagfairPriceService` (`GetFleaPriceForItem` :171,
`GetDynamicOfferPriceForOffer` :258, `GetDynamicItemPrice` :295 and its callees including
`RandomiseOfferPrice` :464) and the folded collaborator pieces of `RagfairServerHelper`
(`CalculateDynamicStackCount`, `GetOfferCountByBaseType`, `GetDynamicOfferCurrency`,
`IsItemValidRagfairItem`).

**Stays C#, untouched:**

- The insert loop. Rust returns finished offers; C# loops `ragfairOfferService.AddOffer(offer)`.
  The holder's live per-template cap — which re-draws `GetOfferCountByBaseType` against live
  `_fakePlayerOffers` counts and silently rejects (`Utils/RagfairOfferHolder.cs:153-163`) —
  keeps running in C# with identical semantics on both paths.
- The player-offer path: `RagfairController.CreatePlayerOffer` →
  `CreateAndAddFleaOffer`/`CreateOffer` (:66/:85). Per-HTTP-request, one offer, coupled to
  `ProfileHelper`/`SaveServer`. `CreateAndAddFleaOffer` is a three-way junction (player, trader
  batch, dynamic batch); on the native path the dynamic batch no longer routes through it.
- `GenerateFleaOffersForTrader` (:534): trivial volume (one trader's hideout-root assort rows),
  mutates the live trader base (`RefreshTraderRagfairOffers = false`, :629).
- All of `RagfairPriceService` as a C# class — `Load`/`RefreshStaticPrices`/
  `ReplaceFleaBasePrices` (startup, ~4.6k handbook lookups, mutates `templateTable.Prices` in
  place at :98) and its eight external callers (`InsuranceController`, `PMCLootGenerator`,
  `ScavCaseRewardGenerator`, `TraderController`, `RagfairTaxService`, two HTTP price
  endpoints). The Rust module reimplements the *math*; the C# service keeps serving everyone
  else.

## 2. Dispatch and override contract

Same shape as the bot port (`BotInventoryGenerator.cs` dispatcher precedent), fail-closed:

1. `RagfairConfig.ForceLegacyRagfairGeneration` — new `public bool { get; set; }`, default
   `false` (native default, consistent with loot and bots).
2. `Harmony.GetPatchInfo` over the hookable set: every public/protected/protected-internal
   declared member of `RagfairOfferGenerator` **except** `GenerateDynamicOffers` itself (it is
   the dispatcher; everything else is never called natively, so a patch on one must flip to
   legacy or it would silently do nothing). Statics included, property accessors excluded
   (`!IsSpecialName`). Includes the dead-but-frozen `GetRating` (:244) and `GetAvatarUrl`
   (:213).
3. `GetType()` checks on the three folded-in injected collaborators: `RagfairPriceService`,
   `RagfairServerHelper`, `RagfairAssortGenerator`. A TypePriority subclass of any of them
   flips to legacy.
4. Harmony detection also covers the public/protected members of those three collaborator
   classes (their logic executes inside Rust on the native path). Patches on deeper shared
   helpers (`RandomUtil`, `ItemHelper`, `HandbookHelper`, `PresetHelper`, `PaymentHelper`,
   `BotHelper`, `WeightedRandomHelper`, `ICloner`) are NOT detected — documented limitation,
   same list mechanism as the bot port.

The legacy path is the verbatim 4.1.2 body (`GenerateDynamicOffersLegacy` rename, 0-deletion
diff proof as before). Frozen public+protected surface enforced by `dotnet apicompat` in
`mpex-api-compat`. The 23-parameter constructor does not change; any service the projection
layer needs that isn't a ctor param is reached via `internal` accessors on injected
collaborators (bot-port precedent) — expected: none, the ctor already has everything.

## 3. Payload

### Input (projected per call, playbook rule 3)

- Items view: reuse `Native/Loot/PayloadProjection.cs BuildItemsView` output shape
  (`ItemView`/`ItemPropsView`).
- Preset maps: `item_presets` / `default_presets_by_tpl` / `presets_by_id` per the bot
  envelope (`bot/models.rs:628-631`).
- Price maps: **new** whole-table projections — `templateTable.Prices` (flea base prices) and
  handbook prices (whole table, not the bot port's pool-scoped `handbook_prices`).
- Config: the full `ragfairConfig.Dynamic` block plus the used top-level fields, and the
  `traderConfig`/globals values the pricing math reads (trader-lowest-loyalty sell-price
  coefficients via `TraderHelper.GetHighestSellToTraderPrice` inputs — project the resolved
  per-template highest-trader-price map rather than the trader tables, since building it is a
  cache-backed C# loop).
- `expiredOffers`: null for a full pass, or the cloned expired-offer item lists for a
  regeneration pass (`RagfairServer.cs:71` shape).
- Timestamp (`TimeUtil.GetTimeStamp()`), so offer end-times are computed in Rust with no clock
  skew across the boundary.
- Test seed (nullable), same seam as `NativeTestSeed` in the bot port.

### Output

- The finished offer list: full `RagfairOffer` wire objects — user block (nickname, rating,
  member type, avatar, generated account id), item trees with re-generated MongoIds, barter
  scheme (`requirements`), `itemsCost`/`requirementsCost`/`summaryCost`, stack/pack counts,
  `startTime`/`endTime`, `loyaltyLevel`, `sellInOnePiece`.
- `rejectedCanSellTemplates`: template ids for which `IsItemValidRagfairItem` decided
  `CanSellOnRagfair = false` (`Helpers/Ragfair/RagfairServerHelper.cs:61` writes the live db
  template today). C# replays these onto the live `templateTable` after the call — the only
  replay this port needs. (`templateTable.Prices` mutation lives in startup code that stays
  C#; no replay.)
- `diagnostics`: log-line replay envelope, existing mechanism.

Envelope types are internal, shipped in lockstep; `ABI_VERSION` bumps 5→6 together with
`SptNative.ExpectedAbiVersion`.

## 4. Rust side

New module `rust/spt-native/src/ragfair/`: `mod.rs` (RagfairContext), `models.rs` (request/
response wire types with `#[serde(flatten)] extra` passthrough), `offer_generator.rs`,
`price_service.rs` (pure pricing math), `assort_generator.rs`, `server_helper.rs` (stack
count, offer count, currency draw, validity check). Reuses `loot::item_helper`,
`loot::random_util` (+`TestSeedGuard`), `loot::mongo_id` (byte-identical ObjectId for
`new MongoId()` at :120/:365), `loot::probability_object_array`.

New `random_util` primitives, each with twin C#/Rust known-answer tests:

- `get_biased_random_number` (`RandomUtil.GetBiasedRandomNumber` — the only primitive on the
  price path, `RagfairPriceService.cs:464`)
- `get_bool` (:166)
- `generate_account_id` (`HashUtil.GenerateAccountId`, :168)
- `get_item_quality_modifier` if the ported pricing math reaches it (verify during
  implementation; add only if reached).

Legacy's `Task.Factory.StartNew` fan-out (:311) is NOT reproduced — Rust runs the assort walk
sequentially (or with rayon later if the benchmark demands it; sequential first, simplest
correct thing). This is sanctioned divergence: production RNG is crypto-random and the legacy
order is nondeterministic anyway (§5).

Bug-for-bug rules carry over: preserve dead branches, intentional quirks (e.g.
`RandomiseItemCondition` :699 reading `Max.Min` twice), draw order within a single item's
offer creation, IndexMap insertion order for every map that feeds a draw.

## 5. Parity — what is and is not promised

**Legacy `GenerateDynamicOffers` is nondeterministic even under a fixed seed**: it fans out
per-assort-item tasks over a shared RNG (`:311`, `Task.WaitAll` :318), so draw interleaving
varies run to run. Whole-pass byte-parity against the legacy oracle is therefore impossible
without modifying the frozen legacy body — not done. What this port promises instead:

1. **Per-item byte-parity.** With the pass restricted to a single item, legacy runs one task
   and its seeded draw sequence is deterministic. The vehicle is the *expired-offers* entry
   point — `GenerateDynamicOffers(expiredOffers)` takes item lists directly and bypasses the
   assort walk, and there is no side-effect-free way to restrict the assort itself (the only
   per-template lever, `dynamic.blacklist.custom`, mutates `CanSellOnRagfair` on the live db).
   The branches that mode skips (validity check, banned-plate removal, `RemoveArmorPlates`,
   the offer-count draw) are covered by Rust module tests, the replay test and the whole-pass
   structural checks instead. Fixtures pin native == legacy byte-equality per item class:
   weapon default preset, armor with removable plates, ammo, plain barter-eligible item,
   pack-eligible item, money. Same `LootIdNormalizer` mechanism as the bot parity tests.
2. **Primitive parity.** Twin KATs for every RNG primitive, existing and new.
3. **Whole-pass structural checks.** Offer counts within configured bounds per template,
   all MongoIds valid and internally consistent (parent/child links), barter schemes
   non-empty and well-formed, prices > 0, end times in range.

Distributional equivalence (same population shape under crypto RNG) is the production-level
claim; per-item byte-parity is what the test suite pins.

## 6. Testing

- `RagfairParityTests.cs`: the per-item fixtures from §5.1, seeded via the test seam, both
  paths, byte-equal after id normalization. Non-vacuity check: seed+1 perturbation must fail.
- KAT pairs in `Testing/UnitTests` + `rust/spt-native` for the new primitives.
- `RagfairPathDispatchTests.cs`: force-flag, Harmony patch on a frozen member (and on a
  folded collaborator member), TypePriority subclass of each of the three collaborators, mag-
  style negative control (unpatched ⇒ native).
- `RagfairHookLivenessTests.cs`: assert the hookable-member reflection sets stay non-empty and
  exclude only the dispatcher, so a rename regression fails loudly.
- Replay test: native pass on a fixture whose assort contains an invalid-for-flea template ⇒
  live `templateTable` template has `CanSellOnRagfair == false` afterwards.
- Wire round-trip tests (`SptNativeRagfairWireTests.cs`): enum-dict key re-keying (the
  EftEnumConverter numeric-key pitfall from the bot port), `extra` passthrough.
- Full suite + apicompat + cargo trio + csharpier, per the gate loop.

## 7. Performance stance

Amortisation favors this port: the call is rare (startup + per ~1,400 expirations, against an
8-second update throttle) and the batch is huge, unlike the per-bot 45–65× regression. The
unknown is response volume — tens of thousands of offers with full item trees is more
serialisation than any existing export. **A benchmark is an early plan task, not an
afterthought**: measure native vs legacy wall time for a full pass and for a regeneration
pass as soon as the happy path works. If serialisation drowns the win, stop and reassess
before porting the long tail (the shared items-view cache, roadmap #3, is the sanctioned
lever — not snapshots).

## 8. Mod-facing limitations (to document in ARCHITECTURE.md + RUST-ROADMAP.md)

- Patches on deep shared helpers (list in §2.4) do not reach the native path and do not flip
  to legacy — `ForceLegacyRagfairGeneration` is the escape hatch.
- Native offers are generated in one batch before insertion; legacy interleaves generation
  with holder insertion. Under crypto RNG this is distribution-identical; under a fixed seed
  the sequences differ (already nondeterministic in legacy).
- Runtime mutations mods make between passes (config, price table, blacklists) stay visible —
  projection is per call.
- The `AllowedFleaPriceItemsForBarter` per-instance cache quirk (never invalidated in legacy,
  :56/:933) is reproduced per-call in Rust, which makes the native path *fresher* than legacy
  for runtime-added items; divergence documented, not "fixed" — the legacy body stays
  verbatim.

## 9. Gate

`dotnet build -c Release` → `mpex-api-compat/ci/check-api-compat.sh` (six assemblies, zero
breaking changes; the only public-surface additions are `ForceLegacyRagfairGeneration` and
any new wire-view records) → `dotnet test` → `cargo test && cargo fmt --check && cargo clippy
--all-targets -- -D warnings` → `csharpier format .` → `graphify update .`.

## 10. Implementation phasing (for the plan)

1. Wire models + projections (items view reuse, new price-map projections, envelope).
2. New RNG primitives with twin KATs.
3. Rust pricing math (`price_service.rs`) — pure, testable first.
4. Assort walk + server-helper pieces.
5. Offer generation core (single item end-to-end), per-item parity fixture running early.
6. Barter/pack/currency schemes, condition randomisation, plates.
7. FFI export + ABI bump + C# dispatcher + replay + dispatch conditions.
8. Dispatch/hook-liveness/wire/replay tests.
9. Benchmark (early gate per §7 — runs as soon as 7 lands).
10. Docs: ARCHITECTURE.md section, RUST-ROADMAP.md status, BENCHMARK.md numbers.
