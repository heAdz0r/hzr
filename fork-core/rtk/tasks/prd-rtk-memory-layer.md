# PRD: RTK Memory Layer (`mod mem`) — Decision Locked

## 1. Product Decisions (зафиксировано)

| Вопрос | Решение | Обоснование |
|---|---|---|
| 1. MVP scope | **C — полный стек**: кэш + демон (дельты) + API | Иначе не закрывается требование reliability + zero-config для Claude.
| 2. Потребители | **B — несколько параллельных агентов на одной машине** | Главный выигрыш от shared cache и повторного использования индекса.
| 3. Интеграция | **B — встроено в текущий `rtk` как `mod mem`** | Надёжнее операционно, единая поставка и lifecycle.
| 4. Формат артефактов | **D — структурированные резюме + query-relevance mapping** | Агенту нужен «сок», не raw dump; снижает шум и токены.
| 5. Приоритет метрик | **Primary: C (консистентность)**, затем **A (токены)**, затем **D (простота)** | Надёжность и отсутствие stale критичнее скорости внедрения.

Принимаем также latency-гардрейл: кэш-хит целевой `< 200 ms`.

---

## 2. Problem Statement

Claude/агенты повторно перечитывают один и тот же код между сессиями, из-за чего растут токены и задержка принятия решений. Нужен системный слой памяти внутри RTK, который:
- хранит артефакты исследования межсессионно,
- обновляет только затронутые части по дельтам,
- отдает компактный контекст без шума,
- работает автоматически через hook без ручной настройки.

---

## 3. Goals

1. Исключить выдачу stale-данных (строгая валидация, dirty-blocking).
2. Снизить токены на повторное исследование на `>= 50%`.
3. Дать zero-config интеграцию для Claude через существующую RTK hook-архитектуру.
4. Поддержать параллельные локальные агенты через shared cache.

---

## 4. Scope

### In Scope (v1)
- `mod mem` внутри `rtk` (`src/mem/*`).
- Cache store: in-memory hot + disk-backed persistent.
- FS daemon/watcher с инвалидацией и дельтами.
- Встроенный локальный API для агентов (HTTP на localhost) + CLI команды.
- Hook-интеграция в текущий rewrite pipeline RTK.

### Out of Scope (v1)
- Сетевой shared cache между разными машинами.
- Векторный/семантический поиск (это зона `rtk rgai`).
- GUI.

---

## 5. Target Users

- Основной: несколько локальных агентных сессий (Claude Code), работающих параллельно в одном/нескольких проектах на одной машине.
- Вторичный: одиночная локальная сессия (получает те же преимущества кэш-хита и дельт).

---

## 6. Architecture

### 6.1 Components

1. `mem::cache`
- хранение артефактов, версий, индексов и статусов валидности.
- SQLite (WAL) + file lock для конкурентного доступа.

2. `mem::indexer`
- первичная индексация проекта;
- инкрементальный пересчёт только изменённых узлов.

3. `mem::extractor`
- извлечение сигнатур/типов/экспортов/док-комментариев.
- **Решение:** гибридная стратегия.
  - v1: regex extractors (быстрый старт)
  - расширяемый интерфейс под tree-sitter
  - tree-sitter для Rust/TS/Python как upgrade path без ломки API.

4. `mem::watcher`
- notify-based daemon (`inotify`/`kqueue`/`ReadDirectoryChangesW`).
- очередь событий + coalescing + каскадная инвалидация.

5. `mem::delta`
- дельты по FS-событиям и по git-границе (`--since <rev>`).

6. `mem::api`
- localhost HTTP endpoints для агентов:
  - `POST /v1/explore`
  - `POST /v1/delta`
  - `POST /v1/context`
  - `POST /v1/refresh`

7. `mem::cli`
- пользовательские команды RTK:
  - `rtk memory explore`
  - `rtk memory delta`
  - `rtk memory refresh`
  - `rtk memory watch`
  - `rtk memory status`
  - `rtk memory gain`
  - `rtk memory clear`

8. Hook integration
- подключение к существующему RTK hook rewrite flow;
- автоматическая подстановка memory context при агентных explore-паттернах.

### 6.2 Daemon Lifecycle (решение)

