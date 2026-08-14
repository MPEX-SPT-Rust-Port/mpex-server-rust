# SPTarkov.Server/ — Architecture

The executable host (`SPT.Server`, `SPT.Server.Linux` on linux-x64). ~1.9k lines across 15 `.cs` files —
everything here is *startup*: bring up logging, load configs, run the mod loader, import the database, build
the DI container, hand off to Kestrel. No game logic lives in this project; that is all
[`Libraries/SPTarkov.Server.Core`](../Libraries/SPTarkov.Server.Core/ARCHITECTURE.md).

Neighbouring documents:

| Document | Answers |
|---|---|
| [root `ARCHITECTURE.md`](../ARCHITECTURE.md) | Request pipeline, DI conventions, `OnLoadOrder`, persistence |
| [`Libraries/ARCHITECTURE.md`](../Libraries/ARCHITECTURE.md) | Which library project owns what |
| [`rust/ARCHITECTURE.md`](../rust/ARCHITECTURE.md) | The Rust layer  |

References `SPTarkov.Server.Core`, `.Web`, `.Assets` and `.Reflection`. Its only direct NuGet dependency is
`AsmResolver.DotNet` (used by the enum prepatcher).

## Startup sequence

`Program.Main` → `StartServer` → `StartServerAfterModLoading`:

1. `RegisterSatelliteLocalizations()` — resolver hook so satellite `.resources.dll`s load from `SPT_Data/dotnet/`
   instead of cluttering the root (the csproj targets put them there).
2. `ProgramStatics.Initialize()`, then a pre-DI `SptEarlyLoggerFactory` from `sptLogger[.Development].json`.
3. `IsRunFromInstallationFolder()` — refuses to run unless one of those logger files is in the CWD.
4. `ConfigLoader.Initialize` (Core) reads `SPT_Data/configs/`.
5. `CreateEarlySptProvider` builds a **throwaway** container — every `[Injectable]` in Core, plus the mod loader,
   validator, database importer and an early locale table (`ProgramHelpers.CreateEarlyLocaleTable` reads the
   locale JSON directly, since the database isn't imported yet). Mods must load before the real container exists.
6. `ModLoader.RunModLoader` — may not return to this process at all (see below).
7. `DatabaseImporter.LoadDatabaseAsync` — probes `spt_native`, hash-verifies outside DEBUG (delegated to the Rust
   verifier), deserialises `SPT_Data/database/` into `DatabaseTables`.
8. `CreateNewHostBuilder` + `RegisterSptServicesAsync` build the real container; each config object and each
   `DatabaseTables` member is registered as its own singleton. `ValidateOnBuild`/`ValidateScopes` are on, so a
   broken registration fails at `app.Build()` rather than on first request.
9. Kestrel HTTPS config, `app.Build()`, middleware wiring, `RunPreSptLoadCallbacks`, port check, `app.RunAsync`.

**`StartServerAfterModLoading` must stay a separate method** — inlining it pulls types into context before
prepatching runs and breaks it.

## Files

| Path | Contents |
|---|---|
| `Program.cs` | `Main` (top-level error triage: mod-assembly load failures, port-bind failures, everything else), `StartServer`, `StartServerAfterModLoading`, Kestrel/web-app config, satellite assembly resolver |
| `Helpers/ProgramHelpers.cs` | `CreateNewHostBuilder`, `RegisterSptServicesAsync` (the single registration point), `CreateEarlySptProvider`, `CreateEarlyLocaleTable` |
| `Helpers/DatabaseImporter.cs` | `SptNative.EnsureLoadable()`, hash verification via `checks.dat`, recursive JSON import |
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

## Modding

Mods are directories under `user/mods/` (one or more DLLs, exactly one `IModMetadata` implementation).
Prepatcher definitions live in `user/patchers/<ModGuid>/<one>.json`.

| File | Role |
|---|---|
| `ModLoader.cs` | Discovers and loads mod assemblies (sorted by `ModGuid`, so load order is deterministic), collects enum prepatch definitions, applies them, and — if any applied cleanly — reboots the server in-process against the patched Core. Returns `ModLoaderRunResult(ShouldStartServer, ValidRuntimeMods)` |
| `ModValidator.cs` | Rejects/warns: bad GUID format (`^[a-zA-Z0-9-]+(\.[a-zA-Z0-9-]+)*$`), duplicates, unmet/outdated `ModDependencies`, declared `Incompatibilities`, `SptVersion` semver mismatch, folder shapes that mean the user installed a client or legacy JS mod. Any error → **no mods load at all**. Also hard-throws if a mod references a newer `SPTarkov.Server.Core` assembly than the running one |
| `EnumPatcher.cs` | Adds a literal field to an existing Core enum via AsmResolver; validates name/value collisions and underlying-type fit |
| `PrepatchAssemblyWriter.cs` | Writes the module with `MetadataBuilderFlags.PreserveAll` so the original PDB stays valid |
| `PrepatchLoadContext.cs` | `AssemblyLoadContext` ("SPT.PrepatchHost") serving the patched Core from memory while sharing framework/`MudBlazor`/`ZLogger`/Harmony assemblies with the default context |

**The prepatch reboot**: if any prepatch definitions are found, `ModLoader` patches `SPTarkov.Server.Core.dll`
in memory, writes it to `SPTarkov.Server.Core.Patched.dll` (for debugging), loads the host assembly *again*
inside `PrepatchLoadContext`, and invokes `Program.Main` there. The hosted copy detects the context by name and
skips straight to mod loading. The outer call returns `ShouldStartServer: false`. Mods load into whichever
context the loader is running in, so under prepatching they bind to the patched Core.

Two things about that pass are worth knowing before touching it. It runs *before* mods are loaded and validated,
so it walks `user/patchers/` directly — a patcher whose mod is later rejected still gets applied. And if any
prepatch throws, no reboot happens: the loader logs it and carries on into normal mod loading against the
*unpatched* Core.

## Web pipeline

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

## Build

`Microsoft.NET.Sdk.Web`, server GC, `InternalsVisibleTo(UnitTests)`. MSBuild targets of note:

- `CheckSptNativeRuntimeIdentifier` — fails a cross-RID build/publish when `Build.props` maps no Rust triple for
  the RID, rather than silently packaging a host-triple `spt_native`.
- `RelocateSatelliteAssemblies` / `RelocatePublishedSatelliteAssemblies` / `CleanStaleSatelliteFolders` — move
  satellite assemblies into `SPT_Data/dotnet/`, matched by the resolver hook in `Program.cs`.
- `StaticWebAssetBasePath` points Blazor assets at `SPT_Data/wwwroot`, which is also the host builder's
  `WebRootPath`.
- `sptLogger.json` copies on Release only, `sptLogger.Development.json` on Debug only.
