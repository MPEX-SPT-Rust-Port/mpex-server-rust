# SPTarkov.Server.Core — Architecture

All game logic. 854 tracked `.cs` files, ~91% of the code under `Libraries/`. Every path below is
relative to `Libraries/SPTarkov.Server.Core/`.

A map of Core: what each folder is for and where to add things. For repo-spanning behaviour (Rust
FFI contract, dual-path dispatch, build pipeline, mods) see the [root
ARCHITECTURE.md](../../ARCHITECTURE.md); for which library project owns what, see
[`Libraries/ARCHITECTURE.md`](../ARCHITECTURE.md).

Core references `SPTarkov.Common` + `SPTarkov.DI`, four NuGet packages (`HarmonyX` for live-patch
detection, `FastCloner`, `MessagePack`, `System.IO.Hashing`), and `Spectre.Console.Ansi` — a
Rust-emitted facade kept only to hold the mod ABI (four files still name Spectre types). Core's
csproj invokes `cargo`, so any `dotnet build` needs the Rust toolchain on `PATH`.

## Folder map

| Folder | Files | What it is |
|---|---:|---|
| `Models/` | 437 | Data records. `Eft/` client wire contracts, `Spt/` server-internal, `Enums/`, `Common/` primitives |
| `Helpers/` | 68 | Stateless computation (29 of them the `Dialogue/` chat-bot framework) |
| `Services/` | 55 | Stateful, long-lived, cache-owning |
| `Routers/` | 50 | URL → callback dispatch (`Static/` 23, `ItemEvents/` 11, `Dynamic/` 7, `SaveLoad/` 4, `Serializers/` 2, root 3) |
| `Utils/` | 43 | JSON layer (23), RNG, cloning, collections, IO, importers. Plus gitignored `ProgramStatics.Generated.cs` |
| `Callbacks/` | 34 | HTTP entry point per domain |
| `Generators/` | 34 | Build game data from scratch |
| `Controllers/` | 30 | Orchestration |
| `Extensions/` | 23 | Domain extension methods, one file per extended type (`ProfileExtensions` extends `PmcData`, `FullProfileExtensions` extends `SptProfile`) |
| `Migration/` | 21 | Versioned profile migrations (`3.11`, `4.0`, `4.1`) plus unversioned `Migrations/Fixes/` (7) |
| `Native/` | 20 | C# side of the Rust FFI |
| `Exceptions/` | 13 | Typed exceptions (`Helpers/` 7, `Items/` 3, `Database/` 3) |
| `Servers/` | 11 | HTTP, WebSocket, save, ragfair |
| `DI/` | 8 | Router base classes + lifecycle interfaces |
| `Constants/` | 5 | Id and slot-name constants (`BodyPartContants` typo is in the source) |
| `Loaders/` | 2 | `ConfigLoader`, `BundleLoader` |

## Request pipeline

No MVC, no attribute routing. **An endpoint = a router entry + a callback method + (usually) a
controller method.**

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
  never fires them and the `ItemRouterOnBefore/AfterEventRequestData` records are unused.
- Responses go through `Utils/HttpResponseUtil`, which wraps them in the client envelope
  (`data`/`err`/`errmsg`) — the wrong helper (`GetBody`, `GetUnclearedBody`, `NullResponse`,
  `EmptyResponse`, `EmptyArrayResponse`) is silently rejected by the game. `NoBody` serializes raw,
  no envelope; `AppendErrorToOutput` adds a warning to an item-event response.
- The controller layer is optional — `BtrDelivery`, `Bundle`, `ItemEvent` and `Save` callbacks skip it.
- Name traps: `BuildsCallbacks`/`BuildController`, `Handbook`/`HandBook`, `Inraid`/`InRaid`, and
  `Routers/Static/ModLoaderRouter.cs` is the one file there not named `*StaticRouter`.

### Item events

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

## Lifecycle and DI

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

## Configuration

`Loaders/ConfigLoader` is a **static class, not `[Injectable]`** — it runs before the container
exists. It maps each file in `SPT_Data/configs/` to a `BaseConfig` subclass in `Models/Spt/Config/`;
the host registers each as its own singleton, so services inject e.g. `BotConfig` directly.

