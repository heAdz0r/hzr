# Memory Layer Benchmarks

## Repro
```bash
bash benchmarks/memory/bench_memory.sh
python3 benchmarks/memory/analyze_memory.py
python3 -m unittest discover -s benchmarks/memory/tests -p 'test_*.py'
```

## Environment
```text
Date: Wed Feb 18 09:33:10 UTC 2026
Commit: b8b7d3fbd7dba29002fc6dce2f969e3bc6ef53a7
cold_runs: 5
hot_runs: 80
api_runs: 80
mem_port: 17770
rtk_bin: /Users/andrew/Programming/rtk/target/release/rtk
native_explore_tokens: 52000
OS: Darwin MacBook-Pro-Andy.local 25.2.0 Darwin Kernel Version 25.2.0: Tue Nov 18 21:09:56 PST 2025; root:xnu-12377.61.12~1/RELEASE_ARM64_T6041 arm64
```

## Threshold Gates
- [PASS] cli hot p95 < 200ms (p95=11.30ms)
- [PASS] cli hot cache-hit rate >= 0.95 (rate=1.00)
- [PASS] api hot p95 < 200ms (p95=8.05ms)
- [PASS] cli hot p50 < cli cold p50 (hot=10.40ms cold=43.01ms)
- [PASS] memory gain savings >= 50% (savings=89.00%)
- [PASS] estimated memory tokens <= 50% of native explore baseline (native=52000 est=5720)
- [PASS] 5-step cumulative savings >= 1x native explore baseline (saved_5=231400 native=52000)

## Metrics
| scenario | runs | p50 ms | p95 ms | p99 ms | cache_hit_rate |
|---|---:|---:|---:|---:|---:|
| api_hot | 80 | 7.334 | 8.052 | 8.723 | 1.00 |
| cli_cold | 5 | 43.006 | 57.382 | 60.214 | 0.00 |
| cli_hot | 80 | 10.402 | 11.301 | 12.190 | 1.00 |

memory_gain savings: **89.00%**

## Native Explore Baseline Projection
Assumed native Task/Explore baseline: **52000 tokens** per run (override via `NATIVE_EXPLORE_TOKENS`).
Estimated memory-context cost per run: **5720 tokens** (saved: **46280**).

| explore-driven steps | native tokens | memory tokens (est) | saved tokens | savings % |
|---:|---:|---:|---:|---:|
| 1 | 52000 | 5720 | 46280 | 89.00% |
| 3 | 156000 | 17160 | 138840 | 89.00% |
| 5 | 260000 | 28600 | 231400 | 89.00% |
