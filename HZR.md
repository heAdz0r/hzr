# HZR agent contract

HZR = heAdz0r's Zero-Redundancy engine. It is the only public control plane.
The inherited RTK implementation remains available as HZR's internal engine;
do not invoke a separately installed `rtk` binary.

**One rule behind every rule below:** a second index, a second memory store or a
second engine process costs tokens twice and can disagree with itself. Route
through HZR and there is exactly one of each.

`contracts/agent-capabilities.json` is the machine-readable SSOT for command routes,
tool inventories, and harness capabilities. This file is its normative human projection:
it adds safety and recovery semantics without redefining those facts. Managed agent blocks
are rendered from the SSOT and point back here for detail. Ordinary tasks must not import or read this whole file. When a routing question is
not answered by the managed block, read this file with `--outline`, then retrieve only the
relevant `--from`/`--to` range. Full exact content is reserved for material contract audits.

## If you have HZR MCP tools

When your client is configured with the HZR MCP server (`hzr mcp serve`), prefer
these tools — they reach the same single store and index as the CLI, and their
results are accounted:

| Tool | Use it for |
|---|---|
| `hzr_context_plan` | Build a bounded graph-first evidence plan for unfamiliar, architectural or cross-cutting work. |
| `hzr_search` | Find code by intent (`mode: "semantic"`) or by exact pattern (`mode: "exact"`). |
| `hzr_read` | Read bounded exact content through the daemon-owned confined fork path. |
| `hzr_write` | Apply confined atomic patch or create operations with typed CAS receipts. |
| `hzr_exec` | Run shell commands through daemon policy and accounting without direct fallback. |
| `hzr_observability` | Return typed daemon, engine, capability and workspace health. |
| `hzr_doctor` | Run desired-state diagnostics for the exact MCP workspace. |
| `hzr_memory_recall` | Recall durable facts, past decisions and resolved errors before re-reading earlier work. |
| `hzr_memory_get` | Retrieve one exact memory ID after verifying namespace ownership. |
| `hzr_memory_store` | Persist a decision, resolved error, user preference or finished handoff. Not ephemeral state or raw tool output. |
| `hzr_memory_update` | Replace a superseded memory after HZR verifies project/global namespace ownership. |
| `hzr_memory_forget` | Delete one invalid memory after namespace verification. |
| `hzr_memory_prune` | Preview or remove low-weight memories in one namespace; preview is the default. |
| `hzr_codec` | Remove exact duplicate paragraphs from a long answer while provably preserving code, commands, paths, identifiers, errors and numbers. It does not reword or summarise prose, so text with no repetition comes back byte-identical. Use `profile: "shadow"` to measure the counterfactual without changing the text. |

The gateway negotiates the latest stable MCP revision it supports
(`2025-11-25`) while retaining compatible older revisions. Tools publish JSON
Schema 2020-12 inputs and outputs, reject unknown or invalid arguments, and return
both text and `structuredContent`. Each tool schema declares its own limits. Exact reads
support shared budgets and continuation; exec supports start/wait/cancel job handles.
The workspace is pinned when the server launches — you
cannot pass it or widen scope to another repository.

Long-running tool calls are concurrent and honor `notifications/cancelled`; a cancelled
request stops work, releases its HTTP request, and emits no late response. Task-augmented
requests are not advertised because HZR negotiates the stable base protocol rather than an
experimental task extension.

The MCP process is client-managed stdio. `hzr init` never starts it. Codex uses the exact
project-scoped `.codex/config.toml` pin written by init. Claude Code should use a per-worktree
`.mcp.json` project scope; linked worktrees can share a local identity, so HZR audits but never
retargets Claude's local state. Claude Desktop has one singleton selected workspace. Selecting
another project makes it explicitly `unavailable_for_this_workspace`; a mismatched server must
not be used, and the workspace-pinned CLI remains safe. Run `hzr mcp status` in the project to
inspect capability, effective scope, availability and remediation.

Claude and Codex do not expose a trusted hook that can replace every final assistant response.
Managed instructions and Claude Code SessionStart tell the agent to call `hzr_codec` for eligible
long prose. The returned tool/CLI payload is an observable transform and can receive estimated
codec-token credit. It does not prove that a later final assistant response was replaced:
global-response coverage stays `instructed` (or `unavailable`), shadow stays counterfactual, and
neither receives final-response economic credit or provider-billed dollar credit.

