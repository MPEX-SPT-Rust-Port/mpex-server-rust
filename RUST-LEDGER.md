# Rust port — history ledger

The append-only record of every state-ownership flip, phase and port PR: what was decided, what was
declined, and why. Live status, rules and divergences are [RUST-ROADMAP.md](RUST-ROADMAP.md);
measurements are [BENCHMARK.md](BENCHMARK.md).

Entries are historical — written as of landing and not maintained afterward. **Decision numbers are
stable**: cross-references elsewhere in the tree cite them. Line numbers inside entries (`Foo.cs:123`)
are as-of-landing and may have drifted; the symbol names beside them are the durable reference.

## Flip and phase ledgers

Each flip and phase landed with its own plan, ABI bump, goldens passing unchanged, and BENCHMARK.md
re-measured before the next started.

**Flip #1 — ragfair (ABI 22).**
- Freshness: legacy's hydrate-once caches (`TraderHelper` trader prices, `HandbookHelper` price
  lookup, `PresetHelper` preset store and default-preset maps) could serve stale values into a
  rebuilt slice; Rust re-derives every view per publish, so the resident path is uniformly *fresher*.
  The practical edge: a resident send and a `viewsOverride` send can diverge after a runtime
  mutation, because the override is still built through those caches.
- (b) pmc name lists stay C#-projected in the varying block. Flip #6 was the named revisit point and
  declined it (decision 1); Phase 4 declined it again (decision 10).
- (c) Runtime *config* edits bypassed the stamp — **closed by Phase 4** for scalar edits. Collection
  edits remain, as RUST-ROADMAP.md § *Broken*'s container bullet.