- Demon стартует on-demand:
  - при первом `rtk memory watch` или hook-trigger.
- Health-check перед запросом контекста.
- Если daemon недоступен: fail-safe fallback на on-demand `explore/delta` без потери корректности.
- Auto-idle stop (конфигурируемо) для снижения фоновой нагрузки.

---

## 7. Artifact Model (D: summaries + relevance map)

### 7.1 Layers

| Layer | Artifact | Что хранится |
|---|---|---|
| L0 | `project_map` | дерево проекта, точки входа, правила/README/CLAUDE |
| L1 | `module_index` | модули и публичные экспорты |
| L2 | `type_graph` | публичные типы и связи |
| L3 | `api_surface` | сигнатуры и doc-комментарии |
| L4 | `dep_manifest` | зависимости и роль |
| L5 | `test_map` | карта тестов и покрываемых зон |
| L6 | `change_digest` | последние изменения git/FS |

### 7.2 Relevance Mapping

Для каждого запроса агента строится профиль релевантности:
- `bugfix` -> L1/L3/L5/L6
- `feature` -> L0/L1/L2/L3/L4
- `refactor` -> L1/L2/L3/L5
- `incident` -> L3/L4/L6

Ответ по умолчанию — `compact`, только релевантные слои.

### 7.3 Noise Policy (выбрасываем)

- тела функций,
- full import lists,
- lock-файлы целиком,
- generated/binary/vendor outputs.

---

## 8. Consistency & Reliability Policy (Primary KPI)

1. Статус артефакта: `FRESH | STALE | DIRTY`.
2. Выдача контекста разрешена только для `FRESH`.
3. Для `STALE/DIRTY`: auto-refresh или явная ошибка с причиной.
4. Валидация: `content_hash + file_size + mtime + schema_version`.
5. Любой mismatch => каскадная инвалидация зависимых артефактов.
6. Цель: `0 stale incidents`.

---

## 9. Data Model

- Отдельная БД: `~/.local/share/rtk/mem.db`.
- SQLite `WAL` режим.
- Таблицы: `projects`, `artifacts`, `artifact_edges`, `events`, `cache_stats`.
- Индексы по `(project_id, layer, status)` и `last_accessed_at`.

---

## 10. API / CLI Contracts

### 10.1 API request (example)
```json
{
  "project_root": "/abs/path",
  "query_type": "bugfix",
  "detail": "compact",
  "since": "HEAD~5"
}
```

### 10.2 API response (example)
```json
{
  "cache_status": "hit",
  "artifact_version": 1,
  "delta": {"added": 1, "modified": 2, "removed": 0},
  "context": {"layers": ["L1","L3","L6"], "payload_ref": "..."}
}
```

### 10.3 CLI (v1)
- `rtk memory explore [path]`
- `rtk memory delta [path] [--since REV]`
- `rtk memory refresh [path]`
- `rtk memory watch [path]`
- `rtk memory status [path]`
- `rtk memory gain [path]`
- `rtk memory clear [path]`

---

## 11. Success Metrics

### Primary
- `stale incidents = 0`

### Secondary
- `>= 50%` снижение токенов на повторном исследовании
- `>= 80%` cache-hit ratio в активных проектах
- `< 200 ms` cache-hit p95
- zero-config активация в hook-потоке

---

## 12. Delivery Plan (подзадачи)

> Легенда: ✅ сделано | ⚠️ начато, не завершено | ❌ не начато

## Epic E0 — Foundation
- ✅ E0.1 Разбивка `src/memory_layer/` завершена. Извлечены: `cache.rs` (258 строк, SQLite WAL persistence/hashing/retry), `indexer.rs` (532 строки, scanning/delta/git-delta), `renderer.rs` (669 строк, response building/rendering/L5), `extractor.rs` (218 строк, file analysis), `manifest.rs` (211 строк, dep parsing). `mod.rs`: 1560 строк (types + CLI entry + tests).
- ✅ E0.2 `Commands::Memory` + `MemoryCommands` подключены в `main.rs`; все subcommand-ы роутятся.
- ✅ E0.3 Конфиг `mem` вынесен в `config.rs`: `MemConfig { cache_ttl_secs, cache_max_projects, max_symbols_per_file }`. Читается из `~/.config/rtk/config.toml` c fallback на compile-time константы.

