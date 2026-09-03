//! Тексты `long_about` / Examples для высокочастотных subcommands.
//!
//! Корневой help UX живёт в отдельной ветке; этот модуль намеренно не трогает
//! группировку root `--help`.

pub const UPDATE_LONG_ABOUT: &str = "\
Download the newest published GitHub release for this platform, verify its \
SHA-256 checksums, install it, and switch `current`. No-ops when this install \
is already the newest published release. With `--check`, only query GitHub and \
report whether a newer release exists — nothing is downloaded or installed. \
`--check` exits 0 when the check succeeds (current or available); non-zero only \
on check failure.";

pub const UPDATE_AFTER_HELP: &str = "\
Examples:
  hzr update
  hzr update --check
  hzr update --json
";

pub const STATS_LONG_ABOUT: &str = "\
Show cumulative zero-redundancy gains from the HZR ledger. Without \
`--workspace`, totals are global; with `--workspace`, the view is limited to \
that project root. Estimated savings and billed provider usage stay separate.";

pub const STATS_AFTER_HELP: &str = "\
Examples:
  hzr stats
  hzr stats --workspace /path/to/project
  hzr stats --json
";

pub const ENABLE_LONG_ABOUT: &str = "\
Enable HZR for one workspace and select project-only activation. Hooks and \
SessionStart become active for that root; other workspaces stay no-ops until \
enabled. Index and memory data are kept.";

pub const ENABLE_AFTER_HELP: &str = "\
Examples:
  hzr enable
  hzr enable --workspace /path/to/project
";

pub const DISABLE_LONG_ABOUT: &str = "\
Disable HZR for one workspace without deleting its index or memory. Project-only \
activation removes the root from the enabled set; hooks become no-ops there.";

pub const DISABLE_AFTER_HELP: &str = "\
Examples:
  hzr disable
  hzr disable --workspace /path/to/project
";

pub const DOCTOR_LONG_ABOUT: &str = "\
Verify current managed instructions, pins, ownership, daemon health, and duplicate indexes for one workspace. \
Use `--fix` to transactionally migrate one unambiguous legacy index with a retained backup. \
Duplicate or conflicting indexes remain unchanged and fail with their exact paths. Stale \
instructions remain read-only by default. `--reconcile-fleet` repairs managed instructions and \
project Codex pins only for registered workspaces. Add `--migrate-legacy-indexes` to migrate \
unambiguous root indexes; duplicate archives always require an explicit reported command.";

pub const DOCTOR_AFTER_HELP: &str = "\
Examples:
  hzr doctor
  hzr doctor --workspace /path/to/project
  hzr doctor --fix --workspace /path/to/project
  hzr doctor --reconcile-fleet --dry-run
  hzr doctor --reconcile-fleet --migrate-legacy-indexes --dry-run
  hzr doctor --json
";

pub const AGENT_LONG_ABOUT: &str = "\
Run the managed caveman-code agent with HZR-owned tools (search, memory, exec). \
The agent stays inside the HZR control plane instead of spawning orphan engines.";

pub const AGENT_AFTER_HELP: &str = "\
Examples:
  hzr agent run \"summarize the auth flow\"
  hzr agent run --file prompt.txt --workspace /path/to/project
";

pub const AGENT_RUN_LONG_ABOUT: &str = "\
Run one managed prompt. Pass PROMPT on the command line, `--file PATH`, or pipe \
stdin when both are absent.";

pub const AGENT_RUN_AFTER_HELP: &str = "\
Examples:
  hzr agent run \"find the rewrite decision path\"
  hzr agent run --file prompt.txt
  cat prompt.txt | hzr agent run --response-format json
";

pub const MCP_SERVE_LONG_ABOUT: &str = "\
Serve stdio MCP until the parent agent closes stdin. Routes tools to the one HZR \
store and does not spawn orphan engines. Workspace precedence is an explicit \
`--workspace` pin, one client root from initialize, `CLAUDE_PROJECT_DIR`, then cwd; \
filesystem root and the user home are refused.";

pub const MCP_SERVE_AFTER_HELP: &str = "\
Examples:
  hzr mcp serve
  hzr mcp serve --workspace /path/to/project
";

