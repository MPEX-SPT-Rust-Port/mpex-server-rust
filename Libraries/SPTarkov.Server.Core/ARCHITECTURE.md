# SPTarkov.Server.Core — Architecture

All game logic. 839 tracked `.cs` files, ~91% of the code under `Libraries/`. Every path below is
relative to `Libraries/SPTarkov.Server.Core/`.

This file is a map of Core: what each folder is for and where to add things. For behaviour that
spans the repo (Rust FFI contract, build pipeline, mods) see the [root
ARCHITECTURE.md](../../ARCHITECTURE.md); for which of the six library projects owns what, see
[`Libraries/ARCHITECTURE.md`](../ARCHITECTURE.md).

Core references only `SPTarkov.Common` and `SPTarkov.DI`, plus three NuGet packages: `HarmonyX`
(the dual-path generators and `BotWaveBatcher` use it to detect live patches), `FastCloner` (behind `Utils/Cloners/`)
and `System.IO.Hashing`. Core's csproj is also what invokes `cargo` to build `rust/spt-native`, so
any `dotnet build` needs the Rust toolchain on `PATH`.

## Folder map

| Folder | Files | What it is |
|---|---:|---|
| `Models/` | 437 | Data records. `Eft/` client wire contracts, `Spt/` server-internal, `Enums/`, `Common/` primitives |
| `Helpers/` | 68 | Stateless computation (29 of them the `Dialogue/` chat-bot framework) |
| `Services/` | 55 | Stateful, long-lived, cache-owning |
| `Routers/` | 50 | URL → callback dispatch (`Static/` 23, `ItemEvents/` 11, `Dynamic/` 7, `SaveLoad/` 4, `Serializers/` 2, root 3) |
| `Utils/` | 43 | JSON layer (23), RNG, cloning, collections, IO, importers. Plus gitignored `ProgramStatics.Generated.cs` |
| `Callbacks/` | 34 | HTTP entry point per domain |
| `Generators/` | 33 | Build game data from scratch |
| `Controllers/` | 30 | Orchestration |
| `Extensions/` | 23 | Domain extension methods, one file per extended type |
| `Migration/` | 21 | Versioned profile migrations |
| `Exceptions/` | 13 | Typed exceptions (`Database/`, `Helpers/`, `Items/`) |
| `Servers/` | 11 | HTTP, WebSocket, save, ragfair |
| `DI/` | 8 | Router base classes + lifecycle interfaces |
| `Native/` | 10 | C# side of the Rust FFI |
| `Constants/` | 5 | Id and slot-name constants |
| `Loaders/` | 2 | `ConfigLoader`, `BundleLoader` |

## Request pipeline

No MVC, no attribute routing. Host middleware → `Servers/HttpServer` → an `IHttpListener` →
`Routers/HttpRouter`, which tries every static router first and falls back to dynamic routers only
if none matched.

`HttpServer.HandleRequestAsync` picks one of three destinations, and only the last reaches the
router layer:

1. WebSocket upgrades → `Servers/Ws/`.
2. The first `IHttpListener` that claims the request. There are two, and it is worth knowing which:
   `Routers/ImageRouter` serves static image files directly and **never touches callbacks** (it
   lives in `Routers/` but is a listener, not a `Router`); `Servers/Http/SptHttpListener` is the
   normal pipeline below.
3. Nothing claims it → falls through to the host's minimal APIs and the admin panel.

`SptHttpListener` owns the wire concerns so nothing downstream has to: session id from the
`PHPSESSID` cookie, zlib compression both ways, and the `ISerializer` escape hatch for the few
non-JSON responses (`Routers/Serializers/` — bundles and notifications). Two behaviours surprise
people: it serves **only GET, PUT and POST**, and an unmatched route still returns HTTP 200, with
the 404 signalled inside the response envelope.

**An endpoint = a router entry + a callback method + (usually) a controller method.** Three files.

- Static routes match the URL exactly; dynamic routes match by *substring* anywhere in the URL.
- A concrete router subclasses `StaticRouter`/`DynamicRouter` and passes `RouteAction<TRequest>`
  records (url + delegate) to its base constructor. The base deserializes the body into `TRequest`
  via `JsonUtil`, using `EmptyRequestData` when there is no body.
