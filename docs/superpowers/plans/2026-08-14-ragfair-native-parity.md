# Ragfair Native Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. Dispatch implementation subagents on **Opus 5**
> (`model: "opus"`). Every subagent prompt that explores code MUST include: "run
> `graphify query \"<question>\"` before reading source files — the repo hooks enforce it."

**Goal:** Close the ragfair native full-pass gap (1550 ms native vs 517 ms legacy) to ≤ 1.25×
legacy, via a parallel batch walk, a framed FFI response with parallel direct deserialization,
and a MessagePack payload encoding.

**Architecture:** Three verified stages. Stage A parallelizes the Rust assort walk with rayon,
sequential-when-seeded so all parity guarantees hold. Stage B replaces the monolithic JSON
response with a length-prefixed per-offer framed envelope (encoding-tagged) that C# deserializes
in parallel straight into `RagfairOffer`, deleting the wire→DTO hop (ABI 8→9). Stage C flips the
frame payload encoding from JSON to MessagePack with hand-written C# formatters (ABI 9→10). Each
stage ends with the full test suite and a recorded benchmark.

**Tech Stack:** Rust 1.97.1 (rayon 1.12.0 already in-crate; rmp-serde new), .NET 10 / C# (STJ;
MessagePack-CSharp new), NUnit.

**Spec:** `docs/superpowers/specs/2026-08-14-ragfair-native-parity-design.md` — read it first;
the spec's §5 "Unchanged surfaces" list is binding.

## Global Constraints

- `rmp-serde` is pinned **exactly**: `rmp-serde = "=1.3.1"` (user requirement).
- Rust: no new dependencies beyond `rmp-serde`. `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings` must pass at every commit.
- C#: one new NuGet package total (`MessagePack`, stage C only). `csharpier format .` before the final commit.
- ABI lockstep: `rust/spt-native/src/lib.rs` `ABI_VERSION` and `SptNative.ExpectedAbiVersion` (`Libraries/SPTarkov.Server.Core/Native/SptNative.cs:50`) change **in the same commit**, as do the asserts in `ffi.rs`'s `abi_version_export_matches_crate_const`. Stage B → 9, stage C → 10.
- Seeded runs (`request.test_seed.is_some()`) must stay on the sequential walk, byte-for-byte identical to today. `RagfairParityTests` must stay 19/19 green at every commit — never adjust its expectations.
- Spec §5 surfaces are untouched: legacy path, Harmony dispatch, `ForceLegacyRagfairGeneration`, `AddOffer`/holder/`OfferCounter`, one-timestamp-per-batch, request projection and request JSON encoding.
- C# style (CLAUDE.md): always brace bodies, no expression-bodied members, file-scoped namespaces, `_camelCase` private fields.
- Benchmarks only in Release (`dotnet test -c Release --filter "FullyQualifiedName~RagfairBenchmarkTests" --logger "console;verbosity=detailed"`); record native and legacy medians from the same run into `todo/RAGFAIR_PARITY_RESULTS.md`.
- `dotnet build` requires `cargo` on PATH and `scripts/decompress-assets.sh` run once beforehand.
- Commit prefixes follow repo convention: `feat:` / `test:` / `docs:` / `perf:`.

---

### Task 1: Stage A — parallel batch walk in `generate_dynamic_offers`

**Files:**
- Modify: `rust/spt-native/src/ragfair/mod.rs` (add `RagfairContext::fork`)
- Modify: `rust/spt-native/src/ragfair/offer_generator.rs:472-492` (the walk)
- Test: `rust/spt-native/src/ffi.rs` (tests module)

**Interfaces:**
- Consumes: `create_offers_from_assort(&mut RagfairContext, &mut Vec<Item>, bool, &mut Vec<RagfairOfferWire>, &mut IndexSet<String>, &mut i32) -> Result<(), LootError>` (unchanged).
- Produces: `generate_dynamic_offers` with identical signature and identical `DynamicOffersResult`; offers ordered by assort entry, `internal_id` sequential from `offer_counter_start`. `RagfairContext::fork(&self) -> RagfairContext<'a>`.

