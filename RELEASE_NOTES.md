# HZR 0.6.0 — one control plane, end to end

0.6.0 is the architectural release where HZR stops behaving like a collection of adjacent
optimizers and becomes one owned system. RTK fork-core, grepai, ICM, the Caveman-derived agent
loop, policy, accounting, doctor, MCP, and the local UI now share project identity, lifecycle,
typed contracts, and failure semantics.

## What changes for users

- Agent shell commands go through one canonical policy route. A managed filter cannot be bypassed
  by choosing a direct RAW wrapper; legitimate exact recovery is explicit, budgeted, and receives
  zero savings credit.
- One grepai index belongs to one worktree, and one HZR-supervised ICM process owns durable memory.
  Failed watchers, restarts, stale caches, and project mismatches are visible instead of silently
  producing empty or cross-project answers.
- The local observatory connects service topology, memory graph, grepai health, routes, latency,
  traces, and accounting posture. Its public loopback view is pseudonymized and content-redacted.
- MCP is now a real typed capability surface: 13 tools use one schema/dispatch contract and route
  confined reads, writes, execution, memory, observability, and doctor through `hzrd`.
- `init`, `install`, and `doctor` converge the desired state transactionally across workspaces,
  including project-scoped Codex configuration and forward recovery after interrupted adoption.

## Fidelity and accounting

Exact-output allowance is durably reserved before execution. The state machine is
`Reserved → Executing → Executed`; an error after spawn becomes an operator-visible unknown
execution and is not automatically retried or refunded. Completed records replay idempotently
after restart, corrupt records are quarantined, and new fidelity work fails closed while the
ledger cannot establish a safe state.

Actual provider receipts remain separate from token estimates. Native, degraded, excluded, and
unmeasured operations affect coverage instead of becoming savings. 0.6.0 still has no paired
provider-billed benchmark, so `economic_claim_ready` remains false.

## Architecture and documentation

The README is shorter and rebuilt around the ownership architecture and what it provides. It
includes a native Mermaid system diagram, the pinned command-output evidence with its limits, a
one-minute workflow, the 13-tool MCP boundary, and an explicit note that HZR is built by an RTK
contributor with hands-on experience of the techniques being evolved.

Historical PRDs that retain unique normative evidence are marked superseded and mapped to their
current owners. Stale false-ready reviews, unsupported projections, obsolete screenshots, and
runtime backup/lock residue were removed.

## Compatibility

The release continues to ship native self-contained bundles for Linux x86_64/ARM64 and macOS
Apple Silicon/Intel. Windows is not published. `hzr build` remains inherited fork maintenance
compatibility; project builds use `hzr exec run '<project build command>'`.

## Verification status

Targeted compile and implementation gates passed during development, including real MCP
stdio-to-daemon confined I/O and the durable post-spawn recovery path. At the user's explicit
release priority, repeated full-suite, browser, fleet, and immutable-SHA acceptance runs were
deferred rather than presented as completed. The exact mandatory follow-up list is
[`docs/TEST_DEBT_0.6.0.md`](docs/TEST_DEBT_0.6.0.md).

This release must therefore be described as **built with deferred full verification**, not as a
fully accepted or economically proven release.