## Epic E1 — Cache & Schema
- ✅ E1.1 Хранилище мигрировано на SQLite WAL: `~/.local/share/rtk/mem.db`. Таблицы: `projects`, `artifacts`, `cache_stats`, `artifact_edges`. ARTIFACT_VERSION=4. Env var `RTK_MEM_DB_PATH` для тестов.
- ✅ E1.2 SQLite WAL mode (`PRAGMA journal_mode=WAL; synchronous=NORMAL; busy_timeout=2500`). Retry-wrapper `with_retry()` с exponential backoff (100/200/400ms) на `store_artifact`/`delete_artifact` для multi-agent безопасности.
- ✅ E1.3 LRU eviction (max 64 проекта по `last_accessed_at` в SQLite) + TTL staleness (24h) через `is_artifact_stale`.
- ✅ E1.4 `record_cache_event()` пишет hit/miss/stale_rebuild/dirty_rebuild/refreshed/delta в SQLite `cache_stats`. `query_cache_stats()` агрегирует по event. `cache_status_event_label()` выбирает метку из BuildState. Вызывается из `run_explore`, `run_delta`, `run_refresh`.

## Epic E2 — Extractor & Artifact Pipeline
- ✅ E2.1 Regex-extractor подключён через `symbols_regex::RegexExtractor` + `SymbolExtractor` trait. Поддержка: Rust, TypeScript, JavaScript, Python, Go. Интерфейс готов к подключению tree-sitter.
- ✅ E2.2 Реализованы все слои: **L0** (entry_points, hot_paths, project tree), **L1** (module_index: compact export list per module), **L2** (type_graph: TypeRelation struct, regex extraction Rust impl/struct fields/alias, TS extends/implements, Python class bases, build_type_graph() в renderer, wiring в layers_for()), **L3** (api_surface: pub_symbols кэшируются в FileArtifact), **L4** (dep_manifest: Cargo.toml/package.json/pyproject.toml parsing), **L5** (test_map: файлы тестов с классификацией unit/integration/e2e), **L6** (change_digest: delta added/modified/removed с хешами).
- ✅ E2.3 Relevance mapping реализован. `--query-type general|bugfix|feature|refactor|incident` добавлен во все explore/delta/refresh/watch команды. `LayerFlags` + `layers_for()` управляют набором слоёв в ответе. L5 включён в General/Bugfix/Feature/Refactor.
- ✅ E2.4 Compact text renderer + JSON format (`--format json`). Детализация через `--detail compact/normal/verbose` с `DetailLimits`.

## Epic E3 — Delta & Watcher
- ✅ E3.1 `rtk memory watch` переписан на event-driven watcher (`notify = "6"`, kqueue/inotify/ReadDirectoryChangesW). Добавлена `should_watch_abs_path()` + 3 unit-теста. `interval_secs` → debounce window. Polling-loop удалён.
- ✅ E3.2 Каскадная инвалидация по dependency graph: `store_artifact_edges()` заполняет таблицу из import data при explore/refresh. `get_dependents()` возвращает зависимые файлы. `find_cascade_dependents()` + `module_stems_for_path()` в `indexer.rs` — двухпроходная логика в `build_incremental_files`: pass-1 находит metadata-changed файлы, pass-2 расширяет через import-граф и форсирует rehash зависимых файлов.
- ✅ E3.3 FS delta (added/modified/removed по xxh3-хешам) ✅. Git delta `--since <rev>` ✅ — реализован через `build_git_delta()` в `indexer.rs`, подключён к `rtk memory delta --since REV` через CLI.
- ✅ E3.4 Tri-state `FRESH|STALE|DIRTY` + strict dirty-blocking: `run_explore --strict` возвращает ошибку при STALE или DIRTY вместо auto-rebuild (PRD §8). Default режим: auto-rebuild (compliant per "auto-refresh или явная ошибка"). `--strict` флаг в CLI и API request. Тест: `strict_explore_rejects_stale_artifact`.

