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
| [`RUST-ROADMAP.md`](RUST-ROADMAP.md) | Rust port status: what works, what flips to legacy, known divergences, roadmap. |
| [`BENCHMARK.md`](BENCHMARK.md) | Native vs legacy timings. Every measurement lives there; none are repeated elsewhere. |

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

- `MongoId` — 24-hex-char id for every item, template and profile key; the single most-referenced
  type in Core.
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
(polled every 5 s by `SPTStartupHostedService`), `IOnDIConstruct` (static hook letting a mod add
registrations).

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

`SPTarkov.Server.Assets` hashes `SPT_Data` into `checks.dat` on Release builds, which
`DatabaseImporter` verifies at startup outside DEBUG. The format is a contract shared with
`rust/spt-native/src/verify.rs`; scope is manifest-driven and exact in both directions over
`configs/` and `database/`, so deletions and swaps are caught, while `images/` is unverified.

## Native Rust layer

`rust/spt-native` is a `cdylib` called over C ABI from `Libraries/SPTarkov.Server.Core/Native/`
(`NativeMethods.cs`, `SptNative.cs`) — and, for the log exports, from the twin
`Libraries/SPTarkov.Common/Native/NativeMethods.cs`, because `SPTarkov.Common` cannot reference
Server.Core. It owns database hash verification, the ported generation paths (location loot, reward
loot, whole-bot inventory, dynamic ragfair offers, repeatable quests, scav case rewards), the item
base-class cache build, the ragfair linked-item table, and the whole log pipeline. Twenty-two
exports, JSON in / JSON out — except the ragfair response, a framed MessagePack envelope, and the
log exports — with `spt_native_abi_version` handshaking against `SptNative.ExpectedAbiVersion`.

Every ported *class* keeps its complete 4.1.2 C# implementation as a **legacy path**, taken
automatically when a mod hooks it or forced by config, so a Rust cutover never removes a mod's
extension point. `DatabaseImporter` calls `SptNative.EnsureLoadable()` on every startup, so a missing
or ABI-mismatched library fails fast. The one exception is logging, which has no legacy path:
`AddSptLogger` initialises the native pipeline and `SPTLoggerDispatcher.Log` emits straight into it,
with mod `ILogHandler`s fanned out from the dispatcher (resolve it from DI and call `RegisterHandler`
— registering an `ILogHandler` in the host container alone never reaches it). It is failure-tolerant
by contract: a broken library or config produces one stderr notice and logging stays off rather than
stopping the server.

Payloads are projected from the live database on every call, except for the call-invariant halves of
the ragfair and repeatable-quest requests, which are resent only when `DatabaseMutationStamp` has
moved. Because a mod writing an injected table directly never reaches those bump sites, the skip is
gated on no mods being loaded, with opt-in and kill-switch flags per family; a cache miss self-heals
by resending.

Native is not uniformly faster — several families are slower than the C# they replace and stay the
default anyway, each with a force-legacy flag. The argument per family is in
[`BENCHMARK.md`](BENCHMARK.md), next to the numbers.

Build coupling: `BuildSptNative` shells out to `cargo build` before compiling, so **`cargo` on
`PATH` is a hard build dependency**. Cross-RID builds need `-p:SptNativeRid=<rid>`; only `linux-x64`
is mapped.

→ [`rust/ARCHITECTURE.md`](rust/ARCHITECTURE.md) for the crate internals and the FFI contract.
→ [`RUST-ROADMAP.md`](RUST-ROADMAP.md) § *Exceptions in force* for what flips each family to legacy,
and § *Broken / known divergences* for the mod-facing limits.
