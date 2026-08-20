# **SPTarkov.Server** Architecture

## 1. Overview Summary

The executable host (`SPT.Server`, `SPT.Server.Linux` on linux-x64). Everything here is *startup*: bring up
logging, load configs, run the mod loader, import the database, build the DI container, hand off to Kestrel. No
game logic lives in this project; that is all
[`Libraries/SPTarkov.Server.Core`](../Libraries/SPTarkov.Server.Core/ARCHITECTURE.md). It references
`SPTarkov.Server.Core`, `.Web` and `.Reflection`; `SPT_Data` is not a project reference at all — it is staged
into the output by importing `Libraries/SPTarkov.Server.Assets/Assets.props`. Its only direct NuGet dependency
is `AsmResolver.DotNet`, used by the enum prepatcher.

| Language | Lines of Code | File Count |
|-----------|-----------------|-----------|
| `C#` | `1,936` | `15` |
| `JSON` (logger/launch/runtime config) | `176` | `4` |

---

## 2. High Level Design

`Program.Main` → `StartServer` → `StartServerAfterModLoading`:

```
Main
 ├─ RegisterSatelliteLocalizations()      resolver hook → SPT_Data/dotnet/
 ├─ ProgramStatics.Initialize() → SptEarlyLoggerFactory (pre-DI)
 ├─ IsRunFromInstallationFolder()         refuses to run without sptLogger[.Development].json in CWD
 ├─ ConfigLoader.Initialize (Core)        reads SPT_Data/configs/
 ├─ CreateEarlySptProvider()              throwaway container: Core [Injectable]s, mod loader,
 │                                        validator, importer, early locale table
 ├─ ModLoader.RunModLoader() ─────────────► may reboot in-process and never return here
 │
 └─ StartServerAfterModLoading
     ├─ DatabaseImporter.LoadDatabaseAsync()   probe spt_native, push locales to Rust,
     │                                         hash-verify, deserialise
     ├─ CreateNewHostBuilder + RegisterSptServicesAsync   the real container
     └─ Kestrel HTTPS → app.Build() → middleware → RunPreSptLoadCallbacks → port check → RunAsync
```

`CreateEarlySptProvider` builds a throwaway container because mods must load before the real one exists; its
early locale table (`ProgramHelpers.CreateEarlyLocaleTable`) reads the locale JSON directly, since the database
isn't imported yet. `CreateNewHostBuilder` registers every config object and each of the ten `DatabaseTables`
members as its own singleton — that is why Core code injects `HttpConfig` or `BotTable` directly rather than
going through a wrapper. `ValidateOnBuild`/`ValidateScopes` are on in the real container, so a broken
registration fails at `app.Build()` rather than on first request.

**`StartServerAfterModLoading` must stay a separate method** — inlining it pulls types into context before
prepatching runs and breaks it.

| Component | Responsibility | Interacts With |
|-----------|-----------------|------------------|
| `Program` | Entry point, startup ordering, Kestrel/web-app config, satellite assembly resolver, top-level error triage | Everything below; Core's `HttpServer`, `CertificateHelper` |
| `Helpers/ProgramHelpers` | The single DI registration point, plus the early throwaway provider | Core `[Injectable]`s, `ModLoader`, `DatabaseImporter` |
| `Helpers/DatabaseImporter` | Probe `spt_native`, push the locale table to it, hash-verify, import `SPT_Data/database/` into `DatabaseTables` | `SptNative` (Rust), `DatabaseTables`, `ProgramHelpers` |
| `Modding/ModLoader` | Discover mods, collect and apply enum prepatches, optionally reboot in-process | `ModValidator`, `EnumPatcher`, `PrepatchLoadContext` |
| `Middleware/SptLoggerMiddleware` | Request/websocket logging, 404 and unhandled-exception logging | Kestrel pipeline, Core `HttpConfig` |
| `Extensions/ProgramExtensions` | `IOnLoad` callbacks that must run before binding; port availability check | Core `IOnLoad` implementations |

---

## 3. Low Level Design

### Files

