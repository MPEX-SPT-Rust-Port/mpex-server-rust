# Ragfair Offer + Price Generation Native Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `RagfairOfferGenerator.GenerateDynamicOffers` — the whole dynamic flea batch pass, including the assort walk, the pricing math and the folded `RagfairServerHelper` pieces — to `rust/spt-native` behind one `spt_generate_dynamic_offers` call per pass, dual-dispatched with the verbatim 4.1.2 C# retained as the executable oracle.

**Architecture:** One FFI cut at `Generators/Ragfair/RagfairOfferGenerator.cs:293`, whose only two callers are `RagfairServer.Load()` (`Servers/RagfairServer.cs:29`) and `RagfairServer.ProcessExpiredFleaOffers()` (`:79`). Rust returns finished `RagfairOffer` wire objects plus the template ids whose `CanSellOnRagfair` flag the validity check flipped; C# replays those onto the live `templateTable`, advances `OfferCounter`, and loops `ragfairOfferService.AddOffer` exactly as legacy does — the holder's live per-template cap (`Utils/RagfairOfferHolder.cs:153-163`) keeps running in C# with identical semantics on both paths. Frozen 4.1.2 surface; fail-closed dispatch (force flag → Harmony patches on this class → collaborator `GetType()` checks → Harmony patches on the collaborators); per-call projection, no snapshots; bug-for-bug parity down to the `RandomiseItemCondition:699` `Max.Min` double-read.

**Tech Stack:** C# (.NET 10) + Rust (cdylib, serde/IndexMap/rand_xoshiro), NUnit, cargo test.

