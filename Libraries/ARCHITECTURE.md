# Libraries/ — Architecture

Map of the six projects under `Libraries/`. This is the *layout and ownership* document: which
project owns a thing, what each depends on, and what deliberately lives elsewhere.

Two neighbouring documents carry the detail this one omits:

| Document | Answers |
|---|---|
| [`SPTarkov.Server.Core/ARCHITECTURE.md`](SPTarkov.Server.Core/ARCHITECTURE.md) | Everything internal to Core (91% of the code here): its layers, routing mechanics, item-event protocol, config loading, model conventions |
| [root `ARCHITECTURE.md`](../ARCHITECTURE.md) | Behaviour spanning `Libraries/` + `SPTarkov.Server/` + `rust/` |

Scope note: `Libraries/SPTarkov.Server.Assets/` is excluded from the knowledge graph by
`.graphifyignore` (it is ~all JSON data, not code). It is summarised here for completeness only.

## Project graph

```
SPTarkov.Common ──────┐
                      ├──> SPTarkov.Server.Core ──> SPTarkov.Server.Web
SPTarkov.DI ──────────┘
     │
     └───────────────> SPTarkov.Reflection      (referenced by the host, not by Core)
SPTarkov.Server.Assets                          (content-only, no code references)
```

| Project | .cs files | Depends on | NuGet of note |
|---|---:|---|---|
| `SPTarkov.Common` | 24 | — | SemanticVersioning, ZLinq |
| `SPTarkov.DI` | 3 | — | MS.Extensions.DependencyInjection.Abstractions, Hosting.Abstractions |
| `SPTarkov.Reflection` | 8 | `SPTarkov.DI` | HarmonyX |
| `SPTarkov.Server.Core` | 840 | `SPTarkov.Common`, `SPTarkov.DI` | HarmonyX, FastCloner, System.IO.Hashing |
| `SPTarkov.Server.Web` | 50 | `SPTarkov.Server.Core` | MudBlazor, Argon2Sharp |
| `SPTarkov.Server.Assets` | 1 | — | — |

(`.cs` only — `SPTarkov.Server.Web` additionally has 34 `.razor` components.)

---

## SPTarkov.Common

Framework-agnostic primitives. It has no project references at all, so it structurally cannot see
a game type — no `MongoId`, no `Item`, no config record. Logging, semver and generic extensions only.

| Folder | Contents |
|---|---|
| `Logger/` | `SptLogger`, `SptLoggerProvider`, `SPTLoggerDispatcher`, `SptEarlyLoggerFactory` (the pre-DI logger used during startup), `LogFileRollManager`, `SPTLoggerWrapper` (adapts the dispatcher to `Microsoft.Extensions.Logging.ILogger`) |
| `Logger/Handlers/` | `BaseLogHandler` → `ConsoleLogHandler`, `FileLogHandler` |
| `Models/Logging/` | `ISptLogger`, `ILogHandler`, `SptLogMessage`, `SptLoggerConfiguration` (bound from `sptLogger.json`), `FileLogger` (empty marker type used as a log category) |
| `Semver/` | `ISemVer` + `SemanticVersioningSemVer` — used for mod `SptVersion` range checks |
| `Extensions/` | `String`, `List`, `Object`, `MemberInfo`, `HttpContext`, and two logger extension sets |
| `Json/Converters/` | `BaseSptLoggerReferenceConverter` — resolves a logger config entry's `"type"` to `File`/`Console` |

The `sptLogger.json` / `sptLogger.Development.json` files the server refuses to start without are
this project's config.

## SPTarkov.DI

Three files. The whole attribute-driven container.

- `Annotations/Injectable.cs` — `[Injectable]`, `InjectionType` (Singleton / Transient / Scoped /
  HostedService), `TypePriority`.
- `DependencyInjectionHandler.cs` — assembly scan; registers each type against itself, its
  interfaces and its base types. `InjectAll` applies them in ascending `TypePriority`.
