# **SPTarkov.Server.Core** Architecture

## 1. Overview Summary

All game logic — a map of Core: what each folder is for and where to add things. 854 tracked `.cs` files
(plus the build-generated `Utils/ProgramStatics.Generated.cs`), ~91% of the `.cs` files under `Libraries/`. Every
path below is relative to `Libraries/SPTarkov.Server.Core/`. Core references `SPTarkov.Common` +
`SPTarkov.DI`, four NuGet packages (`HarmonyX` for live-patch detection, `FastCloner`, `MessagePack`,
`System.IO.Hashing`), and `Spectre.Console.Ansi` — a Rust-emitted facade kept only to hold the mod ABI (four
files still name Spectre types). Core's csproj invokes `cargo`, so any `dotnet build` needs the Rust
toolchain on `PATH`.

| Language | Lines of Code | File Count |
|-----------|-----------------|-----------|
| `C#` | `117,812` | `855` |

---

## 2. High Level Design

No MVC, no attribute routing. **An endpoint = a router entry + a callback method + (usually) a
controller method.**

```
HttpServer.HandleRequestAsync
  ├─ WebSocket upgrade ────────────────────────────────► Servers/Ws/
  ├─ Routers/ImageRouter        (IHttpListener, serves image files, never touches callbacks)
  ├─ Servers/Http/SptHttpListener  (IHttpListener, the normal pipeline)
  │     → HttpRouter → Routers/Static (exact URL) else Routers/Dynamic (substring)
  │     → Callbacks/  → Controllers/ → Services/ | Helpers/ | Generators/
  │                                        ↕
  │                                   Models/Spt/Tables (DI singletons)  ↔  Native/ → rust/spt-native
  └─ nothing claims it ─────────────────────────────────► host minimal APIs, admin panel
```

### Folder map

| Folder | Files | What it is |
|---|---:|---|
| `Models/` | 437 | Data records. `Eft/` client wire contracts, `Spt/` server-internal, `Enums/`, `Common/` primitives |
| `Helpers/` | 68 | Stateless computation (29 of them the `Dialogue/` chat-bot framework) |
| `Services/` | 55 | Stateful, long-lived, cache-owning |
| `Routers/` | 50 | URL → callback dispatch via declarative route records (`Static/` 23, `ItemEvents/` 11, `Dynamic/` 7, `SaveLoad/` 4, `Serializers/` 2, root 3) |
| `Utils/` | 43 | JSON layer (23), RNG, cloning, collections, IO, importers. Plus gitignored `ProgramStatics.Generated.cs` |
| `Callbacks/` | 34 | HTTP entry point per domain: deserialize in, serialize out via `HttpResponseUtil` |
| `Generators/` | 34 | Build game data from scratch; nine forward to Rust by default |
| `Controllers/` | 30 | Orchestration. Optional — four callback families skip it entirely |
| `Extensions/` | 23 | Domain extension methods. The file name is usually the extended type, but not always — `ProfileExtensions` extends `PmcData`, `FullProfileExtensions` extends `SptProfile` |
| `Migration/` | 21 | `IProfileMigration` / `AbstractProfileMigration`, versioned sets (`3.11`, `4.0`, `4.1`) plus unversioned `Migrations/Fixes/` (7 corruption repairs) |
| `Native/` | 20 | C# side of the Rust FFI: P/Invokes, payload projections, `DbPublisher` |
| `Exceptions/` | 13 | Typed exceptions (`Helpers/` 7, `Items/` 3, `Database/` 3) |
| `Servers/` | 11 | HTTP, WebSocket, save (`user/profiles/`), ragfair state |
| `DI/` | 8 | Router base classes + lifecycle interfaces (`IOnLoad`, `IOnUpdate`, `IOnDIConstruct`) |
| `Constants/` | 5 | Game string constants. Classes are named for the plural (`BodyParts`, `Slots`), never for their `*Constants.cs` file; the `BodyPartContants` typo is in the source |
| `Loaders/` | 2 | `ConfigLoader` (static, pre-DI), `BundleLoader` |

---

## 3. Low Level Design

