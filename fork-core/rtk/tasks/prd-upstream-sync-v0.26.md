# PRD: Upstream Sync v0.23.0 -> v0.26.0 (Incremental Migration)

**Date**: 2026-03-05
**Fork base**: v0.22.2 (fork.21)
**Upstream target**: v0.26.0 (32ade38)
**Upstream versions to absorb**: v0.23.0, v0.24.0, v0.25.0, v0.26.0

---

## 1. Introduction

Fork diverged от upstream на базе v0.22.2. С тех пор upstream выпустил 4 мажорных релиза (0.23 -> 0.26) с архитектурными изменениями, которые конфликтуют с fork-only модулями. Нужна поэтапная миграция, сохраняющая fork-only функциональность (memory layer, write ops, rgai, ssh, read extensions) при адоптии upstream улучшений.

### Масштаб расхождения

| Метрика | Значение |
|---------|----------|
| Upstream-only коммитов | 10 |
| Fork-only коммитов | 10 |
| Файлов изменено (diff) | 395 |
| Fork-only модулей (22) | `memory_layer/`, `write_cmd/core/lock/semantics`, `rgai_cmd`, `grepai`, `ssh_cmd`, `read_*` (6), `symbols_regex`, `build_cmd`, `bun_cmd`, `diag_summary`, `lsof_cmd`, `ps_cmd`, `session_stats` |
| Upstream-only модулей (6) | `aws_cmd`, `psql_cmd`, `mypy_cmd`, `integrity`, `rewrite_cmd`, `telemetry` |
| Общих модулей (49) | `cargo_cmd`, `config`, `git`, `grep_cmd`, `tracking`, `init`, `discover/registry`, `find_cmd`, `gh_cmd`, `read`, `utils`, etc. |

---

## 2. Goals

- **G1**: Импортировать все новые upstream модули (aws, psql, mypy, integrity, rewrite, telemetry)
- **G2**: Принять upstream bugfixes в общих модулях (git, grep, find, playwright, cargo, tracking, config, gain, gh)
- **G3**: Сохранить 100% fork-only функциональности (memory layer, write ops, rgai, ssh, read extensions)
- **G4**: Перейти на upstream архитектуру hook rewrite (thin delegator + `rtk rewrite`) при сохранении fork-only hook расширений
- **G5**: `cargo test` + `cargo clippy` зелёные после каждого этапа
- **G6**: Минимизировать будущий merge debt — максимально выровнять структуру с upstream

---

## 3. Risk Analysis

### 3.1 Критические конфликты (требуют ручного merge)

| Зона | Fork | Upstream | Конфликт |
|------|------|----------|----------|
| **src/main.rs** | 68 модулей, fork Commands enum | 55 модулей, реструктуренный enum | Полный rewrite enum; нужно merge 22 fork-only mod + 6 upstream-only mod |
| **src/init.rs** | 1295 строк, multi-hook install (rewrite + block-grep + block-read + block-write + block-task + mem-context) | 314 строк, single hook + integrity hash | Fork hooks >> upstream; нужно сохранить fork hook install при адопции integrity |
| **src/config.rs** | GrepaiConfig + MemConfig + MemFeatureFlags | TelemetryConfig + HooksConfig (exclude_commands) | Merge обеих конфигов |
| **src/tracking.rs** | project_where_clause, tracking_meta table, OnceLock, maybe_cleanup_old | Упрощённая версия без tracking_meta, без OnceLock | Cherry-pick upstream simplifications, сохранить project scoping |
| **src/discover/registry.rs** | 887 строк, OnceLock patterns, fork-specific rules (rgai, ssh, write, memory) | 1085 строк, lazy_static, rewrite_command(), rules.rs extraction | Принять rewrite_command() + rules extraction, добавить fork-only rules |
| **hooks/rtk-rewrite.sh** | 454 строки, inline if-else + audit + fork commands | 51 строка, thin delegator -> `rtk rewrite` | Принять thin delegator, перенести fork-only logic в registry |
| **Cargo.toml** | csv, toml_edit, fs2 deps | Удалены; добавлен sha2 для integrity | Keep fork deps (csv for read_digest, fs2 for write_lock, toml_edit for write_cmd set) |

### 3.2 Безопасные upstream changes (чистый cherry-pick)

