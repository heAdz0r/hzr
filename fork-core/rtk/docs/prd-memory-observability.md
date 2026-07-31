# PRD: RTK Memory Layer — Observability & Dev Environment

**Version**: 1.0  
**Date**: 2026-02-19  
**Status**: Ready for implementation  
**Scope**: `src/memory_layer/mod.rs`, `src/tracking.rs`, `src/discover/`, новые subcommands

---

## Problem Statement

Memory layer технически работает (hook инжектирует контекст, dirty detection корректен, 99.9% savings).  
Но у разработчика нет ответа на вопрос **"всё ли настроено и работает прямо сейчас?"** без запуска 4–5 разных команд.  
Кроме того, реальный эффект memory layer invisible в `rtk gain` — нет строки "memory hook fired N times, saved X KB".

---

## Goals

| ID | Goal |
|----|------|
| G1 | Один `rtk memory doctor` заменяет ручную проверку 5 команд |
| G2 | `rtk memory setup` — идемпотентный installer: два шага в одном |
| G3 | `rtk gain -p` показывает memory layer строку наравне с другими командами |
| G4 | `rtk discover` детектирует Task-вызовы без memory context injection |
| G5 | `rtk memory devenv` запускает готовую tmux-сессию из бинаря |

---

## Non-Goals

- Замена grepai watch на rtk memory watch (оба нужны, разные индексы)
- Интеграция с облаком или внешними сервисами
- GUI/TUI
- Изменение схемы mem.db

---

## Feature Specifications

### F1 — `rtk memory doctor`

**Command**: `rtk memory doctor [PROJECT]`  
**Purpose**: Единая диагностика — выход 0 если всё ок, выход 1 если есть проблема.

**Output (all ok):**
```
[ok] hook: rtk-mem-context.sh registered (PreToolUse:Task)
[ok] hook: rtk-block-native-explore.sh registered (PreToolUse:Task)
[ok] cache: fresh, files=329, updated=42s ago
[ok] memory.gain: raw=3.1MB → context=3.9KB (99.9% savings)
[ok] rtk binary: 0.21.1-fork.10
```

**Output (problems):**
```
[ok] hook: rtk-mem-context.sh registered (PreToolUse:Task)
[FAIL] hook: rtk-block-native-explore.sh — NOT in settings.json
       Fix: rtk init -g --auto-patch
[WARN] cache: stale, updated=90000s ago
       Fix: rtk memory refresh .
[ok] memory.gain: raw=3.1MB → context=3.9KB (99.9% savings)
```

**Exit codes**:
- `0` — все проверки прошли
- `1` — есть `[FAIL]` (критично)
- `2` — есть `[WARN]` но нет `[FAIL]` (некритично)

**Checks (в порядке выполнения)**:
1. Читает `~/.claude/settings.json`, ищет оба Task hook по имени файла
2. Вызывает `rtk memory status` — проверяет `fresh`/`stale`/`dirty`
3. Вызывает `rtk memory gain` — выводит компактную строку
4. Проверяет что бинарь rtk найден в PATH

---

### F2 — `rtk memory setup`

**Command**: `rtk memory setup [--auto-patch] [--no-watch]`  
**Purpose**: Полная установка за один вызов.

**Steps** (каждый идемпотентен):
1. `rtk init -g --auto-patch` (или с prompt если без `--auto-patch`)
2. `rtk memory install-hook`
3. `rtk memory refresh .`
4. `rtk memory doctor` (финальная проверка, выводит итог)

**Output:**
```
RTK Memory Layer Setup

[1/4] installing policy hooks...     ok (6 hooks, settings.json patched)
[2/4] installing memory context...   ok (rtk-mem-context.sh registered)
[3/4] building memory cache...       ok (329 files, 3.1MB → 3.9KB)
[4/4] running doctor...

[ok] hook: rtk-mem-context.sh registered
[ok] hook: rtk-block-native-explore.sh registered
[ok] cache: fresh, files=329
[ok] memory.gain: 99.9% savings

Setup complete. Restart Claude Code if hooks were just added.
```

---

### F3 — `rtk gain -p` memory layer строка

**Изменение**: добавить строку `rtk memory (hook)` в таблицу "By Command".

**Источник данных**: `mem.db` таблица `cache_events`, колонки `event_type` (hit/miss/rebuild), `project_id`.

**Расчёт**:
- Count = число `explore` + `plan` событий за период
- Saved = сумма `(raw_bytes - context_bytes)` из `cache_stats`
- Avg% = среднее `savings_pct`