When `[activation].mode = "selected"`, project-scoped MCP tools additionally require an
initialized workspace whose repository/worktree identity appears in `enabled_workspaces`.
Anything else returns `isError` before dispatch. Project-only installation intentionally removes
client-global MCP registrations; a manual pinned registration requires a separate client profile
if the tool itself must be invisible in baseline projects.

`isError: true` means the requested result was not confirmed and HZR did not
fall back to a second engine or store. Validation and pre-dispatch failures do not
write anything. If a store transport fails after dispatch, completion can be
unknown; recall the fact before retrying instead of assuming either success or
failure.

Do **not** register or launch `icm`, `grepai` or `rtk` as your own MCP server or
subprocess. Each direct launch adds another writer to the store HZR supervises
and — observed in practice — leaves orphaned processes behind when your session
dies.

## Command paths

```text
Context  -> hzr context plan "<intent>"
Map      -> hzr rtk -- memory explore <dir>   (entry points, hot paths, API surface)
Memory   -> hzr memory recall|store|update|forget|prune   (see scopes below)
Semantic -> hzr rgai "<intent>"
Literal  -> hzr search "<pattern>" --mode exact [--path FILE|DIR ...]
Ranked   -> hzr search "<terms>" --mode auto
Read     -> hzr read <file> [--from N --to M | --outline | --symbols | --changed | -n]
Write    -> hzr write patch|replace|set|create|batch ...
Shell    -> hzr exec run '<shell command>'   (default policy route; preserves shell grammar)
Known output -> hzr rtk -- test|err|summary|log <command...>   (plain argv only)
Density  -> hzr codec compile --profile shadow|adaptive|compact
Exact raw -> HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr exec run '<command>'
TDD      -> hzr tdd                  (optional; strict when selected)
MCP      -> hzr mcp serve            (launched by a client, never by hand)
Config   -> hzr mcp config --client codex|claude-desktop  (prints a snippet; --apply selects explicitly)
MCP state -> hzr mcp status
Health   -> hzr doctor [--fix]     (--fix migrates one unambiguous legacy .grepai with backup)
Enable   -> hzr enable [--workspace DIR]
Disable  -> hzr disable [--workspace DIR]   (keeps index and memory)
Gains    -> hzr stats [--workspace DIR] [--since 7d]
Project build -> hzr exec run '<project build command>'
HZR release   -> hzr release --force (rebuild and reinstall HZR itself)
```

Managed execution forwards the caller's validated `PATH` to the daemon, including through an
approval, but does not copy the rest of the caller environment. Commands that need explicit
environment values must continue to declare them in the command itself.

Codex keeps exact workspace pins in each repository and uses an unpinned user registration only
as a dynamic client-root fallback. Claude Desktop has one explicit selected workspace; ordinary
global installation preserves a valid selection rather than retargeting it to the install cwd.
Automatic `init --if-needed` and fleet reconciliation preserve Git-tracked shared instruction
contracts; changing instruction scope explicitly remains authoritative.

## Update notices

HZR checks published GitHub releases without auto-installing them. A negative result is cached for
one hour; a known newer release is cached for 24 hours. Network failure is silent and never blocks
workspace startup or a tool call.

Claude's managed `SessionStart` hook returns a visible `systemMessage` and the same fact as agent
context. Codex receives the compact managed contract without an automatic full read of this file;
the update notice is written separately so canonical contract output remains byte-exact. When a
notice says a newer release exists, inform the user once and do not run `hzr update` without
explicit approval.

TDD is opt-in, not the default for every agent change. Use `hzr tdd` when the
user or repository requires it, or when regression risk justifies test-first
overhead. When token or time efficiency matters, skip it and use proportionate
verification instead. Repository-required quality gates still apply. When TDD
is selected, follow its strict RED → GREEN → REFACTOR contract. The canonical
skill is shipped at `share/hzr/skills/hzr-tdd/SKILL.md`; a passing test without
an observed relevant failure is regression coverage, not TDD.

