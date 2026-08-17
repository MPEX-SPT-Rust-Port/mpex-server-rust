# Architecture

How the SPT server is put together — a map, not a manual. For build/run commands see
[CLAUDE.md](CLAUDE.md).

Detail lives in the per-directory documents:

| Document | Covers |
|---|---|
| [`SPTarkov.Server/ARCHITECTURE.md`](SPTarkov.Server/ARCHITECTURE.md) | The executable host: startup sequence, mod loader, web pipeline hookup. No game logic. |
| [`Libraries/ARCHITECTURE.md`](Libraries/ARCHITECTURE.md) | Which of the six library projects owns what, and what they depend on. |
| [`Libraries/SPTarkov.Server.Core/ARCHITECTURE.md`](Libraries/SPTarkov.Server.Core/ARCHITECTURE.md) | Everything inside Core (~91% of the code): folder map, dispatch mechanics, item events, configs, models. |
| [`rust/ARCHITECTURE.md`](rust/ARCHITECTURE.md) | Inside the `spt-native` crate: modules, FFI boundary, porting conventions, tests. |
| [`RUST-ROADMAP.md`](RUST-ROADMAP.md) | Rust port status: what works, what's broken, guidelines, roadmap. |
| [`BENCHMARK.md`](BENCHMARK.md) | Native vs legacy timings. |

## Solution layout

| Project | Role |
|---|---|
| `SPTarkov.Server` | Executable host: `Program.cs`, mod-loading bootstrap, the single catch-all HTTP middleware |
| `Libraries/SPTarkov.Server.Core` | All game logic — everything below lives here unless noted |
| `Libraries/SPTarkov.Server.Web` | Blazor Server admin panel (MudBlazor) |
| `Libraries/SPTarkov.Server.Assets` | `SPT_Data/`: configs, JSON database, images; the largest files ship compressed as `looseLoot.7z` |
| `Libraries/SPTarkov.DI` | Attribute-driven DI container: `[Injectable]`, `DependencyInjectionHandler` |
| `Libraries/SPTarkov.Common` | Shared primitives and the logging front end (`SptLogger`, `SPTLoggerDispatcher`) |
| `Libraries/SPTarkov.Reflection` | Runtime method patching for mods (`AbstractPatch`, `PatchManager`) |
| `rust/` | Cargo workspace: the `spt-native` cdylib called over C ABI |
| `Tools/Ceciler` + `Patches/Ceciler.JsonExtensionData` | Mono.Cecil IL rewriter run on Release builds, and the patch assembly it applies |
| `Tools/MongoIdTplGenerator`, `Tools/JsonExtensionDataGenerator`, `Tools/HideoutCraftQuestIdGenerator` | Dev-time one-shot generators |
| `Testing/UnitTests` | NUnit suite |
| `Testing/TestMod`, `Testing/TestMod2` | Reference mod implementations |

Folder map inside `SPTarkov.Server.Core`: `Callbacks/` (HTTP entry per domain), `Controllers/`
(orchestration), `Services/`, `Helpers/`, `Generators/` (logic, grouped by domain), `Routers/`
(URL → callback dispatch), `Servers/` (HTTP, WebSocket, save, ragfair state), `Loaders/`
(`ConfigLoader`, `BundleLoader`; `ModLoader` itself is in the host), `Migration/` (profile
migrations), `Models/` (`Eft/` mirrors client contracts, `Spt/` is server-internal, `Common/` shared
primitives), `DI/` (lifecycle interfaces + router base classes), `Native/` (the `spt-native` P/Invoke
wrapper), `Utils/`, `Constants/`, `Extensions/`, `Exceptions/`.

## Request pipeline

ASP.NET Core, but **no MVC controllers and no attribute routing**. `Program.cs` installs one
catch-all middleware that hands every request to `HttpServer.HandleRequestAsync`:

```
Kestrel (HTTPS, self-generated cert)
  → catch-all middleware → HttpServer.HandleRequestAsync → SptHttpListener
  → HttpRouter → StaticRouter (exact URL match) or DynamicRouter (substring match)
  → *Callbacks   (deserialize request, serialize response via HttpResponseUtil)
  → *Controller  (orchestration)
  → Services / Helpers / Generators (logic)  ←→  database tables (DI singletons)
```

Routers are declarative: a subclass passes `RouteAction<TRequest>` records to its base constructor.
Adding an endpoint means touching the router, the callback, and usually a controller.

Router families under `Routers/`: `Static/` and `Dynamic/` (the path above); `ItemEvents/`
(`/client/game/profile/items/moving` batches, accumulated into one `ItemEventRouterResponse` by
`EventOutputHolder`); `SaveLoad/` (run on profile load to patch saved data); `Serializers/`
(non-JSON responses); `ImageRouter.cs` (a second `IHttpListener` serving registered images).

