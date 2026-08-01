# HZR awareness for Codex

Use HZR as the sole agent-efficiency control plane. Plan with `hzr context
plan`, search with `hzr rgai` or exact `hzr search`, and persist memory with
`hzr memory`. Use `hzr rtk -- read|write|raw` for the complete inherited engine
surface. Do not invoke another RTK installation or create local grepai/ICM
stores outside HZR ownership.

For Rust implementation, fixes, behavior changes, and refactoring, run `hzr tdd`
before production changes. Preserve the focused RED and GREEN command evidence;
post-hoc passing tests are regression coverage, not TDD.

## MCP tools

Register HZR as an MCP server instead of `icm`:

```bash
hzr mcp config --client codex   # prints the [mcp_servers.hzr] block
hzr mcp status                  # audits native registration and lifecycle
```

`hzr init` never starts MCP. Codex launches the registered `hzr mcp serve`
child natively when it opens the connection and closes it through stdin EOF.
`hzr install --force` writes this registration once.

Then prefer these tools — they reach the same single store and index as the CLI:

- `hzr_context_plan` — build a bounded graph-first evidence plan for unfamiliar,
  architectural or cross-cutting work.
- `hzr_search` — find code by intent (`mode: "semantic"`) or exactly
  (`mode: "exact"`), optionally narrowed by `path`.
- `hzr_memory_recall` — recall decisions, resolved errors and prior context
  *before* re-reading files analysed in an earlier session.
- `hzr_memory_store` — persist a decision, resolved error, preference or finished
  work. Not for ephemeral session state.

Inputs reject invalid types, enums, unknown fields and limits outside 1–50.
Successful calls return typed `structuredContent` and a text copy for older
clients. `isError: true` means no success was confirmed and no fallback store
was used. If store transport fails after dispatch, recall before retrying because
completion may be unknown.

A direct `[mcp_servers.icm]` entry is what produced eight orphaned `icm serve`
processes from dead sessions: Codex spawns one per session and does not reap it.
Routing through `hzr mcp serve` avoids that, because the adapter holds no store
of its own and exits as soon as its parent closes stdin.

When `source=degraded` is reported, fork behavior remains pinned but managed
usage accounting is incomplete. Preserve this distinction in diagnostics and
benchmarks.