- `src/aws_cmd.rs` — новый модуль (880 строк)
- `src/psql_cmd.rs` — новый модуль (382 строки)
- `src/mypy_cmd.rs` — новый модуль (389 строк)
- `src/integrity.rs` — новый модуль (537 строк)
- `src/rewrite_cmd.rs` — новый модуль (50 строк)
- `src/telemetry.rs` — новый модуль (144 строки)
- `src/discover/rules.rs` — новый модуль (377 строк)
- `.claude/agents/*`, `.claude/skills/*`, `.claude/commands/*` — Claude Code agents/skills (чистое добавление)

### 3.3 Модули удалённые upstream, но нужные fork

Upstream удалил концепции, которые fork развил дальше. **Их НЕ удаляем**:

| Модуль | Причина сохранения |
|--------|--------------------|
| `memory_layer/` (14 файлов, ~9000 строк) | Core fork feature: semantic indexing, graph, planning |
| `write_cmd.rs` + `write_core.rs` + `write_lock.rs` + `write_semantics.rs` | Atomic writes — ежедневно используется через `rtk write` |
| `rgai_cmd.rs` + `grepai.rs` | Semantic search — primary search tool |
| `ssh_cmd.rs` | SSH output filtering |
| `read_cache.rs`, `read_changed.rs`, `read_digest.rs`, `read_render.rs`, `read_source.rs`, `read_symbols.rs`, `read_types.rs`, `symbols_regex.rs` | Extended read pipeline: CSV digest, git-aware diffs, symbol extraction |
| `build_cmd.rs`, `bun_cmd.rs` | Build/bun wrappers |
| `lsof_cmd.rs`, `ps_cmd.rs`, `session_stats.rs` | System diagnostics |
| `diag_summary.rs` | Diagnostic aggregation |

---

## 4. Migration Plan (6 этапов)

### Phase 1: New upstream modules (low risk)

**Scope**: Чистое добавление новых модулей без конфликтов.

| Task | Files | Action |
|------|-------|--------|
| P1.1 | `src/aws_cmd.rs` | Copy from upstream |
| P1.2 | `src/psql_cmd.rs` | Copy from upstream |
| P1.3 | `src/mypy_cmd.rs` | Copy from upstream |
| P1.4 | `src/integrity.rs` | Copy from upstream |
| P1.5 | `src/rewrite_cmd.rs` | Copy from upstream |
| P1.6 | `src/telemetry.rs` | Copy from upstream |
| P1.7 | `src/discover/rules.rs` | Copy from upstream |
| P1.8 | `src/main.rs` | Add `mod` declarations for new modules |
| P1.9 | `Cargo.toml` | Add `sha2` dep (for integrity); keep fork-only deps |

**Gate**: `cargo check` passes.

### Phase 2: Upstream bugfixes in shared modules (medium risk)

**Scope**: Cherry-pick upstream fixes в модулях, которые есть и у нас. Конфликты возможны, но локальные.

| Task | PR/Commit | Module | Change |
|------|-----------|--------|--------|
| P2.1 | #246 | `discover/registry.rs` | Fix "fi" shadowing find |
| P2.2 | #193 | `playwright_cmd.rs` | Fix JSON parser format |
| P2.3 | #211 | `find_cmd.rs` | Accept native find flags |
| P2.4 | #200 | `main.rs` | Passthrough fallback on Clap parse fail |
| P2.5 | #192 | `git.rs` | Git global options (-C, -c, --git-dir, etc.) |
| P2.6 | #234 | `git.rs` | Propagate exit codes in push/pull/fetch/stash/worktree |
| P2.7 | #248 | `grep_cmd.rs` / `git.rs` | Fix duplicate output on `git show` grep |
| P2.8 | #274 | `go_cmd.rs` | Surface build failures in go test summary |
| P2.9 | #245 | `discover/registry.rs` | Filter docker compose hook rewrites |
| P2.10 | #128 | `gain.rs` | Per-project token savings with `-p` flag |
| P2.11 | #109 | Commands enum | Mypy command routing |
| P2.12 | #297 | `curl_cmd.rs` | JSON size guard |
| P2.13 | #243 | `config.rs` | `exclude_commands` config |

**Note**: Часть из этих (P2.1, P2.3, P2.5, P2.7) уже была backported в fork.16. Проверить перед применением — возможно noop.

**Gate**: `cargo test` passes, `cargo clippy` clean.

### Phase 3: Config & tracking merge (high risk)

**Scope**: Merge конфигурации и tracking — оба модуля сильно расходятся.

| Task | Module | Strategy |
|------|--------|----------|
| P3.1 | `src/config.rs` | Add upstream `TelemetryConfig` + `HooksConfig` alongside fork `GrepaiConfig` + `MemConfig` |
| P3.2 | `src/tracking.rs` | Accept upstream simplifications (remove `tracking_meta` if unused, simplify `get_stats`); preserve fork `project_where_clause` if `-p` flag adopted in P2.10 |
| P3.3 | `src/gain.rs` | Merge upstream per-project flag with fork gain display |