**Spec:** `docs/superpowers/specs/2026-08-13-ragfair-port-design.md` — read it first; every task below argues from it. The three prior ports are the pattern library: `Generators/Bot/BotInventoryGenerator.cs` (dispatch/hookable set/replays), `Libraries/SPTarkov.Server.Core/Native/Bot/BotPayloads.cs` + `BotPayloadProjection.cs` (envelope + projection style), `rust/spt-native/src/bot/` (module style), `Testing/UnitTests/Tests/Generators/BotParityTests.cs` / `BotPathDispatchTests.cs` / `BotHookLivenessTests.cs` / `SptNativeBotWireTests.cs` (test style), `docs/superpowers/plans/2026-08-13-bot-family-port.md` (this plan's ancestor).

## Global Constraints

- **Frozen 4.1.2 surface** (RUST-ROADMAP.md § Guidelines rule 1): no signature, parameter-name or visibility change to any existing public/protected member of `RagfairOfferGenerator`, `RagfairPriceService`, `RagfairAssortGenerator`, `RagfairServerHelper` or their DTOs. The 23-parameter `RagfairOfferGenerator` constructor does not change. Enforced by `dotnet apicompat` in the sibling `mpex-api-compat` repo. **The only permitted public-surface additions are `RagfairConfig.ForceLegacyRagfairGeneration` and any new wire-view records** (which are `internal`, so they do not even show up).
- **Verbatim legacy path**: the 4.1.2 `GenerateDynamicOffers` body moves to `private void GenerateDynamicOffersLegacy(...)` with **zero interior edits** — the rename diff must show 0 deleted lines other than the signature line. Never delete it.
- **Anything the projection needs that is not a constructor parameter is reached via an `internal` accessor on an already-injected collaborator** (bot-port precedent: `botLootGenerator.BotLootCacheService`). One is needed: `BotHelper.BotTable` (Task 10).
- **Project per call, never cache** (rule 3): the payload is rebuilt from the live database, config and services on every call. Accept the cost.
- **RNG parity** (rule 4): every Rust draw goes through `loot::random_util` so `TestSeedGuard` covers it; both sides draw through the shared seeded xoshiro256\*\* seams (`Utils/RandomSource.cs` / `rust/spt-native/src/loot/random_util.rs`), pinned by twin known-answer tests. Production C# randomness stays bit-for-bit unchanged. Never draw eagerly where C# short-circuits, and vice versa.
- **C# nomenclature in Rust casing** (CLAUDE.md § Style): `CreateSingleOfferForItem` → `create_single_offer_for_item`; wire names pinned with serde renames; `IndexMap` everywhere a C# `Dictionary` feeds enumeration or a draw.
- **Bug-for-bug** (rule 3): the quirk tables in each task are requirements, not bugs to fix. Dead branches stay dead, intentional NREs become `LootError`s carrying an equivalent message, draw *order* and draw *count* within a single item's offer creation must match legacy exactly under a shared seed.
- **FFI envelopes are internal** (rule 5), shipped in lockstep: `ABI_VERSION` 5→6 exactly once (Task 9), `SptNative.ExpectedAbiVersion` 5→6 in the same commit.
- **C# style** (CLAUDE.md): always brace single-line bodies; no expression-bodied members (lambdas fine); file-scoped namespaces; `_camelCase` private fields; language keywords over BCL types.
- **Gate loop per task**: Rust tasks end `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`; C# tasks end `dotnet build server-csharp.slnx && dotnet test` (+ `csharpier format .` before commit). Full gate in Task 15: `dotnet build -c Release` → `mpex-api-compat/ci/check-api-compat.sh` → `dotnet test` → cargo trio → `csharpier format .` → `graphify update .`.
- **Commits**: one per task, message given in the task. `git add` only the task's files.

## File Structure

```
rust/spt-native/src/
  lib.rs                            # modify: `pub mod ragfair;`, ABI 5→6 (Task 9 only)
  ffi.rs                            # modify: 1 export + ABI pin test (Task 9)
  loot/random_util.rs               # modify (Task 2): get_biased_random_number, get_bool, generate_account_id
  loot/item_helper.rs               # modify (Task 2): get_item_quality_modifier, is_valid_item,
                                    #   armor_item_has_removable_plate_slots, get_removable_plate_slot_ids
  loot/models.rs                    # modify (Task 1): 4 new ItemView fields
  ragfair/
    mod.rs                          # new (Task 1): pub mod decls + RagfairContext
    models.rs                       # new (Task 1): wire envelopes, grown by later tasks
    price_service.rs                # new (Task 3): pure pricing math
    server_helper.rs                # new (Task 4): stack count, offer count, currency, validity
    assort_generator.rs             # new (Task 5): the assort walk
    offer_generator.rs              # new (Tasks 6-8): condition/plates, schemes, orchestrator
Libraries/SPTarkov.Server.Core/
  Native/Loot/PayloadProjection.cs  # modify (Task 1): 4 new ItemView projections
  Native/Ragfair/RagfairPayloads.cs # new (Task 10): C# envelopes
  Native/Ragfair/RagfairPayloadProjection.cs # new (Task 10): projection
  Native/NativeMethods.cs           # modify (Task 9): 1 LibraryImport
  Native/SptNative.cs               # modify (Task 9): wrapper + enum arm + ABI 6
  Helpers/Bot/BotHelper.cs          # modify (Task 10): 1 internal accessor (BotTable)
  Models/Spt/Config/RagfairConfig.cs# modify (Task 11): ForceLegacyRagfairGeneration
  Services/Ragfair/RagfairPriceService.cs     # modify (Task 11): 1 internal accessor (TraderHelper)
  Generators/Ragfair/RagfairAssortGenerator.cs# modify (Task 11): 2 internal accessors
                                    #   (ItemFilterService, SeasonalEventService)
  Generators/Ragfair/RagfairOfferGenerator.cs # modify (Task 11): dispatch ONLY
Testing/UnitTests/Tests/
  Utils/RandomSourceParityTests.cs  # modify (Task 2): twin KATs
  Generators/SptNativeRagfairWireTests.cs # new (Task 10)
  Generators/RagfairBenchmarkTests.cs     # new (Task 12)
  Generators/RagfairParityTests.cs        # new (Task 13)
  Generators/RagfairPathDispatchTests.cs  # new (Task 14)
  Generators/RagfairHookLivenessTests.cs  # new (Task 14)
```

C# reference sources (read-only oracles — never edited except where listed above):
`Generators/Ragfair/RagfairOfferGenerator.cs`, `Generators/Ragfair/RagfairAssortGenerator.cs`,
`Services/Ragfair/RagfairPriceService.cs`, `Helpers/Ragfair/RagfairServerHelper.cs`,
`Helpers/Items/ItemHelper.cs`, `Helpers/Profile/HandbookHelper.cs`,
`Helpers/Traders/TraderHelper.cs`, `Helpers/Items/PresetHelper.cs`, `Helpers/Bot/BotHelper.cs`,
`Utils/RandomUtil.cs`, `Utils/HashUtil.cs`, `Utils/RagfairOfferHolder.cs`, `Servers/RagfairServer.cs`.

---

### Task 1: `ragfair/` module scaffold, wire models, `ItemView` growth

**Files:**
- Create: `rust/spt-native/src/ragfair/mod.rs`, `rust/spt-native/src/ragfair/models.rs`
- Modify: `rust/spt-native/src/lib.rs` (add `pub mod ragfair;` — do **NOT** touch `ABI_VERSION`), `rust/spt-native/src/loot/models.rs` (4 new `ItemView` fields), `Libraries/SPTarkov.Server.Core/Native/Loot/LootPayloads.cs` + `Native/Loot/PayloadProjection.cs` (declare and project those 4)
- Test: inline `#[cfg(test)]` at the bottom of `ragfair/models.rs`

**Interfaces:**
- Produces: `GenerateDynamicOffersRequest`, `DynamicOffersResult`, `RagfairOfferWire`, `RagfairOfferUserWire`, `OfferRequirementWire`, `DynamicConfigWire` and the nested config views below. Later Rust tasks consume these; Task 10 mirrors them in C#.
- Consumes: `crate::loot::models::{ItemView, Item, Diagnostic, PresetView}` — reuse via `use`, do not duplicate. **There is no `ItemsView` type in this crate**: `IndexMap<String, ItemView>` *is* the view, which is what `loot::item_helper`'s helpers take. **`PresetView` (`loot/models.rs:656`) already carries exactly `items`/`id`/`name`/`encyclopedia`** — use it for all four preset maps rather than declaring a ragfair copy. Its `encyclopedia` is the root tpl `PresetHelper.IsPresetBaseClass` (`PresetHelper.cs:147`) baseclass-checks, which the pricing path leans on heavily.

- [ ] **Step 1: Grow `ItemView` first.** The ragfair pricing and condition paths read five `TemplateItem.Properties` members the existing view does not carry. Add to `rust/spt-native/src/loot/models.rs`'s `ItemView` struct (all `Option`, so existing loot/bot payloads still deserialize), with a `// -- Added for the ragfair port` banner comment matching the bot banner already there:

```rust
    // -- Added for the ragfair port (`ragfair::offer_generator`, `ragfair::server_helper`).
    /// `TemplateItem.Properties.Durability` — `AddMissingConditions` (`RagfairOfferGenerator.cs:838`)
    /// decides "is repairable" on its presence and "> 0" on its value.
    pub durability: Option<f64>,
    /// `TemplateItem.Properties.MaximumNumberOfUsage` — key uses (`:740/:743`).
    pub maximum_number_of_usage: Option<i32>,
    /// `TemplateItem.Properties.MaxRepairResource` — repair kit uses (`:759/:842`).
    pub max_repair_resource: Option<f64>,
    /// `TemplateItem.Properties.CanSellOnRagfair` — the BSG flea blacklist
    /// (`RagfairServerHelper.cs:53`). The custom-blacklist arm at `:61` writes it back to `false`;
    /// that write leaves Rust as `rejectedCanSellTemplates` for the C# side to replay.
    pub can_sell_on_ragfair: Option<bool>,
```

  Before writing them, read `Extensions/TemplateItemExtensions.cs`'s `IsQuestItem` (the quest-item arm of `IsItemValidRagfairItem`, `RagfairServerHelper.cs:73`): if it reads anything beyond `Properties.QuestItem`, which the view already carries, add that member here too and to the projection below. `StackMaxSize`, `MaxHpResource`, `MaxResource`, `FoodUseTime`, `MaxDurability`, `ArmorClass`, `Name`, `Type`, `Parent` and `QuestItem` are already on the view — do not re-add them.

  Then project them in `PayloadProjection.BuildItemsView` (`Native/Loot/PayloadProjection.cs:40-119`), inside the existing object initializer, in the same style as the neighbours:

```csharp
                Durability = props.Durability,
                MaximumNumberOfUsage = props.MaximumNumberOfUsage,
                MaxRepairResource = props.MaxRepairResource,
                CanSellOnRagfair = props.CanSellOnRagfair,
```

  and add the matching `public double? Durability { get; set; }` etc. to the C# `ItemView` record in `Native/Loot/LootPayloads.cs` with the right `JsonPropertyName` (camelCase of the member name — `durability`, `maximumNumberOfUsage`, `maxRepairResource`, `canSellOnRagfair`). Check each C# property's declared type in `Models/Eft/Common/Tables/TemplateItem.cs` and mirror it exactly (`int?` vs `double?` matters — a `double` field fed an integer JSON token is fine, the reverse is not).

- [ ] **Step 2: Write failing round-trip tests** in `ragfair/models.rs`, same style as `loot/models.rs`'s tests: deserialize `GenerateDynamicOffersRequest` from a minimal literal JSON string covering every top-level field; serialize a `DynamicOffersResult` and assert the exact wire names (`offers`, `rejectedCanSellTemplates`, `diagnostics`); assert `"testSeed": null` → `None` and an absent `expiredOffers` → `None`; assert an unknown key inside a nested game-data type lands in its `#[serde(flatten)] extra` map and round-trips.

- [ ] **Step 3: Run** — `cd rust && cargo test -p spt-native ragfair::models` → FAIL (module missing).

- [ ] **Step 4: Implement.** `mod.rs` declares `pub mod models;` only for now. `models.rs`, spec §3 verbatim:

```rust
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loot::models::{Diagnostic, Item, ItemView, PresetView};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateDynamicOffersRequest {
    /// Test-only: draws come from a seeded generator when set.
    pub test_seed: Option<u64>,
    /// `TimeUtil.GetTimeStamp()` taken once by the caller. Legacy re-reads the clock per offer
    /// (`RagfairOfferGenerator.cs:491`); one timestamp for the batch is a sanctioned divergence.
    pub timestamp: i64,
    /// The generator's `OfferCounter` (`:59`) before the pass; offers come back numbered from it.
    pub offer_counter_start: i32,
    /// `null` for a full pass; the cloned expired-offer item lists for a regeneration pass
    /// (`RagfairServer.cs:69-79`).
    pub expired_offers: Option<Vec<Vec<Item>>>,
    pub dynamic: DynamicConfigWire,
    /// `GlobalTable.ItemPresets` — `PresetHelper.IsPreset`/`GetPreset` read this map, and
    /// `GetAllPresets()` is its `Values` in insertion order.
    pub item_presets: IndexMap<String, PresetView>,
    /// `PresetHelper.GetDefaultPresets().Values.ToList()` — the assort walk's preset source when
    /// `showDefaultPresetsOnly` is set (`RagfairAssortGenerator.cs:115-117`).
    pub default_presets: Vec<PresetView>,
    /// `PresetHelper.GetDefaultPresetByTpl()` — `GetDefaultPreset(tpl)` for the weapon-preset price.
    pub default_presets_by_tpl: IndexMap<String, PresetView>,
    /// `PresetHelper.GetPresets(tpl)` resolved for every tpl that has presets — the fallback arm of
    /// `RagfairPriceService.GetWeaponPreset` (`:577`).
    pub presets_by_tpl: IndexMap<String, Vec<PresetView>>,
    /// `templateTable.Prices` — the whole flea base price table, insertion ordered: it is the
    /// source order of `GetFleaPricesAsArray` (`RagfairOfferGenerator.cs:938`), which feeds an
    /// index draw.
    pub flea_prices: IndexMap<String, f64>,
    /// `HandbookHelper.GetTemplatePrice` for the whole items table.
    pub handbook_prices: IndexMap<String, f64>,
    /// `TraderHelper.GetHighestSellToTraderPrice` resolved per template (a cache-backed C# loop, so
    /// it stays on the C# side and crosses as a map).
    pub highest_trader_prices: IndexMap<String, f64>,
    /// `ItemFilterService.GetBlacklistedItems()` — read by `ItemHelper.IsValidItem`.
    pub config_blacklist: Vec<String>,
    /// `SeasonalEventService.SeasonalEventEnabled()` (`RagfairAssortGenerator.cs:57`).
    pub seasonal_event_active: bool,
    /// `SeasonalEventService.GetInactiveSeasonalEventItems()` (`:58`).
    pub seasonal_item_tpl_blacklist: Vec<String>,
    /// `BotHelper.GatherPmcNamesOfLength` for each faction at `botConfig.BotNameLengthLimit`,
    /// pre-filtered. The faction is still drawn natively (`BotHelper.cs:151`, `GetInt(0, 1)`).
    pub pmc_names_usec: Vec<String>,
    pub pmc_names_bear: Vec<String>,
    pub items: IndexMap<String, ItemView>,
}

/// `Models/Spt/Config/RagfairConfig.cs:102-239` `Dynamic`, whole. Reuse the C# record's wire names.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicConfigWire {
    pub use_trader_price_for_offers_if_higher: bool,
    pub barter: BarterDetailsWire,
    pub pack: PackDetailsWire,
    pub offer_adjustment: OfferAdjustmentWire,
    /// Keys are a tpl **or** the literal `"default"` (`RagfairConfig.cs:139`).
    pub offer_item_count: IndexMap<String, MinMaxIntWire>,
    pub price_ranges: PriceRangesWire,
    pub show_default_presets_only: bool,
    pub ignore_quality_price_variance_blacklist: Vec<String>,
    pub end_time_seconds: MinMaxIntWire,
    /// Keyed by base-class tpl; **iteration order is the match order** in
    /// `GetDynamicConditionIdForTpl` (`RagfairOfferGenerator.cs:676-683`).
    pub condition: IndexMap<String, ConditionWire>,
    pub stackable_percent: MinMaxDoubleWire,
    pub non_stackable_count: MinMaxIntWire,
    pub rating: MinMaxDoubleWire,
    pub armor: ArmorSettingsWire,
    pub item_price_multiplier: Option<IndexMap<String, f64>>,
    #[serde(rename = "offerCurrencyChancePercent")]
    pub offer_currency_change_percent: IndexMap<String, f64>,
    pub show_as_single_stack: Vec<String>,
    pub remove_seasonal_items_when_not_in_event: bool,
    pub blacklist: RagfairBlacklistWire,
    pub unreasonable_mod_prices: IndexMap<String, UnreasonableModPricesWire>,
    pub generate_base_flea_prices: GenerateFleaPricesWire,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinMaxIntWire {
    pub min: i32,
    pub max: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinMaxDoubleWire {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicOffersResult {
    pub offers: Vec<RagfairOfferWire>,
    /// Template ids whose `CanSellOnRagfair` the custom-blacklist arm of `IsItemValidRagfairItem`
    /// (`RagfairServerHelper.cs:61`) set to `false`. The caller replays these onto the live
    /// `templateTable`; nothing else in this port mutates the database.
    pub rejected_can_sell_templates: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// `Models/Eft/Ragfair/RagfairOffer.cs:8-91`, only the members `CreateOffer` (`:118-138`) sets.
/// `sellResult`, `unlimitedCount`, `buyRestrictionMax`, `buyRestrictionCurrent` are never set on
/// this path and are omitted rather than sent as null.
#[derive(Debug, Serialize)]
pub struct RagfairOfferWire {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "intId")]
    pub internal_id: i32,
    pub user: RagfairOfferUserWire,
    pub root: String,
    pub items: Vec<Item>,
    #[serde(rename = "itemsCost")]
    pub items_cost: f64,
    pub requirements: Vec<OfferRequirementWire>,
    #[serde(rename = "requirementsCost")]
    pub requirements_cost: f64,
    #[serde(rename = "summaryCost")]
    pub summary_cost: f64,
    #[serde(rename = "startTime")]
    pub start_time: i64,
    #[serde(rename = "endTime")]
    pub end_time: i64,
    #[serde(rename = "loyaltyLevel")]
    pub loyalty_level: i32,
    #[serde(rename = "sellInOnePiece")]
    pub sell_in_one_piece: bool,
    pub locked: bool,
    pub quantity: i32,
}

/// `RagfairOffer.cs:111-140`. `memberType` is the numeric `MemberCategory` (`Default` = 0) —
/// `EftEnumConverter` writes enums as numbers, so this must stay an integer on the wire.
#[derive(Debug, Serialize)]
pub struct RagfairOfferUserWire {
    pub id: String,
    pub nickname: Option<String>,
    pub rating: f64,
    #[serde(rename = "memberType")]
    pub member_type: i32,
    pub avatar: Option<String>,
    #[serde(rename = "isRatingGrowing")]
    pub is_rating_growing: bool,
    pub aid: i32,
}

/// `RagfairOffer.cs:93-109`. `level`/`side` are only set for dogtag barters, which the dynamic
/// path never produces (`CreateOffer:97-101` reads `barter.Level`, always null here) — they are
/// `Option` so the wire stays faithful if that ever changes.
#[derive(Debug, Serialize)]
pub struct OfferRequirementWire {
    #[serde(rename = "_tpl")]
    pub template_id: String,
    pub count: f64,
    #[serde(rename = "onlyFunctional")]
    pub only_functional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<i32>,
}
```

  The remaining config wire types (`BarterDetailsWire`, `PackDetailsWire`, `OfferAdjustmentWire`, `PriceRangesWire`, `ConditionWire`, `ArmorSettingsWire`, `RagfairBlacklistWire`, `UnreasonableModPricesWire`, `GenerateFleaPricesWire`) mirror their C# records in `Models/Spt/Config/RagfairConfig.cs:280-518` member for member, camelCase, with `#[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>` on each — read that file and transcribe. `ConditionWire` carries `condition_chance: f64`, `current: MinMaxDoubleWire`, `max: MinMaxDoubleWire`.

- [ ] **Step 5: Run** — `cargo test -p spt-native ragfair::models` → PASS.
- [ ] **Step 6: Build the C# side too** — `dotnet build server-csharp.slnx` (the `ItemView` additions must compile) → PASS.
- [ ] **Step 7: Gates + commit** — `cd rust && cargo fmt --check && cargo clippy --all-targets -- -D warnings`; `csharpier format .`; then

```bash
git add rust/ Libraries/SPTarkov.Server.Core/Native/
git commit -m "feat: add ragfair wire models and the item view fields they need"
```

### Task 2: RNG + item-helper primitives with twin known-answer tests

**Files:**
- Modify: `rust/spt-native/src/loot/random_util.rs`, `rust/spt-native/src/loot/item_helper.rs`
- Modify: `Testing/UnitTests/Tests/Utils/RandomSourceParityTests.cs` (twin KATs)

**Interfaces:**
- Produces:
  - `pub fn get_biased_random_number(min: f64, max: f64, shift: f64, n: f64) -> f64`
  - `pub fn get_bool() -> bool`
  - `pub fn generate_account_id() -> i32`
  - `pub fn get_item_quality_modifier(items_view: &IndexMap<String, ItemView>, item: &Item, skip_armor_items_without_durability: bool) -> f64` (in `item_helper.rs`)
  - `pub fn is_valid_item(items_view: &IndexMap<String, ItemView>, blacklist: &HashSet<String>, handbook_prices: &IndexMap<String, f64>, flea_prices: &IndexMap<String, f64>, tpl: &str, invalid_base_types: &[&str]) -> bool` (in `item_helper.rs`)
  - `pub fn armor_item_has_removable_plate_slots(items_view: &IndexMap<String, ItemView>, tpl: &str) -> bool`, `pub fn get_removable_plate_slot_ids() -> &'static [&'static str]` (in `item_helper.rs`)
- Consumes: the existing `TestSeedGuard`, `next_double48`, `get_int`, `get_double` in `random_util.rs`.

**Why `get_item_quality_modifier` is in scope (spec §4 asked us to decide):** `RagfairPriceService.GetDynamicItemPrice:344-348` calls `itemHelper.GetItemQualityModifier(item)` whenever `item is not null`, and the only caller on this path — `GetDynamicOfferPriceForOffer:272` — always passes an item. So it is reached on **every** priced offer and must be ported. It is not an RNG primitive (`ItemHelper.cs:582-646` is pure), so its twin test is a value-parity test over hand-built `Upd` shapes, not a seeded KAT.

- [ ] **Step 1: Read the C# sources.** `Utils/RandomUtil.cs:361-432` — `GetBiasedRandomNumber` plus its two privates. The exact shape, to transcribe:
  - `max < min` → log error, return `-1`; `n < 1` → log error, return `-1`; `min == max` → return `min` (**no draw consumed** in any of these three).
  - `shift > max - min` → two warnings, then continues.
  - `biasedMin = shift >= 0 ? min - shift : min`; `biasedMax = shift < 0 ? max + shift : max`.
  - `do { num = round(biasedMin + gaussian(n) * (biasedMax - biasedMin + 1)) } while (num < min || num > max)` where `gaussian(n) = (sum of n GetSecureRandomNumber() draws) / n` — **`n` is a `double` used as a loop bound with `i += 1`, so `n = 2` is exactly 2 draws per attempt** and every rejected attempt consumes its draws.
  `Utils/RandomUtil.cs:87-90` — `GetBool` is `GetSecureRandomNumber() < 0.5`, one `next_double48` draw. `Utils/HashUtil.cs:118-124` — `GenerateAccountId` is `GetInt(1000000, 1999999)`, one draw through the existing `get_int`. `Helpers/Items/ItemHelper.cs:582-646` + `GetRepairableItemQualityValue` (`:655-...`, read to the end) — transcribe the whole `if/else if` chain **in order** including the `result == 0 → 0.01` floor and the `-1` armor early-out. `ItemHelper.cs:289-298` `IsValidItem` (the `TemplateItem` overload) — `!QuestItem && Type == "Item" (ordinal-ignore-case) && GetItemPrice(id) > 0 && !blacklisted && no invalid base type`, where `GetItemPrice` is `ItemHelper.cs:431-440`: handbook price if `>= 1`, else the flea price, else null. `ItemHelper.cs:354-362` `ArmorItemHasRemovablePlateSlots` and `:1679-1682` `GetRemovablePlateSlotIds` — the six-name set already hard-coded in `rust/spt-native/src/bot/bot_equipment_mod_generator.rs:2452` (`is_removable_plate_slot`); **move that function into `loot::item_helper` and have the bot module call it there** rather than declaring a second copy.

- [ ] **Step 2: Write failing Rust KATs** in `random_util.rs`'s existing `#[cfg(test)]` block, matching the established pattern (`let _g = TestSeedGuard::install(KAT_SEED);`, pin the printed values after the first run):

```rust
    #[test]
    fn get_biased_random_number_matches_the_csharp_kat() {
        let _g = TestSeedGuard::install(KAT_SEED);
        // RandomiseOfferPrice's exact arguments for the default price range (0.8..1.2 * 100).
        let values: Vec<f64> = (0..5)
            .map(|_| get_biased_random_number(80.0, 120.0, 2.0, 2.0))
            .collect();
        assert_eq!(values, vec![/* pin from the first run, must equal the C# twin */]);
    }

    #[test]
    fn get_biased_random_number_guard_arms_consume_no_draws() {
        let _g = TestSeedGuard::install(KAT_SEED);
        assert_eq!(get_biased_random_number(120.0, 80.0, 2.0, 2.0), -1.0);
        assert_eq!(get_biased_random_number(80.0, 120.0, 2.0, 0.5), -1.0);
        assert_eq!(get_biased_random_number(80.0, 80.0, 2.0, 2.0), 80.0);
        // The stream is untouched, so this is the same value the previous test's first draw was.
        assert_eq!(get_biased_random_number(80.0, 120.0, 2.0, 2.0), /* pinned */);
    }

    #[test]
    fn get_bool_matches_the_csharp_kat() {
        let _g = TestSeedGuard::install(KAT_SEED);
        let values: Vec<bool> = (0..8).map(|_| get_bool()).collect();
        assert_eq!(values, vec![/* pinned */]);
    }

    #[test]
    fn generate_account_id_matches_the_csharp_kat() {
        let _g = TestSeedGuard::install(KAT_SEED);
        let values: Vec<i32> = (0..4).map(|_| generate_account_id()).collect();
        assert_eq!(values, vec![/* pinned */]);
    }
```

  And a pure-value test for `get_item_quality_modifier` in `item_helper.rs`'s test block covering, against a small hand-built items view: a medkit (`Upd.MedKit.HpResource` half of `MaxHpResource` → `0.5`), a repairable weapon (durability/max), a food item, a key (**counts upwards**: `(max - used) / max`), a fuel item with `Upd.Resource.UnitsConsumed > 0`, a repair kit, an item with no `Upd` (→ `1.0`), an armor with `MaxDurability == 0` and `skip_armor_items_without_durability` true (→ `-1.0`), and a case whose ratio is `0` (→ `0.01`).

- [ ] **Step 3: Run** — `cargo test -p spt-native random_util item_helper` → FAIL.

- [ ] **Step 4: Implement + pin.** Run with `cargo test -p spt-native -- --nocapture`, pin the printed values into the `assert_eq!`s, re-run → PASS. Implementation note: `get_biased_random_number`'s three error/warning arms have no logger in Rust and no context to push a diagnostic onto (`random_util` is context-free). Return `-1.0` / `min` silently and record the dropped log lines in the function's doc-comment, naming their C# text (`"Invalid argument, Bounded random number generation max is smaller than min({max} < {min}"` — the missing closing paren is in the C#, quote it as-is). This is the same treatment the already-ported `get_chance_100` gives its clamp.

- [ ] **Step 5: Write the C# twin KATs** in `Testing/UnitTests/Tests/Utils/RandomSourceParityTests.cs`, same fixture style as the existing twins:

```csharp
    [Test]
    public void GetBiasedRandomNumberMatchesTheRustKat()
    {
        var randomUtil = BuildSeededRandomUtil(KatSeed);

        var values = Enumerable.Range(0, 5).Select(_ => randomUtil.GetBiasedRandomNumber(80d, 120d, 2d, 2d)).ToArray();

        Assert.That(values, Is.EqualTo(new[] { /* the same pinned values as the Rust test */ }));
    }

    [Test]
    public void GetBoolMatchesTheRustKat()
    {
        var randomUtil = BuildSeededRandomUtil(KatSeed);

        var values = Enumerable.Range(0, 8).Select(_ => randomUtil.GetBool()).ToArray();

        Assert.That(values, Is.EqualTo(new[] { /* the same pinned values */ }));
    }

    [Test]
    public void GenerateAccountIdMatchesTheRustKat()
    {
        var hashUtil = new HashUtil(BuildSeededRandomUtil(KatSeed));

        var values = Enumerable.Range(0, 4).Select(_ => hashUtil.GenerateAccountId()).ToArray();

        Assert.That(values, Is.EqualTo(new[] { /* the same pinned values */ }));
    }
```

  `BuildSeededRandomUtil`/`KatSeed` are whatever the existing fixture already uses to install a `SeededRandomSource` into a `RandomUtil` — read the file and reuse its helper verbatim rather than adding a second one. If `HashUtil` cannot be constructed with a hand-built `RandomUtil` in that fixture, resolve it from `DI.GetInstance()` and swap `RandomSource` on the shared `RandomUtil` in a `try/finally`, as the loot parity tests do.

- [ ] **Step 6: Run both** — `cargo test -p spt-native random_util item_helper` and `dotnet test --filter "FullyQualifiedName~RandomSourceParityTests"` → PASS. If the values disagree, the Rust side is wrong: the C# is the oracle.
- [ ] **Step 7: Gates + commit** — `feat: add biased-random, bool, account-id and quality-modifier primitives with twin KATs`

### Task 3: `price_service.rs` — the pricing math

**Files:**
- Create: `rust/spt-native/src/ragfair/price_service.rs` (+ `pub(crate) mod price_service;` in `ragfair/mod.rs`)
- Modify: `rust/spt-native/src/ragfair/mod.rs` (declare `RagfairContext`)

**Interfaces:**
- Produces, mirroring `Services/Ragfair/RagfairPriceService.cs` names:
  - `pub fn get_flea_price_for_item(ctx: &RagfairContext, tpl: &str) -> f64` (`:171`)
  - `pub fn get_static_price_for_item(ctx: &RagfairContext, tpl: &str) -> f64` (`:212`)
  - `pub fn get_dynamic_offer_price_for_offer(ctx: &RagfairContext, offer_items: &[Item], desired_currency: &str, is_pack_offer: bool) -> f64` (`:258`)
  - `pub fn get_dynamic_item_price(ctx: &RagfairContext, item_template_id: &str, desired_currency: &str, item: Option<&Item>, offer_items: Option<&[Item]>, is_pack_offer: Option<bool>) -> f64` (`:295`)
  - privates `adjust_unreasonable_price` (`:386`), `get_offer_type_range_values` (`:412`), `adjust_price_if_below_handbook` (`:435`), `randomise_offer_price` (`:461`), `get_weapon_preset_price` (`:477`), `get_preset_price_by_children` (`:527`), `get_highest_handbook_or_trader_price_as_rouble` (`:551`), `get_weapon_preset` (`:569`), `get_price_difference` (`:246`)
  - `pub struct RagfairContext<'a>` in `ragfair/mod.rs`, the analog of `bot::BotContext` (`rust/spt-native/src/bot/mod.rs:28-77`) — copy its shape exactly: every view borrowed for `'a` (`items: &'a IndexMap<String, ItemView>`, `dynamic: &'a DynamicConfigWire`, the four preset maps, the three price maps, the two blacklists, the two PMC name pools), the scalars owned (`timestamp: i64`, `seasonal_event_active: bool`), and a plain **`pub diagnostics: Vec<Diagnostic>`** field last — not a `RefCell`; the borrow discipline the `BotContext` doc-comment describes ("copying one view out releases the `&mut ctx` and leaves the diagnostics writable") is what makes that work, so follow it. Add the `#[cfg(test)]` empty stand-ins (`NO_PRESETS`, `NO_BLACKLIST`, …) the bot module declares beneath its context, for the fixtures in Tasks 4-8. Also declare the two `Diagnostic` constructors the bot modules each re-declare — `fn plain(level: &str, message: String) -> Diagnostic` and `fn localised(level: &str, locale_key: &str, args: serde_json::Value) -> Diagnostic` (bodies at `rust/spt-native/src/bot/bot_inventory_generator.rs:1020-1036`) — **once, in `ragfair/mod.rs`**, and `use` them from the four ragfair modules rather than copying them per file.
- Consumes: Task 1's wire types, Task 2's `get_biased_random_number` and `get_item_quality_modifier`, `loot::item_helper::{get_item, is_of_baseclass}`.

**C# mapping — `Services/Ragfair/RagfairPriceService.cs`:**

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `get_flea_price_for_item` | 171-193 | `flea_prices[tpl]` first, **then** the handbook price (`GetStaticPriceForItem` = `HandbookHelper.GetTemplatePrice`, i.e. `handbook_prices` with `0` for a miss); a `None` result logs the `ragfair-unable_to_find_item_price_for_item_in_flea_handbook` warning via `ctx.diagnostics.push(localised("warning", …, json!({ "tpl": …, "name": … })))`; `0` becomes `1` **after** the warning check, not before |
| `get_static_price_for_item` | 212-215 | never null in Rust — a handbook miss is `0.0` (`HandbookHelper.cs:106-131` caches and returns `0`) |
| `get_dynamic_offer_price_for_offer` | 258-285 | skips `BUILT_IN_INSERTS` baseclass items (`:267`) **before** pricing; `?? 0` on a null item price; the **preset break at `:275-281`** — once an item's `Upd.SptPresetId` is a weapon-baseclass preset, the loop stops, so a preset's mods are never priced individually; `Math.Round` on the total (banker's rounding — use the existing `round_half_even`) |
| `get_dynamic_item_price` | 295-377 | **draw-order contract, top to bottom:** flea price → `adjust_price_if_below_handbook` (gated on `offerAdjustment.adjustPriceWhenBelowHandbookPrice`) → trader price if higher (gated on `useTraderPriceForOffersIfHigher`, reads `highest_trader_prices`) → weapon-preset branch `:323-335` (`GeneratePresetPriceByChildren && UseHandbookPrice` picks `get_preset_price_by_children`, else `get_weapon_preset_price`; sets `is_preset`) → `itemPriceMultiplier` lookup → **quality modifier** (`item.is_some() && !ignoreQualityPriceVarianceBlacklist.contains(tpl)`) → the `unreasonableModPrices` loop **in map order** → `randomise_offer_price` (**the only draw**, always consumed) → currency conversion → `price <= 0 → 0.1` |
| `adjust_price_if_below_handbook` | 435-453 | `GetPriceDifference(handbook, price) = 100 * a / (a + b)` — note the **divide-by-zero when both are 0** produces `NaN` in C# doubles and `NaN > x` is false, so the branch is not taken; Rust `f64` does the same, do not "fix" it |
| `randomise_offer_price` | 461-468 | `get_biased_random_number(min * 100, max * 100, 2, 2)` then `price * (multiplier / 100)` |
| `get_weapon_preset_price` | 477-520 | `get_weapon_preset` → `IsDefault` short-circuits to `get_flea_price_for_item(root.template)`; the "mods not in the default preset" filter is by **template**, the "replaced mods" filter by **slot id**; `newOrReplacedModsInPresetVsDefault` is a lazy LINQ query enumerated **three times** (`:493`, `:500`, `:503`) — a materialized `Vec` is output-equivalent, keep the same filtering |
| `get_weapon_preset` | 569-589 | `default_presets_by_tpl` hit → `{ is_default: true }`; miss → `presets_by_tpl[tpl][0]` with the debug log; **`nonDefaultPresets[0]` on an empty/absent list is an unguarded C# NRE** → `LootError` naming it |
| `get_preset_price_by_children` | 527-544 | root (`ParentId` null or `"hideout"`, ordinal-ignore-case) uses the static price, everything else the flea price |
| `get_highest_handbook_or_trader_price_as_rouble` | 551-561 | `max(handbook, highest_trader_prices[tpl])` |
| currency conversion | `HandbookHelper.cs:215-225` `FromRoubles` | roubles → passthrough; else `price = handbook_prices[currency]`, result `price > 0 ? max(1, round(roubles / price)) : 0` |

`Load`/`RefreshStaticPrices`/`ReplaceFleaBasePrices`/`GetAllFleaPrices`/`GetAllStaticPrices`/`GetDynamicPriceForItem` are **not** ported — they are startup and external-caller surface that stays in C# (spec §1).

- [ ] **Step 1: Read `Services/Ragfair/RagfairPriceService.cs` in full**, plus `Helpers/Profile/HandbookHelper.cs:106-131` and `:202-225`. Write the RNG-call list for one `get_dynamic_item_price` run as a module doc-comment — it is exactly one draw, inside `randomise_offer_price`, always consumed. That single fact is this module's parity checklist.
- [ ] **Step 2: Failing tests.** A module fixture: a 6-template items view (a plain item, a weapon with a default preset, a weapon with only a non-default preset, an ammo box, a currency tpl, an item on the quality-variance blacklist), price maps with known values, and a `DynamicConfigWire` built from a JSON literal. Cases: (a) `get_flea_price_for_item` hits flea, falls back to handbook, floors `0` at `1`, and emits the warning diagnostic when neither has it; (b) `get_dynamic_item_price` with the price ranges pinned to `min == max` so `get_biased_random_number` returns without a draw, asserting each stage's arithmetic in isolation by toggling one config flag at a time; (c) seeded end-to-end price for the plain item, pinned; (d) the preset break: an offer of `[weapon-preset-root, mod, mod]` prices as one item, asserted by the total equalling the root-only price and by the **draw count** (one, pinned by reading the next draw); (e) the quality-blacklist tpl skips the quality modifier; (f) `get_weapon_preset` on a tpl with no presets at all → `LootError`.
- [ ] **Step 3: Implement.** **Step 4: Run** — `cargo test -p spt-native ragfair::price_service` → PASS.
- [ ] **Step 5: Gates + commit** — `feat: port ragfair dynamic pricing math to spt-native`

### Task 4: `server_helper.rs` — stack count, offer count, currency, validity

**Files:**
- Create: `rust/spt-native/src/ragfair/server_helper.rs` (+ `pub(crate) mod` line in `ragfair/mod.rs`)

**Interfaces:**
- Produces, mirroring `Helpers/Ragfair/RagfairServerHelper.cs`:
  - `pub fn calculate_dynamic_stack_count(ctx: &RagfairContext, tpl: &str, is_preset: bool) -> Result<i32, LootError>` (`:138`)
  - `pub fn get_offer_count_by_base_type(ctx: &RagfairContext, item_parent_type: &str) -> i32` (`:224`)
  - `pub fn get_dynamic_offer_currency(ctx: &RagfairContext) -> Result<String, LootError>` (`:175`)
  - `pub fn is_item_valid_ragfair_item(ctx: &RagfairContext, tpl: &str, rejected: &mut IndexSet<String>) -> bool` (`:37`)
- Consumes: Task 2's `is_valid_item`, Task 3's `RagfairContext`, `loot::random_util::{get_int, get_double, get_percent_of_value, get_weighted_value}`, `loot::item_helper::is_of_baseclasses`.

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `calculate_dynamic_stack_count` | 138-169 | tpl not in the items view → `LootError` with the `ragfair-item_not_in_db_unable_to_generate_dynamic_stack_count` text (**no draw**); `is_preset \|\| is_of_baseclasses(showAsSingleStack)` → `1` (**no draw**); `StackMaxSize ?? 1 == 1` → **one** `get_int(nonStackableCount.min, max)`; otherwise **two** draws — `get_double(stackablePercent.min, max)` then `get_percent_of_value(pct, maxStackSize, 0)`, result `max(.., 1)` after an `(int)` **truncating** cast |
| `get_offer_count_by_base_type` | 224-232 | `offerItemCount[parentTpl]`, else `offerItemCount["default"]`; **the `GetValueOrDefault` miss yields a null `MinMax` and an unguarded C# NRE** → `LootError` if `"default"` is absent; one `get_int(min, max)` |
| `get_dynamic_offer_currency` | 175-178 | one `get_weighted_value` over `offer_currency_change_percent` in **insertion order** |
| `is_item_valid_ragfair_item` | 37-89 | order is the contract: not in items view → false; `!is_valid_item(...)` (default invalid base types — read `ItemHelper._defaultInvalidBaseTypes`) → false; `blacklist.enableBsgList && !can_sell_on_ragfair.unwrap_or(false)` → false; **`blacklist.custom.contains(tpl)` → record the tpl in `rejected` and return false** (this is the only database write the port replays); `enableCustomItemCategoryList && customItemCategoryList.contains(parent)` → false; `enableQuestList && is_quest_item` → false; `damagedAmmoPacks && parent == AMMO_BOX && name.contains("_damaged")` → false; else true. **No draws anywhere in this function** |

- [ ] **Step 1: Read `Helpers/Ragfair/RagfairServerHelper.cs` in full** plus `ItemHelper._defaultInvalidBaseTypes` and `Extensions/TemplateItemExtensions.cs` `IsQuestItem`. Doc-comment the per-function draw counts (0, 1 or 2) — that table is what the tests pin.
- [ ] **Step 2: Failing seeded tests.** Fixture: a stackable item (`StackMaxSize` 60), a non-stackable item, a preset root, a `showAsSingleStack` tpl, a custom-blacklisted tpl, a quest item, a `_damaged` ammo box. Cases: (a) each `calculate_dynamic_stack_count` arm with its exact draw count pinned by reading the next draw afterwards; (b) `get_offer_count_by_base_type` hits the per-parent entry, falls back to `"default"`, and errors with neither; (c) `get_dynamic_offer_currency` pinned sequence over a 3-currency weight map; (d) every `is_item_valid_ragfair_item` rejection arm, with the custom-blacklist case asserting the tpl landed in `rejected` **and** that no other arm writes to it.
- [ ] **Step 3: Implement.** **Step 4: Run** — `cargo test -p spt-native ragfair::server_helper` → PASS.
- [ ] **Step 5: Gates + commit** — `feat: port ragfair server helper stack/offer/currency/validity to spt-native`

### Task 5: `assort_generator.rs` — the assort walk

**Files:**
- Create: `rust/spt-native/src/ragfair/assort_generator.rs` (+ `pub(crate) mod` line in `ragfair/mod.rs`)

**Interfaces:**
- Produces: `pub fn generate_ragfair_assort_items(ctx: &RagfairContext) -> Result<Vec<Vec<Item>>, LootError>` — the port of `RagfairAssortGenerator.GenerateRagfairAssortItems` (`Generators/Ragfair/RagfairAssortGenerator.cs:45-106`), plus privates `get_presets_to_add` (`:113`) and `create_ragfair_assort_root_item` (`:126`).
- Consumes: Task 2's `is_valid_item`, Task 3's `RagfairContext`, `loot::item_helper::{replace_ids, remap_root_item_id}`, `loot::mongo_id::new_mongo_id`.

| C# lines | Parity requirements |
|---|---|
| 45-59 | `dbItems` = `templateTable.Items` filtered to `Type != "Node"` (ordinal-ignore-case) and not in `config_blacklist`. **Iteration order is the items-view order**, which is the C# dictionary's insertion order carried across the wire by `IndexMap` |
| 60-80 | Presets first, items second — this is the batch order and therefore the draw order. Per preset: `replace_ids` on a clone of `preset.items`, then `remap_root_item_id`, then `processed_armor_items.insert(preset.items[0].template)`, then the root gets `parent_id = "hideout"`, `slot_id = "hideout"`, `Upd { stack_objects_count: 99999999, unlimited_count: true, spt_preset_id: preset.id }`. **`processedArmorItems` is keyed on the *original* preset's root tpl, read before the id remap** |
| 82-103 | Per item: `is_valid_item(item, RagfairItemInvalidBaseTypes)` (the seven-entry set at `:30-39` — transcribe it as a Rust `const`), then the seasonal skip (`removeSeasonalItemsWhenNotInEvent && !seasonal_event_active && seasonal_item_tpl_blacklist.contains(tpl)`), then the `processed_armor_items` skip, then one root item with **`id == tpl`** (`:101`, "tpl and id must be the same so hideout recipe rewards work") |
| 47/79/102 | `results = results.Union([...])` — `IEnumerable.Union` **de-duplicates by reference**, which for freshly-allocated lists never removes anything, and preserves first-seen order. A `Vec<Vec<Item>>` push is output-equivalent; note it in a comment so nobody "restores" the Union |
| — | **No RNG anywhere in this walk.** `create_ragfair_assort_root_item`'s `new MongoId()` branch (`:128-131`) is dead on this path: the only caller passes `tpl` as the id. Port the branch anyway (bug-for-bug), and note that it is dead |

- [ ] **Step 1: Read `Generators/Ragfair/RagfairAssortGenerator.cs` in full.** Doc-comment "no RNG in this module" so a later reader does not go looking for a draw.
- [ ] **Step 2: Failing tests.** Fixture: an items view of 6 templates (one `Node`, one blacklisted, one seasonal, one that is also a preset root, two plain) plus two presets. Cases: (a) the output is presets-then-items in that exact order; (b) the preset root carries the `hideout` parent/slot and the 99999999 unlimited `Upd` with `sptPresetId`; (c) the preset's root tpl is skipped in the item loop; (d) the seasonal skip fires only when all three conditions hold (four sub-cases); (e) a plain item's assort row has `id == template`; (f) child ids inside a cloned preset were replaced and re-parented (assert no id equals its source id, and every child's `parentId` resolves inside the list).
- [ ] **Step 3: Implement.** **Step 4: Run** — `cargo test -p spt-native ragfair::assort_generator` → PASS.
- [ ] **Step 5: Gates + commit** — `feat: port the ragfair assort walk to spt-native`

### Task 6: `offer_generator.rs` — condition randomisation and plates

**Files:**
- Create: `rust/spt-native/src/ragfair/offer_generator.rs` (+ `pub(crate) mod` line in `ragfair/mod.rs`)

**Interfaces:**
- Produces, mirroring `Generators/Ragfair/RagfairOfferGenerator.cs`:
  - `pub(crate) fn randomise_offer_item_upd_properties(ctx: &RagfairContext, item_with_mods: &mut Vec<Item>, item_details_tpl: &str, offer_creator: OfferCreator) -> Result<(), LootError>` (`:641`)
  - `fn get_dynamic_condition_id_for_tpl(ctx, tpl) -> Option<String>` (`:673`)
  - `fn randomise_item_condition(ctx, condition_settings_id: &str, item_with_mods: &mut Vec<Item>, item_details_tpl: &str) -> Result<(), LootError>` (`:694`)
  - `fn randomise_weapon_durability(ctx, item: &mut Item, item_db_tpl: &str, max_multiplier: f64, current_multiplier: f64)` (`:783`)
  - `fn randomise_armor_durability_values(ctx, armor_with_mods: &mut [Item], current_multiplier: f64, max_multiplier: f64)` (`:804`)
  - `fn add_missing_conditions(ctx, item: &mut Item) -> Result<(), LootError>` (`:835`)
  - `fn remove_banned_plates_from_preset(ctx, preset_with_children: &mut Vec<Item>, plate_settings: &ArmorPlateBlacklistSettingsWire) -> bool` (`:381`)
  - `fn remove_armor_plates(ctx, item_with_children: &mut Vec<Item>)` (`:508`)
  - `pub(crate) enum OfferCreator { Player, Trader, FakePlayer }` mirroring `Models/Enums/OfferCreator.cs` (check its numeric values — the wire never carries it, but keep the names)
- Consumes: Tasks 2-4; `loot::item_helper::{get_item, is_of_baseclass, is_of_baseclasses, armor_item_can_hold_mods, armor_item_has_removable_plate_slots, get_removable_plate_slot_ids}`.

| Rust fn | C# lines | Parity requirements (each is an assertion target) |
|---|---|---|
| `randomise_offer_item_upd_properties` | 641-666 | `add_missing_conditions(first)` **always**, before anything else; then, **only for `FakePlayer`**, `get_dynamic_condition_id_for_tpl`; `None` → return with **no draw**; else one `get_chance_100(condition[id].conditionChance * 100)` and, on success, `randomise_item_condition` |
| `get_dynamic_condition_id_for_tpl` | 673-686 | iterates `dynamic.condition` **keys in insertion order**, first baseclass match wins — `IndexMap` is the contract |
| `randomise_item_condition` | 694-774 | **`:699` reads `itemConditionValues.Max.Min` twice** — `get_double(max.min, max.min)`, which is a degenerate range that **still consumes a draw** (`RandomUtil.GetDouble:76-80` always calls `GetSecureRandomNumber`). Preserve exactly; do not "fix" to `Max.Max`. Then `get_double(current.min, current.max)`. Branch order: armor (`armor_item_can_hold_mods \|\| is_of_baseclasses([ARMOR_PLATE, ARMORED_EQUIPMENT])`) → weapon → medkit → key → food/drink → repair kit → fuel. Armor branch: `randomise_armor_durability_values`, then the visor hunt — **`item.ParentId == BaseClasses.ARMORED_EQUIPMENT.ToString() && item.SlotId == "mod_equipment_000"`, comparing a *parent item id* against a *baseclass tpl*, which never matches on real data** (dead branch, port it); on a match, `get_chance_100(25)` then `get_int(1, 3)`. Medkit: `round(hp * max_multiplier)`, `0 → 1`. Key: `round(maxUses * (1 - max_multiplier))`. Food: `round(maxResource * max_multiplier)`, `0 → 1`. Repair kit: `round(maxRepairResource * max_multiplier)`, `0 → 1`. Fuel (`is_of_baseclass(FUEL)`): one more `get_double(max_multiplier, 1)`, then `Upd.Resource { units_consumed: total - remaining, value: remaining }` |
| `randomise_weapon_durability` | 783-796 | four draws in order: `get_double(max_mult, 1)`, `get_double(lowest_max, base_max)`, `get_double(current_mult, 1)`, `get_double(lowest_current, chosen_max)`; `round` on the 2nd and 4th; `durability == 0 → 1` |
| `randomise_armor_durability_values` | 804-827 | **per child item**, and only when `armor_class > 1` — so the draw count depends on the child list; four draws per qualifying item in the same order as above; writes a fresh `UpdRepairable` |
| `add_missing_conditions` | 835-877 | first matching arm wins and **returns**: repairable (`durability != null && durability > 0`) → `UpdRepairable{d, d}`; medkit (`maxHpResource != null && > 0`) → `UpdMedKit`; key (`maximumNumberOfUsage != null`) → `UpdKey{0}`; consumable (`maxResource > 1 && foodUseTime != null`) → `UpdFoodDrink`; repair kit (`maxRepairResource != null`) → `UpdRepairKit`. **No draws.** A template missing from the items view is an unguarded C# NRE at `:837` → `LootError` |
| `remove_banned_plates_from_preset` | 381-416 | `!armor_item_can_hold_mods(root)` → false; plate slots are the children whose lowercased `slotId` is in `get_removable_plate_slot_ids()`; per plate: `ignoreSlots` skip, then `armorClass ?? 0 > maxProtectionLevel` → remove **by `IndexOf` on the live list while iterating a snapshot** (`:410` `Splice`), which shifts later indexes — reproduce by collecting the plate items first, then removing each by identity in that same order. **No draws** |
| `remove_armor_plates` | 508-528 | one `get_chance_100(armor.removeRemovablePlateChance)` **always drawn first**, then the `armor_item_has_removable_plate_slots` gate (so a non-plate armor still consumes the draw); removals are by **descending index** off a `HashSet` of indexes — `HashSet<int>.OrderByDescending` is a total order, so a `Vec<usize>` sorted descending is equivalent |

- [ ] **Step 1: Read `RagfairOfferGenerator.cs:381-416`, `:508-528` and `:633-877` in full.** Write the RNG-call list for one `randomise_offer_item_upd_properties` call per item class as a module doc-comment — that list is the parity checklist for Task 13.
- [ ] **Step 2: Failing seeded tests.** Fixture: a weapon template with `MaxDurability`, an armor with two plate children (classes 4 and 6) and a soft insert, a medkit, a key, a food item, a repair kit, a fuel can, and a plain item with no condition config entry. Cases: (a) each branch of `randomise_item_condition` with its exact draw count pinned by reading the next draw; (b) the `Max.Min` double-read asserted directly — set `max.min != max.max` and assert the chosen multiplier equals `max.min` exactly while the draw was still consumed; (c) `add_missing_conditions` arm precedence with an item matching two arms; (d) `remove_armor_plates` consumes its draw for an armor with no removable plate slots; (e) `remove_banned_plates_from_preset` removes only the over-level, non-ignored plate and leaves indexes consistent; (f) the dead visor branch never fires on realistic data (assert the plate armor's `Upd.FaceShield` is absent).
- [ ] **Step 3: Implement.** **Step 4: Run** — `cargo test -p spt-native ragfair::offer_generator` → PASS.
- [ ] **Step 5: Gates + commit** — `feat: port ragfair item condition randomisation and plate removal to spt-native`

### Task 7: `offer_generator.rs` — barter schemes, currency schemes, the offer object

**Files:**
- Modify: `rust/spt-native/src/ragfair/offer_generator.rs`, `rust/spt-native/src/ragfair/models.rs` (nothing new expected; grow only if a config member is missing)

**Interfaces:**
- Produces:
  - `fn create_barter_barter_scheme(ctx, offer_items: &[Item], barter_config: &BarterDetailsWire) -> Result<Vec<BarterScheme>, LootError>` (`:885`)
  - `fn create_currency_barter_scheme(ctx, offer_with_children: &[Item], is_pack_offer: bool, multiplier: f64) -> Result<Vec<BarterScheme>, LootError>` (`:963`)
  - `fn get_flea_prices_as_array(ctx) -> Vec<TplWithFleaPrice>` (`:933`)
  - `fn create_offer(ctx, details: &CreateFleaOfferDetails, offer_counter: &mut i32) -> Result<RagfairOfferWire, LootError>` (`:85`)
  - `fn create_user_data_for_flea_offer(ctx, user_id: &str, is_trader: bool) -> Result<RagfairOfferUserWire, LootError>` (`:151`)
  - `fn convert_offer_requirements_into_roubles(ctx, requirements: &[OfferRequirementWire]) -> f64` (`:198`)
  - `fn calculate_rouble_price(ctx, currency_count: f64, currency_type: &str) -> f64` (`:229`)
  - `fn get_offer_end_time(ctx, creator: OfferCreator, user_id: &str, time: i64) -> i64` (`:269`)
  - `pub(crate) struct CreateFleaOfferDetails { user_id, time, items, barter_scheme, loyal_level, quantity, creator, sell_in_one_piece }` mirroring `Models/Spt/Ragfair/CreateFleaOfferDetails.cs` (read it for exact members), and `pub(crate) struct BarterScheme { count: f64, template: String, only_functional: Option<bool>, level: Option<i32>, side: Option<i32> }` mirroring the C# `BarterScheme`
- Consumes: Tasks 2-6; `loot::mongo_id::new_mongo_id`, `loot::random_util::{get_int, get_double, get_array_value, get_bool, generate_account_id}`, `loot::item_helper::add_cartridges_to_ammo_box`.

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `create_barter_barter_scheme` | 885-927 | `get_dynamic_offer_price_for_offer(items, ROUBLES, false)` first (**consumes its own price draw**); price below `minRoubleCostToBecomeBarter` → fall through to `create_currency_barter_scheme` (**which draws again** — two price draws total on that path, and that is legacy behaviour); else `get_int(itemCountMin, itemCountMax)`; `desiredItemCostRouble = round(price / count)`; variance = `desired * priceRangeVariancePercent / 100`; filter `get_flea_prices_as_array()` to `min <= p <= max && tpl != rootTpl`; **empty → `create_currency_barter_scheme` (a third price draw)**; else one `get_array_value` over the filtered list **in flea-price-map order** |
| `get_flea_prices_as_array` | 933-954 | legacy caches this in `AllowedFleaPriceItemsForBarter` **per generator instance and never invalidates it** (`:56`); Rust re-derives per call, which makes the native path *fresher*. Documented divergence (spec §8) — put it in the doc-comment. Derivation: every `flea_prices` entry whose tpl is in the items view, minus `barter.itemTypeBlacklist` baseclasses, minus `barter.itemTplBlacklist`, **in flea-price-map insertion order** |
| `create_currency_barter_scheme` | 963-969 | `get_dynamic_offer_currency()` **first** (one weighted draw), then `get_dynamic_offer_price_for_offer(items, currency, is_pack)` (one biased draw), then `* multiplier` |
| `create_offer` | 85-143 | requirement mapping: `count = round(barter.count, 2)` (**`Math.Round(x, 2)` is banker's rounding to 2 dp** — use `round_to_digits`), `only_functional = barter.onlyFunctional ?? false`, `level`/`side` only when `barter.level != null`; **ammo-box hydration at `:110-113`** — `items.len() == 1 && is_of_baseclass(items[0].template, AMMO_BOX)` → `add_cartridges_to_ammo_box`; `roubleListingPrice = round(convert_offer_requirements_into_roubles(reqs))`; `singleItemListingPrice = sell_in_one_piece ? rouble / quantity : rouble`; the offer object per `:118-138` with `id = new_mongo_id()`, `internal_id = *offer_counter`, `items_cost = round(handbook_prices[root.template])`, `requirements_cost = round(single)`, `summary_cost = rouble`, `locked = false`; **`*offer_counter += 1` at the end (`:140`), even for offers the C# holder later rejects** |
| `create_user_data_for_flea_offer` | 151-170 | trader → `{ id, member_type: Trader }` **and no draws**; fake player → **five draws in source order**: (1) `get_int(0, 1)` for the PMC faction (`BotHelper.cs:151`), (2) `get_array_value` over that faction's name pool, (3) `get_double(rating.min, rating.max)`, (4) `get_bool()`, (5) `generate_account_id()`. `avatar` is `None`. An empty name pool is an unguarded C# throw in `GetRandomElement` → `LootError` |
| `convert_offer_requirements_into_roubles` | 198-205 | per requirement: money tpl → `round(calculate_rouble_price(count, tpl))`; otherwise `get_flea_price_for_item(tpl) * count` (**no rounding on this arm**). Money tpls are the three `Money` constants — read `Models/Enums/Money.cs` and `Helpers/Commerce/PaymentHelper.cs` `IsMoneyTpl` for the exact set |
| `calculate_rouble_price` | 229-237 | roubles → passthrough; else `round(count * handbook_prices[currency])` (`HandbookHelper.InRoubles`) |
| `get_offer_end_time` | 269-287 | `FakePlayer` is the only arm this port reaches: one `get_double(endTimeSeconds.min, endTimeSeconds.max)`, result `round(time + spread)` as an `i64`. Port the `Player` and `Trader` arms too (bug-for-bug), returning a `LootError` for `Trader` (it needs the trader table, which is not on the wire) and noting both are unreachable from `generate_dynamic_offers` |

`GetAvatarUrl` (`:213`) and `GetRating` (`:244`) are **not** ported — they are dead on this path (nothing calls them from `GenerateDynamicOffers`). They stay in the C# hookable set (Task 11) precisely because a mod can still patch them.

- [ ] **Step 1: Read `RagfairOfferGenerator.cs:66-237`, `:269-287` and `:879-969` in full**, plus `Models/Spt/Ragfair/CreateFleaOfferDetails.cs` and `Helpers/Commerce/PaymentHelper.cs`. Extend the module doc-comment's RNG list with the offer-object draws.
- [ ] **Step 2: Failing seeded tests.** Extend the module fixture with three currencies, a barter-eligible price map and both PMC name pools. Cases: (a) `create_user_data_for_flea_offer` draw order — pin all five values and assert the nickname came from the faction the first draw selected; (b) barter scheme above the threshold picks an in-range tpl, pinned; (c) below the threshold falls back to currency **and the fall-through consumed a second price draw** (pinned by the next draw); (d) an empty in-range list falls back to currency; (e) `create_offer` on a single ammo-box item hydrates cartridges; (f) `create_offer` increments the counter and the `intId`s of two consecutive offers differ by one; (g) `requirements_cost` for a pack offer is `round(total / quantity)`.
- [ ] **Step 3: Implement.** **Step 4: Run** — `cargo test -p spt-native ragfair::offer_generator` → PASS.
- [ ] **Step 5: Gates + commit** — `feat: port ragfair barter schemes and offer object construction to spt-native`

### Task 8: `offer_generator.rs` — the orchestrator

**Files:**
- Modify: `rust/spt-native/src/ragfair/offer_generator.rs`, `rust/spt-native/src/ragfair/mod.rs` (finalize `RagfairContext`)

**Interfaces:**
- Produces: `pub fn generate_dynamic_offers(request: GenerateDynamicOffersRequest) -> Result<DynamicOffersResult, LootError>` — the fn Task 9 exports. Installs `TestSeedGuard` from `request.test_seed` (plain `install`, single entry point, same as reward loot and bots). Plus `fn create_offers_from_assort(ctx, assort_item_with_children, is_expired_offer, offers: &mut Vec<RagfairOfferWire>, rejected: &mut IndexSet<String>, offer_counter: &mut i32) -> Result<(), LootError>` (`:332`) and `fn create_single_offer_for_item(ctx, seller_id, item_with_children, is_preset, item_to_sell_tpl, is_expired_offer, offer_creator, offers, offer_counter) -> Result<(), LootError>` (`:427`).
- Consumes: everything above.

| Rust fn | C# lines | Parity requirements |
|---|---|---|
| `generate_dynamic_offers` | 293-324 | `replacing_expired_offers = expired_offers.is_some() && !expired_offers.is_empty()`; the assort source is the expired list when replacing, else `assort_generator::generate_ragfair_assort_items(ctx)`; the two `Stopwatch` debug lines become `ctx.diagnostics.push(plain("debug", …))` lines with the same text (`"Took {n}ms to GetRagfairAssorts - {count} items"` / `"Took {n}ms to CreateOffersFromAssort"`) using a Rust `Instant`. **`Task.Factory.StartNew` (`:311`) is NOT reproduced — the walk is sequential** (spec §4, sanctioned divergence: production RNG is crypto-random and the legacy interleaving is nondeterministic anyway). Say so in the doc-comment |
| `create_offers_from_assort` | 332-373 | root's template must be in the items view — legacy dereferences `itemToSellDetails.Value` unguarded at `:352` → `LootError`; **`!(is_expired_offer \|\| is_item_valid_ragfair_item(...))` → return early with no draws** (note the short-circuit: an expired offer never runs the validity check, so it never contributes to `rejected`); `is_preset` = `root.upd.spt_preset_id` present **and** in `item_presets`; banned-plate removal only when `!is_expired && is_preset && blacklist.enableBsgList`; `offer_count` = `1` when expired (**no draw**) else `get_offer_count_by_base_type(parent)` (one draw); per offer: clone the assort list, `reparent_item_and_children`, clear the root's `parent_id` and `slot_id`, then `create_single_offer_for_item` with a fresh `new_mongo_id()` seller id |
| `create_single_offer_for_item` | 427-501 | **draw order is the whole contract:** (1) `calculate_dynamic_stack_count(root.template, is_preset)` (1 or 2 draws, see Task 4); (2) `root.upd.stack_objects_count = 1` (no draw); (3) `!is_expired && armor_item_can_hold_mods(root.template)` → `remove_armor_plates` (1 draw, conditional); (4) `get_chance_100(barter.chancePercent)` → `is_barter_offer`; (5) `is_pack_offer` = **short-circuit `&&` chain** — `!is_barter && get_chance_100(pack.chancePercent) && items.len() == 1 && is_of_baseclasses(root.template, pack.itemTypeWhitelist)`, so **the pack chance draw is skipped entirely when the barter roll won**; (6) pack branch → `get_int(pack.itemCountMin, itemCountMax)` then `create_currency_barter_scheme(items, true, stack)`; barter branch → `randomise_offer_item_upd_properties` then `create_barter_barter_scheme` then the `makeSingleStackOnly` reset; else branch → `randomise_offer_item_upd_properties` then `create_currency_barter_scheme(items, false, 1)`; (7) `create_offer` with `time = ctx.timestamp`, `loyal_level = 1`, `quantity = desired_stack_size`, `sell_in_one_piece = is_pack_offer`. **Legacy calls `CreateAndAddFleaOffer` (`:500`), which also sets `CreatedBy` and inserts; natively we only build the offer — the insert is the caller's loop** |

- [ ] **Step 1: Read `RagfairOfferGenerator.cs:289-501` in full.** Write the complete draw sequence for one non-preset, non-pack, non-barter item as a doc-comment; that sequence is what Task 13's first fixture pins.
- [ ] **Step 2: Failing tests.** Cases: (a) full pass over the Task 5 fixture, seeded → pinned offer count and pinned normalized offer list; (b) expired-offers pass with one hand-built item list → exactly one offer, and the validity check was **not** run (assert `rejected` is empty even though the tpl is custom-blacklisted); (c) an invalid tpl in a full pass contributes to `rejected` and produces no offers; (d) the pack short-circuit: seeds chosen so the barter roll wins, asserting the next draw is the one `calculate_dynamic_stack_count` of the *following* item would take (i.e. the pack chance was never rolled); (e) `offer_counter_start = 7` → the first offer's `intId` is 7 and they ascend; (f) a template missing from the items view → `LootError`.
- [ ] **Step 3: Implement.** **Step 4: Run** — `cargo test -p spt-native ragfair` → PASS (the whole module).
- [ ] **Step 5: Gates + commit** — `feat: port the ragfair dynamic offer batch pass to spt-native`

### Task 9: FFI export + ABI bump

**Files:**
- Modify: `rust/spt-native/src/ffi.rs`, `rust/spt-native/src/lib.rs` (`ABI_VERSION` 5→6), `Libraries/SPTarkov.Server.Core/Native/NativeMethods.cs` (1 `[LibraryImport]`), `Libraries/SPTarkov.Server.Core/Native/SptNative.cs` (`ExpectedAbiVersion` 5→6 **in the same commit**, wrapper + enum arm)

**Interfaces:**
- Produces: `spt_generate_dynamic_offers(req_ptr: *const u8, req_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32`, and on the C# side `internal static DynamicOffersResult SptNative.GenerateDynamicOffers(GenerateDynamicOffersRequest request)` plus a `LootExport.DynamicOffers` enum arm. `GenerateDynamicOffersRequest`/`DynamicOffersResult` are the C# records Task 10 declares — this task adds the wrapper against them, so **Task 10 must land its record declarations in the same working tree before this task's C# half compiles**; if executing strictly in order, write the wrapper first and let it fail to compile until Task 10, or land Task 10 first. Recommended: do Task 10 first, then this task. The plan keeps this order because the ABI pin test is what proves lockstep, and it wants the C# side already present.
- Consumes: Task 8's `generate_dynamic_offers`.

- [ ] **Step 1: Failing FFI tests** in `ffi.rs`'s existing `#[cfg(test)]` block, mirroring the bot export's three:

```rust
    #[test]
    fn a_minimal_dynamic_offers_request_returns_an_empty_offer_list() {
        // Empty items view and empty presets: the assort walk yields nothing, so no draws happen.
        let (status, out) = call_generate(spt_generate_dynamic_offers, ragfair_request().as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(result["offers"].as_array().unwrap().len(), 0);
        assert_eq!(result["rejectedCanSellTemplates"], serde_json::json!([]));
    }

    #[test]
    fn unparseable_dynamic_offers_request_returns_bad_args_with_the_parse_error() {
        let (status, out) = call_generate(spt_generate_dynamic_offers, b"{\"timestamp\":");

        assert_eq!(status, STATUS_BAD_ARGS);
        assert!(String::from_utf8(out).unwrap().contains("EOF while parsing"));
    }

    #[test]
    fn a_dynamic_offers_failure_returns_status_error_and_the_message() {
        // `offerItemCount` without a "default" entry is the unguarded C# dictionary miss.
        let (status, out) = call_generate(spt_generate_dynamic_offers, ragfair_request_missing_default().as_bytes());

        assert_eq!(status, STATUS_ERROR);
        assert!(String::from_utf8(out).unwrap().contains("default"));
    }
```

  and bump the existing ABI pin test's expected value 5 → 6.

- [ ] **Step 2: Implement the Rust half** — the four-line delegation, pattern-copied from `spt_generate_bot_inventory` (`ffi.rs:225-238`):

```rust
/// # Safety
/// `req_ptr` must point to `req_len` readable bytes of JSON; `out_ptr` and `out_len` must be valid
/// for writes. Any buffer handed back — a result or an error message — must be released with
/// `spt_buf_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_dynamic_offers(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe { run_generator(req_ptr, req_len, out_ptr, out_len, generate_dynamic_offers) }
}
```

  plus the `use crate::ragfair::offer_generator::generate_dynamic_offers;` import and `pub const ABI_VERSION: u32 = 6;` in `lib.rs`.

- [ ] **Step 3: Implement the C# half** — in `NativeMethods.cs`, after the bot import:

```csharp
    [LibraryImport(LibraryName, EntryPoint = "spt_generate_dynamic_offers")]
    internal static partial int GenerateDynamicOffers(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);
```

  in `SptNative.cs`: `private const uint ExpectedAbiVersion = 6;`, a `DynamicOffers` member on the `LootExport` enum, the switch arm

```csharp
                LootExport.DynamicOffers => NativeMethods.GenerateDynamicOffers(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen),
```

  and the wrapper

```csharp
    /// <summary>
    /// Generates one full batch of dynamic flea offers - the assort walk, the per-item offer
    /// creation and the pricing math.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    internal static DynamicOffersResult GenerateDynamicOffers(GenerateDynamicOffersRequest request)
    {
        return Generate<DynamicOffersResult>(LootExport.DynamicOffers, JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }
```

- [ ] **Step 4: Run** — `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`; then `dotnet build server-csharp.slnx && dotnet test --filter "FullyQualifiedName~SptNativeVerifyTests"` → PASS (this is what proves the two version constants moved together).
- [ ] **Step 5: csharpier + commit** — `feat: expose dynamic ragfair offer generation over C ABI and bump ABI to 6`

### Task 10: C# payloads, projection and the wire test

**Files:**
- Create: `Libraries/SPTarkov.Server.Core/Native/Ragfair/RagfairPayloads.cs`, `Libraries/SPTarkov.Server.Core/Native/Ragfair/RagfairPayloadProjection.cs`
- Modify: `Libraries/SPTarkov.Server.Core/Helpers/Bot/BotHelper.cs` (one `internal` accessor)
- Test: `Testing/UnitTests/Tests/Generators/SptNativeRagfairWireTests.cs`

**Interfaces:**
- Consumes: Task 1's Rust wire names, member for member.
- Produces: `internal record GenerateDynamicOffersRequest`, `internal record DynamicOffersResult`, `internal record RagfairOfferWire` (deserialized into the frozen `RagfairOffer` DTO — see Step 3), and `internal static class RagfairPayloadProjection` with

```csharp
    internal static GenerateDynamicOffersRequest BuildRequest(
        IEnumerable<List<Item>>? expiredOffers,
        long timestamp,
        int offerCounterStart,
        ulong? testSeed,
        TemplateTable templateTable,
        HandbookHelper handbookHelper,
        TraderHelper traderHelper,
        PresetHelper presetHelper,
        ItemFilterService itemFilterService,
        SeasonalEventService seasonalEventService,
        BotTable botTable,
        ItemHelper itemHelper,
        BotConfig botConfig,
        RagfairConfig ragfairConfig
    )
```

  (services as parameters, not injected members — this class is a projection, not a component, exactly as `BotPayloadProjection` is).

- [ ] **Step 1: Write the failing wire test** `SptNativeRagfairWireTests.cs`, modelled on `SptNativeBotWireTests.cs`:

```csharp
[TestFixture]
public class SptNativeRagfairWireTests
{
    private const ulong TestSeed = 42;

    private GenerateDynamicOffersRequest _request = default!;
    private RagfairConfig _ragfairConfig = default!;

    [OneTimeSetUp]
    public void Initialize()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions the wrapper serialises the payload with
        di.GetService<JsonUtil>();
        _ragfairConfig = di.GetService<RagfairConfig>();

        _request = RagfairPayloadProjection.BuildRequest(
            null,
            di.GetService<TimeUtil>().GetTimeStamp(),
            0,
            TestSeed,
            di.GetService<TemplateTable>(),
            di.GetService<HandbookHelper>(),
            di.GetService<TraderHelper>(),
            di.GetService<PresetHelper>(),
            di.GetService<ItemFilterService>(),
            di.GetService<SeasonalEventService>(),
            di.GetService<BotTable>(),
            di.GetService<ItemHelper>(),
            di.GetService<BotConfig>(),
            _ragfairConfig
        );
    }

    [Test]
    public void TheProjectionFillsEveryBlockTheNativeSideReads()
    {
        Assert.Multiple(() =>
        {
            Assert.That(_request.Items, Is.Not.Empty);
            Assert.That(_request.FleaPrices, Is.Not.Empty);
            Assert.That(_request.HandbookPrices, Is.Not.Empty);
            Assert.That(_request.HighestTraderPrices, Is.Not.Empty);
            Assert.That(_request.ItemPresets, Is.Not.Empty);
            Assert.That(_request.DefaultPresets, Is.Not.Empty);
            Assert.That(_request.DefaultPresetsByTpl, Is.Not.Empty);
            Assert.That(_request.PresetsByTpl, Is.Not.Empty);
            Assert.That(_request.PmcNamesUsec, Is.Not.Empty);
            Assert.That(_request.PmcNamesBear, Is.Not.Empty);
            Assert.That(_request.ExpiredOffers, Is.Null);
        });
    }

    /// <summary>
    /// The EftEnumConverter pitfall the bot port hit: System.Text.Json writes enum dictionary keys
    /// as numbers. Every ragfair map that crosses must be keyed by a string or a MongoId, never by
    /// an enum, or the native side silently finds nothing.
    /// </summary>
    [Test]
    public void EveryProjectedDictionaryKeyIsAStringOnTheWire()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!.AsObject();

        foreach (var block in new[] { "fleaPrices", "handbookPrices", "highestTraderPrices", "itemPresets", "defaultPresetsByTpl" })
        {
            foreach (var entry in json[block]!.AsObject())
            {
                Assert.That(long.TryParse(entry.Key, out _), Is.False, $"{block} key '{entry.Key}' serialised as a number");
            }
        }

        // dynamic.condition and dynamic.offerItemCount are the two config maps the native side
        // iterates by key; a numeric key here would break the baseclass match and the offer count
        foreach (var entry in json["dynamic"]!["condition"]!.AsObject())
        {
            Assert.That(new MongoId(entry.Key).IsEmpty, Is.False, $"condition key '{entry.Key}' is not a tpl");
        }
    }

    [Test]
    public void TheRequestRoundTripsThroughTheNativeSide()
    {
        var result = SptNative.GenerateDynamicOffers(_request);

        Assert.That(result.Offers, Is.Not.Empty);
        Assert.That(result.Offers[0].Items, Is.Not.Empty);
        Assert.That(result.Offers[0].User.Nickname, Is.Not.Null.And.Not.Empty);
        Assert.That(result.Offers[0].SummaryCost, Is.GreaterThan(0));
        // Id is declared MongoId on the C# record, so a malformed hex string would already have
        // failed the deserialize; this catches an all-zero default
        Assert.That(result.Offers[0].Id.IsEmpty, Is.False);
    }

    /// <summary>
    /// A mod-added field on a game-data object inside the payload must survive the round trip - the
    /// `[serde(flatten)] extra` contract that mirrors Ceciler's `[JsonExtensionData]`.
    /// </summary>
    [Test]
    public void AModAddedConfigFieldSurvivesTheRoundTrip()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!.AsObject();
        json["dynamic"]!["modAddedField"] = "kept";

        // No assertion on the value coming back - the native result carries offers, not the config;
        // this asserts only that an unknown key does not fail the parse.
        var result = SptNative.Generate<DynamicOffersResult>(
            LootExport.DynamicOffers,
            System.Text.Encoding.UTF8.GetBytes(json.ToJsonString())
        );

        Assert.That(result.Offers, Is.Not.Empty);
    }
}
```

- [ ] **Step 2: Run** → FAIL (`RagfairPayloadProjection` missing).

- [ ] **Step 3: Implement `RagfairPayloads.cs`.** Mirror Task 1's Rust types member for member, following the `BotPayloads.cs` conventions verbatim: `internal record`s, `[JsonPropertyName]` on every member, `required` on everything Rust does not declare `Option`, existing `Models` records reused wherever the wire name already matches (`Dynamic` from `RagfairConfig.cs` is reused directly as the `dynamic` block — its `[JsonPropertyName]`s are what Task 1's Rust names were pinned to, so it stays authoritative by construction), `PresetView` from `Native/Loot/LootPayloads.cs:350` reused for the four preset maps, `ItemView` reused for `items`, `Diagnostic` reused for `diagnostics`. The **response** side declares `RagfairOfferWire` rather than deserializing straight into `RagfairOffer`, because `RagfairOffer.Requirements` is an `IEnumerable<OfferRequirement>` and `RagfairOffer.User` is `required`. Every id-shaped member on the wire records (`Id`, `Root`, `User.Id`, `OfferRequirementWire.TemplateId`) is declared `MongoId`, which round-trips through the hex string Rust emits. Add the mapper as a static class in the same file — block-bodied, per CLAUDE.md:

```csharp
internal static class RagfairOfferWireExtensions
{
    /// <summary>
    /// The native offer as the frozen 4.1.2 DTO the holder stores. <c>SellResults</c>,
    /// <c>UnlimitedCount</c> and the two buy-restriction members stay at their defaults: the
    /// dynamic path never sets them (<c>RagfairOfferGenerator.cs:118-138</c>).
    /// </summary>
    internal static RagfairOffer ToRagfairOffer(this RagfairOfferWire wire)
    {
        return new RagfairOffer
        {
            Id = wire.Id,
            InternalId = wire.InternalId,
            User = new RagfairOfferUser
            {
                Id = wire.User.Id,
                Nickname = wire.User.Nickname,
                Rating = wire.User.Rating,
                // The wire carries the numeric EftEnumConverter value, matching CreateUserDataForFleaOffer
                MemberType = (MemberCategory)wire.User.MemberType,
                Avatar = wire.User.Avatar,
                IsRatingGrowing = wire.User.IsRatingGrowing,
                Aid = wire.User.Aid,
            },
            Root = wire.Root,
            Items = wire.Items,
            ItemsCost = wire.ItemsCost,
            Requirements = wire
                .Requirements.Select(requirement => new OfferRequirement
                {
                    TemplateId = requirement.TemplateId,
                    Count = requirement.Count,
                    OnlyFunctional = requirement.OnlyFunctional,
                    Level = requirement.Level,
                    Side = (DogtagExchangeSide?)requirement.Side,
                })
                .ToList(),
            RequirementsCost = wire.RequirementsCost,
            SummaryCost = wire.SummaryCost,
            StartTime = wire.StartTime,
            EndTime = wire.EndTime,
            LoyaltyLevel = wire.LoyaltyLevel,
            SellInOnePiece = wire.SellInOnePiece,
            Locked = wire.Locked,
            Quantity = wire.Quantity,
            // What CreateAndAddFleaOffer:72 sets; the holder's fake-player cap keys off it
            CreatedBy = OfferCreator.FakePlayer,
        };
    }
}
``` Document that `sellResult`/`unlimitedCount`/`buyRestriction*` are left at their defaults because the dynamic path never sets them.

- [ ] **Step 4: Implement `RagfairPayloadProjection.BuildRequest`.** Every block, with its source:

| Wire member | Source |
|---|---|
| `TestSeed`, `Timestamp`, `OfferCounterStart`, `ExpiredOffers` | parameters |
| `Dynamic` | `ragfairConfig.Dynamic` (the live object — mods mutate it at runtime and the projection must see that) |
| `ItemPresets` | `globalTable.ItemPresets` via `presetHelper` — reuse `BotPayloadProjection.ToPresetViews`' shape; if that private helper cannot be reached, copy it (four lines) rather than widening its visibility |
| `DefaultPresets` | `presetHelper.GetDefaultPresets().Values.ToList()` projected to `PresetView` |
| `DefaultPresetsByTpl` | `presetHelper.GetDefaultPresetByTpl()` |
| `PresetsByTpl` | for each tpl that `presetHelper.HasPreset(tpl)` over the items table, `presetHelper.GetPresets(tpl)`; build it in items-table order |
| `FleaPrices` | `templateTable.Prices` — the whole map, order preserved |
| `HandbookPrices` | `handbookHelper.GetTemplatePrice(tpl)` for every tpl in `templateTable.Items` (whole table, not pool-scoped: the pricing math reaches arbitrary tpls through barter and preset children) |
| `HighestTraderPrices` | `traderHelper.GetHighestSellToTraderPrice(tpl)` for every tpl in `templateTable.Items` — cache-backed on the C# side (`TraderHelper.cs:516-547`), so this loop is cheap after the first pass |
| `ConfigBlacklist` | `itemFilterService.GetBlacklistedItems()` |
| `SeasonalEventActive` | `seasonalEventService.SeasonalEventEnabled()` |
| `SeasonalItemTplBlacklist` | `seasonalEventService.GetInactiveSeasonalEventItems()` |
| `PmcNamesUsec` / `PmcNamesBear` | `botTable.Types["usec"].FirstNames` / `["bear"].FirstNames` filtered to `name.Length <= botConfig.BotNameLengthLimit`, **falling back to the unfiltered list when the filtered one is empty** — `BotHelper.GatherPmcNamesOfLength:183-200`, read it and mirror the fallback exactly |
| `Items` | `PayloadProjection.BuildItemsView(itemHelper.TemplateTable.Items)` |

  `BotTable` is not a `RagfairOfferGenerator` constructor parameter, so add the accessor to `Helpers/Bot/BotHelper.cs` next to the class's other members (the `botLootGenerator.BotLootCacheService` precedent):

```csharp
    /// <summary>
    ///     The bot table this helper reads its PMC name pools from, so the native ragfair projection
    ///     can build those pools without a constructor change on its caller.
    /// </summary>
    internal BotTable BotTable
    {
        get { return botTable; }
    }
```

- [ ] **Step 5: Run** — `dotnet build server-csharp.slnx && dotnet test --filter "FullyQualifiedName~SptNativeRagfairWireTests"` → PASS, and `dotnet test --filter "FullyQualifiedName~DependencyInjectionValidationTests"` → green.
- [ ] **Step 6: csharpier + commit** — `feat: C# payloads and projection for native ragfair offer generation`

### Task 11: dispatch cutover in `RagfairOfferGenerator`

**Files:**
- Modify: `Libraries/SPTarkov.Server.Core/Generators/Ragfair/RagfairOfferGenerator.cs` (dispatch only — the 4.1.2 body moves verbatim to `GenerateDynamicOffersLegacy`), `Libraries/SPTarkov.Server.Core/Models/Spt/Config/RagfairConfig.cs` (one property), `Libraries/SPTarkov.Server.Core/Services/Ragfair/RagfairPriceService.cs` (one `internal TraderHelper` accessor), `Libraries/SPTarkov.Server.Core/Generators/Ragfair/RagfairAssortGenerator.cs` (`internal ItemFilterService` + `internal SeasonalEventService` accessors)

**Interfaces:**
- Consumes: Task 10's `RagfairPayloadProjection.BuildRequest` / `SptNative.GenerateDynamicOffers`.
- Produces: the seams Tasks 12-14 test — `internal LootGenerationPath LastPathTaken { get; private set; }` and `internal ulong? NativeTestSeed { get; set; }` (copy the declarations verbatim from `BotInventoryGenerator.cs:96/:102`), and the `private static readonly List<MethodBase> _hookableMembers` set.

- [ ] **Step 1: Add the config flag** to `Models/Spt/Config/RagfairConfig.cs`, at the end of the `RagfairConfig` record (top level, **not** inside `Dynamic`):

```csharp
    /// <summary>
    ///     Force dynamic flea offer generation down the retained 4.1.2 C# path instead of
    ///     spt-native. The escape hatch for hooks the patch detection cannot see - patches on the
    ///     shared helpers listed in ARCHITECTURE.md's ragfair section.
    /// </summary>
    [JsonPropertyName("forceLegacyRagfairGeneration")]
    public bool ForceLegacyRagfairGeneration { get; set; }
```

  An absent JSON key deserializes to `false`, so `configs/ragfair.json` needs no edit — same as `ForceLegacyLootGeneration` and `ForceLegacyBotGeneration`.

- [ ] **Step 2: Build the hookable set.** Pattern-copy `BotInventoryGenerator.cs:107-119` but over **four types**, with `GenerateDynamicOffers` excluded:

```csharp
    /// <summary>
    ///     The 4.1.2 members a mod can Harmony-patch, across this class and the three collaborators
    ///     the native path folds in. Public, protected and protected-internal methods declared on
    ///     each - exactly the surface the apicompat gate freezes, statics included.
    ///     <see cref="GenerateDynamicOffers"/> itself is excluded: a patch on the dispatcher wraps
    ///     whichever path runs and does not need the legacy body. Everything else is never called
    ///     natively, so a patch on one would silently do nothing - including the dead-but-frozen
    ///     <c>GetRating</c> and <c>GetAvatarUrl</c>.
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. new[]
        {
            typeof(RagfairOfferGenerator),
            typeof(RagfairPriceService),
            typeof(RagfairServerHelper),
            typeof(RagfairAssortGenerator),
        }
            .SelectMany(type =>
                type.GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
            )
            // Property accessors and operators are IsSpecialName; constructors are not returned at all
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            .Where(method => method != typeof(RagfairOfferGenerator).GetMethod(nameof(GenerateDynamicOffers))),
    ];
```

- [ ] **Step 3: Write `UseLegacyPath`.** Four conditions, cheapest first, exactly the spec §2 order:

```csharp
    /// <summary>
    ///     The legacy path runs when forced by config, when any of the frozen 4.1.2 members carries a
    ///     live Harmony patch, or when a mod has substituted one of the collaborators the native path
    ///     folded in - running the retained C# implementation is the only way those hooks and
    ///     replacements can take effect with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (ragfairConfig.ForceLegacyRagfairGeneration)
        {
            return true;
        }

        if (
            _hookableMembers.Any(member =>
                Harmony.GetPatchInfo(member) is { } patches
                && (
                    patches.Prefixes.Count > 0
                    || patches.Postfixes.Count > 0
                    || patches.Transpilers.Count > 0
                    || patches.Finalizers.Count > 0
                )
            )
        )
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return ragfairPriceService.GetType() != typeof(RagfairPriceService)
            || ragfairServerHelper.GetType() != typeof(RagfairServerHelper)
            || ragfairAssortGenerator.GetType() != typeof(RagfairAssortGenerator);
    }
```

  (Conditions 2 and 4 of the spec are one check: the hookable set already spans all four types.)

- [ ] **Step 4: Convert `GenerateDynamicOffers` to the dispatcher.** Rename the current body to `private void GenerateDynamicOffersLegacy(IEnumerable<List<Item>>? expiredOffers = null)` **with zero interior edits** — verify with `git diff --stat` that the rename commit deletes no lines from inside the body. The new body:

```csharp
    /// <summary>
    ///     Create multiple offers for items by using a unique list of items we've generated previously
    /// </summary>
    /// <param name="expiredOffers"> Optional, expired offers to regenerate </param>
    public void GenerateDynamicOffers(IEnumerable<List<Item>>? expiredOffers = null)
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;

            GenerateDynamicOffersLegacy(expiredOffers);

            return;
        }

        LastPathTaken = LootGenerationPath.Native;

        var result = SptNative.GenerateDynamicOffers(
            RagfairPayloadProjection.BuildRequest(
                expiredOffers,
                timeUtil.GetTimeStamp(),
                OfferCounter,
                NativeTestSeed,
                templateTable,
                handbookHelper,
                ragfairPriceService.TraderHelper,
                presetHelper,
                ragfairAssortGenerator.ItemFilterService,
                ragfairAssortGenerator.SeasonalEventService,
                botHelper.BotTable,
                itemHelper,
                botConfig,
                ragfairConfig
            )
        );

        PayloadProjection.ReplayDiagnostics(result.Diagnostics, logger, localisationService);

        // The native side decided these templates are unsellable and, unlike everything else it
        // touched, that decision belongs to the live database (RagfairServerHelper.cs:61)
        foreach (var tpl in result.RejectedCanSellTemplates)
        {
            if (templateTable.Items.TryGetValue(tpl, out var template) && template.Properties is not null)
            {
                template.Properties.CanSellOnRagfair = false;
            }
        }

        // Legacy inserts each offer as it creates it; the holder's live per-template cap runs the
        // same way either way, it just sees the whole batch at once here
        foreach (var offer in result.Offers)
        {
            ragfairOfferService.AddOffer(offer.ToRagfairOffer());
        }

        // CreateOffer increments the counter per offer created, not per offer the holder accepted
        OfferCounter += result.Offers.Count;
    }
```

  `TraderHelper`, `ItemFilterService` and `SeasonalEventService` are not `RagfairOfferGenerator` constructor parameters. **Do not add constructor parameters** — reach them through `internal` accessors on the collaborators that already have them. In `Services/Ragfair/RagfairPriceService.cs`:

```csharp
    /// <summary>
    ///     Exposed so the native ragfair projection can resolve the per-template highest trader
    ///     price without a constructor change on <c>RagfairOfferGenerator</c>.
    /// </summary>
    internal TraderHelper TraderHelper
    {
        get { return traderHelper; }
    }
```

  and in `Generators/Ragfair/RagfairAssortGenerator.cs`:

```csharp
    /// <summary>
    ///     Exposed so the native ragfair projection can build the same blacklist and seasonal
    ///     inputs this generator reads, without a constructor change on its caller.
    /// </summary>
    internal ItemFilterService ItemFilterService
    {
        get { return itemFilterService; }
    }

    /// <inheritdoc cref="ItemFilterService"/>
    internal SeasonalEventService SeasonalEventService
    {
        get { return seasonalEventService; }
    }
```

- [ ] **Step 5: Run** — `dotnet build server-csharp.slnx && dotnet test` → all green. Existing `RagfairHolderTests` and the price-service tests sit below the boundary and still run C#; anything driving `GenerateDynamicOffers` now runs native by default.
- [ ] **Step 6: Verify the legacy body is untouched** — `git diff -- Libraries/SPTarkov.Server.Core/Generators/Ragfair/RagfairOfferGenerator.cs | grep '^-' | grep -v '^---'` should show only the old signature line and its doc-comment.
- [ ] **Step 7: csharpier + commit** — `feat: route dynamic ragfair offer generation through spt-native behind the dual path`

### Task 12: benchmark — the early gate (spec §7)

**Files:**
- Create: `Testing/UnitTests/Tests/Generators/RagfairBenchmarkTests.cs`
- Modify: `BENCHMARK.md`

**This task gates the rest of the plan.** Spec §7: the unknown is response volume — tens of thousands of offers with full item trees is more serialisation than any existing export. Run it as soon as the happy path works. If serialisation drowns the win, **stop and reassess before Tasks 13-15**; the sanctioned lever is the shared items-view cache (RUST-ROADMAP.md roadmap #3), not snapshots.

**Interfaces:**
- Consumes: `LastPathTaken`, `ForceLegacyRagfairGeneration`, `RagfairPayloadProjection.BuildRequest`.

- [ ] **Step 1: Write the benchmark**, pattern-copied from `RewardLootBenchmarkTests.cs` and `BotBenchmarkTests.cs` (`[TestFixture] [Explicit("benchmark, run on demand in Release")] [NonParallelizable]`, `WarmupRuns = 1`, `TimedRuns = 5` — a full pass is tens of thousands of offers, so 20 runs is minutes). Two scenarios, both paths each:
  - **Full pass**: `GenerateDynamicOffers()` with no expired offers.
  - **Regeneration pass**: `GenerateDynamicOffers(expired)` where `expired` is 1,400 cloned single-item lists sampled from the assort — the shape `RagfairServer.cs:69` produces at the configured `expiredOfferThreshold`.
  Time `RagfairPayloadProjection.BuildRequest` separately (that number is the floor under the native path and the share an items-view cache would buy), and record the offer count each path produced. **Purge the holder between runs** — `ragfairOfferService.RemoveOfferById` over the ids added during the run, captured by diffing `GetOffers()` before and after — otherwise run 2 measures a holder that rejects most of what it is handed.
- [ ] **Step 2: Run in Release** — `dotnet test -c Release --filter "FullyQualifiedName~RagfairBenchmarkTests"` with the `[Explicit]` override the other benchmarks document in their headers. Record median ms per pass, native vs legacy, plus the projection share and the peak working set, following `BENCHMARK.md` § Methodology.
- [ ] **Step 3: Judge.** Native slower than legacy on the full pass ⇒ **stop and report** rather than continuing to Task 13. Faster or within noise ⇒ continue and record the numbers.
- [ ] **Step 4: Write the numbers into `BENCHMARK.md`** as a new "Results — ragfair offer generation" section, matching the structure of the bot-generation section (wall clock table, projection share, caveats).
- [ ] **Step 5: csharpier + commit** — `test: benchmark native vs legacy dynamic flea offer generation`

### Task 13: parity tests

**Files:**
- Create: `Testing/UnitTests/Tests/Generators/RagfairParityTests.cs`

**Interfaces:**
- Consumes: `LastPathTaken`/`NativeTestSeed` seams, `LootIdNormalizer` (reuse as-is), `LootJsonAssert`, the `SaveServer.CreateProfile(new Info { ProfileId = sessionId })` stub pattern.

**Why the per-item fixtures drive the *expired-offers* path.** Spec §5.1 asks for "the assort restricted to a single template". There is no side-effect-free way to restrict the live assort walk to one tpl — the only per-tpl lever, `dynamic.blacklist.custom`, mutates `CanSellOnRagfair` on the live database as it rejects. `GenerateDynamicOffers(expiredOffers)` takes the item lists directly (`:299`), which restricts the pass to exactly the items handed in, is a real production path (`RagfairServer.cs:79`) and is deterministic on both sides. It is therefore the per-item parity vehicle. Its cost: `isExpiredOffer == true` skips `IsItemValidRagfairItem`, `RemoveBannedPlatesFromPreset`, `RemoveArmorPlates` and the offer-count draw (`:338/:345/:352/:444`). Those four branches are covered by the Rust module tests (Tasks 5-8), the replay test (Task 14) and the whole-pass structural case below.

**Two more hazards this fixture must handle, both discovered while reading `RagfairOfferHolder.AddOffer` (`:143-181`):**
1. **The holder draws.** Its per-template cap calls `ragfairServerHelper.GetOfferCountByBaseType` — a C# draw — but only after `_fakePlayerOffers.TryGetValue(itemTpl, …)` succeeds. With a clean holder the first offer for a tpl consumes nothing. So every case must remove the offers it added, in a `finally`, or the next case's legacy run interleaves a holder draw the native run does not.
2. **`OfferCounter` is a live instance counter.** The two runs start it at different values, so `intId` can never match. Strip `intId` before comparing, and assert the counter contract separately.

- [ ] **Step 1: Write the fixture skeleton and the per-item parity test.**

```csharp
/// <summary>
/// Golden parity gate on the ragfair port: the same seed must make the legacy 4.1.2 C# path and the
/// spt-native path build byte-equal offers (after LootIdNormalizer) for one item at a time. Whole-pass
/// parity is not promised - legacy fans the assort out over tasks sharing one RNG (:311), so its draw
/// interleaving is nondeterministic even under a fixed seed. Mutates the shared config singleton, the
/// RandomUtil seam and the offer holder, so it restores all of them and never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairParityTests
{
    private static readonly ulong[] _seeds = [42, 1337];

    private RagfairOfferGenerator _ragfairOfferGenerator = default!;
    private RagfairOfferService _ragfairOfferService = default!;
    private RagfairAssortGenerator _ragfairAssortGenerator = default!;
    private RagfairConfig _ragfairConfig = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private TemplateTable _templateTable = default!;
    private ICloner _cloner = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _ragfairOfferGenerator = di.GetService<RagfairOfferGenerator>();
        _ragfairOfferService = di.GetService<RagfairOfferService>();
        _ragfairAssortGenerator = di.GetService<RagfairAssortGenerator>();
        _ragfairConfig = di.GetService<RagfairConfig>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();
        _templateTable = di.GetService<TemplateTable>();
        _cloner = di.GetService<ICloner>();

        di.GetService<SaveServer>().CreateProfile(new Info { ProfileId = new MongoId() });
    }

    // One tpl per item class the spec names (§5.1). Resolve each against the live database in
    // BuildItem rather than hard-coding a tree, so the fixture tracks the shipped data.
    private static readonly string[] _itemClasses =
    [
        "weapon-default-preset",
        "armor-with-removable-plates",
        "ammo",
        "plain-barter-eligible",
        "pack-eligible",
        "money",
    ];

    [Test]
    public void TheSameSeedGeneratesEquivalentOffersOnBothPaths(
        [ValueSource(nameof(_itemClasses))] string itemClass,
        [ValueSource(nameof(_seeds))] ulong seed
    )
    {
        var item = BuildItem(itemClass);

        var native = Generate(item, seed, forceLegacy: false, LootGenerationPath.Native);
        var legacy = Generate(item, seed, forceLegacy: true, LootGenerationPath.Legacy);

        LootJsonAssert.AssertEqual(legacy, native, $"itemClass={itemClass}", seed);
    }

    /// <summary>
    /// Without this the parity cases could pass by both paths producing something seed-independent.
    /// </summary>
    [Test]
    public void ADifferentSeedProducesADifferentOffer()
    {
        var item = BuildItem("weapon-default-preset");

        var atSeed = Generate(item, 42, forceLegacy: false, LootGenerationPath.Native);
        var atSeedPlusOne = Generate(item, 43, forceLegacy: false, LootGenerationPath.Native);

        Assert.That(atSeedPlusOne, Is.Not.EqualTo(atSeed), "seed+1 produced an identical offer, so the seed is not reaching the draws");
    }
```

- [ ] **Step 2: Write the `Generate` helper** — this is where every hazard above is handled:

```csharp
    /// <summary>
    /// One regeneration pass over exactly one item, on one path, returning the normalized JSON of
    /// the offers it added to the holder. The expired-offer entry point is what makes the pass
    /// single-item: the assort walk is bypassed entirely (RagfairOfferGenerator.cs:299).
    /// </summary>
    private string Generate(List<Item> itemWithChildren, ulong seed, bool forceLegacy, LootGenerationPath expected)
    {
        var originalForce = _ragfairConfig.ForceLegacyRagfairGeneration;
        var originalSource = _randomUtil.RandomSource;
        var originalProbabilitySource = ProbabilityRandomSource.Current;
        var idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
        List<MongoId> addedIds = [];

        try
        {
            _ragfairConfig.ForceLegacyRagfairGeneration = forceLegacy;
            if (forceLegacy)
            {
                // One instance in both seams: one shared draw stream, mirroring the single
                // thread-local the Rust side installs for testSeed.
                var seeded = new SeededRandomSource(seed);
                _randomUtil.RandomSource = seeded;
                ProbabilityRandomSource.Current = seeded;
            }
            else
            {
                _ragfairOfferGenerator.NativeTestSeed = seed;
            }

            _ragfairOfferGenerator.GenerateDynamicOffers([_cloner.Clone(itemWithChildren)]);

            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(expected), $"generation did not take the {expected} path");

            var added = _ragfairOfferService.GetOffers().Where(offer => !idsBefore.Contains(offer.Id)).ToList();
            addedIds = added.Select(offer => offer.Id).ToList();

            // An expired-offer pass is a like-for-like replacement: exactly one offer, always
            Assert.That(added, Has.Count.EqualTo(1), $"{expected} path produced {added.Count} offers, expected 1");

            // intId is a live per-instance counter, so it differs between the two runs by
            // construction; the counter contract is asserted by TheOfferCounterAdvancesPerOffer
            var json = _jsonUtil.Serialize(added)!;

            return LootIdNormalizer.Normalize(RemoveInternalIds(json));
        }
        finally
        {
            foreach (var id in addedIds)
            {
                // Leaving these behind would make the next case's holder draw a per-template cap,
                // which only one of the two paths would spend a seeded draw on
                _ragfairOfferService.RemoveOfferById(id);
            }

            _ragfairConfig.ForceLegacyRagfairGeneration = originalForce;
            _randomUtil.RandomSource = originalSource;
            ProbabilityRandomSource.Current = originalProbabilitySource;
            _ragfairOfferGenerator.NativeTestSeed = null;
        }
    }

    /// <summary>
    /// Drops every "intId" member. RagfairOfferGenerator.OfferCounter is process state, not
    /// generated content - both paths increment it identically, they just start from different values.
    /// </summary>
    private static string RemoveInternalIds(string json)
    {
        var array = JsonNode.Parse(json)!.AsArray();
        foreach (var offer in array)
        {
            offer!.AsObject().Remove("intId");
        }

        return array.ToJsonString();
    }
```

- [ ] **Step 3: Write `BuildItem`** — resolve one real tpl per item class off the live database rather than hard-coding, so the fixture survives data updates:

```csharp
    /// <summary>
    /// One assort-shaped item-with-children per class, built the way RagfairAssortGenerator would
    /// (:126-141 for plain items, :61-80 for presets) so the expired path sees exactly what a real
    /// regeneration pass hands it.
    /// </summary>
    private List<Item> BuildItem(string itemClass)
    {
        if (itemClass == "weapon-default-preset")
        {
            var preset = DI.GetInstance().GetService<PresetHelper>().GetDefaultPresets().Values.First(candidate =>
                DI.GetInstance().GetService<ItemHelper>().IsOfBaseclass(candidate.Items[0].Template, BaseClasses.WEAPON)
            );
            var clone = _cloner.Clone(preset.Items).ReplaceIDs().ToList();
            clone.RemapRootItemId();
            clone[0].ParentId = "hideout";
            clone[0].SlotId = "hideout";
            clone[0].Upd = new Upd
            {
                StackObjectsCount = 99999999,
                UnlimitedCount = true,
                SptPresetId = preset.Id,
            };

            return clone;
        }

        var tpl = itemClass switch
        {
            // Resolved by predicate, not by literal, so a data change cannot silently make a case vacuous
            "armor-with-removable-plates" => FirstTplWhere(template =>
                DI.GetInstance().GetService<ItemHelper>().ArmorItemHasRemovablePlateSlots(template.Id)
            ),
            "ammo" => FirstTplWhere(template => template.Parent == BaseClasses.AMMO),
            "pack-eligible" => FirstTplWhere(template =>
                _ragfairConfig.Dynamic.Pack.ItemTypeWhitelist.Contains(template.Parent)
            ),
            "money" => Money.DOLLARS,
            "plain-barter-eligible" => FirstTplWhere(template =>
                template.Parent == BaseClasses.BARTER_ITEM && _templateTable.Prices.ContainsKey(template.Id)
            ),
            _ => throw new ArgumentOutOfRangeException(nameof(itemClass), itemClass, "no case defined"),
        };

        return
        [
            new Item
            {
                Id = tpl,
                Template = tpl,
                ParentId = "hideout",
                SlotId = "hideout",
                Upd = new Upd { StackObjectsCount = 99999999, UnlimitedCount = true },
            },
        ];
    }

    private MongoId FirstTplWhere(Func<TemplateItem, bool> predicate)
    {
        var match = _templateTable.Items.Values.FirstOrDefault(template =>
            string.Equals(template.Type, "Item", StringComparison.OrdinalIgnoreCase) && template.Properties is not null && predicate(template)
        );

        Assert.That(match, Is.Not.Null, "no template in the live database matches this item class");

        return match!.Id;
    }
```

- [ ] **Step 4: Write the whole-pass structural test** (spec §5.3), which needs no legacy oracle:

```csharp
    /// <summary>
    /// Whole-pass parity is impossible (legacy's task fan-out is nondeterministic), so a full native
    /// pass is checked structurally instead: every offer well-formed, counts inside the configured
    /// bounds, end times in range.
    /// </summary>
    [Test]
    public void AFullNativePassProducesWellFormedOffersWithinTheConfiguredBounds()
    {
        var idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
        var now = DI.GetInstance().GetService<TimeUtil>().GetTimeStamp();
        List<MongoId> addedIds = [];

        try
        {
            _ragfairOfferGenerator.NativeTestSeed = 42;
            _ragfairOfferGenerator.GenerateDynamicOffers();

            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));

            var added = _ragfairOfferService.GetOffers().Where(offer => !idsBefore.Contains(offer.Id)).ToList();
            addedIds = added.Select(offer => offer.Id).ToList();

            Assert.That(added, Is.Not.Empty);
            Assert.Multiple(() =>
            {
                foreach (var offer in added)
                {
                    Assert.That(offer.Items, Is.Not.Empty, $"offer {offer.Id} has no items");
                    Assert.That(offer.Root, Is.EqualTo(offer.Items![0].Id), $"offer {offer.Id} root does not match its first item");
                    Assert.That(offer.Requirements, Is.Not.Empty, $"offer {offer.Id} has an empty barter scheme");
                    Assert.That(offer.SummaryCost, Is.GreaterThan(0), $"offer {offer.Id} is free");
                    Assert.That(offer.Quantity, Is.GreaterThan(0), $"offer {offer.Id} has no quantity");
                    Assert.That(
                        offer.EndTime,
                        Is.InRange(now + _ragfairConfig.Dynamic.EndTimeSeconds.Min, now + _ragfairConfig.Dynamic.EndTimeSeconds.Max + 1),
                        $"offer {offer.Id} expires outside the configured window"
                    );

                    // Every child's parent resolves inside its own offer
                    var ids = offer.Items!.Select(item => item.Id).ToHashSet();
                    foreach (var child in offer.Items!.Skip(1))
                    {
                        Assert.That(ids, Does.Contain(new MongoId(child.ParentId!)), $"offer {offer.Id} has an orphaned child");
                    }
                }
            });

            // The holder caps at GetOfferCountByBaseType per tpl, so the surviving count per tpl can
            // never exceed the configured max for that parent
            foreach (var group in added.GroupBy(offer => offer.Items![0].Template))
            {
                var parent = _templateTable.Items[group.Key].Parent;
                var bounds = _ragfairConfig.Dynamic.OfferItemCount.GetValueOrDefault(
                    parent.ToString(),
                    _ragfairConfig.Dynamic.OfferItemCount["default"]
                );

                Assert.That(group.Count(), Is.LessThanOrEqualTo(bounds.Max), $"tpl {group.Key} exceeded its configured offer cap");
            }
        }
        finally
        {
            foreach (var id in addedIds)
            {
                _ragfairOfferService.RemoveOfferById(id);
            }

            _ragfairOfferGenerator.NativeTestSeed = null;
        }
    }
```

- [ ] **Step 5: Write the offer-counter test:**

```csharp
    /// <summary>
    /// The native path numbers offers from the generator's live OfferCounter and advances it by the
    /// number it created - the same contract CreateOffer:140 has on the legacy path.
    /// </summary>
    [Test]
    public void TheOfferCounterAdvancesPerOfferOnTheNativePath()
    {
        var counter = typeof(RagfairOfferGenerator).GetField(
            "OfferCounter",
            BindingFlags.Instance | BindingFlags.NonPublic
        )!;
        var before = (int)counter.GetValue(_ragfairOfferGenerator)!;
        var idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
        List<MongoId> addedIds = [];

        try
        {
            _ragfairOfferGenerator.NativeTestSeed = 42;
            _ragfairOfferGenerator.GenerateDynamicOffers([BuildItem("ammo")]);

            var added = _ragfairOfferService.GetOffers().Where(offer => !idsBefore.Contains(offer.Id)).ToList();
            addedIds = added.Select(offer => offer.Id).ToList();

            Assert.That(added[0].InternalId, Is.EqualTo(before));
            Assert.That((int)counter.GetValue(_ragfairOfferGenerator)!, Is.EqualTo(before + added.Count));
        }
        finally
        {
            foreach (var id in addedIds)
            {
                _ragfairOfferService.RemoveOfferById(id);
            }

            _ragfairOfferGenerator.NativeTestSeed = null;
        }
    }
}
```

- [ ] **Step 6: Run** — `dotnet test --filter "FullyQualifiedName~RagfairParityTests"` → PASS. Any mismatch is a porting bug, not a test bug: normalize both JSONs, diff, find the first diverging field, walk back to the draw that produced it, and fix in Rust with Tasks 6-8's quirk tables as the checklist. The most likely first failures, in order of likelihood: the `Max.Min` double-read (Task 6), the pack short-circuit skipping a draw (Task 8), the barter fall-through consuming a second price draw (Task 7), and the five-draw user block order (Task 7).
- [ ] **Step 7: Full suite** — `dotnet test` green.
- [ ] **Step 8: csharpier + commit** — `test: seeded per-item parity coverage for native ragfair offer generation`

### Task 14: dispatch, hook-liveness and replay tests

**Files:**
- Create: `Testing/UnitTests/Tests/Generators/RagfairPathDispatchTests.cs`, `Testing/UnitTests/Tests/Generators/RagfairHookLivenessTests.cs`

**Interfaces:**
- Consumes: `LastPathTaken`, `ForceLegacyRagfairGeneration`, `_hookableMembers` (via reflection).

- [ ] **Step 1: Write `RagfairPathDispatchTests.cs`** — `[TestFixture] [NonParallelizable]`. Both fixtures in this task open with this setup and these two private helpers; write them out in each file rather than sharing, so each fixture reads on its own:

```csharp
    private RagfairOfferGenerator _ragfairOfferGenerator = default!;
    private RagfairOfferService _ragfairOfferService = default!;
    private RagfairConfig _ragfairConfig = default!;
    private TemplateTable _templateTable = default!;

    private HashSet<MongoId> _idsBefore = [];

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _ragfairOfferGenerator = di.GetService<RagfairOfferGenerator>();
        _ragfairOfferService = di.GetService<RagfairOfferService>();
        _ragfairConfig = di.GetService<RagfairConfig>();
        _templateTable = di.GetService<TemplateTable>();

        di.GetService<SaveServer>().CreateProfile(new Info { ProfileId = new MongoId() });
    }

    [SetUp]
    public void SetUp()
    {
        _idsBefore = _ragfairOfferService.GetOffers().Select(offer => offer.Id).ToHashSet();
    }

    /// <summary>
    /// Offers left behind would make the next case's holder spend a per-template cap draw
    /// (RagfairOfferHolder.cs:153-163) that only one of the two paths pays for.
    /// </summary>
    private void PurgeAddedOffers()
    {
        foreach (var offer in _ragfairOfferService.GetOffers().Where(offer => !_idsBefore.Contains(offer.Id)).ToList())
        {
            _ragfairOfferService.RemoveOfferById(offer.Id);
        }
    }

    /// <summary>
    /// One assort-shaped row, exactly as RagfairAssortGenerator.CreateRagfairAssortRootItem builds
    /// it (:126-141) - id and tpl deliberately identical.
    /// </summary>
    private List<Item> BuildSingleItem()
    {
        var tpl = _templateTable
            .Items.Values.First(template =>
                string.Equals(template.Type, "Item", StringComparison.OrdinalIgnoreCase)
                && template.Properties?.CanSellOnRagfair == true
                && _templateTable.Prices.ContainsKey(template.Id)
            )
            .Id;

        return
        [
            new Item
            {
                Id = tpl,
                Template = tpl,
                ParentId = "hideout",
                SlotId = "hideout",
                Upd = new Upd { StackObjectsCount = 99999999, UnlimitedCount = true },
            },
        ];
    }
```

  Every case below calls `_ragfairOfferGenerator.GenerateDynamicOffers([BuildSingleItem()])` unless it says otherwise, and calls `PurgeAddedOffers()` in a `finally`. Cases:
  - **(a) negative control**: default config, no patches → `LastPathTaken == Native` and at least one offer added.
  - **(b) force flag**: `ForceLegacyRagfairGeneration = true` → `Legacy`, restored in `finally`.
  - **(c-e) TypePriority subclasses**, one test each: construct a `RagfairOfferGenerator` by hand from the container's own services, substituting a trivial subclass for one collaborator at a time —

```csharp
    private sealed class TestRagfairPriceServiceSubclass(
        ISptLogger<RagfairPriceService> logger,
        TemplateTable templateTable,
        HideoutTable hideoutTable,
        RandomUtil randomUtil,
        HandbookHelper handbookHelper,
        TraderHelper traderHelper,
        PresetHelper presetHelper,
        ItemHelper itemHelper,
        ServerLocalisationService serverLocalisationService,
        RagfairConfig ragfairConfig
    )
        : RagfairPriceService(
            logger,
            templateTable,
            hideoutTable,
            randomUtil,
            handbookHelper,
            traderHelper,
            presetHelper,
            itemHelper,
            serverLocalisationService,
            ragfairConfig
        ) { }
```

    (and the analogous `TestRagfairServerHelperSubclass` / `TestRagfairAssortGeneratorSubclass` — read each collaborator's constructor and mirror it). Each substitution → `Legacy`. A hand-built generator with **stock** services → `Native`, as the negative control for these three (`BotPathDispatchTests.AHandBuiltGeneratorWithStockServicesTakesTheNativePath` is the precedent). If a collaborator cannot be constructed by hand, build an isolated `ServiceCollection` per the `ModCompatibilityTests` recipe instead.
  - **(f) the replay test** (spec §6):

```csharp
    /// <summary>
    /// The one database write this port replays: IsItemValidRagfairItem flags a custom-blacklisted
    /// template as unsellable by players (RagfairServerHelper.cs:61). That happens inside Rust, so
    /// the native path has to write it back.
    /// </summary>
    [Test]
    public void ACustomBlacklistedTemplateIsFlaggedUnsellableAfterANativePass()
    {
        var tpl = BuildSingleItem()[0].Template;
        var template = _templateTable.Items[tpl];
        var originalCanSell = template.Properties!.CanSellOnRagfair;
        var originalCustom = _ragfairConfig.Dynamic.Blacklist.Custom;

        try
        {
            _ragfairConfig.Dynamic.Blacklist.Custom = [.. originalCustom, tpl];
            _ragfairOfferGenerator.NativeTestSeed = 42;

            // A full pass, not the expired path: the expired path never runs the validity check (:338)
            _ragfairOfferGenerator.GenerateDynamicOffers();

            Assert.That(_ragfairOfferGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
            Assert.That(template.Properties.CanSellOnRagfair, Is.False, "the CanSellOnRagfair replay did not reach the live template");
        }
        finally
        {
            template.Properties.CanSellOnRagfair = originalCanSell;
            _ragfairConfig.Dynamic.Blacklist.Custom = originalCustom;
            _ragfairOfferGenerator.NativeTestSeed = null;
            PurgeAddedOffers();
        }
    }
```

    `BuildSingleItem()[0].Template` is the tpl the fixture's helper already resolves — a `Type == "Item"` template with `CanSellOnRagfair == true` and a flea price, which is exactly what this case needs to flip. This is the one case that runs a **full** pass rather than an expired-offers pass, because the expired path short-circuits the validity check at `:338`; it is therefore also the slowest case in the fixture.

- [ ] **Step 2: Write `RagfairHookLivenessTests.cs`** — `[TestFixture] [NonParallelizable]`, pattern-copied from `BotHookLivenessTests.cs`. One live Harmony patch per class, each asserted to both fire and flip the path, unpatched in a `finally`:
  - `RagfairOfferGenerator.GenerateFleaOffersForTrader` — a public member of the ported class that the native path never calls; proves the set is not narrowed to the dispatcher's callees.
  - `RagfairPriceService.GetDynamicItemPrice` — a folded collaborator's public member (spec §2.4).
  - `RagfairServerHelper.CalculateDynamicStackCount` — likewise.
  - `RagfairAssortGenerator.GenerateRagfairAssortItems` — likewise.
  - `RagfairOfferGenerator.GetRating` — the dead-but-frozen member (`:244`), which must still flip the path even though nothing calls it.
  - A patch on `GenerateDynamicOffers` **itself** does **not** flip the path, and its prefix/postfix observably fire around the native body.
  Plus the liveness assertions the spec asks for, which fail loudly on a rename regression:

```csharp
    /// <summary>
    /// The hookable set is built by reflection, so a rename or a visibility change would silently
    /// shrink it. Pin its shape instead of its exact size.
    /// </summary>
    [Test]
    public void TheHookableMemberSetCoversAllFourClassesAndExcludesOnlyTheDispatcher()
    {
        var members = (List<MethodBase>)
            typeof(RagfairOfferGenerator)
                .GetField("_hookableMembers", BindingFlags.Static | BindingFlags.NonPublic)!
                .GetValue(null)!;

        Assert.Multiple(() =>
        {
            foreach (
                var type in new[]
                {
                    typeof(RagfairOfferGenerator),
                    typeof(RagfairPriceService),
                    typeof(RagfairServerHelper),
                    typeof(RagfairAssortGenerator),
                }
            )
            {
                Assert.That(members.Any(member => member.DeclaringType == type), $"no hookable members found on {type.Name}");
            }

            Assert.That(
                members.Any(member => member.Name == nameof(RagfairOfferGenerator.GenerateDynamicOffers)),
                Is.False,
                "the dispatcher must not be in its own hookable set"
            );
            Assert.That(members.Any(member => member.IsSpecialName), Is.False, "property accessors leaked into the hookable set");
            Assert.That(
                members.Any(member => member.Name == "GetRating"),
                "the dead-but-frozen GetRating fell out of the hookable set"
            );
        });
    }
```

- [ ] **Step 3: Run** — `dotnet test --filter "FullyQualifiedName~Ragfair"` → PASS.
- [ ] **Step 4: Full suite** — `dotnet test` green.
- [ ] **Step 5: csharpier + commit** — `test: dispatch, hook liveness and CanSellOnRagfair replay coverage for ragfair`

### Task 15: docs and the final gate loop

**Files:**
- Modify: `ARCHITECTURE.md`, `rust/ARCHITECTURE.md`, `RUST-ROADMAP.md`, `BENCHMARK.md`, `todo/TODO.md`

**Check the docs layout before writing.** The architecture docs were being split into per-directory guides while this plan was written (`rust/ARCHITECTURE.md`, `Libraries/ARCHITECTURE.md`, `Libraries/SPTarkov.Server.Core/ARCHITECTURE.md` are new). The split is: root `ARCHITECTURE.md` § *Native Rust layer* owns the **boundary** (what crosses, what dispatches, what a mod can hook); `rust/ARCHITECTURE.md` owns the **crate internals** (the `src/ragfair/` module list and what each file stands in for). Put each half where it belongs, and keep both to the orientation-guide register the surrounding sections use — no wire-level field lists.

- [ ] **Step 1: `ARCHITECTURE.md`** — a new "Ragfair offer generation" subsection inside § *Native Rust layer*, after the bot-generation one, covering:
  - the one-call-per-batch boundary at `GenerateDynamicOffers` and its two callers;
  - what stays C#: the `AddOffer` insert loop and the holder's live per-template cap, the player-offer path (`CreateAndAddFleaOffer`/`CreateOffer`), `GenerateFleaOffersForTrader`, and `RagfairPriceService` as a class for its eight external callers;
  - the dispatch conditions, including that the hookable set spans **four** classes and that a patch on any member except the dispatcher flips to legacy;
  - `RagfairConfig.ForceLegacyRagfairGeneration` (no constructor change — `RagfairConfig` was already injected);
  - the one replay (`rejectedCanSellTemplates` → `templateTable`) and the `OfferCounter` advance;
  - spec §8's mod-facing limitations **verbatim**: patches on `RandomUtil`/`ItemHelper`/`HandbookHelper`/`PresetHelper`/`PaymentHelper`/`BotHelper`/`WeightedRandomHelper`/`ICloner` do not reach the native path; native generates the whole batch before insertion where legacy interleaves; runtime config/price/blacklist mutations stay visible because projection is per call; the `AllowedFleaPriceItemsForBarter` cache quirk is reproduced *per call*, making the native path fresher than legacy for runtime-added items;
  - the two sanctioned divergences this plan added: **no task fan-out** (sequential assort walk) and **one timestamp per batch** rather than one per offer;
  - the benchmark numbers from Task 12, and the items-view-cache follow-up if they are poor.
- [ ] **Step 1b: `rust/ARCHITECTURE.md`** — add `src/ragfair/` to § *Layout* with its six files and the C# file each stands in for, update the line-count and file-count figures in the opening paragraph, and note the two crate-internal facts a reader of that file needs: the walk is sequential where C# fans out over tasks, and `GetFleaPricesAsArray`'s per-instance C# cache is re-derived per call here.
- [ ] **Step 2: `RUST-ROADMAP.md`** — add the ragfair row to the § Working table (`RagfairOfferGenerator.GenerateDynamicOffers` / `spt_generate_dynamic_offers`), update the § Status paragraph's export count (ten → eleven) and its "loot family and bot family" sentence, add any regression found in Task 12/13 to § Broken / known divergences, add the ragfair collaborator list to the existing "Patches on collaborators do not reach the native path" bullet, add an § Exceptions in force bullet for the sequential-walk and single-timestamp divergences, and strike roadmap item 2 (ragfair) — promoting the shared items-view cache to the head of the list.
- [ ] **Step 3: `todo/TODO.md`** — mark candidate #4 done, strike-through plus a pointer to the spec date, matching #1-#3's format.
- [ ] **Step 4: `BENCHMARK.md`** — confirm Task 12's section is present and its numbers are the final ones; add the ragfair invocation to § Running them.
- [ ] **Step 5: Final gate loop** (spec §9) —

```bash
dotnet build server-csharp.slnx -c Release
../mpex-api-compat/ci/check-api-compat.sh   # six assemblies, zero breaking changes;
                                            # the only public addition is RagfairConfig.ForceLegacyRagfairGeneration
dotnet test
cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings
csharpier format .
graphify update .
```

  Expected: all clean. An apicompat finding other than the flag means something in the dispatch cutover changed a frozen member — revert it, do not baseline it.
- [ ] **Step 6: Commit** — `docs: record the native ragfair offer generation boundary and its limitations`

---

## Self-review notes (already applied)

- **Spec coverage:** §1 boundary → Tasks 8/11 (and the explicit "stays C#" list is Task 11's dispatcher body + Task 15's docs); §2 dispatch, all four conditions and the frozen surface → Task 11, seams → Tasks 11/13/14; §3 payload input → Tasks 1/10 (items view reuse, four preset maps, whole-table flea and handbook maps, resolved trader-price map, `dynamic` block, `expiredOffers`, timestamp, nullable test seed), output → Tasks 1/8/11 (offers, `rejectedCanSellTemplates`, diagnostics, C# replay, C# `AddOffer` loop); §4 Rust modules → Tasks 1/3/4/5/6/7/8, new primitives → Task 2, no task fan-out → Task 8, bug-for-bug quirks → the mapping tables in Tasks 3-8 (every quirk the spec names appears in exactly one table); §5 parity promises → Task 13 (per-item byte parity, non-vacuity, whole-pass structural) and Task 2 (primitive parity); §6 testing → Tasks 10 (wire), 13 (parity), 14 (dispatch, hook liveness, replay), plus per-task Rust tests; §7 performance → Task 12, which explicitly gates Tasks 13-15; §8 limitations → Task 15; §9 gate → Task 15 plus the per-task gates; §10 phasing → task order, with the wire tests folded into Task 10 (the bot plan's precedent) and the benchmark placed immediately after the cutover as §7 demands.
- **`get_item_quality_modifier` decision (spec §4's open question):** **in scope, ported in Task 2.** `RagfairPriceService.GetDynamicItemPrice:344-348` calls it whenever `item is not null`, and the sole caller on this path (`GetDynamicOfferPriceForOffer:272`) always passes an item — so every priced offer reaches it. It is *not* an RNG primitive (`ItemHelper.cs:582-646` is pure), so its twin is a value-parity test over hand-built `Upd` shapes rather than a seeded KAT, and it lives in `loot::item_helper` next to the other `ItemHelper` ports rather than in `random_util`.
- **`probability_object_array` is not reused.** Spec §4 lists it among the modules to reuse, but no ragfair code path constructs a `ProbabilityObjectArray`: the only weighted draw is `WeightedRandomHelper.GetWeightedValue` in `GetDynamicOfferCurrency` (`RagfairServerHelper.cs:177`), which maps to the already-generic `loot::random_util::get_weighted_value`. Recorded here rather than silently dropped; the `ProbabilityRandomSource` seam is still swapped in the parity tests because it and `RandomUtil.RandomSource` must hold the same instance.
- **`presets_by_id` collapsed, two maps added.** The bot envelope sends `itemPresets` and `presetsById` as the same dictionary twice; this envelope sends it once as `itemPresets`. Ragfair needs two maps the bot port did not: `defaultPresets` (the ordered `Values` list the assort walk enumerates) and `presetsByTpl` (the fallback arm of `GetWeaponPreset`).
- **"Used top-level `RagfairConfig` fields" is the empty set.** Spec §3 asks for `ragfairConfig.Dynamic` "plus the used top-level fields". Tracing every read in the ported call tree, the only top-level members touched are `Traders` and `RunIntervalSeconds`, both in code that stays C# (`RagfairServer`). The envelope therefore carries `dynamic` alone, and the new `ForceLegacyRagfairGeneration` never crosses.
- **Parity vehicle resolved to the expired-offers entry point** — reasoning in Task 13's preamble, with the four branches that entry point skips explicitly reassigned to Tasks 5-8 (Rust module tests), Task 13's structural case and Task 14's replay test.
- **Two hazards the spec did not name, both handled in Task 13:** `RagfairOfferHolder.AddOffer:153-163` spends a C# draw on the per-template cap once a tpl already has offers, so every case purges the holder in a `finally`; and `OfferCounter` is live instance state, so `intId` is stripped before comparison and asserted separately.
- **Type consistency check done:** `RagfairContext` (Task 3) is the ctx type used in Tasks 4-8; `RagfairOfferWire`/`RagfairOfferUserWire`/`OfferRequirementWire` (Task 1) are produced in Task 7, assembled in Task 8, mirrored in C# in Task 10 and mapped to the frozen `RagfairOffer` by `ToRagfairOffer` in Tasks 10/11; `DynamicConfigWire` (Task 1) is read by Tasks 3-8; `CreateFleaOfferDetails`/`BarterScheme` (Task 7) are consumed by Task 8; `GenerateDynamicOffersRequest`/`DynamicOffersResult` name the same types on both sides of the FFI in Tasks 1, 9 and 10; `_hookableMembers` (Task 11) is the field name Task 14 reflects on.
- **Task ordering caveat:** Task 9's C# half references the records Task 10 declares. Executed strictly in order, Task 9's `dotnet build` fails until Task 10 lands. Either swap the two tasks or land Task 9's Rust half and ABI bump first and its C# half with Task 10 — Task 9 Step 3 says so inline.