- [ ] **Step 1: Write the pin test** (in `ffi.rs`'s tests module, next to the existing ragfair request helpers at `ffi.rs:665-771`). This is a behavior pin, not red-green: it passes on the sequential path today and must keep passing once the walk is parallel. The expired-offers route is used because it deterministically produces exactly one offer per entry with the entry's root id (roots keep their `_id` through `reparent_item_and_children`), and an unseeded request takes the parallel path.

```rust
    /// Thirty expired single-item entries, unseeded — the parallel walk. One offer per expired
    /// entry, `intId` sequential from `offerCounterStart`, offers in assort order: the merge
    /// contract stage A must preserve.
    #[test]
    fn an_unseeded_expired_pass_keeps_assort_order_and_sequential_int_ids() {
        let expired: Vec<String> = (0..30)
            .map(|i| format!(r#"[{{"_id":"{i:024x}","_tpl":"{SELLABLE_TPL}"}}]"#))
            .collect();
        let request = ragfair_request_with(
            &format!(
                r#"{{"{SELLABLE_TPL}":{{"parent":"cccccccccccccccccccccccc","type":"Item",
                "stackMaxSize":1,"canSellOnRagfair":true}}}}"#
            ),
            r#"{"default":{"min":2,"max":5}}"#,
        )
        // splice the expired entries and a non-zero counter start into the request JSON
        .replacen(
            r#"{"timestamp":"#,
            &format!(r#"{{"expiredOffers":[{}],"timestamp":"#, expired.join(",")),
            1,
        )
        .replacen(r#""offerCounterStart":0"#, r#""offerCounterStart":7"#, 1);

        let (status, out) = call_generate(spt_generate_dynamic_offers, request.as_bytes());

        assert_eq!(status, STATUS_OK);
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let offers = result["offers"].as_array().unwrap();
        assert_eq!(offers.len(), 30);
        for (i, offer) in offers.iter().enumerate() {
            assert_eq!(offer["intId"], serde_json::json!(7 + i as i64));
            assert_eq!(offer["root"], serde_json::json!(format!("{i:024x}")));
        }
    }
```

Note: `ragfair_request_with` builds `{"timestamp":...}` as its first key and `"offerCounterStart":0` — the two `replacen` splices rely on that; if the helper changed, adjust the anchors. `testSeed` is absent from the helper's output, so the request is unseeded.

- [ ] **Step 2: Run it — must pass already (pin), and note it exercises today's sequential code**

Run: `cd rust && cargo test an_unseeded_expired_pass -- --nocapture`
Expected: PASS

- [ ] **Step 3: Add `RagfairContext::fork`** to `rust/spt-native/src/ragfair/mod.rs`, below the struct:

```rust
impl<'a> RagfairContext<'a> {
    /// A worker's view of the same pass: every shared reference copied, a fresh diagnostics
    /// buffer of its own — what lets the batch walk fan out without sharing `&mut self`.
    pub fn fork(&self) -> RagfairContext<'a> {
        RagfairContext {
            items: self.items,
            dynamic: self.dynamic,
            item_presets: self.item_presets,
            default_presets: self.default_presets,
            default_presets_by_tpl: self.default_presets_by_tpl,
            presets_by_tpl: self.presets_by_tpl,
            flea_prices: self.flea_prices,
            handbook_prices: self.handbook_prices,
            highest_trader_prices: self.highest_trader_prices,
            config_blacklist: self.config_blacklist,
            seasonal_item_tpl_blacklist: self.seasonal_item_tpl_blacklist,
            pmc_names_usec: self.pmc_names_usec,
            pmc_names_bear: self.pmc_names_bear,
            timestamp: self.timestamp,
            seasonal_event_active: self.seasonal_event_active,
            diagnostics: Vec::new(),
        }
    }
}
```

- [ ] **Step 4: Parallelize the walk.** In `offer_generator.rs`, add `use rayon::prelude::*;` to the imports, and replace the sequential loop at `:472-485` (keep the surrounding stopwatch/diagnostic lines):

```rust
    let stopwatch = Instant::now();
    let mut offers = Vec::new();
    let mut rejected = IndexSet::new();
    if _seed_guard.is_some() {
        // A seeded run stays sequential: the seeded RNG is thread-local, so fanning out would
        // silently drop every worker onto entropy. Parity rides this path byte-for-byte.
        let mut offer_counter = offer_counter_start;
        for assort_item_with_children in &mut assort_items_to_process {
            create_offers_from_assort(
                &mut ctx,
                assort_item_with_children,
                replacing_expired_offers,
                &mut offers,
                &mut rejected,
                &mut offer_counter,
            )?;
        }
    } else {
        // Unseeded: one worker context per assort entry, merged in assort order. `intId` is
        // reassigned during the merge so the counter stays sequential regardless of which
        // worker finished first — legacy's own per-entry Task fan-out makes no ordering
        // promise, but the holder insert loop deserves a stable one.
        let worker_results = assort_items_to_process
            .par_iter_mut()
            .map(|assort_item_with_children| {
                let mut worker_ctx = ctx.fork();
                let mut worker_offers = Vec::new();
                let mut worker_rejected = IndexSet::new();
                let mut worker_counter = 0;
                create_offers_from_assort(
                    &mut worker_ctx,
                    assort_item_with_children,
                    replacing_expired_offers,
                    &mut worker_offers,
                    &mut worker_rejected,
                    &mut worker_counter,
                )?;
                Ok((worker_offers, worker_rejected, worker_ctx.diagnostics))
            })
            .collect::<Result<Vec<_>, LootError>>()?;

        let mut offer_counter = offer_counter_start;
        for (worker_offers, worker_rejected, worker_diagnostics) in worker_results {
            for mut offer in worker_offers {
                offer.internal_id = offer_counter;
                offer_counter += 1;
                offers.push(offer);
            }
            rejected.extend(worker_rejected);
            ctx.diagnostics.extend(worker_diagnostics);
        }
    }
```

Rename `_seed_guard` to `seed_guard` if clippy objects to reading an underscore-prefixed binding; keep the `#[must_use]` RAII alive for the whole function either way.

- [ ] **Step 5: Run the Rust gate**

Run: `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: all green, including `an_unseeded_expired_pass_keeps_assort_order_and_sequential_int_ids` — now exercising the parallel arm.

- [ ] **Step 6: Run the C# parity and wire fixtures** (the seeded path must be untouched)

Run: `dotnet test --filter "FullyQualifiedName~RagfairParityTests|FullyQualifiedName~SptNativeRagfairWireTests|FullyQualifiedName~RagfairPathDispatchTests|FullyQualifiedName~RagfairHookLivenessTests"`
Expected: all green (19/19 parity among them).

- [ ] **Step 7: Commit**

```bash
git add rust/spt-native/src/ragfair/mod.rs rust/spt-native/src/ragfair/offer_generator.rs rust/spt-native/src/ffi.rs
git commit -m "perf: fan the ragfair batch walk across rayon when unseeded"
```

---

### Task 2: Stage A checkpoint — full suite + benchmark

**Files:**
- Create: `todo/RAGFAIR_PARITY_RESULTS.md`

- [ ] **Step 1: Full test suites**

Run: `cd rust && cargo test` then `dotnet test`
Expected: everything green. If not, fix before benchmarking — a benchmark of a broken build is noise.

- [ ] **Step 2: Benchmark**

Run: `dotnet test -c Release --filter "FullyQualifiedName~RagfairBenchmarkTests" --logger "console;verbosity=detailed"`
Expected output shape: medians for `full pass native`, `full pass legacy`, `regeneration pass` both paths, plus the speedup line. Baseline for comparison (2026-08-14, same machine): native 1550 ms / legacy 517 ms full pass; native ~103 ms / legacy ~12.8 ms regen. Stage A should put the native full pass near **~1000 ms** (generation 710 → ~165 ms).

- [ ] **Step 3: Record.** Create `todo/RAGFAIR_PARITY_RESULTS.md` with a table: stage, date, full-pass native/legacy medians, regen native/legacy medians, ratio, alloc/run. One row per checkpoint; this file feeds the final BENCHMARK.md update and is deleted in Task 10.

- [ ] **Step 4: Commit**

```bash
git add todo/RAGFAIR_PARITY_RESULTS.md
git commit -m "test: record the stage A ragfair benchmark checkpoint"
```

---

### Task 3: Stage B — framed response envelope, parallel direct deserialize (ABI 9)

The single atomic cross-boundary change: Rust emits the framed envelope, C# reads it, both ABI
constants move to 9 in one commit. The tree is never green mid-task with only one side changed —
work through all steps, then run the gates.

**Files:**
- Modify: `rust/spt-native/src/ffi.rs` (framed writer + parameterized runner + tests)
- Modify: `rust/spt-native/src/ragfair/models.rs` (add `DynamicOffersHeader`)
- Modify: `rust/spt-native/src/lib.rs:8` (`ABI_VERSION` 8 → 9)
- Modify: `Libraries/SPTarkov.Server.Core/Native/SptNative.cs` (version const, framed reader)
- Modify: `Libraries/SPTarkov.Server.Core/Native/Ragfair/RagfairPayloads.cs` (delete wire DTOs, add header/result records)
- Modify: `Libraries/SPTarkov.Server.Core/Generators/Ragfair/RagfairOfferGenerator.cs:414-417` (drop `ToRagfairOffer`)
- Test: `Testing/UnitTests/Tests/Generators/SptNativeRagfairWireTests.cs`

**Interfaces:**
- Envelope, little-endian, in the `STATUS_OK` buffer:
  - `u8 payloadEncoding` — `0` = JSON (this stage), `1` = MessagePack (stage C)
  - `u32 headerLen`, then `headerLen` bytes: `{"rejectedCanSellTemplates":[…],"diagnostics":[…]}` encoded per the tag
  - `u32 offerCount`, then per offer: `u32 payloadLen` + payload bytes (one offer, encoded per the tag)
  - Error statuses (`BAD_ARGS`/`ERROR`/`PANIC`) still write a plain UTF-8 message, unframed.
- Produces (Rust): `write_framed_offers(DynamicOffersResult) -> Vec<u8>`, `run_generator_with(…, encode: fn(Response) -> Vec<u8>)`.
- Produces (C#): `SptNative.GenerateDynamicOffers(GenerateDynamicOffersRequest) -> FramedOffersResult` and `SptNative.GenerateDynamicOffersFramed(ReadOnlySpan<byte>) -> FramedOffersResult`; `internal record FramedOffersResult { List<RagfairOffer> Offers; List<MongoId> RejectedCanSellTemplates; List<Diagnostic> Diagnostics; }`; `internal record DynamicOffersHeader { List<MongoId> RejectedCanSellTemplates; List<Diagnostic> Diagnostics; }`. Each deserialized offer has `CreatedBy = OfferCreator.FakePlayer` set by the reader.
- Deletes: `RagfairOfferWire`, `RagfairOfferUserWire`, `OfferRequirementWire`, `RagfairOfferWireExtensions` (C# only — the Rust wire structs stay, they are the serializer), `LootExport.DynamicOffers` and its `Generate<T>` switch arm, the old C# `DynamicOffersResult`.

- [ ] **Step 1: Rust — add the header struct** to `ragfair/models.rs` next to `DynamicOffersResult`:

```rust
/// The non-offer sections of a framed response — everything except the offer frames.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicOffersHeader {
    pub rejected_can_sell_templates: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}
