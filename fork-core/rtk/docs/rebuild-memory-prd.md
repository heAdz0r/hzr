PRD: Memory Layer Graph-First + Targeted rgai Recovery (Core Path P0)
Summary
Восстанавливаем главный продуктовый путь memory layer:

Сначала строим кандидатный граф/дерево файлов из структурной памяти проекта.
Затем запускаем семантический поиск rgai только по этим кандидатам.
После этого собираем контекст под бюджет токенов.
Это становится дефолтным путём для rtk memory plan и hook-инъекции в Task, с fail-open fallback на legacy pipeline.

Current State (зафиксировано на 2026-02-19)
Hook уже использует memory plan как главный путь: rtk-mem-context.sh (line 63).
plan_context_inner не вызывает rgai/grepai и строит кандидатов только из artifact.files: mod.rs (line 1152).
План-вывод даёт нерелевантные top-файлы (__init__.py, release-манифест), что подтверждает поломку ценностного пути.
ADR для hybrid retrieval уже принят, но не реализован в runtime: ADR-0002-hybrid-retrieval-ranking.md.
Goals
Вернуть качество выбора файлов для задачи до уровня “сначала релевантный graph/tree, потом semantic”.
Снизить шум в context (marker/infra файлы не доминируют).
Сохранить скорость (latency в пределах текущего UX, с fallback).
Сохранить совместимость hook/API.
Non-Goals
Полная реализация IMG (episodic/causal/model registry) в этом PRD.
Изменение explore/delta/refresh контрактов.
Полный ML rerank через Ollama в critical path.
Product Requirements
R1. Graph-First Candidate Builder (обязательный этап 1)
Ввести модуль planner_graph и строить candidate graph из:
artifact files,
import edges,
call-graph edges,
path/domain signals.
Формировать 3 уровня:
Tier A seeds (прямое task/path/symbol совпадение),
Tier B neighbors (1 hop по import/call graph),
Tier C fallback (ограниченный пул для recall).
Жёсткий cap пула: plan_candidate_cap=60 (конфиг).
Добавить noise-фильтр до semantic этапа:
.rtk-lock всегда исключать,
tiny marker files (line_count <= 5 без imports/symbols) исключать,
test/docs/config файлы без task overlap не пускать в Tier A/B.
Сделать язык-независимый token path:
не опираться на англоязычную intent-классификацию как единственный сигнал.
R2. Targeted Semantic Stage via rgai (обязательный этап 2)
Добавить внутренний API в rgai_cmd для поиска по фиксированному списку candidate файлов.
Backend policy (выбранный): rgai ladder.
grepai (global result -> intersect with candidate set),
fallback rg (candidate-scoped),
fallback builtin scorer (candidate-scoped).
Semantic stage должен возвращать:
semantic_score [0..1],
matched_terms,
snippet (token-safe short evidence),
semantic_backend_used.
Если semantic этап не дал валидных hit’ов, использовать graph score (fail-open).
R3. Fusion Ranking + Budget
Объединённый скоринг:
final_score = 0.65 *graph_score + 0.35* semantic_score (если semantic есть),
иначе final_score = graph_score.
Перед budget-ассемблером отбрасывать кандидатов ниже min_final_score=0.12, если нет semantic evidence.
Сохранить token budget hard cap и decision trace.
Ограничить долю test/docs/config в финальном наборе до 20%, если запрос не содержит explicit test/docs intent.
R4. Hook Output Quality
rtk-mem-context.sh остаётся на memory plan, но получает новый текстовый шаблон:
Graph Seeds,
Semantic Hits,
Final Context Files.
Дефолтный RTK_MEM_PLAN_BUDGET=1800 оставить.
При ошибке нового pipeline hook получает legacy memory plan output автоматически.
Public API / Interface Changes
CLI (main.rs)
Добавить флаг rtk memory plan --legacy для принудительного старого пути.
Добавить флаг rtk memory plan --trace для stage trace (text/json).
HTTP (api.rs)
POST /v1/plan-context request (additive):
legacy: bool (optional, default false),
trace: bool (optional, default false).
Response (additive):
pipeline_version: "graph_first_v1" | "legacy_v0",
semantic_backend_used: "grepai" | "rg" | "builtin" | "none",
graph_candidate_count,
semantic_hit_count,
selected[].semantic_score (optional),
selected[].evidence (optional short snippet).
Config (config.rs)
mem.features.graph_first_plan: bool = true.
mem.features.plan_fail_open: bool = true.
mem.plan_candidate_cap: usize = 60.
mem.plan_semantic_cap: usize = 30.
mem.plan_min_final_score: f32 = 0.12.
Internal Rust Types
pub struct PlanTrace {
    pub pipeline_version: String,
    pub graph_candidate_count: usize,
    pub semantic_hit_count: usize,
    pub semantic_backend_used: String,
}

