# Write Benchmarks

## Repro
```bash
bash benchmarks/write/bench_write.sh
python3 benchmarks/write/analyze_write.py
python3 -m unittest discover -s benchmarks/write/tests -p 'test_*.py'
```

## Environment
```text
Date: Tue Feb 17 10:44:35 UTC 2026
Commit: bd240dc00f4302e2d5e36990b0ada8783359898d
runs: 5
write_tool: /Users/andrew/Programming/rtk/target/release/write_bench_tool
write_tool_version: ok
OS: Darwin MacBook-Pro-Andy.local 25.2.0 Darwin Kernel Version 25.2.0: Tue Nov 18 21:09:56 PST 2025; root:xnu-12377.61.12~1/RELEASE_ARM64_T6041 arm64
```

## Threshold Gates
- [PASS] unchanged p50 < 2ms (small, durable) (p50=0.016ms)
- [PASS] unchanged p50 < 2ms (small, fast) (p50=0.015ms)
- [PASS] unchanged p50 < 2ms (medium, durable) (p50=0.029ms)
- [PASS] unchanged p50 < 2ms (medium, fast) (p50=0.032ms)
- [PASS] durable <= 1.25x native (small) (ratio=0.935x)
- [PASS] durable <= 1.25x native (medium) (ratio=1.023x)
- [PASS] durable <= 1.25x native (large) (ratio=1.025x)
- [PASS] fast < durable (changed, small) (fast=0.224ms durable=8.101ms)

## Metrics
| scenario | size | tool | mode | p50 ms | p95 ms | MiB/s | write_amp | fsync | rename | skip_rate |
|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|
| changed | large | native_safe | durable | 9.890 | 11.176 | 808.90 | 1.00 | 2.00 | 1.00 | 0.00 |
| changed | large | write_core | durable | 10.133 | 10.505 | 789.50 | 1.00 | 2.00 | 1.00 | 0.00 |
| changed | large | write_core | fast | 1.118 | 1.164 | 7155.64 | 1.00 | 0.00 | 1.00 | 0.00 |
| changed | medium | native_safe | durable | 7.723 | 8.875 | 16.19 | 1.00 | 2.00 | 1.00 | 0.00 |
| changed | medium | write_core | durable | 7.903 | 8.501 | 15.82 | 1.00 | 2.00 | 1.00 | 0.00 |
| changed | medium | write_core | fast | 0.251 | 0.263 | 498.01 | 1.00 | 0.00 | 1.00 | 0.00 |
| changed | small | native_safe | durable | 8.665 | 8.738 | 0.11 | 1.00 | 2.00 | 1.00 | 0.00 |
| changed | small | write_core | durable | 8.101 | 11.443 | 0.12 | 1.00 | 2.00 | 1.00 | 0.00 |
| changed | small | write_core | fast | 0.224 | 0.272 | 4.36 | 1.00 | 0.00 | 1.00 | 0.00 |
| unchanged | large | write_core | durable | 1.791 | 3.084 | 0.00 | 0.00 | 0.00 | 0.00 | 1.00 |
| unchanged | large | write_core | fast | 0.963 | 0.993 | 0.00 | 0.00 | 0.00 | 0.00 | 1.00 |
| unchanged | medium | write_core | durable | 0.029 | 0.033 | 0.00 | 0.00 | 0.00 | 0.00 | 1.00 |
| unchanged | medium | write_core | fast | 0.032 | 0.034 | 0.00 | 0.00 | 0.00 | 0.00 | 1.00 |
| unchanged | small | write_core | durable | 0.016 | 0.018 | 0.00 | 0.00 | 0.00 | 0.00 | 1.00 |
| unchanged | small | write_core | fast | 0.015 | 0.016 | 0.00 | 0.00 | 0.00 | 0.00 | 1.00 |