Four routes skip all of this as minimal APIs: `/health` (`Program.cs`) and the admin panel's login,
logout and profile-download routes (`SPTarkov.Server.Web/SPTWeb.cs`).

## Core abstractions

- `MongoId` (~1,300 references) — 24-hex-char id for every item, template and profile key.
- `PmcData` — the player character: inventory, quests, hideout, stats, skills.
- `Item` — one inventory item instance (template id, location, upd state).
- `ItemEventRouterResponse` — the accumulated client-sync diff from item-event actions.
- `Models/Spt/Config/*` — one `BaseConfig` subclass per file in `SPT_Data/configs/`, mapped by
  `ConfigTypes` and loaded by `ConfigLoader`.

JSON is System.Text.Json with custom converters in `Utils/Json/` (`ListOrT`, `StringOrInt`,
`DictionaryOrList`, `LazyLoad` for deferred loading of huge database files).

## Dependency injection and lifecycle

Classes opt in with `[Injectable]`; `DependencyInjectionHandler` registers each type against itself,
its interfaces and its base types. `ProgramHelpers.RegisterSptServicesAsync` is the single place
services get registered, and `DependencyInjectionValidationTests` rebuilds that exact container
(mods on and off) so a bad registration fails the test run, not a launch.

Lifecycle interfaces in `DI/`: `IOnLoad` (startup, ordered by `OnLoadOrder`; anything below
`GameCallbacks` runs before Kestrel binds, which is what lets mods mutate `HttpConfig`), `IOnUpdate`
(polled every 5 s), `IOnDIConstruct` (static hook letting a mod add registrations).

## Startup order

`ProgramStatics.Initialize` → early logger → `ConfigLoader` → throwaway "early" provider →
`ModLoader` (validate, prepatch, load assemblies) → `DatabaseImporter` (hash-verified outside DEBUG)
→ real `WebApplicationBuilder` with database tables as singletons → pre-SPT-load callbacks → Kestrel.

The mod-loading split in `Program.StartServerAfterModLoading` is deliberate: merging it back breaks
prepatching by forcing types into context too early.

## Persistence

`Servers/SaveServer.cs` owns the on-disk JSON profiles in `user/profiles/`. `SaveCallbacks` loads
every profile at startup and saves on the `CoreConfig.ProfileSaveIntervalInSeconds` interval;
`BackupService` takes timer-driven backups per `BackupConfig`.

Old profile data is patched two ways on load: `SaveLoadRouter` subclasses (always-on structural
fixes) and versioned `IProfileMigration` implementations under `Migration/`, orchestrated by
`ProfileMigrationService`. `ProfileFixerService` repairs known corruption beyond that.

## WebSockets and notifications

`Servers/WebSocketServer.cs` accepts upgrades and dispatches to `IWebSocketConnectionHandler`
implementations; `SptWebSocketConnectionHandler` tracks live per-session sockets.
`NotificationSendHelper` sends immediately or queues for next poll; `MailSendService` builds
NPC/system mail on top. Payload types live in `Servers/Ws/Message/` and `Models/Eft/Ws/`.

## Admin panel

`SPTarkov.Server.Web`, a Blazor Server app served by the same host: profile editing, live config
editing, database browsing, MongoId tools, all behind `AuthService` login. See
[`Libraries/ARCHITECTURE.md`](Libraries/ARCHITECTURE.md) for the page and component breakdown.

## Mods

A mod DLL in `user/mods/` implements exactly one `IModMetadata` (GUID, semver, `SptVersion` range,
dependencies, incompatibilities) plus any number of `[Injectable]` classes.
`Testing/TestMod` is the reference implementation.

Replacing a core registration is the sharp edge: `[Injectable]` defaults `TypePriority` to
`int.MaxValue` and the core assembly is scanned *after* mod assemblies, so `TypePriority` cannot put
a mod ahead of a core service. Register from `IOnDIConstruct.OnDIConstructAsync` (which runs last) or
patch the type at runtime instead. Substituting an implementation only changes behaviour where the
call site goes through an interface or a `virtual` member.

`HasPrepatcher = true` opts into enum prepatching from `user/patchers/{ModGuid}`. Runtime method
patching uses `SPTarkov.Reflection` (`AbstractPatch`/`PatchManager`).

## Build-time code generation and data integrity

Two non-obvious steps run during build, both in `SPTarkov.Server.Core.csproj`:

- `GenerateProgramStatics` writes `Utils/ProgramStatics.Generated.cs` from MSBuild properties
  (defaults in `Build.props`). Never edit it.
