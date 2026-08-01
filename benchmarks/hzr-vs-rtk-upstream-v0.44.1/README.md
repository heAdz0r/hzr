# RAW vs upstream RTK v0.44.1 vs HZR

This directory contains the reproducible RAW / upstream RTK / HZR command-output
benchmark, its methodology and recorded evidence.

The benchmark compares three paths on the same immutable upstream fixture:

1. the native command (`RAW`);
2. upstream `rtk-ai/rtk` at commit
   `36591fb00d650bf987b57483c0b3a395a35a8dc1` (`v0.44.1`);
3. the current HZR CLI routing to the current `fork-core/rtk` build (`HZR`).

Each case runs five times with rotating participant order. Participants receive
separate empty `HOME`, XDG and tracking paths, but share the fixture, target and
one isolated Cargo cache prewarmed by the three source builds. Tee and telemetry
are disabled. Output tokens are estimated as
`ceil(UTF-8 bytes / 4)`; this is deliberately labelled as an estimate and is
not a provider tokenizer or a billing measurement.

## Reproduce

Requirements: Git, Rust/Cargo, Python 3 and network access to clone the pinned
upstream repository.

```bash
benchmarks/hzr-vs-rtk-upstream-v0.44.1/run.sh
```

Optional controls:

```bash
HZR_BENCHMARK_REPETITIONS=5 \
HZR_BENCHMARK_RUN_ID=my-run \
benchmarks/hzr-vs-rtk-upstream-v0.44.1/run.sh
```

The runner uses a blob-filtered, no-checkout clone, checks out the exact pinned
commit, builds all three compared executables from source, writes evidence under
`runs/<run-id>/`, and deletes its temporary upstream checkout on exit.

## Evidence layout

- `summary.json`: machine-readable rows and every repetition's latency, exit
  code, byte count, estimated token count and output SHA-256;
- `summary.csv`: compact comparison table;
- `environment.json`: safe allowlisted build identity, versions and binary
  hashes (never a full environment dump);
- `outputs/<case>/{raw,upstream,hzr}.txt`: canonical full output for inspection;
- `upstream-help.txt` and `hzr-fork-help.txt`: command-surface proof;
- `checksums.sha256`: digest of every evidence file in the run.

Verify a recorded run from this directory:

```bash
(cd runs/2026-08-01-v2 && shasum -a 256 -c checksums.sha256)
```

The benchmark measures command-output size and local wall-clock latency. It
does not measure answer fidelity, total agent-session tokens, provider-billed
tokens, cost, or accepted-task quality.

## Recorded result

The canonical `2026-08-01-v2` run contains 14 cases × 5 repetitions and the
self-describing Markdown digest contract:

| Aggregate | RAW | Upstream RTK | HZR | HZR vs RAW | HZR vs upstream |
|---|---:|---:|---:|---:|---:|
| All cases | 287,152 | 58,108 | **44,307** | **−84.6%** | **−23.8%** |

HZR won 8 measured cases and tied upstream on 6; after remediation there were
no measured HZR losses. See [`runs/2026-08-01-v2/RESULTS.md`](runs/2026-08-01-v2/RESULTS.md)
for the full table and [`runs/2026-08-01-v2/outputs/`](runs/2026-08-01-v2/outputs) for
the captured proof.
