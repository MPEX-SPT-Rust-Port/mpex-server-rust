# **Libraries** Architecture

## 1. Overview Summary

Map of the five projects under `Libraries/`: which project owns what, what each depends on, and what
deliberately lives elsewhere. All game logic sits in one of them (`SPTarkov.Server.Core`, ~91% of the files
and ~94% of the lines here); the other four are a container, a logging front end, a patching toolkit and the
Blazor admin panel.

| Language | Lines of Code | File Count |
|-----------|-----------------|-----------|
| `C#` | `125,848` | `938` |
| `Razor` | `6,394` | `34` |

Per project (`.cs` only, `obj/`+`bin/` excluded; all 34 `.razor` files are Web's, and one of Core's
files is the build-generated `Utils/ProgramStatics.Generated.cs`):

| Project | .cs files | Lines | Depends on | NuGet of note |
|---|---:|---:|---|---|
| `SPTarkov.Common` | 22 | 1,515 | — | SemanticVersioning, ZLinq |
| `SPTarkov.DI` | 3 | 304 | — | MS.Extensions.DependencyInjection.Abstractions, Hosting.Abstractions |
| `SPTarkov.Reflection` | 8 | 757 | `SPTarkov.DI` | HarmonyX |
| `SPTarkov.Server.Core` | 855 | 117,812 | `SPTarkov.Common`, `SPTarkov.DI` | HarmonyX, FastCloner, System.IO.Hashing, MessagePack |
| `SPTarkov.Server.Web` | 50 | 5,460 | `SPTarkov.Server.Core` | MudBlazor, Argon2Sharp |

---

## 2. High Level Design

```
SPTarkov.Common ──────┐
                      ├──> SPTarkov.Server.Core ──> SPTarkov.Server.Web
SPTarkov.DI ──────────┘
     │
     └───────────────> SPTarkov.Reflection      (referenced by the host, not by Core)
Libraries/SPTarkov.Server.Assets/Assets.props   (not a project; imported by the host and the two
                                                 Tools generators to copy SPT_Data to output)
```

| Component | Responsibility | Interacts With |
|-----------|-----------------|------------------|
| `SPTarkov.Common` | Framework-agnostic primitives: logging front end, semver, generic extensions | `rust/spt-native` (log P/Invokes), `rust/spectre-facade`; consumed by Core and Web |
| `SPTarkov.DI` | The whole attribute-driven container: `[Injectable]`, assembly scan, registration | Core, `.Reflection`, the host's `RegisterSptServicesAsync` |
| `SPTarkov.Reflection` | Runtime method patching for mods, over HarmonyX | Mod assemblies, the host — *not* Core |
| `SPTarkov.Server.Core` | All game logic — 855 of the 938 `.cs` files here | `.Common`, `.DI`, `rust/spt-native`; consumed by `.Web` and the host |
| `SPTarkov.Server.Web` | Blazor Server admin panel (MudBlazor), served by the same Kestrel host | `.Server.Core`, mod `IModBlazorMetadata` implementations |
| `SPTarkov.Server.Assets` | Content only, no project: `SPT_Data/` plus `looseLoot.7z` and `Assets.props` | Consumer builds (via `Assets.props`), `DatabaseImporter`, `gen_checks` |

---

## 3. Low Level Design

### SPTarkov.Common

Framework-agnostic primitives. No project references at all, so it structurally cannot see a game
type — no `MongoId`, no `Item`, no config record. Logging, semver, generic extensions.

| Folder | Contents |
|---|---|
| `Logger/` | `SptLogger`, `SptLoggerProvider`, `SPTLoggerDispatcher`, `SptEarlyLoggerFactory` (pre-DI logger used during startup), `SptLoggerWrapper` (adapts to `Microsoft.Extensions.Logging.ILogger`; the class inside is spelled `SPTLoggerWrapper`) |
| `Logger/Handlers/` | `BaseLogHandler` only. Its two implementations moved to Rust, but the abstract class is live mod surface — a mod subclasses it and registers via `SPTLoggerDispatcher.RegisterHandler` |
| `Native/` | `NativeMethods` — the logger P/Invokes into `rust/spt-native` |
| `Models/Logging/` | `ISptLogger`, `ILogHandler`, `SptLogMessage`, `SptLoggerConfiguration` (bound from `sptLogger.json`), `FileLogger` (empty marker type used as a log category) |
| `Semver/` | `ISemVer` + `SemanticVersioningSemVer` — mod `SptVersion` range checks |
| `Extensions/` | `String`, `List`, `Object`, `MemberInfo`, `HttpContext`, two logger extension sets |
| `Json/Converters/` | `BaseSptLoggerReferenceConverter` — resolves a config entry's `"type"` to `File`/`Console` |

