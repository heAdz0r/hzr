# PRD: RTK Read/Write Quality — Session Analysis Driven Improvements

## Introduction

Analysis of 46 real sessions in `dkp_rag26` (3 883 tool calls) revealed two systemic token-waste
patterns:

1. **97% of `rtk read` calls use `--level none`** — zero filtering, zero savings.
   Root cause: the Read-hook hardcodes `--level none` when auto-running rtk, and its fallback
   message explicitly says `"Try: rtk read <file> --level none"`, teaching the model a bad habit.

2. **402 write workarounds via `python3`/`cat` heredocs** — 54% of all write ops bypass rtk.
   Root cause: the Write-hook block message lists only `replace|patch|set|batch` and omits
   `create`. The model doesn't know `rtk write create` exists, so it falls back to shell heredocs
   that bypass atomicity guarantees.

## Goals

- Reduce `--level none` usage from 97% → ≤30% (use `minimal` for overview reads).
- Eliminate python3/cat heredoc write workarounds by advertising `rtk write create` / `file`.
- Zero regression on existing 341 legitimate `rtk write patch/replace/batch` calls.
- Zero regression on targeted edit reads (with `--from`/`--to` — must keep `--level none`).

## User Stories

### US-001: Smart per-extension default level in Read hook

**Description:** As the AI model, I want the Read-hook to auto-select the correct filter level
based on file extension so I stop wasting tokens on `--level none` for overview reads.

**Acceptance Criteria:**
- [ ] Hook selects `--level minimal` for code files (`.go .rs .py .ts .tsx .js .jsx .java .rb .sh`)
- [ ] Hook keeps `--level none` for config/data files (`.json .yaml .yml .toml .env .lock .mod .sum`)
- [ ] Hook selects `--level minimal` for docs (`.md .txt .rst`)
- [ ] Hook uses `--level none` when `--from`/`--to` range is specified (edit mode → full context)
- [ ] Fallback: if `minimal` returns empty content, retry automatically with `--level none`
- [ ] Fallback message says `"Use --level none only when editing (with --from/--to)"` — not `"Try --level none"`
- [ ] Unit test: extension table coverage (≥8 extensions verified)

### US-002: `rtk write file` alias for `create`

**Description:** As the AI model, I want `rtk write file <path> --content @/tmp/f` to work as an
alias for `rtk write create` so the mental model is consistent with `write file` semantics.

**Acceptance Criteria:**
- [ ] `rtk write file <path> --content @/tmp/f` is accepted by Clap (alias or separate variant)
- [ ] `rtk write file` delegates to `run_create` with identical semantics (atomic, idempotent)
- [ ] `--content @/tmp/file` and `--content @-` (stdin) both work
- [ ] `--dry-run` flag works
- [ ] Idempotency: running twice with same content = noop with exit 0
- [ ] `cargo test write::tests::` passes
- [ ] Smoke test: `rtk write file /tmp/rtk_test_new.txt --content "hello" && rtk write file /tmp/rtk_test_new.txt --content "hello"` exits 0 both times

### US-003: Write-hook block message includes `create`/`file`

**Description:** As the AI model, when I try to use Native Write for a NEW file and get blocked,
I want the error message to tell me to use `rtk write create` / `rtk write file`.

**Acceptance Criteria:**
- [ ] Block message lists `replace|patch|set|batch|create` (adds `create`)
- [ ] Block message adds line: `"For new files: rtk write file <path> --content @/tmp/file"`
- [ ] Example in message uses `@/tmp/` pattern (consistent with @file convention)
- [ ] Message is ≤12 lines (no bloat)

### US-004: CLAUDE.md / RTK.md instruction updates

**Description:** As the AI model, I want authoritative instructions in CLAUDE.md that teach the
correct read level and write file patterns so I don't need hook nudges to learn.

**Acceptance Criteria:**
- [ ] `~/.claude/RTK.md` documents `rtk write file` / `rtk write create`
- [ ] RTK.md "Safe File Writes" table includes `rtk write file` row
- [ ] RTK.md "Read" section has explicit rule:
  `"Use --level minimal for overview; --level none only with --from/--to (edit mode)"`
- [ ] No stale references to `replace|patch|set|batch` only (add `create`/`file`)

### US-005: Read-hook delivers INTERCEPTED content with correct level

**Description:** As the AI model, when the Read-hook intercepts my Read call, I want to receive
the file content filtered at the correct level (not always raw), so token savings happen
transparently.

**Acceptance Criteria:**
- [ ] Hook INTERCEPTED path uses the same smart-level logic as US-001
- [ ] INTERCEPTED header includes the actual level used: `"(filtered: minimal)"`
- [ ] For a typical 200-line Go file (no range), intercepted content is shorter than raw
- [ ] For a 50-line JSON config (no range), intercepted content equals raw (level=none, no loss)
- [ ] For a read with `--from 10 --to 30`, intercepted content is always level=none

## Functional Requirements

- FR-1: `rtk-block-native-read.sh` — extension-based level table (code→minimal, config→none, docs→minimal, range→none)
- FR-2: `rtk-block-native-read.sh` — two-pass: try smart level, fallback to `--level none` if empty
- FR-3: `rtk-block-native-read.sh` — INTERCEPTED header shows actual level used
- FR-4: `rtk-block-native-read.sh` — fallback message does NOT say `"Try --level none"` unconditioned
- FR-5: `rtk write file` Clap variant (alias or new arm) delegating to `run_create`
- FR-6: `rtk-block-native-write.sh` — block message adds `create` and `file` with example
- FR-7: `~/.claude/RTK.md` — write file row, read level rule documented
- FR-8: Smoke test coverage for `rtk write file` (both new-file and idempotent-noop paths)

## Non-Goals

- No changes to `run_create` logic (already correct: atomic, idempotent, refuses overwrite)
- No changes to filter.rs `MinimalFilter` logic
- No server-side changes, no database changes
- No changes to `rtk write batch` JSON schema
- Not fixing `--level aggressive` under-use (separate concern)
- Not analyzing other projects beyond dkp_rag26 (this PRD is scoped to confirmed patterns)

## Technical Considerations

- `rtk write file` in Clap: add `#[command(alias = "file")]` to the existing `Create` variant
  OR add a new `File` variant that calls `run_create`. Alias is simpler (1-line change).
- Hook smart-level uses bash `case` on `${FILE_PATH##*.}` (no extra deps).
- Two-pass fallback in hook: run with smart level, check non-empty, re-run with `--level none` if needed.
  Cost: one extra process spawn only on empty results (rare).

## Success Metrics

- `--level none` share of `rtk read` calls drops from 97% → ≤30% within 5 sessions of deployment
- python3/cat heredoc write workarounds drop from 402/session-corpus → near 0 for NEW files
- Zero new failing tests after changes (`cargo test`, `bash scripts/test-all.sh`)

## Open Questions

- Should `rtk write file` silently overwrite existing files (like `cp`)? Current `create` refuses.
  **Proposed answer:** Keep refuse-on-different-content semantics — model should use `patch`/`replace` for existing files.
