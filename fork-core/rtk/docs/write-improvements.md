# Write Improvements: атомарный I/O, semantic parity и benchmark plan

## Цель
Сделать write-путь в RTK безопасным и быстрым на уровне файловых операций, при этом исключить semantic drift при автозамене нативных mutating-команд (чтобы LLM не терял доверие к поведению команды).

## Текущее состояние и выявленные риски

### 1) Что уже хорошо
1. Есть `atomic_write` с tempfile + rename в `/Users/andrew/Programming/rtk/src/init.rs:305`.
2. Есть idempotent helper `write_if_changed` в `/Users/andrew/Programming/rtk/src/init.rs:275`.
3. Есть hook rewrite/audit инфраструктура:
   - rewrite: `/Users/andrew/Programming/rtk/hooks/rtk-rewrite.sh`
   - audit metrics: `/Users/andrew/Programming/rtk/src/hook_audit_cmd.rs`

### 2) P0-риск semantic parity (mutating commands)
В `git`-обертках есть пути, где при ошибке пишется `FAILED`, но процесс может вернуть `Ok(())` и не передать non-zero код наверх.
Примеры:
1. `/Users/andrew/Programming/rtk/src/git.rs:660` (`run_commit`)
2. `/Users/andrew/Programming/rtk/src/git.rs:724` (`run_push`)
3. `/Users/andrew/Programming/rtk/src/git.rs:785` (`run_pull`)
4. `/Users/andrew/Programming/rtk/src/git.rs:870` (`run_branch` action-mode)
5. `/Users/andrew/Programming/rtk/src/git.rs:998` (`run_fetch`)
6. `/Users/andrew/Programming/rtk/src/git.rs:1042` (`run_stash` mutating branches)
7. `/Users/andrew/Programming/rtk/src/git.rs:1180` (`run_worktree` action-mode)

Это главный источник «LLM confusion»: визуально команда "переписана корректно", но семантика exit code может отличаться от native.

### 3) I/O-консистентность write-path
1. Атомарная запись используется точечно, не как общий стандарт write-операций.
2. Нет единой политики durability (`flush`, `sync_data`, `fsync(dir)`) для файлов, где потеря данных критична.
3. Нет общей модели optimistic concurrency (CAS по метаданным/хэшу), чтобы не перетирать чужие изменения между read->write.

## Принципы целевой архитектуры
1. **Semantic parity first**: для mutating-команд приоритет = корректный exit code и side-effect parity, а не token reduction.
2. **Atomic by default** для user-facing writes: `tempfile in same dir -> write -> flush -> sync_data -> rename -> fsync(parent dir)`.
3. **Idempotent writes**: если контент не изменился, диск не трогаем.
4. **Explicit durability modes**:
   - `durable`: для конфигов/кода (default).
   - `fast`: только для кэшей и некритичных артефактов.
5. **No blind rewrite of mutating native commands** без подтвержденной parity.

## Целевая модульная структура
1. `/Users/andrew/Programming/rtk/src/write_core.rs` (новый)
   - `AtomicWriter`, `WriteOptions`, `WriteStats`, `DurabilityMode`.
2. `/Users/andrew/Programming/rtk/src/write_semantics.rs` (новый)
   - parity contract, exit-code policy, mutation command classifier.
3. `/Users/andrew/Programming/rtk/src/write_cmd.rs` (новый, опционально)
   - будущий `rtk write` subcommand (patch/apply/set).
4. `/Users/andrew/Programming/rtk/src/init.rs`
   - перевод текущих write helper на `write_core`.
5. `/Users/andrew/Programming/rtk/src/git.rs`
   - унификация error/exit propagation для mutating paths.
6. `/Users/andrew/Programming/rtk/hooks/rtk-rewrite.sh`
   - risk-aware rewrite policy для mutating команд.

## Атомарный I/O: reference алгоритм

### Single-file write (durable mode)
1. `lstat(target)` + capture metadata snapshot (`size`, `mtime`, `ino` where available).
2. Если включен CAS: проверить `expected_hash/expected_mtime`.
3. `NamedTempFile::new_in(parent_dir)`.
4. Запись через `BufWriter` (tunable buffer, default 64 KiB).
5. `flush()` + `temp_file.sync_data()`.
6. Сохранить permissions/ownership (если применимо).
7. Atomic `rename(temp, target)` в том же каталоге.
8. `fsync(parent_dir)` для durability rename metadata.
9. Вернуть `WriteStats` (bytes_written, fsync_count, rename_count, elapsed).

### Failure semantics
1. Любая ошибка до rename -> target нетронут.
2. Ошибка после rename -> ошибка наружу + target в последнем консистентном состоянии.
3. Temp cleanup best-effort.