```

- [ ] **Step 2: Rust — parameterize the runner and add the framed writer** in `ffi.rs`. Add `use rayon::prelude::*;` and `use crate::ragfair::models::{DynamicOffersHeader, DynamicOffersResult};`. Change `run_generator` to delegate:

```rust
/// The payload encoding tags of the framed ragfair envelope.
pub const PAYLOAD_JSON: u8 = 0;

unsafe fn run_generator<Request, Response>(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    generate: fn(Request) -> Result<Response, LootError>,
) -> i32
where
    Request: DeserializeOwned,
    Response: Serialize,
{
    unsafe {
        run_generator_with(req_ptr, req_len, out_ptr, out_len, generate, |response| {
            serde_json::to_vec(&response).expect("result serialization cannot fail")
        })
    }
}

/// `run_generator` with the response encoding open: the ragfair export frames its response
/// instead of emitting one JSON document.
///
/// # Safety
/// As documented on the exports.
unsafe fn run_generator_with<Request, Response>(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    generate: fn(Request) -> Result<Response, LootError>,
    encode: fn(Response) -> Vec<u8>,
) -> i32
where
    Request: DeserializeOwned,
{
    // body is the existing run_generator body, with the serialize expression replaced by
    // `generate(request).map(encode)` inside the catch_unwind
    ...
}

/// The framed ragfair response: encoding tag, length-prefixed header, then one length-prefixed
/// payload per offer, serialized across rayon. Stage C changes only the payloads' encoding.
fn write_framed_offers(result: DynamicOffersResult) -> Vec<u8> {
    let header = serde_json::to_vec(&DynamicOffersHeader {
        rejected_can_sell_templates: result.rejected_can_sell_templates,
        diagnostics: result.diagnostics,
    })
    .expect("header serialization cannot fail");
    let payloads: Vec<Vec<u8>> = result
        .offers
        .par_iter()
        .map(|offer| serde_json::to_vec(offer).expect("offer serialization cannot fail"))
        .collect();

    let body: usize = payloads.iter().map(|payload| 4 + payload.len()).sum();
    let mut out = Vec::with_capacity(1 + 4 + header.len() + 4 + body);
    out.push(PAYLOAD_JSON);
    out.extend_from_slice(&u32::try_from(header.len()).expect("header fits u32").to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&u32::try_from(payloads.len()).expect("count fits u32").to_le_bytes());
    for payload in &payloads {
        out.extend_from_slice(&u32::try_from(payload.len()).expect("offer fits u32").to_le_bytes());
        out.extend_from_slice(payload);
    }
    out
}
```

Rewire the export:

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spt_generate_dynamic_offers(
    req_ptr: *const u8,
    req_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        run_generator_with(
            req_ptr,
            req_len,
            out_ptr,
            out_len,
            generate_dynamic_offers,
            write_framed_offers,
        )
    }
}
```

- [ ] **Step 3: Rust — bump ABI and update tests.** `lib.rs:8` → `pub const ABI_VERSION: u32 = 9;`; in `ffi.rs` `abi_version_export_matches_crate_const` the literal `8` → `9`. Add a test-side frame parser in the tests module and update the three ragfair tests:

