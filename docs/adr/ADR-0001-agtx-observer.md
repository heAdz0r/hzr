# ADR-0001: Observe agtx through a pinned, adapted, read-only helper

- Status: accepted
- Date: 2026-09-07
- Applies to: HZR 0.8.7
- Supersedes: nothing
- Source PRD: [`docs/PRD_HZR_AGTX_AGENT_OBSERVATORY.md`](../PRD_HZR_AGTX_AGENT_OBSERVATORY.md)

## Context

HZR should be able to show what a fleet of coding agents is doing, what it
costs, and how much of that traffic HZR itself removed. agtx already models
exactly the thing worth showing — a board of tasks, their dependencies, the
sessions attached to them and an agent-reported lifecycle — and it stores that
model in SQLite files on the same machine.

Four options were on the table.

1. **Build an HZR-native board.** Rejected: it would be an unrelated board
   wearing the integration's name. The tasks users care about are the ones agtx
   already has.
2. **Depend on the agtx library from the HZR daemon.** Rejected: HZR is on
   `rusqlite 0.36` and the pinned agtx on `0.34`, so one process would link two
   SQLite bindings with conflicting native-link expectations. It would also pull
   agtx's TUI, MCP server and `serve` routes into the daemon — all of which can
   move a task, start an agent with `--dangerously-skip-permissions` or answer a
   permission prompt — and would put upstream's toolchain floor in front of
   HZR's 1.85 MSRV.
3. **Read the SQLite files directly from HZR.** Rejected: it works right up
   until upstream changes the path-hash scheme, the status vocabulary or the
   `referenced_tasks` separator, at which point HZR silently reads the wrong
   file or the wrong meaning. The layout is upstream's to define.
4. **A separate, pinned, adapted read-only executable.** Chosen.

## Decision

Ship `hzr-agtx-observer`: the pinned agtx source at
`d307c4c182dff19a65370a50403185cb826f7f49` plus one auditable patch, built
outside the HZR Cargo workspace, invoked by argv with one bounded JSON request
on stdin and exactly one JSON object on stdout.

The patch adds a read-only reader, a bounded snapshot exporter and the binary.
Its only edits to existing upstream files make two functions reachable —
`Database::hash_path` as `observer_path_hash`, `dep_graph::parse_refs` as
`parse_referenced_task_ids` — so the store layout and the dependency parse are
*shared with* upstream rather than reimplemented beside it. Upstream's own
library tests pass unchanged against the patched tree; that is the regression
gate for every future re-pin.

The helper reads with `SQLITE_OPEN_READ_ONLY` and `PRAGMA query_only=ON`, never
calls a constructor that creates, migrates, renames or chmods, and never uses
`immutable=1` against a live WAL. It starts no process and opens no socket.

## Field ownership

| Concern | Owner |
| --- | --- |
| Component pin, hashes, install, upgrade, rollback | HZR (`engines.lock.toml`, `hzr agents component`) |
| Enrollment, opt-in flags, polling cadence, backoff, cancellation | HZR (`[integrations.agtx]`, daemon worker) |
| Store layout, task/status vocabulary, dependency parse | agtx, reused through the patch |
| Task identity (`task_key`), run identity, session links | HZR |
| Pseudonyms (`project_hash`, `session_hash`) | HZR's existing privacy mechanism |
| Durable projections, events, retention | HZR ledger |
| Pricing, currency, money arithmetic | HZR `billing.rs`, unchanged rules |
| Everything a human wrote (titles, prose, transcripts) | agtx, and it stays there |

## Consequences

- Monitoring is genuinely optional: absent component means the Agents workspace
  degrades and nothing else does. HZR's build, start and core routes do not
  reference agtx at all.
- Every upgrade is an explicit re-pin: new commit, new patch digests, rerun of
  the compatibility, privacy, upstream-regression and economics fixtures.
- The helper costs a process spawn per poll. That is the price of not linking a
  second SQLite and not inheriting a mutating API surface, and it is bounded by
  a 3 s timeout, one helper per enrollment and two globally.
- The observed model is a series of snapshots, not an event journal. Transitions
  between polls are lost, and the UI must say so rather than imply continuity.
- The upstream license file (Apache-2.0) and package metadata (MIT) disagree at
  the pinned commit. Both are recorded in `PROVENANCE.json`. This blocks
  publishing a prebuilt adapted binary; it does not block building one locally
  on request, which is what 0.8.7 does.
