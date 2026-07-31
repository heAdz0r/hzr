# ADR-0002: Hybrid Retrieval and Two-Stage Ranking

**Status**: Accepted  
**Date**: 2026-02-18

## Context

Ни один источник сам по себе не дает оптимальный context quality:

1. RTK структурные индексы дают точность структуры, но не всегда intent relevance.
2. grepai дает semantic relevance, но может шуметь на широких запросах.
3. История прошлых сессий дает практическую полезность, но подвержена смещению.

## Decision

Использовать **hybrid candidate generation** и **two-stage ranking**:

1. Candidate pool = RTK L0-L6 + grepai semantic hits + episodic affinity + causal links.
2. Stage-1 ranking: deterministic Rust scorer (всегда доступен, быстрый).
3. Stage-2 ranking: optional local rerank (Ollama) только для top-K.
4. Финальная сборка идет через budget-aware optimizer.

## Consequences

1. Улучшение качества без потери deterministic fallback.
2. Увеличение сложности feature engineering.
3. Нужен мониторинг качества модели и fallback rate.

## Alternatives Considered

1. Только grepai retrieval.
1. Отклонено: слабее структурный сигнал и risk of semantic drift.
2. Только rule-based routing по `query_type`.
1. Отклонено: ограниченный quality ceiling.