```rust
    /// Splits a framed ragfair response: (encoding, header, offer payloads).
    fn parse_framed(out: &[u8]) -> (u8, serde_json::Value, Vec<Vec<u8>>) {
        let encoding = out[0];
        let mut at = 1;
        let read_len = |buf: &[u8], at: usize| {
            u32::from_le_bytes(buf[at..at + 4].try_into().unwrap()) as usize
        };
        let header_len = read_len(out, at);
        at += 4;
        let header = serde_json::from_slice(&out[at..at + header_len]).unwrap();
        at += header_len;
        let count = read_len(out, at);
        at += 4;
        let mut payloads = Vec::with_capacity(count);
        for _ in 0..count {
            let len = read_len(out, at);
            at += 4;
            payloads.push(out[at..at + len].to_vec());
            at += len;
        }
        assert_eq!(at, out.len(), "trailing bytes after the last frame");
        (encoding, header, payloads)
    }
```

  - `a_minimal_dynamic_offers_request_returns_an_empty_offer_list`: parse the frames; assert `encoding == PAYLOAD_JSON`, `payloads.is_empty()`, `header["rejectedCanSellTemplates"] == json!([])`.
  - `an_unseeded_expired_pass_keeps_assort_order_and_sequential_int_ids` (Task 1): parse frames, deserialize each payload with `serde_json::from_slice::<serde_json::Value>`, keep the same 30 assertions.
  - `unparseable_dynamic_offers_request…` and `a_dynamic_offers_failure…`: unchanged — error buffers stay plain messages.

- [ ] **Step 4: Run the Rust gate**

Run: `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 5: C# — replace the wire types.** In `RagfairPayloads.cs`: delete `RagfairOfferWire`, `RagfairOfferUserWire`, `OfferRequirementWire`, `RagfairOfferWireExtensions`, and the old `DynamicOffersResult`; add:

```csharp
/// <summary>
/// The header section of the framed <c>spt_generate_dynamic_offers</c> response — everything
/// except the offer frames, which deserialize straight into <see cref="RagfairOffer"/>.
/// </summary>
internal record DynamicOffersHeader
{
    [JsonPropertyName("rejectedCanSellTemplates")]
    public required List<MongoId> RejectedCanSellTemplates { get; set; }

    [JsonPropertyName("diagnostics")]
    public required List<Diagnostic> Diagnostics { get; set; }
}

/// <summary>
/// A parsed framed response: the header sections plus the materialized offers, each already
/// stamped <see cref="OfferCreator.FakePlayer"/> the way <c>CreateAndAddFleaOffer:72</c> does.
/// </summary>
internal record FramedOffersResult
{
    public required List<RagfairOffer> Offers { get; set; }

    public required List<MongoId> RejectedCanSellTemplates { get; set; }

    public required List<Diagnostic> Diagnostics { get; set; }
}
```

Keep the `using` set consistent (`SPTarkov.Server.Core.Models.Eft.Ragfair` for `RagfairOffer`); drop usings the deletions orphan.

- [ ] **Step 6: C# — the framed reader** in `SptNative.cs`. `ExpectedAbiVersion` → `9`. Delete `LootExport.DynamicOffers` and its switch arm. Replace the old wrapper:

```csharp
    /// <summary>
    /// Generates one full batch of dynamic flea offers. Unlike the other exports the response is
    /// a framed envelope — encoding tag, length-prefixed header, one length-prefixed payload per
    /// offer — deserialized in parallel straight into <see cref="RagfairOffer"/>.
    /// </summary>
    /// <exception cref="InvalidOperationException">Generation failed, or the native side misbehaved.</exception>
    internal static FramedOffersResult GenerateDynamicOffers(GenerateDynamicOffersRequest request)
    {
        return GenerateDynamicOffersFramed(JsonSerializer.SerializeToUtf8Bytes(request, LootJsonOptions));
    }

    /// <summary>
    /// The raw-bytes seam of <see cref="GenerateDynamicOffers"/>, kept internal so tests can hand
    /// it JSON no typed payload can express — a mod-added field, or a malformed request.
    /// </summary>
    internal static unsafe FramedOffersResult GenerateDynamicOffersFramed(ReadOnlySpan<byte> requestUtf8)
    {
        EnsureLoadable();

        byte* outPtr = null;
        nuint outLen = 0;
        int status;

        fixed (byte* requestPtr = requestUtf8)
        {
            status = NativeMethods.GenerateDynamicOffers(requestPtr, (nuint)requestUtf8.Length, &outPtr, &outLen);
        }

        try
        {
            if (status == StatusOk)
            {
                return ParseFramedOffers(outPtr, checked((int)outLen));
            }

            var message = outPtr == null ? "no message" : Encoding.UTF8.GetString(outPtr, checked((int)outLen));
            if (status == StatusError)
            {
                throw new InvalidOperationException($"spt_native DynamicOffers generation failed: {message}");
            }

            throw new InvalidOperationException(
                $"spt_native DynamicOffers generation failed with internal status {status}: {message}; this indicates a native library bug, not corrupt game data."
            );
        }
        finally
        {
            NativeMethods.BufFree(outPtr, outLen);
        }
    }

    private const byte PayloadJson = 0;

    private static unsafe FramedOffersResult ParseFramedOffers(byte* buffer, int length)
    {
        var span = new ReadOnlySpan<byte>(buffer, length);
        var encoding = span[0];
        var at = 1;

        var headerLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span[at..]));
        at += 4;
        var header =
            DeserializeHeader(encoding, span.Slice(at, headerLength))
            ?? throw new InvalidOperationException("spt_native returned an empty DynamicOffers header.");
        at += headerLength;

        var offerCount = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span[at..]));
        at += 4;
        var frames = new (int Offset, int Length)[offerCount];
        for (var i = 0; i < offerCount; i++)
        {
            var frameLength = checked((int)BinaryPrimitives.ReadUInt32LittleEndian(span[at..]));
            frames[i] = (at + 4, frameLength);
            at += 4 + frameLength;
        }

        if (at != length)
        {
            throw new InvalidOperationException($"spt_native DynamicOffers envelope has {length - at} trailing bytes.");
        }

        var offers = new RagfairOffer[offerCount];
        var basePointer = (nint)buffer;
        Parallel.For(
            0,
            offerCount,
            i =>
            {
                offers[i] = DeserializeOfferFrame(encoding, basePointer, frames[i].Offset, frames[i].Length);
            }
        );

        return new FramedOffersResult
        {
            Offers = [.. offers],
            RejectedCanSellTemplates = header.RejectedCanSellTemplates,
            Diagnostics = header.Diagnostics,
        };
    }

    private static DynamicOffersHeader? DeserializeHeader(byte encoding, ReadOnlySpan<byte> payload)
    {
        if (encoding != PayloadJson)
        {
            throw new InvalidOperationException($"unknown ragfair payload encoding {encoding}.");
        }

        return JsonSerializer.Deserialize<DynamicOffersHeader>(payload, LootJsonOptions);
    }

    private static unsafe RagfairOffer DeserializeOfferFrame(byte encoding, nint buffer, int offset, int length)
    {
        var frame = new ReadOnlySpan<byte>((byte*)buffer + offset, length);
        if (encoding != PayloadJson)
        {
            throw new InvalidOperationException($"unknown ragfair payload encoding {encoding}.");
        }

        var offer =
            JsonSerializer.Deserialize<RagfairOffer>(frame, LootJsonOptions)
            ?? throw new InvalidOperationException("spt_native returned an empty ragfair offer frame.");
        offer.CreatedBy = OfferCreator.FakePlayer;
        return offer;
    }