`hzr build` is a compatibility alias for the inherited fork-core self-build pipeline;
it is not a generic project-build wrapper. Route ordinary project builds through
`hzr exec run` so policy can select the correct filter. `hzr release` rebuilds and
reinstalls **HZR itself**; do not use it to build another project.

## Project-only activation

The normal installation enables HZR for every project. A controlled baseline comparison uses:

```text
hzr install --project-only --dry-run
hzr install --project-only --force
```

In that mode the global hook is only a dispatcher: it resolves the canonical repository/worktree
identity and emits no hook response outside the enabled set. `SessionStart` uses
`init --if-enabled`, so merely opening another directory cannot create `.grepai` or register it.
Agent instructions live in the enabled project's root `CLAUDE.md` and `AGENTS.md`; the user-global
managed blocks and HZR-owned global MCP registrations are removed transactionally with backups.

Instruction placement is independently configurable. The default `[instructions] scope =
"shared"` preserves the behavior above. `scope = "local"` (or `--instruction-scope local` on
`init`/`install`) writes `CLAUDE.local.md` and `AGENTS.override.md`, keeps them in
`.git/info/exclude`, and does not modify shared repository instruction files. The Codex override
requires reading the repository's `AGENTS.md` before work so local HZR routing cannot hide team
policy.

`hzr enable` adds one initialized workspace and installs its local managed instruction blocks.
`hzr disable` removes that activation entry and those blocks without deleting the managed index,
workspace registration, memory, or ledger history. Explicit CLI commands remain available to the
operator even when automatic activation is disabled.

`hzr init` enables the repository graph in the managed grepai configuration, including existing
HZR-owned workspaces where the upstream default left it disabled. `hzr doctor` remains read-only:
it reports graph readiness, while init/warm performs the state change.

## Reading files by total task cost

Use exact full content when the whole file is needed or repeated fragments would cost more.
Use an outline to discover structure and ranges to resolve a focused question. Neither is a
mandatory extra round trip before an already justified full read.

```text
hzr --json read <file>                              # exact content, completeness and source hash
hzr --json read --batch --max-tokens 12000 <files...> # one shared response budget
hzr --json read <file> --from 120 --to 180           # exact source range
hzr --json read <file> --from 181 --expected-sha256 <hash>
hzr read <file> --level none                        # explicitly requested exact full source
hzr read <file> --outline                           # headings or heuristic source symbols
hzr read <file> --changed                           # working-tree hunks
```

Typed reads return explicit completeness, omitted content and continuation coordinates.
A bounded response is not the whole file. Reuse its source hash for expansion and stop fragmenting
when the accumulated wrapper, retry and reread cost exceeds one full pass. Token budgets are
UTF-8/4 estimates, not provider token counts. Full content remains subject to workspace confinement
and response limits; a full-read request does not authorize access outside the workspace.

For repeated exploration, pass `--context-epoch <epoch> --session-id <session>` to typed
CLI reads, or `context_epoch` and `session_id` to MCP. Change the epoch after compaction,
fork or resume. The bounded in-memory advisory records produced response estimates and
source overlap for that workspace, source hash and epoch. It can recommend `read_remaining`
and return the next missing range when further fragments cost more than one full result.
It never hides requested text or assumes the host retained previous output. Restart or cache
eviction resets this advisory; these counters are not provider usage or billed savings.

The ordinary text read retains its smart default. Explicit ranges and `--level none` retain
exact source; no fidelity marker is required for an ordinary exact source read. Binary bytes
still use the dedicated managed raw fidelity path with reason `binary`.
`--outline` supports Markdown headings and heuristic symbols for supported code formats;
it is not a generic symbol query for every language.

## Search modes

`--mode exact` is a **literal, case-sensitive** lookup. The query is matched verbatim;
`hzr search "fn handle_request" --mode exact` returns the definition, not every `fn` in
the repository. Use it for symbols, error strings, config keys and audits. If the literal
begins with `-`, terminate option parsing first: `hzr search --mode exact -- "--outline"`.
Literal matching is per source line; a multi-line signature is not one literal match.