### Performance notes (Rust)
1. Не читать весь файл в память без необходимости.
2. Для "write-if-changed" использовать cheap precheck:
   - size mismatch -> changed,
   - optional hash compare if size equal and strict mode requested.
3. Для больших файлов избегать лишних аллокаций, stream pipeline.
4. Для batch writes группировать `fsync(dir)` по уникальным parent dirs.

## Semantic parity policy (чтобы LLM не путался)

### Rewrite policy levels
1. `read_only_safe`: auto-rewrite разрешен.
2. `mutating_guarded`: rewrite только при доказанной parity и тестовом coverage.
3. `mutating_unsafe`: no rewrite, pass-through native.

### Непосредственно для текущего hook
1. В `/Users/andrew/Programming/rtk/hooks/rtk-rewrite.sh` добавить флаг:
   - `RTK_REWRITE_MUTATING=0` (default): не переписывать mutating git subcommands (`add`, `commit`, `push`, `pull`, branch/stash/worktree actions).
   - `RTK_REWRITE_MUTATING=1`: включить только после прохождения parity benchmark gate.
2. В audit log фиксировать `class=read_only|mutating`.

### Compatibility contract для mutating wrappers
1. Exit code обязан совпадать с native.
2. Side effect обязан совпадать (например, staging/commit state).
3. При ошибке stderr должен сохранять ключевую диагностику native-инструмента.
4. Compact output допустим только при success; на failure — semantically faithful error path.

## План в разрезе PR

### PR-W1 (P0): Semantic parity hardening
Scope:
1. Исправить non-zero propagation во всех mutating ветках `git.rs`.
2. Добавить regression tests: success/failure parity.
3. Добавить явный classification mutating/read-only в коде git layer.

Files:
1. `/Users/andrew/Programming/rtk/src/git.rs`
2. `/Users/andrew/Programming/rtk/src/main.rs` (если потребуется exit handling унификация)
3. `/Users/andrew/Programming/rtk/src/git.rs` tests

Gate:
1. 0 exit-code mismatches на parity suite.

### PR-W2: Общий `write_core` (atomic + idempotent)
Scope:
1. Вынести `atomic_write`/`write_if_changed` в `write_core.rs`.
2. Добавить `DurabilityMode::{Durable, Fast}`.
3. Добавить CAS опцию и `WriteStats`.

Files:
1. `/Users/andrew/Programming/rtk/src/write_core.rs` (new)
2. `/Users/andrew/Programming/rtk/src/init.rs`
3. `/Users/andrew/Programming/rtk/src/utils.rs` (shared helpers if needed)

Gate:
1. Crash-safe tests green.
2. Legacy behavior `init` unchanged.

### PR-W3: Hook policy to avoid LLM confusion
Scope:
1. В `rtk-rewrite.sh` внедрить mutating guardrail.
2. Обновить `hook-audit` отчет: rewrite rate отдельно для mutating/read-only.
3. Добавить тесты rewrite policy.

Files:
1. `/Users/andrew/Programming/rtk/hooks/rtk-rewrite.sh`
2. `/Users/andrew/Programming/rtk/hooks/test-rtk-rewrite.sh`
3. `/Users/andrew/Programming/rtk/src/hook_audit_cmd.rs`

Gate:
1. При default конфиге mutating rewrite disabled.
2. LLM-facing behavior детерминирован и документирован.

### PR-W4: Optional `rtk write` command (safe primitives)
Scope:
1. Добавить базовые subcommands:
   - `rtk write replace` (safe scoped replace)
   - `rtk write patch` (apply hunk)
   - `rtk write set` (structured config set)
2. Всегда использовать `write_core`.

Files:
1. `/Users/andrew/Programming/rtk/src/write_cmd.rs` (new)
2. `/Users/andrew/Programming/rtk/src/main.rs`
3. `/Users/andrew/Programming/rtk/src/write_semantics.rs` (new)

Gate:
1. Dry-run + apply + rollback semantics покрыты integration tests.

### PR-W5: Benchmark harness + quality gates
Scope:
1. Добавить write benchmark suite и analyzer.
2. Ввести CI-gates по parity/latency/durability.

Files:
1. `/Users/andrew/Programming/rtk/benchmarks/write/bench_write.sh` (new)
2. `/Users/andrew/Programming/rtk/benchmarks/write/analyze_write.py` (new)
3. `/Users/andrew/Programming/rtk/benchmarks/write/results_raw.csv` (generated)
4. `/Users/andrew/Programming/rtk/benchmarks/write/RESULTS.md` (generated)

Gate:
1. См. раздел "Benchmark plan + thresholds".

## Benchmark plan + thresholds

### A. Micro I/O benchmarks (Rust path only)
Цель: измерить чистую стоимость write pipeline.

