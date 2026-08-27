# HZR 0.6.6

HZR 0.6.6 makes the orchestrator's accounting and lifecycle decisions single-owned and
release-gated. It also ships the approved bundle-build workstream.

## Accounting and stats

- Fork-core emits invocation-scoped, privacy-safe accounting receipts with exact engine identity,
  correlation and sequence. HZR commits them idempotently through the daemon ledger writer and
  acknowledges their journals only after the durable write.
- Producer failures remain fail-open for command execution but open a durable accounting gap.
  Live status becomes degraded; historical incompleteness remains visible after recovery.
- Bypassed and unmeasured receipts cannot carry positive savings credit.
- Stats aggregate on one typed public key before sorting or limiting. Private commands that share
  a public family/mode/route/stage now form one row instead of duplicate-looking labels.
- MCP delivery stages, search attribution, session precedence and daemon-down evasion attribution
  use shared typed owners.

## Lifecycle and local instructions

- Shared instruction files remain the default.
- Local instruction mode uses repository-local ignored surfaces and never modifies shared
  `AGENTS.md` or `CLAUDE.md` for HZR users who opted into local scope.
- Scope changes, local excludes, activation state, hooks, project MCP registration and uninstall
  run as one serialized desired-state transaction with rollback and no-follow writes.
- Hook dispatch, observation, feedback and statusline use the workspace carried by hook input, so
  one host session cannot leak HZR state into a disabled workspace.

## Bundle and platform contract

Intel macOS (`darwin-x64`) is no longer supported. HZR 0.6.6 publishes exactly three native
platform archives:

- `linux-x64`;
- `linux-arm64`;
- `darwin-arm64`.

Component builds are cached by their semantic inputs and verified by version, license and hashes
before reuse. On the measured machine, the reusable component stage changed from 372.92 seconds
cold to 16.27 seconds warm (22.9x). The measured macOS arm64 bundle is 354204 KiB and the archive
is 101056087 bytes.

The bundled Node runtime contains the executable and license only. Caveman production packages
omit optional dependencies. Independent preflight branches run in parallel, CI warms the same
verified cache, and the reduced smoke matrix retains OS-specific and per-archive proof.

## Release safety

CI and release publishing invoke the same complete source gate. Native bundle and publish jobs
cannot run after an ordinary test, Clippy, fork provenance or source-gate failure.