### Request pipeline

`HttpServer.HandleRequestAsync` picks one of three destinations:

1. WebSocket upgrades → `Servers/Ws/`.
2. The first `IHttpListener` that claims it. There are exactly two: `Routers/ImageRouter` serves
   image files directly and never touches callbacks (it lives in `Routers/` but is a listener, not a
   `Router`); `Servers/Http/SptHttpListener` is the normal pipeline and the only consumer of
   `HttpRouter`.
3. Nothing claims it → host minimal APIs and the admin panel.

`SptHttpListener` owns the wire concerns: zlib both ways, the `PHPSESSID` cookie on the way out, and
the `ISerializer` escape hatch for non-JSON responses (`Routers/Serializers/`). Two surprises: it
serves **only GET, PUT and POST**, and an unmatched route still returns HTTP 200 with the 404 inside
the response envelope.

Routing rules worth knowing:

- Static routes match the URL exactly; dynamic routes match by *substring*. `HttpRouter` tries all
  static routers first and only falls back to dynamic if none matched.
- `HttpRouter` runs *every* matching router, not just the first, and each overwrites the output — so
  across routers the last one enumerated wins. Within a router, duplicate URLs make `StaticRouter`
  throw and `DynamicRouter` silently take the first.
- Concrete routers pass `RouteAction<TRequest>` records to their base constructor; the base
  deserializes the body via `JsonUtil`, using `EmptyRequestData` when there is no body.
- `Router` raises `OnBeforeAction`/`OnAfterAction` around static, dynamic and save-load invocations —
  the seam for observing a route. Item events are the exception: `ItemEventRouter.HandleItemEvent`
  never fires them, which leaves the item-event record types in `DI/Router.cs` dead.
- Responses go through `Utils/HttpResponseUtil`, which wraps them in the client envelope
  (`data`/`err`/`errmsg`). Picking the wrong helper gets the response silently rejected by the game;
  `NoBody` is the one that serializes raw, with no envelope.
- The controller layer is optional — `BtrDelivery`, `Bundle`, `ItemEvent` and `Save` callbacks skip it.
- Name traps: `BuildsCallbacks`/`BuildController`, `Handbook`/`HandBook`, `Inraid`/`InRaid`, and
  `Routers/Static/ModLoaderRouter.cs` is the one file there not named `*StaticRouter`.

#### Item events

`POST /client/game/profile/items/moving` carries a *batch* of actions and is the busiest path in the
server. The 11 `Routers/ItemEvents/` routers are keyed by exact action name and all mutate one shared
`ItemEventRouterResponse`, owned per-session by `EventOutputHolder`; the client receives the union.
One warning aborts the rest of the batch (already-applied actions are kept), and every warning code
except `NotEnoughSpace` turns the whole response into an error. An *unrecognised* action is logged
and skipped, not aborted.

Item event bodies derive from `BaseInteractionRequestData`, **not** `IRequestData`, so they use
`ItemRouteAction` rather than `RouteAction<TRequest>`.

`Routers/SaveLoad/` is the profile-side sibling: unconditional structural fixes on every profile
load. Version-gated changes go in `Migration/` instead.

### Lifecycle and DI

`[Injectable]` and the container live in `SPTarkov.DI`; the lifecycle contracts live here in `DI/`.
Registration happens in one place — the host's `ProgramHelpers.RegisterSptServicesAsync`.

- **`IOnLoad`** — startup work ordered by `DI/OnLoadOrder` (`Watermark` 0 → `PostLoad` 1000000, in
  100000 steps that leave headroom for mods). Anything below `GameCallbacks` runs before Kestrel
  binds, which is what lets a mod mutate `HttpConfig` in time.
- **`IOnUpdate`** — polled every 5 s by `Services/Hosted/SPTStartupHostedService`. Four
  `OnUpdateOrder` constants: dialogue, hideout, insurance, BTR delivery.
- **`IOnDIConstruct`** — static hook after `InjectAll`; the only reliable way for a mod to replace a
  core registration.
- **`ISerializer`** — non-JSON responses; implementations in `Routers/Serializers/`.

