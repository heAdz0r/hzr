# HZR awareness for Codex

Use HZR as the sole agent-efficiency control plane. The managed AGENTS.md block
is rendered from `share/hzr/agent-capabilities.json`: plan with `hzr context plan`,
search with `hzr search`, read and write with the first-class `hzr read` and
`hzr write` routes, and run shell or project-build commands through `hzr exec run`.
Use `hzr rtk -- ...` only for the complete inherited engine surface. Do not invoke
another RTK installation or create local grepai/ICM stores outside HZR ownership.

TDD is opt-in. Use `hzr tdd` when the user or repository requires it, or when
regression risk justifies test-first overhead. When token or time efficiency
matters, skip it and use proportionate verification while preserving repository
quality gates. If selected, preserve focused RED and GREEN command evidence;
post-hoc passing tests are regression coverage, not TDD.

## MCP tools

Register HZR as an MCP server instead of `icm`:

```bash
hzr init                        # writes this project's .codex/config.toml pin
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
- `hzr_read` — read bounded exact content through the confined daemon-owned fork.
- `hzr_write` — apply confined atomic patch or create operations with typed CAS receipts.
- `hzr_exec` — execute through daemon policy and accounting without direct fallback.
- `hzr_observability` — inspect typed daemon, engine, capability and workspace health.
- `hzr_doctor` — run desired-state diagnostics for the exact MCP workspace.
- `hzr_memory_recall` — recall decisions, resolved errors and prior context
  *before* re-reading files analysed in an earlier session.
- `hzr_memory_store` — persist a decision, resolved error, preference or finished
  work. Not for ephemeral session state.
- `hzr_memory_update` — replace one superseded memory after namespace verification.
- `hzr_memory_forget` — delete one invalid memory after namespace verification.
- `hzr_memory_prune` — preview or remove low-weight memories in one namespace.
- `hzr_codec` — return an applied tool-payload transform or a shadow counterfactual. Codex
  cannot expose global final-response replacement, so instructed global coverage receives zero
  credit until a trusted host confirms delivery.

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
