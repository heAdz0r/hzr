# Tasks: Memory Layer Observability & Dev Environment

**PRD**: `docs/prd-memory-observability.md`  
**ADR**: `docs/adr/ADR-0006-memory-layer-devenv-observability.md`  
**Date**: 2026-02-19  
**Status**: Ready

---

## T1 — `rtk memory doctor`

**File**: `src/memory_layer/mod.rs` (новая pub fn `run_doctor`)  
**Clap**: добавить `Doctor { project: PathBuf, verbose: u8 }` в `MemoryCommands`  
**main.rs**: маршрут `MemoryCommands::Doctor`

### Шаги реализации

```
1. Прочитать ~/.claude/settings.json
   - Найти PreToolUse array
   - Проверить наличие записи с command содержащим "rtk-mem-context.sh"
   - Проверить наличие записи с command содержащим "rtk-block-native-explore.sh"
   - Каждая проверка → [ok] или [FAIL] + подсказка

2. rtk memory status (переиспользовать run_status):
   - fresh → [ok]
   - stale → [WARN] + "Fix: rtk memory refresh ."
   - dirty → [WARN] + "Fix: rtk memory refresh ."

3. rtk memory gain (переиспользовать run_gain):
   - вывести одну строку: raw → context (savings%)
   - всегда [ok] (информационно)

4. Проверить rtk в PATH: std::process::Command::new("rtk").arg("--version")
   - ok → [ok] rtk binary: <version>
   - err → [WARN]

5. Подсчитать итог и вернуть exit code:
   - есть [FAIL] → exit 1
   - есть [WARN] только → exit 2
   - всё [ok] → exit 0
```

### Тесты

```rust
// В #[cfg(test)] mod tests:
// test_doctor_all_ok — все проверки проходят на mock данных
// test_doctor_missing_mem_context — settings.json без rtk-mem-context → FAIL, exit 1
// test_doctor_missing_block_explore — settings.json без rtk-block-native-explore → FAIL, exit 1
// test_doctor_stale_cache — artifact.updated_at старый → WARN, exit 2
// test_doctor_both_missing — оба hook отсутствуют → два FAIL, exit 1
```

---

## T2 — `rtk memory setup`

**File**: `src/memory_layer/mod.rs` (новая pub fn `run_setup`)  
**Clap**: `Setup { auto_patch: bool, no_watch: bool, verbose: u8 }`

### Шаги реализации

```
1. Напечатать "RTK Memory Layer Setup\n"

2. [1/4] installing policy hooks
   - вызвать crate::init::run_default_mode(global=true, PatchMode::Auto или Ask, verbose)
   - вывести "ok (N hooks, settings.json patched)" или ошибку

3. [2/4] installing memory context hook  
   - вызвать run_install_hook(uninstall=false, status_only=false, verbose)
   - вывести "ok (rtk-mem-context.sh registered)"

4. [3/4] building memory cache
   - вызвать run_refresh(project, verbose)
   - вывести "ok (N files, X.XMB → X.XKB)"

5. [4/4] running doctor
   - вызвать run_doctor(project, verbose)
   - doctor выводит свои строки

6. Итоговое сообщение:
   "Setup complete. Restart Claude Code if hooks were just added."
   Если doctor вернул exit != 0 → "Setup completed with warnings. See [FAIL]/[WARN] above."
```

### Тесты

```rust
// test_setup_idempotent — два вызова подряд не создают дублей в settings.json
// test_setup_auto_patch — с флагом не спрашивает пользователя
// test_setup_ends_with_doctor_ok — интеграционный: run_setup в temp HOME, doctor exit 0
```

---

## T3 — `rtk gain -p` memory layer строка

**File**: `src/tracking.rs` (метод `get_summary` или `get_project_summary`)  
**File**: `src/memory_layer/cache.rs` (новая fn `get_memory_gain_stats(project_id)`)

### Шаги реализации

```
1. В cache.rs добавить fn get_memory_gain_stats(project_id: &str) -> Result<Option<MemoryGainStats>>:
   struct MemoryGainStats {
       hook_fires: u64,    // число cache_events типа explore/plan
       raw_bytes_total: u64,
       context_bytes_total: u64,
       savings_pct: f64,
       avg_latency_ms: f64,
   }
   SQL: SELECT count(*), sum(raw_bytes), sum(context_bytes), avg(latency_ms)
        FROM cache_stats WHERE project_id = ?

2. В tracking.rs::get_summary() / get_project_summary():
   - Вызвать get_memory_gain_stats(project_id)
   - Если Some(stats) и stats.hook_fires > 0 → добавить в CommandSummary строку
     command = "rtk memory (hook)"
     count = stats.hook_fires
     saved = stats.raw_bytes_total - stats.context_bytes_total
     avg_pct = stats.savings_pct

3. Форматирование в gain output:
   - Строка появляется в таблице "By Command" если hook_fires > 0
   - Сортируется по saved (может быть в топе при активном использовании)
```

