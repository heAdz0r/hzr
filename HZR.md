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
| `hzr_memory_recall` | Recall durable facts, past decisions, resolved errors. Do this **before** re-reading files you already analysed in an earlier session. |
| `hzr_memory_store` | Persist a decision, a resolved error, a user preference or finished work. Not for ephemeral session state. |
| `hzr_search` | Find code by intent (`mode: "semantic"`, the default) or by exact pattern (`mode: "exact"`). |

Arguments are bounded on purpose: `limit` is clamped to 50. The workspace comes
from the server's launch directory — you cannot pass it and cannot widen scope to
another repository.

`isError: true` means **nothing was read or written**. It is never a partial
success, so a failed `hzr_memory_store` stored nothing and may be retried once
the reported cause is fixed. HZR never falls back to a second store to make a
call look successful.

Do **not** register or launch `icm`, `grepai` or `rtk` as your own MCP server or
subprocess. Each direct launch adds another writer to the store HZR supervises
and — observed in practice — leaves orphaned processes behind when your session
dies.

## Command paths

```text
Context  -> hzr context plan "<intent>"
Memory   -> hzr memory recall|store
Semantic -> hzr rgai "<intent>"
Exact    -> hzr search "<pattern>" --mode exact
Read     -> hzr rtk -- read <file>
Write    -> hzr rtk -- write patch|replace|set ...
Raw      -> hzr rtk -- raw <command...>
MCP      -> hzr mcp serve            (launched by a client, never by hand)
Config   -> hzr mcp config --client codex|claude-desktop
Health   -> hzr doctor
Gains    -> hzr stats
```

The installed Bash hook routes commands through the managed daemon and falls
back to the same pinned HZR fork-core when the daemon is unavailable. A
degraded rewrite preserves command policy but is absent from the managed usage
ledger; `hzr doctor` and `hzr stats` report that incomplete accounting instead of
hiding it.

Use raw/exact paths for checksums, parsers, generated files, complete logs and
machine-readable data. Prefer the bounded HZR planner for discovery. Never
create a second `.grepai` index, a second ICM database, or parallel RTK hooks.