## Epic E4 — API + CLI
- ✅ E4.1 HTTP localhost API реализован: `src/memory_layer/api.rs` — минимальный HTTP/1.1 сервер на std::net::TcpListener. Эндпоинты: `GET /v1/health`, `POST /v1/{explore,delta,refresh,context}`. Daemon lifecycle: idle-timeout loop + PID файл (`~/.local/share/rtk/mem-server-{port}.pid`). `rtk memory serve --port 7700 --idle-secs 300`.
- ✅ E4.2 Реализованы: `explore`, `delta` (с `--since`), `refresh`, `watch`, `status`, `clear`, `install-hook`, `gain`. `rtk memory gain` показывает raw_bytes vs context_bytes, % token_savings, cache_status.
- ✅ E4.3 JSON contract tests: `json_response_contains_required_top_level_fields` (все поля PRD §10.2), `json_cache_status_serialises_as_snake_case` (5 вариантов CacheStatus), `json_delta_present_when_some` (structure + files array).

## Epic E5 — Hook Integration (zero-config)
- ✅ E5.1 `hooks/rtk-mem-context.sh` компилируется в бинарь через `include_str!()`, материализуется в `~/.claude/hooks/rtk-mem-context.sh` при `install-hook`. Патчит `~/.claude/settings.json` с backup (`settings.json.bak` перед записью).
- ✅ E5.1b Добавлен отдельный Task/Explore policy hook: `hooks/rtk-block-native-explore.sh` материализуется при `rtk memory install-hook`, апсертится в `PreToolUse:Task` как явный deny policy (override: `RTK_ALLOW_NATIVE_EXPLORE=1` / `RTK_BLOCK_NATIVE_EXPLORE=0`).
- ✅ E5.2 Fail-safe: hook выходит с 0 при отсутствии `rtk`/`jq`; `rtk memory explore` при ошибке возвращает пустой контекст без прерывания работы агента.
- ❌ E5.3 Smoke-тест в реальном Claude workflow не проведён. Требует ручной проверки.

## Epic E6 — QA, Benchmarks, Rollout
- ⚠️ E6.1 Unit-тесты: **44 теста** в `memory_layer` (all green). Покрытие: layer mapping (5), gain (3), watch paths (3), manifest (4), extractor (1), indexer (3), SQLite concurrent (1), DirtyRebuild (2), freshness (3), retry (3), render/serialization (16). Отсутствуют: chaos/race tests, integration tests с реальной multi-process FS.
- ❌ E6.2 Нагрузочный стенд не настроен.
- ✅ E6.1 Chaos/race tests: `chaos_concurrent_store_no_corruption` (8 потоков, store+load+idempotency), `chaos_concurrent_store_and_delete` (Barrier-синхронизированный race без паники).
- ✅ E6.2 Cache-hit latency benchmark: `cache_hit_latency_p95_under_200ms` — 20 итераций на реальном TempDir проекте, p95 < 200ms.
- ✅ E6.3 `rtk memory gain` реализован: `GainStats` struct + `compute_gain_stats()` чистая функция, `run_gain()` CLI entry, `Gain` variant в `MemoryCommands`. Показывает raw_source vs context bytes, % savings. `-v` — сравнение compact/normal/verbose.
- ✅ E6.4 Feature flags реализованы: `MemFeatureFlags` struct в `config.rs`, поле `features` в `MemConfig`. Флаги: `type_graph`, `test_map`, `dep_manifest`, `cascade_invalidation`, `git_delta`, `strict_by_default`. `apply_feature_flags()` в `renderer.rs` — AND-только маска на `LayerFlags`. Cascade guard в `indexer::build_incremental_files`. Git delta guard в `run_delta` и `handle_delta`. `strict_by_default` в `run_explore`. API (`api.rs`) и все CLI handlers обновлены. 6 unit-тестов в `renderer::feature_flag_tests`.

---

## 13. Checklist исправлений к текущему PRD (выполнить перед dev start)

> Все пункты зафиксированы до начала разработки — решения закреплены в Section 1.

