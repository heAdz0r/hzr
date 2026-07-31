# ADR-0006: Memory Layer — Dev Environment & Observability Architecture

**Status**: Accepted  
**Date**: 2026-02-19

## Context

После реализации memory layer (v4, fork.8–10) выявлено три операционных пробела:

1. **Установка требует двух независимых команд** (`rtk init -g` + `rtk memory install-hook`), которые легко пропустить — нет единой точки входа и нет валидации что оба шага выполнены.
2. **Мониторинг разрознен**: freshness проверяется через `rtk memory status`, экономия через `rtk memory gain`, hook-регистрация через `jq ~/.claude/settings.json`. Нет одной команды "всё хорошо?".
3. **Реальное использование невидимо**: `rtk gain -p` не показывает memory-слой, `rtk discover` не детектирует Task-вызовы без инжекции контекста.

Дополнительный контекст: grepai watch (semantic embeddings) и rtk memory watch (AST/symbol index) решают разные задачи и должны работать параллельно. Полагаться только на grepai как на единственный триггер ненадёжно — у grepai watch нет `--exec` hook, парсинг логов хрупок.

## Decision

**D1: Два параллельных watcher — grepai и rtk memory**  
Запускать оба независимо. grepai → vector embeddings для `rtk rgai`. rtk memory watch → mtime+hash delta для prompt injection. Никакого bridge между ними.

**D2: `rtk memory doctor` как единая точка диагностики**  
Одна команда проверяет: оба hook в settings.json, freshness кеша, `rtk memory gain` в компактном виде, hint если что-то отсутствует. Выход 0 = всё ок, выход 1 = есть проблема.

**D3: `rtk memory setup` как единый installer**  
Вызывает `rtk init -g --auto-patch` + `rtk memory install-hook` + `rtk memory refresh .` + `rtk memory doctor` в одной команде. Идемпотентен.

**D4: `rtk gain -p` включает memory-layer строку**  
В секцию "By Command" добавить отдельную строку `rtk memory (hook)` с числом инжекций, context KB и savings%, беря данные из mem.db `cache_events`.

**D5: `rtk discover` детектирует Task без memory context**  
Сканировать session JSONL на PreToolUse:Task события где отсутствует RTK memory prefix в tool_input.prompt. Выводить как `[mem-miss]` категорию.

**D6: tmux dev environment как `rtk memory devenv`**  
Встроенная команда запускает tmux-сессию с тремя панелями. Не внешний скрипт — бинарь знает где mem.db, какой проект, какой интервал.

## Consequences

- Добавляются 3 новых subcommand: `doctor`, `setup`, `devenv`.
- `rtk gain -p` требует join с mem.db cache_events таблицей.
- `rtk discover` требует анализа content PreToolUse событий в JSONL.
- `rtk memory doctor` становится gate для CI-проверки что среда настроена корректно.

## Alternatives Considered

1. **Парсить grepai log → trigger rtk memory refresh**  
   Отклонено: хрупко, grepai log format не является стабильным публичным API, нет debounce-гарантий, потери событий при burst.

2. **Единый daemon вместо двух watch процессов**  
   Отклонено: grepai и rtk memory — независимые проекты с разными схемами хранения. Объединение создаёт coupling без реального выигрыша.

3. **Внешний shell-скрипт для devenv вместо встроенной команды**  
   Отклонено: скрипт не знает RTK_MEM_DB_PATH, не умеет resolve project root, сложнее дистрибутировать.
