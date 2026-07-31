# Архитектурный review: переход RTK fork → HZR

**Дата:** 2026-07-31T18:27:14+03:00  
**Reviewer:** Codex IT Architect + три независимых subagent-аудита  
**Scope:** текущий dirty worktree RTK, RTK IMG, установленный ICM 0.10.57, grepai 0.35.0, Caveman v1.9.1 и целевая платформа HZR  
**Тип review:** архитектура, качество, производительность, безопасность, deploy readiness  
**Связанный PRD:** tasks/prd-hzr-unified-agent-efficiency-platform.md

## Executive Summary

Текущий fork содержит много ценных механизмов, но ещё не образует единую agent-efficiency систему. Shell interception, memory context, grepai и ICM подключены через независимые hooks/subprocesses; каждый слой имеет свой budget, lifecycle и оценку токенов. Добавление Caveman в эту цепочку без центрального execution/response seam увеличит риск потери evidence и повторного сжатия.

Главное решение review:

- продукт переименовать в HZR;
- создать единый control plane hzrd;
- RTK оставить движком исполнения и deterministic filtering;
- grepai назначить единственным semantic code index;
- RTK IMG ограничить краткоживущей workspace-моделью;
- ICM назначить long-term memory;
- Caveman использовать как адаптивный representation/output-density codec;
- HZR Core сделать единственным владельцем budget, policy, provenance, quality и ledger.

Перед началом миграции обязательна P0-стабилизация: текущий baseline имеет рассинхрон hook/CLI protocol, семь падающих pipe tests и не проходит formatting check.

## Архитектурная диаграмма

~~~mermaid
graph TD
    subgraph CURRENT["Текущее состояние"]
        H["Independent shell hooks"] --> RW["rtk rewrite"]
        H --> MP["rtk memory plan"]
        MP --> SR["rtk rgai --files --builtin"]
        H --> SR2["second rgai + pre-read"]
        RW --> CM["many command modules"]
        CM --> PF["per-command print/filter/tracking"]
        IH["Independent ICM hooks/MCP"] --> AG["Agent context"]
        SR --> AG
        SR2 --> AG
        PF --> AG
        GA["Standalone grepai watcher/index"] -. partial .-> SR
    end

    subgraph TARGET["HZR target"]
        AD["One adapter / hook protocol"] --> D["hzrd"]
        D --> P["Policy + Fidelity"]
        D --> B["One Token Budget"]
        D --> R["Retrieval Orchestrator"]
        R --> M["ICM-derived Memory"]
        R --> C["RTK IMG-derived Context"]
        R --> I["grepai-derived Index"]
        D --> E["RTK-derived Exec/Filter"]
        D --> X["Caveman-derived Codec"]
        D --> Q["Quality Gate + Raw Fallback"]
        D --> L["Actual Usage Ledger"]
    end

    CURRENT -->|"strangler migration"| TARGET
~~~

## Scope и метод

Review включал:

1. semantic и exact tracing текущего rewrite, memory plan, rgai/grepai, tracking и tee;
2. проверку установленных версий и runtime процессов ICM/grepai;
3. cargo fmt/test baseline;
4. анализ публичной документации и исходников Caveman v1.9.1;
5. сопоставление с независимыми paired A/B studies;
6. проектирование migration boundary без изменения существующего dirty worktree.

Изменения production-кода не выполнялись. Review и PRD добавлены новыми файлами.

## Текущая архитектура

### Shell execution

    Claude PreToolUse:Bash
      -> hooks/rtk-rewrite.sh
      -> rtk rewrite
      -> permissions::check_command
      -> discover::registry::rewrite_command
      -> main.rs dispatch
      -> command-specific subprocess/filter/print/tracking

Точки:

- hooks/rtk-rewrite.sh:9-31;
- src/rewrite_cmd.rs:7-46;
- src/discover/registry.rs:481+;
- src/main.rs:1782-3097;
- src/stream.rs:5-43;
- src/runner.rs:6-14;
- src/tracking.rs:1026-1048,1143-1157;
- src/tee.rs:102-166,264-290.

src/stream.rs и runner::emit_guarded являются зачатками общего seam, но используются локально. По статическому аудиту Command::new встречается в 46 Rust-файлах, .output — в 51, прямой print в stdout/stderr — в 77, tracking — в 47. Поэтому глобальную codec/quality policy сейчас негде применить единообразно.

### Context planning

    PreToolUse:Task
      -> rtk memory plan --budget 2400
      -> graph-first candidates
      -> rtk rgai --files --builtin
      -> second rgai in shell hook
      -> up to 20 pre-read files
      -> prompt capped by characters, not final model tokens

