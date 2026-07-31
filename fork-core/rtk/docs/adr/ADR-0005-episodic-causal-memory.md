# ADR-0005: Persist Episodic and Causal Agent Memory in SQLite

**Status**: Accepted  
**Date**: 2026-02-18

## Context

Структурный индекс отвечает на вопрос "как устроен код", но не отвечает на вопрос "как агент пришел к изменению и что сработало".
Для повторных задач нужен устойчивый слой знаний о траекториях решений.

## Decision

1. Добавить persistent tables: `episodes`, `episode_events`, `task_file_affinity`, `causal_links`.
2. Фиксировать цепочку:
1. task/start
2. investigated files/symbols
3. edits/delta
4. final outcome/feedback
3. Использовать эти данные в ranking features (affinity + causal relevance).
4. Ввести retention policy (default 90 days) и безопасный purge path.

## Consequences

1. Повторные агентные задачи ускоряются и требуют меньше re-exploration.
2. Появляется data governance responsibility (retention, redaction).
3. Возрастает объем БД, нужны индексы и housekeeping.

## Alternatives Considered

1. Хранить только агрегированные счетчики без событий.
1. Отклонено: слабая диагностируемость и объяснимость решений.
2. Хранить эпизоды во внешнем vector DB.
1. Отклонено: operational complexity и потеря local-first простоты.

