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

Delete this file in Task 10 once BENCHMARK.md carries the final numbers.
