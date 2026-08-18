# Libraries/ — Architecture

Map of the six projects under `Libraries/`: which project owns what, what each depends on, and what
deliberately lives elsewhere.

| Document | Answers |
|---|---|
| [`SPTarkov.Server.Core/ARCHITECTURE.md`](SPTarkov.Server.Core/ARCHITECTURE.md) | Everything internal to Core (91% of the code here): layers, routing, item events, config loading, model conventions |
| [root `ARCHITECTURE.md`](../ARCHITECTURE.md) | Behaviour spanning `Libraries/` + `SPTarkov.Server/` + `rust/` |

## Project graph

```
SPTarkov.Common ──────┐
                      ├──> SPTarkov.Server.Core ──> SPTarkov.Server.Web
SPTarkov.DI ──────────┘
     │
     └───────────────> SPTarkov.Reflection      (referenced by the host, not by Core)
SPTarkov.Server.Assets                          (content-only; the host project-references it
                                                 solely to copy SPT_Data to output)
```

| Project | .cs files | Depends on | NuGet of note |
|---|---:|---|---|
| `SPTarkov.Common` | 22 | — | SemanticVersioning, ZLinq |
| `SPTarkov.DI` | 3 | — | MS.Extensions.DependencyInjection.Abstractions, Hosting.Abstractions |
| `SPTarkov.Reflection` | 8 | `SPTarkov.DI` | HarmonyX |
| `SPTarkov.Server.Core` | 855 | `SPTarkov.Common`, `SPTarkov.DI` | HarmonyX, FastCloner, System.IO.Hashing, MessagePack |
| `SPTarkov.Server.Web` | 50 | `SPTarkov.Server.Core` | MudBlazor, Argon2Sharp |
| `SPTarkov.Server.Assets` | 0 | — | — |

(`.cs` only, `obj/`+`bin/` excluded; Web also has 34 `.razor` components. One of Core's files is the
build-generated `Utils/ProgramStatics.Generated.cs`.)

---

## SPTarkov.Common

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
4.1.2 mods can still bind `Spectre.Console.Color`, frozen into `ISptLogger<T>`, `SptLogMessage`,
`ClientLogRequest` and `Watermark.Draw`. Its `BuildSpectreFacade` target shells out to `cargo`, so
**Common needs the Rust toolchain too**, not just Core. `<Reference>` items are not transitive, so
each of the five projects naming `Color` carries its own: Common, Core, `Testing/UnitTests` and the
two `Tools/` generators.

## SPTarkov.DI

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

## SPTarkov.Reflection

Runtime method patching for mods, over HarmonyX.

- `Patching/` — `AbstractPatch` (the base a mod patch derives from), `IRuntimePatch`,
  `PatchManager`, `PatchException`, `Attributes.cs` (`[PatchPrefix]`, `[PatchPostfix]`, …).
- `CodeWrapper/` — `Code`, `CodeWithLabel`, `CodeGenerator`: IL emit helpers for transpilers.

## SPTarkov.Server.Core

All game logic — 855 of the 938 `.cs` files under `Libraries/`. Referenced by
`SPTarkov.Server.Web` and by the host; references only `SPTarkov.Common` and `SPTarkov.DI`.

→ **[`SPTarkov.Server.Core/ARCHITECTURE.md`](SPTarkov.Server.Core/ARCHITECTURE.md)**.

## SPTarkov.Server.Web

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

## SPTarkov.Server.Assets

Content project, no code — just the `.csproj` and the payload. Ships `SPT_Data/` (`configs/`,
`database/`, `images/`, generated `checks.dat`) plus `looseLoot.7z`, unpacked by
`scripts/decompress-assets.sh`. `SPTarkov.Server`'s build relocates `dotnet/` satellite assemblies
and `wwwroot/` admin-panel assets into the *output* `SPT_Data`, which is why neither is covered by
hash verification.

`checks.dat` is regenerated on Release builds only, by the `PreBuildHashFile` target running the
`gen_checks` bin — a thin wrapper over the same XXH3-128 code the startup verifier uses (see the root
ARCHITECTURE.md).

Excluded from the knowledge graph by `.graphifyignore` (~all JSON data, not code).