Точки:

- hooks/rtk-mem-context.sh:62-73,78-190,201-232;
- src/memory_layer/planner_graph.rs:94-443;
- src/memory_layer/semantic_stage.rs:21-72;
- src/memory_layer/budget.rs:50-165.

Планировщик получает 2400 tokens, но shell hook после него допускает до 80 000 символов pre-read и 120 000 символов итогового context. Этот budget не является hard contract.

### Search/index

Текущий rtk rgai:

- делегирует unscoped запрос внешнему grepai;
- при --files пропускает grepai и использует builtin lexical/rg scoring;
- нормализует JSON после отдельного subprocess;
- имеет 30-секундный timeout;
- смешивает graph и semantic raw scores фиксированными весами.

Точки:

- src/rgai_cmd.rs:78-126,313+,419+;
- src/grepai.rs:317-370;
- src/memory_layer/semantic_stage.rs:51-72;
- src/memory_layer/planner_graph.rs:358-372.

Runtime audit обнаружил:

- grepai status указал PID watcher, фактически обслуживающий другой workspace;
- существуют два индекса: .grepai и src/.grepai;
- candidate-scoped primary planner не использует настоящий grepai semantic search.

### ICM

ICM 0.10.57 установлен отдельно и подключён через hooks, MCP и optional HTTP service. Он уже предоставляет facts, episodic memory, transcripts, memoir graph, recall/store/consolidate. Одновременно наблюдаются несколько icm serve процессов. Текущая интеграция не имеет общего singleton lifecycle и общего token budget с RTK.

### Caveman

Caveman v1.9.1:

- основной эффект получает через prompt instruction, а не response interception;
- full skill занимает около 1,2 тыс. токенов, плюс activation/reminders;
- caveman-shrink сокращает только каталожные descriptions в MCP list responses;
- caveman-compress делает дополнительный LLM-вызов и создаёт lossy derivative;
- post-hoc compression не уменьшает bill текущего ответа.

Готовый npm proxy нельзя ставить в HZR hot path: опубликованная версия отстаёт от репозитория, а regex-преобразования имеют известный случай повреждения enum semantics.

## Requirements Compliance текущего решения

| Requirement | Status | Notes |
|---|---|---|
| Deterministic tool output filters | Partial | много полезных filters, но нет единого ResponsePipeline |
| Permission-aware command rewrite | Fail | hook отбрасывает exit 3 rewrite |
| Green regression suite | Fail | 7 pipe failures; fmt check fail |
| One context budget | Fail | shell post-processing превышает planner budget |
| Real semantic retrieval in primary flow | Fail | --files --builtin обходит grepai |
| Canonical workspace index | Fail | duplicate index и wrong watcher root |
| Long-term memory | Partial | ICM есть, но внешний параллельный lifecycle |
| Quality-preserving compression | Partial | size guard есть, semantic/invariant guard отсутствует |
| Actual token/cost accounting | Fail | chars/4 и estimated baseline |
| Raw recovery | Partial | tee есть, privacy/retention недостаточны |
| Unified observability | Fail | нет trace через все компоненты |
| Deploy as one product | Fail | отдельные binaries/hooks/configs/processes |

## Quality Scores

| Критерий | Оценка | Обоснование |
|---|---:|---|
| Code Quality | 61/100 | Сильные command-specific filters и большое количество тестов. Снижение за 3348-строчный main.rs, 87 приватных модулей, отсутствие общего execution seam, fmt failure и 7 regression failures. |
| Extensibility/Modularity | 52/100 | Memory layer разложен на модули, но engines связаны subprocess/shell parsing, а output paths распределены по десяткам файлов. Добавление общей policy требует cross-cutting edits. |
| Security | 58/100 | Permission layer и localhost intent полезны. Снижение за рассинхрон rewrite verdict, raw tee без обязательной redaction/encryption, ручной HTTP prototype и false certainty retrieval prompt. |
| Optimization/Performance | 49/100 | Локальные filters эффективны на отдельных outputs. Снижение за duplicate search/pre-read, несколько services/watchers, приблизительный accounting и независимый RTK A/B с отрицательным E2E результатом. |
| Architecture & Visualization | 55/100 | IMG имеет продуманную graph-first основу и fail-open. Общая система не имеет единого owner budget/lifecycle/provenance, границы RTK/IMG/grepai/ICM пересекаются. |
| Deploy Cleanliness | 43/100 | Несколько hooks, configs, indexes и serve процессов; stale docs/version; нет единого installer/manifest/SBOM. |
| **Итого** | **53/100** | Набор сильных механизмов, пока ещё не единая production platform. |