Scenarios:
1. `small.txt` 1 KiB, `medium.txt` 128 KiB, `large.txt` 8 MiB.
2. unchanged write (idempotent skip).
3. changed write (`durable`).
4. changed write (`fast`).

Metrics:
1. latency p50/p95 (ms)
2. throughput (MiB/s)
3. write amplification (`bytes_written / file_size`)
4. fsync count, rename count

Target:
1. unchanged path: < 2 ms p50 for small/medium.
2. changed durable path: <= 1.25x latency vs native safe baseline.
3. fast mode: заметно быстрее durable на small files.

### B. Durability/crash benchmarks
Цель: подтвердить отсутствие частично записанных файлов.

Scenarios:
1. fault injection between write and rename.
2. fault injection after rename before parent fsync.
3. interrupted process (kill) in stress loop.

Metrics:
1. corruption rate
2. orphan temp file rate
3. successful recovery rate

Target:
1. corruption rate = 0.
2. target file always valid pre- or post-version.

### C. Semantic parity benchmarks (native vs RTK mutating wrappers)
Цель: доказать, что rewrite mutating команд безопасен.

Command matrix:
1. `git add`, `git commit`, `git push`, `git pull`, `git fetch`, `git stash`, `git worktree`.
2. success and failure variants (auth fail, conflict, no remote, nothing to commit, etc.).

Metrics:
1. exit_code_match_rate
2. side_effect_match_rate (repo state diff)
3. stderr_key_signal_match_rate

Target (gate):
1. exit_code_match_rate = 100%
2. side_effect_match_rate = 100%
3. stderr_key_signal_match_rate >= 99%

### D. LLM confusion proxy benchmarks (hook rewrite)
Цель: оценить риск "агент не понял подмену".

Scenarios:
1. hook audit log classification by command class.
2. measure immediate retries of native command after rewritten command.
3. measure failures immediately after rewrite.

Metrics:
1. mutating_rewrite_rate (default should be ~0)
2. post_rewrite_retry_rate
3. post_rewrite_failure_rate

Target:
1. default mutating_rewrite_rate = 0%
2. retry/failure rates не выше native baseline

## Реализация benchmark tooling
1. Использовать существующий стиль benchmark-папок:
   - `/Users/andrew/Programming/rtk/benchmarks/` уже содержит script + analyzer pattern.
2. Новый pipeline:
   - `bash benchmarks/write/bench_write.sh`
   - `python3 benchmarks/write/analyze_write.py`
   - `python3 -m unittest discover -s benchmarks/write/tests -p 'test_*.py'`
3. Результаты:
   - `results_raw.csv`
   - `results_env.txt`
   - `RESULTS.md`

## Критерии приемки (DoD)
1. P0 semantic drift в mutating wrappers устранен.
2. Общий атомарный write-core внедрен и используется как стандарт.
3. Hook policy исключает опасную автозамену mutating команд по умолчанию.
4. Benchmark suite воспроизводим и запускается локально/в CI.
5. Gate thresholds выполняются до включения mutating rewrite в production-default.

## Короткая практическая рекомендация по rollout
1. Сначала PR-W1 и PR-W3 (parity + rewrite guardrail).
2. Затем PR-W2 (atomic core).
3. После этого PR-W5 (bench gates).
4. И только после прохождения gate — обсуждать расширение `rtk write` и/или mutating auto-rewrite.

---

## PR-W6: Write path hardening (post-audit, 2026-02-17)

Выявлено по результатам аудита LLM-понятности, fuzziness-анализа, clippy (88 warnings), Context7 cross-ref.

### Iteration 1: Quick wins (P0 + P1 cleanup)

#### P0-1: Убрать двойной idempotent-check в write_cmd
**Проблема**: `run_replace`/`run_patch`/`run_set` уже вычисляют `updated` и знают, что контент менялся, но `AtomicWriter` повторно читает файл в `is_unchanged()`.
**Замер**: worst-case 8MB fast: ~2009us → ~1000us при assume_changed (≈2x).
**Решение**: передавать `WriteOptions { idempotent_skip: false, .. }` из `write_cmd` когда diff уже известен. Добавить CAS CLI-флаги (`--if-match-mtime`, `--if-match-len`, `--if-match-hash`) и прокидывать `WriteOptions.cas` из write_cmd.
**Файлы**: `src/write_cmd.rs:95,132,248`, `src/write_core.rs:137`.

#### P0-2: Single-pass replace/patch без двойного скана
**Проблема**: `matches().count()` + `replace()`/`replacen()` = 2 прохода + полная копия.
**Решение**:
- `--all`: считать count и строить результат за один проход.
- Без `--all`: `find()` + `replacen(..., 1)`, без предварительного `count`.
**Файлы**: `src/write_cmd.rs:49,54-58,132,137-141`.