`--mode semantic` and `--mode auto` use the ranked term model and are the discovery default:
the query is lowercased,
split on non-alphanumeric characters, stripped of stop words and stemmed, and the surviving
terms are ranked. Use it to *locate* code you cannot name exactly.

`--path` accepts several directories: `--path crates fork-core/src`.

## Default shell routing

For agent-originated shell work, use `hzr exec run '<shell command>'`. It sends the complete
shell string through the canonical policy, so supported commands such as `ssh`, `curl`, `bun`,
`git`, `find`, and `rg` reach their filtered first-class implementations. Pipes, redirects,
globs, and remote command strings remain part of the one quoted shell command.

Use `hzr exec rewrite '<shell command>'` for a read-only routing check. When the decision is
`allow_rewrite`, using `raw` for that command is forbidden: HZR already has an effective route.
Do not choose `raw` merely because a command uses SSH, JSON, pipes, redirects, or unfamiliar
arguments. If no filter exists, `hzr exec run` still performs the tracked fallback; agents do not
need to select `raw` themselves.

The policy recursively inspects POSIX shell launchers and environment prefixes. Wrapping a managed
command in `/bin/sh`, `bash`, `zsh`, or a simple Python file/JSON/subprocess script does not turn it
into an allowed proxy: HZR rewrites the proven leaf or returns an explicit policy `Ask`. Opaque
computation, migration, code generation, redirects, and mixed pipelines remain usable only as a
tracked no-equivalent route; they receive no savings credit.

## The cost of `raw`

Direct `hzr rtk -- raw ...` and `hzr rtk -- proxy ...` entry points are rejected because they
cannot reserve the managed fidelity allowance atomically. Shell grammar stays on `hzr exec run`,
which transports fidelity intent separately from the command, applies policy before spawning, and
records captured output with equal baseline and delivered estimates. Output that cannot be measured
is never invented as zero and reduces the coverage share shown by `hzr stats`.

Exact operator/debug recovery uses
`HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr exec run '<command>'`, where `reason`
is one of `binary`, `checksum`, `machine_protocol`, `complete_log`, `full_patch`, or
`verbatim_source`. A missing or unknown reason returns a policy `Ask` and is never echoed into
telemetry. Even a valid reason cannot override a deny/ask decision or an equivalent managed HZR
command; it enables unfiltered bytes only after fork-core proves that no byte-faithful first-class
route exists. The default per-session allowance is five operations or 100,000 estimated delivered
tokens, whichever is reached first. A statically identifiable local read that would exceed the
remaining allowance, and any unmeasurable remote exact stream, requires approval before execution.
Ordinary exact source reads are first-class reads, not raw-fidelity exceptions. The normal shell route is still
`hzr exec run`. Unmarked managed RAW wrappers are
removed before fork-core policy runs. Raw is *never* correct when policy reports a safe first-class
replacement.
Reaching for `sed -n`, `nl`, `cat`, `head`, `tail`, `rg`, `ssh`, `curl`, `bun`, `git`, or `find`
through `raw` when a filtered route exists is avoidable token waste.

## Memory scopes

One store, two reachable namespaces. Choose deliberately:

| Scope | Store it here when the fact is… |
|---|---|
| `project` (store default) | about this repository — its architecture, its resolved errors, its context |
| `global` | about the **user** — a preference, a standing rule, a way of working that applies everywhere |

```text
hzr memory store --scope global preferences "<a standing user preference>"
hzr memory recall "<query>"                      # project + global (default)
hzr memory recall --scope global "<query>"       # only user-wide facts
hzr memory update <id> "<replacement>"           # project scope by default
hzr memory forget <id>                            # project scope by default
hzr memory prune --threshold 0.1                   # preview (default)
hzr memory prune --threshold 0.1 --apply           # explicit deletion
```

Recall defaults to `project-and-global`, so standing preferences arrive alongside this
project's history. Another repository's memory is never reachable from any scope — that
isolation is enforced by a positive filter, not by omission. A store targets exactly one
namespace; there is no "both" for writes. Global writes are limited to `preferences`,
`architecture-global`, and `user-*` topics; project context and decisions must remain in the
project namespace.

Recall deliberately uses ICM FTS5 lexical retrieval. Embeddings are disabled in this release;
`memory status` reports that as a capability boundary, not as a health failure.

