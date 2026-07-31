# PRD: HZR 0.1.0 — единая платформа эффективности LLM-агентов

**Продукт:** HZR, от ника heAdz0r
**Релиз:** 0.1.0
**Дата решения:** 2026-07-31
**Статус:** HZR 0.1.0; functional release gates green, economic KPI awaits paired provider benchmark
**Репозиторий:** новый самостоятельный продукт и Git history; внутрь HZR целиком импортируется фактический worktree `heAdz0r/rtk` как неизменяемое fork-core
**Главный критерий:** минимальная полная стоимость успешно решённой задачи при измеримом сохранении качества

## 1. Решение

HZR строится **вокруг доказавшего эффективность текущего fork `heAdz0r/rtk` и 100% его кода**. Fork не переписывается и не заменяется RTK-compatible реализацией. Его фактический dirty worktree становится внутренним execution/context core, а HZR добавляет снаружи единый control plane, централизованные ICM и grepai, Caveman-контракт и managed caveman-code runtime.

### 1.1 Непереговорный fork-core contract

1. Источник истины — текущее фактическое состояние `/Users/andrew/Programming/rtk`, включая tracked modifications и относящиеся к проекту untracked files, а не только commit `HEAD` или stock upstream RTK.
2. В HZR переносится весь source/product surface fork: command filters, rewrite/hooks, `rgai`, grepai adapter, memory layer, read/write pipeline, guards, trust/permissions, discovery, gain/economics, telemetry/tracking, benchmarks, fixtures, tests, scripts и документация.
3. HZR не реализует упрощённую замену этого поведения. Существующие `hzr-exec`, `hzr-context` и другие crates могут быть только adapters/orchestrators вокруг fork-core.
4. Snapshot исключает лишь заведомо генерируемые или локально-секретные данные: `.git`, `target`, `.grepai`, `__pycache__`, локальные DB/data и ignored machine-local settings. Исключения перечисляются явно и не могут скрывать source code.
5. Snapshot сопровождается машинно проверяемым manifest: source branch, source `HEAD`, dirty patch hash, список существующих файлов и SHA-256 каждого файла, а также список tracked deletions.
6. Fork-core собирается и проходит собственные тесты из HZR repository. CI отклоняет потерю, замену или незадокументированное изменение любого snapshot-файла.
7. Интеграция делается преимущественно через environment, process, protocol и storage adapters. Если изменение fork-core неизбежно, исходный snapshot остаётся доступен, изменение оформляется отдельным auditable overlay и обязано пройти весь fork regression suite.
8. Stock RTK не является runtime fallback и не может подменить fork-core. Upstream RTK используется только как reference/base для diff и будущего осознанного backport.

Пользователь получает:

- один CLI `hzr`;
- один локальный daemon `hzrd`;
- один конфигурационный и data root;
- один versioned JSON protocol;
- один сквозной token budget;
- один quality gate и raw fallback;
- один usage/cost/outcome ledger;
- один lifecycle для каждого внешнего движка;
- одну воспроизводимую сборку с hash-locked fork-core и зафиксированными внешними версиями;
- один агентный entry point, где caveman-code отвечает за agent loop и provider UX, а HZR — за context, memory, execution, codec и учёт.

Каноническая формула обработки:

> preserve intent → retrieve once → fuse once → allocate once → encode once → execute safely → verify quality → account actual usage.

Fork-core остаётся целым, но его внешние зависимости и вызовы проходят через HZR ownership boundaries. Независимый запуск нескольких grepai watchers, ICM processes или caveman-code native RTK запрещён: он создаёт повторные scans, повторное сжатие и конкурирующие stores.

## 2. Product contract

HZR обязан уменьшать общую стоимость задачи, а не только размер stdout или ответа модели.

Оптимизируемая функция:

```text
cost_per_accepted_task =
  provider_input_cost
  + provider_output_cost
  + cache_write_cost
  + cache_read_cost
  + retries_cost
  + local_compute_cost
  + failure_penalty
```

Трансформация разрешена, только если ожидаемая выгода положительна после учёта overhead и вероятности retry:

```text
expected_value = saved_tokens_value - transform_overhead - retry_probability * retry_cost
```

Для code, patch, JSON, identifiers, commands, paths, URLs, enums, stack traces и security text применяется exact/lossless policy. При любой неопределённости HZR возвращает raw или content-addressed reference.

## 3. Evidence и выводы исследования

### 3.1 RTK

