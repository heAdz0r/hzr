Отличный вектор. Чтобы поднять все 3 критерия, нужен не один “фиче-добавочный” шаг, а retrieval stack как в IDE+search системах.

Как поднять критерии

Продуктовая новизна (8 -> 9/10)
Сделать memory plan не просто списком файлов, а “планом исследования”:
why this file, next hop, confidence, skip reason.
Добавить RU/EN intent normalization (сейчас это даст большой практический wow).
Ввести режимы incident/feature/refactor с разной стратегией графа и бюджета.
Инженерная уникальность (7 -> 9/10)
Построить двойной индекс:
структурный graph index (imports/calls/symbol refs/tests),
lexical/semantic index (BM25 + embeddings).
Делать retrieval в 3 шага:
graph-prior shortlist,
lexical+semantic rerank внутри shortlist,
budget-aware pack с diversity constraints.
Добавить строгий eval harness: precision@k, MRR, latency p95, token ROI.
Алгоритмическая новизна (4 -> 6-7/10)
Ввести cost-aware graph retrieval (под токен-бюджет, не просто top-k score).
Использовать MMR/diversity на уровне модулей, чтобы не забивать контекст одним файлом.
Учить веса ранжирования на реальных trace-данных агента (offline learning-to-rank).
Что взять из зрелых технологий

LSP/SCIP/LSIF
references, callHierarchy, workspace symbols, semantic tokens.
Это даст точный graph layer вместо regex-only эвристик.
Zoekt/Sourcegraph идеи
Trigram/BM25 prefilter до semantic stage.
Очень быстрое candidate narrowing.
IR/поисковые движки (Lucene/Tantivy)
BM25 как обязательный “middle stage”, чтобы не зависеть только от embeddings.
Tree-sitter
Унифицированный AST extraction для языков без полноценного LSP.
Приоритетный план (чтобы реально поднять оценки)

Починить текущие 4 пункта (пустой контекст при 1800, hook fail-open, API trace/format, telemetry project_id).
Добавить BM25 индекс (Tantivy) и объединить с graph shortlist.
Подключить LSP/SCIP references для Rust/TS как primary graph source.
Ввести MMR + module diversity в budget assembler.
Запустить оффлайн benchmark набор из 100-300 реальных задач (RU+EN).
Обучить и зафиксировать intent-conditioned ranking weights по trace-данным.
Если сделать эти 6 шагов, это уже будет не “еще один memory layer”, а сильный отдельный класс инструмента.