pub const MCP_CONFIG_LONG_ABOUT: &str = "\
Print or apply an MCP server registration for an agent configuration. Pin \
`--workspace` so memory is scoped to the project even when the client launches \
from `/` or a per-session directory. Pass `--apply` to write the registration \
instead of printing a paste snippet. Claude Desktop selects one workspace at a \
time; Codex and Claude Code should use project-scoped registrations.";

pub const MCP_CONFIG_AFTER_HELP: &str = "\
Examples:
  hzr mcp config --client codex
  hzr mcp config --client claude-code --workspace /path/to/worktree
  hzr mcp config --client claude-desktop --workspace /path/to/project
  hzr mcp config --apply --workspace /path/to/project
";

pub const MCP_STATUS_LONG_ABOUT: &str = "\
Report native client registrations, workspace-binding capability and whether \
stdio MCP lifecycle is client-managed. A singleton client selected elsewhere is \
reported as unavailable for this workspace, never as a safe mismatched server.";

pub const MCP_STATUS_AFTER_HELP: &str = "\
Examples:
  hzr mcp status
  hzr mcp status --json
";

pub const MEMORY_RECALL_LONG_ABOUT: &str = "\
Recall relevant memories with full ICM semantics (topic, keyword, scope, limit). \
Default scope is this project plus your user-global memory.";

pub const MEMORY_RECALL_AFTER_HELP: &str = "\
Examples:
  hzr memory recall \"decisions\"
  hzr memory recall \"auth\" --topic decisions --limit 5
  hzr memory recall \"preferences\" --scope global
";

pub const MEMORY_STORE_LONG_ABOUT: &str = "\
Store or update a centralized ICM memory under TOPIC. Pass CONTENT, `--file`, or \
stdin. Use `--scope global` only with `preferences`, `architecture-global`, or `user-*`; default is project-only.";

pub const MEMORY_STORE_AFTER_HELP: &str = "\
Examples:
  hzr memory store decisions \"Use typed protocol JSON\"
  hzr memory store preferences --scope global --importance high \"Prefer English docs\"
  hzr memory store architecture --file note.md --keyword auth
";

pub const MEMORY_FORGET_LONG_ABOUT: &str = "\
Delete one memory after namespace ownership is verified. Requires the memory id \
from recall or status output.";

pub const MEMORY_FORGET_AFTER_HELP: &str = "\
Examples:
  hzr memory forget <id>
  hzr memory forget <id> --scope global
";

pub const MEMORY_UPDATE_LONG_ABOUT: &str = "\
Replace one memory after namespace ownership is verified. Pass new CONTENT or \
`--file`; optional importance and keywords update metadata.";

pub const MEMORY_UPDATE_AFTER_HELP: &str = "\
Examples:
  hzr memory update <id> \"Revised decision text\"
  hzr memory update <id> --file note.md --importance high
";

pub const MEMORY_PRUNE_LONG_ABOUT: &str = "\
Delete low-weight memories only in the selected namespace. Default is a dry-run \
preview; pass `--apply` to delete.";

pub const MEMORY_PRUNE_AFTER_HELP: &str = "\
Examples:
  hzr memory prune
  hzr memory prune --threshold 0.2
  hzr memory prune --apply --scope project
";

pub const MEMORY_STATUS_LONG_ABOUT: &str = "\
Show ICM state reported by hzrd (store health, counts, and ownership signals).";

pub const MEMORY_STATUS_AFTER_HELP: &str = "\
Examples:
  hzr memory status
  hzr memory status --json
";

pub const READ_AFTER_HELP: &str = "\
Forwarded fork-core options:
  --outline                     headings or code symbols with line spans
  --from N --to N               exact inclusive source range
  --max-lines N                 bounded exact head
  --tail-lines N                bounded exact tail
  --level none|minimal|aggressive
  --batch --max-tokens N        several files under one total budget

An unqualified file read uses the smart filter and a 400-line / 12K-token safety budget.
Exact unbounded recovery requires --level none and HZR_EXACT_FIDELITY=1.
";

pub const WRITE_AFTER_HELP: &str = "\
Forwarded fork-core operations:
  patch PATH --old @FILE --new @FILE --cas --retry N
  replace PATH PATTERN REPLACEMENT
  set PATH KEY VALUE
  create PATH --content TEXT
  batch --file PLAN.json

Run `hzr rtk -- write --help` for the complete inherited option reference.
";
