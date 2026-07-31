# ADR-0001: Intent Memory Graph as Next Memory Layer Architecture

**Status**: Accepted  
**Date**: 2026-02-18

## Context

RTK Memory Layer v4 эффективно решает структурный кэш и freshness, но не учитывает intent задачи и не хранит знание о прошлых агентных действиях.
Для целевого продукта нужен переход от "artifact cache" к "decision engine".

## Decision

Принять архитектуру **Intent Memory Graph (IMG)** как расширение `rtk memory`:

1. Сохранить L0-L6 как структурный baseline.
2. Добавить episodic и causal memory.
3. Ввести task-conditioned retrieval и budget-aware assembly как core path.
4. Весь runtime оставить в Rust для предсказуемой производительности.

## Consequences

1. Появляется единый локальный контур принятия контекстных решений.
2. Усложняется схема БД и API контракты.
3. Требуются новые performance gates (latency + precision uplift).

## Alternatives Considered

1. Оставить только L0-L6 + query_type routing.
1. Отклонено: низкий потолок улучшений и отсутствие уникальности.
2. Вынести decision engine в отдельный сервис на Python.
1. Отклонено: рост операционной сложности и снижение runtime-предсказуемости.

