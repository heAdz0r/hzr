# HZR awareness for Claude Code

HZR owns Bash rewrite, graph-first context, the canonical grepai store and the
project-scoped ICM store. The `PreToolUse` dispatcher injects one bounded plan
for agent tasks and routes Bash through the managed daemon or the pinned
fork-core fallback. `SessionStart` runs the read-only-idempotent
`hzr init --if-needed --quiet` path.

For direct operations follow the root `HZR.md` contract. In particular, the
current exact read/write compatibility surface is `hzr rtk -- ...`; standalone
`rtk` and undocumented `hzr read`/`hzr write` aliases are not valid entrypoints.

## MCP tools

The Claude desktop app reaches memory only through MCP. Register HZR rather than
`icm` so both surfaces share one store:

```bash
hzr mcp config --client claude-desktop   # prints the mcpServers block
```

Tools: `hzr_memory_recall`, `hzr_memory_store`, `hzr_search`. Recall before
re-reading files you already analysed; store decisions and resolved errors, not
ephemeral session state. `isError: true` means nothing was read or written.

Claude Code itself does not need this: its hooks and the `CLAUDE.md` contract
already route through `hzr`. The MCP server exists for clients that have no hook
mechanism, and for them it replaces a direct `icm serve` registration — which is
what creates a second writer per session and leaves orphans behind.