- (d) An `_items: []` preset added at runtime now aborts the publish loudly, naming the preset
  (`views.rs`'s `build_preset_cache`), where the old slice path tolerated it. Stricter, deliberately.
- `Native/` delta: +214/−70 across 7 files. `Db/` (`DbPublisher` + `DbPayloadProjection`, 105 lines)
  is new shared infrastructure every later flip reuses.

**Flip #2 — repeatable quests (ABI 23).**
- Freshness: pre-flip the quest slice was rebuilt from live tables on every send that carried it;
  post-flip an *un-stamped* table mutation is invisible until the next stamped publish.
- The quest views share `items`/`handbookPrices`/`fleaPrices` with the ragfair views through one
  `Arc`; `defaultWeaponPresets`, `defaultPresetOrItemPrices`, the repeatableQuests lifts and the two
  location maps are quest-own derivations at publish.
- The `locations` root is `Base` + `AllExtracts` only, keyed by the locations' `JsonPropertyName`
  strings (e.g. `factory4_day`) and domain-bounded by `LocationTable.GetDictionary()`; a null
  `AllExtracts` ships as `[]`.
- `Native/` delta: +175/−103 across 5 files.

**Flip #3 — base-class hydrate + linked-item table (ABI 24).**
- Freshness: an eligible hydrate — including `GetLinkedItems`' rebuild on a cache miss mid-run
  (`RagfairLinkedItemService.cs:126-133`) — reads last-published state. Override sends still project
  live tables per call.
- The walk-input equivalence handshake (`OneShotViewsEquivalenceTests` / `flip3_oneshot_views.rs`)
  ran green over the full real database: 4,553 base-class chains and 4,673 linked-item sets identical.
- `Native/` delta: +207/−1 across 5 files — growth, because the one-shots' whole pre-flip payload
  *was* the projection, which survives as the ineligible arm.

**Flip #4 — loot, six exports (ABI 25).**
- Freshness: statics refresh on publish, not per call, so a transformer registered *after* the last
  stamped publish (registering one bumps no stamp) is invisible to eligible sends until the next bump.
  The kill switches cover it.
- looseLoot stays per-call on both arms — 549 MiB RSS on top of the measured 405.2 MiB publish delta,
  for a payload read once per raid start that already rides a zero-copy `WriteRawValue` splice.
  Residency was deferred to Phase 3, where it was declined outright on the same number.
  `staticAmmoDist` is permanently varying: it is a parameter of the frozen public signatures.
  `GetDefaultPresetsByTplKey`'s duplicate-first-item-tpl case now aborts the publish loudly naming the
  culprit preset, where pre-flip C# threw `ArgumentException` per forced-loot call. One saving landed
  as a side effect: sealed's resident arm no longer builds `presetsByTpl` at all.
- `Native/` delta: +201/−51 across 5 files.

**Flip #5 — scav case (ABI 26).**
- Freshness, a direction flip: pre-flip the native scav case re-derived item and price pools per call,
  so it was *fresher than legacy* for runtime-added items; post-flip the eligible path is
  fresh-per-publish. Presets moved the same way flip #4's did.
- The `hideout` root carries `production.scavRecipes` only, at the real table path's
  `JsonPropertyName`s. No view derives from it at publish — the recipe views derive at request time,
  preserving the C# skip-a-recipe-missing-`endProducts`-or-a-band semantics bug-for-bug, where a
  publish-time derive would have to abort loudly. The raw root pins the capitalized
  `Common`/`Rare`/`Superrare` wire names (`HideoutProduction.cs`); the request-time derivation maps
  them onto the existing lowercase `ScavRecipeView` — zero generator-algorithm change.
- Eligibility + branch + stale-retry mirror `LootGenerator` exactly. The pre-flip hydration sweep
  found readers only and no lazy writer into `Production.ScavRecipes`, so no `DbPublisher` pre-touch
  carve-out was needed.
- `Native/` delta: +110/−30 across 5 files.

**Flip #6 — bots, both exports (ABI 27).** Closed Phase 1.
- Freshness: the database half — items view, `itemPresets`, `defaultPresetIdsByTpl`, exp table — is
  last-published state. Handbook prices carry one mod-only edge: the eligible arm reads
  `RagfairDbViews.handbookPrices`, keyed off the **items table**, where the override arm calls
  `HandbookHelper.GetTemplatePrice` per drawn tpl — so a tpl priced in the handbook but absent from
  the items table prices at its handbook value on the override arm and at 0 on the resident one.
  Unreachable for generatable loot.
- **1, no bots root.** Its only consumer would be the pmc name lists: the bot family reads no bot
  templates resident, so the root would carry 5.7 MiB and ~94.6 ms of warm projection on every
  publish (~13% of the measured ~735 ms) to serve two name lists. And
  `GatherPmcNamesOfLength` filters on a config value, so the derivation could not go resident before
  Phase 4 anyway. Phase 4 declined it a second time (decision 10).
- **2, `modPoolSlotOrder` is not a view.** The plan had it deriving into `BotDbViews`; Task 5's
  field-for-field resident-vs-override identity test caught the divergence, and the root cause is not
  port drift — the C# order is the live `BotEquipmentModPoolService`'s `ConcurrentDictionary`
  enumeration order, process-local and not a function of the database. The Rust derivation was
  deleted and the field moved into `SharedBotVarying` at 26,428 bytes per send, then left the wire
  entirely at ABI 32 when Rust took the pools — the member was deleted rather than re-homed.
- **3, `BotDbViews` as built**: `{ragfair: Arc<RagfairDbViews>, defaultPresetIdsByTpl, expTable}`.
  The two bot-own members are a re-key of `defaultPresetsByTpl` to each preset's own id
  (`ToDefaultPresetIds`) and `globals.config.exp.level.expTable[].exp`, lifted out of
  `BotWaveBatcher`. The derivation is total and `Result`-shaped so a future hard failure aborts the
  publish.
- **4, the handbook-price union stays the override arm's shape.** `BuildViewsOverride` prices the
  union of every loot pool the send can draw from (one cache single-bot, one per level band batched)
  rather than the whole handbook — collision-safe, since a tpl in two pools resolves to the same
  `GetTemplatePrice`. The eligible arm reads the resident items-keyed map and needs no union.
- **5, one envelope for both exports**, one `resolve_bot_views` resolver returning `LootEpochError`,
  so `STATUS_STALE_EPOCH` and the self-heal behave identically on both. `SharedBotViewsWire` was
  renamed `SharedBotVaryingWire`.
- **6, the two-arm dispatch block stays copied — evaluated and declined.** The fifth-copy rule targets
  *identical* blocks; across the 11 sites the block takes ~6 distinct shapes (per-export
  `ViewsOverride` expressions, a `bool viewsOverride` parameter, a `.Result` unwrap, early-return
  one-shots with no varying block, bots' mutate-the-request form). A shared helper would be generic
  over the request type, take two builder closures and a delegate for each site's private-set
  `LastSendIncludedViewsOverride` — a 5-parameter abstraction replacing a 12-line `if/else` whose only
  duplicated line is the flag assignment. Commit 1 extracted the part that *was* identical into
  `ResidentDbDispatch`. Revisit only if a future flip makes the arms converge.
- `Native/` delta: **+227/−285 across 7 files** — the program's **first genuine shrink**.
  `BotPayloads.cs` alone is +88/−151 (wire types collapsed onto the shared envelope) and the seven
  copied dispatch blocks became `Native/Db/ResidentDbDispatch` (38 lines). Both flip-#5 review
  carryovers are discharged here; the stale-epoch retry now has a scav case self-heal test
  (`ScavCaseResidentDbTests`).

**Phase 2 — write barriers (no ABI bump).** The `DatabaseMutationStamp` is moved primarily by
Ceciler-injected barriers (`Patches/Ceciler.WriteBarriers`): every non-`init` property setter
reachable from the published roots — the five tables plus, since Phase 4, the 28 configs, so 33
roots — walking property types *and* base types (`TradersTable` declares nothing and reaches
`Trader` only through its `Dictionary<MongoId, Trader>` base). The full invisible-writes surface
(the roadmap's container bullet carries the compressed form):

- `Add`/`Remove`/indexer-set on a table or config collection, at root level or one below
  (`trader.Assort.Items`, `handbook.Items`); array element writes; reflection-driven writes.
- The setters of the denied live-per-request types — `Item`, `BotBase`, `PmcDataRepeatableQuest`
  and, since Phase 4, `GenerationData`, reachable through
  `EquipmentFilters.Randomisation[].Generation` and `KarmaLevel.ItemLimits` but written per bot by
  `BotWeaponGenerator` and `PlayerScavGenerator.AdjustItemWeights`. Neither field is read from the
  resident root today, but unread is not un-stale.
- The setters of open-generic model types, never barriered by design — `MinMax<T>`'s three, so a
  mod editing a location's `Limit`/`MinMaxBot` bands writes nothing the stamp sees, and since
  Phase 4 the same hole reaches config bands read resident (`ScavCaseConfig`'s
  `RewardItemValueRangeRub`, `AmmoRewards.AmmoRewardValueRangeRub` and the per-rarity `MinMax<int>`
  counts under `MoneyRewards` — the scav case's whole price and money band set).
- Anything behind an `object?`-typed property the walk cannot follow
  (`TemplateSide.EquipmentBuilds`/`WeaponBuilds`).
- A genuine database write inside a native-response decode callback's extent —
  `SptNative.DecodeResult` holds a `WriteBarrier.Suppress()` scope across the decode, because
  deserializing a response into DB-shaped model types was the real churn source. Nothing does that
  today; `WriteBarrierChurnTests` pins the invariant. The publish's own suppression scope spans
  `DbPayloadProjection`'s `LazyLoad.Value` reads, so a mod-registered transformer writing into a
  published root *other* than the value it transforms is suppressed too (both shipped `StaticLoot`
  transformers write only inside the transformed graph).

Root-level tracking collections were evaluated and declined — 4 of ~90 mutation sites and none of
the post-startup ones on a published root, against 17+ apicompat suppressions and a public
`ProfileFixerService` signature break. Two mod-reachable container writes were closed by hand
instead (`RagfairPriceService.ReplaceFleaBasePrices`, `CustomQuestService.CreateQuest`), each as an
additive constructor overload, bringing hand-written bump sites to eight. The barriers also
over-announce in two correctness-safe places: `UserBuild.Id`/`Name` are reached through
`EquipmentBuild` ancestry from the profile templates (a profile build save dirties the stamp), and
`LocationController`'s `mapBase.Loot = []` reset is a genuine table write costing a full six-root
republish once per map per `client/locations` request — accepted, not mitigated (BENCHMARK.md §
Phase 2). Deferred items are on PR #10 (below).

**Phase 3 — fused database load (ABI 29).** `spt_db_load` fuses the `checks.dat` hash walk with
reading `database/`: one walk hashes (when verifying — Debug ships no `checks.dat`), reads and
installs the five resident roots as epoch 1, then hands the eager file bytes back for `ImporterUtil`'s
reflection walk. `CoreConfig.ForceLegacyDatabaseImport` restores the pure-C# arm.

**Measured, the flip is a regression.** At the importer, **935.7 ms against legacy's 480.6 ms — 0.51x,
+419–455 ms** (BENCHMARK.md § Phase 3), with 202 files / 49.4 MiB of eager content crossing as
buffers. The deliverable is the retired startup double-read, but against a warm page cache that read
was nearly free while the buffer plumbing is not: the buffer-fed walk is 50–68 ms *slower* than the
disk walk it replaces (451.1 vs 383.8 ms), and the fused load costs ~380–391 ms over the bare verify
(484.6 vs 96.8 ms) for buffer retention, the FFI copy and five-root assembly, parse and derivation.

**The startup win is not wired up.** `DbPublisher.EnsureCurrent` still republishes every root
whenever `_currentEpoch` is 0, and nothing feeds `DbLoad`'s installed epoch into it, so both arms pay
a 730–745 ms forced publish. Feeding that epoch through buys less than it looks: `EnsureCurrent`
republishes when `_currentEpoch == 0` **or** `_lastPublishedStamp != stamp` (`DbPublisher.cs:46`),
and on Release the barriers move the stamp during `PostDbLoadService` before that first
`EnsureCurrent` ever runs. *(Corrected by the load-epoch seed follow-up: the mover is exactly one
write, `coreConfig.ServerStartTime` at `PostDbLoadService.cs:53-56`, which precedes the first
`EnsureCurrent` — `HydrateItemBaseClassCache`, five lines down. `AdjustLocationBotValues`
(`PostDbLoadService.cs:627`) bumps the stamp too, through `LocationBase`'s Ceciler-injected setters,
but it runs at `:95`, after that first publish, so it was never in the pre-publish window.)*
**Phase 4 moved the goalposts further:** `spt_db_load` stays `database/`-scoped, so epoch 1
installs five of the six published roots and carries no `configs` root — skipping the first
`EnsureCurrent` on the strength of epoch 1 would leave every config-reading family without one.
*(Also corrected: an absent root is not a resolve failure. A family whose view resolver finds no
configs root answers `STATUS_STALE_EPOCH` — status 4 — which the C# side already self-heals with
`ForcePublish` + one retry, so the cost of getting this wrong would have been a silent extra
republish, not a fault.)*
The follow-up now has to publish configs at load time from the live C# objects, not from
`configs/*.json` (the values-not-keys trap), or keep the first republish. *(Delivered 2026-08-26 —
see the load-epoch seeding ledger below.)*

- Freshness: **none at generation time.** Epoch 1 is boot-validation only, always superseded by the
  first `EnsureCurrent` republish, so no generation path ever reads it. *(Corrected by the
  load-epoch seed follow-up: on a modless boot the first `EnsureCurrent` now consumes the seed
  instead of republishing, so epoch 1 plus the configs-only publish — epoch 2 — is exactly what
  every eligible family reads until the `RagfairCallbacks` settle publish.)*
- **1, loose-loot residency declined.** 549 MiB on top of the measured 405.2 MiB publish RSS delta is
  954.2 MiB, leaving no headroom under the ~1 GB line Phase 0's RSS gate drew. The spec's
  byte-serving export was **not built**. `locations/*/looseLoot.json` and `locales/global/*` are
  classified never-read and stay disk-path `LazyLoad`s on both arms;
  `staticLoot.json`/`staticContainers.json` are read for root assembly but not returned.
- **2, per-file buffer handoff, not per-root.** The C# reflection walk stays and remains the
  file→property mapping authority; Rust owns file→wire for the five resident roots only, keyed
  `database/…` on both sides and consumed inside `DeserializeFileAsync`. Reproducing
  `LoadRecursiveAsync`'s mapping semantics exactly was this phase's named risk, and keeping the C#
  walk neutralizes it structurally. The one duplicated semantic — importer skip lists and lazy
  patterns — fails benign both ways: an extra returned file is ignored, a missing one falls back to a
  disk read.
- **3, epoch-1 assembly is validated by parse + derive + the real-tree integration test**
  (`rust/spt-native/tests/phase3_db_load.rs`), not a C#-envelope equivalence harness. **The hazard
  that hides, for whoever wires the load-time epoch through:** `classify` (`load.rs:74`) and
  `LOCATION_MEMBERS` (`load.rs:18`) are a second, independent file→wire mapping duplicating
  `DbPayloadProjection`, and **nothing gates it** — `DatabaseLoadEquivalenceTests` compares
  `DatabaseTables`, never the resident roots. What makes it safe today is only that epoch 1 is
  superseded by the very republish the follow-up exists to remove. Gate it against a
  `DbPayloadProjection` publish before, not after. *(Delivered with the load-epoch seed:
  `ResidentRootEquivalenceTests` is that gate — always-on, five roots, digest compare over the
  typed lift surface via `spt_db_resident_digest`.)*
- **4, the equivalence golden is permanent.** `DatabaseLoadEquivalenceTests` compares the
  legacy-built and native-built `DatabaseTables` root by root and pins that the fused load returns a
  file under every root it compares (`ImporterUtilPreloadedTests` covers consumption). Plain `[Test]`,
  so it runs in `dotnet test`.
- `Native/` delta: +132/−1 across 2 files, almost all additive — one `[LibraryImport]` entry, the
  `DbLoad` wrapper and the framed-response parser with its three internal DTOs. The single deletion is
  the `ExpectedAbiVersion` constant. The phase's other edits land in `ImporterUtil`, `JsonUtil` and
  `DatabaseImporter`.

**Phase 4 — the configs root (ABI 30).** All 28 loaded configs publish as a sixth root keyed by
`Kind`, and six families read their config data off it. No new exports, no new derived views,
`spt_db_load` untouched.

- Freshness, and it is a real cost. For a *scalar* config write it is a wash — the Ceciler walk covers
  all 28 config types (33 roots), so a property set moves the stamp and the next call republishes. For
  a *collection* mutation it is a straight loss: `Add`/`Remove`/indexer-set on `ItemConfig.Blacklist`,
  `LocationConfig.LooseLootBlacklist`, `BotConfig.ItemSpawnLimits` and their kind was read fresh on
  every send before and is now invisible until some other stamped write lands. Config bodies are
  mostly collections, so the window is wider than the table roots', and these are values a family
  genuinely reads. The kill switches restore per-call freshness; root-level tracking collections
  remain the sanctioned remedy, declined on the same arithmetic as Phase 2.
- **1, wire keys are the `kind` strings** — read from each config's own `Kind` while iterating the
  injected dictionary, not C# type names and not file stems. No reflection needed.
- **2, all 28 publish; Rust lifts only the ten stems it reads** (`spt-item`, `spt-scavcase`,
  `spt-ragfair`, `spt-inventory`, `spt-quest`, `spt-location`, `spt-seasonalevents`, `spt-bot`,
  `spt-pmc`, `spt-repair`); everything else rides the flatten map full-fidelity. The root measured
  free.
- **3, configs arrive by `spt_db_publish` only.** Raw `configs/*.json` bytes are not the live objects
  (C# record defaults, `PostDbLoadService` fixups, mod edits), so assembling a root from disk walks
  into the values-not-keys trap.
- **4, consumption is per-call resident reads** (the scav-case recipe-view precedent): no field joined
  `ResidentDb`'s derived views, no derivation gate changed. One functional exception — ragfair gained
  `customMoneyTpls` off `spt-inventory`, retiring the divergence where offers priced in a mod-added
  currency took the unrounded arm.
- **5, the ineligible arm keeps its cost.** Each family's `viewsOverride` bundle gained the config
  block its varying half used to carry. Measured flat: a single-bot override send is 4,152,813 B
  against the pre-phase 4,208,129 B.
- **6, the barrier extension is 28 root FQNs, not a namespace sweep.** The `_denied` list gained
  `GenerationData` (four denied types now) and gained name validation, so a drifted entry fails the
  build instead of silently barriering nothing.
- **7, `Option<Lift>` is the strictness contract at the stem boundary.** An absent stem is `None` —
  the root parses and the family's per-call resolve fails loudly naming the stem. A present-but-
  malformed stem fails the whole publish parse (`STATUS_BAD_ARGS`) — and not for one call only:
  `DbPublisher.PublishLocked` never reaches `_lastPublishedStamp = stamp` when the publish throws, so
  every later `EnsureCurrent()` re-attempts and throws again, from outside `ResidentDbDispatch.Send`'s
  try, and every eligible native call 500s until the config is fixed. Reachable only through a mod
  nulling a `required` member with trust on. Three lifts deliberately break the rule: `spt-item`'s
  four sets and `spt-inventory`'s `customMoneyTpls` stay `#[serde(default)]` despite being C#
  `required`, and `spt-pmc` parses as the soft `PmcConfigWire`; `phase4_configs_root.rs` pins the soft
  members' wire names.
- **8, caller-selected config stays varying** — quest's `repeatableConfig`, loot's
  `containerSettings`/`rewardDetails`, bots' `levelGeneration`.
- **9, the bot equipment blacklists moved to native selection.** The per-(role, level)
  `FirstOrDefault` over `BotConfig.Equipment[role].Blacklist` became a Rust lookup, pinning the
  deliberate `level ?? 0` divergence between the two lists; selection is not a draw, so it is
  RNG-neutral. Both members left the wire entirely on both arms.
- **10, the pmc name lists stay varying** — `GatherPmcNamesOfLength` still reads the bot *table*,
  which has no root. A names-only mini-root is the standing upgrade if the varying cost ever measures.
  The same answer covers every `SeasonalEventService` / `ItemBlacklistCache` /
  `LootableItemBlacklistCache` / `RagfairLinkedItemService` / `GetMoneyTpls`-backed field: those are
  **service state, not config**, and no phase currently owns them.
- **11, `customMoneyTpls` is the one projection divergence fixed**; every other stays bug-for-bug.
- **12, family order** was scav case → ragfair → quest → reward loot → location loot → bots.

Three rulings amended the plan during execution. **The two loot multipliers stay per-call** —
`RaidTimeAdjustmentService.AdjustLootMultipliers` scales `LocationConfig.StaticLootMultiplier` and
`LooseLootMultiplier` **in place** through the dictionary indexer for a shortened scav raid and puts
them back after generation, so no property setter fires and a resident snapshot would hand the raid
unadjusted PMC-density loot. Both ride the varying block as C#-resolved per-location scalars on both
arms and land unread in `LocationConfigLift.extra`;
`LootResidentDbTests.AnInPlaceLootMultiplierAdjustmentReachesAResidentSend` pins it.
**`BotConfig.Equipment` stays varying** — the phase's largest planned lift, declined for a worse
version of the same reason: `BotInventoryGenerator.ReplayRandomisationClamps` writes the nighttime
mod-chance clamps back into `Equipment[role].Randomisation[band].EquipmentMods` through the indexer
after *every* native single-bot send, and that write is a deliberate cross-bot feedback loop the next
bot's C# prelude reads (`BotEquipmentFilterService.cs:63`). A published copy would freeze at the
on-disk values and diverge from bot 2 of a nighttime raid on. Eleven of twelve planned bot lifts
landed; the twelfth landed later at ABI 34 — the **Equipment split** ledger entry below — which
answered this objection by carrying just the written cell on the wire rather than the whole member.
**`ItemConfigLift.blacklist` is a `HashSet<String>`, not
the plan's `IndexSet`** — the override wire mirrors C#'s `HashSet`, so both arms read one shape and
there is no iteration site to observe an order. Zero `[Test]` bodies, assertions, seeds or normalizers
were edited anywhere in the phase.

`Native/` delta: **+289/−210 across 13 files** — mostly members moving from a varying record into the
family's `viewsOverride` record in the same file, each with a doc line naming its resident equivalent.
`Db/DbPayloadProjection.cs` (+17/−3) is the whole configs-root writer;
`Bot/BotPayloadProjection.cs` (+31/−50) is the only genuine shrink, where the two blacklist
projections were deleted in favour of native selection. The wire did shrink: **−138,121 B per
eligible bot send, a flat 23.0–23.4% at every wave size**, down to 10,272 B per bot at wave 45. The
sixth root cost **+3.1 to +3.8 ms** of cold publish — inside a 719–811 ms per-recipe spread, against
a budgeted ~67.7 ms. What now dominates an eligible bot send is the caller's own `templateVariants`
at **83.2%** of the request.

**Phase 5 — profile persistence (ABI 31).** Four `spt_profile_*` exports own `user/profiles/`' live
listing, reads, writes and deletes. No resident state, no new root, no legacy path and no config
flag: the profiles directory arrives in every request and profile bytes are opaque.

Freshness is unchanged — profile bytes were never resident and every load and save still hits disk
through the same MD5 gate. What moved is *failure* visibility and I/O posture:

- **Mid-write cancellation no longer exists** (decision 7). A started write always completes.
  Atomicity is unaffected either way.
- **I/O failures throw a different type** (decision 8). `ProfileError{BadArgs,Io}` crosses as
  `STATUS_BAD_ARGS`/`STATUS_ERROR` and `DecodeResult` raises `InvalidOperationException`, where
  `FileUtil.WriteFileAsync` and `JsonUtil.DeserializeFromFileAsync` raised `IOException`-family types.
  `RemoveProfile` changed the same way; throw-vs-no-throw and the `bool` return are unchanged — a
  missing file is still `false` and still just logged.
- **Profile I/O is no longer on async file handles.** Both `useAsync: true` sites became a blocking
  syscall on a `Task.Run` threadpool thread. `SaveAsync` and `LoadAsync` loop sequentially, so exactly
  one thread is parked at a time — a property of those loops, not parity, and a future concurrent
  caller would not inherit it.
- **A `default`/empty `MongoId` now throws** (decision 9), where the old body silently probed
  `user/profiles/.json`. Unreachable in-tree, but not for the tidy reason: `LoadProfileAsync` applies
  **no** id check of its own (`SaveServer.cs:198-199`), so for loads the native gate genuinely is the
  first thing an empty id meets and the protection is entirely in the callers — `LoadAsync`
  pre-filters on `MongoId.IsValidMongoId`, `LauncherV2Controller.cs:156` passes a freshly minted id,
  and `CreateProfileService.cs:239-244` receives its id from the session (its first statement,
  `saveServer.GetProfile(sessionId)`, throws on `IsEmpty`). `SaveProfileAsync` never reaches Rust
  with one: `IsProfileInvalidOrUnloadable` returns `false` for an absent key
  (`SaveServer.cs:331-343`), so an empty id passes that guard, takes the save lock, and dies on
  `profiles[sessionID]` (`SaveServer.cs:282`) exactly as before. The `!sessionId.IsEmpty` guard at
  `LauncherV2Controller.cs:95` is on `RemoveProfile`, not the load/save pair; other disk-reaching
  callers are `CreateProfileService.cs:239-244`, `GameCallbacks.cs:70`, `PrestigeController.cs:98`
  and `LocationLifecycleService.cs:500,719`.
- **Profile listing is sorted**, where `Directory.GetFiles` order was filesystem-dependent.
- **UTF-8 BOM handling is now explicit.** The `FileStream` deserialize skipped a BOM for free; the
  `ReadOnlySpan<byte>` overload does not, so `profile.rs::load` strips it (reusing
  `db/load.rs::strip_bom`). Net behaviour is unchanged — that is the point — but the guard is now
  load-bearing code, and deleting it silently sends hand-edited BOM'd profiles down the
  `-corrupt.json` + backup-rollback arm.

One further change landed as its own commit (`e7d3a4b`) ahead of the native swap: **autosave failure
isolation changed shape.** `SaveAsync` now catches per profile (rethrowing on cancellation) and
writes `saveMd5` *after* the write instead of before. Together: a failed write no longer marks that
profile version as persisted, and one unwritable profile no longer aborts the rest of the tick.
Before, the second property held only by accident, through the poisoned hash — shipping the reorder
alone would have converted a per-version loss into an unbounded multi-profile autosave outage, which
is why the two halves are one commit. `SaveAsyncSurvivesOneUnwritableProfile` is the pin.

**Listing semantics, corrected against the plan's own text.** Decision 5 said `backups/`,
`-corrupt.json` and stray `.bak` files "are excluded by the same C# lines that exclude them today" —
right about the files, wrong about the directory. `-corrupt.json` and `.bak` do reach C# and are
dropped by the extension filter and the `MongoId.IsValidMongoId` stem gate. `backups/` is a
**directory**: `profile.rs::list` keeps only entries whose `fs::metadata` says `is_file()`, so it
never reaches C#. The false premise matters because it would later justify "simplifying away" C#
filters that are in fact the only thing excluding the two file cases. `fs::metadata(entry.path())` is
used and not `entry.metadata()` because only the free function follows symlinks — `DirEntry::metadata`
is `lstat` on Unix and would classify a symlink-to-a-profile as neither file nor directory.
Following-then-`is_file()` matches `Directory.GetFiles` on the two cases that matter (measured on
.NET 10.0.10) but is **not exact**; the source is the accurate account.

A **dangling** symlink is still skipped (`list_skips_a_dangling_symlink`) — a real divergence, since
`GetFiles` returns the link, but inert: a dangling `{id}.json` link passes both C# filters, and what
saves us is `load`'s own `NotFound` arm answering `found: false` regardless. A **denied** `stat` is
now raised, and this is why the code changed: `readdir` needs only `+r` while `stat`ping a child needs
`+x`, and .NET answers file-vs-directory from `d_type`, so on a `user/profiles/` that lost `+x`
`GetFiles` returns every profile while every `fs::metadata` fails `EACCES`. Swallowing that reported
an empty directory and `LoadAsync` offered to create a new profile beside intact files.
`list_raises_an_unreadable_entry` pins the fix and self-guards (root bypasses the search bit).
**This is an improvement over the pre-phase C#, not a parity restoration:** `File.Exists` also
returns `false` on `EACCES`, and both `LoadProfileAsync`'s guard and `DeserializeFromFileAsync`'s
short-circuit (`JsonUtil.cs:104-107`) are `File.Exists` — so the pre-phase path enumerated every
profile and then silently loaded none, the same zero-profile presentation one stage later.

Decisions: **1, Rust is stateless and `dir` rides every request** — no module static; residency waits
on the profile-model port (`todo/TODO.md` #19). **2, serialization stays C#** — Rust is a
byte-faithful passthrough (`RawValue` on save, raw frame bytes on load), so on-disk format is
byte-identical and the MD5 dirty-check is unaffected. **3, the MD5 dirty-check and per-session save
locks stay C#.** **4, `BackupService` stays C#**: Rust owns live-file writes, deletes and the
load-time listing; C# keeps the read-only probes, the corrupt-copy, the backup copy loop and the
restore copy. The only writer overlap is restore-during-load, already serialized inside
`LoadProfileAsync`'s recovery arm. **5, all four exports take the standard envelope shape**
(`{"schema":1,"dir":…}`, plus `id` on three); not-found rides the load frame header
(`{"found":false}`) and no new status code was added — every filter stays in C# verbatim, so there is
zero filter-parity risk. **6, no legacy path and no `forceLegacy` flag** (the `SPTLoggerDispatcher`
precedent). **7, cancellation before the native call only.** **8, the error surface is
`ProfileError{BadArgs,Io}`**, message naming the path and the OS error. **9, Rust guards the id** —
24 ASCII hex chars, mirroring `MongoId.IsValidMongoId` (`Extensions/MongoIdExtensions.cs:52-68`).
This is the path-traversal guard at the trust boundary and is non-negotiable even though C# always
passes a typed `MongoId`. **10, the `DbPublisher._currentEpoch == 0` unconditional republish is
declined again** — independent of the profile disk boundary and still blocked on the values-not-keys
mapping gate; the discussion is the load-time-epoch follow-up in the Phase 3 ledger. **11, no
benchmark fixture, but the free number was taken.** **12, plain synchronous `std::fs` on the calling
thread** — single-file ops need no tokio, and C# keeps its async posture through `Task.Run`.

`Native/` + `SaveServer` delta: **+266/−18 across 3 files** — `NativeMethods.cs` +12/−0,
`SptNative.cs` +226/−1 (four wrappers, the `ProfileLoadResult` record, the frame parser; the single
deletion is the ABI constant) and `SaveServer.cs` +28/−17.

**The measurement, because decision 11 pre-committed to it.** `SaveProfileAsync`'s returned
milliseconds on a **26.50 MB synthetic profile**, 6 runs per pass and two passes per state: **~161 ms
median (155–186) before, ~192 ms median (187–217) after — about 20% slower, and the ranges do not
overlap across any of the four passes.** A real regression, recorded as one. The profile is synthetic
and the harness throwaway, so the figure sizes the effect rather than pinning it.

Attribution, and the naive version is wrong: the pre-phase path did **not** stream.
`fileUtil.WriteFileAsync(filePath, jsonProfile, ct)` took the `string` overload
(`Utils/FileUtil.cs:103-107`), one `Encoding.UTF8.GetBytes` into a full-size `byte[]` given to a
single `fs.WriteAsync` — so peak was already `jsonProfile` (UTF-16) plus one full-size UTF-8 buffer,
and the `MemoryStream` **replaces** that buffer rather than adding one. The two real new costs are
`profile.rs`'s **owned** `pub profile: Box<RawValue>` (`profile.rs:175`), so serde scan-skips the
profile and then copies all 26.5 MB — the one extra full-size copy at peak — and
`Utf8JsonWriter.WriteRawValue(string)` (`SptNative.cs:633`), which transcodes through a `chars × 3`
scratch buffer rented from `ArrayPool<byte>.Shared`. That second cost is **weaker than it looks**:
the shared pool *does* serve buffers this large (measured on .NET 10.0.10 by reference identity at 1,
4, 16, 80, 128 and 512 MB — the ~1 MB cliff belongs to `ArrayPool<T>.Create()`'s
`ConfigurableArrayPool`, not `Shared`), so there is no guaranteed ~3x allocation per save: the first
save on a thread allocates ~6.2x the char count and every later one on that thread ~2.0x (both
harness-inclusive; the ~4.2x *difference* isolates the scratch). What keeps it real is that
`ProfileSaveAsync` hops through `Task.Run` and the pool's fast path is a per-thread TLS slot, so a
save landing on a cold threadpool thread pays the first-call price. In steady state the honest cost is
the UTF-8 encode pass, not an allocation.

**The ruling is that the regression ships**: the remedy would make `spt_profile_save` the first export
off the shared `run_generator_with` ladder, at the tail of a phase whose entire value is mechanical
parity. It is re-opened as RUST-ROADMAP.md roadmap item 4. **The load side was not timed, but its allocations are pure
addition** where the save path's replaced an old buffer: `DeserializeFromFileAsync` streamed with
`bufferSize: 4096` so no full-size buffer existed, where the native path materialises three transient
ones — `fs::read` (`profile.rs:133`), `encode_load_frame` (`profile.rs:154-165`) copying into a second
exactly-sized `Vec` so `into_boxed_slice` does not realloc, and `ParseProfileFrame`'s
`span[at..].ToArray()` (`SptNative.cs:598`) copying onto the managed heap because the native buffer is
freed as soon as the wrapper returns. On the same 26.50 MB profile that is ~80 MB of churn per load
against approximately zero before; at most two are live at once, so concurrent peak is ~53 MB. Two of
the three are native, so `GC.GetTotalAllocatedBytes` would not see them, and none of it is on the
save-side follow-up's path.

**Phase 6a — `mpex-server` bootstrap (landed 2026-08-18, no ABI change).** An `mpex-server` bin crate
hosts the CLR via `netcorehost`; `run_app` is shipped by publish and is the release container's
entrypoint, with `scripts/smoke-mpex-server.sh` as its e2e check. `mpex-server.exe` ships from the
same wiring but has never been executed on Windows.

**Phase 6b — rlib linkage flip (landed 2026-08-21, no ABI bump).** The resident DB's statics now live
in the executable: `mpex-server` links `spt-native` as an rlib and is linked with
`-Wl,--export-dynamic`, so all 42 exports sit in its own `.dynsym`, and the two
`SetDllImportResolver` callbacks try `NativeLibrary.GetMainProgramHandle()` before the cdylib. The
published Linux tree therefore ships no cdylib and `SPT.Server.Linux` is no longer a working
direct-run fallback there.

It is **not** the design the spec described. The planned shape — `initialize_for_runtime_config` +
`get_delegate_loader_for_assembly` + an `[UnmanagedCallersOnly] Init(HostVTable*)` in a shim
assembly, with `DllImport` replaced by a 34-slot vtable and an ABI bump — was written out in full,
reviewed twice, and replaced: `run_app`, `Program.Main`, `[LibraryImport]` and the ABI all stay, and
the change is ~85 lines. Five spec overrides and the declined `Build.props` order flip (nothing
forces it: `mpex-server` links a sibling crate, not `SPT.Server.dll`) are in the ledger. Carried
forward:

- **Windows exports.** An `.exe` has no export table without `/EXPORT:` args or a `.def` file, so the
  cdylib exclusion is Linux-gated and Windows behaviour is unchanged — which also still means never
  executed, and `Build.props:31` still maps no `win-x64` triple.
- **The one-linkage-path-per-process rule is enforced by publish layout, not structurally.** The
  published tree has no cdylib, so a lost export anchor is a loud boot failure there; a `bin/` tree
  keeps one for `dotnet test`, so the same mistake under a locally-built launcher boots silently with
  the statics in the cdylib. Nothing at runtime can distinguish the two —
  `GetMainProgramHandle()` is a `dlopen(NULL)` pseudo-handle.
- **The launcher arm has no end-to-end gate outside `scripts/smoke-mpex-server.sh`,** and this fork
  has no CI to run it. `dotnet test` always takes the cdylib arm; `DllImportResolverTests` pins only
  that the test host correctly declines the launcher one.

**Mod-pool ownership (ABI 32, landed 2026-08-25).** `BotPayloadProjection.BuildModPoolSlotOrder` is
gone. Flip #6 decision 2 had shown the order could never go resident — it is the live
`BotEquipmentModPoolService`'s `ConcurrentDictionary` enumeration order, process-local — so the exit
taken was the other one Phase 2's write barriers made safe: **own the pools rather than observe
them.** The native pool now enumerates the template's own `Properties.Slots`, and the 26,428 bytes
per send left the wire on both arms. This is a **deletion** on the C# side rather than a port: pool
*contents* were derived natively from the bot port onward (`mod_pool_service.rs`), and only the
*ordering* was ever observed from C#. `BuildRequest` fell **5.19 → 0.23 ms** (assault, BENCHMARK.md
§ Mod-pool ownership). What it bought: the C# order was sized from `Environment.ProcessorCount`, so
it was never machine-independent, and the native draw order is host-independent now. What it cost:
the two arms draw in different orders at randomised levels, and the exact-output coverage there is
gone on both — booked in RUST-ROADMAP.md § *Broken*, together with the process-nondeterminism finding that
made a C#-side golden unimplementable. `BotEquipmentModPoolService` gained a whole-type decline
entry (guideline 2), Rust no longer consulting it — with its two `protected` pool-property getters
(`GearModPool`, `WeaponModPool`) re-admitted to the scan explicitly, because `IsSpecialName` filters
property accessors out of the `_hookableMembers` sweep.

*Coverage forensics behind the two Broken bookings, in full:*

- **The randomised-level coverage loss.** The C# order was the live service's `ConcurrentDictionary`
  enumeration order, sized from `Environment.ProcessorCount` — measured moving at 13 real slot names
  between 8 and 16 cores. A different draw order means different RNG consumption, so no
  order-insensitive cross-arm comparison could ever pass again. The level-1 matrix
  (`TheSameSeedGeneratesEquivalentInventoryOnBothPaths`, 4 roles × 2 seeds) is untouched but reaches
  the module only through `get_required_mods_for_weapon_slot`, which reads the template directly and
  never `derive_pool` — the three pool-building call sites (in `bot_inventory_generator` and twice in
  `bot_equipment_mod_generator`) each sit behind a randomisation gate level 1 never trips. The
  replacement, `BotParityTests.TheNativePathGeneratesAtRandomisedLevels`, is a smoke case over the
  same 44 cases (2 roles × 22 seeds): generation completes, native ran, inventory is non-trivial —
  nothing about *which* items came out. The nighttime clamp's effect on the inventory is therefore
  uncovered on both arms, though `TheNighttimeRandomisationClampIsReplayedOnBothPaths` still pins the
  clamp write. A patch on the pool service declines to legacy, where the machine-dependent order
  still applies.
- **Why no C#-side golden can exist.** `MongoId.GetHashCode` is `HashCode.Combine`, which .NET seeds
  per process, so every `Dictionary<MongoId, …>` the bot projection serialises enumerates in a
  process-random order and the seeded native draw walks it. Two back-to-back runs of one isolated
  fixture produced inventories differing in item **count** (69 → 68), so no normaliser absorbs it.
  **The Rust-side golden does hold:** `flip6_bots_resident.rs` drives both bot exports through the
  FFI in its own process off a synthetic DB, and `src/bot/` has no equivalent hazard (no `HashMap`;
  its four `HashSet`s are membership-tested, never iterated; everything the draw walks is an
  `IndexMap`/`IndexSet`). `RESIDENT_BATCH_GOLDEN` pins the exact bytes of a three-bot batch at fixed
  seeds — both PMC level bands and the preset fallback — and held across five processes and both
  build profiles. It reaches `derive_pool` through all three gated routes
  (`get_compatible_mods_for_weapon_slot`, `get_mods_for_weapon_slot`, `get_mods_for_gear_slot`, each
  confirmed by a panic probe), and pins ordering: a randomised mount's two derived sub-slots make the
  key order observable, a two-candidate `mod_foregrip` makes the inner set order observable through
  the seeded pick. The C#-side fix — sorting the projection's `MongoId`-keyed dictionaries before
  serialising — would work and is deliberately not taken: it changes the draw order on **every**
  native path and so alters generated bots server-wide, a live-wire behaviour change owing its own
  spec and parity gate, not a test repair. Deferring is safe because bots are random by design and no
  consumer asks two processes to agree.

**Load-epoch seeding (ABI 33, landed 2026-08-26).** The Phase 3 follow-up, delivered: on a modless boot
the first `EnsureCurrent` no longer publishes. `DatabaseImporter.LoadDatabaseAsync(seedResidentDb:)` is
opt-in and `Program.cs` passes true only when `loadedMods.Count == 0`; it follows the native load's
five-root epoch-1 install with a **configs-only publish built from the live C# config objects** — never
`configs/*.json`, the values-not-keys trap — reaching epoch 2, and records `(epoch, stamp)` in the static
`DbLoadSeed`. `DbPublisher.EnsureCurrent` consumes that seed once: it forces the same `HandbookHelper`
hydration a first publish would have forced, under the same `WriteBarrier.Suppress()`, then either logs
`Load-time seed consumed at epoch N; first publish skipped.` and starts from that epoch, or logs
`Load-time seed voided: …` and republishes because the stamp moved in the window. Two changes close that
window. `ItemConfig.HandbookPriceOverride` now rides the `spt_db_load` request, so the resident handbook
carries the merged prices C#'s lazy hydration produces — an **envelope-only merge**: the raw handbook
bytes are restored before `files` is handed back, so the C# reflection walk still parses the shipped
file. And `PostDbLoadService`'s `coreConfig.ServerStartTime` write, the one stamp mover that precedes the
first `EnsureCurrent`, is suppressed; the carve-out leaves the resident `spt-core` entry stale by exactly
that field until the next real republish, which is safe only because nothing native lifts `spt-core`
(lift the suppression the day a consumer appears). The gate is `ResidentRootEquivalenceTests`: the
load-installed roots against a `DbPayloadProjection` publish of the same tree, compared through
`spt_db_resident_digest` (ABI 33's new export) as canonical post-parse digests of the **typed lift
surface** — `extra` maps excluded, because envelope text legitimately differs there in member order,
number formatting, explicit nulls and Debug-build model coverage. What it buys is **publishes 2 → 1**,
not 1 → 0: `AdjustLocationBotValues` still bumps the stamp before `RagfairCallbacks` generates offers, so
that second publish stays, by design. Measured at the boot, that is **−861 ms to `Server has started`
against the merge base, −7.5%** — the skipped publish is worth −2286 ms in the `PostDbLoadService`
block and gives 483 ms back on the import line and 880 ms back to the now-cold `RagfairCallbacks`
publish (BENCHMARK.md § Load-epoch seeding, which also explains why boot-to-`/health` reads this as a
*regression*: `/health` answers before the publish it skips). The `Database import took Nms` line now
contains a publish on a modless boot and is no longer comparable to any pre-phase figure.

**Equipment split (ABI 34, landed 2026-08-26).** `BotConfig.Equipment` is resident. The member Phase 4
declined — 39,811 B on every send, both arms, and since ABI 32 by far the largest piece of genuinely
varying process state on a bot request — now rides `BotConfigLift` as a strict, typed
`equipment: IndexMap<String, Option<EquipmentFilters>>`; the resident root keeps the null roles the
per-call projection used to drop and `resolve_equipment` applies that filter instead. The views
override gained the same member for the ineligible arm. One slim varying member survives:
`liveEquipmentMods`, role → band → `EquipmentMods` — the only cells a barrier-invisible runtime writer
touches *and* Rust reads. Inside the subtree the softened `level_range`s are `#[serde(default)]` (the
`EquipmentFilterDetails` precedent): the two nested resident ones and — post-review — the overlay
wire's own (`LiveEquipmentModsBandWire`), without which a mod-nulled `levelRange` (the serializer's
`WhenWritingNull` turns an explicit null into an omitted key on publish and request alike) would
survive the publish only to fail every subsequent bot request; a defaulted `(0, 0)` band matches
nothing and drops. Everything else stays strict, and `generation` stays undeclared, so W3's polluted
cell never enters resident state at all. The two strict leaves remaining inside the subtree —
`NighttimeChanges.equipmentModsModifiers` and `ArmorPlateWeights.values` — are the lift's
publish-abort surface: mod data that nulls either serialises as an omitted key and fails the whole
`spt_db_publish` (previous resident DB retained, every family falls back to override sends) where it
used to fail one bot request's varying parse.

The overlay carries every role whose `Randomisation` is non-null and every band whose `EquipmentMods`
is non-null, on both arms of both envelopes, keyed by `levelRange`. **As built the pairing is
positional among the resident bands that themselves carry `equipment_mods`**, not across the whole
`randomisation` list: the sender skips the bands whose live `EquipmentMods` is null
(`BotPayloadProjection.cs:101`), so the overlay is a *subsequence* of the role's list and the two ends
line up only if the merge skips the same bands (`resolve_equipment`'s doc comment,
`rust/spt-native/src/bot/mod.rs:263-277`; without the `is_some` half of the predicate, two bands
sharing a `levelRange` where the first carries no mods sent the second band's live mods onto the
first). Each matched resident band is written at most once, so duplicate ranges pair positionally
rather than clobbering; unmatched overlay entries drop, degrading into the container stale window
booked in RUST-ROADMAP.md § *Broken*. The merge builds one owned map per request, hoisted to the two entry points
(`generate_inventory` / `generate_inventory_batch`, right after `resolve_bot_views`) and passed into
`generate_prepared` by reference — one added parameter, the only signature change. `BotContext`'s
`equipment` kept its type and no downstream reader changed.

**The writer sweep is what made the lift safe, and it found five writers where Phase 4 named one.**
W1 (`ReplayRandomisationClamps`, after every native single-bot send) and W2 (the legacy per-bot
equivalent) write `Randomisation[band].EquipmentMods` — the cell the overlay carries, so bot 2 of a
nighttime raid still reads bot 1's clamps on both arms. W3 (the UNHEARD pocket-loot alias) and W5
(loot-cache hydration through the same alias) write `Generation`, which never goes resident; both are
booked in RUST-ROADMAP.md § *Broken* and neither is fixed here — breaking the alias at
`BotEquipmentFilterService.cs:100-105` would change generated bots server-wide, the MongoId-sort
precedent. W4 (the admin panel's whole-property `SetValue`) goes through a barriered setter and
republishes **on a barriered (Release/publish) build**: a Debug zero-mod server has no barriers yet
stays resident-eligible (`ResidentDbDispatch.Eligible` consults `WriteBarrier.Installed` only on the
modded arm), so an admin-panel apply there reaches the next send only through the
`liveEquipmentMods` overlay until something republishes — the pre-existing Debug pattern for every
resident member, W4 is just the first writer whose disposition *depends* on the barrier. Its other
pre-existing quirk is that the apply orphans `BotEquipmentFilterService`'s
constructor-cached reference, severing the C# half of the feedback loop — untouched. What the lift
costs is the container stale window, now stretched over the whole equipment graph, plus two C# reads
that stay live while Rust reads resident (the batcher's nighttime decline and its band cutting — so
a runtime-added band cuts batch variants on boundaries the resident side does not know), which
degrade into that same window.

*W3/W5 in full (the roadmap's Broken entries carry the compressed form):*

- **W3 — an UNHEARD PMC permanently widens pocket-loot weights for every later PMC in the process**
  (left bug-for-bug). `BotEquipmentFilterService.AdjustGenerationChances` aliases the config's
  `GenerationData` `Weights`/`Whitelist` dictionaries into the deep-cloned bot template by reference,
  and `BotGenerator.AddAdditionalPocketLootWeightsForUnheardBot` writes through that alias into
  `BotConfig.Equipment["pmc"].Randomisation[band].Generation["pocketLoot"].Weights` — so the next
  bot's prelude copies the polluted reference on, on every path. Pre-existing and invisible to the
  stamp (`GenerationData` is denied in `WriteBarriersPatch`, and the write is an indexer-set
  regardless); never read *via resident state* — Rust's `RandomisationDetails` declares no
  `generation` field, and the split deliberately keeps it that way — though the polluted cells do
  reach Rust on every send, always fresh, through the bot template's `generation` block on the
  varying wire (`BotTemplateWire.generation`: the pocket-loot resize, the loot-count draws, the
  magazine chances). Anyone lifting the bot *template* to resident state inherits the barrier
  analysis that freshness currently makes unnecessary. The pollution is also path-dependent since the
  batch flip: the single-bot path still runs the C# write (gated on `!nativeLevelAndFilter`), while a
  batched UNHEARD PMC gets the extra weights natively on Rust's own template copy and leaves the live
  config clean. Shipped data confines the leak to PMC bands 0–1 (levels 1–22) on both the write and
  the read end: only those bands carry `pocketLoot`. The `PlayerScavGenerator.AdjustItemWeights` twin
  aliases `PlayerScavConfig` unconditionally; what keeps the UNHEARD pocket write off player scavs is
  `BotGenerator`'s `IsPmc` gate, not the aliasing. Breaking the alias (copy, not reference) is the
  root-cause fix and changes generated bots server-wide — declined like the MongoId-sort test repair.
- **W5 — loot-cache hydration writes back into the live bot config through the same alias** (same
  disposition). `BotLootCacheService.GetGenerationWeights` returns the aliased whitelist object
  rather than a copy, so when a shipped whitelist is empty-but-present
  (`pmc.randomisation[0].generation.drugs`, `[1].generation.stims`) hydration `TryAdd`s every
  matching tpl from the combined loot pool into
  `BotConfig.Equipment[role].Randomisation[band].Generation[subtype].Whitelist`, and the cache then
  shares those objects. Pre-existing, barrier-invisible, never read via resident state — and W3's
  template-wire caveat applies: the polluted whitelists cross fresh on the template's `generation`
  block every send.

*The stale window the lift opened, enumerated:* `Add`/`Remove`/indexer-set on any collection
reachable from `EquipmentFilters` — the seven top-level ones (`Randomisation`, `Blacklist`,
`Whitelist`, `WeightingAdjustmentsByBotLevel`, `ArmorPlateWeighting`, `WeaponSightWhitelist`,
`WeaponSlotIdsToMakeRequired`) *and* the natively-read containers nested inside them
(`NighttimeChanges.EquipmentModsModifiers`, `RandomisedArmorSlots`, `RandomisedWeaponModSlots`,
`MinimumMagazineSize`, `ArmorPlateWeights.Values`, the band `Equipment` filter maps) — was read
fresh on every send before the split and is now invisible until the next stamped write. The role
dictionary itself is an indexer surface with a distinct failure shape: a runtime-registered role
works today and is simply missing resident-side, where the equipment phase early-returns with a
diagnostic (`bot_inventory_generator.rs`) and the weapon path errors the bot
(`bot_weapon_generator.rs`). The four `MinMax<int>` `LevelRange`s in the graph
(`RandomisationDetails`, `EquipmentFilterDetails`, `ArmorPlateWeights`,
`WeightingAdjustmentDetails`) are in the window through the open-generics hole (Phase 2 above),
with a failure shape of their own: a runtime `LevelRange.Min`/`.Max` set moves no stamp, so Rust
keeps selecting bands on the published range while the C# prelude uses the live one — and the
overlay pairs by `levelRange` equality, so the edited band's overlay entry matches no resident band
and **drops silently**; the nighttime clamp feedback loop dies for that band while everything else
keeps working. No production code writes a `LevelRange` (two independent sweeps); a mod-facing
hazard. `DisableNativeRequestCache` restores per-call freshness on every arm;
`TrustNativeRequestCacheWithMods` is consulted only on the modded arm
(`ResidentDbDispatch.Eligible`).

**The gate is `BotResidentDbTests.ASecondNighttimeBotSeesTheFirstBotsClampsOnTheResidentPath`,** two
sequential nighttime single-bot resident generations asserting bot 2's clamps compound bot 1's and
that the end state matches a legacy double run; it was sabotage-checked against a disabled merge loop.
Nothing that existed before gated the overlay: `AnInPlaceEquipmentModClampReachesAResidentSend` was
believed to and does not (its perturbation reaches native through the *template*, its wave is daytime,
its band carries no `nighttimeChanges`), and its docblock now says so. `RESIDENT_BATCH_GOLDEN` is
unchanged, as a residency flip requires — but it pins the plumbing, not the merge: its bands carry no
`equipmentMods` and its wave is daytime, so the merge-loop body never executes there. Six Rust unit
cases cover the merge itself (`bot/mod.rs:696-886`), and the batch entry point's hoist has a direct
gate of its own (`the_batch_arm_computes_clamps_off_the_live_overlay_mods`, added post-review after
a sabotage run showed the whole tree stayed green with the batch-path merge blinded — its safety
had rested entirely on `BotWaveBatcher`'s nighttime decline policy). Wire and `BuildRequest` numbers
are in BENCHMARK.md § Equipment split.

**Map/raid setup (ABI 35, landed 2026-08-27).** The whole of `RaidTimeAdjustmentService`'s algorithm
plus `LocationLifecycleService`'s two `LocationBase` passes moved to `src/raid/`, behind four exports:
`spt_get_raid_adjustments`, `spt_make_adjustments_to_map`, `spt_adjust_bot_hostility_settings` and
`spt_adjust_extracts`. `Native/` grew 909 lines across four files — `Native/Raid/RaidPayloads.cs`
(525), `Native/Raid/RaidNativeRequestBuilder.cs` (297), plus the `[LibraryImport]` and wrapper entries
in `NativeMethods.cs` and `SptNative.cs`. The two services keep their full 4.1.2 bodies.

- **Deltas cross, not objects, and that is what the aliasing forced.** Each export takes a small
  C#-projected request carrying only the members the algorithm reads and answers with keep-index
  lists, per-index field updates, append selections and warning flags; a thin C# applier mutates the
  *original* objects in legacy order. Three live-object channels thread through what looks like a
  clone-only mutation — `AdjustPMCSpawns` offsets `.Time` on live `PmcConfig.CustomPmcWaves` instances
  the PMC splice appended by reference (a permanent config mutation that compounds across shortened
  raids — an upstream bug, preserved), `AdjustBotHostilitySettings` appends live `ChancedEnemy`
  instances into the clone, and `AdjustExtracts` assigns a deferred `Exits.Union` over live
  `AllExtractsExit` instances. A whole-`LocationBase` round trip would sever all three, so it would
  need a replay block anyway — at which point the object crossing pays for nothing, while costing a
  29-record model mirror and megabytes per raid start (at `LLS:211`/`:213` the clone already carries
  the generated loot). Deltas keep the reference identity structurally: the applier writes the very
  objects legacy wrote, and the `Exits.Union` statement is kept verbatim. Precedent is the repo's own —
  the bot level draw riding back for the caller to write, `ReplayRandomisationClamps`, the quest
  `QuestTypePool`'s `CopyPoolInto`.
- **The carve-out is `AdjustLootMultipliers`,** which runs C#-side on **both** arms and is excluded
  from the frozen set: it writes the live `LocationConfig` multiplier dictionaries through the indexer,
  and `GenerateLocationLoot` reads them a few lines later when it builds *its* native request. Phase 4
  ruled this exact write is why the multipliers stay per-call in the loot family's varying block. A
  patch on it therefore does **not** decline the raid family to legacy, and fires on the native arm —
  pinned by `RaidAdjustmentHookLivenessTests`.
- **No resident state, no new root, no epoch** — Phase 5's precedent. Option C was costed and declined:
  `EscapeTimeLimit` and `Exits` sit untyped in `LocationBaseView::extra`, `scavRaidTimeSettings` untyped
  in `LocationConfigLift::extra`, and `SurvivedSecondsRequirement` has no globals lift at all, so going
  resident means new lifts, a changed digest surface and the whole eligibility/trust/stale-heal
  machinery — to amortize a ~2 KB payload on a menu-frequency endpoint, while *adding* a stale window
  per-call projection does not have. **It is the named upgrade path** if a later phase wants those lifts
  for other reasons; it is owned by no phase today. Consequence: the four exports carry no `epoch` and
  no `viewsOverride`, `TrustNativeRequestCacheWithMods`/`DisableNativeRequestCache` do not apply, and
  `FfiFailure` has a single arm — raid never returns `STATUS_STALE_EPOCH`.
- **The ABI number has five sites, not three.** `lib.rs:21`, `SptNative.ExpectedAbiVersion`
  (`SptNative.cs:129`) and the `ffi.rs` tripwire assert are the lockstep three; prose adds two more:
  `rust/ARCHITECTURE.md`'s module map ("currently N") and this file's own "Current ABI N" sentence at
  the top. Any renumber — the parallel-branch collision rule, whichever of two same-number bumps
  lands second, which is exactly how this family landed at 35 behind the equipment split's 34 — is
  those five paths, explicitly enumerated. A blanket docs-grep for the stale number is **unsafe
  post-merge**: the previous family's ledger keeps its own historical number legitimately at a dozen
  sites, so grep only to confirm the five, never to rewrite every hit.
  **The export counts are a second silent-merge surface, and a worse one**, because nothing asserts
  them: the count sits in prose — spelled out *and* as a bare numeral — across `ARCHITECTURE.md`, this
  file and `rust/ARCHITECTURE.md` (no per-file tally here; a prior one was wrong twice over, so grep
  is the procedure), and a parallel branch adding exports writes *its* number into the same files
  on adjacent-but-distinct lines — so git merges both cleanly and leaves the tree internally
  inconsistent with no conflict to notice. The integrator's procedure is therefore to **re-derive the
  count, never to sum the branch deltas**: `grep -c '#\[unsafe(no_mangle)\]' rust/spt-native/src/ffi.rs`
  is the ground truth. The two subset counts in `rust/ARCHITECTURE.md` — the generation exports and
  the buffer-returning ones — move with it, as do the bare numerals in the rlib-anchor prose. The
  literals are deliberately not quoted here: they went stale the first time this paragraph was
  re-read, at ABI 36.
- **No standing benchmark fixture**, by ruling rather than omission: menu and raid-start frequency has
  no throughput to win, so Phase 5's decision 11 applies — the free number off the parity run is
  recorded (BENCHMARK.md § Map/raid setup) and no `[Explicit]` harness is added.
- **Booked divergences — all exception-type changes, no behavioural one.** One is reachable on shipped
  data without a mod: `labyrinth` is in `LocationTable` but absent from `scavRaidTimeSettings.maps`, so
  its missing-key `KeyNotFoundException` becomes an `InvalidOperationException` naming the map (whether
  the shipped client flow can queue a scav there is unverified). The rest need a mod-shaped config: null
  `AlwaysEnemies` with additional enemy types (NRE → error) and a non-numeric `ReductionPercentWeights`
  key (`FormatException` → error). `MakeAdjustmentsToMap`'s error paths apply no deltas where legacy
  left a partially-mutated, then-abandoned clone — unobservable, the multiplier side effect being
  identical on both arms. And the two appliers read a **snapshot** where legacy re-reads after its own
  write — `adjustments.rs:463` takes the pmc offset's operand from the request where RTAS:362 re-reads
  the live `spawn.Time`, and the wave applier writes absolute `WaveTimes` where legacy compounds two
  subtractions in place — which diverges only if one `BossLocationSpawn`/`Wave` instance appears at
  two kept indices. **Mod-reachable**, the fourth mod-shaped divergence: the clone yields distinct
  objects, but the PMC splice appends the config's own instances by reference
  (`PmcWaveGenerator.ApplyWaveChangesToMap`), and `AddPmcWaveToLocation` accepts the same
  `BossLocationSpawn` twice — legacy then compounds the offset on the aliased instance, landing on
  the live `PmcConfig` object and accumulating across raids, where the delta write is
  last-write-wins. The parity fixture cannot construct the shape: its `WithWaves`/`WithBossSpawns`
  builders clone prototypes per call precisely to prevent instance sharing. Inherent to the delta
  protocol, so booked rather than mitigated.
- **`AdjustBotHostilitySettings` Quirk-10 error path applies no deltas where legacy left earlier roles
  applied.** Unobservable: the clone half is abandoned identically, and the one surviving mutation — the
  duplicate-`Role` merge write onto the live `PmcConfig` `ChancedEnemy` (LLS:316-324) — is idempotent
  and value-independent (LLS:310 clears and the loop recomputes first := last every run);
  `pmcConfig.HostilitySettings` has no other reader. Reachable only with mod config. The native error
  message *text* is invented — legacy NREs with no message — so the spec books the type change, not the
  text.
- **Null `ExitChanges` on a session-parked `RaidChanges`** would NRE in legacy at RTAS:62 and fails JSON
  parse natively (`Vec` rejects null) → BAD_ARGS → `InvalidOperationException`. Unreachable: RTAS:215
  always writes `[]`. It is booked because the request DTO carries the **real** `RaidChanges` record
  rather than a mirror — a required-`List` mirror could not hold the null the divergence depends on.
- **Two log lines are dropped and booked.** The train-disable debug line (RTAS:351) — its
  `mostPossibleTimeRemainingAfterDeparture` operand is a per-exit intermediate the wire does not carry —
  and the negative-weight warning inside `GetWeightedValue` (WeightedRandomHelper.cs:80), which fires
  mid-draw where the applier cannot see it and which the Rust twin already drops. Neither is
  load-bearing. Every other message is re-emitted verbatim C#-side from the delta fields, under the same
  `IsLogEnabled` guards; only *timing* moves (after the call, not during).
- **Two builder touch-order fidelity notes.** The hostility builder no longer dereferences
  `BotLocationModifier` when the config loop would run zero iterations — legacy no-ops there, so the
  early deref was a *new* exception rather than a booked type change, and the fidelity bar books only
  type changes. The extracts pass gates on the side *before* calling its builder — legacy returns
  after one string compare for every PMC raid, so the projection, the `GetLocation` deref and the FFI
  crossing wait for a scav side; on the native arm `IsSide` is frozen-set-guaranteed unpatched, so
  the C#-side gate is the very test `raid_start.rs` would have run. The map builder's three arrays
  (`Exits`, `Waves`, `BossLocationSpawn` — none `required`, so a mod-added base.json omitting one
  deserializes to null) project null-tolerantly as empty: legacy's touch conditions (non-empty
  `ExitChanges`, `AdjustWaves`, the chance roll) are partly native-side and cannot be reproduced at
  projection time, so the empty projection lands every absent-array case on legacy's *no-op* side.
  The residual booked divergence is the would-have-touched half: an absent array legacy would have
  enumerated NREs there and no-ops (exits: warns) natively.
- **Quirk 4's "a `None` time seeds the offset" sub-state is unreachable** and the spec and plan still
  assert it — recorded here so nobody re-litigates it. The offset filter's pmc set is a strict subset of
  the keep filter's, and the keep filter admits a pmc spawn only with `Some(time) > start`, so every
  spawn that reaches the offset has a time. The dead legs are ported and commented anyway, per the
  fidelity bar; their reachable halves are tested.

**Tier 1 tail (ABI 36, landed 2026-08-27).** TODO.md's whole remaining tier 1 — #4 `PmcWaveGenerator`,
#5 `AchievementController`, #6 `WeatherGenerator` — in one PR, behind `spt_apply_pmc_wave_changes`,
`spt_get_achievement_statistics` and `spt_generate_weather`. **Completeness-only, by the queue's own
framing: no measurable win, and BENCHMARK.md gets no section.** None of the three sits on a hot path:
the wave splice runs once per raid start, and the other two answer cold client routes
(`AchievementCallbacks`, `WeatherController.Generate` — weather also fills `RaidWeatherService`'s
forecast cache, a loop deliberately left unbatched because its between-call time-period draw is
C#-side state). All three ride the raid family's wire pattern — no epoch, no resident
reads, no `viewsOverride`, everything in the request — so all three add a single-armed `FfiFailure`
and none can return `STATUS_STALE_EPOCH`. `src/raid/pmc_waves.rs` (76 lines) joins the raid module as
its fifth export; `src/achievements.rs` (170) and `src/weather.rs` (826) are crate-root modules on the
`base_class.rs` shape.

- **The PMC wave splice is the fifth raid export and added nothing to the family.** It shares
  `RaidNativeRequestBuilder`, `LocationConfig.ForceLegacyRaidAdjustments` and the seven-member frozen
  set unchanged — a patch on any of those seven declines all five. Booked divergences, all
  projection-shaped and all on the raid family's existing null-tolerant-projection precedent:
  a null `BossLocationSpawn` with every gate passing projects an empty `bossNames`, and the applier
  then assigns a fresh list and *does* append into it (`PmcWaveGenerator.cs:121-135`), where legacy
  NREs on its removal filter — unreachable on shipped data, and pinned by
  `ANullBossLocationSpawnProjectsAnEmptyListAndAppendsNatively`; `waveCount` projects
  `wavesToAdd?.Count ?? 0`, so a mod config carrying a null list *value*
  (`"customPmcWaves": {"bigmap": null}`) no-ops natively where legacy NREs on the unguarded `.Count` —
  mod-only, the model declares the dictionary `required` and non-null-valued. The
  `location.Id.ToLowerInvariant()` lookup stays C#-side and is **gated behind
  `RemoveExistingPmcWaves` exactly as legacy gates it**, which closes the null-`Id` quadrant rather
  than booking it: with the flag false the builder sends `wavesFound=false, waveCount=0, bossNames=[]`
  without touching `Id` or `BossLocationSpawn`, so both arms no-op; with it set, both arms NRE
  identically — both halves pinned in `RaidAdjustmentParityTests` by
  `ANullMapIdIsANoOpOnBothPathsWhenTheFlagIsOff` and `ANullMapIdThrowsOnBothArmsWhenTheFlagIsOn`.
  A null `BossName` survives the removal filter on both arms — `HashSet.Contains(null)` is false, and
  the native side is an exact string match on the two PMC names.
- **Achievement statistics contribute zero frozen members.** `ProfileHelper.GetProfiles`, the
  `CoreConfig.Features.AchievementProfileIdBlacklist` filter and the whole profile projection stay
  C#-side, so patches and blacklist mutations are live on **both** arms; the only other member of the
  controller is a bare field return that always runs C#. Nothing hookable is bypassed, so there is no
  `AnyFrozenMemberPatched` scan — `CoreConfig.ForceLegacyAchievementStatistics`, the null builder and
  the subclass check are the whole decline rule. The response is an `IndexMap` in achievement-table
  order, because the legacy dictionary serializes to the client in insertion order and that order is
  observable JSON, and the percentage uses the banker's-rounding twin — `(int)Math.Round(double)` is
  banker's, `f64::round` is not. **Booked divergence:** duplicate achievement ids make legacy's
  `stats.Add` throw `ArgumentException` where native returns an error message crossing as the usual
  `InvalidOperationException`. Unreachable on shipped data; the changed exception type is the
  established pattern for mod-reachable malformed data.
- **Weather froze seventeen members across five classes and added a type check on top.** The set
  spans every body the native arm reimplements *except the shared draw primitives* (listed under
  *What flips to legacy*): `WeightedRandomHelper.GetWeightedValue`/`WeightedRandom` and
  `RandomUtil.GetDouble`/`GetInt` are reimplemented natively yet appear in no frozen set anywhere in
  the repo — the project-wide convention, not an oversight here. A Harmony patch on those steers
  legacy generation only, and weather is the first port whose *whole* draw path is
  `WeightedRandomHelper`. On top of the set, native runs only when the injected
  `IEnumerable<IWeatherPreset>`'s concrete types are exactly the three built-ins — the bots'
  `InventoryMagGenComponents` precedent. The two catch different attacks:
  the type check catches *substitution*, the frozen members catch a Harmony patch on a built-in
  preset, which never changes the type set.
- **Weather pre-resolves three things C#-side, and each is argued rather than assumed.**
  `refillWeights` is fetched unconditionally where legacy fetches it only on refill — unobservable,
  because every member whose patch could see the changed call pattern is frozen. `isNight` comes off
  `weatherHelper.IsHourAtNightTime`, a **live collaborator call**, at the same seconds-as-ticks
  expression legacy uses (an epoch-seconds value fed to the ticks constructor, so day/night comes off
  a year-0001 date — fixing it would change generated weather server-wide). And the applier calls
  `SetCurrentDateTime(result, timestamp)` with the **original, possibly-null** argument, never the
  resolved one: the explicit-timestamp branch's `FormatToBsgDate` runs `ToUniversalTime()` on the
  `Kind=Unspecified` result of `GetDateTimeFromTimeStamp`, which .NET reinterprets as *local* time, so
  substituting the resolved value would shift `Date`/`Time` by the host offset on any non-UTC host —
  and `WeatherController.Generate` passes a null timestamp on every `client/weather` request.
- **Booked divergences (weather).** On null-timestamp calls `isNight` derives from its own
  `GetTimeStamp()` read, one extra clock read ≤ 1 s ahead of the reads `SetCurrentDateTime` makes
  later; there is no field-level skew, since `Date`/`Time`/`Timestamp` all come from
  `SetCurrentDateTime` running legacy's exact branch, and the seconds-as-ticks quirk lands every epoch
  value in hour 0 regardless. The rest are error-message-instead-of-exception cases, every one
  modded-config-only (a hand-edited `weather.json` reaches each, no mod DLL needed): an absent chosen preset block (legacy `KeyNotFoundException` at the `["default"]` indexer —
  though a null `presetWeights` *table* reaches that same native error by a different legacy route, an
  NRE inside `GetWeatherWeightsByPreset` itself (`WeatherGenerator.cs:281-286`), because `Resolve`
  tolerates the null and crosses every block as absent); an unparsable picked weight value (legacy
  `FormatException`); a drawn-but-absent block member (legacy NRE — every `PresetWeights` member but
  `Clouds` is nullable and legacy dereferences lazily, so the wire projects member-wise
  null-tolerantly and native errors at the same place legacy NREs); a drawn-but-*empty*
  member table (`"fog": {}`), which crosses as an empty list rather than as absence — the builder's
  `ToWeighted` maps the dictionary, only a null one nulls out — so native errors out of
  `get_weighted_value` where legacy throws `ArgumentOutOfRangeException` indexing the empty item list
  behind its uniform shortcut, and neither arm consumes a draw doing it, so the stream stays aligned;
  the empty preset state (legacy `ArgumentOutOfRangeException` out of `WeightedRandomHelper`'s
  uniform shortcut); and the empty season table (`GetValueOrDefault("default")` returns null rather
  than throwing, so a config missing both the season key and `"default"` projects empty and native
  errors on the next refill where legacy NREs). That last one has a sibling that is not an
  exception-type change at all: a
  `"weatherPresetWeight": null` config NREs inside `GetWeatherPresetWeightsBySeason` itself, which
  the native arm calls unconditionally at dispatch where legacy calls it only on refill — the same
  NRE, surfaced earlier and on every call rather than on the first refill. Mod-only, since the
  shipped config carries the table, and `forceLegacyWeatherGeneration` is the escape hatch. The whole
  enumeration also diverges in *mutation ordering*, not only in type, because legacy mutates the
  caller's `ref` dict on its way to every one of these throws: with an empty state **and** an empty
  refill table it replaces the dict with the clone (`WeatherGenerator.cs:208`) before the pick throws,
  and on any state it writes the previous-preset decay into it (`:217`) before the pick, the block
  lookup, the member deref or the parse can throw — where native mutates nothing until the applier
  runs on a successful return. Observable only to a caller that catches, and no caller does.
- **An out-of-enum `WeatherPreset` key declines to legacy rather than diverging.** `EftEnumConverter`
  parses any numeric config key, so a `"4"` crosses as `(WeatherPreset)4`, which `Enum.GetValues`
  mints no `presetBlocks` entry for — the native arm would error where legacy generates fine off a
  `"default"` block. `HasOutOfEnumPresetKey`, inside the native branch, sends both carriers of such a
  key — the caller's preset-weight state and the season refill table — to legacy first
  (`UseLegacyPath()` itself is unchanged, and the arm's `GetWeatherPresetWeightsBySeason` fetch is
  hoisted above the guard so it stays one unconditional call). Legacy then handles the key at
  `GenerateWeatherByPreset` (`WeatherGenerator.cs:251-273`): a warning, a fall back to the Sunny
  *generator*, and a `KeyNotFoundException` only when neither the key's own block nor `"default"`
  resolves — which is what the shipped config, carrying no `"default"` block, does.
- **Weighted values parse at draw time, only the picked one, and the two parsers disagree at the
  edges — in both directions.** A non-numeric entry that is never picked never throws on either arm,
  matching legacy. When one *is* picked, Rust's `str::parse::<f64>` accepts `inf`/`+inf`/`-inf`, which
  `double.Parse(s, CultureInfo.InvariantCulture)` rejects as a `FormatException` (invariant culture's
  symbol is `Infinity`, not `inf`); conversely C# accepts thousands separators — `double.Parse`'s
  two-argument overload carries `NumberStyles.AllowThousands`, so `"1,234.5"` parses there and errors
  natively — and, the third direction, leading or trailing whitespace, which that overload's
  `NumberStyles.Float` allows (`AbstractWeatherPreset.cs:22-37`) and `str::parse::<f64>` rejects
  outright (`weather.rs:365-367`). `Infinity` and `NaN` parse on both, case-insensitively, and there
  the divergence moves to the *response*: serde_json writes a non-finite `f64` as JSON `null`, and
  `GenerateWeatherResponse.Cloud` is `required double` (`WeatherPayloads.cs:190`) like every other
  drawn field, so a non-finite draw makes the native arm throw `JsonException` out of `DecodeResult` —
  not the `InvalidOperationException` the export's docs name, though the response buffer is still
  freed in the `finally` — where legacy assigns `double.PositiveInfinity` and fails much later,
  client-side. Mod-only either way: shipped weather configs carry plain decimal strings.
- **Legacy's `WeightedRandomHelper` diagnostics have no native counterpart.** The per-negative-weight
  warning (`WeightedRandomHelper.cs:80`, already booked as dropped by the raid family) and the three
  localized error logs for empty or mismatched item/weight lists (`:55`/`:60`/`:65`) simply do not
  fire on the native arm; the draw semantics they narrate are matched bug-for-bug, but the table they
  fire on is hand-edited config, so silent-native-versus-warning-legacy is user-visible.
- **The `ICloner` on the refill path is never called natively** — the standing documented collaborator
  hole, recorded here rather than fixed. Legacy clones the season table into the caller's state
  through `cloner.Clone`; native rebuilds it from the wire with clone semantics but without the call,
  so a patched or substituted `ICloner` does not observe the refill.
- **No new resident state, no new root, no new flag beyond the two.**
  `CoreConfig.ForceLegacyAchievementStatistics` and `WeatherConfig.ForceLegacyWeatherGeneration` are
  C#-default `false` and unserialised, like every force-legacy flag but `forceLegacyLootGeneration`.
  Weather is the only one of the three that draws, so it is the only one carrying `testSeed`; its
  parity fixture re-seeds **both arms before every call**, because `TestSeedGuard::install` starts a
  fresh xoshiro stream per FFI call while a single legacy `SeededRandomSource` continues its stream
  across calls — a one-seed multi-call comparison fails by construction.

**The ported 4.1.2 quirks are documented at their call sites** as numbered `Quirk N` comments in
`rust/spt-native/src/quest/*.rs`, `src/scav_case/generator.rs`, `src/base_class.rs`,
`src/linked_items.rs`, `src/loot/container_extensions.rs` and `src/raid/*.rs`; grep
case-insensitively for `quirk`,
which also turns up unnumbered ones in the bot, loot, ragfair and weather modules. Some numbers have no Rust
site because the quirk lives on the C# side or on no code at all. The behaviour these preserve is
deliberate; reverting one silently diverges from C#. The bare `:N` line numbers in those comments are
the 4.1.2 body the port was written against, not the current file.

**`BotGenerator`'s prelude draws — roadmap item 20 (ABI 38, landed 2026-08-30).** The last of
`Generators/Bot/BotGenerator.cs`'s per-bot draw work moves inside the batch call, in
`GenerateBotPrelude`'s statement order: `GetExperienceRewardForKillByDifficulty`, the voice draw,
health (`GenerateHealth`/`GetLowestHpBodyPart`), skills (`GenerateSkills` and its two randomisers),
the PMC branch of `SetRandomisedGameVersionAndCategory`, `SetBotAppearance` — then, after the
inventory, `GenerateBotFinish`'s `AddDogtagToBot`. **No new export
and no new root** — the item-20 rule was that it moves inside `spt_generate_bot_inventory_batch` or
it does not move, so the export count stays 43 and the wire grows only sibling fields:
`appearance`/`health`/`skills`/`experience.reward` on the template *variant* (per band, because
`BotEquipmentFilterService` mutates appearance per band), `isNikita` per bot, and
`skip_serializing_if` response members (`customization`, `health`, `skills`, `settingsExperience`,
`gameVersion`, `memberCategory`, `selectedMemberCategory`). The single-bot and player-scav *response*
wires are byte-identical to ABI 37 by construction — every new member is `skip_serializing_if` and
neither export sets one. Their *request* wires are not: three members are always serialized on the
override arm. `BotViewsOverride.BotRolesWithDogTags` and `BotViewsOverride.BodyToFixedHands` are
`required`, and `BotPayloadProjection.BuildViewsOverride` is shared by all three arms (wave batcher,
single-bot generator, player-scav request builder), so both ride every override-arm per-bot request —
`bodyToFixedHands` is ~77 entries ≈ 4 KB on shipped data. `BotSlice.IsNikita` likewise emits
`"isNikita":false` on the single-bot request (non-nullable `bool`, and `JsonUtil` only omits nulls).
All three are inert: `spt_generate_bot_inventory` and `spt_generate_player_scav` read neither
`BotViewsWire` member, and `BotSliceWire::is_nikita` carries `#[serde(default)]`. This is consistent
with how every other `BotViewsWire` member already behaves and is *not* a defect — but a slice that
grows the per-bot override request must re-verify it against
`BotPayloadSizeTests.RequestStaysUnderTheWireBudget`. New module `rust/spt-native/src/bot/bot_generator.rs`
(853 lines); `PmcConfigWire` lifts `gameVersionWeight`/`accountTypeWeight`/`dogtags`, `BotConfigLift`
lifts `botRolesWithDogTags`, and a `bodyTpl → handsTpl` derive view (`body_to_fixed_hands`, on the
`default_preset_ids_by_tpl` pattern) collapses `globals.config.Customization.Body` plus
`templates.customization` name resolution into one map.

- **Batch arm only, deliberately.** `spt_generate_bot_inventory` (the per-bot fallback) and
  `spt_generate_player_scav` keep their C# prelude untouched, so their streams and their goldens do
  not move — `player_scav_resident.rs`'s `RESIDENT_GOLDEN` (`78B74F37A38AEA0D85A10AF79B763A26`) is
  unchanged, and `PlayerScavParityTests`' identical-prelude invariant survives intact. Extending
  either arm is a queue follow-up, not this slice.
- **The "a changed non-PMC pin is a bug" invariant is retired on purpose.** Every batch bot — not
  just PMCs — now consumes prelude draws before its inventory draws, so the inventory stream shifts
  for every role. `RESIDENT_BATCH_GOLDEN` (`flip6_bots_resident.rs`) re-blessed **once**:
  `87A743ED988C6A8F7ADEE225F0E28062` → `8B40FC9288B1C75A329BB9D140040A15`. What must *not* move,
  and did not: the per-bot `(level, exp)` literals `(1,0)`, `(1,226)`, `(2,300)`, because nothing
  precedes the level draw in a bot's rayon task. A moved literal means a draw landed ahead of the
  level draw and is a bug, not a repin. The replacement invariant is in RUST-ROADMAP.md's batch
  section: the level/exp literals are the only cross-ABI-stable pins on this path.
- **Both arms' twins of the retired invariant were re-targeted, not deleted.** C#'s
  `BotBatchTests.BatchGeneratesTheSameBotsAsThePerBotPath` compared the serialized per-bot
  `.Inventory` across arms; its premise dies with this slice, so it becomes
  `BatchCarriesTheNativePreludeDrawsThePerBotPathOmits` — a contract test asserting the batch
  response carries all four native blocks and the single-bot response carries none of them (the
  `Is.Null` arm is what stops the single-bot wire growing a batch-only field by accident). Rust's
  `bot_inventory_generator.rs::a_non_pmc_bot_reports_level_one_and_no_exp` asserted the batch stream
  equalled the single-bot stream; it now pins the literals plus native-field presence, and
  `an_unheard_pmc_batch_bot_gets_the_extra_pocket_weights` is re-driven through `gameVersionWeight`
  because the native draw overwrites the `details.gameVersion` it used to be driven by. The
  single-bot `an_unheard_pmc_gets_the_tue_pockets` keeps its `details`-driven form — that arm still
  reads the wire value. *For the retired cross-arm inventory invariant* exact-output coverage did
  not shrink; it moved to per-arm seeded pins, the PR #24 pattern.
- **That is not a claim about the new C# consumption code, which a Rust golden cannot reach.**
  `BotWaveBatcher.BuildBotsFromEnvelopes` is new C# in this slice — `ToBotBaseHealth`'s per-part
  transform, `Enum.Parse<SkillTypes>`, the `MemberCategory` casts, the `Settings.Experience`
  assignment — and a golden that pins what the *native side drew* says nothing about what the
  caller then does with it. `BotWaveBatcherTests.TheConsumedPreludeValuesMatchTheirTemplateBands`
  closes it by driving a PMC wave over the template a `pmcUSEC` wave actually resolves to, the
  shipped `usec.json`: its single `min == max` `BodyParts` band makes all seven parts constants, so
  they are asserted exactly (distinct maxima across Head/Chest/Stomach and arms-vs-legs, so a key
  swap in `ToBotBaseHealth`'s `ToDictionary` fails); `Settings.Experience` lands in the `normal`
  band, against `base.json`'s `-1`; `SelectedMemberCategory == MemberCategory` carries a wave-level
  non-zero check so the equality is not vacuous against `base.json`'s `0`/`0`; and `usec.json`'s
  19-entry `skills.Common` is the **only** thing in either language that reaches
  `Enum.Parse<SkillTypes>`. Verified non-vacuous by deleting the `SelectedMemberCategory` write-back
  and watching it go red — the check that found this gap in review.
- **Writing that test surfaced the nikita quirk as a live wave outcome, and it is now pinned.**
  `SetRandomisedGameVersionAndCategory`'s special case assigns `GameVersion` and `MemberCategory`
  and returns *without* touching `SelectedMemberCategory`, so a nikita bot keeps `base.json`'s
  `Default` while its category is `Developer` — the one bot for which the two do not track. The
  Rust port reproduces it (`set_randomised_game_version_and_category` returns
  `selected_member_category: None` on that branch only; every other path returns
  `Some(member_category)`, and `bot_inventory_generator.rs` has the crate's only assignment of the
  field, so `None` alongside a non-`Developer` category is unreachable). `usec.json`'s 619-entry
  name pool carries `Nikita`, so a 12-bot PMC wave reaches the branch roughly one run in eight —
  which is how the first draft of the test found it. It is now an explicit arm asserting all three
  members, and the only end-to-end pin of the quirk in either language; the Rust unit tests pin the
  draw, not the consumption.
- **Naming is the documented carve-out and stays C# on every arm.** `BotNameService`'s
  `UsedNameCache` is a cross-wave, cross-arm singleton `HashSet<string>` that
  `LocationLifecycleService` clears per raid. Shipping it both ways per wave is unbounded wire
  growth that collides with `BotPayloadSizeTests.RequestStaysUnderTheWireBudget`, so the whole
  nickname path — `GenerateUniqueBotNickname`, `BotHelper.GetPmcNicknameOfMaxLength`, the locale
  prefix — stays where it is. Because naming stays, so does the sim-pscav cluster
  (`ShouldSimulatePlayerScav`, `AddRandomPmcNameToBotMainProfileNicknameProperty`, the sim-path
  `SetRandomisedGameVersionAndCategory`): it is naming-coupled, and it runs only for `assault`,
  which never reaches the PMC branch, so no double game-version draw arises. Native naming is a
  follow-up blocked on a `UsedNameCache` design, not a forgotten member.
- **Four booked divergences**, all in RUST-ROADMAP.md § *Broken*. The one that is not
  merely mod-data trivia: a null `Appearance.Head`/`Hands`/`Voice` map serialises as `[]` through
  `ArrayToObjectFactoryConverter` and fails the **whole batch request** deserialise, where legacy
  NRE'd one bot. Mod-data-only — all 57 shipped bot type files were scanned and none carries such a
  member — and `ForcePerBotGeneration`/`ForceLegacyBotGeneration` are the escape hatches, but it is
  a blast-radius change (wave, not bot) and was accepted on the strength of being written down
  here. The other three: a drawn body tpl absent from `templates.customization` raises
  `KeyNotFoundException` out of `SetBotAppearance`'s dictionary indexer on the legacy path and
  falls through to the ordinary weighted hands draw natively; an **unknown dogtag side, or a side
  with no `default` band**, NREs on the legacy path — `GetDogtagTplByGameVersionAndSide` discards
  both `TryGetValue` results, so an unknown side dereferences a null `gameVersionWeights` and a
  missing `default` hands `GetWeightedValue(null)` a null list — where the native arm returns a
  per-bot error envelope and the wave survives (the *game-version* key is **not** a divergence:
  both arms fall back to `default` on a miss, identically, and shipped `pmc.json` has no `standard`
  band at all, so `default` is already the live path for most PMCs on both arms); and the Debug log
  line
  `GetExperienceRewardForKillByDifficulty` writes on its `normal` fallback has no native
  counterpart.
- **No decline-set change and no new flag.** Every ported member is `protected` on `BotGenerator`
  and already swept by `BotWaveBatcher._hookableWaveMembers`' type-wide scan, so a patch on any of
  them already declined the batch — which stays exactly correct, since the patched body runs on the
  per-bot path. Dispatch rides the existing `nativeLevelAndFilter` flag on `GenerateBotPrelude`
  (the batcher is its only `true` caller and always wants both halves); `GenerateBotFinish` took a
  new `internal bool nativeDogtag = false`. Both are `internal`, so apicompat sees nothing —
  no public or protected signature changed in this slice.
- **`BotPayloadSizeTests`' budgets were not raised.** The four new per-band blocks were added to
  the fixture and the wire still fits under the existing budget; a raised budget would have needed
  its own argument, and did not arise.
- **Carryover: `BotWaveBatcher`'s `weightedRandomHelper` constructor parameter is now unread.**
  Deleting the post-call voice draw orphaned it, and it emits `CS9113: Parameter
  'weightedRandomHelper' is unread`. It was left in place deliberately: `BotWaveBatcher` is public,
  so removing a primary-constructor parameter is an apicompat surface change, and nine sibling
  CS9113s already sit in `SPTarkov.Server.Core`, spread over six classes — `ClientLogCallbacks`
  (three), `TraderController` (two), and one each on `ProfileController`,
  `RepeatableQuestNativeRequestBuilder`, `AbstractLocalisationService` and `PostDbLoadService` —
  with no `TreatWarningsAsErrors` anywhere in the tree. Removable at the next
  deliberate public-surface break.
- **`rust/spt-native/tests/phase4_configs_root.rs` stays `#[ignore]`d**, so the three `spt-pmc`
  wire-name pins this slice added (`gameVersionWeight`, `accountTypeWeight`, `dogtags`) do not run
  in `cargo test`. They were exercised by hand once at landing —
  `DbPublishFixtureTests.WriteConfigsRootFixture` regenerated the dump and the ignored test passed
  against it, which also confirms the shipped `spt-bot` stem carries `botRolesWithDogTags` (a
  strict member, so `configs.bot.is_some()` already gates its wire name; it needs no soft-member
  pin). The live in-suite guards on the same surface are
  `SptNativeBotWireTests.TemplateVariantBlocksSerialiseWithTheNamesTheNativeSideExpects` and
  `BotResidentDbTests.AResidentSendAndAnOverrideSendProduceIdenticalBotsFieldForField`.
- **Pre-existing flake, not caused by this slice.** `BotHookLivenessTests`' private helper
  `AssertPatchForcesLegacyPath` — not itself a `[Test]` — runs a bounded
  `for (i < MaxBots && !_patchFired)` loop over randomly generated bots and asserts `_patchFired`.
  Five `[Test]` methods call it: the `BotEquipmentModGenerator`, `BotWeaponGenerator`,
  `BotLootGenerator`, `BotInventoryGenerator` and `BotEquipmentModPoolService` variants of
  `HarmonyPatchOn…ForcesTheLegacyPath`. (A sixth,
  `HarmonyPatchOnAPoolPropertyGetterForcesTheLegacyPath`, calls `Generate()` once with no retry loop
  and is not flake-capable.) One of the five failed once in three `~Bot` filtered runs during review
  and passed on every run since. They drive the **legacy single-bot** path, which this branch cannot
  reach at all. Recorded so a future full-suite run seeing one red does not misread it as an item-20
  regression.
- **Review fix: the sim-pscav game-version draw now reaches the wire.**
  `GenerateBotPrelude`'s `ShouldSimulatePlayerScav` block is ungated by `nativeLevelAndFilter`, so
  it draws on the batch arm too — but it wrote only `bot.Info.GameVersion`, while this slice's
  native dogtag reads `details.game_version`, which `BuildBotSlice` ships as `""` for a non-PMC
  (the `if (!nativeLevelAndFilter)` block that assigns `botGenerationDetails.GameVersion` is
  PMC-only). The Rust-side overwrite from the draw is gated on `details.is_pmc`, so it never fired
  for these bots: `side_weights.get("")` missed and fell back to `dogtagSettings`' `default` band —
  a wrong rarity, not a crash. Dead on shipped config in both directions (`botRolesWithDogTags` is
  `pmcbear`/`pmcusec`; `ShouldSimulatePlayerScav` fires on `assault` alone) and one config edit from
  live. Fixed by assigning `SetRandomisedGameVersionAndCategory`'s return value into
  `botGenerationDetails.GameVersion`, which is a no-op on the legacy arm — that arm's
  `AddDogtagToBot` reads `bot.Info` directly, and the only other reader of the details field,
  `GetPocketPoolByGameEdition`, is `isPmc`-gated on both sides.
- **Review fix: `entry.Details.GameVersion = native.GameVersion` deleted as dead.** `entry.Details`
  is a per-bot clone; every read after the write-back is of a different member
  (`ClearBotContainerCacheAfterGeneration`, `ReplayRandomisationClamps`' `RoleLowercase`/`BotLevel`,
  `GenerateBotFinish`'s `EventRole`), and `GenerateBotFinish`'s only `RoleLowercase` read sits in
  the `!nativeDogtag` branch the batch arm never takes.
- **Three follow-ups deferred** (also queued in todo/TODO.md's *Removed from this file*): extend
  the prelude draws to `spt_generate_bot_inventory` (the per-bot arm); extend them to
  `spt_generate_player_scav` (arm C — reworks `PlayerScavParityTests`' identical-prelude
  invariant); and native naming, blocked on a `UsedNameCache` design.

## Pull-request ledger

Everything integrates on `dev`; `origin/main` is a stale snapshot, so a PR based there diffs the
whole porting history. Carryovers deliberately deferred out of a PR live on that PR (body or review
comment); this list records where, plus the deferred items recorded nowhere else in the repo.

- **PR #9 — flip #6, bots resident** (2026-08-19, `5c94484`, ABI 27). Open: the single-bot dispatch
  site's stale-epoch *retry* branch has no end-to-end test — the self-heal is covered on the batcher
  and every other family.
- **PR #10 — Phase 2, write barriers** (2026-08-19, `e265396`, no bump). Deferred, named only there:
  `_denied` name validation in `WriteBarriersPatch` (a renamed model type silently drops off the
  denylist); `WriteBarrier.Suppress()` inside `ParseFramedOffers`' `Parallel.For` body (worker
  threads are unsuppressed — safe today only because `RagfairOffer` holds the denied `Item`).
- **PR #11 — native console ownership** (2026-08-19, `8927eea`, ABI 28; follow-ups at `250cb72`).
  Open: a real Windows run; the `SendError(Raw)` shutdown-race recovery arm is untested (correct by
  inspection).
- **PR #12 — Phase 3, native SPT_Data load** (2026-08-20, `9222c49`, ABI 29). Carryovers on the
  merge comment; the sharp one: any future classify/`LOCATION_MEMBERS` equivalence gate must compare
  **values, not key sets** — the projection serializes post-transformer state while `db::load`
  splices raw file bytes.
- **PR #13 — Assets project removal** (2026-08-20, `b8a4e11`). Carryovers on a PR comment; a Windows
  Release run is still owed.
- **PR #14 — Phase 4, configs resident** (2026-08-20, `1ce0ee0`, ABI 30; review fixes at `f876ec1`).
  No open carryovers.
- **PR #15/#16 — Phase 5, profile persistence** (2026-08-21, `ba96141`/`1ca26af`, ABI 31). Open: one
  never-identified flaky `dotnet test` failure (754/1/23 once; lead suspect is `BackupService`'s
  timer enumerating `user/profiles/` while fixtures write it); pre-existing:
  `BackupService.RestoreProfile` throws `DirectoryNotFoundException` instead of returning false when
  `user/profiles/backups` is missing, and `LoadAsync` runs before `StartBackupSystem`.
- **PR #17 — Phase 6b, rlib linkage flip** (2026-08-22, `80f37ba`, no bump). Carried-forward items
  are in the Phase 6b ledger above.
- **PR #18 — bot mod-pool ownership** (2026-08-25, `b563367`, ABI 32). Declined on the PR:
  substitution-decline tests for the four `GetType` checks in `UseLegacyPath` (the three
  pre-existing sibling checks are equally untested).
- **PR #19 — db-load epoch seed** (2026-08-26, `a8535ba`, ABI 33). A 20-item carryover ledger on the
  PR review comment. Pre-existing and issue-worthy: `forceLegacyDatabaseImport` on a Release tree
  trips `checks.dat` verification while the in-code hint recommends exactly that flag.
- **PR #20 — equipment split** (2026-08-27, `67253c08`, ABI 34). Booked on the PR's second carryover
  comment: the silent unmatched-overlay drop has no diagnostic; one red DB test poisons
  `DB_TEST_LOCK` into ~40 phantom failures; `ResidentDbDispatch.Eligible`'s modless arm never checks
  `WriteBarrier.Installed` (Debug zero-mod freshness gap); W3/W5 fire on stock data. The W3/W5
  alias-break stays declined and owes its own spec plus parity gate.
- **PR #21 — map/raid setup** (2026-08-27, `18a2b70`, ABI 35). Carryovers on the PR body (deferred
  tests, the ASCII-vs-Ordinal case-fold convention notes, stale line refs).
- **PR #22 — tier 1 tail** (2026-08-28, `1f05a86`, ABI 36). Carryovers on the PR's disposition
  comment.
- **PR #23 — player scav port** (2026-08-29, `c633991`, ABI 37). Natives the pscav generation body:
  karma equipment/mod chances, the equipment blacklist, the inventory build and the additional-loot
  pass. The profile boundary stayed C#-side on both arms — karma-value reads, the fence/limit
  hydration onto the crossing template, and every profile write. Follow-ups from the final review
  were booked as roadmap item 7, retired by PR #24.
- **PR #24 — pscav follow-ups** (2026-08-29, `4db6fc7`, no bump). Retires roadmap item 7. Carryovers,
  all review-triaged as ride-able: `PlayerScavResidentDbTests`' per-test `finally` hard-codes the
  kill switch to `false` instead of the captured original (wrong the moment a second `[Test]` is
  added); three of the four karma-map wire pins are key-presence-only; the hook-liveness
  both-arms filter closes the overload hole (identity, not name) but a member *moving* native while
  keeping its line still subtracts silently (documented on the field); the downward direction of
  `Modifiers.Mod` is unexercisable cross-arm at the parity seed (every filled MP-133 slot is
  `_required`). One
  unreproduced single-test failure in one of six full-suite runs (name lost to an output pipe;
  three logged re-runs green, fixture teardown audit clean).
