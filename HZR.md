# HZR agent contract

HZR = heAdz0r's Zero-Redundancy engine. It is the only public control plane.
The inherited RTK implementation remains available as HZR's internal engine;
do not invoke a separately installed `rtk` binary.

**One rule behind every rule below:** a second index, a second memory store or a
second engine process costs tokens twice and can disagree with itself. Route
through HZR and there is exactly one of each.

## If you have HZR MCP tools

When your client is configured with the HZR MCP server (`hzr mcp serve`), prefer
these tools — they reach the same single store and index as the CLI, and their
results are accounted:

| Tool | Use it for |
|---|---|
| `hzr_context_plan` | Build a bounded graph-first evidence plan for unfamiliar, architectural or cross-cutting work. |
| `hzr_search` | Find code by intent (`mode: "semantic"`) or by exact pattern (`mode: "exact"`). |
| `hzr_memory_recall` | Recall durable facts, past decisions and resolved errors before re-reading earlier work. |
| `hzr_memory_store` | Persist a decision, resolved error, user preference or finished handoff. Not ephemeral state or raw tool output. |
| `hzr_memory_update` | Replace a superseded memory after HZR verifies project/global namespace ownership. |
| `hzr_memory_forget` | Delete one invalid memory after namespace verification. |
| `hzr_memory_prune` | Preview or remove low-weight memories in one namespace; preview is the default. |
| `hzr_codec` | Remove exact duplicate paragraphs from a long answer while provably preserving code, commands, paths, identifiers, errors and numbers. It does not reword or summarise prose, so text with no repetition comes back byte-identical. Use `profile: "shadow"` to measure the counterfactual without changing the text. |

The gateway negotiates the latest stable MCP revision it supports
(`2025-11-25`) while retaining compatible older revisions. Tools publish JSON
Schema 2020-12 inputs and outputs, reject unknown or invalid arguments, and return
both text and `structuredContent`. Arguments are bounded on purpose: limits are
validated at 1–50. The workspace comes from the server's launch directory — you
cannot pass it or widen scope to another repository.

Long-running tool calls are concurrent and honor `notifications/cancelled`; a cancelled
request stops work, releases its HTTP request, and emits no late response. Task-augmented
requests are not advertised because HZR negotiates the stable base protocol rather than an
experimental task extension.

The MCP process is client-managed stdio. `hzr init` never starts it: Codex or
Claude Desktop launches `hzr mcp serve` when opening a connection and closes it
through stdin EOF. Run `hzr install --force` once to install native client
registrations, then `hzr mcp status` to inspect them. The only persistent
background process is the single `hzrd` service.

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
Read     -> hzr rtk -- read <file> [--from N --to M | --outline | --symbols | --changed | -n]
Write    -> hzr rtk -- write patch|replace|set|create|batch ...
Density  -> hzr codec compile --profile shadow|adaptive|compact
Raw      -> hzr rtk -- raw <command...>   (escape hatch — see the cost below)
TDD      -> hzr tdd                  (read before production changes)
MCP      -> hzr mcp serve            (launched by a client, never by hand)
Config   -> hzr mcp config --client codex|claude-desktop  (prints a snippet)
MCP state -> hzr mcp status
Health   -> hzr doctor
Enable   -> hzr enable [--workspace DIR]
Disable  -> hzr disable [--workspace DIR]   (keeps index and memory)
Gains    -> hzr stats [--workspace DIR]
Project build -> hzr build <args>    (your project, token-optimized output)
HZR release   -> hzr release --force (rebuild and reinstall HZR itself)
```

## Update notices

HZR checks published GitHub releases without auto-installing them. A negative result is cached for
one hour; a known newer release is cached for 24 hours. Network failure is silent and never blocks
workspace startup or a tool call.

Claude's managed `SessionStart` hook returns a visible `systemMessage` and the same fact as agent
context. Codex reaches the check through the mandatory exact read of this installed `HZR.md` file;
the notice is written separately so canonical file output remains byte-exact. When a notice says a
newer release exists, inform the user once and do not run `hzr update` without explicit approval.

For implementation, bug fixes, behavior changes and refactoring, run `hzr tdd`
before changing production code and follow its RED → GREEN → REFACTOR contract.
The canonical skill is shipped at `share/hzr/skills/hzr-tdd/SKILL.md`. A passing
test without an observed relevant failure is regression coverage, not TDD.

`hzr build` and `hzr release` are different verbs on purpose: `build` builds **your
project**, `release` rebuilds and reinstalls **HZR itself**. Do not use `release` to build
a project.

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

`hzr enable` adds one initialized workspace and installs its local managed instruction blocks.
`hzr disable` removes that activation entry and those blocks without deleting the managed index,
workspace registration, memory, or ledger history. Explicit CLI commands remain available to the
operator even when automatic activation is disabled.

## Reading a file: reach for the flags, not for `sed`

`hzr rtk -- read` is not "cat with filtering". It takes the arguments you would
otherwise express by piping through another tool, and it is the single most common
source of avoidable output:

```text
hzr rtk -- read <file> --from 120 --to 180   # a line span  (instead of `sed -n 120,180p`)
hzr rtk -- read <file> -n                    # with line numbers (instead of `nl -ba`)
hzr rtk -- read <file> --outline             # structure only, ~98% smaller
hzr rtk -- read <file> --symbols             # the same structure as JSON, with line spans
hzr rtk -- read <file> --changed             # only the working-tree hunks
hzr rtk -- read <file> --since HEAD~3        # only what changed since a revision
hzr rtk -- read <file> --max-lines N         # head(1)
hzr rtk -- read <file> --tail-lines N        # tail(1)
```

Markdown defaults to a bounded digest; use `--level none` for exact full content or
`--from N --to M` for an exact range. `-n` defaults to exact content and prints original
source coordinates, including for ranges and tails. `--max-lines N` returns the first N
lines followed by the file total, omitted count, and an exact recovery command; tails and
explicit ranges carry the same bound evidence. `--outline` emits ATX Markdown headings (`#` through `######`) with source
spans, or heuristic symbols for supported Rust, Python, TypeScript, JavaScript, Go and Java
files. It is not a generic symbol query for every file format.

