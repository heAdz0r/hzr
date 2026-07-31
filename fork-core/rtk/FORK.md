# Fork Architecture: heAdz0r/rtk

> This document details all architectural changes in the [heAdz0r/rtk](https://github.com/heAdz0r/rtk) fork compared to the upstream [rtk-ai/rtk](https://github.com/rtk-ai/rtk).

**Version**: 0.20.1-fork.4 | **Base**: rtk-ai/rtk v0.20.0 | **Commits ahead**: 21

---

## Table of Contents

1. [Design Philosophy](#design-philosophy)
2. [Architecture Overview](#architecture-overview)
3. [Semantic Search (rgai)](#1-semantic-search-rgai)
4. [Write Infrastructure](#2-write-infrastructure)
5. [Read Pipeline](#3-read-pipeline)
6. [Semantic Parity System](#4-semantic-parity-system)
7. [Hook Audit & Classification](#5-hook-audit--classification)
8. [Module Map](#module-map)
9. [Data Flow Diagrams](#data-flow-diagrams)
10. [Development Roadmap](#development-roadmap)

---

## Design Philosophy

The upstream rtk applies **output filtering** — it compresses what the LLM sees after a command runs. This fork extends the concept to **every layer of LLM-tool interaction**:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Token Optimization Layers                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Layer 5: DISCOVERY   — rtk discover, rtk gain                  │
│           Find missed opportunities, measure ROI                │
│                                                                 │
│  Layer 4: HOOK        — rtk-rewrite.sh + audit                  │
│           Transparent interception, classification, logging     │
│                                                                 │
│  Layer 3: SEARCH      — rtk rgai (semantic > regex > raw)       │
│           Find the right code faster = fewer search iterations  │
│                                                                 │
│  Layer 2: READ        — modular pipeline, cache, digest         │
│           Read only what matters, in the format that saves most │
│                                                                 │
│  Layer 1: WRITE       — atomic I/O, CAS, minimal output         │
│           Crash-safe mutations with sub-10-token confirmations  │
│                                                                 │
│  Layer 0: OUTPUT      — smart filtering (upstream core)         │
│           Git, test, lint, container output compression         │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Core principles**:
1. **Every interaction is optimizable** — not just command output, but search queries, file reads, and writes
2. **Safety over savings** — crash-safe writes, semantic parity for mutating commands
3. **Zero semantic drift** — LLM agent must not be confused by command substitution
4. **Graceful degradation** — advanced features fall back to working basics, never crash

---

## Architecture Overview

### Fork Module Dependencies

```
main.rs (CLI entry + routing)
│
├── Search Layer
│   ├── rgai_cmd.rs ─────── Semantic search engine (63.5KB)
│   │   ├── grepai.rs        External semantic service delegation
│   │   ├── ripgrep backend  Fast exact-match fallback
│   │   └── built-in walker  Guaranteed availability
│   └── grep_cmd.rs ──────── Regex search (rg -> grep fallback)
│
├── Read Layer
│   ├── read.rs ─────────── Orchestrator (dispatch by mode)
│   │   ├── read_types.rs    ReadMode, ReadRequest, ReadContext
│   │   ├── read_source.rs   Bytes/text I/O, line-range logic
│   │   ├── read_cache.rs    Persistent cache (key, load/store/prune)
│   │   ├── read_digest.rs   Smart digests (CSV/TSV/lock/package.json)
│   │   ├── read_render.rs   Text/JSON renderers
│   │   ├── read_symbols.rs  Symbol extraction traits + routing
│   │   └── read_changed.rs  Git diff provider + hunk parser
│   ├── filter.rs ────────── Language-aware code filtering
│   └── summary.rs ───────── Heuristic code summary
│
├── Write Layer
│   ├── write_cmd.rs ──────── rtk write replace/patch/set
│   ├── write_core.rs ─────── AtomicWriter, WriteOptions, WriteStats
│   └── write_semantics.rs ── Semantic contracts, mutation classifier
│
├── Git Layer (enhanced)
│   └── git.rs ────────────── All git ops + semantic parity hardening
│
├── Hook Layer (enhanced)
│   └── hooks/rtk-rewrite.sh  Rewrite + audit + classification
│
├── Output Layer (upstream core)
│   ├── lint_cmd.rs, tsc_cmd.rs, next_cmd.rs, ...
│   ├── ruff_cmd.rs, pytest_cmd.rs, pip_cmd.rs
│   ├── go_cmd.rs, golangci_cmd.rs
│   ├── cargo_cmd.rs, gh_cmd.rs, pnpm_cmd.rs
│   └── container.rs (docker/kubectl)
│
└── Analytics Layer
    ├── tracking.rs ───────── SQLite metrics (3-tier fallback parser)
    ├── gain.rs ───────────── Colored dashboard + efficiency meter
    └── discover/ ─────────── Claude Code session history analysis
```

---

## 1. Semantic Search (rgai)

**Module**: `src/rgai_cmd.rs` (63.5KB) + `src/grepai.rs`

### Problem

Regex grep is a blunt instrument for LLM-driven development. The agent often needs to find code by **concept** ("authentication logic", "database retry"), not by **pattern** (`auth.*token`). Poor search = more iterations = more tokens wasted.

### Solution: Multi-Tier Semantic Search

```
rtk rgai "auth token refresh"
         │
         ▼
    ┌─── Tier 1: grepai delegation ───┐
    │    External semantic service      │
    │    Full NLP understanding         │
    └──────── success? ────────────────┘
              │ no
              ▼
    ┌─── Tier 2: ripgrep backend ─────┐
    │    Fast regex matching            │
    │    Query decomposition + scoring  │
    └──────── success? ────────────────┘
              │ no
              ▼
    ┌─── Tier 3: built-in walker ─────┐
    │    walkdir + pattern matching     │
    │    Always available               │
    └─────────────────────────────────┘
```

### Key Features

- **Intent decomposition**: splits "auth token refresh" into scored search terms
- **Relevance ranking**: results scored by match quality, not just occurrence
- **Compact mode**: `--compact` returns top 5 files, 1 snippet each (~87% token reduction)
- **Output formats**: text (default), JSON, JSON-compact
- **Gitignore-aware**: respects `.gitignore` patterns via `ignore` crate

### Hook Integration

The hook enforces `rgai > grep > raw` search ladder:
- `grepai search <query>` → `rtk rgai <query>`
- `rgai <query>` → `rtk rgai <query>`
- `rg/grep <pattern>` → `rtk grep <pattern>`

---

## 2. Write Infrastructure

**Modules**: `src/write_core.rs`, `src/write_cmd.rs`, `src/write_semantics.rs`

### Problem

LLM agents mutate files through `echo >`, `sed -i`, `cat << EOF > file`, `perl -pi -e`. These commands:
1. Produce verbose output (wasted tokens)
2. Are not crash-safe (partial writes on interruption)
3. Have no idempotency (rewriting identical content touches disk)
4. Lack concurrency control (multiple agents can clobber each other)

### Solution: AtomicWriter

```
write_core.rs::AtomicWriter
│
├── DurabilityMode::Durable (default)
│   tempfile(same_dir) → BufWriter → flush → sync_data → rename → fsync(parent)
│
├── DurabilityMode::Fast
│   tempfile(same_dir) → BufWriter → flush → rename
│
├── Idempotent check
│   size mismatch → changed (fast path)
│   content equal → skip write entirely
│
└── CAS (Compare-and-Swap)
    Verify mtime/size/hash before write → reject if changed externally
```

### Write Commands

| Command | Purpose | Example |
|---------|---------|---------|
| `write replace` | String substitution | `rtk write replace f.rs --from "old" --to "new"` |
| `write patch` | Semantic patch (alias) | `rtk write patch f.rs --from "old" --to "new"` |
| `write set` | Structured key-value | `rtk write set config.toml --key "port" --value 8080` |

### Failure Semantics

1. Error before `rename` → target file **untouched** (always safe)
2. Error after `rename` → target in last consistent state
3. Temp file cleanup: best-effort
4. Exit code: non-zero on any failure

### Token Output

```bash
# Default mode: ~8 tokens per operation
ok replace: 1 occurrence(s)

# --json mode: machine-parseable
{"ok":true,"applied":1,"file":"src/config.rs"}

# --quiet mode: 0 tokens on success
# (empty stdout)
```

### Hook Rewrite Rules

```
sed -i 's/old/new/g' file    →  rtk write replace file --from 'old' --to 'new' --all
perl -pi -e 's/old/new/' file →  rtk write replace file --from 'old' --to 'new'
```

---

## 3. Read Pipeline

**Modules**: `src/read.rs` (orchestrator) + 7 sub-modules

### Problem

The original `read.rs` was a 1081-line monolith mixing I/O, caching, digesting, rendering, and orchestration. Adding new modes (outline, symbols, changed) would balloon it further.

### Solution: Modular Decomposition

| Module | Responsibility | Size Target |
|--------|---------------|-------------|
| `read.rs` | Orchestrator: route by mode, fallback | ~200 lines |
| `read_types.rs` | `ReadMode`, `ReadRequest`, `ReadContext` | ~80 lines |
| `read_source.rs` | Bytes/text I/O, line-range, stdin/file | ~200 lines |
| `read_cache.rs` | Cache key (path+size+mtime+ino+opts), load/store/prune | ~150 lines |
| `read_digest.rs` | CSV/TSV digest, format-aware summaries | ~300 lines |
| `read_render.rs` | Text/JSON renderers, line-number policies | ~150 lines |
| `read_symbols.rs` | `SymbolExtractor` trait, backend routing | ~100 lines |
| `read_changed.rs` | `DiffProvider` trait, git hunk parser | ~200 lines |

### Read Modes

```
rtk read file.rs
         │
         ├── --level none      → Exact bytes (cat parity)
         ├── --level minimal   → Light filtering (default)
         ├── --level aggressive → Signatures only, bodies stripped
         ├── --outline          → Structural map with line spans
         ├── --symbols          → JSON symbol index (versioned schema)
         ├── --changed          → Git working tree hunks only
         └── --since HEAD~3    → Diff hunks relative to revision
```

### Smart Digest Strategies

Auto-triggered by filename pattern:

| Pattern | Strategy | Token Savings |
|---------|----------|--------------|
| `*.lock`, `pnpm-lock.yaml` | Package count + version summary | ~95% |
| `package.json` | Scripts + deps + devDeps summary | ~70% |
| `Cargo.toml` | Deps + features summary | ~70% |
| `.env*` | Keys only (values masked) | ~60% |
| `Dockerfile` | Key instructions summary | ~50% |
| `*.csv`, `*.tsv` | Schema + sampling + numeric stats | ~80% |
| `*.md` | Headers + section preview | ~60% |

### Symbol Extraction Backend Strategy

```
SymbolExtractor trait
├── RegexExtractor     — fast, dependency-light, ~85% accuracy
├── TreeSitterExtractor — precise boundaries, language-grammar based
└── auto (default)     — tree-sitter if supported, regex fallback
```

---

## 4. Semantic Parity System

**Context**: `src/git.rs`, `src/write_semantics.rs`

### Problem

The upstream git wrappers sometimes return `Ok(())` even when the underlying git command fails. This causes "LLM confusion" — the agent believes the operation succeeded when it didn't.

### Identified P0 Risk Points

| Function | Line | Issue |
|----------|------|-------|
| `run_commit` | git.rs:660 | Non-zero exit swallowed |
| `run_push` | git.rs:724 | Non-zero exit swallowed |
| `run_pull` | git.rs:785 | Non-zero exit swallowed |
| `run_branch` | git.rs:870 | Non-zero on action-mode |
| `run_fetch` | git.rs:998 | Non-zero exit swallowed |
| `run_stash` | git.rs:1042 | Mutating branch failures |
| `run_worktree` | git.rs:1180 | Action-mode failures |

### Solution: Parity Contract

Every mutating command wrapper guarantees:

1. **Exit code** = native command exit code (100% match)
2. **Side effects** = identical repo state (staging, commits, refs)
3. **Failure stderr** = key diagnostic signals preserved
4. **Success output** = compact format allowed (token savings)

### Classification System

```rust
// write_semantics.rs
enum CommandClass {
    ReadOnly,    // auto-rewrite always safe
    Mutating,    // requires parity proof before rewrite
}
```

The hook reads this classification:
- `RTK_REWRITE_MUTATING=0` (default): pass-through mutating commands
- `RTK_REWRITE_MUTATING=1`: rewrite after parity benchmark passes

---

## 5. Hook Audit & Classification

**File**: `hooks/rtk-rewrite.sh` (~315 lines)

### Enhancement Over Upstream

| Feature | Upstream | Fork |
|---------|---------|------|
| Command rewriting | Yes | Yes |
| Audit logging | No | Yes — every rewrite logged with timestamp + class |
| Command classification | No | Yes — `read_only` / `mutating` tagging |
| Mutating guardrails | No | Yes — configurable policy (`RTK_REWRITE_MUTATING`) |
| `sed`/`perl` rewriting | No | Yes — `sed -i` → `rtk write replace` |
| Search ladder | Partial | Full — `rgai > grep > raw` enforcement |

### Audit Log Format

```
~/.local/share/rtk/hook-audit.log

TIMESTAMP           ACTION   CLASS       ORIGINAL → REWRITTEN
2026-02-17T10:30:15 REWRITE  read_only   git status → rtk git status
2026-02-17T10:30:22 REWRITE  mutating    git push → rtk git push
2026-02-17T10:31:01 REWRITE  read_only   cat src/main.rs → rtk read src/main.rs
2026-02-17T10:31:05 PASS     -           rtk rgai "query" (already rtk)
2026-02-17T10:31:15 REWRITE  read_only   rg "pattern" → rtk grep "pattern"
```

### Rewrite Decision Tree

```
Input command
│
├── Already starts with "rtk"? → PASS (no rewrite)
├── Contains heredoc (<<)?      → PASS (too complex)
│
├── Is git command?
│   ├── read_only (status/log/diff/show) → REWRITE
│   └── mutating (add/commit/push/pull)  → check RTK_REWRITE_MUTATING
│       ├── =1 → REWRITE
│       └── =0 → REWRITE (with audit class=mutating)
│
├── Is search (rg/grep/rgai)? → REWRITE to rtk grep/rgai
├── Is read (cat/head)?       → REWRITE to rtk read
├── Is write (sed/perl)?      → REWRITE to rtk write replace
│
└── No match? → PASS
```

---

## Module Map

### Fork-Specific Modules (not in upstream)

| Module | Lines | Purpose |
|--------|-------|---------|
| `rgai_cmd.rs` | ~1500 | Semantic code search engine |
| `grepai.rs` | ~200 | External semantic service delegation |
| `write_core.rs` | ~250 | AtomicWriter, CAS, durability modes |
| `write_cmd.rs` | ~500 | `rtk write replace/patch/set` |
| `write_semantics.rs` | ~80 | Mutation classification, parity contracts |
| `read_types.rs` | ~80 | ReadMode, ReadRequest, ReadContext |
| `read_source.rs` | ~200 | Bytes/text I/O, line-range logic |
| `read_cache.rs` | ~150 | Persistent read cache |
| `read_digest.rs` | ~700 | Smart format digests (CSV/TSV/lock/etc.) |
| `read_render.rs` | ~150 | Text/JSON renderers |
| `read_symbols.rs` | ~100 | Symbol extraction traits |
| `read_changed.rs` | ~350 | Git diff provider, hunk parser |

### Enhanced Modules (modified from upstream)

| Module | Change | Impact |
|--------|--------|--------|
| `git.rs` | Semantic parity hardening | Exit code fidelity for all mutating ops |
| `init.rs` | Hook audit, settings.json auto-patch | Frictionless installation |
| `gain.rs` | Colored dashboard, efficiency meter | Better analytics UX |
| `tracking.rs` | 3-tier fallback parser, exec time | Robust metrics |
| `hooks/rtk-rewrite.sh` | Audit + classification + sed/perl rewrite | Full I/O coverage |

---

## Data Flow Diagrams

### Search Flow (rgai)

```
User intent: "find auth token refresh logic"
     │
     ▼
rtk rgai "auth token refresh"
     │
     ├─ [Tier 1] grepai service available?
     │   Yes → delegate, return ranked results
     │   No ↓
     ├─ [Tier 2] ripgrep available?
     │   Yes → decompose query → run rg → score → rank
     │   No ↓
     └─ [Tier 3] built-in walker
         walkdir + pattern match → score → rank
     │
     ▼
Compact output: top 5 files, 1 snippet each
Token cost: ~200 tokens (vs ~1600 for raw grep)
```

### Write Flow (atomic)

```
rtk write replace file.rs --from "old" --to "new"
     │
     ├─ Read file content
     ├─ Check idempotency (content unchanged? → skip)
     ├─ Apply replacement (single-pass)
     ├─ CAS check (if --if-match-mtime/hash)
     │
     ├─ AtomicWriter::write()
     │   ├─ tempfile in same directory
     │   ├─ BufWriter → flush
     │   ├─ sync_data (if Durable mode)
     │   ├─ rename (atomic)
     │   └─ fsync(parent dir) (if Durable mode)
     │
     └─ Output: "ok replace: 1 occurrence(s)" (~8 tokens)
```

### Read Flow (modular pipeline)

```
rtk read file.rs --outline
     │
     ├─ read.rs: parse mode, check cache
     │
     ├─ read_source.rs: load bytes/text
     │   ├─ Binary? → hex preview
     │   └─ Text → apply line range (--from/--to)
     │
     ├─ Mode dispatch:
     │   ├─ full     → filter.rs → read_render.rs
     │   ├─ outline  → read_symbols.rs → outline renderer
     │   ├─ symbols  → read_symbols.rs → JSON renderer
     │   ├─ changed  → read_changed.rs → hunk renderer
     │   └─ since    → read_changed.rs → hunk renderer
     │
     ├─ read_digest.rs: auto-digest for known formats
     │
     ├─ read_cache.rs: store result (if cacheable)
     │
     └─ Output to stdout
```

---

## Development Roadmap

### Completed

- [x] `rtk rgai` — semantic code search with multi-tier fallback
- [x] `write_core.rs` — AtomicWriter with durability modes
- [x] `write_cmd.rs` — `rtk write replace/patch/set`
- [x] `write_semantics.rs` — mutation classification
- [x] Hook audit logging with command classification
- [x] `sed`/`perl` → `rtk write replace` hook rules
- [x] Search ladder enforcement in hooks
- [x] Colored `rtk gain` dashboard
- [x] Read cache with versioned keys
- [x] CSV/TSV digest strategies
- [x] Git semantic parity hardening (P0 exit code propagation)

### In Progress (PR-W6)

- [ ] Single-pass replace (remove double scan)
- [ ] Remove double idempotent-check
- [ ] Clean LLM-facing output (<=8 tokens per operation)
- [ ] `--json`/`--quiet` output modes for write
- [ ] CAS CLI flags (`--if-match-mtime`, `--if-match-hash`)

### Planned

**Read decomposition** (PR-1 through PR-7):
- [ ] PR-1: Golden tests for current behavior
- [ ] PR-2: Monolith decomposition (read.rs → 6 modules)
- [ ] PR-3: `--outline` + `--symbols` (regex backend)
- [ ] PR-4: Tree-sitter backend
- [ ] PR-5: `--changed` + `--since`
- [ ] PR-6: Format-specific digests (lock, env, Dockerfile, md)
- [ ] PR-7: Optional dedup repetitive blocks

**Write enhancements**:
- [ ] Batch mode (`rtk write batch --plan <json>`)
- [ ] Symlink/TOCTOU protection
- [ ] Fast/durable policy per file type
- [ ] replace/patch deduplication

---

## Benchmarks & Quality Gates

### Write Path Benchmarks

| Scenario | Target |
|----------|--------|
| Unchanged file (idempotent skip) | < 2ms p50 for <= 128KB |
| Changed file (durable) | <= 1.25x native safe baseline |
| Changed file (fast) | Measurably faster than durable on small files |
| Corruption rate (fault injection) | 0% |
| LLM output tokens per success | <= 8 |

### Semantic Parity Benchmarks

| Metric | Target |
|--------|--------|
| Exit code match rate | 100% |
| Side-effect match rate | 100% |
| Stderr key-signal match rate | >= 99% |
| Post-rewrite retry rate | <= native baseline |

### Test Infrastructure

```bash
cargo test                      # 105+ unit tests, 25+ files
bash scripts/test-all.sh        # 69 smoke test assertions
cargo fmt --all --check && cargo clippy --all-targets && cargo test  # Pre-commit gate
```

---

## Version History (Fork Releases)

| Version | Key Changes |
|---------|-------------|
| **0.20.1-fork.4** | Write improvements, hook audit, cache, semantic parity |
| **0.20.1-fork.3** | rgai compact output optimization (~87% reduction) |
| **0.20.1-fork.2** | Hook audit logging, search policy enforcement |
| **0.20.1-fork.1** | rgai semantic search, read cache, digest strategies |

See [CHANGELOG.md](CHANGELOG.md) for full version history.