Seven `ForceLegacy*` flags are the Rust-port escape hatches, one per dual-path family:
`LocationConfig.ForceLegacyLootGeneration`, `BotConfig.ForceLegacyBotGeneration`,
`RagfairConfig.ForceLegacyRagfairGeneration`, `RagfairConfig.ForceLegacyRagfairLinkedItemBuild`,
`QuestConfig.ForceLegacyRepeatableQuestGeneration`, `ScavCaseConfig.ForceLegacyScavCaseGeneration`
and `ItemConfig.ForceLegacyItemBaseClassHydration`. Narrower knobs:
`BotConfig.ForcePerBotGeneration` (unbatch waves without leaving native) and, on `RagfairConfig` and
`QuestConfig`, `TrustNativeRequestCacheWithMods` / `DisableNativeRequestCache` (on ragfair these now
gate resident-DB eligibility rather than a request cache).

## JSON layer (`Utils/Json/`, 23 files)

System.Text.Json with a custom converter set for the client's loose typing: union types
(`ListOrT<T>`, `StringOrInt`, `DictionaryOrList`, `FloatOrIrregularFloatArray`), 16 `Converters/`
(enums, `MongoId`, number coercion), and `LazyLoad<T>`, which defers parsing the huge database files
and whose transformer hook is the supported way to modify loose loot.

Only 9 of the 16 converters are registered globally; the rest are opt-in per property. **Gotcha:**
that global list exists twice — in `SptJsonConverterRegistrator` (runtime, via DI) and hardcoded in
`ConfigLoader` (pre-DI), as two identical 9-entry arrays. A converter configs need goes in both.

## Logic layers

Convention: **Services** hold state, **Helpers** don't, **Generators** create data.

### `Services/` (55)

| Subfolder | Files | Owns |
|---|---:|---|
| `InRaid/` | 9 | `LocationLifecycleService` (raid start/end), airdrops, BTR, custom waves, goon spawns, match location, open zones, raid time, weather |
| `Bot/` | 8 | Equipment filters, mod pools, inventory containers, loot cache, name pool, weapon mod limits, per-match cache, PMC chat responses |
| `Commerce/` | 7 | Fence, gifts, insurance, `MailSendService`, payment, repair, trader purchase persistence |
| `Ragfair/` | 6 | Offers, prices, categories, `RagfairLinkedItemService`, required items, tax |
| `Profile/` | 5 | Backup, creation, activity, `ProfileFixerService`, `ProfileMigrationService` |
| `Modding/` | 5 | Custom item/quest registration (`Custom/`), in-memory cache, mod item cache, profile data access |
| `Server/` | 5 | `PostDbLoadService`, `SeasonalEventService`, notifications, bundle hashes, `DatabaseMutationStamp` |
| `Locales/` | 3 | `AbstractLocalisationService` + `ServerLocalisationService` (server messages), `LocaleService` (client locales) |
| `Items/`, `Hideout/`, `Hosted/`, `Image/` | 7 | Item blacklists + `ItemBaseClassService`, cultist circle + map markers, startup hosted service + system-info logger, image routing |

`ServerLocalisationService.GetText(key, args)` produces every user-visible server string. Its
resolved table is flattened and pushed to Rust at database import (`spt_locales_set`), which renders
its own generator diagnostics against that startup snapshot.

### `Helpers/` (68)

Grouped by domain: `Profile/` (8, incl. `HideoutHelper`, `InventoryHelper`, `ProfileHelper`),
`Ragfair/` (6), `Bot/` (5), `Server/` (5), `Commerce/`, `InRaid/`, `Quest/`, `Traders/` (3 each),
`Items/` (`ItemHelper`, `PresetHelper`), plus `WeightedRandomHelper`.

`Helpers/Dialogue/` (29) is not helpers — it's the in-game chat-bot framework:
`AbstractDialogChatBot` → `SptDialogueChatBot` / `CommandoDialogChatBot`, with `Commando/SptCommands/`
and 15 `IChatMessageHandler` implementations under `SPTFriend/Commands/`.

### `Generators/` (34)