**Gate**: `cargo test` passes, `rtk gain` works.

### Phase 4: Registry & rewrite architecture (high risk)

**Scope**: Самый сложный этап — переход на upstream `rtk rewrite` архитектуру.

| Task | Action |
|------|--------|
| P4.1 | Adopt upstream `discover/registry.rs` rewrite_command() API |
| P4.2 | Adopt `discover/rules.rs` pattern extraction |
| P4.3 | Add fork-only rules into rules.rs: `rgai`, `ssh`, `write`, `memory`, `bun`, `build`, `lsof`, `ps`, `session-stats`, `diag` |
| P4.4 | Verify `rewrite_cmd.rs` works with merged registry |
| P4.5 | Adopt thin delegator `hooks/rtk-rewrite.sh` |
| P4.6 | Move fork-only hook logic (block-native-*, mem-context) into separate hook files (already exist in `hooks/`) |
| P4.7 | Update `src/init.rs`: adopt upstream integrity hashing, keep fork multi-hook install |

**Key decision**: Fork hooks (`rtk-block-native-grep.sh`, `rtk-block-native-read.sh`, `rtk-block-native-write.sh`, `rtk-block-task.sh`, `rtk-mem-context.sh`) stay as separate files. Only `rtk-rewrite.sh` migrates to thin delegator.

**Gate**: `cargo test`, hook rewrite for fork commands works (`rtk rewrite "rgai search foo"` returns `rtk rgai search foo`).

### Phase 5: main.rs Commands enum merge (high risk)

**Scope**: Финальный merge entry point.

