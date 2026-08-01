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

The gateway negotiates the latest stable MCP revision it supports
(`2025-11-25`) while retaining compatible older revisions. Tools publish JSON
Schema 2020-12 inputs and outputs, reject unknown or invalid arguments, and return
both text and `structuredContent`. Arguments are bounded on purpose: limits are
validated at 1–50. The workspace comes from the server's launch directory — you
cannot pass it or widen scope to another repository.

The MCP process is client-managed stdio. `hzr init` never starts it: Codex or
Claude Desktop launches `hzr mcp serve` when opening a connection and closes it
through stdin EOF. Run `hzr install --force` once to install native client
registrations, then `hzr mcp status` to inspect them. The only persistent
background process is the single `hzrd` service.

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
Memory   -> hzr memory recall|store   (see scopes below)
Semantic -> hzr rgai "<intent>"
Exact    -> hzr search "<pattern>" --mode exact
Read     -> hzr rtk -- read <file>
Write    -> hzr rtk -- write patch|replace|set ...
Raw      -> hzr rtk -- raw <command...>
TDD      -> hzr tdd                  (read before production changes)
MCP      -> hzr mcp serve            (launched by a client, never by hand)
Config   -> hzr mcp config --client codex|claude-desktop
MCP state -> hzr mcp status
Health   -> hzr doctor
Gains    -> hzr stats
Project build -> hzr build <args>    (your project, token-optimized output)
HZR release   -> hzr release --force (rebuild and reinstall HZR itself)
```

For implementation, bug fixes, behavior changes and refactoring, run `hzr tdd`
before changing production code and follow its RED → GREEN → REFACTOR contract.
The canonical skill is shipped at `share/hzr/skills/hzr-tdd/SKILL.md`. A passing
test without an observed relevant failure is regression coverage, not TDD.

`hzr build` and `hzr release` are different verbs on purpose: `build` builds **your
project**, `release` rebuilds and reinstalls **HZR itself**. Do not use `release` to build
a project.

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
```

Recall defaults to `project-and-global`, so standing preferences arrive alongside this
project's history. Another repository's memory is never reachable from any scope — that
isolation is enforced by a positive filter, not by omission. A store targets exactly one
namespace; there is no "both" for writes.

The installed Bash hook routes commands through the managed daemon and falls
back to the same pinned HZR fork-core when the daemon is unavailable. A
degraded rewrite preserves command policy but is absent from the managed usage
ledger; `hzr doctor` and `hzr stats` report that incomplete accounting instead of
hiding it.

Use raw/exact paths for checksums, parsers, generated files, complete logs and
machine-readable data. Prefer the bounded HZR planner for discovery. Never
create a second `.grepai` index, a second ICM database, or parallel RTK hooks.