- Duplicate URLs: `StaticRouter` throws, `DynamicRouter` silently takes the first, and across
  routers the last one enumerated wins — `HttpRouter` runs *every* matching router, not just the
  first, and each overwrites the output.
- `Router` raises `OnBeforeAction`/`OnAfterAction` events around every static, dynamic, item event
  and save-load invocation — the seam for observing a route without replacing its registration.
- `Callbacks/` deserialize, delegate, serialize. Responses go through `Utils/HttpResponseUtil`,
  which wraps them in the client envelope (`data` + `err` + `errmsg`) — pick the wrong helper
  (`GetBody`, `GetUnclearedBody`, `NullResponse`, `EmptyResponse`, `EmptyArrayResponse`) and the
  game silently rejects the response. `NoBody` is the odd one out: it serializes raw, no envelope.
  `AppendErrorToOutput` is how you add a warning to an item-event response.
- The controller layer is optional: four callbacks (`BtrDelivery`, `Bundle`, `ItemEvent`, `Save`)
  call services or infrastructure directly.
- Name traps: `BuildsCallbacks`/`BuildController`, `Handbook`/`HandBook`, `Inraid`/`InRaid`, and
  `Routers/Static/ModLoaderRouter.cs` is the one file there not named `*StaticRouter`.

### Item events

`POST /client/game/profile/items/moving` carries a *batch* of actions and is the busiest path in
the server. The 11 routers under `Routers/ItemEvents/` are keyed by exact action name and all
mutate one shared `ItemEventRouterResponse`, owned per-session by the `EventOutputHolder` singleton;
the client receives the union. One warning aborts the rest of the batch (already-applied actions
are kept), and every warning code except `NotEnoughSpace` turns the whole response into an error.

`ItemEventCallbacks` drives the loop. Two things to know before adding an action: item event bodies
derive from `BaseInteractionRequestData`, **not** `IRequestData`, so they use `ItemRouteAction`
rather than the `RouteAction<TRequest>` machinery above; and an *unrecognised* action is logged and
skipped rather than aborting the batch — only a warning stops it.

`Routers/SaveLoad/` is the profile-side sibling: unconditional structural fixes applied on every
profile load. Version-gated changes go in `Migration/` instead.

## Lifecycle and DI

The `[Injectable]` attribute and container live in `SPTarkov.DI`; the lifecycle contracts live here
in `DI/`. Registration happens in one place — the host's `ProgramHelpers.RegisterSptServicesAsync`.

- **`IOnLoad`** — startup work, ordered by the `DI/OnLoadOrder` constants (`Watermark` 0 →
  `PostLoad` 1000000, in 100000 steps that leave headroom for mods). Anything below `GameCallbacks`
  runs before Kestrel binds, which is what lets a mod mutate `HttpConfig` in time.
- **`IOnUpdate`** — polled every 5 s by `Services/Hosted/SPTStartupHostedService`. Four
  `OnUpdateOrder` constants: dialogue, hideout, insurance, BTR delivery.
- **`IOnDIConstruct`** — static hook running after `InjectAll`; the only reliable way for a mod to
  replace a core registration.
- **`ISerializer`** — non-JSON responses; implementations in `Routers/Serializers/`.

## Configuration

`Loaders/ConfigLoader` is a **static class, not `[Injectable]`** — it runs before the container
exists. It maps each file in `SPT_Data/configs/` to a `BaseConfig` subclass in
`Models/Spt/Config/`; the host registers each as its own singleton, so services inject e.g.
`BotConfig` directly. `LocationConfig.ForceLegacyLootGeneration` and
`BotConfig.ForceLegacyBotGeneration` are the Rust-port escape hatches.

## JSON layer (`Utils/Json/`, 23 files)

System.Text.Json with a custom converter set for the client's loose typing: union types
(`ListOrT<T>`, `StringOrInt`, `DictionaryOrList`, `FloatOrIrregularFloatArray`), 16
`Converters/` (enums, `MongoId`, number coercion), and `LazyLoad<T>`, which defers parsing the huge
database files and whose transformer hook is the supported way to modify loose loot.

Gotcha: the converter list exists twice — in `SptJsonConverterRegistrator` (runtime, discovered via
DI) and hardcoded in `ConfigLoader` (pre-DI). A converter that configs need must be added to both.

## Logic layers

Convention: **Services** hold state, **Helpers** don't, **Generators** create data.