## Сильные стороны, которые нужно сохранить

1. Большой набор специализированных RTK filters и command discovery rules.
2. Fail-open pattern во многих memory/semantic paths.
3. Graph-first candidate generation и intent-aware features.
4. SQLite WAL и incremental workspace cache.
5. grepai local-first embeddings, semantic search и code graph.
6. ICM cross-session memory, transcripts и consolidation.
7. Caveman safety escape для неоднозначных/high-risk ответов.
8. Raw tee/recovery как идея, после исправления privacy.
9. Существующая совместимость команд, которую можно сохранить shim-ом.

## Critical Issues — Must Fix

### [P0] Rewrite verdict теряется в hook

hooks/rtk-rewrite.sh:16 использует command substitution с || exit 0. src/rewrite_cmd.rs возвращает 3 для корректного rewrite без auto-allow. Hook выбрасывает output и не передаёт permission decision.

**Рекомендация:** versioned typed RewriteDecision; shell/process exit только для transport success/failure. Golden tests для allow_raw, allow_rewrite, ask, deny.

### [P0] Pipe rewrite regression suite красная

cargo test --all-targets: 1660 passed, 7 failed, 1 ignored. Failures связаны с cargo test | grep, git log | grep, find | xargs wc и комбинациями &&/||.

**Рекомендация:** блокировать архитектурную миграцию до исправления. Добавить property/golden corpus для quoting, pipes, xargs, redirects, heredocs и compound shell expressions.

### [P0] Нет единого ExecutionEnvelope/ResponsePipeline

Разные команды самостоятельно запускают subprocess, фильтруют, печатают, track-ят и tee-ят output.

**Рекомендация:** сначала выделить hzr-protocol, hzr-exec и hzr-filter; постепенно переносить command modules через strangler adapter. Caveman/quality/ledger подключать только к seam.

### [P0] Context budget нарушается после planner

2400-token planner дополняется shell-level контекстом до 120 000 символов. src/memory_layer/budget.rs также может выбрать over-budget minimum-one и записать capped usage вместо фактического.

**Рекомендация:** один tokenizer-aware allocator на фактически сериализованный prompt. Любое превышение возвращается как typed budget_exceeded.

### [P0] Hook создаёт ложную уверенность retrieval

hooks/rtk-mem-context.sh:204-212 сообщает модели, что все релевантные файлы включены и отсутствующие нерелевантны.

**Рекомендация:** передавать coverage/confidence/freshness, разрешать gap-fill и exact verification; никогда не заявлять полноту без измеренного recall/proof.

### [P1] grepai root/lifecycle не изолирован

Watcher PID связан с другим проектом, а nested index дублирует root index.

**Рекомендация:** canonical repo root + git common-dir + worktree ID; root-bound readiness; один indexd owner; doctor quarantine для duplicate index.

### [P1] Primary semantic stage обходит grepai

semantic_stage запускает текущий executable с --files --builtin. Это lexical scorer, а не candidate-filtered vector search.

**Рекомендация:** добавить AllowedPaths/BatchSearch в hzr-indexd; использовать typed RPC; exact fallback оставить явно обозначенным.

### [P1] Savings accounting недостоверен для product KPI

tracking.rs использует ceil(chars/4), memory budget — line estimates, локальные «baselines» не являются paired counterfactual.

**Рекомендация:** UsageLedger с provider actual categories, versioned price table и outcome. Estimated fields всегда маркировать.

### [P1] Raw tee privacy

Raw output сохраняется без обязательной DLP/redaction/encryption и bounded retention.

**Рекомендация:** retention 0 по умолчанию, secret scan before persist, content-addressed encrypted artifacts, explicit reveal и TTL.

## Recommendations — Should Fix

### 1. HZR как единый product control plane

Один hzr CLI, hzrd, config, hook dispatcher, doctor, trace и installer. Multi-process implementation скрыта за lifecycle supervision.

### 2. Развести memory и code index

- grepai: chunks, embeddings, symbols, call graph, watcher;
- IMG: active workspace graph/delta/test/task features;
- ICM: cross-session facts/episodes/preferences;
- HZR Core: fusion и budget.

### 3. Заменить raw score mixing

Текущие 0.65/0.35 scores нельзя считать калиброванными. Использовать weighted RRF, затем MMR/diversity и offline calibration.

### 4. Caveman сделать codec, не глобальным режимом

Сохранять raw intent; включать короткий output contract только при положительном expected value; code/JSON/commands/errors exact. Full skill не добавлять каждый turn.

### 5. Ответ сокращать до генерации

Для gateway использовать output budget, max tokens, provider verbosity/reasoning controls и response schema. Post-response compact view считать только будущей экономией.