- On Release/publish, `Tools/Ceciler` rewrites the compiled `SPTarkov.Server.Core.dll` with
  Mono.Cecil, injecting a `[JsonExtensionData]` property into every type under `Models` so unknown
  client JSON round-trips instead of being dropped. Release binaries therefore differ structurally
  from Debug ones; `PrepatchIsolationTests` guards it.

`SPTarkov.Server.Assets` hashes `SPT_Data` into `checks.dat` on Release builds (base64 JSON of
`{Path, Hash}` pairs, XXH3-128, canonical big-endian hex), which `DatabaseImporter` verifies at
startup outside DEBUG. That format is a contract shared with `rust/spt-native/src/verify.rs`.

## Native Rust layer

`rust/spt-native` is a `cdylib` called over C ABI from `Libraries/SPTarkov.Server.Core/Native/`
(`NativeMethods.cs`, `SptNative.cs`) — and, for the log exports, from the twin
`Libraries/SPTarkov.Common/Native/NativeMethods.cs`, because `SPTarkov.Common` cannot reference
Server.Core. It owns database hash verification, the ported generation
paths — location loot, reward loot, whole-bot inventory, dynamic ragfair offers, repeatable quests,
scav case rewards — and the whole log pipeline. Eighteen exports, JSON in / JSON out — except the
ragfair response, which comes back as a framed MessagePack envelope, and the log exports, where
`spt_logger_init` takes the raw `sptLogger.json` bytes and `spt_log_emit` passes one line's fields
directly — with `spt_native_abi_version` handshaking against `SptNative.ExpectedAbiVersion`.

Payloads are projected from the live database on every call, with one exception: the ragfair
request's call-invariant half is sent only when `DatabaseMutationStamp` — a singleton the
instrumented mutation paths bump — has moved since the last send, and the Rust side keeps the
parsed copy keyed by that stamp. Because a mod writing an injected table directly never reaches
those bump sites, the skip is gated on no mods being loaded, with opt-in and kill-switch flags on
`RagfairConfig`; a cache miss self-heals by resending. See `RUST-ROADMAP.md` § *Exceptions in force*.

Every ported *generator* keeps its complete 4.1.2 C# implementation as a **legacy path**, taken
automatically when a mod hooks it or forced by config — so a Rust cutover never removes a mod's
extension point. `DatabaseImporter` calls `SptNative.EnsureLoadable()` on every startup, so a missing
or ABI-mismatched library fails fast.

Logging is the exception with no legacy path: `AddSptLogger` initialises the native pipeline from the
raw `sptLogger.json` bytes and `SPTLoggerDispatcher.Log` emits straight into it, so the C# side keeps
only the `ISptLogger`/`SptLogger` front end. It is failure-tolerant by contract — a broken library or
config produces one stderr notice and logging stays off rather than stopping the server. The ported
generators log into that pipeline directly rather than handing lines back for C# to replay, so
`DatabaseImporter` pushes the resolved server locales over `spt_locales_set` before anything can generate;
a failed push is one stderr notice and generator lines fall back to their locale keys.

Verification scope is manifest-driven and exact in both directions (`configs/`, `database/`), so
deletions and symlink swaps are caught; `images/` and build-relocated artifacts are unverified by
construction.

Build coupling: `BuildSptNative` shells out to `cargo build` before compiling, so **`cargo` on
`PATH` is a hard build dependency**. Cross-RID builds need `-p:SptNativeRid=<rid>`; only `linux-x64`
is mapped.

→ [`rust/ARCHITECTURE.md`](rust/ARCHITECTURE.md) for the crate internals and the FFI contract.
→ [`RUST-ROADMAP.md`](RUST-ROADMAP.md) for port status, known divergences, porting rules and roadmap.

### Bot wave dispatch

Bot generation has three tiers, chosen per wave in `BotController.GenerateBotWave`. First choice is
the **batch**: `BotWaveBatcher.TryGenerateWave` projects the shared views once and generates the
whole wave in a single `spt_generate_bot_inventory_batch` call (rayon-parallel on the Rust side, one
`{result | error}` envelope per bot in request order — a failed bot is skipped with a Critical log
and the wave continues). If the batcher declines it returns null, and the unchanged **per-bot**
native path runs, `.AsParallel()` over `BotInventoryGenerator.GenerateInventory`; that dispatcher in
turn falls to the **legacy** 4.1.2 C# under its own existing conditions.

The batch declines on any of: `BotConfig.ForcePerBotGeneration`; anything that would send
`GenerateInventory` to legacy anyway (`BotInventoryGenerator.UseLegacyPath()`, which includes
`BotConfig.ForceLegacyBotGeneration`); a live Harmony patch on any frozen 4.1.2 member of
`BotGenerator` or `BotController` except `GenerateBotWave` itself; a container-substituted
`BotGenerator` subclass; or a wave that could write nighttime equipment clamps — a night raid where
the wave role's equipment config carries `NighttimeChanges.EquipmentModsModifiers` in some
randomisation band, because that clamp is a cross-bot feedback loop only the per-bot path replays. A
mod subclassing `BotController` through the frozen 14-parameter constructor gets a null batcher and
never batches; one built through the new 15-parameter constructor gets a batcher but is still
blocked by the `GetType() == typeof(BotController)` guard, so a subclass never batches either way.
The two escape hatches are config flags: `forcePerBotGeneration` (batch off, native per-bot on) and
`forceLegacyBotGeneration` (native off entirely).