Update, forget, and prune enumerate typed ICM records first and apply the same positive
namespace filter as recall. A project command therefore cannot mutate another repository or
user-global memory. `prune` defaults to preview in the API, MCP, and CLI; the CLI requires an
explicit `--apply` for deletion. As in ICM's native lifecycle, high and critical memories are
never selected by threshold pruning.

Legacy-import records lack trustworthy repository provenance. HZR retains them for audit and
explicit migration, but excludes them from automatic project recall instead of assigning all
of them to whichever repository happened to run the import.

The installed Claude Code Bash hook routes commands through the managed daemon and falls
back to the same pinned HZR fork-core when the daemon is unavailable. A
degraded rewrite preserves command policy but is absent from the managed usage
ledger; `hzr doctor` and `hzr stats` report that incomplete accounting instead of
hiding it.

## Host hook capabilities and actual coverage

`hzr hooks capabilities --host claude --probe` and `--host codex --probe` expose
the versioned adapter contract and local fixture checks. They do not inspect installed host
versions, trusted activation, or the next model request; those states remain unverified.

Claude's managed PreToolUse adapter routes supported Bash arguments without granting new
permissions. Unknown tool shapes pass to the host unchanged. Native Read, Grep, Glob, Edit and
Write preserve their exact arguments in all legacy native modes. The PostToolUse observer
stores estimates without tool content and grants no savings credit. Failed accounting is
reported separately. Native execution without an HZR transform is not an avoidable policy violation.

Current Claude documentation supports structured `updatedToolOutput` for built-in tools, but
HZR does not currently implement a validated native output replacement. Never claim replacement
from additional context, a pre-hook rewrite, or a successful local fixture.

Codex supports PreToolUse for canonical Bash inputs (including unified exec) and apply_patch.
HZR's optional `hzr hooks dispatch --host codex` adapter supports canonical Bash only; installation
is manual and requires the host's normal trust review. Because Codex requires an allow decision
with updatedInput, HZR rewrites only when the host explicitly reports bypassPermissions.
Other permission modes and unsupported shapes follow normal host execution and permissions.
HZR does not emit Codex PostToolUse block responses: they can reject code-mode promises after
the command already ran. Use managed CLI or workspace-bound MCP tools for ordinary Codex work.

The managed harness exposes its HZR-owned tools. No adapter can replace every final answer.
Declared host support, installed configuration, observed dispatch, trusted delivery and economic
credit are separate claims. A real host sentinel reaching the next model request is still required
before reporting verified output replacement. Never create separate grepai, ICM or RTK owners.

## Reading `hzr stats`

Stats default to typed v2 plus aggregate-compatible typed v1 fork rows. V1 rows retain their
original label and are not admitted to keyed v2 session identity. Use `--accounting-version all`
only for an older-version compatibility comparison; incompatible rows are excluded from current
headline claims. `--all`
means all aggregate groups, never command payloads. Stored and serialized telemetry contains typed
families, routes, versions and hashes rather than commands, arguments, queries, paths, sessions,
environment values, SQL, heredocs, prompts, responses, or stdin. Read metrics separately report
selection reduction against the full source and transform overhead against the selected slice.

Four numbers, in the order they must be read:

1. **LOCAL OUTPUT REDUCTION** — an estimate from before/after output size. Not a bill.
2. **OPTIMIZER BYPASS** — the share of operations and delivered tokens that skipped HZR
   entirely, with the first-class command that replaces each bypassed tool. A high
   headline ratio next to a high bypass share means the ratio is measuring a shrinking
   fraction of your actual traffic.
3. **PROVIDER USAGE** — actual, billed. Populated by `hzr agent run`, which reports real
   token counts through the managed bridge. Empty means no provider-billed task has run,
   not that the cost was zero.
4. **ACCOUNTING COVERAGE** — the measured share beside native-observed and explicitly
   unmeasured operations, split by CLI/hook, MCP, and native-host channels. It is
   `COMPLETE` only when no operation row or degraded rewrite was lost while the daemon was
   down. Rewrite gaps close on the next managed rewrite; historical gap counts remain visible.
