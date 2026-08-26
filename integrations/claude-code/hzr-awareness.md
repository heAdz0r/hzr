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

Use project scope for Claude Code and select one workspace for Claude Desktop:

```bash
hzr mcp config --client claude-code --workspace "$PWD" # write the output to .mcp.json
hzr mcp config --client claude-desktop --workspace "$PWD" --apply
hzr mcp status
```

Linked Claude Code worktrees can share one local identity. HZR never rewrites that state;
`.mcp.json`/`claude mcp add -s project` isolates the exact worktree, and the pinned CLI is the
safe fallback. Claude Desktop has one selected workspace, so every other workspace is reported
as unavailable until it is selected. A mismatched MCP server must never be used.

`hzr init` does not start MCP. Claude Desktop launches the registered
`hzr mcp serve` child on connection and closes it through stdin EOF; the
persistent HZR process is the single `hzrd` service.

Tools: `hzr_context_plan`, `hzr_search`, `hzr_read`, `hzr_write`, `hzr_exec`,
`hzr_observability`, `hzr_doctor`, `hzr_memory_recall`,
`hzr_memory_store`, `hzr_memory_update`, `hzr_memory_forget`,
`hzr_memory_prune`, and `hzr_codec`. Plan first for unfamiliar or cross-cutting work, recall
before re-reading files you already analysed, and store decisions and resolved
errors rather than ephemeral session state. Use `hzr_codec` to apply or shadow-measure
protected response-density transforms. Its returned payload is observable, but neither Claude
host exposes a trusted global final-response replacement hook. Global coverage is instructed or
unavailable and receives zero final-response credit. Inputs are strictly bounded and
successful calls include typed `structuredContent`. `isError: true` means no
success was confirmed and no fallback engine or store was used; recall before
retrying an ambiguously completed store.

Claude Code hooks cover native operations but do not expose the typed MCP tools or replace final
responses. Its project MCP registration therefore complements the hooks without creating a second
store. In every client HZR replaces a direct `icm serve` registration, which is what creates a
second writer per session and leaves orphans behind.
