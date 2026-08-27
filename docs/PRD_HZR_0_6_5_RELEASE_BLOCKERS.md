# PRD — HZR 0.6.5 release blockers

Status: implemented; local release gates passed; publication pending. Target: 0.6.5.

## 0. Why 0.6.5 exists

0.6.4 made a zero explain its scope, but it did not fix the current producer mismatch that
created the misleading zero. It also treated grepai's initial-index ready marker as a process
startup marker and could kill a healthy large-repository scan after 120 seconds. Both public
GitHub issues are release blockers, not documentation follow-ups.

This release also closes the in-session visibility gap for fail-open accounting and makes HZR
instruction placement configurable without changing the existing default.

## R1 — current fork measurements remain visible

- fork-core writes `privacy_typed_v2` for every new typed row.
- Default aggregate stats accept `privacy_typed_v2` plus aggregate-compatible typed v1 rows.
  This does not relabel v1 session identities as v2 and does not weaken keyed session queries.
- Older/incompatible versions remain excluded and recoverable with
  `--accounting-version all`.

Acceptance A1 reproduces GitHub issue #2 exactly: 435 `rtk/0.44.1-fork.1` v1 rows must produce
44,189 net avoided tokens; its 64 read rows must produce 27,334 transform-avoided tokens. Zero is
forbidden for this fixture.

## R2 — a long initial scan is not a dead watcher

- Watch startup proves that the child remains alive through a bounded stability window.
- The `.ready` marker remains typed readiness evidence, but its absence during a first scan does
  not kill the process.
- Immediate child exit still fails startup. Idle eviction remains independently governed by the
  configured 900-second default TTL.
- Status reports the actual ready-marker state instead of hard-coding `true` for a live child.

Acceptance A2 reproduces GitHub issue #1 with a live watcher that intentionally never emits a
ready marker before the old startup deadline. `prepare` must succeed, the watcher must remain
live, readiness must be false, and the idle TTL must still be 900,000 ms.

## R3 — machine-local instruction placement

Configuration:

```toml
[instructions]
scope = "local"
```

`shared` remains the default and preserves 0.6.4 behavior. `hzr init` and `hzr install` accept
`--instruction-scope shared|local` and persist the selection. Doctor, SessionStart reconciliation,
enable/disable, hooks status, uninstall, and fleet reconciliation use the configured scope.

Local scope writes `CLAUDE.local.md` and `AGENTS.override.md`. It never writes the repository's
`CLAUDE.md`, `AGENTS.md`, or shared `.gitignore`. The two local files are registered in
`.git/info/exclude`. Because Codex gives `AGENTS.override.md` precedence, HZR's local bridge
explicitly requires reading the repository's `AGENTS.md` before work.

Acceptance A3 initializes a Git repository containing sentinel shared instruction files, asserts
they remain byte-identical, asserts both local files are installed and ignored, and asserts the
Codex bridge preserves the shared team contract.

## R4 — live fail-open tells when evidence was lost

- The first degraded rewrite records the gap start; later degraded calls cannot move it.
- Session state records degraded operation count, accumulated duration, and open-gap start.
- One transition notice is emitted on degradation and one on recovery; the persistent status-line
  signal remains `ACCOUNTING: DEGRADED` while the gap is open.
- Session savings remain unknown and partial totals remain withheld after any degraded interval.
- The scorecard states missing seconds, affected operations, and share of observed session time.
- `hzr stats --evasion` states the open gap start and duration, distinguishing a recent outage
  from an almost entirely untracked session.
- No retry, restart, or hzrd supervision behavior is added.

Acceptance A4 uses deterministic times 1000, 1060, 1120, and 1720. It asserts first flip 1000,
last degraded call 1060, a 720-second open global gap, a 120-second recovered session interval,
and two affected session operations.

## Release gate

All of the following must pass before tag or publication:

1. A1–A4 targeted acceptance tests.
2. `cargo fmt --all --check`.
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
4. `cargo test --workspace --all-targets --all-features`.
5. `./scripts/refresh-current-engine.sh` and `./scripts/verify-fork-core.sh --test`.
6. Repository verification, bundle/install smoke, and release preflight.
7. Published tag target, CI jobs, archives, checksums, attestations, installed binary version,
   doctor, and live stats are verified independently.