Обобщённый stock RTK полезен как детерминированный command rewrite и tool-output filter, но его локальная оценка `bytes / 4` не является provider bill. В независимом JetBrains A/B на 86 задачах и 425 billed trials median provider cost в 80 clean low-effort парах вырос на 7,6%, turns — на 13,8%, cache reads — на 14,3%; статистически значимой разницы качества не обнаружено. Узкий Claude Analyzer benchmark, напротив, показал оценочные −18,2% при 3/3 pass. Эти результаты нельзя переносить на текущий `heAdz0r/rtk`: у него значительно более широкий и уже проверенный пользователем pipeline — semantic search, memory planning, modular read, atomic write, guards и расширенные filters. Поэтому именно fork целиком остаётся execution/context core; HZR измеряет end-to-end outcome вокруг него и не заменяет его stock RTK.

Источники: [JetBrains RTK token-savings test](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/), [Claude Analyzer results](https://analyzer.spec-kitty.ai/proof/results.html).

### 3.2 grepai/rgai

grepai даёт semantic/hybrid repository retrieval, symbol graph и call tracing. Maintainer benchmark на Excalidraw сообщает −27,5% billed cost, −55% tool calls и −97% fresh input; узкий независимый Claude Analyzer эксперимент — оценочные −14,5% при 3/3 pass. Оба набора слишком малы для SLA. Вывод: grepai становится единственным владельцем code embeddings/index; `rgai` остаётся stateless facade/router и никогда не создаёт собственную базу.

Источник: [grepai benchmark](https://yoanbernabeu.github.io/grepai/blog/benchmark-grepai-vs-grep-claude-code/).

### 3.3 ICM

ICM подходит для долговременной episodic/semantic memory, structured facts, transcripts и cross-session recall. Он не должен индексировать весь source tree и не должен запускаться отдельно каждым hook. В 0.10.61 HTTP API не имеет feature parity: HTTP store обходит MCP near-duplicate update, auto-link/backrefs и consolidation, а HTTP recall — graph expansion. MCP сохраняет полную write-семантику, но возвращает text-only `ToolResult`. Поэтому HZR держит один stdio MCP process для store, не парсит его human text и использует официальный `icm recall --format json` к той же БД для typed graph-aware recall. Это один store и одна семантика без fork ICM.

ICM не имеет first-class project/role columns: scope задаётся topic namespace, который навязывает HZR. При недоступном ONNX recall деградирует до FTS и обязан отображаться как degraded capability.

Источники: [ICM HTTP implementation](https://github.com/rtk-ai/icm/blob/c3a1bac7cfe401b55fd66af16dfc0c774c02167a/crates/icm-cli/src/http_api.rs), [ICM MCP tools](https://github.com/rtk-ai/icm/blob/c3a1bac7cfe401b55fd66af16dfc0c774c02167a/crates/icm-mcp/src/tools.rs), [ICM protocol](https://github.com/rtk-ai/icm/blob/c3a1bac7cfe401b55fd66af16dfc0c774c02167a/crates/icm-mcp/src/protocol.rs).

### 3.4 Caveman

Caveman v1.9.1 полезен как output contract и representation codec. Его документация указывает нулевую экономию input и overhead около 1–1,5k input tokens/turn. JetBrains на 82 paired coding tasks получил около −8,5% output и примерно −10% expected cost без обнаружимого ухудшения качества, но с большой дисперсией; в независимом 24-prompt тесте короткое `Be brief.` почти совпало с full Caveman. CAVEWOMAN показывает, что input compression часто повышает стоимость и снижает accuracy. Вывод: HZR не переписывает user intent, использует короткий cacheable response contract и безопасный protected codec.

Источники: [Caveman Honest Numbers](https://github.com/JuliusBrussee/caveman/blob/main/docs/HONEST-NUMBERS.md), [JetBrains Caveman study](https://blog.jetbrains.com/ai/2026/07/speak-to-ai-agents-like-cavemen-tosave-tokens/), [Max Taylor benchmark](https://www.maxtaylor.me/articles/i-benchmarked-caveman-against-two-words), [CAVEWOMAN preprint](https://arxiv.org/abs/2606.24083).

### 3.5 caveman-code

`JuliusBrussee/caveman-code` — полноценный TypeScript coding-agent runtime, а не только codec. В нём есть provider streaming, TUI, print/RPC/daemon modes, SDK, tools, sessions, checkpoints, subagents, worktrees, steering/follow-ups и architect/editor patterns. Это ускоряет создание HZR agent UX. Исполняемый npm 0.65.2 имеет source provenance `gitHead=4700b8fad23e45cedbb1a850f03ee9e2d4d49116`; более поздний main commit не является pin опубликованного tarball.

Одновременно runtime по умолчанию содержит собственные:

- RTK adapter;
- PageRank repo map, автоматически инжектируемый каждый turn;
- cavemem/files memory и memory tools;
- tool-result/cave-mode compression;
- session compaction и собственную usage presentation.

Без managed adapter эти функции конфликтуют с HZR. Extension API позволяет перехватывать tool calls и provider payload, но публичный extension context не предоставляет методы отключения repo map и memory. SDK-класс `AgentSession` предоставляет `setRepomapEnabled(false)` и `setMemoryEnabled(false)`. Поэтому HZR запускает caveman-code через SDK bridge, а не как непрозрачный CLI subprocess.

Managed HZR profile обязан:

1. вызвать `setRepomapEnabled(false)` до первого prompt;
2. вызвать `setMemoryEnabled(false)` до первого prompt;
3. установить native `rtk.enabled=false`;
4. отключить native tool-output compression и ML compression;
5. отключить native telemetry и automatic memory hooks;
6. зарегистрировать HZR search/context/memory/exec tools;
7. отправлять provider usage и outcome в HZR ledger;
8. прекратить запуск с понятной диагностикой, если версия SDK больше не гарантирует эти инварианты.

Published benchmark caveman-code рассматривается только как directional maintainer evidence: 25-task MicroBench сообщает около 524k fresh tokens и 14/25 pass против около 1.010m и 15/25 у сравниваемого Codex run. Он включает native RTK, repomap, memory и compression, которые HZR отключает, поэтому результат нельзя приписывать managed HZR и он не заменяет собственный paired benchmark.

Источники: [caveman-code repository](https://github.com/JuliusBrussee/caveman-code), [daemon reference](https://github.com/JuliusBrussee/caveman-code/blob/main/docs/reference/daemon.md).

## 4. Goals и guardrails

### 4.1 Goals для 0.1.x

- Один semantic index на `(workspace_id, canonical_root, embedder, model, dimension)`.
- 100% source/product surface текущего fork присутствует в hash-locked fork-core и доступно через HZR.
- Ноль упрощённых reimplementations в runtime-пути вместо fork-core.
- Ноль project-local index data: допустим только проверенный `.grepai` symlink/pointer на HZR-owned canonical store.
- Один ICM process и одна canonical ICM DB на HZR data root.
- Точный RTK rewrite contract, включая различие rewrite и auto-allow.
- Hard evidence budget по явно маркированной token estimate; никакой hook не добавляет скрытый второй pre-read pack.
- Адаптивный codec с protected spans и raw fallback.
- caveman-code managed mode без native RTK/repo-map/memory/compression duplication.
- Actual provider usage хранится отдельно от estimates.
- Все engines проверяются по version/integrity до запуска.
- Offline local mode по умолчанию; telemetry выключена.

### 4.2 Product metrics

- median actual billed cost per accepted task: минимум −30% к baseline после набора репрезентативных задач;
- median turns: минимум −20%;
- uncached input tokens: минимум −35%;
- tool-result bytes в LLM context: минимум −60%;
- retrieval recall@20: не ниже 95% на gold set;
- task success non-inferiority margin: не хуже baseline более чем на 1 п.п.;
- p95 warm orchestration overhead без LLM latency: не более 250 мс;
- p90 cost отдельной задачи: не выше baseline более чем на 5%;
- stale-index incidents, приведшие к ошибочной правке: 0.

До появления статистически достаточного benchmark UI не имеет права показывать прогноз как доказанную экономию. Значения маркируются `actual`, `tokenizer` или `estimate`.

Managed bridge 0.1.0 фиксирует только наблюдаемые runtime outcomes `completed`, `invalid_response` и `failed`; он не объявляет собственный ответ «принятым». Метку `accepted` и task success задаёт внешний benchmark/harness или будущий явный user-feedback workflow, поэтому текущий `hzr savings` не выдаёт `cost_per_accepted_task` без таких данных.

### 4.3 Non-goals

- Сжатие скрытого chain-of-thought/reasoning провайдера.
- Regex-переписывание code, JSON Schema, enums или command arguments.
- Общая физическая SQLite-база для code index, memory и ledger.
- Облачный control plane по умолчанию.
- Автоматическое удаление найденных legacy/duplicate indexes.
- Копирование всего caveman-code в Rust.
- Переписывание или выборочный перенос текущего RTK fork.
- Замена fork-core на stock RTK при ошибке, version drift или несовпадении API.
- Обещание «нулевой потери качества» без проверяемого критерия.

## 5. Архитектура

```mermaid
flowchart TD
    U["User / HZR CLI"] --> D["authenticated singleton hzrd"]
    C["managed caveman-code"] --> D
    D --> P["prepare one canonical grepai lifecycle"]
    D --> M["one project-scoped ICM recall"]
    P --> F["exact fork memory plan"]
    F --> Z{"code candidates?"}
    Z -->|"yes"| N["normalize metadata"]
    Z -->|"no"| R["one fork rgai adaptive / builtin fallback"]
    R --> G["single grepai store when adaptive"]
    G --> N
    M --> X["dedupe + evidence-estimate budget"]
    N --> X
    X --> C
    C --> E["allowlisted fork exec/read/write tools"]
    E --> FC["complete heAdz0r/rtk fork-core"]
    C --> L["actual / estimated usage ledger"]
```

### 5.1 Ownership matrix

| Concern | Единственный владелец | Derived/read-only consumers |
|---|---|---|
| orchestration, policy, budget | HZR Core | adapters, daemon, agent runtime |
| command surface, rewrite, filters, read/write, guards, trust, discovery | complete `heAdz0r/rtk` fork-core | HZR fork adapter, caveman-code tool bridge |
| fork memory planner и derived workspace cache | complete fork-core | HZR Context; это не durable ICM replacement |
| code embeddings/symbol graph | grepai v0.35.0 | HZR Index, rgai facade |
| exact lexical search и fallback | fork `rgai --builtin` | HZR Context transport |
| cross-session memory | ICM v0.10.61 | HZR Memory, agent runtime |
| transient workspace/git state | HZR Context | retrieval orchestrator |
| natural-language density | HZR Codec, Caveman-derived | prompt/response contracts |
| provider/agent loop | caveman-code managed bridge | HZR CLI |
| usage, cost, retry, outcome | HZR Ledger | reports and policy tuning |

### 5.2 End-to-end flow

1. Adapter передаёт исходный intent без телеграфного переписывания и canonicalizes workspace.
2. HZR подготавливает один managed grepai lifecycle/store, но не запускает отдельный unconditional semantic query.
3. Fork `memory plan --format json` строит основной structural plan; параллельно выполняется один ICM recall с repository-derived project scope.
4. Если planner выбрал code candidates, они используются напрямую. Только при пустом результате выполняется один fork `rgai`: adaptive через canonical grepai либо builtin exact при degradation.
5. Результаты нормализуются в `ContextCandidate` с primary provenance, content hash, generation и token source.
6. Fusion хранит один candidate на content ref, применяет diversity limits и hard limit к маркированным evidence estimates.
7. Agent получает bounded untrusted metadata/snippets/memory summaries и затем exact-читает нужные файлы через fork-backed tool; eager reread всех candidates отсутствует.
8. Перед generation HZR инжектирует короткий stable response-density contract. `hzr codec compile` остаётся отдельной explicit protected transform; ответ agent не проходит второй lossy post-processing.
9. Agent вызывает только HZR allowlisted tools; фактические routing/filter/read/write/guard операции выполняет полный fork-core.
10. Exact JSON дополнительно проходит parser validation, empty output отклоняется, provider usage и terminal outcome записываются в ledger.
11. ICM получает только явно сохранённые durable facts/decisions/handoffs, а не каждый сырой tool output.

## 6. Компоненты 0.1.0

### 6.1 `hzr-protocol`

Versioned envelopes, IDs, privacy/risk/fidelity, token source, intent, context candidate/pack, provenance, health и usage. Protocol type обязан разделять actual и estimated usage.

### 6.2 `hzr-core`

Canonical config/data layout, engine lock, fusion, hard budgets, policy, ledger и migration state. Все решения воспроизводимы по trace ID и policy version.

### 6.3 `fork-core` и `hzr-exec`

`fork-core` — полный hash-locked snapshot текущего `heAdz0r/rtk`, включая его dirty worktree. Это единственная реализация доказанного command/search/read/write/memory-planning поведения. Публичное имя продукта остаётся `hzr`; совместимый внутренний бинарь fork не публикуется как отдельный control plane.

`hzr-exec` — тонкий process/protocol adapter. Он не содержит собственной таблицы RTK rewrites и не повторяет fork filters. Его pipeline:

Typed pipeline:

```text
raw request → HZR policy/permission envelope → fork-core invocation
            → exact exit/stdout/stderr capture → HZR ledger/quality envelope
```

Exit code, stderr, error lines, test failures, paths и identifiers сохраняются. Raw/direct fallback использует предусмотренное самим fork поведение либо исходную команду по явной HZR policy; stock RTK fallback запрещён.

В fork-core сохраняются, среди прочего: полный CLI, все command-specific filters, `rgai_cmd.rs`, `grepai.rs`, `memory_layer/*`, modular read pipeline, atomic write/CAS/locks, hook rewrite/audit, guard/trust/permissions, tracking/gain/economics, discovery, benchmarks и regression tests. Полный перечень и статус находятся в `FORK_PARITY.md` и machine-readable snapshot manifest.

### 6.4 `hzr-index`

- normalizes workspace root through canonical paths and git common dir;
- computes stable workspace/worktree IDs;
- owns one grepai config, watcher and generation;
- checks installed version against `engines.lock.toml`;
- prepares the canonical grepai store for semantic/auto calls;
- exposes lifecycle, placement, generation and migration primitives, but no competing search ranker;
- detects nested/legacy indexes but never deletes them automatically;
- prevents a watcher from another workspace being accepted as healthy;
- rejects real legacy `.grepai` until explicit `hzr migrate apply`;
- invalidates freshness by generation and source content hash.

Stock grepai 0.35.0 не умеет выбирать arbitrary index path и его watcher автоматически обнаруживает linked worktrees. Поэтому HZR создаёт только проверенный `.grepai` symlink на central store и распространяет минимальный source patch `--no-worktree-discovery`. Runtime всегда probes capability и передаёт flag; непатченный watcher в multi-worktree блокируется до spawn, при этом exact search и чтение существующего semantic index продолжают работать.

`rgai` сохраняет реализацию, ranking, compact rendering и fallback chain из fork-core. `hzr search`, `hzr rgai`, context planner и agent tool делегируют ему один запрос; exact mode добавляет `--builtin`, semantic/auto сначала подготавливает managed grepai. `hzr-index` управляет binary/watcher/store/generation и не реализует конкурирующий ranker. `rgai` не владеет storage.

### 6.5 `hzr-memory`

- fixed DB under `<data_root>/memory/icm/memories.db`;
- singleton process lock и один managed stdio MCP process;
- полный MCP store path для near-dup, auto-link/backrefs и consolidation;
- typed official CLI JSON recall к той же DB для graph expansion без human parsing;
- repository-scoped topic namespace и ICM project filter поверх одной общей DB;
- pid/token/log files with private permissions;
- health, recall и store typed client, bounded JSON-RPC framing;
- circuit breaker и correctness-first CLI fallback;
- idempotent start/stop/restart;
- explicit release/version check;
- no automatic indexing of source code.

ICM topics глобальны по upstream design. Поэтому HZR принимает workspace в каждом memory request, вычисляет canonical `repository_id`, добавляет его как отдельный topic segment при store и принудительно применяет тот же project filter при recall. Клиент не может подменить project scope. Память разных репозиториев не смешивается, хотя lifecycle и физическая DB остаются едиными.

### 6.6 `hzr-codec`

Profiles: `off`, `safe`, `adaptive`, `compact`, `shadow`. Protected spans cover code fences, inline code, paths, URLs, flags, hashes, versions, identifiers, enum-like values and structured payloads. `adaptive` checks economics before adding any contract. `shadow` records counterfactual size without changing delivered content.

### 6.7 `hzr-agent`

Managed bridge to caveman-code:

- package version and npm integrity are pinned;
- isolated `agentDir` lives under HZR data root;
- native RTK, repo-map, memory, hooks, compression, external resources, builtin agents/skills and telemetry are disabled before first prompt and rechecked throughout generation;
- only an exact allowlist of HZR context/search/read/edit/write/memory/exec custom tools may execute;
- one bounded unified-context prefetch is injected as untrusted evidence before generation;
- text and strict JSON result modes are supported;
- provider credentials remain in the upstream auth storage or environment and are never copied into HZR ledger;
- daemon health must report protocol 1, HZR 0.1.0 and exactly one ready fork-core before launch;
- provider usage is posted once from the bridge finalizer with `completed`, `invalid_response` or `failed`; accounting failure never masks the primary result;
- managed launch fails closed on invariant mismatch; ordinary HZR tools continue to work.

Exact npm lock разрешает `@juliusbrussee/caveman-agent`, `caveman-ai` и `caveman-tui` в 0.65.3. Из-за transitive `undici` минимальный runtime — Node 20.18.1; Node 26 блокируется из-за известной несовместимости `better-sqlite3` в upstream issue #46. TypeBox закреплён как explicit dependency из-за upstream issue #23. Vulnerable transitive `adm-zip<0.6.0` заменён exact npm override на 0.6.0; release gate требует `npm audit --omit=dev` без high/critical findings.

Остаточный upstream behavior 0.65.2: session construction выполняет неактивный `cavemem --version` probe и строит inactive builtin registry. Runtime guard не позволяет этим builtins выполняться. Полное устранение самого probe требует отдельного SDK patch; это не создаёт вторую memory DB или executable tool path в HZR.

### 6.8 `hzrd` и `hzr-cli`

Minimum daemon API:

```text
GET  /v1/health
GET  /v1/engines
POST /v1/search
POST /v1/context/plan
POST /v1/memory/recall
POST /v1/memory/store
POST /v1/exec/rewrite
POST /v1/exec/run
POST /v1/exec/approval
POST /v1/fork/run
POST /v1/codec/compile
POST /v1/usage
```

CLI surface:

```text
hzr init
hzr doctor [--json]
hzr daemon serve|status|engines
hzr engines status
hzr index status|init
hzr exec rewrite|run|approve|deny
hzr search <query>
hzr rgai <query>
hzr memory recall|store|status
hzr context plan <intent>
hzr codec compile <text>
hzr agent run [--json] <prompt>
hzr savings
hzr migrate scan|apply
hzr rtk -- <fork arguments>
```

`bin/rtk` является относительным compatibility alias на `bin/hzr`, а не вторым installation/control plane. По имени invocation HZR нормализует его в `hzr rtk --` и выполняет private exact `engines/rtk` с исходными argv/cwd/stdio/signals/exit status.

## 7. Data layout и запрет дублей

```text
<hzr-data>/
  runtime/
    hzrd.token
    hzrd.token.lock
    hzrd.lock
  fork/
    mem.db                # fork derived IMG/cache, keyed by project
    history.db            # fork tracking/economics
    tee/                  # managed path sets RTK_TEE=0
    audit/
  workspaces/<repository-id>/<worktree-id>/index/grepai/
    config.yaml
    index.gob
    symbols.gob
    rpg.gob
    hzr-owner.lock
    hzr-runtime/
  migrations/<repository-id>/<worktree-id>/
    grepai-v1.prepared.json
    grepai-v1.json
  memory/icm/
    memories.db
    auth.token
    icm.log
    runtime/
      supervisor.lock
      icm.pid
      token.lock
  ledger/
    usage.sqlite
  sessions/
```

Инварианты:

- source index и memory физически разделены, но имеют общую provenance model;
- HZR не создаёт project-local index data; `.grepai` может быть только проверенным symlink/pointer на canonical store;
- существующая real `.grepai` распознаётся как legacy и блокирует managed search/init до явной migration;
- legacy stores обнаруживаются read-only scan;
- migration выполняется только явной командой с retained full-SHA backup и двумя immutable manifests;
- duplicate/foreign indexes reportable, автоматическое удаление или quarantine запрещено;
- singleton `hzrd` lock плюс worktree owner lock исключают второго HZR watcher;
- один content hash не должен повторно попадать в context pack из разных источников.

## 8. Version и supply-chain policy

Исходный lock для 0.1.0:

| Engine | Версия | Pin |
|---|---:|---|
| grepai | 0.35.0 | tag `v0.35.0`, commit `65c345ca32122c17a39a5bbec2780c2eea773a12` |
| ICM | 0.10.61 | tag `icm-v0.10.61`, commit `c3a1bac7cfe401b55fd66af16dfc0c774c02167a` |
| HZR fork-core | 0.44.1-fork.1, current `heAdz0r/rtk` worktree | branch `feat/upstream-0.42-fork.1`, `HEAD=5f403c465cbdbe148e9ca03e0ac8e856eef0bfee`; 516 files + 4 tracked deletions; canonical snapshot v2 `f4296ec404f461d6fc03c966c0dc79caee6c3118a73d1ed1a078ded5529f0a16`; v1 content manifest `072a62adc754b728ec99a507d2c1a223d83077d067a9249a26d357eec890b4cc` |
| upstream RTK reference only | 0.44.1 | tag `v0.44.1`, commit `36591fb00d650bf987b57483c0b3a395a35a8dc1`; не runtime engine |
| Caveman prompt/codec reference | 1.9.1 | tag `v1.9.1`, commit `0d95a81d35a9f2d123a5e9430d1cfc43d55f1bb0` |
| caveman-code | 0.65.2 | npm integrity + exact lockfile; npm `gitHead=4700b8fad23e45cedbb1a850f03ee9e2d4d49116` |

Исполняемый caveman-code фиксируется npm version, tarball integrity, source `gitHead` и полным lockfile. Exact lock разрешает `caveman-agent`, `caveman-ai` и `caveman-tui` в 0.65.3 с отдельными integrity. Более поздний main не считается provenance tarball.

grepai собирается только из pinned commit после применения [patches/grepai/0.35.0-disable-worktree-discovery.patch](patches/grepai/0.35.0-disable-worktree-discovery.patch); patch должен проходить `git apply --check`, Go tests и capability smoke. ICM source требует отдельного минимального pinned patch, который синхронизирует только устаревшую версию `icm-cli` в upstream `Cargo.lock` с source package 0.10.61 и сохраняет сборку `--locked`. [scripts/build-bundle.sh](scripts/build-bundle.sh) собирает local-platform bundle HZR + **fork-core** + grepai + ICM + exact npm runtime. Сборка stock RTK вместо fork-core является release-blocking ошибкой.

Release build проверяет checksum/integrity, license, executable version и protocol smoke test. Engine auto-update/sync в 0.1.0 отсутствует; будущая реализация не должна обновлять pins без явного подтверждения.

## 9. Security и privacy

- loopback-only daemon по умолчанию;
- bearer token для локального API;
- non-loopback bind не поддерживается в 0.1.0;
- config, DB tokens и runtime secrets имеют private permissions;
- provider API keys не логируются и не сохраняются в ledger;
- managed fork path принудительно выставляет `RTK_TEE=0` и `RTK_TELEMETRY_DISABLED=1`;
- HZR telemetry и raw retention по умолчанию выключены;
- fork read/write API принимает только минимальный allowlist argument shapes и canonical workspace paths;
- shell сохраняется только там, где полная исходная shell-строка необходима для fork rewrite semantics;
- destructive commands требуют отдельного risk/permission verdict;
- daemon body/capture/time limits ограничены, path traversal и symlink escape отклоняются;
- usage ledger хранит счётчики, model/provider metadata и outcome, но не prompt/response body.

## 10. Failure modes

| Failure | Поведение |
|---|---|
| `hzrd` недоступен | managed agent/search/context/memory/exec блокируются; exact compatibility `hzr rtk`/`bin/rtk` остаётся прямым process path |
| grepai отсутствует/устарел | exact rg fallback; semantic status degraded |
| index stale | stale provenance, exact verification перед edit |
| legacy/duplicate/foreign index найден | typed migration-required/error; ничего не удаляется |
| ICM недоступен | context возвращает warning и code plan; прямой memory call сообщает unavailable; agent health сохраняет warning |
| codec invariant нарушен | raw content и failure telemetry |
| fork-core недоступен или version не совпал | managed agent/exec/search блокируются; context может вернуть только ICM с explicit warning; stock RTK не подставляется |
| fork filter выбрал raw/fail-open | сохраняется штатная семантика fork и HZR фиксирует outcome |
| provider usage отсутствует | estimate сохраняется только в estimated columns |
| caveman-code SDK drift | managed agent mode блокируется с remediation; остальные HZR команды работают |
| token budget исчерпан | evidence отклоняется с reason; лимит не расширяется скрыто |

## 11. Migration

`hzr migrate scan` read-only обнаруживает legacy/nested `.grepai`, внешние memory/config/wrapper/process markers и сообщает их без изменения данных.

`hzr migrate apply --workspace` в 0.1.0 намеренно имеет узкую и проверяемую область: он централизует ровно один legacy grepai store. Операция:

1. canonicalizes repository/worktree identity и отклоняет duplicates/foreign entries;
2. удерживает exclusive legacy HZR owner lock;
3. снимает ordered tree snapshot с bytes, Unix modes и safe symlink targets;
4. копирует его в staging и повторно сверяет полный SHA-256;
5. создаёт retained `.grepai.hzr-backup-<full-sha256>` и durable `prepared` manifest;
6. atomically устанавливает managed target и проверенный project `.grepai` symlink;
7. удерживает canonical owner при activation и записывает immutable `applied` manifest;
8. при повторном вызове проверяет manifests/backup/target и возвращает typed `already_applied`.

Escaping symlinks, special files, active HZR owner, source mutation, partial target/stage/manifest и unsafe path relationships блокируют migration. Backup никогда автоматически не удаляется. HZR не останавливает и не удаляет внешние процессы, конфиги, wrappers, hooks или ICM databases без отдельной явно заданной операции.

Старый `/Users/andrew/Programming/rtk` не изменяется разработкой нового репозитория HZR.

## 12. Verification strategy

### 12.1 Rust quality gates

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

### 12.2 Contract tests

- snapshot manifest воспроизводит 100% допустимого source set текущего fork, включая uncommitted/untracked code и tracked deletions;
- весь оригинальный fork test/benchmark harness присутствует и запускается из HZR;
- fork CLI/rewrite/read/write/rgai/memory/guard behavior проходит без функциональных потерь;
- stdout/stderr/exit preservation;
- grepai 0.35.0 JSON fixtures and version drift;
- root/worktree identity and duplicate index detection;
- ICM singleton race, stale PID, token permissions and circuit breaker;
- сумма token estimates выбранных evidence не превышает hard limit;
- protected spans survive codec byte-for-byte;
- estimates never increment actual totals;
- caveman-code duplicate layers are disabled before prompt;
- daemon body limit, timeout, auth and loopback binding.

### 12.3 Paired benchmark

Каждая задача выполняется baseline и HZR с одинаковыми model, temperature, repository revision и max turns. Собираются provider usage, cache usage, turns, tool calls, latency, retries, task success и judge/harness outcome. Отчёт показывает median, p90, confidence intervals и список regressions, а не только суммарные проценты.

## 13. Release acceptance для 0.1.0

Релиз допускается, когда:

- fork-core импортирован целиком из фактического worktree и его manifest проверен независимо;
- `FORK_PARITY.md` не содержит `missing`, `reimplemented` или непроверенных runtime-подмен;
- stock RTK отсутствует в production execution path и bundle;
- все workspace crates компилируются без warnings;
- quality gates зелёные;
- `hzr doctor --json` проверяет все pins и ownership;
- ICM start/stop race test доказывает singleton;
- nested `.grepai` fixture обнаруживается и не удаляется;
- `hzr search` использует grepai 0.35.0 и exact fallback;
- `hzr rgai` использует ту же canonical generation;
- `hzr exec` делегирует полному fork-core и проходит весь fork regression suite плюс adapter contracts;
- codec сохраняет protected spans;
- managed caveman-code smoke test подтверждает отключение duplicate layers;
- CLI/daemon smoke test работает из чистого data root;
- README содержит установку, архитектурные инварианты и recovery;
- ICM содержит актуальный handoff для следующих LOOP-агентов;
- repository имеет initial commit и version `0.1.0`.

## 14. Delivery status и следующий этап

Для 0.1.0 реализованы exact snapshot/parity gate, fork process adapter, singleton daemon, canonical grepai lifecycle/migration, centralized project-scoped ICM, unified context planning, protected codec, Caveman managed bridge, usage ledger, CLI/API и relocatable local-platform bundle.

Сознательно не включены фоновые `daemon start/stop`, automatic engine sync, hook installer и destructive cleanup legacy data. Они не нужны primary managed agent path и расширили бы mutation surface первого релиза. Полный fork setup/init surface остаётся явно доступен через compatibility passthrough.

После release 0.1.0 следующий измеримый этап:

1. paired baseline-vs-HZR benchmark на одинаковых model/repository revision/task/max-turn settings;
2. provider-billed input/output/cache, turns, retries, latency и harness success в одном отчёте;
3. regression corpus для fork filters, context recall и accepted task quality;
4. только после данных — adaptive policy tuning, crash-safe usage outbox и optional background supervisor/installers.

## 15. Decision log

- HZR — самостоятельный продукт и репозиторий, не RTK fork.
- Полный текущий `heAdz0r/rtk` — неизымаемое внутреннее fork-core HZR; HZR строится вокруг него, а не вместо него.
- Новый Git history и имя продукта не дают права удалять, выборочно переносить или переписывать fork functionality.
- grepai — единственный semantic code index.
- rgai — facade, не база.
- ICM — единственная durable agent memory.
- Caveman — адаптивный codec/contract, не обязательный длинный prompt.
- caveman-code — optional managed agent runtime, не второй control plane.
- HZR Core — единственный владелец budget, policy, lifecycle и ledger.
- Actual provider billing — истина; estimates не смешиваются с actual.
- Duplicate stores обнаруживаются безопасно и не удаляются автоматически.
- Quality проверяется task outcome и invariants, а не только количеством токенов.