### `Services/` (55) — stateful, mostly singletons

| Subfolder | Files | Owns |
|---|---:|---|
| `InRaid/` | 9 | `LocationLifecycleService` (raid start/end), airdrops, BTR, custom waves, goon spawns, open zones, raid time, raid weather |
| `Bot/` | 8 | Equipment filters, mod pools, loot cache, name pool, weapon mod limits, per-match cache, PMC chat responses |
| `Commerce/` | 7 | Fence, gifts, insurance, `MailSendService`, payment, repair, trader purchase persistence |
| `Ragfair/` | 6 | Offers, prices, categories, linked items, required items, tax |
| `Profile/` | 5 | Backup, creation, activity, `ProfileFixerService`, `ProfileMigrationService` |
| `Modding/` | 5 | Custom item/quest registration, mod item cache, profile data access |
| `Server/` | 5 | `PostDbLoadService` (post-import DB adjustments), `SeasonalEventService`, notifications, bundle hashes, `DatabaseMutationStamp` |
| `Locales/` | 3 | `ServerLocalisationService` (server messages), `LocaleService` (client locales) |
| `Items/`, `Hideout/`, `Hosted/`, `Image/` | 7 | Item blacklists/base classes, cultist circle + map markers, startup hosted service, image routing |

`ServerLocalisationService.GetText(key, args)` produces every user-visible server string, including
the diagnostics the Rust port replays back across the FFI.

### `Helpers/` (68) — stateless computation

Grouped by domain: `Profile/` (8, incl. `HideoutHelper`, `InventoryHelper`, `ProfileHelper`),
`Ragfair/` (6), `Bot/` (5), `Server/` (5), `Commerce/`, `InRaid/`, `Quest/`, `Traders/` (3 each),
`Items/` (`ItemHelper`, `PresetHelper`), plus `WeightedRandomHelper`.

`Helpers/Dialogue/` (29) is not helpers — it's the in-game chat-bot framework:
`AbstractDialogChatBot` → `SptDialogueChatBot` / `CommandoDialogChatBot`, with `Commando/SptCommands/`
and 15 `IChatMessageHandler` implementations under `SPTFriend/Commands/`.

### `Generators/` (33) — build game data

| Subfolder | Contents |
|---|---|
| `Bot/` | `BotGenerator`, `BotInventoryGenerator`, `BotEquipmentModGenerator`, `BotWeaponGenerator`, `BotLevelGenerator`, `PlayerScavGenerator` |
| `Loot/` | `LocationLootGenerator`, `LootGenerator`, `BotLootGenerator`, `PMCLootGenerator` |
| `RepeatableQuests/` | Completion / elimination / exploration / pickup generators + reward generator |
| `Weapons/` | `IInventoryMagGen` and four implementations: barrel, external, internal magazine, UBGL |
| `Weather/`, `Ragfair/`, root | Weather presets; ragfair offers/assorts; fence assorts, PMC waves, scav case rewards |

**Four are dual-path** — `LocationLootGenerator`, `LootGenerator`, `BotInventoryGenerator` and
`RagfairOfferGenerator` forward to Rust by default and keep their 4.1.2 implementation as a legacy
fallback. They use HarmonyX to detect a live patch and fall back, as does `BotWaveBatcher` — five
files in Core reference it.

`BotInventoryGenerator` is the single entry point for the whole bot inventory, so
`BotWeaponGenerator`, `BotEquipmentModGenerator` and `BotLootGenerator` do **not** forward to Rust
themselves — they run only on the legacy path. They still participate in the decision: patching or
replacing any of them, or deviating from the exact set of four built-in `IInventoryMagGen`
implementations, forces the whole of bot generation back onto legacy. Dispatch conditions and
`forceLegacy*` flags are in the root ARCHITECTURE.md.

## Models (437 files, over half of Core)

| Namespace | Files | Contract |
|---|---:|---|
| `Models/Eft/` | 252 | Mirrors live client wire types — changing a member changes what the game receives |
| `Models/Spt/` | 99 | Server-internal, never sent to the client |
| `Models/Enums/` | 81 | Game enums, incl. generated `ItemTpl` / `QuestTpl` constants |
| `Models/Common/` | 3 | `MongoId`, `MinMax`, `IdWithCount` |
| `Models/Utils/`, root | 2 | `IRequestData`, `RadioStationType` |

