# Code Search Benchmark: grep vs rtk grep vs rtk rgai vs head_n

## Environment & Reproduction

```
Date: Sat Feb 14 22:34:25 UTC 2026
Commit: 4b0a413562c775757d5bc09a6ff966b4e532508c
rtk_bin: /Users/andrew/Programming/rtk/target/release/rtk
rtk: rtk 0.15.3
grep: grep (BSD grep, GNU compatible) 2.6.0-FreeBSD
tiktoken_encoding: cl100k_base
rtk_grep_max: 200
rgai_max: 8
head_n_lines: 100
OS: Darwin MacBook-Pro-Andy.local 25.2.0 Darwin Kernel Version 25.2.0: Tue Nov 18 21:09:56 PST 2025; root:xnu-12377.61.12~1/RELEASE_ARM64_T6041 arm64
CPU: Apple M4 Pro
Rust files: 54
Total LOC: 23240
```

## Dataset: rtk-ai/rtk @ `4b0a413562c775757d5bc09a6ff966b4e532508c`

**Reproduction**:
```bash
rtk --version
bash benchmarks/bench_code.sh
python3 benchmarks/analyze_code.py
python3 -m unittest discover -s benchmarks/tests -p 'test_*.py'
```

## Methodology

### Metrics (reported separately, NO composite score)

| Metric | Definition | Purpose |
|--------|-----------|---------|
| Output bytes | `wc -c` of stdout | Raw size footprint |
| Output tokens | `tiktoken` (`cl100k_base`) on full stdout | Model-aligned token cost |
| Token Efficiency (TE) | `output_tokens / grep_output_tokens` | Token compression vs baseline |
| Result count | Effective output lines / no-result aware count | Distinguish compactness vs empty results |
| Gold hit rate | `% gold_files found` (plus found/min files) | Relevance/correctness |
| Timing | Median of 5 runs, plus min/max in summaries | Performance distribution |

**Critical rule**: if `expect_results=true` and `result_count==0`, mark as **MISS**.
For regex category, `rtk rgai` is marked `EXPECTED_UNSUPPORTED` by design.

### Categories

| Category | Queries |
|----------|---------|
| A: Exact Identifier | 6 |
| B: Regex Pattern | 6 |
| C: Semantic Intent | 10 |
| D: Cross-File Pattern Discovery | 5 |
| E: Edge Cases | 3 |

## Category A: Exact Identifier Search

