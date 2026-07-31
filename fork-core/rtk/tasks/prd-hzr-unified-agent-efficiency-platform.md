# PRD: HZR — единая платформа эффективности LLM-агентов

**Дата:** 2026-07-31  
**Статус:** Decision Locked / Architecture Ready  
**Продукт:** HZR (бренд от ника heAdz0r)  
**Наследуемые системы:** RTK fork, RTK Intent Memory Graph, ICM, rgai/grepai, Caveman  
**Целевой релиз:** HZR 1.0 после поэтапной миграции  
**Главный критерий:** минимальная полная стоимость успешно решённой задачи при доказанном отсутствии значимого ухудшения качества

## 0. Решение

RTK fork прекращает быть продуктовым центром. Новый продукт называется **HZR** и поставляется как единая local-first платформа для coding agents:

1. один CLI — hzr;
2. один долгоживущий daemon — hzrd;
3. один versioned hook-протокол для Claude Code, Codex, Gemini CLI и других адаптеров;
4. один gateway для клиентов, которым доступна настройка LLM endpoint;
5. один конфигурационный контур;
6. один end-to-end token/cost ledger;
7. одна политика качества и один сквозной token budget;
8. один установочный пакет, включающий все необходимые движки.

Внутри HZR сохраняет сильные стороны всех существующих инструментов:

| Источник | Роль в HZR | Чего он больше не делает |
|---|---|---|
| RTK | точное исполнение команд, permission policy, детерминированная фильтрация tool output | не владеет общим планом контекста и не считает локальную «экономию» итоговой |
| RTK IMG | краткоживущая структурная модель текущего workspace, git delta, test map, task/file affinity | не дублирует semantic index grepai и long-term memory ICM |
| grepai | единственный владелец code embeddings, semantic/hybrid search, symbol/call graph index | не запускается ad hoc из каждого hook и не определяет общий token budget |
| rgai | совместимый facade и retrieval router внутри hzr search | не является отдельным индексом |
| ICM | долговременная эпизодическая и семантическая память между сессиями | не индексирует весь исходный код и не вводит собственный параллельный hook pipeline |
| Caveman | representation codec: компактная инструкция, natural-language context и output contract | не выбирает релевантность, не меняет код/JSON/команды и не считается компрессором reasoning |
| HZR Core | lifecycle, risk/fidelity policy, budgets, provenance, retries, telemetry и quality gates | не реализует специализированный поиск или command filters |

Архитектурная формула HZR:

> один retrieval pass → один ranker → один budget allocator → один representation compiler → один quality gate → один ledger.

Независимая цепочка RTK → IMG → grepai → ICM → Caveman запрещена: каждый самостоятельный слой будет оптимизировать собственную метрику, повторно сжимать данные и скрывать источник потери качества.

## 1. Почему нужен новый продукт

### 1.1 Текущий fork оптимизирует лишь часть агентного цикла

RTK хорошо умеет уменьшать отдельные результаты shell-команд. Полная стоимость задачи дополнительно определяется:

- размером системных инструкций и схем инструментов;
- количеством повторных поисков и чтений;
- качеством выбранного repository context;
- размером истории диалога;
- числом agent turns и retries;
- output tokens модели;
- prompt-cache creation/read;
- reasoning tokens, если провайдер их выставляет;
- качеством памяти между сессиями.

Локальная оптимизация stdout не гарантирует экономии всей задачи. Независимый парный тест RTK на 80 задачах обнаружил медианный рост стоимости на 7,6%, рост числа turns и cache reads, несмотря на высокий внутренний счётчик «сэкономленных токенов». Это не доказывает бесполезность RTK-фильтров; это доказывает необходимость считать счёт провайдера и результат задачи, а не сумму локальных оценок: [JetBrains, RTK token savings test](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/).

### 1.2 Caveman полезен, но не является прозрачным LLM-компрессором

Caveman v1.9.1 — прежде всего system-prompt skill, задающий телеграфный стиль ответа. Его автор прямо указывает:

- overhead около 1–1,5 тыс. input tokens на turn;
- выигрыш возникает главным образом на длинных ответах;
- короткие ответы могут стать дороже;
- инструмент не сжимает внутренний reasoning;
- caveman-shrink v1 сокращает описания MCP tools, но не request body и не tool response.

