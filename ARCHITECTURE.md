# **mpex-server-rust** Architecture

## 1. Overview Summary

How the SPT server is put together — a map, not a manual. A .NET solution hosting the Escape from Tarkov
private server, with the hot generation paths ported to a Rust `cdylib` called over C ABI. One executable host
brings up logging, configs, mods and the JSON database, then hands every HTTP request to a single catch-all
middleware that routes it through declarative routers to callbacks, controllers and the logic layers. For
build/run commands see [CLAUDE.md](CLAUDE.md).

---

## 2. High Level Design

| Component | Responsibility | Interacts With |
|-----------|-----------------|------------------|
| `SPTarkov.Server` | Executable host: `Program.cs`, mod-loading bootstrap, the single catch-all HTTP middleware | Everything; owns startup order and Kestrel |
| `Libraries/SPTarkov.Server.Core` | All game logic — everything below lives here unless noted | `.Common`, `.DI`, `rust/spt-native` |
| `Libraries/SPTarkov.Server.Web` | Blazor Server admin panel (MudBlazor) | `.Server.Core`, the shared Kestrel host |
| `Libraries/SPTarkov.Server.Assets` | `SPT_Data/`: configs, JSON database, images; the largest files ship compressed as `looseLoot.7z` | The host build (copies to output), `DatabaseImporter` |
| `Libraries/SPTarkov.DI` | Attribute-driven DI container: `[Injectable]`, `DependencyInjectionHandler` | Core, `.Reflection`, the host |
| `Libraries/SPTarkov.Common` | Shared primitives and the logging front end (`SptLogger`, `SPTLoggerDispatcher`) | `rust/spt-native` (log and console exports), Core |
| `Libraries/SPTarkov.Reflection` | Runtime method patching for mods (`AbstractPatch`, `PatchManager`) | Mods, the host (not Core) |
| `rust/` | Three-member Cargo workspace: the `spt-native` cdylib called over C ABI, `mpex-server`, the CLR-hosting launcher shipped builds run, and `spectre-facade`, which emits the stub `Spectre.Console.Ansi` assembly `SPTarkov.Common` builds and four other projects reference | Core's `Native/`, Common's `Native/`, the published server assembly |
| `Tools/Ceciler` + `Patches/Ceciler.JsonExtensionData` | Mono.Cecil IL rewriter run on Release builds, and the patch assembly it applies | `SPTarkov.Server.Core.dll` post-compile |
| `Tools/MongoIdTplGenerator`, `Tools/JsonExtensionDataGenerator`, `Tools/HideoutCraftQuestIdGenerator` | Dev-time one-shot generators, run by hand and committed | Core's `Models/Enums/`, Core's `Models/` tree, and `SPT_Data/database/hideout/production.json` respectively |
| `Testing/UnitTests` | NUnit suite | Every project |
| `Testing/TestMod`, `Testing/TestMod2` | Reference mod implementations | The mod loader |

Folder map inside `SPTarkov.Server.Core`: `Callbacks/` (HTTP entry per domain), `Controllers/`
(orchestration), `Services/`, `Helpers/`, `Generators/` (logic, grouped by domain), `Routers/`
(URL → callback dispatch), `Servers/` (HTTP, WebSocket, save, ragfair state), `Loaders/`
(`ConfigLoader`, `BundleLoader`; `ModLoader` itself is in the host), `Migration/` (profile
migrations), `Models/` (`Eft/` mirrors client contracts, `Spt/` is server-internal, `Common/` shared
primitives), `DI/` (lifecycle interfaces + router base classes), `Native/` (the `spt-native` P/Invoke
wrapper), `Utils/`, `Constants/`, `Extensions/`, `Exceptions/`.

### Request pipeline

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

Four routes are minimal APIs instead: `/health` (`Program.cs`) and the admin panel's login, logout
and profile-download routes (`SPTarkov.Server.Web/SPTWeb.cs`). They do not bypass the catch-all —
`ConfigureWebApp` registers it ahead of `UseSptBlazor()`, so every request enters
`HandleRequestAsync` first; no `IHttpListener` claims them, `next` runs, and routing dispatches them
from there. Same path the Blazor panel takes.