```

Add `using System.Buffers.Binary;`, `using SPTarkov.Server.Core.Models.Eft.Ragfair;`, `using SPTarkov.Server.Core.Models.Enums;` as needed. `RagfairOffer.User` is `required` — the wire always carries `user`, so STJ deserialization satisfies it; `Requirements` (`IEnumerable<OfferRequirement>?`) deserializes without a model change. **Do not modify `RagfairOffer`** unless the wire tests fail on it, in which case the sanctioned relaxation (spec §3) is: drop `required` from `User`. `MemberType`/`Side` come through `EftEnumConverter` (numbers), replacing the old explicit casts.

- [ ] **Step 7: C# — drop the DTO hop** in `RagfairOfferGenerator.cs:414-417`:

```csharp
        foreach (var offer in result.Offers)
        {
            ragfairOfferService.AddOffer(offer);
        }
```

- [ ] **Step 8: Update the wire tests** in `SptNativeRagfairWireTests.cs`:
  - `TheRequestRoundTripsThroughTheNativeSide`: unchanged assertions (they compile against `RagfairOffer` — `User.Nickname`, `SummaryCost`, `Id.IsEmpty` all exist); add ordering pins:

```csharp
        Assert.That(result.Offers.Select(offer => offer.InternalId), Is.EqualTo(Enumerable.Range(0, result.Offers.Count)));
        Assert.That(result.Offers[0].CreatedBy, Is.EqualTo(OfferCreator.FakePlayer));
```

  - `AModAddedConfigFieldSurvivesTheRoundTrip`: replace the `SptNative.Generate<DynamicOffersResult>(LootExport.DynamicOffers, …)` call with:

```csharp
        var result = SptNative.GenerateDynamicOffersFramed(System.Text.Encoding.UTF8.GetBytes(json.ToJsonString()));
```

  - `TheSameSeedProducesTheSameOffers`: unchanged (nullable members compare fine in the tuple).

- [ ] **Step 9: Sweep for orphaned references**

Run: `grep -rn "RagfairOfferWire\|ToRagfairOffer\|DynamicOffersResult\|LootExport.DynamicOffers" Libraries/ Testing/ SPTarkov.Server/`
Expected: only `DynamicOffersHeader`/`FramedOffersResult` hits and doc comments you just wrote. Fix any straggler.

- [ ] **Step 10: Run the gates**

Run: `dotnet build` then `dotnet test --filter "FullyQualifiedName~RagfairParityTests|FullyQualifiedName~SptNativeRagfairWireTests|FullyQualifiedName~RagfairPathDispatchTests|FullyQualifiedName~RagfairHookLivenessTests|FullyQualifiedName~DependencyInjectionValidationTests"`
Expected: green — parity 19/19 (seeded runs ride the unchanged sequential Rust path and the new envelope; byte-equality must hold because the per-offer JSON is produced by the same serializer from the same structs).

- [ ] **Step 11: Commit**

```bash
git add rust/spt-native/src/lib.rs rust/spt-native/src/ffi.rs rust/spt-native/src/ragfair/models.rs \
  Libraries/SPTarkov.Server.Core/Native/SptNative.cs \
  Libraries/SPTarkov.Server.Core/Native/Ragfair/RagfairPayloads.cs \
  Libraries/SPTarkov.Server.Core/Generators/Ragfair/RagfairOfferGenerator.cs \
  Testing/UnitTests/Tests/Generators/SptNativeRagfairWireTests.cs