pub struct SemanticEvidence {
    pub semantic_score: f32,
    pub matched_terms: Vec<String>,
    pub snippet: String,
}
Implementation Plan (decision-complete)
Phase 1: Pipeline split and legacy guard
В mod.rs вынести legacy plan_context_inner в plan_context_legacy.
Добавить plan_context_graph_first как новый default entry.
Добавить runtime switch через config/env/CLI --legacy.
Phase 2: Graph-first candidate module
Создать planner_graph.rs.
Реализовать построение Tier A/B/C и hard cap.
Реализовать noise filter (без repo-specific хардкода).
Подключить в plan_context_graph_first.
Phase 3: Targeted semantic engine
Добавить internal API в rgai_cmd.rs без изменения текущего CLI поведения.
Создать semantic_stage.rs.
Реализовать ladder backend и candidate-set intersection.
Вернуть semantic evidence в candidate model.
Phase 4: Fusion and response contracts
Обновить ranker.rs для fusion score.
Обновить budget.rs для pre-budget threshold filtering.
Обновить api.rs response schema (additive fields).
Обновить mod.rs text renderer для Graph Seeds / Semantic Hits.
Phase 5: Hook + observability + docs
Обновить rtk-mem-context.sh под новый trace-friendly text output.
Добавить события plan_graph_first, plan_legacy_fallback в cache/event telemetry.
Обновить MEMORY_LAYER.md и ADR links.
Test Cases and Scenarios
Unit
Graph candidate builder:
Tiny marker files не попадают в Tier A/B.
Source files с imports/symbols приоритетнее docs/config.
Semantic stage:
candidate-scoped filtering работает,
backend fallback order корректен,
semantic evidence сериализуется стабильно.
Fusion:
semantic hit повышает ранг релевантного source файла,
low-quality candidates drop по threshold.
Integration
rtk memory plan на этом репо с query memory layer broken graph candidates:
в top-5 минимум 3 файла из src/memory_layer/ или hook-related paths.
Query без слова test:
в top-10 нет __init__.py.
API /v1/plan-context:
additive response fields присутствуют,
old consumers не ломаются.
E2E Hook
PreToolUse Task инъекция содержит 3 секции (Graph Seeds, Semantic Hits, Final Context Files).
При отказе graph-first path включается legacy fallback и hook не падает.
Performance/Quality gates
memory plan p95 <= 250ms на cache-hit для среднего локального проекта.
semantic_hit_count > 0 минимум в 70% запросов к source-heavy задачам.
Снижение нерелевантных infra/marker файлов в финальном контексте минимум на 80% против текущего baseline.
Rollout
Release N: graph_first_plan=true default, plan_fail_open=true.
Canary в локальных hook-сессиях, сбор метрик fallback/hit quality.
При деградации quality: временный forced legacy через config или --legacy.
Assumptions and Defaults (зафиксировано)
Scope: Core Path Only.
Rollout: Default On + Fallback.
Semantic backend policy: rgai ladder.
Языковая стратегия: language-agnostic path (не зависеть от английской intent-модели).
Совместимость API/CLI: только additive изменения, без breaking contracts.