## Search modes

`--mode exact` is a **literal, case-sensitive** lookup. The query is matched verbatim;
`hzr search "fn handle_request" --mode exact` returns the definition, not every `fn` in
the repository. Use it for symbols, error strings, config keys and audits. If the literal
begins with `-`, terminate option parsing first: `hzr search --mode exact -- "--outline"`.
Literal matching is per source line; a multi-line signature is not one literal match.

`--mode semantic` and `--mode auto` use the ranked term model: the query is lowercased,
split on non-alphanumeric characters, stripped of stop words and stemmed, and the surviving
terms are ranked. Use it to *locate* code you cannot name exactly.

`--path` accepts several directories: `--path crates fork-core/src`.

## The cost of `raw`

`hzr rtk -- raw <cmd> <args...>` directly spawns the first argument and forwards its argv
unfiltered; it does not interpret pipes, redirects, globs or shell variables. Use an explicit
shell such as `sh -c '...'` only when shell grammar is actually required. The invocation is
recorded in the
ledger as a bypass and receives **zero** savings credit. Captured output contributes equal
baseline and delivered estimates. Inherited stdio that cannot be captured is marked
`unmeasured`, never invented as zero output, and reduces the coverage share shown by
`hzr stats`.

Raw is correct for checksums, parsers, generated files, complete logs and machine-readable
data. It is *not* correct for reading a file, searching for a symbol, or numbering lines —
the hook will offer you the first-class command with the arguments already filled in, and
you can accept it or re-run the original. Reaching for `sed -n`, `nl`, `cat`, `head`, `tail`
or `rg` through `raw` is the single largest recorded source of wasted tokens.

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
namespace; there is no "both" for writes.

Update, forget, and prune enumerate typed ICM records first and apply the same positive
namespace filter as recall. A project command therefore cannot mutate another repository or
user-global memory. `prune` defaults to preview in the API, MCP, and CLI; the CLI requires an
explicit `--apply` for deletion. As in ICM's native lifecycle, high and critical memories are
never selected by threshold pruning.

Legacy-import records lack trustworthy repository provenance. HZR retains them for audit and
explicit migration, but excludes them from automatic project recall instead of assigning all
of them to whichever repository happened to run the import.

The installed Bash hook routes commands through the managed daemon and falls
back to the same pinned HZR fork-core when the daemon is unavailable. A
degraded rewrite preserves command policy but is absent from the managed usage
ledger; `hzr doctor` and `hzr stats` report that incomplete accounting instead of
hiding it.

## What the hooks cover, and what only you can

The `PreToolUse` hook matches `Bash`, `Agent` and `Task`. It does **not** redirect the host's
own `Read`, `Grep`, `Edit`, `Write` or `Glob` calls. A failure-silent `PostToolUse` observer
records only their route and response-size estimate; it never stores tool content, mutates a
result, blocks a call, or grants savings credit. `hzr stats` therefore reports the fraction
of observed traffic covered by its reduction ratio instead of leaving native calls invisible.

Reaching for a native file tool is not blocked and is sometimes right. Prefer the `hzr`
command whenever one exists; the coverage share will show the cost of routing around it.

Prefer the bounded HZR planner for discovery. Never create a second `.grepai` index,
a second ICM database, or parallel RTK hooks.

## Reading `hzr stats`

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
