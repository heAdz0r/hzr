# ADR-0004: Token Budget-Aware Context Assembly is a Hard Contract

**Status**: Accepted  
**Date**: 2026-02-18

## Context

Статичные режимы детализации (`compact|normal|verbose`) не гарантируют оптимум под конкретный token budget задачи.
Для агентной среды budget является runtime constraint, а не декоративным параметром.

## Decision

1. Ввести обязательный `token_budget` в `plan-context` контракты.
2. Сборка контекста делается оптимизатором utility-per-token.
3. Ответ всегда включает `budget_report` и `decision_trace`.
4. При невозможности уложиться в бюджет:
1. возвращается максимально полезный поднабор
2. сообщаются явно исключенные кандидаты и причины

## Consequences

1. Предсказуемое потребление токенов.
2. Улучшение explainability выбора контекста.
3. Требуется стабильный token estimator и тесты на budget compliance.

## Alternatives Considered

1. Оставить только `detail` переключатели.
1. Отклонено: нет гарантии budget fit.
2. Делегировать бюджетную оптимизацию полностью LLM.
1. Отклонено: теряется детерминизм и воспроизводимость.