git commit -m "perf: frame the ragfair response for parallel direct deserialize (ABI 9)"
```

---

### Task 4: Stage B checkpoint — full suite + benchmark

- [ ] **Step 1:** `cd rust && cargo test` and full `dotnet test` — green.
- [ ] **Step 2:** Benchmark exactly as Task 2 Step 2. Expected: native full pass ≈ **550–700 ms** (response leg ~490 → ~100–170 ms; the fixture runs workstation GC, so expect the worse end there — production server GC does better).
- [ ] **Step 3:** Append the row to `todo/RAGFAIR_PARITY_RESULTS.md`, including the ratio vs the same-run legacy median.
- [ ] **Step 4:** Commit: `git add todo/RAGFAIR_PARITY_RESULTS.md && git commit -m "test: record the stage B ragfair benchmark checkpoint"`

---

### Task 5: Converter-tax measurement (spec §3, measure-first — may legitimately end in "no change")

**Files:**
- Modify (only if the gate below demands it): `Tools/JsonExtensionDataGenerator/JsonExtensionDataGeneratorLauncher.cs` and/or `Tools/Ceciler` injection

The ~129 ms MongoId/ExtensionData tax was measured on a **single-threaded** deserialize;
Task 3's `Parallel.For` spreads it across cores. `StringToMongoIdConverter` already reads
`reader.ValueSpan` with no intermediate string, so the remaining candidates are MongoId's ctor
validation and the eagerly-allocated `[JsonExtensionData] Dictionary<string, object> ExtensionData … = []`
injected into every Models type (Release builds).

- [ ] **Step 1: Decide from the Task 4 numbers.** If the stage B native full-pass median is already ≤ 1.25× the same-run legacy median, **record "absorbed by parallel deserialize — no change" in `todo/RAGFAIR_PARITY_RESULTS.md` and skip to Task 6.** The spec's stage C still runs regardless.
- [ ] **Step 2 (only if the gate missed): Attribute it.** Copy `todo/ragfair-diagnostics/RagfairDeserializeCostTests.cs` into `Testing/UnitTests/Tests/Generators/`, add `<AllowUnsafeBlocks>true</AllowUnsafeBlocks>` to `Testing/UnitTests/UnitTests.csproj`, adapt its input to a single offer frame, and measure `RagfairOffer` vs a mirror POCO. Revert both files afterwards — they must not be committed.
- [ ] **Step 3 (only if step 2 shows ≥ 50 ms attributable to the extension dictionary):** change the injected property from `{ get; init; } = [];` to a lazy, still-never-null shape — `public Dictionary<string, object> ExtensionData { get => field ??= []; init; }` — in the generator template (`JsonExtensionDataGeneratorLauncher.cs:21`) and wherever `Tools/Ceciler` emits the equivalent IL initializer. STJ only touches the property when an unknown member appears, so a clean deserialize allocates nothing; any reader still gets a non-null dictionary. Verify with `dotnet test -c Release --filter "FullyQualifiedName~ModCompatibilityTests|FullyQualifiedName~SptNativeRagfairWireTests"`.
- [ ] **Step 4:** Commit whatever this task actually changed (possibly only the results note): `git commit -m "perf: <measured outcome of the converter-tax pass>"`

---

### Task 6: Stage C part 1 — C# MessagePack offer reader (tag not yet emitted by Rust)

Lands the `encoding == 1` arm fully unit-tested against hand-built buffers while Rust still
emits JSON — the tree stays green at the commit boundary.

**Files:**
- Modify: `Libraries/SPTarkov.Server.Core/SPTarkov.Server.Core.csproj` (add `<PackageReference Include="MessagePack" Version="3.1.4" />` — any current 3.1.x is acceptable if 3.1.4 has been superseded; record the chosen version in the commit message)
- Create: `Libraries/SPTarkov.Server.Core/Native/Ragfair/MsgpackOfferReader.cs`
- Modify: `Libraries/SPTarkov.Server.Core/Native/SptNative.cs` (dispatch `PayloadMsgpack = 1`)
- Test: `Testing/UnitTests/Tests/Generators/MsgpackOfferReaderTests.cs`

**Interfaces:**
- Consumes: frame bytes as `ReadOnlySpan<byte>` (offer) / header payload span.
- Produces: `internal static class MsgpackOfferReader` with
  `internal static RagfairOffer ReadOffer(ReadOnlySpan<byte> payload)` and
  `internal static DynamicOffersHeader ReadHeader(ReadOnlySpan<byte> payload)`.
- Wire contract: the msgpack payloads are **string-keyed maps using exactly the JSON wire names**
  (`_id`, `intId`, `user`, `root`, `items`, `itemsCost`, `requirements`, `requirementsCost`,
  `summaryCost`, `startTime`, `endTime`, `loyaltyLevel`, `sellInOnePiece`, `locked`, `quantity`;
  user: `id`, `nickname`, `rating`, `memberType`, `avatar`, `isRatingGrowing`, `aid`;
  requirement: `_tpl`, `count`, `onlyFunctional`, `level`, `side`;
  item: `_id`, `_tpl`, `parentId`, `slotId`, `location`, `desc`, `upd` + unknown keys),
  because Rust serializes with `rmp_serde::to_vec_named` over the same serde renames.

Design rules for the reader:

1. Known scalar members read directly with `MessagePackReader` (`ReadString`, `ReadDouble`,
   `ReadInt64`, `ReadBoolean`, `TryReadNil` before every nullable). MongoIds via
   `new MongoId(reader.ReadString())`. `memberType`/`side` are wire integers → cast to
   `MemberCategory`/`DogtagExchangeSide?`.
2. Any value that is not schema-known — `upd`, `location`, and every unknown item key — goes
   through one shared transcoder `MsgpackToJson.TranscodeValue(ref MessagePackReader, Utf8JsonWriter)`
   (recursive over msgpack maps/arrays/scalars; msgpack `bin`/`ext` are a wire-contract violation
   → throw). `upd` then materializes via `JsonSerializer.Deserialize<Upd>(bytes, options)`;
   `location` and unknown values via `JsonSerializer.Deserialize<JsonElement>(bytes)` so the
   materialized object graph is indistinguishable from the STJ path.
3. Unknown item keys land in the Ceciler-injected extension dictionary **via reflection with a
   cached delegate**, because the `ExtensionData` property exists only in Release/publish builds:

```csharp
    private static readonly Func<Item, Dictionary<string, object>?>? _itemExtensionData =
        typeof(Item).GetProperty("ExtensionData") is { } property
            ? item => (Dictionary<string, object>?)property.GetValue(item)
            : null;
```

   When `_itemExtensionData` is null (Debug build), unknown keys are skipped — exactly what STJ
   does without the attribute. Unknown keys on `RagfairOffer`/user/requirement maps: skip (the
   Rust serializer never emits any; skipping matches STJ-without-attribute).
4. Every offer gets `CreatedBy = OfferCreator.FakePlayer` before return, same as the JSON arm.

- [ ] **Step 1: Write the failing tests** (`MsgpackOfferReaderTests.cs`). Build buffers with `MessagePackWriter` in the test — no Rust involvement:
  - `AMinimalOfferMaterializesEveryKnownMember`: write a full offer map (all 15 keys, one item with `_id`/`_tpl`/nil `parentId`, one requirement, user); assert every property including `CreatedBy == OfferCreator.FakePlayer` and `Requirements.Single().TemplateId`.
  - `AnItemUpdRoundTripsThroughTheTranscoder`: item with `upd: {"stackObjectsCount": 3, "sptPresetId": "p"}`; assert `offer.Items[0].Upd.StackObjectsCount == 3`.
  - `AnIntegerLocationBecomesAJsonElementNumber`: `location: 5` → `((JsonElement)offer.Items[0].Location!).GetInt32() == 5`.
  - `AModAddedItemFieldLandsInExtensionData`: item map with an extra `"modField": "kept"` key; guard with `if (typeof(Item).GetProperty("ExtensionData") is null) { Assert.Ignore("extension data is Ceciler-injected in Release builds only"); }`, then assert via reflection that the dictionary holds a `JsonElement` `"kept"`.
  - `AHeaderPayloadParses`: header map with one rejected tpl and one diagnostic; assert both.

- [ ] **Step 2: Run to verify they fail**

Run: `dotnet test --filter "FullyQualifiedName~MsgpackOfferReaderTests"`
Expected: FAIL — `MsgpackOfferReader` does not exist.

- [ ] **Step 3: Implement** `MsgpackOfferReader.cs` per the design rules (~250 lines: `ReadOffer`, `ReadUser`, `ReadRequirement`, `ReadItem`, `ReadHeader`, `TranscodeValue`). Skeleton of the dispatch loop shape, applied to each map reader:

```csharp
    internal static RagfairOffer ReadOffer(ReadOnlySpan<byte> payload)
    {
        var reader = new MessagePackReader(payload.ToArray());
        // ponytail: per-frame copy to satisfy MessagePackReader's ReadOnlySequence input; swap to
        // a pooled buffer or a MemoryManager over the native buffer if stage C profiling flags it
        var offer = new RagfairOffer { User = null! };
        var memberCount = reader.ReadMapHeader();
        for (var i = 0; i < memberCount; i++)
        {
            var key = reader.ReadString();
            switch (key)
            {
                case "_id":
                    offer.Id = new MongoId(reader.ReadString());
                    break;
                // ... every wire key ...
                default:
                    reader.Skip();
                    break;
            }
        }

        offer.CreatedBy = OfferCreator.FakePlayer;
        return offer;
    }