#### P1-1: Убрать I/O метрики из LLM-facing output
**Проблема**: `fsync=N, rename=N, skipped=bool` в stdout — noise для LLM-агента.
**Решение**: перенести в `--verbose 2`, оставить в default только `ok replace: N occurrence(s)`.
**Файлы**: `src/write_cmd.rs:97-99,180-182,249-251`.

#### P1-2: Убрать WriteSemantics debug format из output
**Проблема**: `(Replace/Text)` в no-op и dry-run сообщениях — декоративный тип, LLM его не использует.
**Решение**: удалить `{:?}` formatting из println!. `write_semantics.rs` оставить для документации контракта, но не выводить в user-facing output.
**Файлы**: `src/write_cmd.rs:62-65,80-84,144-148,163-167,232-237`.

#### P1-3: Исправить dead code в `run_stash`
**Проблема**: `timer.track()` на строках 1181-1186 и 1215 недостижим при failure (после `exit_with_git_failure -> !`).
**Решение**: вынести `timer.track` из `let msg = if ... { } else { exit }` pattern — tracking для success-path должен быть внутри success-ветки.
**Файлы**: `src/git.rs:1162-1186,1200-1216`.

#### P1-4: Исправить persist error handling — сохранить error chain
**Проблема**: `map_err(|e| anyhow!(..., e.error))` теряет error chain и temp file handle.
**Решение**: `map_err(|e| anyhow::Error::new(e.error).context(format!(...)))`.
**Файлы**: `src/write_core.rs:183-189`.

#### P1-5: Очистить dead code (clippy warnings)
**Проблема**: `FileSnapshot`, `snapshot_file`, `CasOptions::from_snapshot` — 0 callers в production.
**Решение**: После P0-1 (CAS CLI-флаги) эти станут used. До тех пор — `#[allow(dead_code)]` с `// Used by CAS CLI flags (PR-W6 P0-1)` comment.
**Файлы**: `src/write_core.rs:17,31,208`.

### Iteration 2: Core improvements (P1 features + P2)

#### P1-6: `--json`/`--quiet` output mode для write
**Проблема**: 14-21 токен статуса на операцию. Для LLM-цепочек нужен машинный ответ.
**Решение**: `--json` → `{"ok":true,"applied":1,"file":"..."}`, `--quiet` → пустой stdout при success.
**Файлы**: `src/write_cmd.rs`, `src/main.rs` (CLI flags).

#### P1-7: Batch-режим `rtk write batch --plan <json>`
**Проблема**: startup процесса ~34-35ms. При серии правок от LLM — overhead доминирует.
**Решение**: принимать JSON-план с массивом операций, группировать fsync/rename по parent dirs.
**Файлы**: `src/write_cmd.rs` (new `run_batch`), `src/main.rs`.

#### P1-8: Symlink/TOCTOU защита
**Проблема**: `path.exists()` + metadata read → TOCTOU race. Symlink-target может отличаться от ожидаемого.
**Решение**: `--allow-symlink` flag (default: reject symlinks). File-lock (advisory) для multi-agent сценариев.
**Файлы**: `src/write_core.rs:127,183`.

#### P2-1: JSON `--key ""` — запретить перезапись root
**Проблема**: `set_json_path` при пустом key делает `*root = value` — перезаписывает весь файл.
**Решение**: `bail!("--key must be non-empty")` аналогично TOML-ветке.
**Файлы**: `src/write_cmd.rs:372-377`.

#### P2-2: Strict mode для `--value-type auto`
**Проблема**: невалидный JSON молча превращается в строку (fallback `Ok(JsonValue::String(raw))`).
**Решение**: `--strict` flag — при auto-detection failure возвращать ошибку вместо string fallback.
**Файлы**: `src/write_cmd.rs:292-297`.

#### P2-3: Fast/durable policy по типу файла
**Проблема**: для temp/generated файлов `--fast` даёт 2-3x speedup, но нужно явно указывать.
**Решение**: policy config в `~/.config/rtk/config.toml`: `[write] default_durability = "fast"` + per-glob overrides.
**Файлы**: `src/write_cmd.rs`, `src/config.rs`.

#### P2-4: Объединить replace/patch или чётко документировать разницу
**Проблема**: ~170 строк дупликации, функционально идентичные операции с разными flag names.
**Решение**: общий internal `run_text_replace(mode: ReplaceMode)` + aliases.
**Файлы**: `src/write_cmd.rs:31-195`.

### Gate criteria (PR-W6)
1. clippy clean на write-модулях (0 warnings в write_core/write_cmd/write_semantics).
2. Benchmark: single-pass replace не медленнее текущего на 128 KiB.
3. LLM-facing output: <= 8 токенов на success operation (default mode).
4. Все тесты `cargo test write` green.
5. `--json` output parseable `jq` без ошибок.