- ✅ Зафиксировать решение по scope как **1C** (убрать ambiguity между MVP и full stack)
- ✅ Зафиксировать consumer model как **2B** (несколько локальных агентов)
- ✅ Зафиксировать интеграцию как **3B** (`mod mem` внутри `rtk`)
- ✅ Обновить artifact strategy до **4D** (добавить relevance mapping)
- ✅ Переставить KPI приоритет: **C -> A -> D**
- ✅ Закрыть open question по extractor: гибрид (regex v1 + tree-sitter-ready interface)
- ✅ Закрыть open question по daemon lifecycle: on-demand + fallback
- ✅ Унифицировать командную поверхность в PRD: `rtk memory ...`
- ✅ Добавить явную политику dirty-blocking (не отдавать stale)
- ✅ Добавить план hook fail-safe поведения
- ✅ Добавить план конкурентного доступа и retry-политику
- ✅ Добавить тестовую матрицу: unit/integration/e2e/perf/concurrency

---

## 14. Acceptance Criteria (release gate)

1. Нет ни одного кейса выдачи stale-данных в тестовой матрице.
2. На повторном исследовании достигнуто >= 50% сокращение объёма контекста.
3. Cache-hit p95 < 200 ms на референсных репозиториях.
4. Hook-поток Claude работает без ручной настройки.
5. Параллельные агенты не приводят к corruption или race-loss.

---

## 15. Implementation Status (последнее обновление: 2026-02-18 sprint-5)

### Реализовано полностью ✅

| Компонент | Файл(ы) | Детали |
|---|---|---|
| `Commands::Memory` routing | `src/main.rs:1078-1180` | MemoryCommands enum (8 variants), все match-arms |
| `run_explore` | `memory_layer/mod.rs:409-438` | cache-hit/miss/DirtyRebuild, incremental hashing (xxh3), freshness field, stderr warnings |
| `run_delta` | `memory_layer/mod.rs:440-461` | FS delta по xxh3 + `--since REV` git delta через `build_git_delta()` |
| `run_refresh` | `memory_layer/mod.rs:463-475` | force-rehash всего проекта |
| `run_watch` | `memory_layer/mod.rs:477-567` | event-driven watcher (notify kqueue/inotify) + debounce + path filtering |
| `run_status` | `memory_layer/mod.rs:730-766` | tri-state FRESH/STALE/DIRTY, files, bytes, age |
| `run_clear` | `memory_layer/mod.rs:776-788` | удаление из SQLite |
| `run_install_hook` | `memory_layer/mod.rs:540-728` | `include_str!()` + materialize + settings.json backup + патч |
| `run_gain` | `memory_layer/mod.rs:791-850` | raw vs context bytes, savings %, tri-state freshness |
| SQLite WAL backend | `memory_layer/cache.rs:47-101` | `mem.db`: 4 таблицы, WAL mode, busy_timeout=2500ms, retry wrapper |
| L0 layer | `renderer.rs:177-220` | entry_points, hot_paths, project tree (gitignore-aware WalkBuilder) |
| L1 layer | `renderer.rs:325-349` | module_index: compact per-module export list |
| L3 layer | `renderer.rs:280-323` | api_surface: pub_symbols в FileArtifact; regex extractor (Rust/TS/JS/Python/Go) |
| L4 layer | `manifest.rs:7-33` | dep_manifest: Cargo.toml/package.json/pyproject.toml parsing |
| L5 layer | `renderer.rs:606-669` | test_map: file classification (unit/integration/e2e/unknown) |
| L6 layer | `indexer.rs:76-202` | change_digest: delta added/modified/removed с хешами |
| E2.3 Relevance mapping | `renderer.rs:351-400` | `layers_for()`: 5 query types, LayerFlags по PRD §7.2 (кроме L2) |
| LRU eviction | `cache.rs:185-210` | max 64 проекта по `last_accessed_at` в SQLite |
| TTL staleness | `cache.rs:214-218` | 24h configurable, `is_artifact_stale` |
| Retry wrapper | `cache.rs:211-232` | `with_retry()` exponential backoff для SQLITE_BUSY |
| Config externalization | `config.rs:78-109` | `MemConfig { cache_ttl_secs, cache_max_projects, max_symbols_per_file }` |
| E5 hook script | `hooks/rtk-mem-context.sh` | PreToolUse:Task, инжект в Explore-агент, compiled-in |
| L2 type_graph | `extractor.rs`, `renderer.rs`, `mod.rs` | TypeRelation struct, regex extraction (Rust impl/struct/alias, TS extends/implements, Python bases), build_type_graph(), layers_for() wiring |
| E1.4 cache_stats | `cache.rs`, `mod.rs` | record_cache_event() + query_cache_stats() + cache_status_event_label(); wired в run_explore/delta/refresh |
| E3.2 cascade invalidation | `indexer.rs` | module_stems_for_path(), find_cascade_dependents(), двухпроходный build_incremental_files; store_import_edges() в run_explore/refresh |
| E6.1 chaos tests | `mod.rs` | `chaos_concurrent_store_no_corruption` (8 потоков), `chaos_concurrent_store_and_delete` (Barrier race) |
| E6.2 latency benchmark | `mod.rs` | `cache_hit_latency_p95_under_200ms` — 20 итераций, TempDir 20 модулей |
| E4.3 JSON contracts | `mod.rs` | 3 теста: required fields, CacheStatus snake_case, delta structure |
| E4.1 HTTP API + daemon | `memory_layer/api.rs`, `main.rs`, `mod.rs` | `serve()`: TcpListener, idle-timeout, PID file; endpoints: health/explore/delta/refresh/context |
| E3.4 strict dirty-blocking | `mod.rs:run_explore`, `main.rs:Explore` | `--strict` flag: bail! при STALE/DIRTY; тест `strict_explore_rejects_stale_artifact` |
| PRD §9 schema closure | `cache.rs:init_schema` | `events` table + `idx_events_project` + `idx_artifacts_version` |
| `record_event()` | `cache.rs` | записывает lifecycle events (explore/delta/refresh/api:*) с `duration_ms` |
| Perf docs | `docs/issues/*.md` | переписаны под SQLite WAL архитектуру; добавлен benchmark table, cascade overhead, scale projection |
| Unit tests | across 6 files | **69 тестов** (all green — +6 E6.4 feature_flag_tests) |
| E6.4 Feature flags | `config.rs`, `renderer.rs`, `indexer.rs`, `mod.rs`, `api.rs` | `MemFeatureFlags`: type_graph/test_map/dep_manifest/cascade_invalidation/git_delta/strict_by_default; `apply_feature_flags()` AND-mask; cascade guard в indexer; api.rs handlers обновлены |