| Path | Contents |
|---|---|
| `Program.cs` | `Main` (top-level error triage: startup cancellation, port-bind failures, mod-assembly load failures, everything else), `StartServer`, `StartServerAfterModLoading`, Kestrel/web-app config, satellite assembly resolver |
| `Helpers/ProgramHelpers.cs` | `CreateNewHostBuilder`, `RegisterSptServicesAsync` (the single registration point — scans Core, `.Reflection`, `.Web`, the host and every mod assembly for `[Injectable]`), `CreateEarlySptProvider`, `CreateEarlyLocaleTable` |
| `Helpers/DatabaseImporter.cs` | `SptNative.EnsureLoadable()`, `SptNative.SetServerLocales()`, hash verification via `checks.dat`, recursive JSON import |
| `Helpers/DatabaseTables.cs` | The ten-table record the importer materialises |
| `Helpers/StartupCancellation.cs` | Ctrl-C + SIGTERM → one `CancellationToken`; second signal falls through to the runtime. `LinkTo` joins it to `IHostApplicationLifetime` |
| `Extensions/ProgramExtensions.cs` | `RunPreSptLoadCallbacks` (runs `IOnLoad` with priority in `[Watermark, GameCallbacks)` before binding), `VerifyWebServerPortAvailable` |
| `Extensions/ServiceCollectionExtensions.cs` | `AddModDIConstructorsAsync` — reflectively invokes mods' static `IOnDIConstruct.OnDIConstructAsync` |
| `Middleware/SptLoggerMiddleware.cs` | Request/websocket logging (gated on `HttpConfig.LogRequests`), local-vs-remote IP formatting, 404 and unhandled-exception logging. Rethrows in DEBUG |
| `Exceptions/` | `ModLoaderException`, `WebServerPortUnavailableException` (carries IP/port/`SocketError` for the triage in `Main`) |
| `Modding/` | See below |
| `sptLogger.json` / `sptLogger.Development.json` | Logger config: `user/logs/spt/`, `user/logs/requests/`, console — split by regex filters on logger name |
| `Properties/launchSettings.json` | Two profiles, both setting `workingDirectory` to the output dir (the server won't start elsewhere) |
| `runtimeconfig.template.json` | One knob: `System.GC.ConserveMemory: 5` |

### Modding

Mods are directories under `user/mods/` (one or more DLLs, exactly one `IModMetadata` implementation).
Prepatcher definitions live in `user/patchers/<ModGuid>/<one>.json`.

| File | Role |
|---|---|
| `ModLoader.cs` | Discovers and loads mod assemblies (sorted by `ModGuid`, so load order is deterministic), collects enum prepatch definitions, applies them, and — if all applied cleanly — reboots the server in-process against the patched Core. Returns `ModLoaderRunResult(ShouldStartServer, ValidRuntimeMods)` |
| `ModValidator.cs` | Rejects/warns: bad GUID format (`^[a-zA-Z0-9-]+(\.[a-zA-Z0-9-]+)*$`), duplicates, unmet/outdated `ModDependencies`, declared `Incompatibilities`, `SptVersion` semver mismatch, folder shapes that mean the user installed a client or legacy JS mod (`.js`/`.ts` is allowed for `IModBlazorMetadata` mods, which ship admin-panel assets). Any error → **no mods load at all**. Also hard-throws if a mod references a newer `SPTarkov.Server.Core` assembly than the running one |
| `EnumPatcher.cs` | Adds a literal field to an existing Core enum via AsmResolver; validates name/value collisions and underlying-type fit |
| `PrepatchAssemblyWriter.cs` | Writes the module with `MetadataBuilderFlags.PreserveAll` so the original PDB stays valid |
| `PrepatchLoadContext.cs` | `AssemblyLoadContext` ("SPT.PrepatchHost") serving the patched Core from memory while sharing `Microsoft.*`/`System.*`/`MudBlazor`/`0Harmony`/`MonoMod.*`/`Mono.Cecil` with the default context |

**The prepatch reboot**: if any prepatch definitions are found, `ModLoader` patches `SPTarkov.Server.Core.dll`
in memory and — only once every definition applied — writes it to `SPTarkov.Server.Core.Patched.dll`
(for debugging, and deleted at the start of each run), loads the host assembly *again*
inside `PrepatchLoadContext`, and invokes `Program.Main` there. The hosted copy detects the context by name and
skips straight to mod loading. The outer call returns `ShouldStartServer: false`. Mods load into whichever
context the loader is running in, so under prepatching they bind to the patched Core.

Two things about that pass are worth knowing before touching it. It runs *before* mods are loaded and validated,
so it walks `user/patchers/` directly — a patcher whose mod is later rejected still gets applied. And if any
prepatch throws, no reboot happens: the loader logs it and carries on into normal mod loading against the
*unpatched* Core.

### Web pipeline

`ConfigureWebApp` in `Program.cs`, in order: `UseWebSockets` → `SptLoggerMiddleware` → the `/health` minimal API
→ a catch-all `app.Use` that delegates to Core's `HttpServer` → `UseSptBlazor()` (admin panel) → `ForwardedHeaders`.
Kestrel is HTTPS-only (TLS 1.2/1.3) with a certificate from Core's `CertificateHelper`.

Note the tail of that list: `UseForwardedHeaders` is registered *after* `ConfigureWebApp`, so although its
known-proxy lists are cleared (any reverse proxy's `X-Forwarded-For` is trusted), it sits downstream of both the
logging middleware and the router. Requests the router handles never reach it, and are logged with the
connection's IP rather than the forwarded one.

`SptLoggerMiddleware` short-circuits when `HttpConfig.LogRequests` is off — that takes the 404 and
unhandled-exception logging with it, not just the per-request lines.

`/health` is the only route this project registers outside the router pipeline; the other three live in
`SPTarkov.Server.Web/SPTWeb.cs`.

### Build

`Microsoft.NET.Sdk.Web`, server GC, `InternalsVisibleTo(UnitTests)`. MSBuild targets of note:

- `PreBuildHashFile` — Release only, `BeforeTargets="AssignTargetPaths"`: runs the `gen_checks` Rust bin to
  regenerate `Libraries/SPTarkov.Server.Assets/SPT_Data/checks.dat` before content copies are planned.
  `Assets.props` from that same directory is imported here to stage `SPT_Data` into the output.
- `CheckSptNativeRuntimeIdentifier` — fails a cross-RID build/publish when `Build.props` maps no Rust triple for
  the RID, rather than silently packaging a host-triple `spt_native`.
- `IncludeMpexServerLauncher` — copies the Rust `mpex-server` launcher into the publish output beside the app.
  It is *built* by `SPTarkov.Server.Core`'s workspace `cargo build`; this target only places it.
- `RelocateSatelliteAssemblies` / `RelocatePublishedSatelliteAssemblies` / `CleanStaleSatelliteFolders` — move
  satellite assemblies into `SPT_Data/dotnet/`, matched by the resolver hook in `Program.cs`.
- `StaticWebAssetBasePath` points Blazor assets at `SPT_Data/wwwroot`, which is also the host builder's
  `WebRootPath`.
- `sptLogger.json` copies on Release only, `sptLogger.Development.json` on Debug only.

---

## 4. Integration Points

| External System | Integration Type | Notes |
|-------------------|-------------------|-------|
| `spt_native` (Rust cdylib) | Sync FFI | `DatabaseImporter` calls `SptNative.EnsureLoadable()`, pushes the flattened server-locale table over with `SetServerLocales()` so native generator diagnostics render localised, then delegates `checks.dat` hash verification to the Rust verifier (skipped in DEBUG) |
| `mpex-server` (Rust exe) | Publish artifact | `IncludeMpexServerLauncher` publishes it beside `SPT.Server`; it hosts the CLR and `run_app`s the published assembly. See [`rust/ARCHITECTURE.md`](../rust/ARCHITECTURE.md) |
| `SPT_Data/` on disk | Batch read at startup | `configs/` via Core's `ConfigLoader`, `database/` via recursive JSON import, `dotnet/` for satellite assemblies, `wwwroot/` for Blazor assets |
| Kestrel / HTTPS | Async, network | HTTPS-only, TLS 1.2/1.3, certificate from Core's `CertificateHelper`. Port availability checked before `RunAsync` |
| `user/mods/`, `user/patchers/` | Reflective assembly load | Third-party DLLs loaded into the default or prepatch `AssemblyLoadContext`; static `IOnDIConstruct` hooks invoked during container construction |
| Reverse proxies | Async, network | `ForwardedHeaders` with cleared known-proxy lists — but registered downstream of the router, so it rarely applies |
| OS signals | Async | `StartupCancellation`: Ctrl-C + SIGTERM → one `CancellationToken`, linked to `IHostApplicationLifetime` |

---

# Relationship to Other Framework Components

| Component | Responsibility |
|-----------|-----------------|
| [root `ARCHITECTURE.md`](../ARCHITECTURE.md) | Request pipeline, DI conventions, `OnLoadOrder`, persistence |
| [`Libraries/ARCHITECTURE.md`](../Libraries/ARCHITECTURE.md) | Which library project owns what |
| [`Libraries/SPTarkov.Server.Core/ARCHITECTURE.md`](../Libraries/SPTarkov.Server.Core/ARCHITECTURE.md) | All game logic — everything this project starts up |
| [`rust/ARCHITECTURE.md`](../rust/ARCHITECTURE.md) | The Rust layer and its FFI/wire format |