| Task | Action |
|------|--------|
| P5.1 | Merge upstream Commands enum changes (Aws, Psql, Mypy, Rewrite, Verify subcommands) |
| P5.2 | Keep all fork-only Commands (Write, Build, Ssh, Rgai, Memory, Lsof, Ps, SessionStats, Diag, Bun) |
| P5.3 | Adopt upstream Find args restructure (`trailing_var_arg` at field level) |
| P5.4 | Adopt upstream Read simplification (remove fork's `--from/--to/--level` from enum if upstream removed them) — **CAUTION**: fork Read has extended args; keep them, merge carefully |
| P5.5 | Adopt passthrough fallback (match on Clap parse error -> proxy) |
| P5.6 | Wire new commands: `Commands::Aws => aws_cmd::run()`, `Commands::Psql => psql_cmd::run()`, etc. |
| P5.7 | Run `bash scripts/sync-architecture-modules.sh` |

**Gate**: Full pre-commit gate passes.

### Phase 6: Docs, CI, cleanup

| Task | Action |
|------|--------|
| P6.1 | Update `ARCHITECTURE.md` module table (from ~68 to ~74 modules) |
| P6.2 | Update `CLAUDE.md` command reference (add aws, psql, mypy, rewrite, verify) |
| P6.3 | Update `Cargo.toml` version to `0.26.0-fork.1` |
| P6.4 | Adopt upstream `.github/workflows/release.yml` changes (musl target) |
| P6.5 | Adopt upstream `.github/workflows/validate-docs.yml` changes |
| P6.6 | Copy `.claude/agents/*`, `.claude/skills/*`, `.claude/commands/*` from upstream |
| P6.7 | Remove `.rtk-lock` files (upstream doesn't have them — check if still needed) |
| P6.8 | Run `bash scripts/validate-docs.sh` |
| P6.9 | Delete `tasks/prd-upstream-pr-backports.md` (superseded by this PRD) |
| P6.10 | Update `CHANGELOG.md` with combined fork+upstream changes |

**Gate**: Full pre-commit gate + `bash scripts/validate-docs.sh`.

---

## 5. Upstream Features: Accept / Reject / Defer

| Feature | Version | Decision | Rationale |
|---------|---------|----------|-----------|
| AWS CLI module | v0.24.0 | **Accept** | New capability, no conflicts |
| psql module | v0.24.0 | **Accept** | New capability, no conflicts |
| mypy module | v0.23.0 | **Accept** | New capability, no conflicts |
| SHA-256 hook integrity | v0.24.0 | **Accept** | Security improvement, complements fork hooks |
| `rtk rewrite` command | v0.25.0 | **Accept** | Better architecture — single source of truth |
| Thin delegator hook | v0.25.0 | **Accept** | Reduces hook maintenance, fork hooks stay separate |
| Anonymous telemetry | v0.26.0 | **Accept (disabled)** | Take code but compile without URL (opt-in only) |
| Claude Code skills (PR/issue triage) | v0.26.0 | **Accept** | .claude/ files, no code changes |
| `exclude_commands` config | v0.26.0 | **Accept** | Useful for fork users too |
| curl JSON size guard | v0.26.0 | **Accept** | Bugfix |
| Per-project `gain -p` | v0.23.0 | **Accept** | Enhances fork's tracking |
| Removal of `read --from/--to/--level` | v0.25.0 | **Reject** | Fork uses extended read args daily |
| Removal of `write` subcommand | v0.25.0 | **Reject** | Core fork feature |
| Removal of `ssh` command | upstream never had | N/A | Fork-only |
| Removal of `rgai`/`grepai` | upstream never had | N/A | Fork-only |
| Removal of `memory` subcommand | upstream never had | N/A | Fork-only |
| Removal of `bun` subcommand | v0.25.0 | **Reject** | Fork uses bun daily |
| `discover/rules.rs` extraction | v0.25.0 | **Accept** | Better modularity |

---

## 6. Non-Goals

- **NG1**: Rebase fork onto upstream (too many conflicts; merge strategy instead)
- **NG2**: Remove any fork-only functionality
- **NG3**: Change fork hook architecture (block-native-* hooks stay as-is)
- **NG4**: Enable upstream telemetry by default
- **NG5**: Adopt upstream's removal of `csv`/`fs2`/`toml_edit` deps (fork needs them)

---

## 7. Technical Notes

### Cargo.toml deps delta

```toml
# Upstream adds:
sha2 = { version = "0.10", features = ["std"] }   # for integrity.rs
lazy_static = "1"                                    # for registry.rs (replaces OnceLock)

# Fork keeps (upstream removed):
csv = "1"           # read_digest.rs
toml_edit = "0.22"  # write_cmd.rs set operation
fs2 = "0.4"         # write_lock.rs flock
```

### main.rs module count projection

```
Current fork:     68 modules
- Removed by upstream:  0 (we keep all)
+ New from upstream:    6 (aws, psql, mypy, integrity, rewrite, telemetry)
+ New rules.rs:         1
= Total:               75 modules
```

### Hook architecture after migration

```
~/.claude/hooks/
  rtk-rewrite.sh           # Thin delegator (upstream) -> calls `rtk rewrite`
  rtk-block-native-grep.sh # Fork-only: blocks native Grep tool
  rtk-block-native-read.sh # Fork-only: blocks native Read tool
  rtk-block-native-write.sh# Fork-only: blocks native Write tool
  rtk-block-task.sh        # Fork-only: blocks native Task(Explore)
  rtk-mem-context.sh       # Fork-only: memory layer injection
```

---

## 8. Success Metrics

| Metric | Target |
|--------|--------|
| `cargo test` | All pass (existing + new upstream tests) |
| `cargo clippy --all-targets` | 0 warnings |
| Fork commands work | `rtk write`, `rtk rgai`, `rtk ssh`, `rtk memory`, `rtk bun`, `rtk read --from/--to` |
| New commands work | `rtk aws`, `rtk psql`, `rtk mypy`, `rtk rewrite`, `rtk verify` |
| Hook rewrite | `rtk rewrite "git status"` = `rtk git status` |
| Hook integrity | `rtk verify` = Verified |
| Smoke tests | `bash scripts/test-all.sh` passes |
| validate-docs | `bash scripts/validate-docs.sh` passes |

---

## 9. Open Questions

1. **lazy_static vs OnceLock**: Upstream перешёл на lazy_static для registry. Форк использует OnceLock. Принять lazy_static для совместимости или оставить OnceLock? **Recommendation**: принять lazy_static (upstream direction).

2. **tracking_meta table**: Форк добавил `tracking_meta` для cleanup timestamps. Upstream удалил. Оставить или убрать? **Recommendation**: убрать если cleanup работает без неё.

3. **read.rs delta**: Upstream сильно упростил read (338 строк vs fork's ~1800 с расширениями). Как мержить? **Recommendation**: оставить fork read.rs как есть, cherry-pick только конкретные bugfixes из upstream read.rs.

4. **Version scheme**: `0.26.0-fork.1` или другая схема? **Recommendation**: `0.26.0-fork.1` (показывает upstream base).

5. **.rtk-lock файлы**: Upstream их не имеет. Они нужны форку для write_lock. Оставить? **Recommendation**: оставить, добавить в `.gitignore` если нет.
