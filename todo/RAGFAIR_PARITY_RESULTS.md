# Ragfair native parity — benchmark checkpoints

Fixture: `RagfairBenchmarkTests` (`[Explicit]`), Release, n=5 per phase, same machine.
Run with:

```bash
dotnet test -c Release --filter "FullyQualifiedName~RagfairBenchmarkTests" --logger "console;verbosity=detailed"
```

Ratio is the printed speedup: median legacy / median native. Below 1.0x means native is slower.

| Stage | Date | Full pass native | Full pass legacy | Ratio | Regen native | Regen legacy | Ratio | Alloc/run native (full) | Alloc/run legacy (full) |
|---|---|---|---|---|---|---|---|---|---|
| Baseline (pre-stage A) | 2026-08-14 | 1550 ms | 517 ms | 0.33x | ~103 ms | ~12.8 ms | ~0.12x | not recorded | not recorded |
| Stage A (rayon batch fan-out) | 2026-08-14 | 884.67 ms | 454.37 ms | 0.51x | 85.42 ms | 9.87 ms | 0.12x | 182.9 MB | 285.0 MB |
| Stage B (framed FFI response, ABI 9) | 2026-08-14 | 676.12 ms | 443.22 ms | 0.66x | 75.00 ms | 10.50 ms | 0.14x | 208.4 MB | 283.4 MB |
| Stage C (MessagePack payloads, ABI 10) | 2026-08-14 | 719.39 ms | 441.66 ms | 0.61x | 75.67 ms | 10.78 ms | 0.14x | 358.3 MB | 282.6 MB |

## Stage A notes

Commit 5fefd61 (`perf: fan the ragfair batch walk across rayon when unseeded`).

Verbatim from the run:

```
full pass native (rust)              n=5  mean=867.49 ms  median=884.67 ms  min=794.52 ms  max=912.43 ms
                                     offers=24284  alloc/run=182.9 MB  peak RSS=1074 MB (+275 MB over the phase)
full pass legacy (C# 4.1.2)          n=5  mean=465.40 ms  median=454.37 ms  min=436.37 ms  max=521.80 ms
                                     offers=24281  alloc/run=285.0 MB  peak RSS=976 MB (+88 MB over the phase)
full pass BuildRequest only          n=5  mean=14.98 ms  median=13.37 ms  min=8.62 ms  max=20.06 ms
                                     offers=0  alloc/run=7.1 MB  peak RSS=960 MB (+9 MB over the phase)
full pass            speedup (median legacy / median native): 0.51x  projection share of native median: 1.5%

regeneration pass native (rust)      n=5  mean=82.98 ms  median=85.42 ms  min=77.87 ms  max=87.38 ms
                                     offers=897  alloc/run=16.8 MB  peak RSS=983 MB (+32 MB over the phase)
regeneration pass legacy (C# 4.1.2)  n=5  mean=9.87 ms  median=9.87 ms  min=5.59 ms  max=15.63 ms
                                     offers=876  alloc/run=5.9 MB  peak RSS=952 MB (+0 MB over the phase)
regeneration pass BuildRequest only  n=5  mean=13.94 ms  median=14.01 ms  min=9.49 ms  max=18.04 ms
                                     offers=0  alloc/run=7.1 MB  peak RSS=924 MB (+2 MB over the phase)
regeneration pass    speedup (median legacy / median native): 0.12x  projection share of native median: 16.4%
```

Generation itself (the `CreateOffersFromAssort` timer inside the native full pass) fell from ~710 ms to
91–103 ms, past the ~165 ms the plan projected. The full pass only moved 1550 → 885 ms, so the residual
~780 ms sits outside generation — payload transport, which the later stages target.

## Stage B notes

Commit 357c7f3 (`perf: frame the ragfair response for parallel direct deserialize (ABI 9)`).

Verbatim from the run:

```
full pass native (rust)              n=5  mean=664.78 ms  median=676.12 ms  min=598.22 ms  max=738.76 ms
                                     offers=24045  alloc/run=208.4 MB  peak RSS=1277 MB (+449 MB over the phase)
full pass legacy (C# 4.1.2)          n=5  mean=445.44 ms  median=443.22 ms  min=405.89 ms  max=483.81 ms
                                     offers=23915  alloc/run=283.4 MB  peak RSS=1202 MB (+0 MB over the phase)
full pass BuildRequest only          n=5  mean=12.34 ms  median=12.50 ms  min=8.74 ms  max=14.78 ms
                                     offers=0  alloc/run=7.1 MB  peak RSS=1001 MB (+3 MB over the phase)
full pass            speedup (median legacy / median native): 0.66x  projection share of native median: 1.8%

regeneration pass native (rust)      n=5  mean=74.22 ms  median=75.00 ms  min=66.47 ms  max=80.11 ms
                                     offers=906  alloc/run=17.5 MB  peak RSS=1023 MB (+30 MB over the phase)
regeneration pass legacy (C# 4.1.2)  n=5  mean=9.99 ms  median=10.50 ms  min=5.52 ms  max=15.04 ms
                                     offers=902  alloc/run=5.9 MB  peak RSS=995 MB (+0 MB over the phase)
regeneration pass BuildRequest only  n=5  mean=13.57 ms  median=13.04 ms  min=8.74 ms  max=19.41 ms
                                     offers=0  alloc/run=7.1 MB  peak RSS=965 MB (+0 MB over the phase)
regeneration pass    speedup (median legacy / median native): 0.14x  projection share of native median: 17.4%
```