### 6. Не использовать caveman-shrink npm в runtime

Реализовать schema-aware HZR MCP catalog optimizer:

- не менять JSON Schema, IDs, enum, auth/security text;
- поддерживать stdio и Streamable HTTP;
- кэшировать по schema hash;
- проверять golden fixtures.

### 7. Единственная причинная трасса

Связать user intent, retrieval evidence, tool calls, edits, tests, provider usage и outcome одним trace ID. Только это позволяет найти слой, вызвавший retry или потерю качества.

### 8. Safe rollout

off → shadow → safe → adaptive; per-model/task canaries; kill switch; automatic downgrade по retry/reread/test regression.

## Target component contracts

| Component | Input | Output | Failure fallback |
|---|---|---|---|
| hzr-policy | IntentEnvelope | risk/fidelity/profile | exact/off |
| hzr-indexd | SearchRequest + generation | ranked refs | exact rg |
| hzr-context | workspace/task state | structural candidates | minimal paths |
| hzr-memory | scoped intent | memories + provenance | empty memory |
| hzr-retrieval | ranked sources | fused candidates | exact-only |
| hzr-budget | candidates + tokenizer | final pack | budget_exceeded |
| hzr-exec | ToolEnvelope | raw ToolResult | direct raw command |
| hzr-filter | raw ToolResult | lossless filtered | raw |
| hzr-compress | allowed natural language | compact derivative | lossless/raw |
| hzr-quality | raw + transformed + invariants | accept/fallback | raw |
| hzr-ledger | trace events | actual/estimated record | local queue |

## Deploy cleanliness target

HZR 1.0 distribution должна содержать:

- hzr, hzrd, hzr-indexd и временный rtk shim;
- signed engine manifest с upstream commit/hash/license;
- schema-versioned config;
- launchd/systemd/Windows service definitions;
- atomic hook install/remove;
- hzr doctor --deep;
- migration backup/verify/rollback;
- SBOM, LICENSES и THIRD_PARTY_NOTICES;
- telemetry off и raw retention 0 по умолчанию.

## Verification plan

1. Unit/property tests каждого filter и codec.
2. Protocol conformance: verdicts, streaming, cancellation, errors.
3. Golden exact corpus: command/code/JSON/error/enum/schema.
4. Retrieval gold set: recall@K, wide-vs-filtered search, stale generation.
5. Fault injection: daemon/indexd/ICM/Ollama/provider crash.
6. Security: prompt injection, path traversal, secret persistence, socket permissions.
7. Migration: duplicate index, stale PID, rollback.
8. Paired benchmark ≥200 tasks с actual billing.
9. Ablation RTK/ICM/grepai/Caveman, чтобы не приписывать эффект неверному engine.
10. Canary telemetry по retries, rereads, tests и tails cost.

## Public evidence interpretation

- Независимый Caveman study: output −8,5%, статистически обнаружимого quality loss нет. Это реалистичная отправная точка для agentic coding, не обещание 65%: [JetBrains](https://blog.jetbrains.com/ai/2026/07/speak-to-ai-agents-like-cavemen-tosave-tokens/).
- Независимый RTK study: median cost +7,6% в протестированной конфигурации. Значит, internal counters не являются product KPI: [JetBrains](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/).
- LemonCrow получил −44,1% billed output, но примерно нулевое изменение общей стоимости из-за initial context/cache economics. Это подтверждает economic gate: [LemonCrow benchmarks](https://github.com/lemoncrow-lab/lemoncrow/blob/main/BENCHMARKS.md#telegraphic-qa-benchmark).
- Max Taylor обнаружил, что простое «Be brief» почти сравнялось с full Caveman. Ценность HZR должна быть в policy/measurement/quality, не в длинном prompt: [benchmark](https://www.maxtaylor.me/articles/i-benchmarked-caveman-against-two-words).
- Repository intelligence показывает больший потенциальный рычаг через fewer turns/context, хотя reported «до» нельзя переносить как SLA: [JetBrains Context](https://blog.jetbrains.com/ai/2026/07/introducing-jetbrains-context-repository-intelligence-for-coding-agents/).

## Итоговая рекомендация

Архитектура HZR одобрена как целевое направление при условии P0 stabilization и strangler migration. Прямое переименование binary без выделения protocol/control plane не решит проблемы. Прямое добавление Caveman в RTK command filters отклонено.

Первый инженерный milestone должен создать HZR identity, typed ExecutionEnvelope и ResponsePipeline, сохранив текущее поведение. Только после этого следует подключать singleton daemon, unified retrieval и Caveman-derived adaptive codec.