### Configuration

`Loaders/ConfigLoader` is a **static class, not `[Injectable]`** — it runs before the container
exists. It maps each file in `SPT_Data/configs/` to a `BaseConfig` subclass in `Models/Spt/Config/`;
the host registers each as its own singleton, so services inject e.g. `BotConfig` directly.

Eight `ForceLegacy*` flags are the Rust-port escape hatches, one per dual-path family:
`LocationConfig.ForceLegacyLootGeneration`, `BotConfig.ForceLegacyBotGeneration`,
`RagfairConfig.ForceLegacyRagfairGeneration`, `RagfairConfig.ForceLegacyRagfairLinkedItemBuild`,
`QuestConfig.ForceLegacyRepeatableQuestGeneration`, `ScavCaseConfig.ForceLegacyScavCaseGeneration`,
`ItemConfig.ForceLegacyItemBaseClassHydration` and `LocationConfig.ForceLegacyRaidAdjustments`
(the raid family is one flag across two services). `CoreConfig.ForceLegacyDatabaseImport` is a ninth
`ForceLegacy*` flag but is **not** in that count — the database import is not a dual-path generation
family. Narrower knobs: `BotConfig.ForcePerBotGeneration` (unbatch waves without leaving native)
and, on the six configs backing the resident-DB families (`LocationConfig`, `ItemConfig`,
`ScavCaseConfig`, `QuestConfig`, `RagfairConfig`, `BotConfig`), `TrustNativeRequestCacheWithMods` /
`DisableNativeRequestCache` — the names are legacy, they now gate resident-DB eligibility rather
than a request cache. Since Phase 2 `TrustNativeRequestCacheWithMods` defaults **on**, and is
honoured only in a build carrying the Ceciler write barriers (Release or publish, never Debug).

### JSON layer (`Utils/Json/`, 23 files)

System.Text.Json with a custom converter set for the client's loose typing: union types
(`ListOrT<T>`, `StringOrInt`, `DictionaryOrList`, `FloatOrIrregularFloatArray`), 16 `Converters/`
(enums, `MongoId`, number coercion), and `LazyLoad<T>`, which defers parsing the huge database files
and whose transformer hook is the supported way to modify loose loot.

Only 9 of the 16 converters are registered globally; the rest are opt-in per property. **Gotcha:**
that global list exists twice — in `SptJsonConverterRegistrator` (runtime, via DI) and hardcoded in
`ConfigLoader` (pre-DI), as two identical 9-entry arrays. A converter configs need goes in both.

### Logic layers

Convention: **Services** hold state, **Helpers** don't, **Generators** create data. All three are
split into domain subfolders; the folder listing is the roster, and what follows is only what the
folder names don't tell you.

- **`Services/`** (55) — `InRaid/`, `Bot/`, `Commerce/`, `Ragfair/`, `Profile/`, `Modding/`,
  `Server/`, `Locales/`, `Items/`, `Hideout/`, `Hosted/`, `Image/`. `InRaid/LocationLifecycleService`
  owns raid start/end. `Server/DatabaseMutationStamp` is what resident-DB freshness keys off.
  `Locales/` splits two ways: `ServerLocalisationService` (server messages) and `LocaleService`
  (client locales).
- **`Helpers/`** (68) — `Helpers/Dialogue/` is 29 of those 68 and is not helpers at all: it's the
  in-game chat-bot framework, `AbstractDialogChatBot` → `SptDialogueChatBot` /
  `CommandoDialogChatBot`, with 15 `IChatMessageHandler` implementations under `SPTFriend/Commands/`.
- **`Generators/`** (34) — `Bot/`, `Loot/`, `RepeatableQuests/`, `Weapons/`, `Weather/`, `Ragfair/`,
  plus fence assorts, PMC waves and scav case rewards at the root. `Weapons/` holds the
  `IInventoryMagGen` set, whose exact membership is load-bearing (see below).

`ServerLocalisationService.GetText(key, args)` produces every user-visible server string. Its
resolved table is flattened and pushed to Rust at database import (`spt_locales_set`), which renders
its own generator diagnostics against that startup snapshot.

