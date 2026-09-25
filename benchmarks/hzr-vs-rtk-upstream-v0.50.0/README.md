# RAW vs HZR 0.9.13 vs HZR 0.10.0 vs upstream RTK v0.50.0

Delivered command output for the cases an external evaluation measured on a Go repository
(2026-09-25), run against this repository and a small red Go module (one compile failure, one
failing test). Go 1.25.1, golangci-lint 2.8.0, macOS arm64. Sizes are UTF-8 bytes of
stdout+stderr; tokens are not a provider tokenizer.

Two totals are reported because they answer different questions:

- **raw bytes** — what the command printed;
- **host-visible** — each output capped at 30,000 characters, what Claude Code shows the model
  by default (`BASH_MAX_OUTPUT_LENGTH`). Savings above that window never reach the context, so
  this is the honest figure.

| Total over 16 cases | RAW | HZR 0.9.13 | HZR 0.10.0 | upstream v0.50.0 |
|---|---:|---:|---:|---:|
| raw bytes | 991,678 | 966,230 | 86,748 | 120,953 |
| host-visible | 139,256 | 108,006 | **86,748** | 104,644 |

Host-visible, HZR 0.10.0 delivers 38% less than RAW and 17% less than upstream RTK v0.50.0;
HZR 0.9.13 delivered 22% less than RAW. Per-case figures are in
[`runs/2026-09-25/RESULTS.txt`](runs/2026-09-25/RESULTS.txt).

What changed the result, case by case:

- `go test` with a compile failure: 3,451 → 318 bytes (0.9.13 printed the NDJSON it had injected).
- `git diff HEAD~1` / `HEAD~5`: 0.9.13 printed more than RAW (its guard fell back to the complete
  diff); 0.10.0 keeps whole hunks and fits the 30,000-character window with a recovery line.
- `find … -name '*.rs'`: 6,139 → 965 bytes.
- `grep -rn`: 10,355 → 8,264 bytes, lossless (`rg --heading` shape).
- `cat -n`: 16,810 → 14,974 bytes, smaller than RAW's 16,351 (tab separator).
- `git status` grew from 382 to 1,462 bytes on purpose: 0.9.13 hid staged files past five and
  untracked files past three; 0.10.0 prints every porcelain line.

Reproduce: build the current `fork-core/rtk`, keep the previous release's
`engines/rtk`, build upstream `rtk-ai/rtk` at `v0.50.0`, then

```bash
HZR_BENCH_ROOT=/tmp/hzr-bench HZR_OLD_RTK=… HZR_NEW_RTK=… HZR_UPSTREAM_RTK=… \
  benchmarks/hzr-vs-rtk-upstream-v0.50.0/bench.sh /tmp/hzr-bench/out
```

The case list includes `git diff HEAD~1`, so absolute numbers depend on the checkout.