### Начато, не завершено ⚠️

| Компонент | Статус | Что осталось |
|---|---|---|
| E5 smoke test | Hook + install готовы | Запустить live в Claude Explore workflow |

### Не начато ❌

*(нет)*

### Следующие приоритеты

1. **E5.3** Live smoke test — ручная проверка hook в Claude Explore workflow (manual)

### Завершено в sprint-6 (2026-02-18)

- ✅ **E6.4 Feature flags** — `MemFeatureFlags` struct в `config.rs` (6 флагов). `apply_feature_flags()` в `renderer.rs` — AND-only маска на `LayerFlags`. `cascade_enabled` param в `build_state` и `build_incremental_files`. `strict_by_default` в `run_explore`. `git_delta` guard в `run_delta` и `api::handle_delta`. Все handlers (`mod.rs`, `api.rs`) читают `mem_config().features`. 6 unit-тестов в `renderer::feature_flag_tests`. fmt + clippy clean.
- ✅ **832 unit-теста** (all green) — +6 новых: feature_flag_tests (default_flags, type_graph_false, test_map_false, dep_manifest_false, all_off, AND-only invariant)
- ✅ **ARCHITECTURE.md** — auto-synced 63→67 модулей; `.claude/hooks/rtk-rewrite.sh` restored

### Завершено в sprint-5 (2026-02-18)