```

(`User = null!` is overwritten by the `user` key, which the wire always carries; assert it
non-null before returning and throw `InvalidOperationException` otherwise.)

- [ ] **Step 4: Run to verify they pass**

Run: `dotnet test --filter "FullyQualifiedName~MsgpackOfferReaderTests"`
Expected: PASS.

- [ ] **Step 5: Wire the dispatch** in `SptNative.cs`: add `private const byte PayloadMsgpack = 1;`; in `DeserializeHeader` and `DeserializeOfferFrame`, route `encoding == PayloadMsgpack` to `MsgpackOfferReader.ReadHeader`/`ReadOffer` (the frame span), keeping the unknown-tag throw for anything else.

- [ ] **Step 6: Full ragfair filter run** (Rust still emits JSON — proves the JSON arm is undisturbed)

Run: `dotnet test --filter "FullyQualifiedName~Ragfair|FullyQualifiedName~MsgpackOfferReader"`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add Libraries/SPTarkov.Server.Core/SPTarkov.Server.Core.csproj \
  Libraries/SPTarkov.Server.Core/Native/Ragfair/MsgpackOfferReader.cs \
  Libraries/SPTarkov.Server.Core/Native/SptNative.cs \
  Testing/UnitTests/Tests/Generators/MsgpackOfferReaderTests.cs
git commit -m "feat: MessagePack offer reader for the framed ragfair envelope (tag 1)"
```

---

### Task 7: Stage C part 2 — Rust emits MessagePack (ABI 10)

**Files:**
- Modify: `rust/spt-native/Cargo.toml` (`rmp-serde = "=1.3.1"`)
- Modify: `rust/spt-native/src/ffi.rs` (`write_framed_offers` encoding + tests)
- Modify: `rust/spt-native/src/lib.rs` (`ABI_VERSION` 9 → 10)
- Modify: `Libraries/SPTarkov.Server.Core/Native/SptNative.cs` (`ExpectedAbiVersion` → 10)
- Test: `Testing/UnitTests/Tests/Generators/SptNativeRagfairWireTests.cs` (item-level mod field round trip)

**Interfaces:**
- `write_framed_offers` emits tag `PAYLOAD_MSGPACK = 1`; header and every offer payload become `rmp_serde::to_vec_named` output (string-keyed maps, wire names identical to the JSON stage). Frame *structure* is unchanged. C# JSON arm stays in place (tag-dispatched, exercised by Task 6's unit tests only).

- [ ] **Step 1:** Add the dependency to `rust/spt-native/Cargo.toml` under `[dependencies]`:

```toml
rmp-serde = "=1.3.1"
```

- [ ] **Step 2: Flip the writer** in `ffi.rs`:

```rust
pub const PAYLOAD_MSGPACK: u8 = 1;
```

In `write_framed_offers`, replace both `serde_json::to_vec` calls with
`rmp_serde::to_vec_named(...).expect("… serialization cannot fail")` and push
`PAYLOAD_MSGPACK` instead of `PAYLOAD_JSON`. Keep `PAYLOAD_JSON` declared with a doc comment
noting it is the stage-B tag the C# reader still accepts.

- [ ] **Step 3: Bump ABI.** `lib.rs` → `10`; `ffi.rs` abi test literal → `10`; `SptNative.cs` `ExpectedAbiVersion` → `10`.

- [ ] **Step 4: Update the Rust frame tests.** In `parse_framed`, deserialize the header and payloads with `rmp_serde::from_slice::<serde_json::Value>` when `encoding == PAYLOAD_MSGPACK` (msgpack maps/scalars land in `Value` cleanly — the writer never emits bin/ext). Assert `encoding == PAYLOAD_MSGPACK` in the two round-trip tests; keep all 30 assertions of the expired-pass test working on the `Value`s. Add one key-name pin so a serde rename regression fails loudly:

```rust
        let offer: serde_json::Value = rmp_serde::from_slice(&payloads[0]).unwrap();
        for key in ["_id", "intId", "user", "root", "items", "requirements"] {
            assert!(offer.get(key).is_some(), "offer payload lost wire key {key}");
        }
```

- [ ] **Step 5: Rust gate**

Run: `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 6: Add the end-to-end item-level mod-field test** to `SptNativeRagfairWireTests.cs` — the msgpack analog of the config-level test, through the real native library:

```csharp
    /// <summary>
    /// A mod-added field on an expired offer's item must survive Rust's `extra` flatten and come
    /// back through the MessagePack frames into the Ceciler-injected extension data.
    /// </summary>
    [Test]
    public void AModAddedItemFieldSurvivesTheRoundTrip()
    {
        if (typeof(Item).GetProperty("ExtensionData") is not { } extensionData)
        {
            Assert.Ignore("extension data is Ceciler-injected in Release builds only");
            return;
        }

        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!.AsObject();
        var itemTpl = json["items"]!.AsObject().First().Key;
        json["expiredOffers"] = new JsonArray(
            new JsonArray(new JsonObject { ["_id"] = "0123456789abcdef01234567", ["_tpl"] = itemTpl, ["modField"] = "kept" })
        );

        var result = SptNative.GenerateDynamicOffersFramed(System.Text.Encoding.UTF8.GetBytes(json.ToJsonString()));

        Assert.That(result.Offers, Has.Count.EqualTo(1));
        var extension = (Dictionary<string, object>)extensionData.GetValue(result.Offers[0].Items![0])!;
        Assert.That(((JsonElement)extension["modField"]).GetString(), Is.EqualTo("kept"));
    }
