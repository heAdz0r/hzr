# RTK Memory Layer (`rtk memory`)

> **Version**: ARTIFACT_VERSION 4 · SQLite WAL · fork.12 (2026-02-19)

The memory layer is a persistent, shared project-intelligence cache built into rtk.
It replaces repeated Explore subagent passes with a compact, query-typed context slice —
achieving **89% token savings** on re-exploration and sub-10 ms cache-hit latency (p95).

---

## Table of Contents

1. [Why Memory](#1-why-memory)
2. [Architecture](#2-architecture)
3. [How Memory Works](#3-how-memory-works)
4. [Artifact Layers (L0–L6)](#4-artifact-layers-l0l6)
5. [Freshness & Consistency](#5-freshness--consistency)
6. [vs. Native Explore](#6-vs-native-explore)
7. [Benchmarks](#7-benchmarks)
8. [CLI Reference](#8-cli-reference)
9. [HTTP API](#9-http-api)
10. [Hook Integration (zero-config)](#10-hook-integration-zero-config)
11. [Configuration](#11-configuration)
12. [Feature Flags](#12-feature-flags)
13. [Data Model](#13-data-model)
14. [Concurrency & Multi-Agent Safety](#14-concurrency--multi-agent-safety)
15. [Observability (fork.12)](#15-observability-fork12)

---

## 1. Why Memory

Claude and other LLM agents re-read the same source files every session. On a mid-size
Rust codebase this costs ~52 000 tokens per Explore pass, and agents typically run 3–5
explore passes per task.

The memory layer solves this by:

- Storing structured project artifacts between sessions (project map, type graph, API surface, …)
- Updating **only changed files** (incremental xxh3 hashing + import-graph cascade)
- Returning a **compact, relevance-filtered context slice** (query_type routing)
- Operating **automatically** through a Claude Code PreToolUse hook — zero manual setup

---

## 2. Architecture

### Component map

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         RTK Memory Layer                                     │
│                                                                              │
│   CLI / Hook                API Server                 Core pipeline         │
│   ───────────               ──────────                 ─────────────         │
│                                                                              │
│  rtk memory explore ──┐                               ┌─► indexer.rs        │
│  rtk memory delta   ──┤  src/memory_layer/api.rs      │   (scan + hash +    │
│  rtk memory refresh ──┤  TcpListener :7700            │    cascade inval.)  │
│  rtk memory watch   ──┤  GET /v1/health               │                     │
│  rtk memory status  ──┤  POST /v1/explore   ──────────┤─► extractor.rs      │
│  rtk memory gain    ──┤  POST /v1/delta               │   (symbols, types,  │
│  rtk memory serve   ──┤  POST /v1/refresh             │    imports)         │
│  rtk memory clear   ──┤  POST /v1/context             │                     │
│  rtk memory doctor  ──┤                               │                     │
│  rtk memory setup   ──┤                               │                     │
│  rtk memory devenv  ──┘                               │                     │
│                                                        ├─► manifest.rs       │
│                                                        │   (L4: Cargo/npm/   │
│   hook: PreToolUse:Task ──► rtk-mem-context.sh         │    pyproject)       │
│   hook: PreToolUse:Task ──► rtk-block-native-explore   │                     │
│                                                        ├─► renderer.rs       │
│                                                        │   (layer selection, │
│                                                        │    text/JSON out)   │
│                                                        │                     │
│                                                        └─► cache.rs          │
│                                                            (SQLite WAL,      │
│                                                             retry, LRU evict)│
└──────────────────────────────────────────────────────────────────────────────┘

Storage: ~/.local/share/rtk/mem.db  (SQLite WAL, 5 tables)
Config:  ~/.config/rtk/config.toml  [mem] section
```

### Module responsibilities

| Module | Lines | Role |
|---|---:|---|
| `memory_layer/mod.rs` | ~2 840 | Types, CLI entry points (`run_explore` … `run_devenv`), 922 tests |
| `memory_layer/cache.rs` | ~260 | SQLite WAL open/read/write, LRU eviction, retry wrapper |
| `memory_layer/indexer.rs` | ~530 | Project scan, incremental hashing, cascade invalidation, git delta |
| `memory_layer/extractor.rs` | ~220 | Language detection, symbol extraction, type-relation extraction |
| `memory_layer/renderer.rs` | ~670 | Layer selection, context building, text/JSON rendering |
| `memory_layer/manifest.rs` | ~210 | L4: Cargo.toml / package.json / pyproject.toml parsing |
| `memory_layer/api.rs` | ~420 | Minimal HTTP/1.1 daemon, PID file, idle-timeout lifecycle |

---

## 3. How Memory Works

### Explore flow (cache-hit path)

```
rtk memory explore /project
        │
        ▼
cache.rs: load_artifact()
  └─ SQLite: SELECT content_json FROM artifacts WHERE project_id = ?
        │
        ├── Found, version matches, age < TTL ──► BuildState { cache_hit: true }
        │                                                 │
        │                                                 ▼
        │                                        indexer: artifact_dirty_count()
        │                                          compare (size, mtime_ns) pairs
        │                                                 │
        │                              0 dirty ──► CacheStatus::Hit
        │                             >0 dirty ──► CacheStatus::DirtyRebuild
        │                                                 │
        ▼                                                 ▼
Not found / stale / version mismatch            renderer: build_response()
        │                                         layers_for(query_type)
        ▼                                         apply_feature_flags()
indexer: build_state(refresh=false)              build_context_slice()
  scan_project_metadata() — gitignore-aware             │
  build_incremental_files():                             ▼
    pass-1: find metadata-changed files          print_response() / JSON
    pass-2: cascade via import graph
  extractor: analyze_file() per changed file
    symbols_regex (pub fns/structs/enums/…)
    type_relations (impl/extends/contains)
    imports (normalized, deduped, max 64)
  manifest: parse_dep_manifest()
        │
        ▼
cache.rs: store_artifact()   ← with_retry(3, exponential backoff)
cache.rs: store_artifact_edges()
cache.rs: record_cache_event()
```

### Delta flow (`--since REV`)

```
rtk memory delta /project --since HEAD~5
        │
        ▼
indexer: build_git_delta()
  git diff --name-status --find-renames HEAD~5..HEAD
  classify: Added / Modified / Removed
  hash current file (xxh3) for Modified entries
        │
        ▼
renderer: render delta output
  (no cache write — delta is always live)
```

### Watch flow (event-driven)

```
rtk memory watch /project
        │
        ▼
notify::RecommendedWatcher (kqueue / inotify / ReadDirectoryChangesW)
  debounce window = interval_secs (default 1s)
        │
  FS event ──► should_watch_abs_path() filter
        │
        ▼
indexer: build_state(refresh=false) on debounce flush
cache.rs: store_artifact()
  (incremental — only changed files rehashed)
```

---

## 4. Artifact Layers (L0–L6)

Each artifact is a `ProjectArtifact` stored as JSON in SQLite.
Content is split into 7 semantic layers; the renderer selects layers per `query_type`.

| Layer | Name | What is stored | Extraction source |
|---|---|---|---|
| **L0** | `project_map` | Entry points, hot paths (most-imported files), gitignore-aware project tree | `renderer.rs` WalkBuilder |
| **L1** | `module_index` | Per-module public export names | `pub_symbols` → `ModuleIndexEntry` |
| **L2** | `type_graph` | Type relationships: `implements`, `extends`, `contains`, `alias` | `extractor.rs` regex (Rust impl/struct/alias, TS extends/implements, Python bases) |
| **L3** | `api_surface` | Public symbol signatures per file (fn, struct, enum, trait, class, …) | `symbols_regex::RegexExtractor` (Rust/TS/JS/Python/Go) |
| **L4** | `dep_manifest` | Runtime + dev + build dependencies | `manifest.rs` (Cargo.toml / package.json / pyproject.toml) |
| **L5** | `test_map` | Test files with classification: `unit` / `integration` / `e2e` / `unknown` | `renderer.rs` path + content heuristics |
| **L6** | `change_digest` | Added / modified / removed files with xxh3 hashes | `indexer.rs` incremental scan or `build_git_delta()` |

### Layer → query_type routing

| Layer | General | Bugfix | Feature | Refactor | Incident |
|---|:---:|:---:|:---:|:---:|:---:|
| L0 project_map | ✓ | — | ✓ | — | — |
| L1 module_index | ✓ | ✓ | ✓ | ✓ | — |
| L2 type_graph | ✓ | — | ✓ | ✓ | — |
| L3 api_surface | ✓ | ✓ | ✓ | ✓ | ✓ |
| L4 dep_manifest | ✓ | — | ✓ | — | ✓ |
| L5 test_map | ✓ | ✓ | ✓ | ✓ | — |
| L6 change_digest | ✓ | ✓ | — | — | ✓ |
| top_imports | ✓ | — | ✓ | — | — |

### Detail limits

| Mode | max files (L3) | symbols/file | modules (L1) | delta entries |
|---|---:|---:|---:|---:|
| compact (default) | 5 | 8 | 10 | 8 |
| normal | 10 | 16 | 20 | 32 |
| verbose | 30 | 32 | 50 | 100 |

---

## 5. Freshness & Consistency

The memory layer enforces a strict tri-state freshness model. **Stale data is never returned.**

### States

| State | Meaning | Response |
|---|---|---|
| `FRESH` | Cache hit, no file changes detected | Serve immediately |
| `STALE` | Artifact age exceeds TTL (default 24 h) | Auto-rebuild, then serve |
| `DIRTY` | Files changed since last index (mtime/size mismatch) | Auto-rebuild or `--strict` error |

`--strict` flag (or `strict_by_default = true` in config) turns auto-rebuild into a hard error:

```
error: project has 3 dirty file(s) — use `rtk memory refresh` or remove --strict
```

### Validation fields

Every stored artifact carries: `content_hash (xxh3) + file_size + mtime_ns + ARTIFACT_VERSION`.
Any mismatch triggers cascade invalidation through the import graph.

### Cascade invalidation (E3.2)

When file A changes, all files that import A are also marked dirty and rehashed:

```
pass-1: find metadata-changed files (size or mtime_ns differs)
pass-2: for each changed file, expand via import graph (stored in artifact_edges)
        module_stems_for_path() generates: stem, rel/path, colon::sep, ./stem
        find_cascade_dependents() queries artifact_edges WHERE to_id IN (stems)
        force-rehash all dependents regardless of their own metadata
```

---

## 6. vs. Native Explore

"Native Explore" refers to the Claude Code `Task(subagent_type="Explore")` tool call,
which makes the Explore subagent read files directly from disk using `Glob` / `Grep` / `Read`.

### Mechanism comparison

| Dimension | Native Explore | RTK Memory |
|---|---|---|
| **How it reads** | `Read`, `Grep`, `Glob` tool calls per session | Pre-built SQLite artifact, single `rtk memory explore` call |
| **Context freshness** | Always reads live files | FRESH/STALE/DIRTY tri-state; auto-rebuild on stale |
| **Repeatability** | Full re-read every session | Incremental — only changed files rehashed |
| **Token volume** | ~52 000 tokens/pass (observed baseline) | ~5 700 tokens/pass (compact, General query) |
| **Latency** | Proportional to project size | Cache hit: p50 = 10 ms, p95 = 11 ms |
| **Parallelism** | Each agent reads independently | Shared SQLite cache — all agents reuse first agent's index |
| **Query filtering** | Agent decides what to read | `query_type` routing — only relevant layers returned |
| **Session persistence** | Ephemeral per-session | Cross-session artifact survives restarts |
| **Noise policy** | Raw file content | Bodies stripped, imports deduped, lock files excluded |

### What native Explore does that memory doesn't

- **Semantic search** — Explore can run `Grep` for a specific pattern; memory returns structural summaries only.
- **Full file content** — memory never stores function bodies. Use `rtk read` for full content.
- **Ad-hoc pivots** — Explore can follow any trail mid-session; memory serves what was indexed.

### What memory does that native Explore doesn't

- **Cross-session persistence** — artifacts survive process restarts.
- **Delta-only updates** — only changed files are re-analysed.
- **Parallel agent sharing** — second agent gets a cache hit from the first agent's work.
- **Token budget enforcement** — `detail=compact` hard-caps output size.
- **Automatic hook injection** — context prepended to Explore prompts transparently.

### When to prefer which

| Situation | Prefer |
|---|---|
| First time exploring an unknown codebase | Native Explore (more flexible) |
| Re-exploring a project already seen | **RTK Memory** |
| Need full function bodies | `rtk read` |
| Find all callers of a specific function | `rtk grep` / `rtk rgai` |
| Background structural summary for every agent | **RTK Memory hook** |
| Parallel agents on same project | **RTK Memory** (shared cache) |

---

## 7. Benchmarks

All numbers measured on the rtk codebase (~67 modules, ~15 000 LOC Rust).

### Latency

| Scenario | p50 ms | p95 ms | p99 ms | cache_hit_rate |
|---|---:|---:|---:|---:|
| CLI hot (cache hit) | 10.4 | 11.3 | 12.2 | 1.00 |
| API hot (daemon) | 7.3 | 8.1 | 8.7 | 1.00 |
| CLI cold (first run) | 43.0 | 57.4 | 60.2 | 0.00 |

PRD guardrail: **p95 < 200 ms** — all scenarios pass with >15× margin.

### Token savings vs. native Explore

Baseline: observed native Explore = **52 000 tokens/run** (rtk codebase, compact, General).
Memory output: **~5 700 tokens/run** (89% savings).

| Explore passes | Native tokens | Memory tokens | Saved | Savings |
|---:|---:|---:|---:|---:|
| 1 | 52 000 | 5 720 | 46 280 | 89.0% |
| 3 | 156 000 | 17 160 | 138 840 | 89.0% |
| 5 | 260 000 | 28 600 | 231 400 | 89.0% |

> `rtk memory gain` shows live savings for your current project.

### Internal latency gate (CI)

`cache_hit_latency_p95_under_200ms` — 30 iterations on a 20-module TempDir project,
hard gate 2 000 ms, soft warn ≥ 200 ms. Runs as part of `cargo test`.

---

## 8. CLI Reference

```bash
rtk memory explore  [PATH] [--query-type general|bugfix|feature|refactor|incident]
                           [--detail compact|normal|verbose]
                           [--format text|json]
                           [--strict]

rtk memory delta    [PATH] [--since REV] [--query-type …] [--detail …] [--format …]

rtk memory refresh  [PATH] [--query-type …] [--detail …] [--format …]

rtk memory watch    [PATH] [--interval-secs N] [--query-type …]

rtk memory status   [PATH]           # Show freshness, file count, age, bytes

rtk memory gain     [PATH] [-v]      # Token savings breakdown (raw vs context)

rtk memory clear    [PATH]           # Remove cached artifact from mem.db

rtk memory serve    [--port 7700] [--idle-secs 300]   # Start HTTP daemon

rtk memory install-hook              # Materialize hooks + patch settings.json

rtk memory doctor   [PATH]           # Diagnose: hooks + cache + gain + rtk binary
                                     #   exit 0 = all ok | 1 = [FAIL] | 2 = [WARN]

rtk memory setup    [PATH]           # Idempotent 4-step installer:
                    [--auto-patch]   #   [1/4] policy hooks  [2/4] mem hook
                    [--no-watch]     #   [3/4] cache build   [4/4] doctor
                                     #   → "Setup complete / with warnings"

rtk memory devenv   [PATH]           # Launch tmux session "rtk" with 3 panes:
                    [--interval N]   #   pane 0: grepai watch
                    [--session-name] #   pane 1: rtk memory watch
                                     #   pane 2: health loop (status+doctor+gain)
                                     #   fallback: prints 3 terminal commands if no tmux
```

### JSON response shape (PRD §10.2)

```json
{
  "command": "explore",
  "project_root": "/abs/path",
  "artifact_version": 4,
  "cache_status": "hit",
  "freshness": "fresh",
  "stats": { "file_count": 67, "total_bytes": 312400, "reused_entries": 67, "rehashed_entries": 0 },
  "delta": { "added": 0, "modified": 0, "removed": 0, "files": [] },
  "context": {
    "entry_points": ["src/main.rs", "Cargo.toml"],
    "module_index": [{ "module": "src/git.rs", "lang": "rust", "exports": ["run"] }],
    "type_graph": [{ "source": "MemConfig", "target": "Default", "relation": "implements", "file": "src/config.rs" }],
    "api_surface": [{ "path": "src/cache.rs", "lang": "rust", "symbols": [{ "kind": "fn", "name": "store_artifact", "sig": "pub fn store_artifact(...)" }] }],
    "dep_manifest": { "runtime": [{ "name": "rusqlite", "version": "0.31" }], "dev": [] },
    "test_map": [{ "path": "src/memory_layer/mod.rs", "kind": "unit" }]
  },
  "graph": { "nodes": 67, "edges": 142 }
}
```

---

## 9. HTTP API

Start the daemon: `rtk memory serve --port 7700 --idle-secs 300`

The daemon writes a PID file to `~/.local/share/rtk/mem-server-{port}.pid` and stops
automatically after `idle-secs` seconds with no requests.

### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/v1/health` | Liveness check — `{"status":"ok","artifact_version":4}` |
| `POST` | `/v1/explore` | Build / return cached context for `project_root` |
| `POST` | `/v1/delta` | FS delta or git delta (`"since":"HEAD~5"`) |
| `POST` | `/v1/refresh` | Force full rehash |
| `POST` | `/v1/context` | Alias for `/v1/explore` |

### Request body

```json
{
  "project_root": "/abs/path",
  "query_type": "bugfix",
  "detail": "compact",
  "since": "HEAD~5",
  "format": "json"
}
```

All fields except `project_root` are optional. `since` is only used by `/v1/delta`.

---

## 10. Hook Integration (zero-config)

Run once to activate:

```bash
rtk memory install-hook
```

This:
1. Materializes `~/.claude/hooks/rtk-mem-context.sh` (compiled into binary via `include_str!`)
2. Materializes `~/.claude/hooks/rtk-block-native-explore.sh`
3. Backs up `~/.claude/settings.json` → `settings.json.bak`
4. Patches `settings.json` to wire both hooks as `PreToolUse:Task`

### `rtk-mem-context.sh` — context injection

Fires on **every `Task` call** (all subagent types — Explore, general-purpose, Plan, it-architect-reviewer, Bash, and any future types).

```
Claude Code  ──► PreToolUse:Task hook fires
                 tool_name=Task, any subagent_type
                       │
                       ▼
              rtk memory explore $PROJECT_ROOT
                       │
              ┌────────┴──────────────┐
           cache hit                  miss / dirty
           (~10 ms)                   (rebuild, ~50 ms)
                       │
                       ▼
              Prepend context to prompt:
              "=== RTK MEMORY CONTEXT ===\n..."
                       │
                       ▼
              Return {"updatedInput": {…}} to Claude Code
```

Fail-safe: if `rtk` or `jq` is missing, hook exits 0 (pass-through, no interruption).

### `rtk-block-native-explore.sh` — Explore policy

Fires on `Task(subagent_type="Explore")`. Blocks (exit 2) unless:
- `RTK_ALLOW_NATIVE_EXPLORE=1`, or
- `RTK_BLOCK_NATIVE_EXPLORE=0`

This ensures the Explore subagent always receives the memory context before reading files.

---

## 11. Configuration

File: `~/.config/rtk/config.toml`

```toml
[mem]
cache_ttl_secs       = 86400   # Artifact TTL (seconds). Default: 24 h.
cache_max_projects   = 64      # LRU eviction limit. Default: 64 projects.
max_symbols_per_file = 64      # L3 symbol cap per file. Default: 64.

[mem.features]
type_graph           = true    # L2 type relationship extraction
test_map             = true    # L5 test file classification
dep_manifest         = true    # L4 Cargo/npm/pyproject parsing
cascade_invalidation = true    # E3.2 import-graph cascade on file change
git_delta            = true    # E3.3 --since REV git delta
strict_by_default    = false   # Fail on DIRTY instead of auto-rebuild
```

Runtime override: `RTK_MEM_DB_PATH=/tmp/test.db rtk memory explore .`

---

## 12. Feature Flags

All flags are opt-out (`true` by default) except `strict_by_default`.

| Flag | Layer | Effect when `false` |
|---|---|---|
| `type_graph` | L2 | Type relations not extracted or returned |
| `test_map` | L5 | Test file classification skipped |
| `dep_manifest` | L4 | Cargo/npm/pyproject manifest not parsed |
| `cascade_invalidation` | E3.2 | Only directly changed files rehashed |
| `git_delta` | E3.3 | `--since REV` flag rejected with error |
| `strict_by_default` | E3.4 | DIRTY/STALE triggers error without explicit `--strict` |

Flags are AND-masked via `apply_feature_flags()` — they can only disable layers,
never enable layers that `query_type` routing excluded.

---

## 13. Data Model

Database: `~/.local/share/rtk/mem.db` (SQLite WAL, `busy_timeout=2500ms`)

```sql
CREATE TABLE projects (
    project_id       TEXT    PRIMARY KEY,
    root_path        TEXT    NOT NULL UNIQUE,
    created_at       INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL    -- LRU eviction key
);

CREATE TABLE artifacts (
    project_id       TEXT    PRIMARY KEY,
    artifact_version INTEGER NOT NULL,   -- must match ARTIFACT_VERSION=4
    content_json     TEXT    NOT NULL,
    updated_at       INTEGER NOT NULL
);

CREATE TABLE artifact_edges (
    from_id   TEXT,
    to_id     TEXT,
    edge_type TEXT,
    PRIMARY KEY (from_id, to_id, edge_type)
);

CREATE TABLE cache_stats (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT    NOT NULL,
    event      TEXT    NOT NULL,   -- hit|miss|stale_rebuild|dirty_rebuild|refreshed|delta
    timestamp  INTEGER NOT NULL
);

CREATE TABLE events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  TEXT    NOT NULL,
    event_type  TEXT    NOT NULL,  -- explore|delta|refresh|api:explore|…
    timestamp   INTEGER NOT NULL,
    duration_ms INTEGER
);
```

LRU eviction: when `store_artifact` would exceed `cache_max_projects`, the project with
the oldest `last_accessed_at` is deleted by `prune_cache()`.

---

## 14. Concurrency & Multi-Agent Safety

Multiple Claude Code sessions writing to the same `mem.db` is the primary use-case.

| Mechanism | Location | What it protects |
|---|---|---|
| SQLite WAL mode | `cache.rs:configure_connection` | Concurrent readers never block a writer |
| `busy_timeout=2500ms` | `PRAGMA busy_timeout` | Writer waits up to 2.5 s before returning SQLITE_BUSY |
| `with_retry(3, exponential)` | `cache.rs:store_artifact` | Retries on SQLITE_BUSY: 100 ms → 200 ms → 400 ms |
| `INSERT OR REPLACE` | `store_artifact_inner` | Idempotent upsert — no duplicate rows from races |
| `OnceLock<()>` schema guard | `cache.rs:SCHEMA_INITIALIZED` | DDL runs once per process, thread-safe |
| Chaos tests | `mod.rs` | 8-thread store+load+delete race: no corruption, no panic |

---

## 15. Observability (fork.12)

Four commands added in fork.12 to make the memory layer self-diagnosing and visible.

### `rtk memory doctor` — health check

Runs 4 checks and exits with a machine-readable code:

| Check | Pass | Fail/Warn |
|---|---|---|
| `rtk-mem-context.sh` in `settings.json` | `[ok]` | `[FAIL]` exit 1 |
| `rtk-block-native-explore.sh` in `settings.json` | `[ok]` | `[FAIL]` exit 1 |
| Cache freshness (fresh/stale/dirty) | `[ok]` | `[WARN]` exit 2 |
| `rtk` binary in PATH | `[ok]` | `[WARN]` exit 2 |

```
[ok] hook: rtk-mem-context.sh registered (PreToolUse:Task)
[ok] hook: rtk-block-native-explore.sh registered (PreToolUse:Task)
[ok] cache: fresh, files=329, updated=42s ago
[ok] memory.gain: raw=3.1MB -> context=3.9KB (99.9% savings)
[ok] rtk binary: 0.21.1-fork.12
```

### `rtk gain -p` — memory hook row

When `--project` scope is active, a synthetic `rtk memory (hook)` row is injected
into the "By Command" table, sorted by saved bytes alongside other commands:

```
By Command
──────────────────────────────────────────────────────────────────────────
 1.  rtk memory (hook)    42   3.0MB   99.9%   0ms   ████████████████████
 2.  rtk git diff         87  18.4K    82.3%  12ms   ███
 3.  rtk cargo test       12   9.1K    90.1%  98ms   ██
```

Source: `cache.rs::get_memory_gain_stats(project_id)` — counts `cache_stats` events,
derives bytes from artifact `total_bytes` and `file_count` heuristic.

### `rtk discover` — memory miss detection

`rtk discover` now scans session JSONL files for `Task` tool-use events and checks
whether each prompt contains the `"RTK Project Memory Context"` marker.

```
Memory Context Misses (3/12)
------------------------------------------------------------
  [34c5bd09] Explore: Thoroughly explore the codebase at /...
  [66445127] general-purpose: Read ALL server-side code fil...
  [5d92169a] it-architect-reviewer: You have access to the ...

Fix: rtk memory doctor
```

JSON format includes `memory_total_tasks` and `memory_miss_count` fields (zero-suppressed).

### `rtk memory devenv` — tmux dev environment

Launches (or attaches to) a named tmux session with three panes:

```
┌─────────────────────────┬─────────────────────────┐
│  pane 0                 │  pane 1                 │
│  grepai watch           │  rtk memory watch .     │
│  (semantic index)       │  --interval 2           │
│                         ├─────────────────────────┤
│                         │  pane 2 (health loop)   │
│                         │  rtk memory status      │
│                         │  rtk memory doctor      │
│                         │  rtk gain -p            │
└─────────────────────────┴─────────────────────────┘
```

Project root is resolved by walking up parent directories to the nearest `.git`.
If tmux is not installed, three fallback terminal commands are printed instead.