| Subfolder | Contents |
|---|---|
| `Bot/` | `BotGenerator`, `BotInventoryGenerator`, `BotEquipmentModGenerator`, `BotWeaponGenerator`, `BotLevelGenerator`, `PlayerScavGenerator`, `BotWaveBatcher` |
| `Loot/` | `LocationLootGenerator`, `LootGenerator`, `BotLootGenerator`, `PMCLootGenerator` |
| `RepeatableQuests/` | `IRepeatableQuestGenerator` + completion / elimination / exploration / pickup generators, plus `RepeatableQuestRewardGenerator` |
| `Weapons/` | `IInventoryMagGen`, the `InventoryMagGen` dispatcher and four implementations: barrel, external, internal magazine, UBGL |
| `Weather/`, `Ragfair/`, root | `WeatherGenerator` + interface/abstract + three presets; ragfair offers/assorts; fence assorts, PMC waves, scav case rewards |

### Dual-path (Rust) sites

**Eleven** classes forward to Rust by default and keep their 4.1.2 implementation as a legacy
fallback — nine generators (`LocationLootGenerator`, `LootGenerator`, `BotInventoryGenerator`,
`RagfairOfferGenerator`, `ScavCaseRewardGenerator` and the four `RepeatableQuests/` quest-type
generators) and two services (`ItemBaseClassService`, `RagfairLinkedItemService`). Each holds a
frozen list of 4.1.2 members and uses HarmonyX to detect a live patch before dispatching, as does
`BotWaveBatcher` — twelve Core files reference HarmonyX (the eleven, plus `BotWaveBatcher`).

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

## Models (437 files, over half of Core)

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

## Infrastructure

| Folder | Contents |
|---|---|
| `Servers/` | `HttpServer`, `WebSocketServer`, `SaveServer` (owns `user/profiles/`), `RagfairServer`; `Http/` listener + `RequestLogger`; `Ws/` connection and message handlers |
| `Native/` | `NativeMethods` (`[LibraryImport]`), `SptNative` (safe wrapper), and payload/projection/request-builder types under `BaseClass/`, `Bot/`, `Db/` (also `DbPublisher`, the resident-DB publish site), `Loot/`, `Ragfair/`, `RepeatableQuests/`, `ScavCase/`. Contract details in the root ARCHITECTURE.md |
| `Migration/` | `IProfileMigration` / `AbstractProfileMigration` / context, versioned sets under `Migrations/3.11`, `4.0`, `4.1`, plus unversioned `Migrations/Fixes/` (7 corruption repairs) |
| `Exceptions/` | `Helpers/` (7, one per helper that throws), `Items/` (3, modded item/trader/clothing validation), `Database/` (3) |

The repeatable-quest request skips its call-invariant half when `DatabaseMutationStamp` has not
moved and no mods are loaded; the ragfair request instead rides the native resident DB, republished
by `Native/Db/DbPublisher` when the stamp moves.

`WebSocketServer` matches `IWebSocketConnectionHandler.GetHookUrl()` as a substring of the path and
notifies *every* matching handler — unlike `IHttpListener`, where only the first match runs.

### `Utils/` (43)

- `RandomUtil` + `RandomSource` — `IRandomSource` is the test-only seam letting C# and Rust share a
  seeded xoshiro256\*\* for parity tests. Production randomness is unchanged.
- `Collections/` — `ProbabilityObjectArray` (weighted draws, has a Rust twin), `ExhaustableArray`.
- `Cloners/` — `ICloner` + `FastCloner`; item-event responses are cloned before return.
- `ImporterUtil` / `ImageRouteImporter` — the database and image tree imports the host's
  `DatabaseImporter` drives.
- `ProgramStatics` is `static partial`; its other half `Utils/ProgramStatics.Generated.cs` is written
  at build time from MSBuild properties. **Never edit the generated file.**
- Also `FileUtil`, `HashUtil`, `HttpFileUtil`, `HttpResponseUtil`, `JsonUtil`, `MathUtil`,
  `QteRandomUtil`, `TimeUtil`, `Watermark`, `RagfairOfferHolder`.

## Conventions

Repo-wide style rules are in [CLAUDE.md](../../CLAUDE.md). Core-specific:

- Mark a class `[Injectable]` and register it in `ProgramHelpers.RegisterSptServicesAsync`.
  `DependencyInjectionValidationTests` rebuilds that container with mods on and off, so a bad
  registration fails the test run rather than a launch.
- An endpoint is a router entry + a callback + (usually) a controller. Never an `[HttpGet]`.
- Item-moving actions go in `Routers/ItemEvents/`; profile-load fixes in `Routers/SaveLoad/` if
  unconditional, `Migration/` if versioned.