The full pass fell 884.67 → 676.12 ms, landing in the plan's 550–700 ms band at the worse end the fixture's
workstation GC predicted. Generation itself did not move — the `CreateOffersFromAssort` timer inside the native
full pass still reads 91–100 ms — so the whole 208 ms came off the response leg, which is the ~490 ms stage B
targeted. That leg is now ~280 ms rather than the projected ~100–170 ms, so roughly half the framing win the
plan expected is still outstanding.

Native alloc/run rose 182.9 → 208.4 MB and the phase RSS delta rose +275 → +449 MB: the per-offer frames cost
more managed allocation than the single-buffer path they replaced, and that GC pressure is part of why the
response leg stopped short of the projection.

## Stage C notes

Commit d076b39 (`perf: MessagePack payloads in the ragfair frames via rmp-serde 1.3.1 (ABI 10)`).

Verbatim from the run:

```
full pass native (rust)              n=5  mean=788.57 ms  median=719.39 ms  min=679.47 ms  max=937.93 ms
                                     offers=24258  alloc/run=358.3 MB  peak RSS=1252 MB (+440 MB over the phase)
full pass legacy (C# 4.1.2)          n=5  mean=451.93 ms  median=441.66 ms  min=430.87 ms  max=508.01 ms
                                     offers=24098  alloc/run=282.6 MB  peak RSS=1185 MB (+0 MB over the phase)
full pass BuildRequest only          n=5  mean=14.28 ms  median=13.02 ms  min=8.44 ms  max=22.80 ms
                                     offers=0  alloc/run=7.1 MB  peak RSS=1000 MB (+2 MB over the phase)
full pass            speedup (median legacy / median native): 0.61x  projection share of native median: 1.8%

regeneration pass native (rust)      n=5  mean=76.07 ms  median=75.67 ms  min=74.68 ms  max=79.23 ms
                                     offers=899  alloc/run=20.8 MB  peak RSS=1025 MB (+33 MB over the phase)
regeneration pass legacy (C# 4.1.2)  n=5  mean=11.07 ms  median=10.78 ms  min=6.00 ms  max=17.62 ms
                                     offers=888  alloc/run=5.9 MB  peak RSS=981 MB (+0 MB over the phase)
regeneration pass BuildRequest only  n=5  mean=14.36 ms  median=13.79 ms  min=8.35 ms  max=22.15 ms
                                     offers=0  alloc/run=7.1 MB  peak RSS=929 MB (+0 MB over the phase)
regeneration pass    speedup (median legacy / median native): 0.14x  projection share of native median: 18.2%
```

Because the primary run is a *regression* against stage B, a second run was taken back-to-back on the same
machine to rule out noise. It reproduces:

```
full pass native (rust)              n=5  mean=799.32 ms  median=727.67 ms  min=685.19 ms  max=953.73 ms
                                     offers=24213  alloc/run=357.0 MB  peak RSS=1242 MB (+453 MB over the phase)
full pass legacy (C# 4.1.2)          n=5  mean=447.26 ms  median=429.17 ms  min=427.89 ms  max=512.59 ms
                                     offers=24121  alloc/run=283.6 MB  peak RSS=1163 MB (+0 MB over the phase)
full pass            speedup (median legacy / median native): 0.59x
```

MessagePack made the full pass **slower**, not faster: 676.12 → 719.39 ms median (+43 ms, +6.4%), and the
mean moved further, 664.78 → 788.57 ms (+18.6%). Legacy is unchanged (443.22 → 441.66 ms), so this is the
native leg alone. The regeneration pass did not move (75.00 → 75.67 ms).

The mechanism is visible in the allocation column: native alloc/run jumped **208.4 → 358.3 MB** (+150 MB,
+72%) while the offer count is flat (~24.2k). The msgpack reader allocates substantially more managed memory
per pass than the JSON path it replaced, and under the fixture's workstation GC that extra pressure costs
more than the parsing win it buys. The spread supports that reading: native min/max widened to
679.47–937.93 (stage B: 598.22–738.76), the signature of GC pauses landing inside timed runs.

## Spec §1 gate decision — MISSED

The gate: stage C native full-pass median ≤ **1.25×** the same-run legacy median.

```
native median  719.39 ms
legacy median  441.66 ms   (same run)
ratio          719.39 / 441.66 = 1.629x
bar            1.25x
```

**The gate is missed at 1.63x** — native is 63% slower than legacy, needing 552.08 ms to pass, i.e. **167 ms
still to remove**. The confirmation run is worse, 727.67 / 429.17 = **1.70x**. Stage C did not close the gap;
it widened it, from stage B's 1.53x to 1.63x.