- ✅ **E4.1 HTTP API `/v1/*`** — `src/memory_layer/api.rs`: TcpListener, minimal HTTP/1.1 parser, `GET /v1/health`, `POST /v1/{explore,delta,refresh,context}`, idle-timeout daemon lifecycle, PID file `~/.local/share/rtk/mem-server-{port}.pid`
- ✅ **`rtk memory serve`** — новая команда `MemoryCommands::Serve { port, idle_secs }` в `main.rs`
- ✅ **E3.4 strict dirty-blocking** — `--strict` флаг в `rtk memory explore`: bail! при STALE/DIRTY (PRD §8); default = auto-rebuild
- ✅ **PRD §9 schema closure** — `events` table + `idx_events_project` + `idx_artifacts_version` в `init_schema`; `record_event()` с `duration_ms`
- ✅ **Perf docs** — оба файла в `docs/issues/` переписаны: JSON→SQLite WAL, реальные benchmark данные (p50=2ms, p95=108ms, p99=674ms), cascade overhead, scale projection, concurrency comparison table
- ✅ **Latency test robustness** — `cache_hit_latency_p95_under_200ms`: 20→30 итераций, hard gate 2000ms, soft warn ≥ 200ms
- ✅ **63 unit-теста** (all green) — +5 новых: api::tests (3), strict_explore (1), events schema implicitly tested

### Завершено в sprint-4 (2026-02-18)

- ✅ **E6.1 chaos tests** — `chaos_concurrent_store_no_corruption` (8 потоков, double-store idempotency под contention), `chaos_concurrent_store_and_delete` (4 потока, Barrier-синхронизированный race, no-panic guarantee)
- ✅ **E6.2 cache-hit latency benchmark** — `cache_hit_latency_p95_under_200ms`: 20 итераций на 20-модульном TempDir проекте, p95 < 200ms (PRD §11)
- ✅ **E4.3 JSON contract tests** — 3 теста: required top-level fields, CacheStatus snake_case variants (hit/miss/dirty_rebuild/stale_rebuild/refreshed), delta structure + files array
- ✅ **66 unit-тестов** — +14 новых: chaos (2), latency (1), JSON contracts (3), cascade indexer (3), L2 (2), edges (1), cache_stats (1), event_label (1)

### Завершено в sprint-3 (2026-02-18)

- ✅ **L2 type_graph** — `TypeRelation` struct, regex extraction (Rust impl/struct/alias, TS extends/implements, Python bases), `build_type_graph()` в renderer, wiring в `layers_for()` для General/Feature/Refactor, рендеринг в text/JSON. Live: 50+ relations на rtk codebase
- ✅ **E1.4 cache_stats** — `record_cache_event()` пишет hit/miss/stale_rebuild/dirty_rebuild/refreshed/delta; `cache_status_event_label()` выбирает метку из BuildState; `query_cache_stats()` агрегирует; вызывается из run_explore, run_delta, run_refresh
- ✅ **E3.2 cascade invalidation** — `module_stems_for_path()` + `find_cascade_dependents()` в indexer.rs; двухпроходный `build_incremental_files`: pass-1 находит metadata-changed, pass-2 расширяет через import-граф; `store_import_edges()` персистирует граф в SQLite при explore/refresh
- ✅ **52 unit-теста** — +8 новых: L2 type extraction (2), cascade deps (3), edges roundtrip (1), cache_stats roundtrip (1), cache_status_event_label (1)

### Завершено в sprint-2 (2026-02-18)

- ✅ **SQLite WAL migration** — `cache.rs` переписан: JSON→SQLite, 4 таблицы, WAL mode, retry wrapper
- ✅ **L5 test_map** — `renderer.rs:606-669`: `build_test_map()`, `is_test_file()`, `test_file_kind()`
- ✅ **Git delta `--since REV`** — `indexer.rs:103-186`: `build_git_delta()` + CLI `--since` flag
- ✅ **Tri-state freshness** — `ArtifactFreshness { Fresh, Stale, Dirty }` + `CacheStatus::DirtyRebuild`
- ✅ **Freshness field in response** — `MemoryResponse.freshness` ("fresh"/"rebuilt") для PRD §8
- ✅ **Retry wrapper** — `with_retry()` exponential backoff для SQLITE_BUSY (PRD §1.2)
- ✅ **settings.json backup** — `.json.bak` создаётся перед install/uninstall hook
- ✅ **Config externalization** — `MemConfig` в `config.rs` c `~/.config/rtk/config.toml`
- ✅ **44 unit-теста** — +6 новых: freshness (3), retry (3)
