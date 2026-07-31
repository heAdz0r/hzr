# HZR awareness for Codex

Use HZR as the sole agent-efficiency control plane. Plan with `hzr context
plan`, search with `hzr rgai` or exact `hzr search`, and persist memory with
`hzr memory`. Use `hzr rtk -- read|write|raw` for the complete inherited engine
surface. Do not invoke another RTK installation or create local grepai/ICM
stores outside HZR ownership.

## MCP tools

Register HZR as an MCP server instead of `icm`:

```bash
hzr mcp config --client codex   # prints the [mcp_servers.hzr] block
```

Then prefer these tools — they reach the same single store and index as the CLI:

- `hzr_memory_recall` — recall decisions, resolved errors and prior context
  *before* re-reading files analysed in an earlier session.
- `hzr_memory_store` — persist a decision, resolved error, preference or finished
  work. Not for ephemeral session state.
- `hzr_search` — find code by intent (`mode: "semantic"`) or exactly
  (`mode: "exact"`). `limit` is clamped to 50.

`isError: true` means nothing was read or written — never a partial success, so a
failed store may be retried once its stated cause is fixed.

A direct `[mcp_servers.icm]` entry is what produced eight orphaned `icm serve`
processes from dead sessions: Codex spawns one per session and does not reap it.
Routing through `hzr mcp serve` avoids that, because the adapter holds no store
of its own and exits as soon as its parent closes stdin.

When `source=degraded` is reported, fork behavior remains pinned but managed
usage accounting is incomplete. Preserve this distinction in diagnostics and
benchmarks.