Источник: [Caveman Honest Numbers](https://github.com/JuliusBrussee/caveman/blob/main/docs/HONEST-NUMBERS.md) и [репозиторий Caveman](https://github.com/JuliusBrussee/caveman).

Независимое исследование на 82 парных задачах получило уменьшение output tokens на 8,5% без статистически обнаружимого изменения качества, значительно ниже maintainer headline. Следовательно, HZR должен включать Caveman адаптивно, а не принудительно на каждом turn: [JetBrains, Caveman study](https://blog.jetbrains.com/ai/2026/07/speak-to-ai-agents-like-cavemen-tosave-tokens/).

### 1.3 Основной потенциал — меньше ненужного контекста и turns

Repository intelligence способен влиять на гораздо большую долю счёта, чем shell-only interception. В публичном исследовании JetBrains Context заявлены улучшения «до» 68% по turns, 59% по latency и 48% по cost на нескольких наборах задач. Эти цифры нельзя переносить на HZR как SLA, но они подтверждают приоритет retrieval/context orchestration над микрофильтрами: [JetBrains Context](https://blog.jetbrains.com/ai/2026/07/introducing-jetbrains-context-repository-intelligence-for-coding-agents/).

Официальный небольшой benchmark grepai на Excalidraw сообщает −27,5% billed cost и −55% tool calls, но содержит только один репозиторий, пять вопросов и maintainer-run оценку. HZR использует его как directional evidence, не как доказанный целевой результат: [grepai benchmark](https://yoanbernabeu.github.io/grepai/blog/benchmark-grepai-vs-grep-claude-code/).

### 1.4 Публичные кейсы: где краткость сработала и где метрика обманула

| Кейс | Наблюдение | Вывод для HZR |
|---|---|---|
| JetBrains Caveman, 82 paired tasks / около 240 trials | output −8,5%; 8 задач лучше, 10 хуже, 64 ties; статистически обнаружимого quality loss нет | реалистичная цель agentic output — high-single-digit, включать адаптивно |
| LemonCrow, 20 engineering Q&A × 5 повторов × 3 даты | billed output −44,1%, total token volume −7,8%, стоимость около +0,06%; отдельный run дал +3,3% cost | output reduction не равен monetary savings; учитывать initial prompt и cache |
| Max Taylor, 24 prompts | full Caveman около −36,5% output; короткое «Be brief» дало почти тот же эффект | использовать короткий cacheable contract, а не 1,2k-token skill |
| grepai Excalidraw, 5 вопросов | −27,5% billed cost, −55% tool calls | promising direction, но нужен собственный larger paired benchmark |
| JetBrains Context, несколько наборов coding tasks | reported до −48% cost и −68% turns | главный рычаг — repository intelligence и fewer turns |
| CAVEWOMAN preprint | output constraints снижали стоимость; наивное POS-сжатие input повышало cost примерно на 15% и ухудшало accuracy | исходный user intent нельзя превращать в «ломаный» compressed query |

Ссылки: [LemonCrow raw methodology](https://github.com/lemoncrow-lab/lemoncrow/blob/main/BENCHMARKS.md#telegraphic-qa-benchmark), [Max Taylor benchmark](https://www.maxtaylor.me/articles/i-benchmarked-caveman-against-two-words), [CAVEWOMAN preprint](https://arxiv.org/abs/2606.24083).

Также обнаружен прямой риск готового caveman-shrink: regex удаляет слова модальности и имеет известный кейс повреждения enum Todoist. HZR не включает опубликованный npm proxy; он реализует schema-aware catalog optimizer, который не меняет IDs, JSON Schema, enum, auth/security text и tools/call: [Caveman issue #444](https://github.com/JuliusBrussee/caveman/issues/444).

## 2. Результаты аудита текущего состояния

### 2.1 Критические факты

| Область | Наблюдение | Риск |
|---|---|---|
| Rewrite hook | hooks/rtk-rewrite.sh отбрасывает результат rtk rewrite при exit 3, хотя Rust-контракт использует 3 для rewrite без auto-allow | корректный rewrite молча теряется; permission semantics рассинхронизированы |
| Pipe rewriting | cargo test --all-targets: 1660 passed, 7 failed, 1 ignored; семь failures связаны с выбором стороны pipe | базовый interception path не готов к миграции |
| Formatting | cargo fmt --all --check не проходит в текущем dirty worktree | baseline не воспроизводим |
| Execution seam | Command::new, .output и прямой print распределены по десяткам модулей | Caveman невозможно безопасно вставить одним middleware |
| Context budget | memory plan получает 2400 tokens, затем hook способен добавить до 120 000 символов | заявленный hard budget фактически не сквозной |
| Retrieval | semantic_stage вызывает rtk rgai --files --builtin и тем самым обходит внешний grepai | основной graph-first flow не использует настоящий semantic backend |
| Duplicate search | после memory plan hook ещё раз запускает rgai и pre-read | повторные scans, subprocess и контекст |
| Retrieval certainty | hook сообщает модели, что все релевантные файлы уже включены | false negative retrieval превращается в систематическую ошибку |
| grepai lifecycle | найден watcher PID, относящийся к другому workspace | индекс может обслуживать неверный проект |
| grepai storage | существуют корневой .grepai и вложенный src/.grepai | двойные embeddings, symbols и watcher state |
| ICM lifecycle | ICM подключён как внешний набор hooks/MCP; одновременно работают несколько serve процессов | дублирование memory model и startup overhead |
| Token accounting | RTK использует приближение ceil(chars/4), а memory planner — line-based оценки | локальные проценты нельзя сравнивать с billing |
| Never-worse | guard сравнивает размер raw и filtered | потеря error line или identifier не обнаруживается |
| Raw artifacts | tee хранит raw output без обязательной redaction/encryption | риск секретов и чрезмерной retention |

### 2.2 Текущий поток, который требуется заменить

~~~mermaid
graph TD
    A["Agent hooks"] --> B["Shell scripts"]
    B --> C["rtk rewrite / rtk memory plan"]
    C --> D["RTK command modules"]
    C --> E["rtk rgai subprocess"]
    E --> F["grepai or builtin rg"]
    B --> G["Additional rgai + pre-read"]
    A --> H["Independent ICM hooks / MCP"]
    D --> I["Per-command filters + local token estimates"]
    G --> J["Large injected prompt"]
    H --> J
    J --> K["LLM"]

    style B fill:#ffd6d6
    style G fill:#ffd6d6
    style I fill:#fff0bf
~~~

Главные дефекты потока: shell parsing вместо typed protocol, несколько владельцев бюджета, дублирующий retrieval, несколько lifecycle-процессов и отсутствие общей причинной трассировки от user intent до оплаченного ответа.

## 3. Product Vision

HZR — это **Agent Efficiency Platform**, которая уменьшает полную стоимость и время выполнения задачи на четырёх уровнях:

1. **До LLM:** выбирает только нужные инструменты, memory и repository evidence.
2. **В запросе:** дедуплицирует историю, защищает exact spans, компилирует компактные инструкции и сохраняет cache-stable prefix.
3. **На tools:** безопасно переписывает команды и детерминированно фильтрует output с recoverable raw.
4. **В ответе:** до генерации задаёт адаптивный output contract и лимит; после генерации создаёт компактное производное представление только для будущего контекста.

HZR не обещает «нулевую потерю качества» как нефальсифицируемый лозунг. Продуктовый контракт формулируется проверяемо:

> на репрезентативном paired benchmark нижняя граница 95% доверительного интервала разницы task success не хуже baseline более чем на 1 процентный пункт; exact-классы сохраняют обязательные данные побайтно; при сомнении HZR возвращает raw.

## 4. Goals, метрики и non-goals

### 4.1 Primary goals для HZR 1.0

1. Снизить median actual billed cost на успешно решённую задачу минимум на 30% относительно агента без HZR.
2. Снизить median turns минимум на 20%.
3. Снизить uncached input tokens минимум на 35%.
4. Снизить tool-result bytes, переданные модели, минимум на 60% при сохранении required evidence.
5. Не допустить статистически значимого ухудшения task success; non-inferiority margin — 1 п.п.
6. Для security, destructive operations, code patches, JSON, commands и stack traces обеспечить invariant-based fidelity, а не semantic similarity.
7. Все savings показывать по реальным provider counters; estimates всегда маркировать.

### 4.2 Guardrail metrics

- retry rate не выше baseline более чем на 2 п.п.;
- p90 billed cost отдельной задачи не выше baseline более чем на 5%;
- p95 warm orchestration overhead не более 250 мс без model latency;
- p95 first-byte overhead gateway не более 30 мс;
- retrieval recall@20 не ниже 95% на gold-наборе;
- stale-index incidents, приведшие к ошибочной правке, равны нулю;
- raw fallback всегда доступен по trace ID;
- local telemetry и raw retention выключены по умолчанию.

### 4.3 Secondary goals

- единая установка на macOS/Linux/Windows;
- offline mode для source code, index и memory;
- поддержка 1–10 локальных concurrent agents;
- optional team mode на 100 и 1000 concurrent agents;
- плавная миграция существующих RTK/ICM/grepai данных.

### 4.4 Non-goals

1. Сжатие opaque chain-of-thought или скрытых reasoning tokens провайдера.
2. Regex-переписывание уже оплаченного LLM-ответа как способ уменьшить текущий bill.
3. Замена LLM-провайдера.
4. Автоматическая семантическая компрессия кода, patch, JSON, command arguments или error diagnostics.
5. Обязательная отправка кода/telemetry в HZR cloud.
6. Единая физическая база данных для ICM memory, workspace graph и vector index.
7. Мгновенный big-bang rewrite всего RTK.

## 5. Пользователи и сценарии

### 5.1 Personas

1. **Solo developer:** один ноутбук, 1–3 агента, Ollama и локальный индекс.
2. **Power user:** несколько worktree и 3–10 subagents, жёсткий budget и необходимость объяснить каждую трансформацию.
3. **Team:** общий remote index/memory, 10–100 concurrent agents, RBAC и workspace isolation.
4. **Platform engineer:** 100–1000 agents, provider gateway, cost attribution и canary policies.

### 5.2 Основные jobs-to-be-done

- найти правильные файлы без полного grep/read обхода;
- восстановить прошлые решения и ошибки без повторного исследования;
- выполнить shell/tool call с меньшим output, сохранив exit semantics и errors;
- сформировать короткий, но достаточный LLM request;
- ограничить длинный ответ до генерации;
- увидеть, почему HZR выбрал или отбросил контекст;
- одной командой вернуть raw и повторить запрос без компрессии;
- доказать экономию по счёту провайдера, а не по эвристике.

## 6. Архитектурные принципы

1. **One owner per concern.** У code index, memory, budget, policy и ledger ровно по одному владельцу.
2. **Canonical raw, derived compact.** Raw или content-addressed reference является источником истины; compact representation всегда производное и versioned.
3. **Preserve intent.** Исходный user intent не сжимается перед retrieval.
4. **Retrieval before compression.** Сначала определить релевантность, затем уплотнять natural language.
5. **Exact by type, not by hope.** Fidelity определяется типом данных и risk policy.
6. **Adaptive economics.** Transform включается, только когда ожидаемая экономия превышает overhead и риск retry.
7. **Cache-aware prompts.** Стабильный system prefix важнее агрессивного переписывания каждой инструкции.
8. **Fail open to raw.** Недоступность HZR не блокирует обычный tool/agent flow, кроме явно запрещённых security policy.
9. **Typed internal protocols.** Human text предназначен человеку, protobuf/typed JSON — компонентам.
10. **Actual billing is truth.** Локальные estimates не суммируются и не называются итоговой экономией.
11. **Local-first privacy.** Сеть и telemetry — opt-in.
12. **One product, replaceable engines.** Пользователь видит HZR; движки обновляются независимо за stable contract.

## 7. Целевая архитектура

### 7.1 Runtime topology

~~~mermaid
graph TD
    U["User / Coding Agent"] --> AD["HZR Adapter<br/>Claude, Codex, Gemini, MCP"]
    API["Direct LLM Client"] --> GW["HZR Gateway<br/>OpenAI / Anthropic compatible"]
    AD --> D["hzrd singleton"]
    GW --> D

    subgraph CORE["HZR Control Plane"]
        D --> SES["Session Orchestrator"]
        SES --> POL["Policy + Risk + Fidelity"]
        SES --> BUD["Hierarchical Token Budget"]
        SES --> RET["Retrieval Orchestrator"]
        SES --> COD["Representation Compiler"]
        SES --> QG["Quality Gate + Raw Fallback"]
        SES --> LED["Usage / Cost / Outcome Ledger"]
    end

    RET --> MEM["HZR Memory<br/>ICM-derived long-term memory"]
    RET --> CTX["HZR Context<br/>RTK IMG workspace model"]
    RET --> IDX["hzr-indexd<br/>grepai-derived index"]
    RET --> EX["Exact search<br/>rg / symbol / paths"]

    SES --> EXEC["HZR Exec<br/>RTK-derived rewrite + permissions"]
    EXEC --> FIL["HZR Filter<br/>deterministic tool-output codecs"]
    FIL --> QG
    COD --> LLM["LLM Provider"]
    LLM --> QG

    MEM --> STORE["HZR data root<br/>separate stores + provenance"]
    CTX --> STORE
    IDX --> STORE
    LED --> STORE
~~~

### 7.2 Почему «один продукт» не означает «один процесс»

Целевой HZR является одной платформой по контрактам продукта:

- единая команда установки;
- один public CLI/API;
- единый daemon lifecycle;
- единый config;
- единая version matrix и upgrade;
- единый diagnostics/doctor;
- единый trace ID и ledger;
- единая quality policy.

grepai реализован на Go и имеет собственную оптимизированную модель vector storage/watch. Встраивание его исходников в Rust big-bang rewrite создаст риск и затруднит upstream sync. Поэтому HZR 1.0 включает hzr-indexd как закреплённый sidecar, которым полностью управляет hzrd. Для пользователя это такой же внутренний engine, как browser helper в IDE.

ICM и RTK написаны на Rust. Их функциональность поэтапно выделяется в workspace crates. На переходных релизах HZR может обращаться к закреплённому ICM service adapter, но GA-цель — hzr-memory как внутренний crate/service без отдельной пользовательской установки.

### 7.3 Целевая структура workspace

    crates/
      hzr-cli          # public CLI hzr и compatibility binary rtk
      hzr-daemon       # hzrd lifecycle, queues, health, supervision
      hzr-protocol     # versioned envelopes и engine contracts
      hzr-core         # session orchestration, policy, budgets
      hzr-intercept    # adapters: hooks, MCP, IDEs, provider gateway
      hzr-exec         # RTK-derived command discovery, rewrite, permissions
      hzr-filter       # RTK-derived deterministic output filters
      hzr-search       # exact routing, rgai facade, grepai RPC client
      hzr-context      # RTK IMG-derived workspace model
      hzr-memory       # ICM-derived long-term memory
      hzr-compress     # Caveman-derived codecs, prompt compiler, validators
      hzr-store        # data catalog, migrations, content-addressed raw
      hzr-observe      # trace, actual usage, cost and outcome ledger
      hzr-gateway      # OpenAI/Anthropic-compatible streaming proxy
    engines/
      grepai-indexd/   # pinned Go engine + HZR RPC patch layer
    adapters/
      claude/
      codex/
      gemini/
      mcp/

## 8. Владение компонентов и запреты

### 8.1 HZR Core

Владеет:

- session state machine;
- task/risk classification;
- fidelity class;
- deadline и cancellation;
- общим input/output/tool budget;
- policy selection;
- fallback;
- provenance и trace.

Запрещено:

- считать semantic score самостоятельно;
- выполнять regex-компрессию кода;
- хранить engine-specific schema.

### 8.2 HZR Exec и HZR Filter, наследники RTK

Владеют:

- парсингом shell grammar;
- rewrite verdict;
- permission verdict;
- точным запуском subprocess;
- раздельными stdout/stderr/exit/signal/duration;
- deterministic filters;
- streaming и recoverable raw.

Контракт rewrite больше не использует shell exit code как многозначный transport:

    {
      "protocol": "hzr.exec.v1",
      "verdict": "allow_raw | allow_rewrite | ask | deny",
      "original": "git log ...",
      "rewritten": "hzr git log ...",
      "reason_code": "supported_filter",
      "permission": "ask",
      "semantic_risk": "low"
    }

Любой adapter обязан обработать все verdict values. Exit code процесса используется только для transport failure.

### 8.3 HZR Index, наследник grepai

Владеет:

- canonical workspace index;
- chunks/embeddings;
- symbol и call graph;
- file watcher;
- vector/hybrid retrieval;
- index generation и embedder fingerprint.

Обязательный HZR patch contract:

- AllowedPaths или file-set pushdown;
- BatchSearch;
- content_ref вместо обязательной передачи полных snippets;
- root-bound readiness handshake;
- workspace ID и worktree ID;
- cancellation/deadline;
- generation lag;
- reconcile scan после fsnotify overflow;
- model/config/dimension fingerprint;
- единственный owner GOB на workspace.

rgai сохраняется как команда совместимости:

    hzr rgai "intent"       -> hzr search --mode auto
    rtk rgai "intent"       -> compatibility shim -> hzr search

### 8.4 HZR Context, наследник RTK IMG

Владеет краткоживущими данными:

- manifests и module map;
- git delta/churn;
- test map;
- workspace structural graph, пока нужные части не получены из indexd;
- active task/file affinity;
- freshness state;
- context candidate features.

Не хранит полный второй embedding index. Дубли call graph с grepai устраняются после parity migration; до этого каждый edge имеет source и generation.

### 8.5 HZR Memory, наследник ICM

Владеет долговременными данными:

- facts, decisions и preferences;
- episodic traces и outcomes;
- ошибки прошлых подходов;
- memoir/topic graph;
- cross-session recall;
- consolidation и retention.

Исходный source code в память целиком не индексируется. Memory item обязан иметь provenance, confidence, scope, created_at, last_verified_at и invalidation rule.

Canonical fact/event сохраняется независимо от Caveman. Compact memory view содержит:

    raw_hash
    canonical_ref
    codec_id
    codec_version
    compact_text
    protected_entities
    created_at

### 8.6 HZR Compress, наследник Caveman

Владеет только представлением:

- коротким output contract;
- компиляцией natural-language instructions;
- structural dedupe;
- сокращением разрешённых tool descriptions;
- compact view для повторного контекста;
- protected spans и validators.

Не владеет:

- retrieval query;
- truth/relevance;
- permission decisions;
- code transformations;
- post-hoc обещанием bill savings.

## 9. Сквозной execution pipeline

### 9.1 User request и context plan

~~~mermaid
sequenceDiagram
    participant A as Agent Adapter
    participant H as hzrd
    participant M as HZR Memory
    participant C as HZR Context
    participant I as HZR Index
    participant P as Prompt Compiler
    participant L as LLM

    A->>H: IntentEnvelope(raw user intent)
    H->>H: classify risk, fidelity, budget
    par Long-term recall
        H->>M: recall(intent, scope, budget)
    and Workspace candidates
        H->>C: graph/delta/test candidates
    and Semantic/exact retrieval
        H->>I: batch search(raw/canonical intent)
    end
    H->>H: RRF + MMR + hard-budget pack
    H->>P: selected evidence + protected spans
    P->>P: cache-stable instruction + adaptive codec
    P->>L: final serialized request
    L-->>H: response + provider usage
    H->>H: validate, fallback, ledger
    H-->>A: response + trace ID
~~~

### 9.2 Tool call

1. Adapter передаёт typed ToolEnvelope.
2. HZR Policy классифицирует exactness и destructive risk.
3. HZR Exec формирует allow_raw, allow_rewrite, ask или deny.
4. Команда исполняется один раз.
5. Raw stdout/stderr записываются только по retention/privacy policy.
6. HZR Filter выполняет детерминированное структурное сокращение.
7. Если budget всё ещё превышен и content class разрешает, HZR Compress создаёт semantic compact view.
8. Quality Gate проверяет обязательные errors, paths, identifiers, exit code и protected spans.
9. При failure возвращается raw или lossless variant.
10. ICM получает outcome event асинхронно, вне latency hot path.

### 9.3 Response pipeline

Для уменьшения оплачиваемых output tokens HZR обязан действовать **до генерации**:

- выбрать output budget;
- передать короткий task-specific brevity contract;
- использовать provider verbosity/reasoning controls, если они доступны;
- ограничить max output tokens с reserve для корректного завершения;
- попросить структурированный ответ только при реальной необходимости.

Компрессия уже полученного ответа уменьшает только будущий input context или сетевой payload. Она не отменяет токены, уже сгенерированные и оплаченные провайдеру.

### 9.4 Feedback loop

После turn HZR связывает:

    intent
      -> retrieved evidence
      -> files actually read
      -> files changed
      -> tests run
      -> task outcome
      -> actual provider usage
      -> retries/fallbacks

Эта связь улучшает task/file affinity и policy selection. Автоматическое обучение не может менять exact policy и security rules без versioned release.

## 10. Retrieval architecture

### 10.1 Запрос не сжимается до поиска

HZR сохраняет:

- raw intent на языке пользователя;
- normalized intent;
- optional bilingual/English retrieval intent;
- extracted identifiers, paths и symbols.

Caveman-style телеграфный текст нельзя использовать как единственный embedding query: он может выбросить семантические связи и почти не даёт денежной экономии при локальных embeddings. grepai рекомендует описательные запросы; для русских задач HZR должен проверить multilingual embedder либо использовать производный bilingual intent: [grepai Search Guide](https://yoanbernabeu.github.io/grepai/search-guide/).

### 10.2 Router

| Intent | Primary path | Verification |
|---|---|---|
| точный identifier/path/regex | exact rg/symbol index | exact zero-result proof |
| концептуальный вопрос | vector search | lexical identifiers из top hits |
| identifier + concept | hybrid lexical/vector | exact symbol match |
| impact/refactor | vector + callers/callees + git delta | precise trace для изменяемых symbols |
| высокий-confidence candidate set | AllowedPaths semantic search | wide-search sample |
| низкий confidence | broad discovery | не ограничивать преждевременно |
| security/destructive | exact + precise graph | semantic summary не является доказательством |

### 10.3 Fusion

Сырые scores разных backend не складываются как 0.65 graph + 0.35 semantic. HZR использует:

1. weighted Reciprocal Rank Fusion;
2. source-specific calibration на offline relevance set;
3. MMR/diversity, чтобы один файл и соседние chunks не заполнили budget;
4. coverage constraints для implementation, tests, config и callers;
5. token-value packing;
6. reread exact lines с диска непосредственно перед edit.

### 10.4 Единственный hard budget

Контекст выбирается по фактически сериализованным токенам выбранной модели:

    total_input_budget =
      stable_system_prefix
      + user_intent
      + tool_schema_subset
      + long_term_memory
      + workspace_evidence
      + conversation_delta
      + protocol_overhead

    context_window >= total_input_budget + reserved_output + safety_margin

chars/4 и tokens-per-line разрешены только как быстрый pre-estimate. Финальный pack обязан использовать модельный tokenizer или provider token count endpoint.

Over-budget minimum-one исключение не маскирует реальную стоимость. Если обязательный fragment не помещается, planner возвращает explicit budget_exceeded и предлагает увеличить budget, убрать другой fragment или использовать content_ref.

## 11. Adaptive Caveman policy

### 11.1 Fidelity classes

| Class | Контент | Разрешённые преобразования |
|---|---|---|
| exact | commands, args, code, patch, JSON, schemas, paths, symbols, line ranges, errors, stack traces, security output | byte-preserving framing, chunking, references |
| lossless_structural | повторяющиеся headers, boilerplate, duplicate lines, ANSI, known progress frames | deterministic dedupe/filter с invariants |
| semantic | документация, narrative logs, history explanations, memory prose | Caveman compact view + entity/coverage validation |
| summary | пользовательский обзор, status, low-risk explanation | aggressive brevity contract с raw reference |

### 11.2 Protected spans

Всегда защищаются:

- fenced/inline code;
- URL и file path;
- symbol/function/type names;
- CLI flags и environment variables;
- numbers, units, versions, hashes;
- citations;
- quoted user requirements;
- error/failure lines;
- policy/security instructions;
- serialization keys.

### 11.3 Economic gate

Codec включается, если:

    expected_benefit =
      expected_removed_input_tokens * input_price
      + expected_removed_output_tokens * output_price
      + expected_removed_cache_write_tokens * cache_write_price
      + expected_avoided_turns * expected_turn_cost
      - codec_instruction_tokens * effective_input_price
      - codec_compute_cost
      - retry_probability_delta * expected_retry_cost

    expected_benefit > policy_margin

Дополнительные правила:

1. full Caveman skill не добавляется на каждый turn;
2. HZR компилирует короткий task-specific contract и держит stable prefix cacheable;
3. для ожидаемого короткого ответа codec выключается;
4. teaching, architecture rationale и ambiguous tasks получают normal, а не ultra-terse profile;
5. security и irreversible actions используют exact/lite;
6. при первом validation failure профиль автоматически понижается;
7. при втором failure transform отключается для session;
8. post-response compact view никогда не заменяет canonical response.

### 11.4 Profiles

| Profile | Назначение |
|---|---|
| off | baseline/raw; диагностика и high-risk |
| safe | structural dedupe, protected spans, короткая инструкция |
| adaptive | default; economic gate и risk-aware output budget |
| compact | заранее выбранные low-risk long-form tasks |
| shadow | строит compact вариант и метрики, но модели/пользователю отдаёт baseline |

## 12. Integration surfaces и реальные возможности

| Surface | Repository context | Tool output | Prompt/history | Billed LLM output | Provider usage |
|---|---|---|---|---|---|
| HZR Gateway для direct API | полный контроль | полный контроль | полный контроль | pre-generation contract и limits | точные counters |
| Claude/Codex/Gemini adapter с hooks | зависит от hook API | да, на перехваченных tools | injection/delta, не весь opaque prompt | только style instruction; финальный stream может быть недоступен | если surface сообщает |
| MCP-only | результаты HZR tools и tool descriptions | только HZR tools | нет полного контроля | нет | estimates/частичные |
| Caveman skill-only | нет | нет | добавляет instruction overhead | влияет через prompting | обычно нет |

HZR UI обязан показывать capability report. Нельзя обещать transparent request/response compression на surface, который не позволяет увидеть соответствующие данные.

## 13. Protocol и data contracts

### 13.1 Envelope

    Envelope {
      protocol_version
      request_id
      trace_id
      session_id
      workspace_id
      worktree_id
      event_kind
      source_adapter
      deadline_ms
      model_id
      tokenizer_id
      privacy_class
      fidelity_class
      token_budget
      raw_content_ref
      content_hash
      protected_spans[]
      provenance[]
      payload
    }

### 13.2 Candidate

    ContextCandidate {
      candidate_id
      source: exact | index | context | memory
      content_ref
      workspace_generation
      path
      symbol
      line_start
      line_end
      source_rank
      calibrated_relevance
      estimated_tokens
      freshness
      trust
      evidence_kind
      provenance
    }

### 13.3 Tool result

    ToolResult {
      command_original
      command_effective
      rewrite_verdict
      permission_verdict
      exit_code
      signal
      stdout_ref
      stderr_ref
      filtered_content
      filter_id
      filter_version
      required_evidence[]
      duration_ms
      fallback_reason
    }

### 13.4 Usage ledger

    UsageRecord {
      trace_id
      provider
      model
      input_tokens_actual
      output_tokens_actual
      reasoning_tokens_actual
      cache_write_tokens_actual
      cache_read_tokens_actual
      estimated_fields[]
      tool_raw_bytes
      tool_filtered_bytes
      turns
      retries
      latency_ms
      outcome
      quality_score
      policy_version
    }

Поле без provider counter не получает суффикс actual. Никакие проценты разных layers не складываются и не перемножаются.

### 13.5 Internal transport

- Unix domain socket с mode 0600 по умолчанию;
- Windows named pipe;
- protobuf/Connect или gRPC с version negotiation;
- localhost HTTP только opt-in с bearer token;
- MCP — публичный adapter, не internal bus;
- streaming с backpressure;
- cancellation и deadlines обязательны;
- idempotency key для memory/event writes.

## 14. Daemon lifecycle

hzrd:

1. вычисляет canonical workspace через realpath, git common-dir и worktree ID;
2. создаёт root-bound singleton registry;
3. прогревает ICM embeddings;
4. запускает ровно один hzr-indexd watcher на workspace;
5. проверяет root, generation и embedder fingerprint при каждом readiness;
6. обслуживает bounded priority queues;
7. выносит extraction/consolidation/telemetry из hot path;
8. применяет circuit breaker для Ollama, indexd и provider;
9. умеет direct-mode fallback без daemon;
10. завершает sidecars и очищает stale PID/socket безопасно.

Health response:

    {
      "status": "ready | degraded | rebuilding",
      "hzr_version": "1.x",
      "protocol_version": 1,
      "workspace_root": "/canonical/path",
      "workspace_id": "...",
      "worktree_id": "...",
      "index_generation": 42,
      "embedder_fingerprint": "...",
      "memory_generation": 18,
      "capabilities": ["exact", "semantic", "memory", "gateway"]
    }

## 15. Storage и cache architecture

Единый data root не означает одну schema:

    $XDG_DATA_HOME/hzr/
      state.sqlite
      workspaces/<workspace-id>/
        context/context.sqlite
        memory/icm.sqlite
        indexes/grepai/
        artifacts/raw/
        migrations/manifest.json
      ledger/usage.sqlite

Причины разделения:

- ICM memory имеет retention и trust semantics;
- workspace context инвалидируется по git/files;
- vector index имеет собственный backend и generation;
- ledger может храниться дольше без raw content;
- failure одного store не должен повреждать остальные.

### 15.1 Cache levels

| Level | Key | Invalidation |
|---|---|---|
| L0 request | request ID + operation | конец request |
| L1 retrieval | intent hash + workspace generation + filters + embedder + config | generation/config change |
| L2 embeddings | content hash + embedder fingerprint | model/dimension change |
| L3 context graph | workspace + file hashes + extractor version | file/git delta |
| L4 memory recall | normalized intent + memory generation + scope | memory write/consolidation |
| L5 packed prompt | candidate hashes + budget + tokenizer + policy | любое изменение inputs |
| L6 provider prefix | exact stable prefix + model/provider | provider cache semantics |

Для GOB разрешён единственный hzr-indexd owner. Team mode использует Postgres/pgvector или Qdrant с server-side filters. Upstream отмечает ограничения concurrent GOB access и отдельные индексы для worktree: [grepai storage backends](https://yoanbernabeu.github.io/grepai/backends/stores/) и [worktree guide](https://yoanbernabeu.github.io/grepai/git-worktrees/).

## 16. Public CLI и UX

### 16.1 Основные команды

    hzr init
    hzr doctor [--deep] [--json]
    hzr daemon start|stop|status|logs
    hzr hook install|remove|status <adapter>
    hzr search <intent> [--exact|--semantic|--hybrid|--trace]
    hzr trace <symbol> [--precise]
    hzr context plan <task> --budget <tokens>
    hzr memory recall|store|forget|inspect
    hzr run -- <command>
    hzr explain <trace-id>
    hzr raw <trace-id>
    hzr gain [--actual-only]
    hzr policy show|test|set
    hzr gateway start|status
    hzr bench paired
    hzr migrate plan|apply|verify|rollback

### 16.2 Compatibility

    rtk <command>  -> hzr compatibility dispatcher
    rtk rgai       -> hzr search
    RTK_*          -> fallback only if matching HZR_* is absent

Compatibility shim существует минимум два стабильных release cycles после HZR 1.0. Каждое использование deprecated command печатает machine-readable warning в diagnostics channel, не в stdout tool result.

### 16.3 Single hook dispatcher

Вместо набора shell scripts adapter вызывает:

    hzr hook dispatch --adapter claude --event PreToolUse

stdin/stdout — versioned JSON. Dispatcher внутри вызывает policy, context, memory и exec в правильном порядке. Human/emoji output никогда не парсится через grep/sed.

## 17. Configuration

Приоритет:

    session overrides
      > project .hzr/config.toml
      > user $XDG_CONFIG_HOME/hzr/config.toml
      > HZR_* environment
      > RTK_* compatibility fallback
      > defaults

Пример:

    schema_version = 1

    [daemon]
    mode = "local"
    autostart = true
    socket = "auto"

    [workspace]
    canonical_root = "git-common-dir"
    worktree_isolation = true

    [retrieval]
    router = "adaptive"
    fusion = "weighted-rrf"
    diversity = "mmr"
    recall_target = 0.95
    exact_verify = true

    [index]
    engine = "grepai"
    backend = "gob"
    embedder = "nomic-embed-text"
    watcher_owner = "hzrd"

    [memory]
    engine = "icm"
    extraction = "async"
    raw_source_code = false

    [codec]
    engine = "caveman-derived"
    profile = "adaptive"
    protected_spans = true
    full_skill_per_turn = false

    [budget]
    tokenizer = "provider"
    output_reserve_ratio = 0.15
    hard_enforcement = true

    [privacy]
    telemetry = "off"
    raw_retention = "0s"
    secret_redaction = true

    [gateway]
    enabled = false
    bind = "127.0.0.1"

Config имеет schema version, dry-run validation и automatic backup. Неизвестные security-related keys являются ошибкой, а не молча игнорируются.

## 18. Security, privacy и trust

1. UDS/pipe доступен только текущему пользователю.
2. TCP требует explicit enable, bearer token и origin/host validation.
3. Retrieved repository/memory content маркируется как untrusted data, не как instruction.
4. Prompt injection из source files не может менять HZR policy.
5. Destructive/write command не auto-rewrite без доказанной semantic equivalence.
6. Secrets detector работает до raw persistence и telemetry.
7. Raw retention выключена по умолчанию; включение требует duration/size limit.
8. Optional encryption key хранится в OS keychain.
9. Memory supports scope, TTL, forget/export и provenance.
10. Provider egress имеет allowlist и redact policy.
11. Sidecars закреплены по version/hash; поставка включает SBOM.
12. Project path канонизируется и проверяется против workspace boundary.
13. Trace UI не показывает raw secrets без explicit reveal.
14. Telemetry никогда не отправляет user prompt, source code или memory text по умолчанию.

## 19. Quality system

### 19.1 Validators

| Data class | Validator |
|---|---|
| command | AST/argv equivalence, quoting, redirect, pipe and heredoc tests |
| process result | exit/signal, stderr/error lines, required identifiers |
| code/patch | byte hashes protected spans, parser and test/lint |
| JSON/schema | parse + schema validation + key/value invariants |
| retrieval | gold recall@K, exact reread, generation freshness |
| semantic compact | entities, numbers, negations, citations, entailment/coverage |
| memory | provenance reachable, confidence and invalidation |
| LLM response | task-specific verifier/tests plus output truncation detection |

Semantic LLM-as-judge разрешён как дополнительный сигнал, но не заменяет deterministic verifier.

### 19.2 Never-worse v2

Текущий size-only guard заменяется multi-dimensional decision:

    use_transformed =
      token_count(transformed) < token_count(raw)
      AND required_evidence_preserved
      AND schema_valid
      AND semantic_risk <= policy_threshold
      AND expected_retry_cost < expected_savings

При failure:

1. lossless structural variant;
2. raw content_ref + selected exact excerpts;
3. full raw по запросу/политике.

### 19.3 Circuit breakers

- per filter/version;
- per command family;
- per workspace;
- per provider/model;
- per codec profile;
- global emergency HZR_BYPASS=1.

Regression detector автоматически переводит transform в shadow/off, если растут retries, rereads, failed tests или cost outliers.

## 20. Observability и доказательство экономии

### 20.1 Единственный trace

trace_id проходит через adapter, memory, context, indexd, exec, codec, provider и outcome. Trace хранит causal decisions, но raw content — только по privacy policy.

### 20.2 Primary metric

    actual billed cost / accepted task

Если provider не сообщает цену, HZR хранит actual token categories и применённую versioned price table отдельно. Estimates не смешиваются с actual.

### 20.3 Paired evaluation

Для release gate:

1. не менее 200 парных задач;
2. минимум 10 репозиториев;
3. Rust, Go, TypeScript/JavaScript, Python и mixed monorepo;
4. bugfix, feature, refactor, investigation и documentation;
5. одинаковые model, effort, environment и initial state;
6. randomized order и clean checkout;
7. минимум три повтора для stochastic tasks;
8. automatic tests плюс blinded review;
9. actual provider counters;
10. bootstrap confidence intervals и outlier report.

Сравниваются:

- baseline agent;
- HZR safe;
- HZR adaptive;
- ablation без каждого engine;
- shadow policy candidates.

### 20.4 Нельзя делать

- считать grep output целиком прочитанным baseline без наблюдения;
- называть chars/4 фактическими токенами;
- складывать «60% RTK + 30% Caveman»;
- исключать failed/retried runs;
- публиковать только median без tails;
- считать post-hoc compressed response экономией текущего API call.

## 21. Performance и reliability SLO

| Operation | Local warm p95 | Local cold p95 | Failure behavior |
|---|---:|---:|---|
| hook dispatch + policy | 10 мс | 30 мс | adapter raw fallback |
| exact search | 80 мс | 250 мс | native rg |
| memory recall | 75 мс | 800 мс | no-memory continuation |
| semantic search | 300 мс | 2 с | exact/lexical fallback |
| context plan | 500 мс | 3 с | minimal exact context |
| exec filter overhead | 5% duration или 25 мс | 10% | raw output |
| gateway first-byte overhead | 30 мс | 100 мс | direct provider bypass |
| index freshness | 2 с | rebuild state explicit | exact reread before edit |

Memory extraction, consolidation, benchmark upload и heavy index reconcile не входят в foreground hot path.

## 22. Scalability modes

### 22.1 Personal: 1–3 concurrent agents

- один hzrd;
- один indexd per active workspace;
- GOB/vector files;
- SQLite WAL для context/memory/ledger;
- in-process caches;
- resource target: до 1,5 ГБ RAM на крупный workspace;
- no external infrastructure.

### 22.2 Team: до 100 concurrent agents

- stateless HZR gateways;
- workspace-affinity routing;
- Postgres для metadata/ledger;
- pgvector или Qdrant для embeddings;
- object storage для encrypted raw artifacts;
- queue для memory consolidation/index jobs;
- per-tenant quotas и RBAC;
- local exact reread через checked-out worker.

### 22.3 Platform: до 1000 concurrent agents

- sharding по tenant/workspace;
- replicated control plane;
- event bus/outbox;
- autoscaled index/search workers;
- cache isolation и admission control;
- regional provider egress;
- SLO/error-budget based codec rollout;
- immutable policy and engine manifests.

Подробные оценки находятся в связанном performance report. Эти числа являются capacity targets, а не текущими benchmark results.

## 23. Packaging и лицензирование

### 23.1 Distribution

Одна установка HZR включает:

- hzr;
- hzrd;
- hzr-indexd;
- rtk compatibility shim;
- adapter manifests;
- default policies;
- license/NOTICE/SBOM;
- systemd/launchd service definitions, optional Windows service.

Sidecar manifest содержит version, upstream commit, SHA-256, protocol version и license.

### 23.2 License strategy

- существующий RTK-код сохраняет MIT notices;
- Caveman-derived части сохраняют MIT notices;
- grepai-derived sidecar сохраняет MIT notices;
- ICM-derived части сохраняют Apache-2.0 и NOTICE obligations;
- umbrella distribution HZR рекомендуется выпускать под Apache-2.0, сохраняя component-level notices; новый код, предназначенный для внешнего reuse, может быть dual MIT/Apache-2.0 после юридической проверки.

Запрещено удалять upstream authorship и изображать HZR как код, написанный с нуля.

## 24. Миграция RTK → HZR

### 24.1 Правила

1. Никакого destructive move пользовательских данных.
2. Сначала plan, затем backup, apply, verify.
3. Migration idempotent.
4. Старый binary/config остаётся rollback target.
5. Hooks переключаются atomically.
6. Каждый импортированный store получает manifest и checksum.
7. Nested/duplicate grepai indexes не объединяются без canonical-root проверки.

### 24.2 Data paths

| Legacy | HZR target | Метод |
|---|---|---|
| RTK history/tracking DB | ledger/import/rtk | read-only import, estimates marked |
| RTK mem.db | workspace/context | schema-versioned copy |
| RTK tee | artifacts/raw | opt-in import после secret scan |
| .grepai | indexes/grepai | canonical index import/rebuild |
| nested src/.grepai | quarantine candidate | doctor report, no auto-delete |
| ICM DB | memory/icm | adapter migration + backup |
| RTK hooks | HZR adapter manifest | atomic install with restore file |

### 24.3 Compatibility timeline

| Release | Behaviour |
|---|---|
| HZR 0.1 | hzr facade, no data move, rtk unchanged |
| HZR 0.2 | hzrd, typed protocol, adapters; rtk shim opt-in |
| HZR 0.3 | unified retrieval/context; old memory path fallback |
| HZR 0.4 | adaptive codec shadow/safe |
| HZR 0.5 | gateway and actual ledger |
| HZR 1.0 | hzr default, rtk shim supported |
| HZR 1.x + 2 stable cycles | deprecation telemetry/docs |
| HZR 2.0 earliest | removal considered only with migration evidence |

## 25. Delivery phases

### P0 — Baseline stabilization

- исправить exit 3 hook protocol;
- исправить семь pipe rewrite regressions;
- получить clean fmt/clippy/test baseline;
- golden corpus для pipes, redirects, heredocs, quoting;
- перестать утверждать неподтверждённые savings;
- отключить false certainty в Task hook.

Exit gate: текущий RTK baseline зелёный и воспроизводимый.

### P1 — HZR identity и protocol seam

- переименовать product/package docs в HZR;
- создать Cargo workspace;
- выделить hzr-protocol, hzr-core, hzr-cli;
- общий ExecutionEnvelope/ToolResult/ResponsePipeline;
- compatibility rtk shim;
- config schema v1.

Exit gate: все существующие RTK команды проходят через один typed execution seam без изменения output.

### P2 — hzrd и engine supervision

- singleton daemon;
- root-bound workspace registry;
- ICM warm adapter;
- hzr-indexd lifecycle;
- health/version/capability negotiation;
- direct-mode fallback.

Exit gate: один ICM service и один index watcher на workspace; wrong-root test блокирует запрос.

### P3 — Unified retrieval и budget

- убрать повторный rgai/pre-read;
- AllowedPaths/BatchSearch patch;
- RRF + MMR;
- один tokenizer-aware packer;
- exact reread before edit;
- unified context/memory provenance.

Exit gate: recall@20 ≥95%, hard budget не нарушается молча, end-to-end trace полный.

### P4 — Caveman shadow и safe mode

- импортировать/портировать safe rules с notices;
- protected spans;
- output contract compiler;
- schema/tool description compaction;
- shadow evaluation и ablation.

Exit gate: invariants 100% на exact corpus; quality non-inferiority на paired set.

### P5 — Adaptive request/tool compression

- economic gate;
- stable prefix;
- history dedupe;
- semantic compact только после deterministic filter;
- circuit breakers.

Exit gate: median actual cost −20% и p90 regression guard выполнен.

### P6 — HZR Gateway и response economics

- streaming OpenAI/Anthropic adapters;
- provider usage ingestion;
- output budgets/verbosity controls;
- compact derived response для следующего turn;
- failover/bypass.

Exit gate: gateway protocol parity, no stream corruption, actual ledger coverage ≥95%.

### P7 — GA и product migration

- 200+ paired tasks;
- security/privacy audit;
- rollback drill;
- team deployment reference;
- docs/installer/SBOM/licenses;
- RTK brand переводится в compatibility namespace.

Exit gate: HZR 1.0 goals и guardrails выполнены.

## 26. Functional requirements

| ID | Requirement | Priority |
|---|---|---|
| HZR-F001 | Один hzr CLI управляет всеми engines | Must |
| HZR-F002 | hzrd обеспечивает singleton lifecycle и direct fallback | Must |
| HZR-F003 | Все hook events используют versioned typed protocol | Must |
| HZR-F004 | Один hierarchical token budget применяется к final serialized request | Must |
| HZR-F005 | grepai является единственным semantic code index owner | Must |
| HZR-F006 | ICM хранит long-term memory с provenance/invalidation | Must |
| HZR-F007 | RTK filters работают через единый ResponsePipeline | Must |
| HZR-F008 | Caveman codec respects fidelity/protected spans | Must |
| HZR-F009 | Raw recovery доступен по trace ID согласно privacy policy | Must |
| HZR-F010 | Actual provider usage отделён от estimates | Must |
| HZR-F011 | rtk и rgai compatibility shims поддерживаются | Must |
| HZR-F012 | Migration plan/apply/verify/rollback idempotent | Must |
| HZR-F013 | Capability report честно показывает доступный interception | Must |
| HZR-F014 | Retrieval использует router, RRF, diversity и exact reread | Must |
| HZR-F015 | Codec economic gate учитывает overhead и retries | Must |
| HZR-F016 | Telemetry/raw persistence являются opt-in | Must |
| HZR-F017 | Gateway сохраняет streaming и provider errors | Should |
| HZR-F018 | Team mode поддерживает remote vector/metadata stores | Should |
| HZR-F019 | hzr explain показывает provenance и policy decisions | Should |
| HZR-F020 | Paired benchmark и ablation запускаются одной командой | Must |

## 27. Non-functional requirements

| ID | Requirement |
|---|---|
| HZR-N001 | Rust unsafe запрещён без отдельного ADR/security review |
| HZR-N002 | Protocol backward compatibility минимум два stable cycles |
| HZR-N003 | Engine crash не повреждает canonical data |
| HZR-N004 | Все writes idempotent или transactionally protected |
| HZR-N005 | Secrets отсутствуют в default logs/telemetry |
| HZR-N006 | Exact data сохраняет required bytes/entities на 100% corpus |
| HZR-N007 | Warm local orchestration p95 ≤250 мс сверх model/tool work |
| HZR-N008 | Offline source retrieval остаётся доступным без cloud |
| HZR-N009 | Index freshness видима и проверяется перед edit |
| HZR-N010 | Каждая трансформация versioned и attributable |

## 28. Acceptance criteria

HZR 1.0 разрешён к выпуску, когда одновременно:

1. cargo fmt, clippy и all-target tests зелёные;
2. legacy RTK golden parity пройдена;
3. rewrite verdict не кодируется shell exit status;
4. нет duplicate watcher/index на canonical workspace;
5. один context plan укладывается в реальный hard budget;
6. exact corpus сохраняет commands, code, JSON, errors, identifiers и exit semantics;
7. paired benchmark ≥200 tasks показывает median billed cost не хуже −30%;
8. нижняя 95% CI task success delta ≥−1 п.п.;
9. p90 cost regression ≤+5%;
10. retry rate delta ≤+2 п.п.;
11. recall@20 ≥95%;
12. raw fallback и HZR_BYPASS протестированы;
13. provider usage coverage ≥95% для gateway runs;
14. migration rollback восстанавливает прежние binary/hooks/data;
15. privacy/security review закрывает P0/P1 issues;
16. SBOM и license notices присутствуют;
17. crash/restart/rebuild drills проходят без data loss;
18. capability matrix отображается в hzr doctor.

## 29. Риски и mitigation

| Риск | Вероятность | Влияние | Mitigation |
|---|---|---|---|
| Cascading lossy compression | высокая без архитектуры | critical | один compiler, fidelity classes, raw fallback |
| Caveman overhead превышает savings | высокая на коротких turns | medium | economic gate, stable short contract |
| Prompt cache ломается от dynamic rewrite | средняя | high | cache-stable prefix, delta-only compile |
| Semantic retrieval теряет exact symbol | средняя | high | router и exact verification |
| Wrong workspace index | наблюдается сейчас | critical | canonical ID, root handshake, one owner |
| Duplicate ICM/index services | наблюдается сейчас | high | hzrd supervision/singleton |
| Stale index перед edit | средняя | high | generation + reread exact lines |
| Local estimates создают ложный ROI | наблюдается сейчас | high | actual ledger, estimates label |
| Gateway меняет streaming/error semantics | средняя | high | protocol conformance corpus, bypass |
| Memory сохраняет prompt injection | средняя | high | trust labels, instruction/data boundary |
| Raw artifacts содержат secrets | средняя | critical | retention off, redaction, encryption |
| Upstream divergence | средняя | medium | pinned engines, minimal patch layer, sync cadence |
| Big-bang rewrite затянется | высокая | high | strangler migration и compatibility shim |

## 30. Decision log

| Decision | Статус | Причина |
|---|---|---|
| Новый бренд и продукт называется HZR | Locked | RTK становится engine/legacy namespace |
| Один продукт, multi-process internals допустимы | Locked | Go grepai sidecar сохраняет зрелость и upstream path |
| hzrd — единственный lifecycle owner | Locked | устраняет duplicate services/hooks |
| grepai — единственный semantic code index | Locked | устраняет двойной index и score confusion |
| ICM — long-term memory, IMG — workspace context | Locked | разные retention/invalidation/trust |
| Caveman — codec, не relevance engine | Locked | контроль качества и причинности |
| Raw intent не сжимается до retrieval | Locked | защита semantic recall |
| Response savings достигаются до generation | Locked | post-hoc compression не уменьшает bill |
| Actual billed cost per accepted task — primary metric | Locked | исключает misleading layer counters |
| rtk shim минимум два release cycles | Locked | безопасная миграция |
| Telemetry и raw retention opt-in | Locked | local-first privacy |
| Apache-2.0 umbrella с component notices | Proposed, legal review | совместимость с ICM obligations |

## 31. Первые implementation epics

1. **HZR-EPIC-00 Baseline Truth:** pipe/rewrite fixes, reproducible benchmark, actual usage importer.
2. **HZR-EPIC-01 Product Skeleton:** workspace, hzr CLI, hzrd, protocol, config.
3. **HZR-EPIC-02 Unified Exec:** ExecutionEnvelope, process adapter, ResponsePipeline, legacy command migration.
4. **HZR-EPIC-03 Engine Lifecycle:** ICM and grepai supervision, root identity, health.
5. **HZR-EPIC-04 Retrieval:** router, AllowedPaths, BatchSearch, RRF/MMR, freshness.
6. **HZR-EPIC-05 Budget:** tokenizer adapters, serialized prompt accounting, content refs.
7. **HZR-EPIC-06 Codec:** protected spans, safe Caveman contract, validators, shadow.
8. **HZR-EPIC-07 Gateway:** streaming provider adapters, output budget, usage.
9. **HZR-EPIC-08 Ledger:** trace, actual/estimated fields, outcome attribution.
10. **HZR-EPIC-09 Migration:** config/data/hooks, rtk shim, rollback.
11. **HZR-EPIC-10 Evaluation:** paired suite, ablations, non-inferiority gates.
12. **HZR-EPIC-11 Distribution:** installer, service units, SBOM, licenses.

## 32. Источники

### Локальный fork

- hooks/rtk-rewrite.sh:16 — nonzero rewrite result discarded.
- src/rewrite_cmd.rs:7-46 — documented rewrite/permission exit contract.
- hooks/rtk-mem-context.sh:62-232 — independent budgets, duplicate retrieval/pre-read and injected certainty.
- src/memory_layer/semantic_stage.rs:21-72 — recursive builtin rgai path.
- src/memory_layer/planner_graph.rs:358-372 — semantic stage and raw score fusion.
- src/memory_layer/budget.rs:50-165 — token estimates and minimum-one accounting.
- src/rgai_cmd.rs:78-126 — grepai delegation versus builtin paths.
- src/grepai.rs:317-370 — external grepai process and timeout.
- src/guard.rs:5-29 — size-oriented never-worse guard.
- src/tracking.rs:1026-1048 — chars/4 token estimation.
- src/tee.rs:34-69,102-166 — raw tee persistence.
- src/memory_layer/api.rs:379+ — current manual HTTP daemon prototype.

### Внешние

- [Caveman repository](https://github.com/JuliusBrussee/caveman)
- [Caveman Honest Numbers](https://github.com/JuliusBrussee/caveman/blob/main/docs/HONEST-NUMBERS.md)
- [Caveman evals](https://github.com/JuliusBrussee/caveman/tree/main/evals)
- [Independent Caveman evaluation](https://blog.jetbrains.com/ai/2026/07/speak-to-ai-agents-like-cavemen-tosave-tokens/)
- [Independent RTK evaluation](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/)
- [JetBrains Context repository intelligence](https://blog.jetbrains.com/ai/2026/07/introducing-jetbrains-context-repository-intelligence-for-coding-agents/)
- [grepai repository](https://github.com/yoanbernabeu/grepai)
- [grepai Excalidraw benchmark](https://yoanbernabeu.github.io/grepai/blog/benchmark-grepai-vs-grep-claude-code/)
- [ICM repository](https://github.com/rtk-ai/icm)

## 33. Итог

HZR должен заменить продуктовую идею «RTK fork с дополнительными hooks». Его ценность возникает из сквозной оптимизации: правильный контекст сокращает turns; RTK сокращает tool evidence; ICM не даёт повторять прошлые исследования; grepai находит код по intent; Caveman уменьшает допустимую natural-language оболочку и задаёт короткий output до генерации; общий ledger доказывает эффект.

Максимальная эффективность здесь достигается не максимальным сжатием каждого фрагмента. Она достигается минимальной полной стоимостью корректного результата, когда HZR умеет оставить данные нетронутыми именно там, где дальнейшее сокращение увеличит число ошибок и повторных запросов.
