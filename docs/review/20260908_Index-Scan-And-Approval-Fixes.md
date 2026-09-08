# Index scan and approval fixes

## Issue #3: unreadable index subtrees

The duplicate-index walker treated every traversal error as fatal. A registered home
workspace could therefore hit a macOS TCC boundary and abort initial discovery before
doctor produced a report.

Permission-denied traversal errors now preserve their paths and allow discovery to
continue. Workspace and index-status JSON expose `unreadable_index_paths`; doctor and
fleet output warn that the audit is incomplete. Other I/O failures remain errors.
Media libraries and application bundles are pruned alongside existing build/vendor
directories. Directory symlinks are still not followed.

Unreadable does not mean index-free. Legacy migration refuses an incomplete audit
before writing anything. Canonical index use and instruction reconciliation can
continue; known active duplicate writers retain their existing exclusion gate.
This patch does not add scan-exclusion configuration or change home registration.

## Claude Auto mode: missing permission rule

The typed fork planner collapsed `PermissionVerdict::Default` (no matching rule)
and `PermissionVerdict::Ask` (an explicit ask rule) into the same mandatory
`RewritePlanDecision::Ask`. Thus an ordinary `hzr exec run 'ls'` could request a
second approval even after the host allowed its Bash invocation.

The managed typed planner now selects the existing optimized route for a default
verdict. Explicit ask/deny rules and canonical-policy ambiguity retain their
previous behavior. Claude's hook adapter still strips optimization-only
`permissionDecision: allow`, so normal host checks, including Auto mode, remain
in charge. No Auto-mode execution grant, wildcard permission, or bypass setting
is introduced.

## Fork parity delta

Only `src/rewrite_cmd.rs::run_plan` changes execution-planning behavior.
The inherited legacy `rewrite` command keeps its default exit code 3 and proposed
command. Filters, output bytes, explicit permission precedence, shell ambiguity,
and fidelity policy are unchanged. `tests/w7_routes.rs` covers the typed default,
explicit ask/deny, and unchanged legacy default. Current-engine manifests are
regenerated; the immutable import baseline is unchanged.

## Validation scope

Index integration fixtures cover unreadable directories, readable duplicates,
media/application bundles, and refusal to migrate after an incomplete audit.
Mode-000 coverage exercises permission denial on non-root Unix; root runners
instead assert the visible indexes. These fixtures do not simulate macOS TCC
grants or prove a live Claude Auto-mode session.

Verified on 2026-09-08:

- `cargo test --locked --workspace --all-targets --all-features`: 957 passed,
  0 failed, 2 ignored across 40 suites. After the final doctor-deferral change,
  `cargo test --locked -p hzr-cli --bin hzr --all-features`: 392 passed,
  0 failed, 1 ignored.
- `scripts/verify-fork-core.sh --test`: 1964 passed, 0 failed, 1 ignored;
  immutable baseline and refreshed current-engine identity verified.
  The inherited Clippy warning ratchet passed with 141 baseline warnings.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`:
  passed on the final source.
- `cargo fmt --all --check`, `git diff --check`, and shell syntax checks: passed.
- `python3 scripts/verify-release-gates.py --self-test`: 4 passed.
- An isolated CLI hook smoke with the rebuilt engine and
  `permission_mode=auto` passed three cases: no rule emits a managed rewrite
  without a permission decision/grant; explicit ask emits ask; explicit deny
  emits deny. The timeout input field was preserved.

The fork regression tests first failed for the old typed default-Ask behavior,
then passed after the planner patch. An initial fork identity check was started
before manifest refresh completed and rejected the stale checksums; the complete
gate above was rerun successfully after refresh.

At the implementation handoff, no real agent configuration was changed, and no release, tag, or installation was
performed. Bundle/platform builds and a live Claude user session were not run.
The optional configurable scan exclusions and home-registration redesign from
the issue are outside this focused fix.
