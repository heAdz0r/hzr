# HZR LLM utility contract

This benchmark verifies the machine-checkable part of HZR's agent-facing value:
self-describing bounded reads, exact recovery, and safe single/batch writes. It does
not claim that a particular model understood an answer or completed a task correctly.

The checks are deterministic and API-free:

| Gate | Verified contract |
|---|---|
| Markdown overview | identifies omitted content, shows bounded lead prose, reports source/section coverage, and gives exact recovery paths |
| Exact read | `--level none` is byte-for-byte identical to the fixture |
| Focused read | `--from`/`--to` returns the requested exact line range |
| Single write | `create`, `replace`, `patch`, and structured `set` produce the expected files |
| Idempotency | repeating an identical create is a no-op and preserves content plus modification time |
| Batch write | ordered same-file operations and cross-file operations are applied in one process |
| Batch dry-run | reports the plan without changing the file |
| Structured feedback | write responses use JSON schema version 1 |
| Differentiation | recorded upstream RTK `v0.44.1` help has no `write`; HZR fork-core does |

Batch writes are atomic per file, not a transaction spanning every file in a plan.
Failures are reported per operation and successful independent file groups can proceed.

## Reproduce

Build fork-core, then run the verifier:

```bash
cargo build --locked --manifest-path fork-core/rtk/Cargo.toml --bin rtk
python3 benchmarks/hzr-llm-utility-v0.3.1/verify.py
```

Use `--rtk-bin PATH`, `--upstream-help PATH`, or `--run-id ID` to override the
defaults. The verifier refuses to overwrite an existing run. Evidence contains
the fixture, captured stdout/stderr, JSON summary, human-readable table, binary
identity, and SHA-256 manifest.

Verify the recorded evidence from its run directory:

```bash
(cd benchmarks/hzr-llm-utility-v0.3.1/runs/2026-08-01 && shasum -a 256 -c checksums.sha256)
```

## Recorded result

The canonical [`runs/2026-08-01/RESULTS.md`](runs/2026-08-01/RESULTS.md) records
all gates passing for HZR `0.3.1` fork-core `0.44.1-fork.1`.
