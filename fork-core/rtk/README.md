<div align="center">

# rtk (fork) — Rust Token Killer

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)
[![Fork of rtk-ai/rtk](https://img.shields.io/badge/fork%20of-rtk--ai%2Frtk-blue)](https://github.com/rtk-ai/rtk)
[![Version](https://img.shields.io/badge/version-0.21.1--fork.12-green)](https://github.com/heAdz0r/rtk)

**High-performance CLI proxy that minimizes LLM token consumption —<br>from file reads to git operations to semantic code search.**

[Upstream](https://github.com/rtk-ai/rtk) ·
[This Fork](https://github.com/heAdz0r/rtk) ·
[Architecture](FORK.md) ·
[Install](#installation) ·
[Commands](#command-reference)

</div>

> **Fork**: [heAdz0r/rtk](https://github.com/heAdz0r/rtk) — based on [rtk-ai/rtk](https://github.com/rtk-ai/rtk) by Patrick Szymkowiak.
> This fork introduces **cardinal architectural changes** to achieve token savings at every layer of the LLM interaction pipeline.

---

## Why This Fork

The upstream rtk is a solid token-reduction proxy. This fork takes the concept further — **architectural changes at every layer** turn rtk from a filter tool into a comprehensive LLM I/O optimization engine:

| Layer | Upstream rtk | This Fork |
|-------|-------------|-----------|
| **Search** | `grep` with regex | `rgai` — semantic intent-aware search with multi-tier fallback |
| **Read** | Basic filtering + line ranges | Modular pipeline: cache, digest, symbols, changed, outline modes |
| **Memory** | None | Shared project artifacts + incremental deltas (`rtk memory`) |
| **Write** | No write path | Atomic I/O with `write_core`, CAS, durability modes, `rtk write` command |
| **Git** | Compact output | + Semantic parity for mutating commands (exit code + side-effect fidelity) |
| **Hooks** | Auto-rewrite | + Audit logging, mutating command guardrails, classification system |
| **Analytics** | Basic gain stats | + Colored dashboard, efficiency meter, per-project breakdown |

**Result**: **60-90% token savings** on every operation, with crash-safe writes and zero semantic drift.

---

## Key Fork Features

> Five architectural additions that transform rtk from a filter proxy into a full LLM I/O engine.

### 1. `rtk rgai` — Semantic Code Search

Unlike regex grep, `rgai` understands **intent** — search for concepts, not just patterns.

```bash
rtk rgai "auth token refresh"         # Find auth-related code by meaning
rtk rgai "error handling in API"      # Concept search, not string matching
rtk rgai "database connection pool"   # Works across naming conventions
```

**Multi-tier execution**:
1. **grepai delegation** — external semantic service (if available)
2. **ripgrep backend** — fast exact-match fallback
3. **built-in walker** — guaranteed availability

**Compact mode**: Top 5 files, 1 snippet each — maximum token savings (~87% reduction).

**Search priority policy**: `rgai > grep > raw`. The hook system enforces this automatically.

### 2. Read Pipeline — Modular Architecture

The monolithic `read.rs` (1081 lines) is decomposed into a modular pipeline:

```
read.rs (orchestrator)
├── read_source.rs   — bytes/text I/O, line-range logic
├── read_cache.rs    — persistent read cache with versioned keys
├── read_digest.rs   — smart digests (CSV/TSV/lock files/package.json)
├── read_render.rs   — text/JSON renderers, line-number policies
├── read_symbols.rs  — symbol extraction (regex + tree-sitter backends)
└── read_changed.rs  — git-aware diff reading with hunk parser
```

**Target read modes**:
```bash
rtk read file.rs                      # Smart filtered read (default)
rtk read file.rs --outline            # Structural map with line spans
rtk read file.rs --symbols            # Machine-readable JSON symbol index
rtk read file.rs --changed            # Only modified hunks (git working tree)
rtk read file.rs --since HEAD~3       # Changes relative to a revision
rtk read file.rs -l aggressive        # Signatures only, bodies stripped
rtk read file.rs --from 120 --to 220  # Exact line range
```

**Smart digests** auto-detect format and produce optimal summaries for:
lock files, `package.json`, `Cargo.toml`, `.env`, `Dockerfile`, `tsconfig.json`, markdown.

### 3. Write Infrastructure — Atomic I/O Engine

No more `echo >` or `sed -i`. The write path is a crash-safe, atomic file operation engine.

```bash
rtk write replace file.rs --from "old" --to "new"        # Safe string replace
rtk write replace file.rs --from "old" --to "new" --all  # Replace all occurrences
rtk write patch file.rs --from "old" --to "new"           # Semantic patch
rtk write set config.toml --key "server.port" --value 8080  # Structured config edit
rtk write set package.json --key "version" --value "2.0.0"  # JSON key-value
```

**Core architecture** (`write_core.rs`):
- **AtomicWriter**: tempfile-in-same-dir → write → flush → sync_data → rename → fsync(parent)
- **Durability modes**: `Durable` (full fsync, default) / `Fast` (skip non-critical fsyncs)
- **Idempotent writes**: disk untouched if content unchanged
- **Compare-and-swap (CAS)**: optimistic concurrency with mtime/hash checks
- **Output modes**: `--quiet` (silent), default (concise), `--json` (machine-readable)

**Failure semantics**: any error before rename → target file untouched. Always consistent state.

### 4. Semantic Parity for Mutating Commands

The upstream wraps git commands but can silently swallow non-zero exit codes. This fork guarantees:

1. **Exit code parity** — wrapper exit code = native command exit code
2. **Side-effect fidelity** — staging, commit, push state matches native
3. **Error preservation** — failure stderr contains key diagnostic signals
4. **Classification system** — every command tagged `read_only` or `mutating`

The hook system uses this classification to apply guardrails:
- `read_only` commands: auto-rewrite always safe
- `mutating` commands: guarded rewrite with proven parity

### 5. Hook Audit System

Every command rewrite is logged and classified:

```bash
# Audit log at ~/.local/share/rtk/hook-audit.log
2026-02-17T10:30:15 REWRITE read_only  git status → rtk git status
2026-02-17T10:30:22 REWRITE mutating   git push → rtk git push
2026-02-17T10:31:01 REWRITE read_only  cat src/main.rs → rtk read src/main.rs
2026-02-17T10:31:15 REWRITE read_only  rg "pattern" → rtk grep "pattern"
```

Classification-based policy: read-only commands rewrite freely, mutating commands require parity proof.

---

## Token Savings

<details open>
<summary><strong>Typical 30-min Claude Code session</strong></summary>

| Operation | Frequency | Without rtk | With rtk (fork) | Savings |
|-----------|-----------|-------------|-----------------|---------|
| `cat` / `read` | 20x | 40,000 | 12,000 | **-70%** |
| `grep` / `rgai` | 8x | 16,000 | 3,200 | **-80%** |
| `git status` | 10x | 3,000 | 600 | **-80%** |
| `git diff` | 5x | 10,000 | 2,500 | **-75%** |
| `git add/commit/push` | 8x | 1,600 | 120 | **-92%** |
| `ls` / `tree` | 10x | 2,000 | 400 | **-80%** |
| `npm test` / `cargo test` | 5x | 25,000 | 2,500 | **-90%** |
| `write replace/set` | 5x | 1,500 | 40 | **-97%** |
| **Total** | | **~100,000** | **~21,400** | **~79%** |

> With `rtk write`, file mutations that used to produce verbose `sed`/`echo` output now emit `ok replace: 1 occurrence(s)` — 97% reduction.

</details>

<details>
<summary><strong>Real session data (3 days)</strong></summary>

```
📊 RTK Token Savings (3-day session)
════════════════════════════════════════

Total commands:    133
Input tokens:      30.5K
Output tokens:     10.7K
Tokens saved:      25.3K (83.0%)

By Command:
────────────────────────────────────────
Command               Count      Saved     Avg%
rtk git status           41      17.4K    82.9%
rtk git push             54       3.4K    91.6%
rtk grep                 15       3.2K    26.5%
rtk ls                   23       1.4K    37.2%
```

</details>

---

## Installation

### Pre-Installation Check

```bash
rtk --version        # Check if already installed
rtk gain             # Verify it's Token Killer (not Type Kit)
```

> **Name collision**: Two packages named "rtk" exist. This is **Rust Token Killer** (rtk-ai/rtk), not Rust Type Kit (reachingforthejack/rtk). If `rtk gain` doesn't work — you have the wrong one.

### From This Fork

```bash
# Build from source (recommended for fork features)
git clone https://github.com/heAdz0r/rtk.git
cd rtk
cargo install --path .

# Native build pipeline (replacement for ./rtk-build.sh)
rtk build sh --set-version 0.44.1-fork.1
# or (from repo root, same behavior)
./rtk-build.sh --set-version 0.44.1-fork.1

# Verify
rtk --version   # Should show 0.44.1-fork.1
rtk gain         # Token savings stats
```

### From Upstream

```bash
# Quick install (upstream version, without fork features)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh

# Or via cargo
cargo install --git https://github.com/rtk-ai/rtk
```

### Pre-built Binaries (Upstream)

Download from [rtk-ai/releases](https://github.com/rtk-ai/rtk/releases):
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-gnu.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

> Note: pre-built binaries are upstream-only. Fork features require building from source.

---

## Quick Start

```bash
# 1. Verify installation
rtk gain

# 2. Initialize for Claude Code (hook-first mode, recommended)
rtk init --global
# Installs the rewrite hook, creates a slim RTK.md, and offers to patch settings.json

# Codex CLI, global or project-local
rtk init --global --codex
rtk init --codex

# 3. Test
rtk git status       # Compact output
rtk init --show      # Verify hook installed
```

---

## Command Reference

<details open>
<summary><strong>40+ commands across 10 ecosystems</strong></summary>

### Search (rgai-first policy)
```bash
rtk rgai "auth token refresh"   # Semantic search (FIRST CHOICE)
rtk rgai query --compact         # Top 5 files, 1 snippet each
rtk grep "pattern" .             # Regex search (rg -> grep fallback)
rtk find "*.rs" .                # Compact find results
```

### Files — Read
```bash
rtk read file.rs                        # Smart filtered read
rtk read file.rs -l aggressive          # Signatures only
rtk read file.rs --from 120 --to 220    # Line range
rtk read file.rs --level none           # Exact (no filtering)
rtk read -                              # Read from stdin
rtk smart file.rs                       # 2-line heuristic summary
rtk ls .                                # Token-optimized directory tree
```

### Files — Write (fork-only)
```bash
rtk write replace file.rs --from "old" --to "new"         # Single replace
rtk write replace file.rs --from "old" --to "new" --all   # Replace all
rtk write patch file.rs --old "block A" --new "block B"    # Exact hunk replacement
rtk write set config.toml --key "port" --value 8080        # TOML key-value
rtk write set package.json --key "version" --value "2.0"   # JSON key-value
rtk write batch --plan '[{"op":"replace","file":"f","from":"a","to":"b"}]'  # Multi-op batch
rtk write replace file.rs --from "a" --to "b" --dry-run   # Preview without writing
rtk write replace file.rs --from "a" --to "b" --output json  # Machine-readable output
```

### Git
```bash
rtk git status                  # Compact status
rtk git log -n 10               # One-line commits
rtk git diff                    # Condensed diff
rtk git add                     # → "ok ✓"
rtk git commit -m "msg"         # → "ok ✓ abc1234"
rtk git push                    # → "ok ✓ main"
rtk git pull                    # → "ok ✓ 3 files +10 -2"
```

### JavaScript / TypeScript
```bash
rtk lint                         # ESLint/Biome grouped by rule (84% reduction)
rtk tsc                          # TypeScript errors by file (83% reduction)
rtk next build                   # Next.js build compact (87% reduction)
rtk prettier --check .           # Files needing formatting (70% reduction)
rtk vitest run                   # Test failures only (99.5% reduction)
rtk playwright test              # E2E failures only (94% reduction)
rtk prisma generate              # No ASCII art (88% reduction)
```

### Python
```bash
rtk ruff check                   # Linting with JSON (80%+ reduction)
rtk ruff format                  # Format check
rtk pytest                       # Failures only (90%+ reduction)
rtk pip list                     # Package list (auto-detect uv)
```

### Go
```bash
rtk go test                      # NDJSON parser (90%+ reduction)
rtk go build                     # Errors only (80% reduction)
rtk go vet                       # Issues only (75% reduction)
rtk golangci-lint run            # Grouped by rule (85% reduction)
```

### Rust / Cargo
```bash
rtk cargo test                   # Failures only
rtk cargo build                  # Compact output
rtk cargo clippy                 # Lint summary
rtk cargo install <crate>        # Filtered install output
```

### Containers
```bash
rtk docker ps                    # Compact container list
rtk docker images                # Compact image list
rtk docker logs <container>      # Deduplicated logs
rtk kubectl pods                 # Compact pod list
```

### GitHub CLI
```bash
rtk gh pr list                   # Compact PR listing
rtk gh pr view 42                # PR details + checks summary
rtk gh issue list                # Compact issue listing
rtk gh run list                  # Workflow run status
```

### Analytics & Discovery
```bash
rtk gain                         # Token savings summary
rtk gain --graph                 # ASCII graph (30 days)
rtk gain --history               # Recent command history
rtk gain -p                      # Per-project scope + memory hook row in table
rtk gain --daily                 # Day-by-day breakdown
rtk gain --quota --tier 20x      # Monthly quota analysis

rtk discover                     # Find missed savings + memory context misses
rtk discover --all               # All Claude Code projects
rtk discover --format json       # Machine-readable (memory miss count in JSON)

rtk memory explore .             # Build/reuse shared context artifact
rtk memory delta .               # Emit changed files/modules only
rtk memory refresh .             # Force full reindex and persist
rtk memory watch . --interval 2  # Continuous low-noise delta stream
rtk memory doctor                # Diagnose hooks + cache health (exit 0/1/2)
rtk memory setup --auto-patch    # Idempotent installer: hooks + cache + doctor
rtk memory gain                  # Token savings: raw source → compact context
rtk memory devenv                # Launch tmux session: watchers + health loop

rtk json config.json             # Structure without values
rtk deps                         # Dependencies summary
rtk env -f AWS                   # Filtered env vars
```

### Utility
```bash
rtk raw <cmd>                    # Exact output with tracking (alias: proxy)
rtk config                       # Show config
rtk wget https://example.com     # Download, strip progress bars
rtk ssh user@host "cmd"          # SSH output filtering
```

</details>

---

## Auto-Rewrite Hook

The hook delegates supported-command decisions to `rtk rewrite` and leaves
unsupported or unsafe command shapes unchanged.

```bash
# Install
rtk init --global                 # Hook + RTK.md
rtk init --global --auto-patch    # Auto-patch settings.json
rtk init --global --dry-run       # Preview without writing

# Verify
rtk init --show                   # Show hook status
```

### What Gets Rewritten

| Raw Command | Rewritten To |
|-------------|-------------|
| `git status/diff/log/add/commit/push/pull` | `rtk git ...` |
| `cat <file>` / `head -N <file>` | `rtk read <file>` |
| `sed -i 's/a/b/' file` / `perl -pi -e 's/a/b/' file` | `rtk write replace file --from 'a' --to 'b'` |
| `grepai/rgai <query>` | `rtk rgai <query>` |
| `rg <pattern>` / `grep <pattern>` | `rtk rg <pattern>` / `rtk grep <pattern>` |
| `ls` | `rtk ls` |
| `cargo test/build/clippy` | `rtk cargo ...` |
| `gh pr/issue/run` | `rtk gh ...` |
| `vitest/tsc/eslint/prettier/playwright/prisma` | `rtk ...` |
| `ruff/pytest/pip` | `rtk ...` |
| `go test/build/vet` / `golangci-lint` | `rtk ...` |
| `docker/kubectl` | `rtk ...` |

Commands already using `rtk`, heredocs, and unrecognized commands pass through unchanged.
Commands whose stdout is redirected to a file also pass through unchanged, so
truncation markers and summaries are never persisted as command data.

Independent commands in multiline Bash blocks are rewritten independently.
Pipeline producers are rewritten only when all following stages are plain
display-only `head`, `tail`, or `cat` consumers. Parsing, transforming,
redirected, or otherwise decorated consumers keep the producer native.

### Exact Output And Files

Filtered RTK output is intended for agent context. Use raw mode whenever
completeness is part of the result: generated files, checksums, counts, machine
parsing, or exact pipelines.

Automatic bypass applies to native commands processed by the rewrite hook.
An explicitly selected filter such as `rtk rg ... > file` remains filtered;
write `rtk raw rg ... > file` when the file must contain exact command output.

```bash
rg pattern . > matches.txt          # Auto-rewrite bypass: native full output
rtk raw rg pattern . > matches.txt  # Explicit full output + RTK usage tracking
rtk raw rg pattern . | awk '{print $1}'
rtk raw ssh host 'rg pattern /srv' > matches.txt
```

`rtk raw` is a visible alias for `rtk proxy`. Redirection to `/dev/null` and
stderr-only redirection do not disable filtering because they cannot persist
filtered stdout as data.

### Hook Architecture

```
Claude Code → PreToolUse hook → rtk-rewrite.sh
                                    │
                                    ├── Delegate to `rtk rewrite`
                                    ├── Preserve unsupported/unsafe commands
                                    └── Return the rewritten command to Claude Code
```

The Rust rewrite engine is the single source of truth for command recognition,
multiline handling, pipeline safety, exclusions, and trusted TOML filters.

---

## Global Flags

```bash
-u, --ultra-compact    # ASCII icons, inline format (extra savings)
-v, --verbose          # Increase verbosity (-v, -vv, -vvv)
```

---

## Configuration

| Command | Scope | Hook | RTK.md | Context Tokens | Use Case |
|---------|-------|------|--------|----------------|----------|
| `rtk init -g` | Claude global | Yes | Slim | Low | **Recommended** |
| `rtk init -g --codex` | Codex global | No | Slim | Low | **Recommended for Codex** |
| `rtk init --codex` | Codex project | No | Slim | Low | Project-only |
| `rtk init -g --claude-md` | Claude global | No | Inline | Higher | Legacy |
| `rtk init -g --hook-only` | Global | Yes | None | 0 | Minimal |
| `rtk init` | Claude project | No | Inline | Higher | Single project |

### Custom Database Path

```bash
# Environment variable (highest priority)
export RTK_DB_PATH="/path/to/custom.db"

# Or config file (~/.config/rtk/config.toml)
[tracking]
database_path = "/path/to/custom.db"
```

---

## Documentation

| Document | Purpose |
|----------|---------|
| [**FORK.md**](FORK.md) | Fork architectural deep-dive — all changes vs upstream |
| [**ARCHITECTURE.md**](ARCHITECTURE.md) | Full system architecture and module map |
| [**CHANGELOG.md**](CHANGELOG.md) | Version history |
| [docs/rtk-memory-v0.1.md](docs/rtk-memory-v0.1.md) | Shared-memory/delta layer specification |
| [docs/read-improvements.md](docs/read-improvements.md) | Read pipeline architecture plan |
| [docs/write-improvements.md](docs/write-improvements.md) | Write infrastructure specification |
| [docs/new-commands.md](docs/new-commands.md) | Guide for adding new commands |
| [docs/tracking.md](docs/tracking.md) | Tracking API for programmatic access |
| [docs/AUDIT_GUIDE.md](docs/AUDIT_GUIDE.md) | Token savings analytics guide |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | Common issues and fixes |
| [SECURITY.md](SECURITY.md) | Security policy and PR review |

---

## Troubleshooting

### Wrong rtk Installed

```bash
rtk gain   # "command not found" = wrong package
# Fix: uninstall reachingforthejack/rtk, install rtk-ai/rtk
```

### Hook Not Working

```bash
rtk init --show                           # Check hook status
cat ~/.claude/settings.json | grep rtk    # Verify registration
# Then restart Claude Code
```

### Settings.json Issues

```bash
rtk init -g --no-patch   # Manual mode — prints JSON snippet
cp ~/.claude/settings.json.bak ~/.claude/settings.json  # Restore backup
```

See [TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) for more.

---

## Uninstalling

```bash
rtk init -g --uninstall   # Remove hook, RTK.md, settings.json entry
cargo uninstall rtk        # Remove binary
```

---

## Contributing

Contributions welcome. PRs undergo automated security review (see [SECURITY.md](SECURITY.md)).

**For fork-specific features**: Please open issues/PRs at [heAdz0r/rtk](https://github.com/heAdz0r/rtk/issues).
**For upstream features**: Use [rtk-ai/rtk](https://github.com/rtk-ai/rtk/issues).

---

## Credits

- **Upstream**: [rtk-ai/rtk](https://github.com/rtk-ai/rtk) by Patrick Szymkowiak
- **Fork**: [heAdz0r/rtk](https://github.com/heAdz0r/rtk)

## License

MIT License — see [LICENSE](LICENSE) for details.
