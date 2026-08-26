# HZR 0.6.1 verification debt

Supersedes [`TEST_DEBT_0.6.0.md`](TEST_DEBT_0.6.0.md). That list described gates that had not
been run. This list describes what happened when they were, and what is still owed.

Status vocabulary is deliberately narrow. **Closed** means a gate runs and passes, or a
mechanism exists and is exercised by a test. **Partial** means part of the acceptance is met and
the rest is named below it. **Open** means not started. Nothing here is marked closed on the
strength of an argument alone.

## P0 — trustworthiness and absence of bypass

### 1. Full immutable-SHA regression gate — closed

Run against the immutable 0.6.0 tree at `27440ae` in a detached worktree, then re-run on the
fix. The result on `27440ae` is the finding, not a pass:

| Gate | `27440ae` | 0.6.1 |
|---|---|---|
| `cargo fmt --all --check` | pass | pass |
| workspace clippy, `-D warnings` | **fail** (3 lints) | pass |
| full locked workspace test | **fail** (2 tests + 1 load-flake) | pass |
| `cargo +1.85.0 check --locked` | **fail** (20 × E0658) | pass |
| fork-core parity, digest, clippy ratchet, tests | **fail** (2 tests) | pass |
| Caveman bridge test / npm audit / syntax | **fail** (contract mismatch) | pass |
| visualizer test / typecheck / build | pass | pass |

The MSRV run needs an explicitly pinned compiler. `rustup run 1.85.0 cargo check` is **not**
sufficient on a host with another `rustc` earlier in `PATH`: it silently compiles with that
compiler and reports a false pass. Use
`RUSTC=$(rustup which --toolchain 1.85.0 rustc) cargo check --locked …`.

### 2. Fleet anti-bypass reconciliation — partial

Closed: the hardcoded path exemption is gone, replaced by a declared `.hzr/policy.toml` waiver
that names one rule, requires an auditable justification, refuses to waive any execution route,
and is reported by `hzr doctor` under `fleet_instruction_exemptions` instead of passing
silently. Waivers are declared for the two repositories whose measured subject is RTK itself.
The user-global direct-ICM directive is routed through `hzr memory store`. MCP ownership is
workspace-aware (item 10), so the Claude Code pin no longer conflicts across projects.

Open: 158 registered instruction files across roughly 79 workspaces still carry a stale managed
block. Each needs `hzr init --if-needed` in its own workspace, and that should follow the 0.6.1
install so the refreshed block is the current contract rather than one that goes stale again.
Sequencing this behind item 8 is deliberate: reconciling 79 working trees before `init` reports
its project-local writes would create exactly the pollution item 8 is about.

### 3. Completeness of filtered output — partial

Closed: a structured protocol falls back to exact bytes when the filtered rendering is not the
same protocol carrying the same payload, and git porcelain is recognized without being
left-trimmed first. The guard now distinguishes an automatic filter, where the machine-protocol
fallback applies, from a view the caller explicitly requested (`rtk json`, `rtk read` digests),
where it does not — that distinction is what stopped every digest from rendering.

Open: a per-command-family completeness validator, differential RAW/filtered fixtures across
those families, and a rule that denies savings credit to any result proven incomplete.

### 4. Observation of host-native operations — open

`hzr doctor` already reports both global-codec paths as permanently unintercepted, so that
particular gap is visible. `hzr stats` still does not separate observed, unobserved, and
provably-absent traffic, so `observed_scope_complete=true` continues to mean "complete for the
channels we can see" without saying how large the unseen set is.

## P1 — operational reliability

### 5. Fidelity fault-injection corpus — partial

The durable state machine, post-spawn unknown execution, pre-spawn recovery and the public
allowance boundaries are covered by existing tests, and the operator remediation wording that
had drifted is restored and asserted. A deliberate fault-injection corpus — crash before spawn,
crash after spawn, restart replay, cleanup failure, corrupt-record quarantine, and both
reconciliation outcomes driven as injected faults rather than unit fixtures — does not exist.

### 6. Full MCP end-to-end — partial

Closed: stdio-to-daemon confined read and write, project confinement including escape attempts,
and exactly-once ledger accounting — which is where 0.6.0's largest accounting defect was found.

Open: all thirteen tools driven end to end, schema/handler parity and budget under load,
cancellation, and the daemon-down and malformed-response paths.

### 7. Multi-day engine soak — not run

Excluded from this release by explicit decision. It needs days of wall-clock time.

### 8. `init` must not pollute a repository — partial

`hzr init --dry-run` already prints every project-local mutation with before/after SHA-256 and
the backup path, which satisfies "show the required changes in advance".

Open: `--dry-run` is rejected when combined with `--if-needed`, which is precisely the
combination fleet reconciliation needs; and a non-dry-run `init` does not summarise the
project-local files it wrote.

### 9. Unambiguous self-restart installer — open

Installing through a running daemon still loses the response at the restart boundary even when
the bundle installed correctly. The client should receive a final receipt after reconnect
instead of an ambiguous `daemon request failed`.

### 10. Workspace-aware MCP ownership — closed

HZR audited only Claude Code's user-global `mcpServers`, ignoring the per-project
`projects[<path>].mcpServers` scope that Claude Code actually launches. One global pin therefore
made every other project permanently mismatched. The project scope now takes precedence, exactly
as it already did for Codex, and a regression test covers it.

## P2 — UI, efficiency, and economic proof

### 11. Browser A/B/C acceptance — closed

Run against the current visualizer source on `127.0.0.1:47392`, proxied to the live daemon, with
three registered projects and `fetch` instrumented to record aborts.

| Property | Result |
|---|---|
| fast project switching | three back-to-back switches; final state is always the last selection |
| aborting superseded requests | in-flight request for the superseded project aborted in 1–8 ms; superseded observability polls aborted too |
| generation barrier | a single rendered transition straight to the final project; no earlier project's response was ever applied |
| no stale memory / index / trace data | the ledger panel's project identity always matched the selected card; switching from a project with 176 ICM memories to one with none reset the panel to 0 rather than carrying the previous project's numbers |

Switching also coalesces: a selection superseded within the same tick never issues its request
at all, so the barrier holds without a doomed round trip.

### 12. Idle-state UI semantics — closed

Reproduced first: with nothing selected and the daemon, fork-core and ICM all `Ready`, the
posture chip read `Rebuilding`. The cause was `dashboard_overall_state` scoring every registered
workspace, so any project that had never been indexed — 75 of 82 here — counted as a rebuild in
progress forever.

Posture now describes this control plane and the project in view. Fleet progress keeps its own
separate reading in `projects_index_ready / projects_total`. With no project selected the state
is `standby`, and the chip says "No project selected" rather than leaving `Standby` to mean two
different things.

Remaining: `warming` and `rebuilding` still share one wire state. Splitting them needs a new
`DashboardState` variant, which is a protocol change rather than a hotfix.

### 13. UI performance budget — open

Lazy-loaded Cytoscape, cold-load profiling, a large memory graph, bounded DOM and data growth,
and mobile layout.

### 14. Provider-billed benchmark — not run

Excluded from this release by explicit decision. It requires identical real tasks across
RAW/RTK/HZR with provider input/output receipts, result quality and retries, latency and cost,
over several models and repetitions.

**Until it exists, `economic_claim_ready` stays `false`.** HZR reports estimated local reduction
and must never present it as billed savings.