- `Extensions/DependencyInjectionExtensions.cs` — the `IServiceCollection` hookup.

The lifecycle interfaces (`IOnLoad`, `IOnUpdate`, `IOnDIConstruct`) live in **Core**'s `DI/` folder,
not here — this project knows nothing about server startup.

## SPTarkov.Reflection

Runtime method patching for mods, over HarmonyX.

- `Patching/` — `AbstractPatch` (the base a mod patch derives from), `IRuntimePatch`,
  `PatchManager`, `PatchException`, `Attributes.cs` (`[PatchPrefix]`, `[PatchPostfix]`, …).
- `CodeWrapper/` — `Code`, `CodeWithLabel`, `CodeGenerator`: IL emit helpers for transpilers.

## SPTarkov.Server.Core

All game logic — 840 of the 926 `.cs` files under `Libraries/`. Referenced by `SPTarkov.Server.Web`
and by the host; references only `SPTarkov.Common` and `SPTarkov.DI`.

→ **[`SPTarkov.Server.Core/ARCHITECTURE.md`](SPTarkov.Server.Core/ARCHITECTURE.md)** for per-folder
contents, dispatch mechanics, the item-event batch protocol, `OnLoadOrder` values, config-loading
behaviour, the JSON layer, and model conventions.

## SPTarkov.Server.Web

Blazor Server admin panel (MudBlazor), served by the same Kestrel host.

| Folder | Contents |
|---|---|
| `Pages/` | Routed pages: `/` `LandingPage`, `/login`, `/profiles` `ProfileControlPage`, `/credentials` `UserCredentialsPage`, `/configs` `ConfigEditorPage`, `/database` `DatabasePage`, `/tools` `ToolsPage`, `/status`, `/thank-you`, `/example-page` |
| `Pages/Database/` | `DatabasePage` split into ten `.razor.cs` partials — one per table (Items, Quests, Traders, Bots, Globals, Handbook, Achievements, Customization) plus `Filters` and `Formatting` |
| `Layout/` | `BaseMainLayout`, `BaseMudBlazorLayout` — the shells mod pages inherit from |
| `Components/` | Reusable pieces grouped `Auth/` (1), `Configs/` (2), `Database/` (9), `Profiles/` (6) |
| `Models/` | View models: `Database/` (13 — rows, columns, filters, trader assorts), `Profiles/` (12 — edit models for quests, skills, hideout, prestige, traders), `Configs/` (4) |
| `Services/` | `AuthService` + `IPasswordHasher`/`Argon2idPasswordHasher`, `ConfigEditorService` + `IConfigEditorConfigProvider`, `WebLocalizationService` |
| `Utils/` | `JsonPropertyFlattener` (drives the structured config editor) |
| root | `SPTWeb.cs` (registration + three minimal-API routes: login, logout, profile download), `App.razor`, `Routes.razor`, `_Imports.razor`, `IModBlazorMetadata.cs` |

`IModBlazorMetadata` is the marker a mod assembly implements to have its `wwwroot` linked and its
Blazor pages and MVC controllers registered — the one place in the solution where MVC controllers
are supported at all.

Those three minimal APIs in `SPTWeb.cs` plus `/health` in the host are the only routes in the
solution that bypass the router pipeline.

## SPTarkov.Server.Assets

Content project, no runtime code. Ships `SPT_Data/` — `configs/`, `database/`, `images/` and the
generated `checks.dat` — plus `looseLoot.7z` (unpacked by `scripts/decompress-assets.sh`). The
build also relocates `dotnet/` satellite assemblies and `wwwroot/` admin-panel assets into the
output `SPT_Data`, which is why neither is covered by hash verification. Its one `.cs` file is
`build/PostBuild.cs`, an MSBuild-time task that hashes `SPT_Data` into `checks.dat` with
`System.IO.Hashing.XxHash128` on Release builds. That hash format is a contract shared with
`rust/spt-native/src/verify.rs`; see the root ARCHITECTURE.md.

Excluded from the knowledge graph by `.graphifyignore`.
