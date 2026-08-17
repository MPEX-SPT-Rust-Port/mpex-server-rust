# Follow-ups

Two items left open by the Completion quest performance investigation (`49730f2`). Both are
independent of that fix and of each other.

| | Item | Kind | Evidence | Status |
|---|---|---|---|---|
| 1 | Port `ItemBaseClassService`'s ancestor cache | performance | measured | **done** — `dc4e409`…`8963a41` |
| 2 | `SptNative.QuestJsonOptions` publishes its memo unsafely | correctness | read, not reproduced | not started |

---

## 1. Port `ItemBaseClassService`'s ancestor cache — done

**Resolved in `dc4e409`…`8963a41`. Completion warm: 12.80 → 5.02 / 4.75 ms**, against the other
three quest types' ~2.4 ms in the same session. Numbers and methodology in
[BENCHMARK.md](BENCHMARK.md), *What still costs*.

The cache was built once per cached invariant slice (quest lazily behind a `OnceLock`, ragfair
eagerly in `PreparedSlice::from`) and all 19 direct call sites in `quest/` and `ragfair/` answer
from it. `bot/` and `loot/` keep the walk — they get their views per request, with nothing to
amortise a build against.

**The expectation below that a cache would "subsume both effects" was wrong**, and the correction is
worth keeping. The two effects are independent: the cache removes the parent-chain *walk*, roughly
four `IndexMap` probes per item, and that was never the dominant term. The 6x was the linear scan of
the candidate list at each link, which caching does not touch — with Completion's 137-entry
whitelist it is `chain_len × 137` string comparisons per item. The flattened cache alone
(`52a27e0`) left Completion warm at 11.74 / 13.50 ms, i.e. unmoved. `8963a41` added
`ItemBaseClassCache::is_of_baseclasses_set` and used it at the two Completion whitelist/blacklist
sites, where the candidate list is already a `HashSet`; that is what produced the drop. Every other
caller passes one to seven ids, where the slice scan is the cheaper form and is kept.

Bot and ragfair were re-measured and neither moved, as anticipated below.

The original write-up follows, unedited apart from this heading.

### What

`loot/item_helper.rs:122` — `is_of_baseclasses` walks a template's parent chain live on every call,
with an `IndexMap` lookup per link and a linear `slice::contains` at each link. C#
(`Services/Items/ItemBaseClassService.cs`) answers the same question from
`Dictionary<MongoId, HashSet<MongoId>>`, built once at startup, flattening each template's whole
ancestor chain into one set. Its lookups are a single hash probe (`ItemHasBaseClass` → `Contains`)
or a set intersection (`Overlaps`), never a walk.

The port is answer-faithful — I traced `AddBaseItems`'s recursion against the walk and they agree —
but it traded an O(1) lookup for an O(depth) one. `item_helper.rs:110-121` documents the swap and
reasons about it entirely in terms of answer-equivalence, never cost.

### Why it is worth doing

`49730f2` fixed the worst *consumer* of the uncached walk (Completion's whitelist filter restarting
a walk per candidate). It did not fix the walk itself. What remains, measured:

- **Completion warm sits at ~13 ms against the other three quest types' ~3.3 ms.** The ~10 ms gap is
  `get_items_to_retrieve_pool` → `reward_generator::is_valid_reward_item` over all 4,673 templates,
  each doing at least two `is_of_baseclasses` calls.
- Measured on the real table, for the whitelist filter alone: **9.85 ms** for one walk per item
  against **1.65 ms** when the candidates sit in a `HashSet` instead of a slice — the inner linear
  scan is 6x on its own, and a real ancestor cache subsumes both effects.

### Scope

**20 direct call sites** outside `item_helper`, plus **7 more** behind `item_helper`'s own wrappers
(`armor_item_can_hold_mods`, `is_valid_item`, …), whose callers are where the volume actually comes
from:

| Module | Direct call sites | Files |
|---|---|---|
| `bot/` | 8 | `bot_equipment_mod_generator.rs` (6), `mod_pool_service.rs`, `bot_generator_helper.rs` |
| `quest/` | 6 | `completion.rs` (3), `reward_generator.rs` (3) |
| `ragfair/` | 4 | `offer_generator.rs` (3), `server_helper.rs` |
| `loot/` | 2 | `location_loot_generator.rs`, `loot_generator.rs` |

> An earlier revision of `BENCHMARK.md` and `RUST-ROADMAP.md` said "58 call sites". That number came
> from a bare-word grep that also matched comments and test names, and has been corrected in both.

### Design sketch

Build the flattened ancestor map once and hang it off the parsed invariant slice, not off the
request. The native side already caches parsed slices (`quest/slice_cache.rs`), so a map built
alongside one amortises to zero across every warm call that reuses it — which is the common case on
a stock server.

Cost to build: one pass over 4,673 templates × chain depth, roughly 19k inserts. Expect low
single-digit milliseconds, paid once per slice rather than per call.

### Parity notes before starting

