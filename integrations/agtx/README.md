# agtx Agent Observatory (optional component)

HZR can *observe* an [agtx](https://github.com/fynnfluegge/agtx) board: its tasks,
their dependencies, the transitions HZR sees while it is watching, which coding
sessions belong to which task, and what that work reportedly cost. It cannot
operate one. Nothing here creates a task, starts an agent, advances a phase,
sends terminal input, creates a worktree, merges a branch or answers a
permission prompt. A running agtx remains the sole author of its own state.

Everything on this page is **off by default**. A default HZR install downloads
nothing from agtx, runs no agtx process, opens no agtx database and adds no
listener.

## Ownership

| Owned by HZR | Owned by agtx |
| --- | --- |
| Component pin, build, hash verification and installation | The board, its scheduler and every mutating action |
| Enrollment, polling, cancellation, backoff | The task/session model and the store layout |
| Durable projections, events, pricing, ledger writes | Task titles, descriptions, branches and transcripts |
| The public dashboard API and the Agents UI | Its own TUI, MCP server and web routes |

HZR never registers an agtx MCP server, never proxies agtx's web routes and never
treats agtx's loopback listener as an authorization boundary.

## Two steps, both explicit

```bash
hzr agents component install
hzr agents enable --project /absolute/worktree --agtx-data-dir /absolute/agtx/data
```

Installing the component does not enable monitoring for anything; enabling a
project does not install the component. Both must have happened, and the global
`[integrations.agtx] enabled` flag flips to true only on the first successful
enable.

```bash
hzr agents status --json                      # component identity, enrollments, lag
hzr agents sync --project /absolute/worktree  # one bounded observation, through the daemon
hzr agents disable --project /absolute/worktree
```

`disable` stops observation and reaps HZR's own helper within five seconds. It
does not touch agtx, tmux or any coding agent, and it keeps the history HZR has
already recorded. Re-enabling resumes under a new enrollment generation.

## The component

`hzr-agtx-observer` is built from the pinned agtx commit plus one auditable
patch (`patches/agtx/1.0.4-readonly-observer.patch`). It is a separate
executable, outside HZR's Cargo workspace, because:

- HZR is on `rusqlite 0.36` and the pinned agtx on `0.34`, and linking both
  risks an incompatible SQLite native link in one process;
- importing the agtx library would pull its TUI, MCP server and web routes —
  and their mutating actions — into the HZR daemon;
- upstream's toolchain floor must never silently raise HZR's MSRV of 1.85.

The patch adds a read-only reader, a bounded snapshot exporter and the binary.
It changes no upstream behaviour: the two upstream edits make
`Database::hash_path` and `dep_graph::parse_refs` reachable under new public
names so the observer shares the store layout and the dependency parse rather
than reimplementing either. Upstream's own library test suite passes unchanged
against the patched tree.

Exact commit, digests, license evidence and verified toolchain are in
[`PROVENANCE.json`](PROVENANCE.json).

## Protocol

```
hzr-agtx-observer snapshot --request-stdin
```

One JSON request on stdin, exactly one JSON object on stdout, stable codes on
stderr. Argv spawning, no shell. Request ≤ 64 KiB, response ≤ 2 MiB, 200 tasks
and 1 000 edges per page, 3 s default timeout, 250 ms SQLite busy timeout, one
helper in flight per enrollment and at most two globally.

`snapshot_consistency` is always `per_project_transaction`: the project database
is one read, `index.db` is a separate file and a separate read, and the pair is
not an atomic snapshot of the whole store.

An unknown protocol major or an unexpected `patch_identity` is refused; the
previous snapshot is retained and marked stale rather than replaced.

## What the store sees, and what leaves it

Reads use `SQLITE_OPEN_READ_ONLY` with `PRAGMA query_only=ON`. The observer
never calls a constructor that creates directories, renames a legacy database,
sets `journal_mode`, chmods a file or runs a migration, and it never uses
`immutable=1` — that would misread a live WAL. Required tables and columns are
checked by read-only introspection; an unsupported layout is refused, and a
missing optional table only reduces reported capabilities.

Hook records are read only from `<enrolled worktree>/.agtx/status/<task>.json`,
with the task id validated as a plain filename component and the file required
to be a regular non-symlink under the size cap. `transcript_path` is never
followed. `message` and `tool` are never exported.

Nothing on the wire carries a task description, PR URL, worktree path, tmux
session name, notification prose or transcript path. The task title and branch
cross only when the enrollment asks for them (`publish_task_titles`, on by
default), because a board labelled only `Task 7ac3` cannot be read; both are
bounded and stripped of control characters at the source, and the pseudonym is
published beside them either way. Turning the flag off returns the dashboard to
pseudonyms alone.

## Fixtures

`fixtures/seed-store.py` builds a sanitized upstream-compatible store from
schema statements transcribed from the pinned commit; it refuses to write over
an existing store and never touches a real agtx installation. The
`snapshot-v1-*.json` files are recorded wire responses (ready, advanced,
partial, source-reset, incompatible) used by the HZR-side ingestion tests.
`pricing-synthetic.json` and `usage-import-v1.json` are the deterministic
economics fixtures; the prices in them are invented for arithmetic, not market
rates.

## Measured budgets

Verified against the acceptance fixture in the PRD — 1 000 tasks and 4 985
dependency references — using the real pinned helper against a real SQLite
store, not a mock:

| Budget | Limit | Measured |
| --- | --- | --- |
| Full snapshot traversal | 3 000 ms | 59 ms across 5 pages |
| Peak helper RSS | 128 MiB | 8.7 MiB |
| Response page | 2 MiB | 192 KB |
| Warm `GET /v1/dashboard/agents` p95, 100 requests | 250 ms | 11.6 ms |
| Warm `…/agents/events` p95 | 250 ms | 7.9 ms |
| Warm `…/agents/economics` p95 | 250 ms | 7.3 ms |
| Durable growth over unchanged polls | 1 KiB / 100 polls | 0 bytes over ~20 cycles |

The zero is the point of the idempotency rule: an identical snapshot bumps no
revision, records no event and rewrites no projection, so a board nobody is
touching costs nothing to keep watching.

Still unverified: Windows. The component reports itself unavailable on
platforms without a verified build rather than assuming one exists.

## Known limits

- Polling observes transitions, not every message between agents. Changes
  between polls can be missed, and the UI shows the gaps.
- Missing or unlinked usage is unknown spending, not zero spending.
- Imported reported costs are what a source claims, never a verified invoice.
- Source state can be stale whenever no agtx runtime is publishing it.
- Monitoring does not improve answer quality by itself.
- Upstream's interactive tmux workflow is not promised on native Windows; the
  observer is reported unavailable there rather than assumed to work.