| ID | Query | Tool | Bytes | Tokens | TE | Result Count | Gold Hit | Timing (med) | Status |
|----|-------|------|-------|--------|----|-------------|----------|-------------|--------|
| A1 | TimedExecution | grep | 10338 | 2927 | 1.000 | 104 | 100% (34/30) | 6.0ms | OK |
| A1 | TimedExecution | rtk_grep | 6527 | 1979 | 0.676 | 159 | 100% (34/30) | 25.0ms | OK |
| A1 | TimedExecution | rtk_rgai | 2797 | 841 | 0.287 | 94 | 60% (8/30) | 11.0ms | LOW_COVERAGE |
| A1 | TimedExecution | head_n | 9933 | 2810 | 0.960 | 100 | 100% (32/30) | 0μs | OK |
| A2 | FilterLevel | grep | 2196 | 605 | 1.000 | 23 | 100% (3/3) | 5.0ms | OK |
| A2 | FilterLevel | rtk_grep | 902 | 288 | 0.476 | 25 | 100% (3/3) | 25.0ms | OK |
| A2 | FilterLevel | rtk_rgai | 691 | 223 | 0.369 | 32 | 100% (3/3) | 10.0ms | OK |
| A2 | FilterLevel | head_n | 2196 | 605 | 1.000 | 23 | 100% (3/3) | 0μs | OK |
| A3 | classify_command | grep | 2524 | 626 | 1.000 | 22 | 100% (2/2) | 7.0ms | OK |
| A3 | classify_command | rtk_grep | 817 | 225 | 0.359 | 20 | 100% (2/2) | 24.0ms | OK |
| A3 | classify_command | rtk_rgai | 782 | 200 | 0.319 | 25 | 100% (2/2) | 11.0ms | OK |
| A3 | classify_command | head_n | 2524 | 626 | 1.000 | 22 | 100% (2/2) | 0μs | OK |
| A4 | package_manager_exec | grep | 918 | 260 | 1.000 | 9 | 100% (5/5) | 7.0ms | OK |
| A4 | package_manager_exec | rtk_grep | 797 | 246 | 0.946 | 21 | 100% (5/5) | 25.0ms | OK |
| A4 | package_manager_exec | rtk_rgai | 1370 | 381 | 1.465 | 44 | 100% (5/5) | 11.0ms | OK |
| A4 | package_manager_exec | head_n | 918 | 260 | 1.000 | 9 | 100% (5/5) | 0μs | OK |
| A5 | strip_ansi | grep | 1852 | 539 | 1.000 | 20 | 100% (5/5) | 8.0ms | OK |
| A5 | strip_ansi | rtk_grep | 1197 | 388 | 0.720 | 33 | 100% (5/5) | 24.0ms | OK |
| A5 | strip_ansi | rtk_rgai | 1264 | 425 | 0.788 | 51 | 100% (5/5) | 10.0ms | OK |
| A5 | strip_ansi | head_n | 1852 | 539 | 1.000 | 20 | 100% (5/5) | 0μs | OK |
| A6 | HISTORY_DAYS | grep | 201 | 61 | 1.000 | 2 | 100% (1/1) | 5.0ms | OK |
| A6 | HISTORY_DAYS | rtk_grep | 182 | 66 | 1.082 | 6 | 100% (1/1) | 24.0ms | OK |
| A6 | HISTORY_DAYS | rtk_rgai | 686 | 208 | 3.410 | 23 | 100% (2/1) | 11.0ms | OK |
| A6 | HISTORY_DAYS | head_n | 201 | 61 | 1.000 | 2 | 100% (1/1) | 0μs | OK |

### Category A: Exact Identifier Search — Summary

- **grep**: | TE min/med/max=1.000/1.000/1.000 | gold hit min/med/max=100%/100%/100% | time min/med/max=5.0ms / 6.5ms / 8.0ms
- **rtk_grep**: | TE min/med/max=0.359/0.698/1.082 | gold hit min/med/max=100%/100%/100% | time min/med/max=22.0ms / 24.5ms / 44.0ms
- **rtk_rgai**: | TE min/med/max=0.287/0.579/3.410 | gold hit min/med/max=60%/100%/100% | time min/med/max=10.0ms / 11.0ms / 13.0ms | LOW_COVERAGE=1
- **head_n**: | TE min/med/max=0.960/1.000/1.000 | gold hit min/med/max=100%/100%/100% | time min/med/max=0μs / 0μs / 0μs

## Category B: Regex Pattern Search

> `rtk rgai` does not support regex; misses are EXPECTED_UNSUPPORTED.