- **`MongoId`** — the 24-hex-char id keying every item, template and profile, and by a wide margin
  the most connected type in the repo.
- **`Models/Eft/Common/PmcData`** — the second most connected, and `PmcData : BotBase`. The player
  character is modelled as a bot, which is why bot helpers and generators operate on player
  profiles unchanged. Threaded through every item event and most controllers.
- **`Models/Eft/Common/Tables/`** — the hottest types: `Item`, `TemplateItem`, `BotBase`, `Trader`,
  `Quest`, `Reward`.
- **`Models/Spt/Tables/`** — the ten in-memory database tables. The host registers each as its own
  singleton and Core injects them individually; there is no database service, and the aggregate
  `DatabaseTables` record lives host-side.
- **`IRequestData`** is an empty marker interface but load-bearing: `RouteAction<TRequest>` is
  constrained on it and `DI/Router.cs` uses it to pick a deserialization target.

On Release builds and any publish, `Tools/Ceciler` IL-injects a `[JsonExtensionData]` property into
every `Models` type so unknown client fields round-trip. `Utils/Reference/StaticReferences.cs` is
the template it copies — **do not delete it**, despite nothing referencing it in source.

## Infrastructure

| Folder | Contents |
|---|---|
| `Servers/` | `HttpServer`, `WebSocketServer`, `SaveServer` (owns `user/profiles/`), `RagfairServer`; `Http/` listener + `RequestLogger`; `Ws/` connection and message handlers |
| `Native/` | `NativeMethods` (`[LibraryImport]`), `SptNative` (safe wrapper), `Loot/`, `Bot/` and `Ragfair/` payload + projection types. The ragfair request's call-invariant half is skipped when `DatabaseMutationStamp` has not moved and no mods are loaded. Contract details in the root ARCHITECTURE.md |
| `Migration/` | `IProfileMigration` / `AbstractProfileMigration` / context, versioned sets under `Migrations/3.11`, `4.0`, `4.1`, plus unversioned `Migrations/Fixes/` (7 corruption repairs) |
| `Constants/` | `BodyPartContants` (typo is in the source), `ContainerConstants`, `RoleConstants`, `SideConstants`, `SlotConstants` |
| `Exceptions/` | `Helpers/` (7, one per helper that throws), `Items/` (3, modded item/trader/clothing validation), `Database/` (3, incl. `DatabaseTablesAlreadySetException`) |

`WebSocketServer` matches `IWebSocketConnectionHandler.GetHookUrl()` as a substring of the path and
notifies *every* matching handler — unlike `IHttpListener`, where only the first match runs.

### `Utils/` (43)

- `RandomUtil` + `RandomSource` — `IRandomSource` is the test-only seam letting C# and Rust share a
  seeded xoshiro256\*\* for parity tests. Production randomness is unchanged.
- `Collections/` — `ProbabilityObjectArray` (weighted draws, has a Rust twin), `ExhaustableArray`.
- `Cloners/` — `ICloner` + `FastCloner`; item-event responses are cloned before return.
- `ImporterUtil` / `ImageRouteImporter` — the database and image tree imports the host's
  `DatabaseImporter` drives.
- `ProgramStatics` is `static partial`; its other half `Utils/ProgramStatics.Generated.cs` is
  written at build time from MSBuild properties. **Never edit the generated file.**
- Also `FileUtil`, `HashUtil`, `HttpFileUtil`, `HttpResponseUtil`, `JsonUtil`, `MathUtil`,
  `TimeUtil`, `Watermark`, `RagfairOfferHolder`.

## Conventions

- Mark a class `[Injectable]` and register it in `ProgramHelpers.RegisterSptServicesAsync`.
  `DependencyInjectionValidationTests` rebuilds that container with mods on and off, so a bad
  registration fails the test run rather than a launch.
- An endpoint is a router entry + a callback + (usually) a controller. Never an `[HttpGet]`.
- Item-moving actions go in `Routers/ItemEvents/`; profile-load fixes in `Routers/SaveLoad/` if
  unconditional, `Migration/` if versioned.
- Style: file-scoped namespaces, `_camelCase` private fields, no `this.`, language keywords over BCL
  types, braces on all single-line bodies, no expression-bodied members. `csharpier format .` before
  a PR.