### Dual-path (Rust) sites

**Thirteen** classes forward to Rust by default and keep their 4.1.2 implementation as a legacy
fallback — nine generators (`LocationLootGenerator`, `LootGenerator`, `BotInventoryGenerator`,
`RagfairOfferGenerator`, `ScavCaseRewardGenerator` and the four `RepeatableQuests/` quest-type
generators) and four services (`ItemBaseClassService`, `RagfairLinkedItemService`,
`RaidTimeAdjustmentService` and `LocationLifecycleService` — the last two share one frozen set, so a
patch on any of its seven members declines both). Each holds a frozen list of 4.1.2 members and uses
HarmonyX to detect a live patch before dispatching, as does `BotWaveBatcher`.

Two families fold collaborators into the native call, so those collaborators run legacy-only while
still participating in the dispatch decision:

- `BotInventoryGenerator` is the single entry point for the whole bot inventory, so
  `BotWeaponGenerator`, `BotEquipmentModGenerator` and `BotLootGenerator` never go native. Patching
  any of them, or deviating from the exact set of four built-in `IInventoryMagGen` implementations,
  forces all of bot generation back onto legacy.
- The repeatable-quest generators fold in `RepeatableQuestRewardGenerator` and
  `RepeatableQuestHelper`. Their frozen list spans all six types, so a patch anywhere in it forces
  all four quest-type generators onto legacy together. The four `Generate` dispatchers are
  deliberately excluded — a patch there wraps whichever path runs.

Full dispatch conditions are in [RUST-ROADMAP.md](../../RUST-ROADMAP.md) § *Exceptions in force*.

### Models (437 files, over half of Core)

| Namespace | Files | Contract |
|---|---:|---|
| `Models/Eft/` | 252 | Mirrors live client wire types — changing a member changes what the game receives |
| `Models/Spt/` | 99 | Server-internal, never sent to the client |
| `Models/Enums/` | 81 | Game enums, incl. generated `ItemTpl` / `QuestTpl` constants |
| `Models/Common/` | 3 | `MongoId`, `MinMax`, `IdWithCount` |
| `Models/Utils/`, root | 2 | `IRequestData`, `RadioStationType` |

- **`MongoId`** — the 24-hex-char id keying every item, template and profile; by a wide margin the
  most connected type in the repo.
- **`Models/Eft/Common/PmcData`** — second most connected, and `PmcData : BotBase`. The player is
  modelled as a bot, which is why bot helpers and generators operate on player profiles unchanged.
- **`Models/Eft/Common/Tables/`** — the hottest types: `Item`, `TemplateItem`, `BotBase`, `Trader`,
  `Quest`, `Reward`.
- **`Models/Spt/Tables/`** — the ten in-memory database tables, each registered host-side as its own
  singleton and injected individually. There is no database service; the aggregate `DatabaseTables`
  record lives host-side.
- **`IRequestData`** is an empty marker but load-bearing: `RouteAction<TRequest>` is constrained on
  it and `DI/Router.cs` uses it to pick a deserialization target.

On Release builds and any publish, `Tools/Ceciler` IL-injects `[JsonExtensionData]` into every
`Models` type so unknown client fields round-trip. `Utils/Reference/StaticReferences.cs` is the
template it copies — **do not delete it**, despite nothing referencing it in source.

### Infrastructure

`Native/` is `NativeMethods` (`[LibraryImport]`) plus `SptNative` (the safe wrapper), with the
payload/projection/request-builder types grouped into a subfolder per dual-path family and `Db/` for
the resident store. Contract details are in the root ARCHITECTURE.md.

Seven families ride the native resident DB — ragfair offers, repeatable quests, the two startup
one-shots (`ItemBaseClassService`, `RagfairLinkedItemService`), the loot pair (location loot, reward
loot) and scav case. Instead of a call-invariant half they carry an epoch from
`Native/Db/DbPublisher`, which republishes the templates/traders/globals/locations/hideout roots when
`DatabaseMutationStamp` moves. A stale epoch self-heals with a force-publish and one retry; an
ineligible caller ships the full views instead. Loot is the partial case: it still sends a varying
block, because `looseLoot` never goes resident.