| ID | Query | Tool | Bytes | Tokens | TE | Result Count | Gold Hit | Timing (med) | Status |
|----|-------|------|-------|--------|----|-------------|----------|-------------|--------|
| B1 | fn run\(.*verbose: u8 | grep | 3128 | 999 | 1.000 | 27 | 100% (27/25) | 7.0ms | OK |
| B1 | fn run\(.*verbose: u8 | rtk_grep | 3518 | 1206 | 1.207 | 83 | 100% (27/25) | 25.0ms | OK |
| B1 | fn run\(.*verbose: u8 | rtk_rgai | 3264 | 1065 | 1.066 | 99 | 38% (8/25) | 12.0ms | EXPECTED_UNSUPPORTED |
| B1 | fn run\(.*verbose: u8 | head_n | 3128 | 999 | 1.000 | 27 | 100% (27/25) | 0μs | OK |
| B2 | timer\.track\( | grep | 10764 | 3338 | 1.000 | 116 | 100% (34/30) | 8.0ms | OK |
| B2 | timer\.track\( | rtk_grep | 5723 | 1979 | 0.593 | 158 | 100% (34/30) | 24.0ms | OK |
| B2 | timer\.track\( | rtk_rgai | 2542 | 803 | 0.241 | 93 | 50% (8/30) | 12.0ms | EXPECTED_UNSUPPORTED |
| B2 | timer\.track\( | head_n | 9143 | 2822 | 0.845 | 100 | 100% (32/30) | 0μs | OK |
| B3 | \.unwrap_or\(1\) | grep | 5347 | 1513 | 1.000 | 48 | 100% (20/15) | 7.0ms | OK |
| B3 | \.unwrap_or\(1\) | rtk_grep | 3806 | 1200 | 0.793 | 87 | 100% (20/15) | 24.0ms | OK |
| B3 | \.unwrap_or\(1\) | rtk_rgai | 2777 | 885 | 0.585 | 106 | 50% (8/15) | 11.0ms | EXPECTED_UNSUPPORTED |
| B3 | \.unwrap_or\(1\) | head_n | 5347 | 1513 | 1.000 | 48 | 100% (20/15) | 0μs | OK |
| B4 | #\[cfg\(test\)\] | grep | 2605 | 845 | 1.000 | 41 | 100% (41/35) | 5.0ms | OK |
| B4 | #\[cfg\(test\)\] | rtk_grep | 3098 | 1142 | 1.351 | 125 | 100% (41/35) | 25.0ms | OK |
| B4 | #\[cfg\(test\)\] | rtk_rgai | 2247 | 716 | 0.847 | 101 | 40% (7/35) | 11.0ms | EXPECTED_UNSUPPORTED |
| B4 | #\[cfg\(test\)\] | head_n | 2605 | 845 | 1.000 | 41 | 100% (41/35) | 0μs | OK |
| B5 | HashMap<String, Vec< | grep | 751 | 211 | 1.000 | 6 | 100% (6/6) | 6.0ms | OK |
| B5 | HashMap<String, Vec< | rtk_grep | 797 | 255 | 1.209 | 20 | 100% (6/6) | 24.0ms | OK |
| B5 | HashMap<String, Vec< | rtk_rgai | 2771 | 865 | 4.100 | 96 | 100% (8/6) | 14.0ms | EXPECTED_UNSUPPORTED |
| B5 | HashMap<String, Vec< | head_n | 751 | 211 | 1.000 | 6 | 100% (6/6) | 0μs | OK |
| B6 | lazy_static! | grep | 930 | 280 | 1.000 | 12 | 100% (9/9) | 8.0ms | OK |
| B6 | lazy_static! | rtk_grep | 841 | 292 | 1.043 | 32 | 100% (9/9) | 25.0ms | OK |
| B6 | lazy_static! | rtk_rgai | 2218 | 708 | 2.529 | 72 | 89% (8/9) | 10.0ms | EXPECTED_UNSUPPORTED |
| B6 | lazy_static! | head_n | 930 | 280 | 1.000 | 12 | 100% (9/9) | 0μs | OK |

### Category B: Regex Pattern Search — Summary

- **grep**: | TE min/med/max=1.000/1.000/1.000 | gold hit min/med/max=100%/100%/100% | time min/med/max=5.0ms / 7.0ms / 9.0ms
- **rtk_grep**: | TE min/med/max=0.593/1.125/1.351 | gold hit min/med/max=100%/100%/100% | time min/med/max=23.0ms / 24.5ms / 31.0ms
- **rtk_rgai**: expected unsupported for this category.
- **head_n**: | TE min/med/max=0.845/1.000/1.000 | gold hit min/med/max=100%/100%/100% | time min/med/max=0μs / 0μs / 0μs

## Category C: Semantic Intent Search

> For multi-concept queries, grep exact-substring misses are expected and shown as MISS.

| ID | Query | Tool | Bytes | Tokens | TE | Result Count | Gold Hit | Timing (med) | Status |
|----|-------|------|-------|--------|----|-------------|----------|-------------|--------|
| C1 | token savings tracking database | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 8.0ms | **MISS** |
| C1 | token savings tracking database | rtk_grep | 45 | 12 | MISS | 0 | 0% (0/1) | 24.0ms | **MISS** |
| C1 | token savings tracking database | rtk_rgai | 2801 | 832 | N/A | 102 | 100% (8/1) | 15.0ms | OK |
| C1 | token savings tracking database | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |
| C2 | exit code preservation | grep | 0 | 0 | MISS | 0 | N/A (0/2) | 9.0ms | **MISS** |
| C2 | exit code preservation | rtk_grep | 36 | 11 | MISS | 0 | 0% (0/2) | 25.0ms | **MISS** |
| C2 | exit code preservation | rtk_rgai | 2158 | 702 | N/A | 96 | 80% (8/2) | 12.0ms | OK |
| C2 | exit code preservation | head_n | 0 | 0 | MISS | 0 | N/A (0/2) | 0μs | **MISS** |
| C3 | language aware code filtering | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 8.0ms | **MISS** |
| C3 | language aware code filtering | rtk_grep | 43 | 12 | MISS | 0 | 0% (0/1) | 24.0ms | **MISS** |
| C3 | language aware code filtering | rtk_rgai | 3112 | 926 | N/A | 103 | 100% (8/1) | 14.0ms | OK |
| C3 | language aware code filtering | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |
| C4 | output grouping by file | grep | 0 | 0 | MISS | 0 | N/A (0/2) | 8.0ms | **MISS** |
| C4 | output grouping by file | rtk_grep | 37 | 12 | MISS | 0 | 0% (0/2) | 25.0ms | **MISS** |
| C4 | output grouping by file | rtk_rgai | 3348 | 989 | N/A | 105 | 0% (8/2) | 13.0ms | LOW_COVERAGE |
| C4 | output grouping by file | head_n | 0 | 0 | MISS | 0 | N/A (0/2) | 0μs | **MISS** |
| C5 | three tier parser degradation | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 9.0ms | **MISS** |
| C5 | three tier parser degradation | rtk_grep | 43 | 12 | MISS | 0 | 0% (0/1) | 25.0ms | **MISS** |
| C5 | three tier parser degradation | rtk_rgai | 2453 | 741 | N/A | 95 | 50% (7/1) | 13.0ms | OK |
| C5 | three tier parser degradation | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |
| C6 | ANSI color stripping cleanup | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 6.0ms | **MISS** |
| C6 | ANSI color stripping cleanup | rtk_grep | 42 | 13 | MISS | 0 | 0% (0/1) | 27.0ms | **MISS** |
| C6 | ANSI color stripping cleanup | rtk_rgai | 2139 | 697 | N/A | 92 | 100% (8/1) | 14.0ms | OK |
| C6 | ANSI color stripping cleanup | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |
| C7 | hook installation settings json | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 8.0ms | **MISS** |
| C7 | hook installation settings json | rtk_grep | 45 | 12 | MISS | 0 | 0% (0/1) | 27.0ms | **MISS** |
| C7 | hook installation settings json | rtk_rgai | 2940 | 907 | N/A | 104 | 100% (8/1) | 15.0ms | OK |
| C7 | hook installation settings json | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |
| C8 | command classification discover | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 9.0ms | **MISS** |
| C8 | command classification discover | rtk_grep | 45 | 11 | MISS | 0 | 0% (0/1) | 26.0ms | **MISS** |
| C8 | command classification discover | rtk_rgai | 2867 | 796 | N/A | 104 | 100% (8/1) | 13.0ms | OK |
| C8 | command classification discover | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |
| C9 | pnpm yarn npm auto detection | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 8.0ms | **MISS** |
| C9 | pnpm yarn npm auto detection | rtk_grep | 42 | 14 | MISS | 0 | 0% (0/1) | 27.0ms | **MISS** |
| C9 | pnpm yarn npm auto detection | rtk_rgai | 2682 | 931 | N/A | 104 | 100% (8/1) | 14.0ms | OK |
| C9 | pnpm yarn npm auto detection | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |
| C10 | SQLite retention cleanup policy | grep | 0 | 0 | MISS | 0 | N/A (0/1) | 11.0ms | **MISS** |
| C10 | SQLite retention cleanup policy | rtk_grep | 45 | 12 | MISS | 0 | 0% (0/1) | 25.0ms | **MISS** |
| C10 | SQLite retention cleanup policy | rtk_rgai | 806 | 241 | N/A | 27 | 100% (2/1) | 14.0ms | OK |
| C10 | SQLite retention cleanup policy | head_n | 0 | 0 | MISS | 0 | N/A (0/1) | 0μs | **MISS** |

### Category C: Semantic Intent Search — Summary

- **grep**: | time min/med/max=6.0ms / 8.0ms / 37.0ms | MISS=10
- **rtk_grep**: | gold hit min/med/max=0%/0%/0% | time min/med/max=23.0ms / 25.0ms / 63.0ms | MISS=10
- **rtk_rgai**: | gold hit min/med/max=0%/100%/100% | time min/med/max=12.0ms / 14.0ms / 18.0ms | LOW_COVERAGE=1
- **head_n**: | time min/med/max=0μs / 0μs / 0μs | MISS=10

## Category D: Cross-File Pattern Discovery

| ID | Query | Tool | Bytes | Tokens | TE | Result Count | Gold Hit | Timing (med) | Status |
|----|-------|------|-------|--------|----|-------------|----------|-------------|--------|
| D1 | verbose > 0 | grep | 6540 | 2112 | 1.000 | 90 | 100% (36/30) | 6.0ms | OK |
| D1 | verbose > 0 | rtk_grep | 4307 | 1634 | 0.774 | 162 | 100% (36/30) | 25.0ms | OK |
| D1 | verbose > 0 | rtk_rgai | 2238 | 709 | 0.336 | 97 | 50% (8/30) | 11.0ms | LOW_COVERAGE |
| D1 | verbose > 0 | head_n | 6540 | 2112 | 1.000 | 90 | 100% (36/30) | 0μs | OK |
| D2 | anyhow::Result | grep | 753 | 235 | 1.000 | 11 | 100% (11/11) | 8.0ms | OK |
| D2 | anyhow::Result | rtk_grep | 954 | 333 | 1.417 | 35 | 100% (11/11) | 24.0ms | OK |
| D2 | anyhow::Result | rtk_rgai | 2416 | 765 | 3.255 | 102 | 73% (8/11) | 12.0ms | LOW_COVERAGE |
| D2 | anyhow::Result | head_n | 753 | 235 | 1.000 | 11 | 100% (11/11) | 0μs | OK |
| D3 | process::exit | grep | 5234 | 1474 | 1.000 | 47 | 100% (19/15) | 7.0ms | OK |
| D3 | process::exit | rtk_grep | 3682 | 1154 | 0.783 | 84 | 100% (19/15) | 24.0ms | OK |
| D3 | process::exit | rtk_rgai | 2538 | 804 | 0.545 | 106 | 83% (8/15) | 12.0ms | LOW_COVERAGE |
| D3 | process::exit | head_n | 5234 | 1474 | 1.000 | 47 | 100% (19/15) | 0μs | OK |
| D4 | Command::new | grep | 9867 | 2999 | 1.000 | 111 | 100% (24/20) | 5.0ms | OK |
| D4 | Command::new | rtk_grep | 5321 | 1790 | 0.597 | 145 | 100% (24/20) | 25.0ms | OK |
| D4 | Command::new | rtk_rgai | 2283 | 769 | 0.256 | 102 | 57% (8/20) | 12.0ms | LOW_COVERAGE |
| D4 | Command::new | head_n | 8937 | 2700 | 0.900 | 100 | 100% (23/20) | 0μs | OK |
| D5 | from_utf8_lossy | grep | 17304 | 5038 | 1.000 | 157 | 100% (28/25) | 7.0ms | OK |
| D5 | from_utf8_lossy | rtk_grep | 8386 | 2572 | 0.511 | 168 | 100% (28/25) | 25.0ms | OK |
| D5 | from_utf8_lossy | rtk_rgai | 2767 | 867 | 0.172 | 94 | 29% (8/25) | 11.0ms | LOW_COVERAGE |
| D5 | from_utf8_lossy | head_n | 10775 | 3127 | 0.621 | 100 | 43% (17/25) | 0μs | LOW_COVERAGE |

### Category D: Cross-File Pattern Discovery — Summary

- **grep**: | TE min/med/max=1.000/1.000/1.000 | gold hit min/med/max=100%/100%/100% | time min/med/max=5.0ms / 7.0ms / 8.0ms
- **rtk_grep**: | TE min/med/max=0.511/0.774/1.417 | gold hit min/med/max=100%/100%/100% | time min/med/max=23.0ms / 25.0ms / 27.0ms
- **rtk_rgai**: | TE min/med/max=0.172/0.336/3.255 | gold hit min/med/max=29%/57%/83% | time min/med/max=11.0ms / 12.0ms / 13.0ms | LOW_COVERAGE=5
- **head_n**: | TE min/med/max=0.621/1.000/1.000 | gold hit min/med/max=43%/100%/100% | time min/med/max=0μs / 0μs / 0μs | LOW_COVERAGE=1

## Category E: Edge Cases

> Edge cases are discussed per-case; no category-level winner is inferred.

| ID | Query | Tool | Bytes | Tokens | TE | Result Count | Gold Hit | Timing (med) | Status |
|----|-------|------|-------|--------|----|-------------|----------|-------------|--------|
| E1 | the | grep | 19971 | 5421 | 1.000 | 178 | N/A | 8.0ms | OK |
| E1 | the | rtk_grep | 11273 | 3399 | 0.627 | 239 | N/A | 26.0ms | OK |
| E1 | the | rtk_rgai | 2359 | 779 | 0.144 | 106 | N/A | 10.0ms | OK |
| E1 | the | head_n | 11771 | 3170 | 0.585 | 100 | N/A | 0μs | OK |
| E2 | fn | grep | 77939 | 23141 | 1.000 | 784 | N/A | 7.0ms | OK |
| E2 | fn | rtk_grep | 12744 | 4052 | 0.175 | 264 | N/A | 26.0ms | OK |
| E2 | fn | rtk_rgai | 2733 | 872 | 0.038 | 101 | N/A | 10.0ms | OK |
| E2 | fn | head_n | 10320 | 3103 | 0.134 | 100 | N/A | 0μs | OK |
| E3 | error handling retry backoff | grep | 0 | 0 | N/A | 0 | N/A | 9.0ms | OK |
| E3 | error handling retry backoff | rtk_grep | 42 | 13 | N/A | 0 | N/A | 25.0ms | OK |
| E3 | error handling retry backoff | rtk_rgai | 2340 | 756 | N/A | 102 | N/A | 13.0ms | **UNEXPECTED_HIT** |
| E3 | error handling retry backoff | head_n | 0 | 0 | N/A | 0 | N/A | 0μs | OK |

## Summary: When to Use Which Tool

| Situation | Recommended | Evidence |
|-----------|-------------|----------|
| Exact identifier search (Category A) | rtk_grep | median gold hit=100%, MISS=0, LOW_COVERAGE=0, median TE=0.698 |
| Cross-file pattern discovery (Category D) | rtk_grep | median gold hit=100%, MISS=0, LOW_COVERAGE=0, median TE=0.774 |
| Semantic intent search (Category C) | rtk_rgai | median gold hit=100%, MISS=0, LOW_COVERAGE=1, UNEXPECTED_HIT=0, median TE=N/A |
| Regex patterns (Category B) | grep / rtk grep | `rtk rgai` expected unsupported for regex |
| Exact zero-result validation (E3) | grep / rtk grep | Unexpected hits observed for: rtk_rgai |

## Failure Modes

### grep
- Floods output on broad/common queries.
- Misses semantic intent queries that do not appear as exact substrings.
- No built-in grouping/truncation.

### rtk grep
- Output truncation (`--max 200`) can reduce recall in high-frequency queries.
- Still exact-match based (no semantic expansion).

### rtk rgai
- Regex queries are unsupported by design.
- Can return semantically related content even when strict zero results are expected.
- Quality depends on ranking/model behavior and may vary by environment.

### head_n (negative control)
- Naive truncation may look token-efficient but is relevance-blind.
- Useful as a floor comparator, not as a production recommendation.

## Limitations

- Single codebase benchmark (`src/` Rust files only).
- Gold standards are author-defined and include subjective intent mapping.
- Gold hit is computed from first-run samples; non-deterministic tools may vary across runs.
- Timing is hardware and background-load dependent.
