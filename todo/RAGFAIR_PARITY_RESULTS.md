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

Delete this file in Task 10 once BENCHMARK.md carries the final numbers.