- **The `_rootNodeIds` divergence already exists and does not have to be resolved here.** C# returns
  `false` for any template whose `_type` is not `"Item"` (120 of 4,673 are `Node`); `ItemView`
  carries no `_type`, so Rust walks them anyway. `item_helper.rs:118-121` argues nothing asks about
  node tpls. Porting the cache does not require changing that, and changing it *would* require
  adding `_type` to the payload projection — a wire change with ABI consequences. Keep the current
  semantics unless a caller is found that actually needs the distinction.
- **No lazy-fill path is needed.** C# adds on cache miss (`ItemHasBaseClass:107-110`) to cope with
  mod-added items. The Rust map would be built from the slice's items view, which is projected from
  the live table and therefore already contains mod-added templates.

### How to verify

- `cargo test` (643) and `dotnet test` (595) must stay green — the parity suites are the contract.
- `quest::completion::tests::the_whitelist_filter_walks_each_item_chain_once` and
  `tests/completion_whitelist_baseclass.rs` both stay valid; the ratio they assert should collapse
  toward 1x from either direction.
- Re-run `dotnet test -c Release --filter "FullyQualifiedName~RepeatableQuestBenchmarkTests"` twice.
  Success is Completion warm falling from ~13 ms toward the ~3.3 ms the other three types cost.
- **Also re-run `BotBenchmarkTests` and `RagfairBenchmarkTests`, but do not expect much there.** The
  bot path is ~92% payload transport, so a generation-side win is mostly invisible in its totals.
  Measure it rather than assuming either way.

---

## 2. `SptNative.QuestJsonOptions` publishes its memo unsafely

### What

`Libraries/SPTarkov.Server.Core/Native/SptNative.cs:459-479`:

```csharp
var source = LootJsonOptions;
if (!ReferenceEquals(_questJsonOptionsSource, source))
{
    var options = new JsonSerializerOptions(source);
    options.Converters.Insert(0, new JsonStringEnumConverter<ELocationName>());
    _questJsonOptions = options;
    _questJsonOptionsSource = source;
}

return _questJsonOptions!;
```

Two plain static fields, no lock, no barrier, published as a logically-paired unit. The XML doc
above it claims:

> "the derived field is written before the source it is keyed on, so a reader that sees the new
> source sees the options built from it"

That argument covers only the *writer's* store order. Two hazards it does not cover:

**a. Interleaved writers can mismatch the pair.** Thread A enters the branch and writes
`_questJsonOptions = optionsA`. Before it writes `_questJsonOptionsSource`, thread B — running after
a further container rebuild, so holding a newer `sourceB` — enters, writes `optionsB`, then writes
`sourceB`. A resumes and writes `_questJsonOptionsSource = sourceA`. The cache now holds `optionsB`
keyed by `sourceA`, and every later reader arriving with `sourceA` gets options built from the wrong
container's converters — precisely the failure the re-keying was introduced to prevent. This needs
no memory-model argument at all; plain interleaving is enough.

**b. The reader performs two independent loads.** `_questJsonOptionsSource` and `_questJsonOptions`
are read separately with nothing ordering them. A reader that observes the fresh source alongside a
stale null derived field takes the early-out and evaluates `return _questJsonOptions!` — a null
return through a null-forgiving operator, so it surfaces as an `NullReferenceException` at the call
site rather than here.

### Likelihood

Low in production and not reproduced. `JsonUtil` is `[Injectable(InjectionType.Singleton)]`, so a
running server builds one container and takes the branch effectively once. The exposure is where
containers are rebuilt repeatedly and concurrently — `DependencyInjectionValidationTests` rebuilds
the exact container twice (mods on and off), and any future parallel fixture doing the same widens
it. Treat this as a latent defect worth closing cheaply, not an incident.

### Suggested fix

Hold both values behind a single reference so publication is one atomic write and reads cannot tear:

```csharp
private sealed record QuestOptions(JsonSerializerOptions Source, JsonSerializerOptions Derived);

private static QuestOptions? _questJsonOptions;

internal static JsonSerializerOptions QuestJsonOptions
{
    get
    {
        var source = LootJsonOptions;
        var cached = _questJsonOptions;

        if (cached is null || !ReferenceEquals(cached.Source, source))
        {
            var options = new JsonSerializerOptions(source);
            // Appending would leave the global enum factory ahead of it: first match wins
            options.Converters.Insert(0, new JsonStringEnumConverter<ELocationName>());
            cached = new QuestOptions(source, options);
            _questJsonOptions = cached;
        }

        return cached.Derived;
    }
}
```

Reading into the `cached` local also removes hazard (b): the value returned is the one whose source
was checked, and it can never be null. Losing a race still just drops the loser's copy, which was
already the intended behaviour. No lock, and the existing comment about converter ordering survives
unchanged.

Worth confirming a positional `record` suits the codebase before using one — `CLAUDE.md` mandates
block bodies for members, and a private sealed class with two properties is the conservative
alternative.

### How to verify

`dotnet test` (595) covers the behaviour; the race itself will not show up there. The value of the
change is structural — it removes the reasoning burden rather than fixing an observed failure, so
resist adding a threading test that cannot reliably fail.
