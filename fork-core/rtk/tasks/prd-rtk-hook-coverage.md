# PRD: RTK Hook Coverage & Go Run Command

**Date:** 2026-02-25
**Source:** `rtk discover -p dkp-rag26 --since 60` analysis
**Project context:** dkp_rag26 — Go backend + Bun/React frontend

---

## Problem Statement

Analysis of 145 sessions / 11,811 Bash commands in dkp_rag26 shows:
- **54% RTK coverage** (good base, but 46% still bypasses)
- **1,434 commands missed → ~148.5K tokens saveable**
- **100% memory context miss rate** (53/53 subagent tasks) — cache was dirty

### Top missed commands by token impact

| Command | Count | RTK equivalent | Est. savings |
|---------|-------|----------------|-------------|
| `cat <file>` | 184 | `rtk read` | 53.9K tokens |
| `grep -n` | 251 | `rtk grep` | 30.9K tokens |
| `ls -la` | 418 | `rtk ls` | 24.5K tokens |
| `go build` | 277 | `rtk go build` | 11.4K tokens |
| `bun run` | 85 | `rtk bun run` | 2.4K tokens |

Note: `cat`, `grep -n`, `ls -la`, `go build` are already handled in the hook rewrite.
Their continued appearance means edge cases slip through (flags, absolute paths, piped patterns).

### Unhandled commands (no RTK rule yet)

| Command | Count | Gap |
|---------|-------|-----|
| `tail` | 58 | Log/file tailing — no hook rule |
| `go run` | 37 | Go execution — no hook rule and no RTK command |
| `/usr/bin/grep` | 22 | **Explicit hook bypass** via absolute path |
| `bunx tsc` | 8 | Should route → `rtk tsc` |
| `bunx vite` | 9 | Should route → `rtk npx vite` |
| `/usr/bin/find` | 8 | Absolute path bypass |

---

## Goals

1. **Close absolute-path bypass** — `/usr/bin/grep`, `/usr/bin/find` escape the rewrite hook
2. **Add `bunx` routing** — `bunx tsc` → `rtk tsc`, `bunx vite` → `rtk npx vite`, generic `bunx X` → `rtk bun x X`
3. **Add `go run` support** — hook rule + Rust command in `go_cmd.rs`
4. **Add `tail -N <file>` routing** — `tail -5 file` → `rtk read file --max-lines 5`

---

## Non-Goals

- `ps aux`, `lsof`, `pkill` — process management, no filtering value
- `xcodebuild`, `xcrun simctl` — Apple toolchain, out of scope
- `tail -f` (streaming follow) — real-time, can't buffer/filter

---

## Implementation Tasks

### T1 — Hook: `bunx` routing
File: `hooks/rtk-rewrite.sh`

Add before existing `bun` block:
- `bunx tsc` → `rtk tsc`
- `bunx vue-tsc` → `rtk npx vue-tsc`
- `bunx vite` → `rtk npx vite`  
- `bunx <X>` (generic) → `rtk bun x <X>`

### T2 — Hook: absolute path grep/find
File: `hooks/rtk-rewrite.sh`

Extend existing `grep`/`find` regex to also match `/usr/bin/grep`, `/usr/local/bin/grep`, `/usr/bin/find`.

### T3 — Hook: `tail -N <file>` → `rtk read`
File: `hooks/rtk-rewrite.sh`

After `head` rule:
- `tail -N file` → `rtk read file --max-lines N`
- `tail -f file` → passthrough (streaming)

### T4 — Hook: `go run` → `rtk go run`
File: `hooks/rtk-rewrite.sh`

Add to existing go block (before `run_other` fallthrough):
- `go run` → `rtk go run`

### T5 — Rust: `rtk go run` command
Files: `src/go_cmd.rs`, `src/main.rs`

Add `Go::Run` variant. `run_run()` function:
- Runs `go run <args>`
- Exit 0 → `✓ go run: ok`
- Exit ≠ 0 → filter errors via `filter_go_build()` (reuse)
- Tracks savings

### T6 — Hook tests
File: `hooks/test-rtk-rewrite.sh`

New assertions for T1–T4 cases.

### T7 — Unit tests: go_cmd run_run
File: `src/go_cmd.rs`

`#[cfg(test)]` for filter logic (success + error cases).

---

## Success Metrics

- `rtk discover -p dkp-rag26` shows: `bunx` → 0, `/usr/bin/grep` → 0, `tail` → 0
- `bash hooks/test-rtk-rewrite.sh` — all new cases pass
- `cargo test go_cmd` passes
- No regressions in existing hook tests

---

## File Scope

| File | Tasks |
|------|-------|
| `hooks/rtk-rewrite.sh` | T1, T2, T3, T4 |
| `hooks/test-rtk-rewrite.sh` | T6 |
| `src/go_cmd.rs` | T5, T7 |
| `src/main.rs` | T5 |