### Ragfair offer generation

One export, `spt_generate_dynamic_offers`, called **once per batch** from
`RagfairOfferGenerator.GenerateDynamicOffers` — its two callers are `RagfairServer.Load()` (the
startup full pass, ~58k offers generated, ~24k accepted past the holder's cap) and
`RagfairServer.Update()`'s expired-offer regeneration. The call folds in three collaborators:
`RagfairPriceService`, `RagfairServerHelper`, `RagfairAssortGenerator`. Uniquely among the exports
it answers with a framed MessagePack envelope — one frame per offer, deserialised in parallel —
rather than one JSON buffer.

What stays C#: the `AddOffer` insert loop and the holder's live per-template cap; the player-offer
path (`CreateAndAddFleaOffer`/`CreateOffer`); `GenerateFleaOffersForTrader`. `RagfairPriceService`
remains a normal class — the ragfair, trader, insurance, scav-case and PMC-loot paths all still call
it directly, only its dynamic-offer slice is mirrored in Rust.

Dispatch to legacy on any of: `RagfairConfig.ForceLegacyRagfairGeneration` (no constructor change —
`RagfairConfig` was already injected); a live Harmony patch on any public/protected/protected-internal
member of those **four** classes except `GenerateDynamicOffers` itself (the dispatcher is excluded
because a patch there wraps whichever path runs; every other member is never called natively, so a
patch on one would silently do nothing); or a container-substituted subclass of any of the three
collaborators.

Two pieces of state come back for C# to apply: `rejectedCanSellTemplates` is replayed onto the live
`templateTable` as `CanSellOnRagfair = false` before the insert loop, and `OfferCounter` advances by
the number of offers *created*, not the number the holder accepted.

Mod-facing limitations:

- Patches on the deep shared helpers do **not** reach the native path and do **not** flip it to
  legacy: `RandomUtil`, `ItemHelper`, `HandbookHelper`, `PresetHelper`, `PaymentHelper`, `BotHelper`,
  `WeightedRandomHelper`, `TraderHelper`, `ItemFilterService`, `SeasonalEventService`, `ICloner`.
- Native generates the whole batch before insertion where legacy interleaves generation and
  insertion. Distribution-identical under the production RNG; a mod counting on interleaving sees a
  different order of `AddOffer` calls.
- Runtime config, price and blacklist mutations stay visible, because the payload is projected per
  call and never cached.
- `AllowedFleaPriceItemsForBarter` is a per-instance C# cache that is never invalidated; the native
  path re-derives it per call, so it is **fresher** than legacy for items added at runtime. A
  documented divergence, not a bug.
- `customMoneyTpls` (mod-added currencies) are not projected — offers priced in one are routed
  through the unrounded arm.
- A mod that assigns `RagfairConfig.Dynamic.GenerateBaseFleaPrices = null` makes the serialiser omit
  the member (`WhenWritingNull`), and the native request parse fails on the missing field, aborting
  the whole pass. Legacy only dereferences it on the weapon-preset arm, so it survives the null for
  every other offer. `ForceLegacyRagfairGeneration` is the workaround.

Two sanctioned divergences this port added: an **unseeded** assort walk fans across rayon like
legacy's per-entry task fan-out while a **seeded** walk stays sequential (that is what keeps the
parity tests byte-equal), and the batch takes **one timestamp**, where legacy stamps each offer as
it is built (so `startTime`/`endTime` fold to the batch clock plus the per-offer spread).

Performance: native still **loses** here — 1.47x on the full pass (626 ms vs 427 ms) and 7.3x on
regeneration (75 ms vs 10 ms), see [`BENCHMARK.md`](BENCHMARK.md). Generation is no longer the
problem: it is ~85 ms of a ~626 ms pass, and the payload projection ~13 ms. What is left is the C#
side binding the ~58k offers the pass produces out of the response. Absolute cost is small (startup,
then per-expiry bursts) and native stays the default for family consistency, with
`ForceLegacyRagfairGeneration` as the one-line opt-out.

→ [`rust/ARCHITECTURE.md`](rust/ARCHITECTURE.md) for the crate internals and the FFI contract.
→ [`RUST-ROADMAP.md`](RUST-ROADMAP.md) for port status, known divergences, porting rules and roadmap.
