# Lifecycle implementation review — 2026-09-04

Scope: W08 / INT2–4, following the approved independent review. Changes are source changes; the installed daemon, real ICM processes, user configuration and release artifacts were not changed.

## Result

HZR handles SIGINT and SIGTERM through one shared, synchronously installed signal future. Both `hzrd` and `hzr daemon serve` use it. Server shutdown cancels execution jobs before stopping memory and index engines.

An owned ICM process that remains alive but fails readiness no longer loops forever without recovery. The daemon requires three consecutive failed health polls, rechecks readiness under the lifecycle lock, terminates and reaps only its owned child, and retries under the existing bounded exponential backoff. A healthy poll resets the failure count. Attached processes are never stopped or restarted by this path.

A private atomic `memory/icm/runtime/service.json` records the database, executable path and SHA-256, endpoint, PID and process start identity. The endpoint is restored before constructing the client. A new supervisor reattaches only after the identity, executable and authenticated readiness checks pass. PID reuse or a dead recorded process permits a fresh start without signalling the stale PID. A pending launch, live unverified legacy PID or live process with an unavailable endpoint prevents a second writer.

The pending manifest is durable before spawn; identity is committed before readiness. Normal shutdown removes it after the child is reaped. Dropping an owner keeps the record until a later supervisor proves the process has exited. Runtime records reject malformed, oversized and symlink inputs. Legacy PID reads are likewise bounded and reject symlinks; they fence startup but never authorize attachment or signalling.

Foreign-process diagnostics recognize options before the engine subcommand, distinguish wrappers, and redact arguments. Managed classification requires a daemon parent plus the durable ICM identity, or the legacy exact PID/database evidence for installed older engines. grepai requires the expected canonical workspace runtime and a matching readiness PID marker. A data-root substring is no longer evidence of ownership.

## Canonical files

- `crates/hzr-memory/src/runtime.rs`: durable launch state and process identity.
- `crates/hzr-memory/src/supervisor.rs`: attachment, duplicate-writer fences, owner-only recovery and cleanup ordering.
- `crates/hzr-memory/src/error.rs`, `installation.rs`, `lib.rs`: typed ownership failure, reuse of the existing hash implementation, and read-only diagnostic export.
- `crates/hzr-memory/src/supervisor_tests.rs`, `tests/icm_integration.rs`: isolated process/identity regressions.
- `crates/hzr-daemon/src/shutdown.rs`, `lib.rs`, `main.rs`; `crates/hzr-cli/src/main.rs` daemon-serve arm: shared signal handling.
- `crates/hzr-daemon/src/state.rs`: bounded unhealthy-child recovery and execution-job store initialization coordinated with the execution implementation.
- `crates/hzr-daemon/src/server.rs`: execution-job shutdown before engine teardown.
- `crates/hzr-daemon/tests/shutdown_signal.rs`: actual isolated daemon SIGTERM and lock-release test.
- `crates/hzr-cli/src/foreign.rs`: structured diagnostic matching and redaction.

There are no generated distribution mirrors for these changes. The exact-memory lookup changes in `snapshot.rs`, `tests/exact_get.rs`, and their exports belong to the retrieval workstream.

## Verification recorded before centralized gate

| Command or check | Observed result |
|---|---|
| `hzr rtk -- test cargo test --locked -p hzr-memory --all-targets --all-features` | 36 passed: 20 unit, 1 exact lookup, 12 ICM integration, 3 snapshot. This run preceded the final legacy-PID regression. |
| `hzr rtk -- test cargo clippy --locked -p hzr-memory --all-targets --all-features -- -D warnings` | Passed before the final legacy-PID changes. |
| `cargo test --locked -p hzr-daemon state::tests::memory_supervision -- --nocapture`, managed shell with complete log redirected | Both lifecycle tests passed in 3.07s. Compilation/lock waiting consumed 24.89s; the shell wrapper later timed out during unrelated zero-match integration discovery. Overall command is not reported as passed. Evidence: `/tmp/hzr-lifecycle-daemon-state.log`. |
| `hzr rtk -- test cargo test --locked -p hzr-daemon --test shutdown_signal -- --nocapture` | 1 passed in 1.06s. The isolated child exited successfully after SIGTERM and released its daemon lock. |
| Targeted CLI foreign-process tests | 5 passed before the final legacy exact-PID/database/parent regression was added. |
| Final scoped `rustfmt --edition 2024` and `git diff --check` | Passed. |

One earlier memory run under concurrent compilation had integration failures; the subsequent full memory run above passed. An earlier broad daemon run had 77 passed, 8 failed and 1 ignored, including the two lifecycle tests that subsequently passed in isolation. Concurrent Cargo contention is observed; it is not a proven explanation for every failure. The broad run must be repeated centrally.

Daemon/CLI clippy identified three `panic!` calls in the concurrently implemented execution-job tests and a new CLI call treating `shutdown_signal()` as a Future without unwrapping its Result. Exact diagnostics were sent to the owning agent and saved in `/tmp/hzr-lifecycle-clippy.log`.

To eliminate concurrent Cargo contention, additional compilation was stopped at the coordinator's request. The final bounded legacy-PID regression and the sixth foreign-process regression require the centralized full workspace gate. No release readiness is claimed here.

## Explicit limits

- Linux identity uses boot ID plus process start ticks. macOS uses process start time reported by `ps`, which has second-level precision. Authenticated endpoint and executable checks are additionally required; macOS identity is not claimed to be a nanosecond-precision PID reuse proof.
- Non-Unix orphan identity inspection remains unsupported and fails closed. A fresh owned process can still run; an uncertain orphan cannot be adopted. Linux/macOS are the verified implementation targets; only macOS runtime tests ran in this session.
- A crash between durable pending-state creation and child identity commit intentionally requires operator reconciliation. Automatically deleting that fence could create a second database writer. This is a safe refusal, not automatic crash recovery in that narrow window.
- A verified surviving orphan is Attached: it is reused without a duplicate process but cannot be signalled as though the new daemon owned its child handle.
- Legacy diagnostic evidence is less strong than the new durable launch identity. It is used only for classification of existing installed engines, never to authorize process control.
- Process enumeration is diagnostic. It cannot establish complete host usage, provider-billed token savings or successful production rollout.