### Startup order

`ProgramStatics.Initialize` → early logger → `ConfigLoader` → throwaway "early" provider →
`ModLoader` (validate, prepatch, load assemblies) → `DatabaseImporter` (hash-verified outside DEBUG)
→ real `WebApplicationBuilder` with database tables as singletons → pre-SPT-load callbacks → Kestrel.

The mod-loading split in `Program.StartServerAfterModLoading` is deliberate: merging it back breaks
prepatching by forcing types into context too early. `PrepatchIsolationTests` guards the specific
failure — a prepatcher re-hosts the server in its own `AssemblyLoadContext`, so anything reflecting
over `SPT.Server` before that decision must not pull `SPTarkov.Server.Web` into the default context;
a duplicate there kills every Blazor circuit and the admin panel goes dead.

Shipped builds launch through `rust/mpex-server`, a thin Rust executable that hosts the CLR via
netcorehost and `run_app`s the published server assembly with argv forwarded — the C# code,
FFI boundary, and mod contract are unchanged, and dev builds still run the `SPT.Server`
executable directly. `Containerfile.release`'s entrypoint is `/app/mpex-server`.

---

## 3. Low Level Design

### Core abstractions

- `MongoId` — 24-hex-char id for every item, template and profile key; the single most-referenced
  type in Core.
- `PmcData` — the player character: inventory, quests, hideout, stats, skills.
- `Item` — one inventory item instance (template id, location, upd state).
- `ItemEventRouterResponse` — the accumulated client-sync diff from item-event actions.
- `Models/Spt/Config/*` — one `BaseConfig` subclass per file in `SPT_Data/configs/`, mapped by
  `ConfigTypes` and loaded by `ConfigLoader`.

JSON is System.Text.Json with custom converters in `Utils/Json/` (`ListOrT`, `StringOrInt`,
`DictionaryOrList`, `LazyLoad` for deferred loading of huge database files).

### Dependency injection and lifecycle

Classes opt in with `[Injectable]`; `DependencyInjectionHandler` registers each type against itself,
its interfaces and its base types. `ProgramHelpers.RegisterSptServicesAsync` is the single place
services get registered, and `DependencyInjectionValidationTests` rebuilds that exact container
(mods on and off) so a bad registration fails the test run, not a launch.

Lifecycle interfaces in `DI/`: `IOnLoad` (startup, ordered by `OnLoadOrder`; anything below
`GameCallbacks` runs before Kestrel binds, which is what lets mods mutate `HttpConfig`), `IOnUpdate`
(polled every 5 s by `SPTStartupHostedService`), `IOnDIConstruct` (static hook letting a mod add
registrations).

### Persistence

`Servers/SaveServer.cs` owns the on-disk JSON profiles in `user/profiles/`. `SaveCallbacks` loads
every profile at startup and saves on the `CoreConfig.ProfileSaveIntervalInSeconds` interval;
`BackupService` takes timer-driven backups per `BackupConfig`.

Old profile data is patched two ways on load: `SaveLoadRouter` subclasses (always-on structural
fixes) and versioned `IProfileMigration` implementations under `Migration/`, orchestrated by
`ProfileMigrationService`. `ProfileFixerService` repairs known corruption beyond that.

### WebSockets and notifications

`Servers/WebSocketServer.cs` accepts upgrades and dispatches to `IWebSocketConnectionHandler`
implementations; `SptWebSocketConnectionHandler` tracks live per-session sockets.
`NotificationSendHelper` sends immediately or queues for next poll; `MailSendService` builds
NPC/system mail on top. Payload types live in `Models/Eft/Ws/`; the message-handler seam in
`Servers/Ws/Message/`.

### Admin panel