```

(If the chosen `itemTpl` turns out unpriced and the expired pass errors, pick a tpl present in
`json["fleaPrices"]` instead — expired entries still price.)

- [ ] **Step 7: Full gates end-to-end on the msgpack envelope**

Run: `dotnet build` then `dotnet test --filter "FullyQualifiedName~RagfairParityTests|FullyQualifiedName~SptNativeRagfairWireTests|FullyQualifiedName~RagfairPathDispatchTests|FullyQualifiedName~RagfairHookLivenessTests"`
Expected: green — parity now rides msgpack frames; byte-equality of *offer content* is what the parity fixtures compare, and content is encoding-independent.

- [ ] **Step 8: Commit**

```bash
git add rust/spt-native/Cargo.toml rust/spt-native/Cargo.lock rust/spt-native/src/ffi.rs rust/spt-native/src/lib.rs \
  Libraries/SPTarkov.Server.Core/Native/SptNative.cs Testing/UnitTests/Tests/Generators/SptNativeRagfairWireTests.cs
git commit -m "perf: MessagePack payloads in the ragfair frames via rmp-serde 1.3.1 (ABI 10)"
```

---

### Task 8: Stage C checkpoint — full suite, benchmark, gate decision

- [ ] **Step 1:** `cd rust && cargo test` and full `dotnet test` (Debug) — green.
- [ ] **Step 2:** `dotnet test -c Release` full suite once — the Release-only extension-data tests (`AModAddedItemFieldSurvivesTheRoundTrip`, `AModAddedItemFieldLandsInExtensionData`) must run, not `Assert.Ignore`.
- [ ] **Step 3:** Benchmark exactly as Task 2 Step 2; append the stage C row to `todo/RAGFAIR_PARITY_RESULTS.md`.
- [ ] **Step 4: Evaluate the spec §1 gate:** native full-pass median ≤ **1.25×** same-run legacy median.
  - **Gate met:** mark Task 9 skipped in this plan file (`- [x] … SKIPPED, gate met at N.NNx`) and continue to Task 10.
  - **Gate missed:** execute Task 9.
- [ ] **Step 5:** Commit the results row.

---

### Task 9: CONDITIONAL fallback — build offer items without clone-then-reparent (spec §1)

Execute only if Task 8's gate missed. Worth ~100 ms sequential, less after stage A — this is
the last in-scope lever.

**Files:**
- Modify: `rust/spt-native/src/ragfair/offer_generator.rs:560-584` (`create_offers_from_assort`'s per-offer loop)

**Interfaces:**
- Produces: `fn clone_with_fresh_ids(assort_item_with_children: &[Item]) -> Vec<Item>` — one pass that clones each item, keeps the root's id, mints `mongo_id::generate()` ids for children, and remaps every `parent_id` through an old→new map; root's `parent_id`/`slot_id` cleared. Replaces the `clone()` + `cloned_root` snapshot + `reparent_item_and_children` + two clearing lines.

- [ ] **Step 1:** The behavior is already pinned by `every_offer_of_one_assort_is_a_detached_clone_with_its_own_seller` (`offer_generator.rs:3235`) — run it first, keep it green throughout. Add one new test asserting a grandchild's `parent_id` remaps to its parent's *fresh* id (three-level tree), since the existing pin only checks depth 1.
- [ ] **Step 2:** Implement `clone_with_fresh_ids`; replace the loop body. `mongo_id::generate()` draws are outside the seeded stream (clock + atomic counter), so the seeded draw sequence — and therefore parity — is unaffected; the parity fixtures normalize ids regardless.
- [ ] **Step 3:** `cd rust && cargo test && cargo clippy --all-targets -- -D warnings`; then the C# parity filter; then re-benchmark and append a row.
- [ ] **Step 4:** Commit: `perf: build ragfair offer items in one fresh-id pass`

---

### Task 10: Documentation

**Files:**
- Modify: `BENCHMARK.md` (ragfair section, `## Results — ragfair offer generation` at :239)
- Modify: `RUST-ROADMAP.md`
- Delete: `todo/RAGFAIR_PARITY_RESULTS.md` (contents graduate into BENCHMARK.md)

- [ ] **Step 1: BENCHMARK.md.** Replace the stale ragfair figures with the checkpoint table from `todo/RAGFAIR_PARITY_RESULTS.md` (per-stage medians, final ratio). Corrections from the spec §6, wherever the old claims appear: the full pass *produces* ~58k offers (~24k is the post-cap accepted count — re-measure the wire size now that it's msgpack and state both); the fixture runs workstation GC while `SPTarkov.Server.csproj` sets `ServerGarbageCollection=true`.
- [ ] **Step 2: RUST-ROADMAP.md.**
  - Mark roadmap item 4 done with the measured result; delete the now-false "ship them only if…" framing.
  - Rewrite the "**The ragfair batch walk is sequential**" divergence bullet: unseeded batches fan across rayon with per-worker contexts, seeded batches stay sequential; the response is a framed MessagePack envelope (ABI 10) parsed in parallel on the C# side.
  - Amend the item-3 retraction with the regeneration-pass exception (worth ~1% of a full pass but ~60% of a regeneration pass) and add a new roadmap entry: *database mutation stamp + cached serialized request slice — the sanctioned way to reopen the request-side cache; the stamp is the precondition* (spec §7).
  - Add the latent `get_flea_prices_as_array` hazard (dead at `barter.chancePercent: 0`, O(offers × price-table) if a mod raises it) to the known-divergences list.
- [ ] **Step 3:** `git rm todo/RAGFAIR_PARITY_RESULTS.md`. Also check `ARCHITECTURE.md` and `todo/TODO.md` for the phrase "JSON buffer out" / ragfair-envelope claims and update the one-line descriptions to mention the framed envelope (orientation-level only — no wire-format detail, per the ARCHITECTURE.md scope convention).
- [ ] **Step 4:** Commit: `docs: record ragfair parity results and the framed envelope`

---

### Task 11: Final verification and format

- [ ] **Step 1:** `csharpier format .` — commit any churn separately as `style: csharpier`.
- [ ] **Step 2:** `cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
- [ ] **Step 3:** Full `dotnet test` (Debug) and `dotnet test -c Release` — both green.
- [ ] **Step 4:** `graphify update .` (keeps the knowledge graph current, per CLAUDE.md).
- [ ] **Step 5:** Confirm the spec §1 success criteria against the recorded numbers and state the outcome plainly — including the measured ratio — in the final report. If the gate was missed even after Task 9, say so with the numbers; do not soften it.