**Output в gain -p:**
```
 6.  rtk memory (hook)             847   847.0M   99.9%    12ms  ██░░░░░░░░
```

**Fallback**: если mem.db не найден или нет записей — строка не отображается (нет ошибок).

---

### F4 — `rtk discover` memory miss detection

**Изменение**: новая секция в выводе `rtk discover`.

**Логика**: сканировать JSONL session events, искать `PreToolUse` с `tool_name=Task` где `tool_input.prompt` не содержит `RTK Project Memory Context` (маркер инжекции из `rtk-mem-context.sh`).

**Output:**
```
Memory Context Misses (Task calls without RTK memory injection)
──────────────────────────────────────────────────────────────
  3 Task calls had no memory context injected.
  Likely cause: rtk-mem-context.sh not in settings.json, or hook failed.
  Fix: rtk memory doctor
  
  Sessions affected:
    2026-02-19 14:23 — subagent_type=Explore, prompt="scan auth files..."
    2026-02-19 15:01 — subagent_type=general-purpose, prompt="refactor..."
    2026-02-19 15:44 — subagent_type=Plan, prompt="design new feature..."
```

**Если все Task calls имеют инжекцию:**
```
[ok] Memory context: all Task calls had RTK memory injected (47/47)
```

---

### F5 — `rtk memory devenv`

**Command**: `rtk memory devenv [PROJECT] [--interval N] [--session-name NAME]`  
**Purpose**: Запустить tmux-сессию с 3 панелями из бинаря.

**Поведение**:
- Если tmux не установлен — выводит инструкцию с командами для запуска вручную
- Если сессия `rtk` уже существует — attach (не создавать дублей)
- Resolve project root автоматически (walk up к .git)
- Параметры: `--interval` (debounce для memory watch, default 2), `--session-name` (default `rtk`)

**Панели:**
```
┌──────────────────────────┬──────────────────────────┐
│ pane 0: grepai watch     │ pane 1: rtk memory watch │
│ grepai watch --background│ rtk -v memory watch .    │
│ + tail -F grepai log     │ --interval N             │
├──────────────────────────┴──────────────────────────┤
│ pane 2: health loop (×10s)                          │
│ rtk memory status + gain -p --ultra-compact + doctor│
└─────────────────────────────────────────────────────┘
```

**Если tmux недоступен — вывести готовые команды:**
```
tmux not found. Run these in separate terminals:

  Terminal 1: grepai watch
  Terminal 2: rtk -v memory watch . --interval 2
  Terminal 3: while true; do
                clear
                rtk memory doctor
                rtk gain -p --ultra-compact
                sleep 10
              done
```

---

## Acceptance Criteria

| Feature | Criterion |
|---------|-----------|
| F1 doctor | `rtk memory doctor` exit 0 на корректно настроенной машине |
| F1 doctor | `rtk memory doctor` exit 1 если rtk-block-native-explore.sh отсутствует в settings.json |
| F1 doctor | `rtk memory doctor` exit 2 если cache stale > TTL |
| F2 setup | `rtk memory setup --auto-patch` на чистой машине: doctor exit 0 в конце |
| F2 setup | Повторный вызов `rtk memory setup` идемпотентен, нет дублей в settings.json |
| F3 gain | `rtk gain -p` содержит строку `rtk memory (hook)` если есть cache_events |
| F3 gain | Строка не появляется на машине где memory никогда не использовалось |
| F4 discover | `rtk discover` выводит секцию `Memory Context Misses` |
| F4 discover | При 0 miss — выводит `[ok] Memory context: all Task calls had RTK memory injected` |
| F5 devenv | `rtk memory devenv` создаёт tmux-сессию с 3 панелями |
| F5 devenv | Повторный вызов attach к существующей сессии, не создаёт дубль |
| F5 devenv | При отсутствии tmux — выводит команды для ручного запуска |

---

## Testing Requirements

- Unit: `doctor` checks каждый `[ok]`/`[FAIL]`/`[WARN]` в изоляции (mock settings.json / mem.db)
- Unit: `setup` шаги — каждый вызывается ровно один раз даже при повторном запуске
- Unit: gain строка появляется при наличии cache_events, отсутствует при пустой БД
- Unit: discover парсит Task event с/без RTK prefix и классифицирует корректно
- Integration: `rtk memory setup --auto-patch` в temp HOME, doctor exit 0 в конце