`SPTarkov.Server.Web`, a Blazor Server app served by the same host: profile editing, live config
editing, database browsing, MongoId tools, all behind `AuthService` login. See
[`Libraries/ARCHITECTURE.md`](Libraries/ARCHITECTURE.md) for the page and component breakdown.

### Mods

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

### Build-time code generation and data integrity

Two non-obvious steps run during build, both in `SPTarkov.Server.Core.csproj`:

- `GenerateProgramStatics` writes `Utils/ProgramStatics.Generated.cs` from MSBuild properties
  (defaults in `Build.props`). Never edit it.
- On Release/publish, `Tools/Ceciler` rewrites the compiled `SPTarkov.Server.Core.dll` with
  Mono.Cecil. Two patch assemblies run, in this order: `Patches/Ceciler.WriteBarriers` prepends a
  stamp bump to the property setters of the model types reachable from the resident DB's published
  roots (a mod's game-data write then republishes without a hand-written call), then
  `Patches/Ceciler.JsonExtensionData` injects a `[JsonExtensionData]` property into every type under
  `Models` so unknown client JSON round-trips instead of being dropped. Order matters: the barrier
  patch walks authored setters, and `ExtensionData`'s is injected. Release binaries therefore differ
  structurally from Debug ones, and `WriteBarrier.Installed` — false in source, rewritten to true by
  the barrier patch — is how production code tells the two apart (`ResidentDbDispatch.Eligible`
  refuses to trust the resident DB with mods loaded where it is false). No test asserts the
  `ExtensionData` rewrite itself; the native tests only pin the `#[serde(flatten)] extra` contract
  that mirrors it, while the barrier patch has its own Release-only fixtures in
  `Testing/UnitTests/Tests/Native/WriteBarrier*Tests`.

`SPTarkov.Server.Assets` hashes `SPT_Data` into `checks.dat` on Release builds, which
`DatabaseImporter` verifies at startup outside DEBUG. The format is a contract shared with
`rust/spt-native/src/verify.rs`; scope is manifest-driven and exact in both directions over
`configs/` and `database/`, so deletions and swaps are caught, while `images/` is unverified.

### Native Rust layer

`rust/spt-native` is a `cdylib` called over C ABI from `Libraries/SPTarkov.Server.Core/Native/`
(`NativeMethods.cs`, `SptNative.cs`) — and, for the log and console exports, from the twin
`Libraries/SPTarkov.Common/Native/NativeMethods.cs`, because `SPTarkov.Common` cannot reference
Server.Core. It owns database hash verification, the ported generation paths (location loot, reward
loot, whole-bot inventory, dynamic ragfair offers, repeatable quests, scav case rewards), the item
base-class cache build, the ragfair linked-item table, the resident DB every ported family but bots
reads from, the whole log pipeline, and the terminal itself. Twenty-nine exports, JSON in / JSON out
— except the ragfair response, a framed MessagePack envelope, and the log and console exports — with
`spt_native_abi_version` handshaking against `SptNative.ExpectedAbiVersion`.

Every ported *class* keeps its complete 4.1.2 C# implementation as a **legacy path**, taken
automatically when a mod hooks it or forced by config, so a Rust cutover never removes a mod's
extension point. `DatabaseImporter` calls `SptNative.EnsureLoadable()` on every startup, so a missing
or ABI-mismatched library fails fast. The one exception is logging, which has no legacy path:
`AddSptLogger` initialises the native pipeline and `SPTLoggerDispatcher.Log` emits straight into it,
with mod `ILogHandler`s fanned out from the dispatcher (resolve it from DI and call `RegisterHandler`
— registering an `ILogHandler` in the host container alone never reaches it). It is failure-tolerant
by contract: a broken library or config produces one stderr notice and logging stays off rather than
stopping the server.

Only the bot family still projects its payload from the live database on every call. Every other
family carries no call-invariant half at all: `DbPublisher` re-publishes the
templates/traders/globals/locations/hideout roots into the native resident DB when
`DatabaseMutationStamp` has moved, and each call carries just an epoch (a stale epoch self-heals by
force-publish and one retry). Since Phase 2 the stamp's bump sites are mostly Ceciler-injected
setter barriers, so a modded server rides the resident path too — `TrustNativeRequestCacheWithMods`
defaults on, honoured only where the barriers were actually injected (Release and publish), with a
per-family kill switch beside it; an ineligible caller ships the full views with every call instead.

Native is not uniformly faster — several families are slower than the C# they replace and stay the
default anyway, each with a force-legacy flag. The argument per family is in
[`BENCHMARK.md`](BENCHMARK.md), next to the numbers.

Build coupling: `BuildSptNative` shells out to `cargo build` before compiling, so **`cargo` on
`PATH` is a hard build dependency**. Cross-RID builds need `-p:SptNativeRid=<rid>`, and only same-OS
targets are mapped: `Build.props` holds one entry (`linux-x64` → `x86_64-unknown-linux-gnu`) inside a
Linux-only `PropertyGroup`, so from a Windows host nothing maps and the guard in
`SPTarkov.Server.csproj` fails the build rather than shipping a host-triple library.

---

## 4. Integration Points

| External System | Integration Type | Notes |
|-------------------|-------------------|-------|
| Escape from Tarkov game client | Sync HTTP + async WebSocket | Every `/client/*` route; zlib both ways, responses wrapped in the `data`/`err`/`errmsg` envelope. `Models/Eft/` mirrors its wire contracts |
| `rust/spt-native` (cdylib) | Sync FFI, C ABI | Twenty-nine exports; JSON in/out except the MessagePack ragfair response and the log and console exports. `spt_native_abi_version` handshakes `SptNative.ExpectedAbiVersion` |
| `SPT_Data/` on disk | Batch read at startup | `configs/` via `ConfigLoader`, `database/` via `DatabaseImporter`, hash-verified against `checks.dat` outside DEBUG |
| `user/profiles/` | Async read/write | `SaveServer` owns the JSON profiles; interval saves plus `BackupService` timers |
| `user/mods/`, `user/patchers/` | Reflective assembly load | Third-party DLLs: `[Injectable]` registrations, `IOnDIConstruct` hooks, HarmonyX patches, enum prepatchers |
| Kestrel / HTTPS | Async, network | HTTPS-only with a self-generated certificate; the same host serves the game API, the admin panel and `/health` |
| `.NET` CLR (shipped builds) | Process host | `rust/mpex-server` hosts the CLR via netcorehost and `run_app`s the published assembly; `Containerfile.release` entrypoint |

---

# Relationship to Other Framework Components

| Component | Responsibility |
|-----------|-----------------|
| [`SPTarkov.Server/ARCHITECTURE.md`](SPTarkov.Server/ARCHITECTURE.md) | The executable host: startup sequence, mod loader, web pipeline hookup. No game logic. |
| [`Libraries/ARCHITECTURE.md`](Libraries/ARCHITECTURE.md) | Which of the six library projects owns what, and what they depend on. |
| [`Libraries/SPTarkov.Server.Core/ARCHITECTURE.md`](Libraries/SPTarkov.Server.Core/ARCHITECTURE.md) | Everything inside Core (~91% of the `.cs` files under `Libraries/`): folder map, dispatch mechanics, item events, configs, models. |
| [`rust/ARCHITECTURE.md`](rust/ARCHITECTURE.md) | Inside the Cargo workspace: `spt-native`'s modules, FFI boundary, porting conventions and tests, plus the `mpex-server` launcher and its no-`spt-native` rule. |
| [`RUST-ROADMAP.md`](RUST-ROADMAP.md) | Rust port status: what works, what flips to legacy, known divergences, roadmap. |
| [`BENCHMARK.md`](BENCHMARK.md) | Native vs legacy timings. Every measurement lives there; none are repeated elsewhere. |
| [`CLAUDE.md`](CLAUDE.md) | Build/run commands, style rules, cross-RID and toolchain requirements. |