### Тесты

```rust
// test_memory_gain_stats_empty — нет записей → None
// test_memory_gain_stats_with_data — 3 cache events → корректная агрегация
// test_gain_output_has_memory_row — при наличии stats строка присутствует в выводе
// test_gain_output_no_memory_row — при пустой БД строки нет
```

---

## T4 — `rtk discover` memory miss detection

**File**: `src/discover/provider.rs` (новая категория `MemoryMiss`)  
**File**: `src/discover/registry.rs` (регистрация нового checker)

### Шаги реализации

```
1. Определить маркер инжекции (константа):
   const MEM_CONTEXT_MARKER: &str = "RTK Project Memory Context";

2. В provider.rs добавить fn check_memory_misses(events: &[SessionEvent]) -> Vec<MemoryMissEvent>:
   - Фильтр: event.type == PreToolUse && event.tool_name == Task
   - Для каждого: проверить есть ли MEM_CONTEXT_MARKER в tool_input.prompt
   - Если нет → MemoryMissEvent { timestamp, subagent_type, prompt_prefix }

3. В registry.rs зарегистрировать checker в пайплайн discover:
   - Секция в выводе называется "Memory Context Misses"
   - Показывать: число miss, список (timestamp, subagent_type, первые 60 chars prompt)
   - Если miss == 0: "[ok] Memory context: all Task calls had RTK memory injected (N/N)"
   - Подсказка при miss > 0: "Fix: rtk memory doctor"

4. Учесть edge case: если tool_input.prompt отсутствует (hook misfired) → тоже miss
```

### Тесты

```rust
// test_memory_miss_no_task_events — нет Task событий → 0 miss
// test_memory_miss_all_injected — все Task с маркером → 0 miss, ok output
// test_memory_miss_some_missing — 2 из 5 без маркера → 2 miss, список
// test_memory_miss_null_prompt — prompt=null → считается miss
```

---

## T5 — `rtk memory devenv`

**File**: `src/memory_layer/mod.rs` (новая pub fn `run_devenv`)  
**Clap**: `Devenv { project: PathBuf, interval: u64, session_name: String }`

### Шаги реализации

```
1. Resolve project root (walk up от CWD к .git)

2. Проверить tmux в PATH:
   Command::new("tmux").arg("-V") → Ok / Err

3. Если tmux недоступен → вывести fallback инструкцию (3 команды для отдельных терминалов) → return Ok(())

4. Проверить существование сессии:
   tmux has-session -t <session_name>
   - Если уже существует → tmux attach-session -t <session_name> → return

5. Создать сессию:
   tmux new-session -d -s <session_name>

6. Pane 0 (grepai watch):
   tmux send-keys -t <session_name>:0.0 "grepai watch" Enter

7. Pane 1 (rtk memory watch):
   tmux split-window -h -t <session_name>:0
   tmux send-keys -t <session_name>:0.1 
     "rtk -v memory watch {project} --interval {interval}" Enter

8. Pane 2 (health loop):
   tmux split-window -v -t <session_name>:0.1
   Отправить while true loop с: clear + memory doctor + gain -p --ultra-compact + sleep 10

9. Выровнять: tmux select-layout -t <session_name>:0 even-horizontal

10. Attach: tmux attach-session -t <session_name>
```

### Тесты

```rust
// test_devenv_no_tmux — tmux не найден → выводит fallback, exit 0
// test_devenv_commands_built_correctly — проверить что команды содержат правильные аргументы
//   (не запускать настоящий tmux в тестах — mock Command)
```

---

## Порядок реализации

```
T1 (doctor)   →  T2 (setup)   →  T3 (gain)   →  T4 (discover)   →  T5 (devenv)
   ↑                ↑
   зависит от       зависит от T1
   status/gain
```

T3 и T4 можно делать параллельно после T1.  
T5 независим, можно начать в любой момент.

---

## Definition of Done

- [ ] `cargo test` зелёный после каждого T
- [ ] `cargo clippy --all-targets` без новых ошибок
- [ ] `cargo fmt --all --check` проходит
- [ ] `rtk memory doctor` exit 0 на текущей машине разработчика
- [ ] `rtk discover` выводит секцию Memory Context Misses
- [ ] `rtk gain -p` содержит строку `rtk memory (hook)` если был хотя бы один inject
- [ ] `bash scripts/validate-docs.sh` проходит
- [ ] Bump patch version: fork.10 → fork.11