`WebSocketServer` matches `IWebSocketConnectionHandler.GetHookUrl()` as a substring of the path and
notifies *every* matching handler — unlike `IHttpListener`, where only the first match runs.

#### `Utils/` (43)

- `RandomUtil` + `RandomSource` — `IRandomSource` is the test-only seam letting C# and Rust share a
  seeded xoshiro256\*\* for parity tests. Production randomness is unchanged.
- `Collections/` — `ProbabilityObjectArray` (weighted draws, has a Rust twin), `ExhaustableArray`.
- `Cloners/` — `ICloner` + `FastCloner`; item-event responses are cloned before return.
- `ImporterUtil` — the recursive database import the host's `DatabaseImporter` drives.
  `ImageRouteImporter` is unrelated: an `IOnLoad` of its own that maps `SPT_Data/images/` into
  `ImageRouter` at `Preload`.
- `ProgramStatics` is `static partial`; its other half `Utils/ProgramStatics.Generated.cs` is written
  at build time from MSBuild properties. **Never edit the generated file.**

### Conventions

Repo-wide style rules are in [CLAUDE.md](../../CLAUDE.md). Core-specific:

- Mark a class `[Injectable]` and register it in `ProgramHelpers.RegisterSptServicesAsync`.
  `DependencyInjectionValidationTests` rebuilds that container with mods on and off, so a bad
  registration fails the test run rather than a launch.
- An endpoint is a router entry + a callback + (usually) a controller. Never an `[HttpGet]`.
- Item-moving actions go in `Routers/ItemEvents/`; profile-load fixes in `Routers/SaveLoad/` if
  unconditional, `Migration/` if versioned.

---

## 4. Integration Points

| External System | Integration Type | Notes |
|-------------------|-------------------|-------|
| Escape from Tarkov game client | Sync HTTP | `Models/Eft/` mirrors its wire types field-for-field; `SptHttpListener` owns the wire concerns. GET, PUT, POST only |
| Game client (notifications) | Async WebSocket | `Servers/WebSocketServer` + `IWebSocketConnectionHandler`; payloads in `Models/Eft/Ws/`. Every matching handler is notified, not just the first |
| `rust/spt-native` (cdylib) | Sync FFI, C ABI | `Native/NativeMethods.cs` (`[LibraryImport]`) + `SptNative`; thirteen dual-path classes, plus `DbPublisher`'s resident-DB publishes and `spt_locales_set` |
| `SPT_Data/configs/` | Batch read pre-DI | `Loaders/ConfigLoader`, a static class — it runs before the container exists |
| `user/profiles/` | Async read/write | `Servers/SaveServer`; `Routers/SaveLoad/` and `Migration/` patch old data on load |
| Mod assemblies | Reflective + HarmonyX | `[Injectable]` replacement via `IOnDIConstruct`; a live patch on a frozen 4.1.2 member flips that family to its legacy path |

---

# Relationship to Other Framework Components

| Component | Responsibility |
|-----------|-----------------|
| [root `ARCHITECTURE.md`](../../ARCHITECTURE.md) | Repo-spanning behaviour: Rust FFI contract, dual-path dispatch, build pipeline, mods |
| [`Libraries/ARCHITECTURE.md`](../ARCHITECTURE.md) | Which library project owns what, and what each depends on |
| [`SPTarkov.Server/ARCHITECTURE.md`](../../SPTarkov.Server/ARCHITECTURE.md) | The host: startup order, `RegisterSptServicesAsync`, the catch-all middleware |
| [`rust/ARCHITECTURE.md`](../../rust/ARCHITECTURE.md) | The crate behind `Native/`: modules, FFI boundary, porting conventions |
| [`RUST-ROADMAP.md`](../../RUST-ROADMAP.md) | § *Exceptions in force* — the full dispatch conditions per dual-path family |
| [`CLAUDE.md`](../../CLAUDE.md) | Repo-wide style rules and build commands |