**Task 9 therefore executes.** Its first question should be the +150 MB allocation regression stage C
introduced, since reverting or fixing that alone plausibly recovers the 43 ms back to stage B's 1.53x — but
1.53x is itself still well over the 1.25x bar, so recovering stage B is necessary and not sufficient.

## Converter-tax attribution (Task 5) — no code change

Measured with a throwaway `RagfairDeserializeCostTests` fixture (deleted afterwards, along with the
`AllowUnsafeBlocks` it needed) that captures one framed full-pass response, then times the shipped
`Parallel.For` deserialize against the tokenizer floor and against each candidate tax at its real
per-pass instance count. Release, n=7 medians, 12 cores, 41.4 MB / 58,137 frames.

```
A. Deserialize<RagfairOffer> (shipped)       median=304.8 ms  alloc=193 MB
A1. Deserialize<RagfairOffer> sequential     median=268.6 ms  alloc=193 MB
B. Utf8JsonReader skim (token floor)         median= 16.3 ms  alloc=  1 MB
C. new Dictionary<string, object>() x census median=  3.4 ms  alloc= 28 MB
D. new MongoId(ReadOnlySpan<byte>) x ids     median= 11.1 ms  alloc= 49 MB
E. string.Intern(nickname) x offers          median= 15.1 ms  alloc=  0 MB

Models-type instances per pass: 366,851   (Item 83,292 | Upd 60,325 | RagfairOffer 58,558 |
OfferRequirement 58,558 | RagfairOfferUser 58,558 | UpdRepairable 43,300 | tail 4,260)
MongoId values per pass: 425,550   nicknames per pass: 58,558
```

The Ceciler-injected `[JsonExtensionData]` dictionary is **3.4 ms** of the ~305 ms leg — 28 MB of the
193 MB the pass allocates, but allocation is a pointer bump and the collection cost rides along with
the other 165 MB. That is 15x under the 50 ms bar the plan set for changing the injected property, so
`JsonExtensionDataGeneratorLauncher`'s template and `Ceciler.JsonExtensionData` are left alone.

MongoId's ctor validation (11.1 ms over 425,550 ids) and `RagfairOfferUser.Nickname`'s `string.Intern`
(15.1 ms over 58,558 offers) do not explain the gap either. All three together are ~30 ms; the
remaining ~275 ms is System.Text.Json binding 366,851 objects, which no converter-level change reaches.

### The response leg's *overshoot* is a harness artifact, not a converter tax

`A1` is the finding that matters: single-threaded deserialize (268.6 ms) beats the `Parallel.For`
one (304.8 ms). The test host runs **workstation GC** — `<ServerGarbageCollection>true</ServerGarbageCollection>`
is set on `SPTarkov.Server.csproj` only, not on `UnitTests.csproj` — so at 193 MB per pass the fan-out
is GC-throttled and Task 3's parallelism is a net loss in the fixture. Re-running the same fixture with
`DOTNET_gcServer=1`:

```
A. Deserialize<RagfairOffer> (shipped)       median=158.2 ms  min=67.8   alloc=191 MB
A1. Deserialize<RagfairOffer> sequential     median=255.4 ms  min=253.4  alloc=191 MB
```

Under the GC the shipped server actually uses, the response leg is **158 ms** — at the top of the
plan's 100–170 ms projection. Read that number with the spread in mind: min 67.8 vs median 158.2 is a
2.3x min/median ratio, where the workstation-GC distribution is tight (292.7–326.4), so at n=7 the
server-GC leg is somewhere in the 70–160 ms range rather than pinned at 158. The "roughly half the
framing win is outstanding" note in the stage B block is measuring workstation GC, not the port.

The whole benchmark under `DOTNET_gcServer=1` (same commit, same machine) moves both sides, so the
gate does not come back:

```
full pass native (rust)              n=5  median=558.95 ms  min=417.63  alloc/run=209.1 MB
full pass legacy (C# 4.1.2)          n=5  median=329.51 ms  min=300.32  alloc/run=282.6 MB
full pass            speedup: 0.59x   (1.70x legacy, vs 1.53x under workstation GC)

regeneration pass native (rust)      n=5  median=63.40 ms
regeneration pass legacy (C# 4.1.2)  n=5  median= 6.29 ms
regeneration pass    speedup: 0.10x
```

Server GC takes ~117 ms off native and ~113 ms off legacy, so the ratio is unchanged-to-slightly-worse
while both absolute numbers improve.

The response leg is still the largest identified component of the deficit and Task 6 should not be
steered off it. Native is 229 ms behind legacy under server GC (559 vs 330), and the ~158 ms response
leg is ~69% of that gap — legacy has no equivalent leg at all, since it never crosses a wire. What the
measurement changes is only the leg's standing *against projection*: it is now at the plan's projected
100–170 ms, so the shortfall relative to the plan lives elsewhere — the request legs and the ~63 ms
floor the regeneration pass shows. Closing the remaining gap to legacy still means making the response
leg cheaper than the plan ever projected, or not paying it at all.

Delete this file in Task 10 once BENCHMARK.md carries the final numbers.
