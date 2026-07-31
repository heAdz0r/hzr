# Performance & Scalability Report: HZR Unified Agent Efficiency Platform

**Дата:** 2026-07-31T18:27:14+03:00  
**Текущий масштаб:** один developer workstation, 1–3 agents, отдельные RTK/ICM/grepai процессы  
**Целевой масштаб:** local 3, team 100, platform 1000 concurrent agents  
**Статус чисел:** текущие наблюдения и архитектурные projections; будущие SLO требуют benchmark

## Executive Summary

Текущий главный performance-риск — не отдельная медленная функция, а повторная работа:

- несколько hooks проходят один и тот же request;
- memory planner, rgai и pre-read формируют контекст раздельно;
- semantic stage запускает новый rtk subprocess;
- grepai indexes/watchers могут дублироваться;
- ICM service processes дублируются;
- token savings считаются локально и не учитывают дополнительные turns/retries/cache.

HZR устраняет это через singleton hzrd, один retrieval plan, один budget, warm engines, request-local dedupe и actual provider ledger. Основной экономический рычаг ожидается от меньшего числа turns и input context; Caveman-derived output policy является дополнительным high-single-digit рычагом для agentic coding, а не центром архитектуры.

## Current scalability flow

~~~mermaid
graph TD
    T["Task / Bash hook"] --> SH["Shell hook parsing"]
    SH --> MP["rtk memory plan"]
    MP --> ART["Build/load workspace artifact"]
    MP --> SEM["spawn rtk rgai --files --builtin"]
    SH --> SEM2["second rgai"]
    SH --> RD["up to 20 rtk read calls"]
    SEM --> DB["SQLite/artifact/cache"]
    SEM2 --> GI["grepai process / index"]
    RD --> CT["120k-char context cap"]
    IC["Multiple ICM hooks/serve"] --> CT
    CT --> LLM["LLM"]
    LLM --> TC["chars/4 local accounting"]

    style SEM fill:#ffd6d6
    style SEM2 fill:#ffd6d6
    style RD fill:#ffd6d6
    style TC fill:#fff0bf
~~~

## Target scalability flow

~~~mermaid
graph TD
    A["Adapters / Gateway"] --> D["hzrd"]
    D --> DD["Request-local dedupe"]
    DD --> R["Parallel Retrieval Orchestrator"]
    R --> M["Warm Memory"]
    R --> C["Incremental Context"]
    R --> I["One indexd per workspace"]
    R --> E["Exact search"]
    M --> F["RRF + MMR"]
    C --> F
    I --> F
    E --> F
    F --> B["Tokenizer-aware hard budget"]
    B --> P["Cache-aware prompt compiler"]
    P --> L["LLM / Tool"]
    L --> Q["Quality + raw fallback"]
    Q --> U["Actual usage/outcome ledger"]
~~~

## Baseline observations

| Observation | Evidence | Performance impact |
|---|---|---|
| Context planner budget 2400 tokens; hook cap 120k chars | hooks/rtk-mem-context.sh:62-73,177+ | input budget unpredictable; cache/context bloat |
| semantic_stage recursively starts rtk | src/memory_layer/semantic_stage.rs:51-72 | process startup and repeated scan |
| --files forces builtin lexical path | src/rgai_cmd.rs:78-126 | expected semantic quality absent in hot path |
| external grepai call timeout 30s | src/grepai.rs:317-370 | long tail when engine hangs |
| duplicate .grepai indexes | runtime audit | duplicate RAM/disk/embedding work |
| watcher PID serves wrong project | runtime audit | stale/wrong results and wasted process |
| multiple icm serve processes | runtime audit | duplicate model/database warm state |
| budget estimates use lines/chars | budget.rs, tracking.rs | false budget/ROI decisions |
| call graph rebuilt/scanned in planner paths | memory layer audit | cold latency grows with files × symbols |
| manual sync HTTP/thread model | src/memory_layer/api.rs | limited pooling/backpressure/auth |

## Token economics

### Primary cost equation

Для одного accepted task:

    total_cost =
      sum(provider input)
      + sum(provider output)
      + sum(reasoning)
      + sum(cache writes)
      + sum(cache reads)
      + local/cloud engine compute
      + retries