The `sptLogger.json` / `sptLogger.Development.json` files the server refuses to start without are
this project's config. It is the front end only: `AddSptLogger` hands the raw config bytes to
`rust/spt-native`, which owns filters, level gate, formatting and the console/file sinks.

Common also *emits* `Spectre.Console.Ansi` — the facade `rust/spectre-facade` builds so compiled
4.1.2 mods can still bind `Spectre.Console.Color`, frozen into signatures like `ISptLogger<T>`,
`SptLogMessage`, `ClientLogRequest`, `Watermark.Draw` and `ProgramStatics.BUILD_TEXT_COLOR` — every one
of them takes or returns a colour it never renders. Its `BuildSpectreFacade` target shells out to `cargo`, so
**Common needs the Rust toolchain too**, not just Core. `<Reference>` items are not transitive, so
each of the five projects naming `Color` carries its own: Common, Core, `Testing/UnitTests` and the
two `Tools/` generators.

### SPTarkov.DI

Three files — the whole attribute-driven container.

- `Annotations/Injectable.cs` — `[Injectable]`, `InjectionType` (Singleton / Transient / Scoped /
  HostedService), `TypePriority`.
- `DependencyInjectionHandler.cs` — assembly scan; registers each type against itself, its
  interfaces and its base types. `InjectAll` applies them in ascending `TypePriority`. Two traps:
  interfaces in a `System.*` namespace are skipped, so nothing is resolvable as `IDisposable` or
  `IComparable`; and `InjectAll` throws on a second call, which is why every caller (including the
  DI validation test) builds a fresh handler rather than reusing one.
- `Extensions/DependencyInjectionExtensions.cs` — the `IServiceCollection` hookup.

The lifecycle interfaces (`IOnLoad`, `IOnUpdate`, `IOnDIConstruct`) live in **Core**'s `DI/` folder,
not here — this project knows nothing about server startup.

### SPTarkov.Reflection

Runtime method patching for mods, over HarmonyX.

- `Patching/` — `AbstractPatch` (the base a mod patch derives from), `IRuntimePatch`,
  `PatchManager`, `PatchException`, `Attributes.cs` (`[PatchPrefix]`, `[PatchPostfix]`, …).
- `CodeWrapper/` — `Code`, `CodeWithLabel`, `CodeGenerator`: IL emit helpers for transpilers.

### SPTarkov.Server.Core

All game logic — 855 of the 938 `.cs` files under `Libraries/`. Referenced by
`SPTarkov.Server.Web` and by the host; references only `SPTarkov.Common` and `SPTarkov.DI`.

→ **[`SPTarkov.Server.Core/ARCHITECTURE.md`](SPTarkov.Server.Core/ARCHITECTURE.md)**.

### SPTarkov.Server.Web

Blazor Server admin panel (MudBlazor), served by the same Kestrel host.

| Folder | Contents |
|---|---|
| `Pages/` | Ten routed pages: `/`, `/login`, `/profiles`, `/configs`, `/database`, `/tools`, `/credentials`, `/status`, `/thank-you`, `/example-page` |
| `Pages/Database/` | `DatabasePage` split across `DatabasePage.razor.cs` plus ten partials — eight per-table (achievements, bots, customization, globals, handbook, items, quests, traders) and `Filters` + `Formatting` |
| `Layout/` | `BaseMainLayout` (the `DefaultLayout` on `Routes.razor`), `BaseMudBlazorLayout` (`@layout` on all ten built-in pages) — also the shells a mod page can opt into |
| `Components/` | `Auth/` (1), `Configs/` (2), `Database/` (9), `Profiles/` (7) |
| `Models/` | View models: `Database/` (13), `Profiles/` (12), `Configs/` (4) |
| `Services/` | `AuthService` + `IPasswordHasher`/`Argon2idPasswordHasher`, `ConfigEditorService` + `IConfigEditorConfigProvider`, `WebLocalizationService` |
| `Utils/` | `JsonPropertyFlattener` — drives the record-detail views in `ProfileControlPage` and the `DatabasePage` table partials (*not* the config editor) |
| root | `SPTWeb.cs` (registration + three minimal-API routes), `App.razor`, `Routes.razor`, `_Imports.razor`, `IModBlazorMetadata.cs` |

