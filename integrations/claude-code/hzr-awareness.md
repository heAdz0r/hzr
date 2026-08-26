# HZR awareness for Claude Code

HZR owns Bash rewrite, graph-first context, the canonical grepai store and the
project-scoped ICM store. The `PreToolUse` dispatcher injects one bounded plan
for agent tasks and routes Bash through the managed daemon or the pinned
fork-core fallback. `SessionStart` runs the read-only-idempotent
`hzr init --if-needed --quiet` path.

For direct operations follow the routes projected from
`share/hzr/agent-capabilities.json` into the managed `CLAUDE.md` block. `hzr read`,
`hzr write`, `hzr search`, and `hzr exec run` are first-class HZR routes;
`hzr rtk -- ...` exposes the complete inherited engine surface. Never invoke a
standalone `rtk` binary.

TDD is opt-in. Use `hzr tdd` when the user or repository requires it, or when
regression risk justifies test-first overhead. When token or time efficiency
matters, skip it and use proportionate verification while preserving repository
quality gates. If selected, preserve focused RED and GREEN command evidence;
post-hoc passing tests are regression coverage, not TDD.

## MCP tools

The Claude desktop app reaches memory only through MCP. Register HZR rather than
`icm` so both surfaces share one store:

```bash
hzr mcp config --client claude-desktop   # prints the mcpServers block
hzr mcp status                           # audits native registration and lifecycle
```

`hzr init` does not start MCP. Claude Desktop launches the registered
`hzr mcp serve` child on connection and closes it through stdin EOF; the
persistent HZR process is the single `hzrd` service.

Tools: `hzr_context_plan`, `hzr_search`, `hzr_memory_recall`,
`hzr_memory_store`, `hzr_memory_update`, `hzr_memory_forget`,
`hzr_memory_prune`, and `hzr_codec`. Plan first for unfamiliar or cross-cutting work, recall
before re-reading files you already analysed, and store decisions and resolved
errors rather than ephemeral session state. Use `hzr_codec` to apply or shadow-measure
protected response-density transforms. Inputs are strictly bounded and
successful calls include typed `structuredContent`. `isError: true` means no
success was confirmed and no fallback engine or store was used; recall before
retrying an ambiguously completed store.

Claude Code itself does not need this: its hooks and the `CLAUDE.md` contract
already route through `hzr`. The MCP server exists for clients that have no hook
mechanism, and for them it replaces a direct `icm serve` registration — which is
what creates a second writer per session and leaves orphans behind.