Оптимизируем:

    cost_per_accepted_task = total_cost / accepted_tasks

Локальная разница raw_bytes − filtered_bytes является диагностикой слоя, а не итоговым KPI.

### Why component percentages cannot be added

RTK, retrieval и Caveman затрагивают пересекающиеся токены:

- RTK уменьшает tool output, который затем попадает в context;
- retrieval исключает часть tool calls полностью;
- Caveman уменьшает prose, но добавляет instruction overhead;
- более короткий ответ может сломать prompt cache или вызвать follow-up;
- post-hoc compact уменьшает следующий input, но не текущий output bill.

Итог определяется только paired end-to-end run.

### External evidence

| Evidence | Result | Architectural implication |
|---|---|---|
| JetBrains RTK, 80 paired tasks | median cost +7,6%, turns/cache reads выше | shell filtering без turn/context control может быть net-negative |
| JetBrains Caveman, 82 paired tasks | output −8,5%, quality delta not significant | realistic agentic gain high-single-digit |
| LemonCrow, 300 runs per arm | output −44,1%, cost около +0,06% | prompt/cache overhead способен съесть output savings |
| Max Taylor, 24 prompts | full Caveman около −36,5% output; «Be brief» почти тот же эффект | длинный skill не обязателен |
| JetBrains Context | до −48% cost и −68% turns | repository intelligence — потенциально главный рычаг |
| grepai Excalidraw, 5 questions | −27,5% billed cost, −55% calls | directional only; нужен собственный paired suite |

Источники:

- [RTK study](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/)
- [Caveman study](https://blog.jetbrains.com/ai/2026/07/speak-to-ai-agents-like-cavemen-tosave-tokens/)
- [LemonCrow](https://github.com/lemoncrow-lab/lemoncrow/blob/main/BENCHMARKS.md#telegraphic-qa-benchmark)
- [Max Taylor](https://www.maxtaylor.me/articles/i-benchmarked-caveman-against-two-words)
- [JetBrains Context](https://blog.jetbrains.com/ai/2026/07/introducing-jetbrains-context-repository-intelligence-for-coding-agents/)
- [grepai benchmark](https://yoanbernabeu.github.io/grepai/blog/benchmark-grepai-vs-grep-claude-code/)

## Hot path budget

### Target local warm request

| Stage | p50 target | p95 target | Notes |
|---|---:|---:|---|
| Adapter decode + workspace identity | 1 ms | 5 ms | cached realpath/git identity |
| Policy/risk/fidelity | 1 ms | 5 ms | deterministic first stage |
| Memory recall | 15 ms | 75 ms | warm embeddings/index |
| Context graph candidates | 20 ms | 120 ms | incremental cache |
| Semantic search | 60 ms | 300 ms | parallel with memory/context |
| Exact search/verify | 15 ms | 80 ms | rg/symbol index |
| RRF/MMR/budget packing | 5 ms | 25 ms | top hundreds, not all chunks |
| Prompt compile/tokenize | 5 ms | 30 ms | protected spans + cached prefix |
| **Parallel orchestration critical path** | **80 ms** | **500 ms** | excludes provider/model |

Memory, context, semantic и exact sources должны выполняться параллельно. Последовательное суммирование их latency запрещено без dependency.

### Cold request

| Stage | p95 target | Policy |
|---|---:|---|
| Daemon autostart | 500 ms | direct fallback после deadline |
| ICM model warm | 800 ms | request continues without memory |
| Small index initial load | 2 s | exact search while rebuilding |
| Large index rebuild | background | readiness=rebuilding; no silent wait |
| Context artifact cold build | 3 s | partial exact context |

Cold initialization не должна происходить внутри непрозрачного hook timeout. Adapter получает explicit degraded/rebuilding state.

## Execution and filter performance

### Current problem

Subprocess execution, filtering, printing, tee и tracking распределены по command modules. Это:

- усложняет streaming;
- повторяет allocations/formatting;
- делает latency attribution неполным;
- мешает общему backpressure;
- не позволяет сделать один zero-copy raw/filtered path.

### Target

ExecutionEnvelope:

1. streaming raw chunks входят один раз;
2. stdout и stderr сохраняются раздельно;
3. deterministic filter работает streaming, где возможно;
4. raw content-addressed spill включается только по policy;
5. semantic codec получает уже сокращённый small payload;
6. quality validator сравнивает required evidence;
7. ledger получает bytes/tokens/duration/fallback.

Targets:

- filter CPU overhead ≤5% command duration или 25 мс p95 для commands до 1 с;
- first filtered chunk ≤20 мс после raw chunk;
- memory amplification ≤1,5× streamed payload;
- max in-memory payload configurable, default 8 MiB;
- larger output spills with bounded disk quota;
- backpressure не блокирует child process indefinitely.

## Retrieval performance

### Query routing

Exact identifier/path queries не вызывают embedding model. Semantic intent не запускает повторный global grep после top-K без verification need. Impact/refactor использует graph expansion только на bounded candidate set.

### Fusion complexity

Для S sources и K top candidates:

- RRF: O(S × K);
- dedupe/content hash: O(S × K);
- MMR naive: O(K²), поэтому rerank pool ограничен, target K ≤200;
- final packing: O(K log K);
- exact reread: только selected N, target N ≤20.

### grepai/indexd requirements

- AllowedPaths pushdown, иначе post-filter top-K теряет recall;
- BatchSearch для multi-intent без process startup;
- content_ref/selective fetch, чтобы не сериализовать все snippets;
- workspace_generation в cache key;
- reconcile после dropped watcher events;
- single GOB owner.

### Cache targets

| Cache | Hit target | Max stale |
|---|---:|---:|
| request dedupe | 100% duplicate ops | request lifetime |
| retrieval query | ≥70% repeated task turns | generation-bound |
| embeddings | ≥95% unchanged files | content-hash exact |
| context artifact | ≥90% warm workspace | file/git generation |
| memory recall | ≥60% repeated task clusters | memory generation |
| stable provider prefix | ≥90% eligible turns | exact bytes/provider |

## Storage/database analysis

### Local mode

Рекомендуются отдельные stores под единым catalog:

- state.sqlite — engine/workspace registry;
- context.sqlite — workspace metadata/edges;
- icm.sqlite — memory;
- usage.sqlite — token/cost/outcome;
- grepai GOB/vector files — embeddings/symbols;
- optional content-addressed raw artifacts.

SQLite WAL подходит для 1–10 local agents, если:

- connection pool/request-scoped connection;
- busy timeout и bounded retry;
- one writer queue для high-frequency ledger events;
- batch event writes;
- extraction/consolidation async;
- migrations transactional.

Не рекомендуется класть vector blobs, raw artifacts и все logical domains в одну SQLite database.

### Team mode

При 100 agents:

- Postgres connection pool;
- partitioned events/usage;
- pgvector или Qdrant;
- Redis optional только для ephemeral coordination/cache;
- object storage для encrypted raw;
- queue/outbox для consolidation/index jobs.

### Platform mode

При 1000 agents:

- metadata/ledger partitioning по tenant/time;
- vector shards по workspace/tenant;
- event bus;
- stateless gateways;
- per-workspace lease для index writer;
- object lifecycle policies;
- quotas/admission control.

## Scalability projections

Это capacity-model, не измеренные результаты.

### 3 concurrent agents

| Metric | Projection | Required architecture |
|---|---:|---|
| Foreground HZR RPC | 10–30 req/s burst | one hzrd |
| Semantic searches | 1–5 req/s | one indexd/workspace |
| Memory recalls | 2–10 req/s | warm ICM |
| Event writes | 20–100/s | batched SQLite WAL |
| RAM | 0,5–1,5 GiB/workspace | bounded caches/GOB |
| Warm context p95 | ≤500 ms | parallel retrieval |
| Availability | best effort local | direct/raw fallback |

Главные риски: multiple worktrees, model RAM, SQLite write bursts. Решение: one owner, worktree identity, async event queue.

### 100 concurrent agents

| Metric | Projection | Required architecture |
|---|---:|---|
| Foreground HZR RPC | 300–1000 req/s burst | 3–10 gateways |
| Semantic searches | 50–200 req/s | sharded/shared vector backend |
| Memory recalls | 100–500 req/s | pooled memory service |
| Event writes | 1k–5k/s | queue + Postgres batches |
| Warm context p95 | ≤750 ms | workspace-affinity + cache |
| Availability | 99,9% target | replicated stateless tier |

GOB и laptop-local SQLite больше не подходят как shared backend. Exact reread требует workspace workers с конкретным checkout/generation.

### 1000 concurrent agents

| Metric | Projection | Required architecture |
|---|---:|---|
| Foreground HZR RPC | 3k–10k req/s burst | autoscaled gateway/control plane |
| Semantic searches | 500–2k req/s | vector shards/replicas |
| Memory recalls | 1k–5k req/s | partitioned memory service |
| Event writes | 10k–50k/s | event bus + batch consumers |
| Warm context p95 | ≤1 s | tenant/workspace sharding |
| Availability | 99,95% target | multi-AZ control/data planes |

При таком масштабе codec rollout должен управляться error budget: новая policy включается canary по model/task/tenant и автоматически откатывается по quality/cost tails.

## Concurrency and backpressure

hzrd local:

- bounded foreground queue;
- separate low-priority background queue;
- semaphore per engine;
- one write lease per workspace index;
- cancellation propagates в indexd/subprocess/provider;
- deadline-aware degradation: memory → semantic → exact/raw;
- no unbounded thread-per-connection.

Priority:

1. permission/exec decisions;
2. exact reread/tool output;
3. active context plan;
4. semantic search;
5. memory write;
6. consolidation/rebuild/telemetry.

## Gateway performance

HZR Gateway может уменьшить LLM bill только через pre-generation controls:

- stable system prefix;
- dynamic tool subset;
- compact history delta;
- max_output_tokens;
- provider verbosity/reasoning settings;
- response schema;
- avoided turns.

Streaming requirements:

- preserve event ordering and provider error semantics;
- no full-buffer response requirement;
- first-byte overhead ≤30 ms warm;
- usage trailer captured;
- client cancellation reaches provider;
- raw bypass path;
- no retry after partial side-effecting stream without idempotency.

Post-response compression выполняется async и влияет только на future replay.

## Caveman-derived codec performance

Полный Caveman skill около 1,2 тыс. tokens, а maintainer описывает overhead 1–1,5 тыс. на turn. HZR target:

- stable base contract 100–200 tokens;
- per-turn delta ≤50 tokens;
- no codec для short expected output;
- cached prefix preserved byte-for-byte;
- deterministic protected span extraction <5 ms p95 на 100 KB;
- semantic compact only when predicted net benefit positive;
- no separate external LLM compression call in normal hot path.

Expected realistic contribution:

- agentic coding E2E: 5–10% на подходящих turns;
- prose/Q&A visible output: 30–50% возможно;
- total monetary savings may be near zero if cache overhead/retries dominate.

## Reliability and failure modes

| Failure | Detection | Degradation |
|---|---|---|
| hzrd unavailable | socket connect deadline | direct adapter/raw |
| ICM unavailable | health/circuit breaker | no-memory |
| grepai unavailable | readiness/deadline | exact/lexical |
| wrong workspace | root/worktree handshake mismatch | reject index result |
| stale generation | file hash/generation | exact reread |
| embedder dimension changed | fingerprint mismatch | rebuild, exact meanwhile |
| watcher overflow | dropped-event counter | reconcile scan |
| GOB concurrent owner | lease violation | reject second writer |
| codec validation fail | invariant result | lossless/raw |
| provider usage missing | response capability | actual fields null, estimates labeled |
| gateway partial stream | stream state | no unsafe retry |
| SQLite busy | retry/queue metrics | batch/defer background write |

## Performance observability

Каждый trace должен включать:

- queue wait и execution latency per stage;
- cache hit/miss/invalidation reason;
- subprocess count;
- raw/filtered/serialized bytes;
- actual model token categories;
- tokenizer estimates только с label;
- retrieval ranks and selected coverage;
- index/memory generations;
- codec profile/overhead/predicted benefit;
- validation/fallback reason;
- turns, retries, rereads;
- tests/outcome;
- end-to-end cost.

Required dashboards:

1. cost per accepted task;
2. latency critical path;
3. turns/retry/reread;
4. retrieval recall/coverage;
5. codec net value;
6. cache stability/hit;
7. workspace/index health;
8. quality regressions and circuit breakers.

## Benchmark plan

### Micro

- tokenizer throughput;
- protected span extraction;
- deterministic filters;
- RRF/MMR/packer;
- RPC serialization;
- SQLite batched events;
- indexd AllowedPaths and BatchSearch;
- streaming first byte.

### Component

- RTK raw vs filter invariants;
- grepai exact/vector/hybrid recall/latency;
- ICM recall warm/cold;
- context cold/warm build;
- codec token delta and semantic defects;
- gateway protocol parity.

### End-to-end

- ≥200 paired tasks;
- ≥10 repos;
- 5 task classes;
- 3 stochastic repeats;
- actual provider usage;
- baseline/safe/adaptive/ablations;
- median, p75, p90, p95 and confidence intervals;
- failures/retries included.

### Release performance gates

1. median actual billed cost per accepted task ≤−30%;
2. p90 task cost ≤+5% baseline;
3. turns median ≤−20%;
4. retry rate delta ≤+2 п.п.;
5. warm orchestration p95 ≤500 ms, pure overhead ≤250 ms;
6. gateway first-byte overhead ≤30 ms;
7. retrieval recall@20 ≥95%;
8. exact invariant defects = 0;
9. memory/index crash does not block raw execution;
10. provider usage coverage ≥95% for gateway runs.

## Risk matrix

| Risk | Probability | Impact | Priority | Mitigation |
|---|---|---|---|---|
| Duplicate work persists behind new facade | medium | high | P0 | request dedupe + one owner contracts |
| Context plan still serial | medium | high | P1 | parallel fan-out and critical-path tracing |
| Codec breaks prompt cache | high | high | P0 | byte-stable prefix and cache metrics |
| Short turns become more expensive | high | medium | P0 | economic gate/off |
| Retrieval cache returns stale code | medium | critical | P0 | generation + exact reread |
| SQLite contention at 10 agents | medium | medium | P1 | pool, queue, batch writes |
| GOB OOM/concurrency | medium | high | P1 | one owner; remote backend for team |
| Gateway stream buffering | medium | high | P0 | streaming conformance tests |
| Quality loss causes extra turns | medium | critical | P0 | validators, canary, raw fallback |
| Estimated savings shown as actual | current | high | P0 | schema-level actual/estimated separation |

## Action Items

### Immediate P0

1. Исправить rewrite exit protocol и семь pipe tests.
2. Получить reproducible fmt/clippy/test baseline.
3. Инструментировать subprocess count, context serialized tokens и provider actual usage.
4. Убрать false completeness claim из Task hook.
5. Запретить duplicate workspace index owner.

### Short-term P1

6. Ввести ExecutionEnvelope/ResponsePipeline.
7. Поднять hzrd с bounded queues и warm ICM/indexd.
8. Убрать second rgai/pre-read.
9. Добавить AllowedPaths/BatchSearch/content_ref.
10. Реализовать one tokenizer-aware budget.
11. Перевести ledger writes в batch/async.

### Medium-term P2

12. RRF/MMR и offline calibration.
13. Cache-stable prompt compiler.
14. Caveman-derived shadow/safe codec.
15. Gateway streaming adapters.
16. Paired benchmark and ablation harness.

### Scale P3

17. Postgres/vector backend для team mode.
18. Workspace-affinity routing и exact checkout workers.
19. Event bus/object store для 1000-agent mode.
20. Automated error-budget codec rollout.

## Conclusion

HZR может дать существенную end-to-end экономию только после устранения повторной работы и ложного accounting. На local scale главный выигрыш принесут warm singleton engines, один retrieval pass, реальный hard budget и fewer turns. Caveman-derived слой добавляет полезную краткость, но должен оставаться adaptive, cache-aware и quality-gated.

Успех HZR измеряется не самым высоким процентом в одном filter. Успех — это более дешёвая успешно решённая задача, стабильные tails и возможность мгновенно вернуться к raw без потери evidence.
