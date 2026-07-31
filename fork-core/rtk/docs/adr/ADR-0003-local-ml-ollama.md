# ADR-0003: Local ML via Ollama with Rust-First Fallback

**Status**: Accepted  
**Date**: 2026-02-18

## Context

Пользовательский сценарий предполагает локальный `ollama`, и продукт должен использовать ML без отправки кода в облако.
При этом reliability и latency не могут зависеть от состояния ML слоя.

## Decision

1. ML-интеграция выполняется локально через Ollama adapters:
1. intent classification (optional)
2. top-K reranking (optional)
2. Обязательный runtime путь всегда проходит через Rust stage-1 ranker.
3. При недоступности Ollama система деградирует в deterministic mode без потери функциональности.
4. Контракты с Ollama строго JSON-only, с timeout и schema validation.

## Consequences

1. Продукт сохраняет local-first privacy.
2. ML дает quality uplift при доступном latency budget.
3. Появляется необходимость жесткого timeout/circuit-breaker контроля.

## Alternatives Considered

1. Обязательный ML path без fallback.
1. Отклонено: unacceptable reliability risk.
2. Cloud hosted reranker.
1. Отклонено: против local-first/privileged code privacy.

