<!-- hzr:begin managed agent contract — do not edit inside -->

# HZR tool contract (managed)

`hzr` — HZR's Zero-Redundancy control plane — is the only control plane. Do not
invoke separately installed `rtk`, `grepai`, `icm` binaries: HZR owns those
engines internally, and a direct call creates the duplicate scan, duplicate
store and unaccounted usage this engine exists to remove.

This managed region defines tool routing only. Keep repository-specific roles,
source paths and test commands in that repository's root instruction file, not
in a user-global instruction file.

Read the full contract at `/Users/andrew/.local/share/hzr/current/share/hzr/HZR.md` only when a bounded lookup cannot resolve HZR-policy ambiguity.
Ordinary tasks must not import or read it in full. Start with `hzr read /Users/andrew/.local/share/hzr/current/share/hzr/HZR.md --outline`, then read only the relevant `--from`/`--to` range.

| Instead of | Use |
|---|---|
| Read | `hzr read <file>`; Uses the smart default; use --outline first for structure and ranges for exact evidence. |
| Grep or repository search | `hzr search "<intent>" --mode auto`; Use --mode exact only for a known literal; hzr rgai is the semantic shorthand. |
| Edit or Write | `hzr write patch\|replace\|set\|create\|batch ...`; Writes are atomic and idempotent per file; a batch is not an all-files transaction. |
| durable memory | `hzr memory recall\|store\|update\|forget\|prune`; Project is the default scope; global is only for durable user-wide facts. |
| cross-cutting context discovery | `hzr context plan "<intent>"`; Builds bounded graph-first evidence from independently calibrated retrieval sources. |
| shell command | `hzr exec run '<shell command>'`; Default policy route; preserves shell grammar and selects managed filtering or tracked fallback. |
| build this project | `hzr exec run '<project build command>'`; Project builds are ordinary managed shell commands; hzr build is not a generic project-build wrapper. |
| optional TDD | `hzr tdd`; Use only when requested, repository-required, or justified by regression risk. |
| explicit unfiltered recovery | `HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr exec run '<command>'`; Allowed reasons: binary, checksum, machine_protocol, complete_log, full_patch, or verbatim_source. |

## Execution invariants

For agent-originated shell work, `hzr exec run` is the default. If
`hzr exec rewrite '<shell command>'` returns `allow_rewrite`, `raw` is forbidden.
When no filter exists, it performs a tracked fallback; policy ambiguity returns `Ask`.
For plain argv commands with known output intent,
`hzr rtk -- test`, `err`, `summary` and `log` routes provide bounded
filtering. Keep pipes, redirects and other shell grammar on `hzr exec run`.

Unbounded `read --level none` defeats the smart default and is automatically reduced.
Prefer `--outline` for structure and `--from`/`--to` for exact evidence. Use
`HZR_EXACT_FIDELITY=1 hzr read <file> --level none` only when the whole file
is authoritative input that cannot be bounded. Multi-file reads use
`hzr read --batch --max-tokens N <files...>`.

TDD is opt-in, not the default. When token or time efficiency matters, skip it
and use proportionate verification; repository-required quality gates still apply.

## Memory scopes

One store, two namespaces. `--scope project` (the store default) is for facts about
*this repository*. `--scope global` is for facts about the **user** — a preference or
standing rule that applies in every repository. Recall may combine project and global;
another repository's memory is never reachable.

## MCP tools

Use a registered `hzr` MCP server only after its initialize result reports
`serverInfo.workspace.bound = true` and `serverInfo.workspace.project` exactly matches
the canonical current worktree. Otherwise use the CLI routes and repair the project pin;
never recommend or use an MCP session bound to another workspace:

| Tool | Use it for |
|---|---|
| `hzr_context_plan` | Build bounded graph-first evidence for unfamiliar or cross-cutting work. |
| `hzr_search` | Find code by intent or by a known exact literal. |
| `hzr_memory_recall` | Recall durable decisions, resolved errors, and prior context. |
| `hzr_memory_store` | Persist one durable fact or finished handoff, not ephemeral state. |
| `hzr_memory_update` | Replace one superseded memory after namespace ownership is verified. |
| `hzr_memory_forget` | Delete one invalid memory after namespace ownership is verified. |
| `hzr_memory_prune` | Preview or remove low-weight memories in one namespace. |
| `hzr_codec` | Apply or shadow-measure protected response-density transforms. |
| `hzr_read` | Read bounded exact content through the daemon-owned confined fork path. |
| `hzr_write` | Apply confined atomic patch or create operations with typed CAS receipts. |
| `hzr_exec` | Run shell commands through daemon policy and accounting without direct fallback. |
| `hzr_observability` | Return typed daemon, engine, capability, and workspace health. |
| `hzr_doctor` | Run desired-state diagnostics for the exact MCP workspace. |

MCP is client-managed stdio; `hzr init` writes the trusted-project Codex registration
but never starts it. `isError: true` confirms no
success and no fallback store. Recall before retrying an ambiguously completed write.
Never register `rtk`, `grepai`, `icm` as separate MCP servers.

The Claude Code `PreToolUse` hook routes Bash through the managed daemon and
falls back to the same pinned fork-core when the daemon is down. A degraded
rewrite keeps command policy but is absent from the usage ledger; `hzr doctor`
and `hzr stats` report that incomplete accounting rather than hiding it.

The failure-open `PreToolUse` hook sees native `Read`, `Grep`, `Glob`, `Edit` and
`Write`. In `steer` mode it prescribes `hzr read`/`hzr search`; `Glob` and native
edits remain allowed. `strict` additionally prescribes `hzr write`, while `observe`
retains measurement-only compatibility. The `PostToolUse` observer stores no tool
content and grants no savings credit. In `steer`/`strict`, policy-allowed native
calls are accounted as typed E10 bypasses, not hidden as `native_unaccounted`.

<!-- hzr:end managed agent contract -->