`IModBlazorMetadata` is the marker a mod's **metadata class** implements — alongside `IModMetadata`,
not on the assembly — to have its `wwwroot` linked and its Blazor pages and MVC controllers
registered. The one place in the solution where MVC controllers are supported at all.

`SPTWeb.cs`'s three minimal APIs (login, logout, profile download) plus `/health` in the host are the
only *minimal-API* routes in the solution — but not the only traffic outside the router pipeline: the
same method calls `MapRazorComponents<App>()` and `MapControllers()`, so every admin-panel page and
mod MVC controller is routed by ASP.NET too.

### SPTarkov.Server.Assets

Content directory, not a project — just `Assets.props` and the payload. Ships `SPT_Data/` (`configs/`,
`database/`, `images/`, generated `checks.dat`) plus `looseLoot.7z`, unpacked by
`scripts/decompress-assets.sh`. `SPTarkov.Server`'s build relocates `dotnet/` satellite assemblies
and `wwwroot/` admin-panel assets into the *output* `SPT_Data`, which is why neither is covered by
hash verification.

`checks.dat` is regenerated on Release builds only, by the `PreBuildHashFile` target in
`SPTarkov.Server.csproj` running the `gen_checks` bin — a thin wrapper over the same XXH3-128 code
the startup verifier uses (see the root ARCHITECTURE.md).

Excluded from the knowledge graph by `.graphifyignore` (~all JSON data, not code).

---

## 4. Integration Points

| External System | Integration Type | Notes |
|-------------------|-------------------|-------|
| `rust/spt-native` (cdylib) | Sync FFI, C ABI | Two P/Invoke sites: Common's `Native/NativeMethods.cs` for the log exports, Core's `Native/` for everything else. Common cannot reference Core, hence the twin |
| `rust/spectre-facade` | Build-time codegen | Emits `Spectre.Console.Ansi.dll` via `BuildSpectreFacade` in `SPTarkov.Common.csproj`; **Common needs `cargo` on `PATH`** |
| `SPT_Data/` on disk | Batch, build + startup | The `Assets.props` glob stages it into consumer output; `gen_checks` hashes it on Release; `DatabaseImporter` reads and verifies it |
| Mod assemblies | Reflective load | `[Injectable]` scan (`.DI`), HarmonyX patches (`.Reflection`), `BaseLogHandler` subclasses (`.Common`), `IModBlazorMetadata` pages and MVC controllers (`.Web`) |
| Browser (admin panel) | Async, Blazor Server circuit | `.Web` over the shared Kestrel host; `AuthService` login, Argon2id hashing |
| `sptLogger[.Development].json` | Config read at startup | Bound to `.Common`'s `SptLoggerConfiguration`, then handed to Rust as raw bytes |

---

# Relationship to Other Framework Components

| Component | Responsibility |
|-----------|-----------------|
| [`SPTarkov.Server.Core/ARCHITECTURE.md`](SPTarkov.Server.Core/ARCHITECTURE.md) | Everything internal to Core (91% of the files here): layers, routing, item events, config loading, model conventions |
| [root `ARCHITECTURE.md`](../ARCHITECTURE.md) | Behaviour spanning `Libraries/` + `SPTarkov.Server/` + `rust/` |
| [`SPTarkov.Server/ARCHITECTURE.md`](../SPTarkov.Server/ARCHITECTURE.md) | The host that starts these projects up and registers them |
| [`rust/ARCHITECTURE.md`](../rust/ARCHITECTURE.md) | The crate behind both `Native/` folders, and the Spectre facade emitter |
