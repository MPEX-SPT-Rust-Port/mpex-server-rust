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
SPTarkov.Server.Assets                          (content-only; the host project-references it
                                                 solely to copy SPT_Data to output)
```

| Project | .cs files | Depends on | NuGet of note |
|---|---:|---|---|
| `SPTarkov.Common` | 22 | — | SemanticVersioning, ZLinq |
| `SPTarkov.DI` | 3 | — | MS.Extensions.DependencyInjection.Abstractions, Hosting.Abstractions |
| `SPTarkov.Reflection` | 8 | `SPTarkov.DI` | HarmonyX |
| `SPTarkov.Server.Core` | 847 | `SPTarkov.Common`, `SPTarkov.DI` | HarmonyX, FastCloner, System.IO.Hashing, MessagePack |
| `SPTarkov.Server.Web` | 50 | `SPTarkov.Server.Core` | MudBlazor, Argon2Sharp |
| `SPTarkov.Server.Assets` | 0 | — | — |

(`.cs` only, `obj/`+`bin/` excluded — `SPTarkov.Server.Web` additionally has 34 `.razor` components.
Core's 847 includes the build-generated `Utils/ProgramStatics.Generated.cs`, so 846 are hand-written.)

---

## SPTarkov.Common

Framework-agnostic primitives. It has no project references at all, so it structurally cannot see
a game type — no `MongoId`, no `Item`, no config record. Logging, semver and generic extensions only.

| Folder | Contents |
|---|---|
| `Logger/` | `SptLogger`, `SptLoggerProvider`, `SPTLoggerDispatcher`, `SptEarlyLoggerFactory` (the pre-DI logger used during startup), `SptLoggerWrapper` (adapts the dispatcher to `Microsoft.Extensions.Logging.ILogger`) |
| `Logger/Handlers/` | `BaseLogHandler` only. Its two implementations moved to Rust; the abstract class stays because the 4.1.2 public surface is frozen |
| `Native/` | `NativeMethods` — the `spt_logger_init` / `spt_log_emit` / `spt_logger_close` / `spt_buf_free` P/Invokes the dispatcher writes through |
| `Models/Logging/` | `ISptLogger`, `ILogHandler`, `SptLogMessage`, `SptLoggerConfiguration` (bound from `sptLogger.json`), `FileLogger` (empty marker type used as a log category) |
| `Semver/` | `ISemVer` + `SemanticVersioningSemVer` — used for mod `SptVersion` range checks |
| `Extensions/` | `String`, `List`, `Object`, `MemberInfo`, `HttpContext`, and two logger extension sets |
| `Json/Converters/` | `BaseSptLoggerReferenceConverter` — resolves a logger config entry's `"type"` to `File`/`Console` |

The `sptLogger.json` / `sptLogger.Development.json` files the server refuses to start without are
this project's config. It is the front end only: `AddSptLogger` hands the raw config bytes to
`rust/spt-native`, which owns the filters, the level gate, formatting and the console/file sinks.

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

All game logic — 847 of the 930 `.cs` files under `Libraries/`. Referenced by `SPTarkov.Server.Web`
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
| `Layout/` | `BaseMainLayout` (the `DefaultLayout` on `Routes.razor`'s `AuthorizeRouteView`), `BaseMudBlazorLayout` (`@layout` on all ten built-in pages) — also the shells a mod page can opt into |
| `Components/` | Reusable pieces grouped `Auth/` (1), `Configs/` (2), `Database/` (9), `Profiles/` (7) |
| `Models/` | View models: `Database/` (13 — rows, columns, filters, trader assorts), `Profiles/` (12 — edit models for quests, skills, hideout, prestige, traders), `Configs/` (4) |
| `Services/` | `AuthService` + `IPasswordHasher`/`Argon2idPasswordHasher`, `ConfigEditorService` + `IConfigEditorConfigProvider`, `WebLocalizationService` |
| `Utils/` | `JsonPropertyFlattener` — `BuildProperties(json)` → `DatabaseProperty` rows; drives the record-detail views in `ProfileControlPage` and eight `DatabasePage` table partials (*not* the config editor) |
| root | `SPTWeb.cs` (registration + three minimal-API routes: login, logout, profile download), `App.razor`, `Routes.razor`, `_Imports.razor`, `IModBlazorMetadata.cs` |

`IModBlazorMetadata` is the marker a mod assembly implements to have its `wwwroot` linked and its
Blazor pages and MVC controllers registered — the one place in the solution where MVC controllers
are supported at all.

Those three minimal APIs in `SPTWeb.cs` plus `/health` in the host are the only *minimal-API*
routes in the solution. They are not the only traffic outside the router pipeline: the same method
also calls `MapRazorComponents<App>()` and `MapControllers()`, so every admin-panel page and every
mod MVC controller is routed by ASP.NET too.

## SPTarkov.Server.Assets

Content project, no code at all — zero `.cs` files, just the `.csproj` and the payload. Ships
`SPT_Data/` — `configs/`, `database/`, `images/` and the generated `checks.dat` — plus
`looseLoot.7z` (unpacked by `scripts/decompress-assets.sh`). `SPTarkov.Server`'s build relocates
`dotnet/` satellite assemblies and `wwwroot/` admin-panel assets into the *output* `SPT_Data`,
which is why neither is covered by hash verification.

`checks.dat` is regenerated by the `PreBuildHashFile` target in `SPTarkov.Server.Assets.csproj` on
Release builds only: `cargo run --locked --release --bin gen_checks -- <SPT_Data>`. That bin
deliberately runs without `$(CargoTargetFlag)` — it executes on the build host, so a cross-RID
publish still hashes with a host-triple binary. The bin is a thin wrapper over `verify::generate` in
`rust/spt-native/src/verify.rs`, so the writer and the startup verifier are literally the same
XXH3-128 code path; see the root ARCHITECTURE.md.

Excluded from the knowledge graph by `.graphifyignore`.
