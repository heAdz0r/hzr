# Связность компонентов HZR: новый независимый аудит

Дата: 2026-09-04. Текущий код и runtime HZR 0.7.1. Старые PRD не использовались. Проверка read-only; процессы не останавливались, настройки и хранилища не менялись.

## Вердикт

Компоненты действительно связаны общим daemon, типизированными API, namespace и управляемыми процессами; это не просто набор alias. Особенно хорошо ограничен собственный managed-agent Caveman harness: его независимые memory/repomap/snapshot механизмы отключены, а набор tools проверяется перед вызовами.

Однако сквозная оркестрация не считается корректной целиком. Прямой CLI fork путь теряет accounting context; ICM recovery не заменяет живой, но неработающий процесс; endpoint ICM не переживает daemon restart; штатное Unix завершение SIGTERM не подключено к graceful shutdown. Runtime ready проверяет доступность отдельных поверхностей, а не полезность retrieval, полноту receipts или доставку сокращенного ответа модели.

## Живой статус

Вызов `hzr daemon status --json` во время проверки, до отметки 2026-09-04 20:06:10 UTC:

| Элемент | Статус | Точный смысл |
|---|---|---|
| hzrd | ready, protocol 1, 0.7.1 | Результат агрегации health |
| grepai | stopped, 0.35.0 | 0 active watcher(s), 0 failed, limit 8, idle TTL 900000ms |
| ICM | ready, 0.10.61 | ICM singleton is ready with FTS5 retrieval; embeddings are disabled |
| RTK fork | ready, 0.44.1-fork.1 | ForkCli |
| caveman-code | stopped, 0.65.2 | managed agent runtime is launched by hzr agent |

Stopped для on-demand engines не является ошибкой. ICM сейчас использует FTS5; semantic memory retrieval не включен, поэтому его качество нельзя описывать как проверенное hybrid retrieval.

`hzr engines status --json` возвращает pinned manifest: grepai с patch отключения автоматического обнаружения worktree, ICM 0.10.61, RTK immutable import provenance, Caveman 1.9.1, caveman-code 0.65.2 и Node 22.17.1. Это декларация компонентов, не успешная end-to-end проверка каждого пути.

## Матрица взаимодействий

| Связь | Владелец и протокол | Запрос и учет | Ошибки и fallback | Shutdown и health |
|---|---|---|---|---|
| Host CLI/MCP → hzrd | Один daemon/data-root lock; authenticated loopback HTTP; typed hzr-protocol | Workspace/caller attribution, policy, accounting и observability API | Типизированные ошибки; host interception и native coverage зависят от harness | API ready не доказывает installed observer и delivery receipt |
| hzrd → ICM | AppState владеет IcmSupervisor; один DB layout, process lock; HTTP JSON + bearer | /health, /stats?format=json, /recall?format=json и mutations; namespace на HZR boundary | Circuit breaker; daemon устанавливает cli_fallback=false, поэтому нет скрытого CLI store | Owned/Attached разделены; alive-unready не рестартуется; graceful path есть, SIGTERM не привязан |
| hzrd → grepai watcher | IndexCoordinator/IndexOwner на canonical workspace; pinned subprocess CLI | Index config, vectors, symbols, graph; fork использует тот же workspace index | Single-index guard, multi-worktree capability check, bounded initialization; explicit degradation | Watch stop с deadline/kill fallback; idle reaper; ready registry показывает процессы, не relevance |
| hzrd → RTK fork | PinnedRtkAdapter/ForkCoreRunner; argv + typed invocation/result + receipt contract | Policy execution, read/write/search; shared runtime paths, receipt correlation | Ограничение процессов, typed fallback metadata; нужны проверенные semantic path conventions | Daemon managed calls дренируют receipts; direct CLI passthrough контекст теряет |
| ContextPlanner → RTK + ICM | Один planner в AppState; tokio::join для code и memory | Fork memory plan / rgai JSON + ICM recall; fuse с hard token limit | Code/memory ошибки становятся source warnings, один источник может дать полезный частичный ответ | Partial response допустим, но не должен рекламироваться как полный context |
| hzr agent → caveman-code → hzrd | Node bridge owned by HZR runner; newline JSON events, HZR tool APIs | Native tools blocked; repomap/memory/autosnapshot отключены; provider usage outbox | Preflight pins/contracts, tool guards, bounded capture, timeout; durable usage replay | Process-group cleanup и kill_on_drop; daemon health проверяет наличие runtime, не model-run |
| hzrd codec → host delivery | In-process hzr-codec; результат typed Transform | Exact protection, structural paragraph dedup, shadow estimate; ledger stage separate | High-risk/exact pass-through; нет универсального host response replacement | Нет отдельного сервиса; proof host-delivery отсутствует в external harness |

## Что уже реализовано правильно

### 1. Один управляемый набор зависимостей

`crates/hzr-daemon/src/state.rs:56-85` создает ledger, ICM supervisor и pinned RTK runner и передает их одному ContextPlanner. Это явная зависимость, а не поиск произвольного бинарника каждым адаптером. `server.rs:111-140` держит daemon lock, запускает accounting sweeper и содержит общий shutdown.

### 2. ICM работает через реальный typed service

`state.rs:168-188` принудительно выбирает HTTP и отключает CLI fallback. `hzr-memory/src/http_transport.rs:58-83` применяет bearer token и JSON; 86-125 ограничивают чтение ответа 8 MiB. `client.rs:112-128` проверяет не только /health status=ok, но также /stats и базовые числовые инварианты. `client.rs:144-178` содержит circuit breaker.

`namespace.rs:104-145` использует положительную проверку принадлежности namespace и отбрасывает чужие и ненормализованные records. По read-only исходному анализу нет оснований утверждать, что обычный daemon recall запускает отдельный постоянный ICM или пишет в fallback store.

### 3. grepai имеет централизованный lifecycle

`hzr-index/src/grepai.rs:168-173` требует single index; 223-242 проверяют topology и поддержку single-worktree watcher. `watch.rs:24-84` закрепляет cwd и runtime directory и держит owner handle. `watch.rs:134-166` начинает ограниченное завершение watcher. `state.rs:87-120` содержит idle reaper; `coordinator.rs:251-261` закрывает watchers и workspace cache.

Это сильнее, чем просто «процесс grepai существует». Но оно не гарантирует, что scoped path и строки найденного документа правильно согласованы между внешним API, RTK и самим grepai.

### 4. ContextPlanner действительно объединяет источники

`hzr-context/src/planner.rs:259-328`: code и ICM вызываются параллельно; результаты нормализуются отдельно; ошибки отражаются warnings; fuse имеет hard limit. `planner.rs:377-398` оставляет structural evidence, если canonical semantic lifecycle недоступен. `planner.rs:621-644` вызывает существующий RTK memory plan в JSON, а 650-720 — rgai JSON с explicit strategy/fallback.

Осторожность: context plan всегда запрашивает ICM, даже если цель могла решаться одним exact read. Его token limit ограничивает доставку, но не доказывает выгодность всей цепочки. Plan recall использует project-only (`planner.rs:275-282, 305-310`), в то время как обычный recall может объединять project+global (`namespace.rs:25-29`). Эту разницу нужно явно отражать в contract/capabilities.

### 5. Managed Caveman предотвращает дублирующие механизмы

`integrations/caveman-code/bridge.mjs:780-787` выключает repomap, memory и autoSnapshot. 789-821 проверяют фактические flags и точный tools set; 823-845 устанавливают beforeToolCall guard и блокируют native execution. Это реальная защита от параллельной памяти и скрытого tool path внутри данного harness.

`hzr-agent/src/runner.rs:96-110` передает daemon endpoint/token, задает cwd, запускает process group; 113-132 ограничивают capture и исполнение. `bridge.mjs:871-919` обеспечивает bounded durable usage outbox/replay; 944-955 раздельно переносит actual input/output/cache поля. Эти свойства относятся к `hzr agent`, их нельзя автоматически переносить на внешний Codex/Claude harness.

## Дефекты связи и их исправление

### INT-1 — P0: direct CLI fork выполняет полезную работу вне полного accounting lifecycle

`crates/hzr-cli/src/fork.rs:29-40` получает `std_command_os` и делает Unix exec. `crates/hzr-exec/src/adapter.rs:523-550` создает accounting handle, но `std_command_os` отбрасывает его через `.map(|(command, _)| command)`. `hzr-daemon/src/accounting_sweeper.rs:31-48` сканирует зарегистрированные context files; наличие одного receipt-файла не обеспечивает восстановление attribution.

Следовательно, одинаковый underlying RTK фильтр через managed daemon и direct `hzr read/test/rtk` имеет разную полноту учета. Это дефект coupling, а не отсутствие фильтра.

Исправление: один typed execution-registration protocol до spawn/exec; persisted correlation, workspace/host/session, receipt destination и execution state; daemon reconciles даже если CLI заменен exec или аварийно завершен. Acceptance: identical deterministic fixtures через CLI, MCP, hooks, managed agent дают один logical receipt с одинаковыми dimensions; никакого double credit на replay.

### INT-2 — P1: живой зависший ICM не восстанавливается supervisor

`state.rs:253-285` при Unready делает backoff и возвращается к start. Но `supervisor.rs:113-125` при Owned+alive снова вызывает readiness и возвращает Err; stop/restart не выполняется. Умерший child корректно заменяется на 113-121, но alive-unhealthy может оставаться в бесконечном degraded/backoff.

Исправление: bounded health-failure threshold; terminate/restart только Owned с проверкой identity, budget и backoff. Attached нельзя убивать. Acceptance: искусственно зависший, но живой owned ICM восстанавливается; transient timeout не вызывает restart storm; attached failure сообщает operator action.

Это source-proven failure path; production kill/restart тест не проводился.

### INT-3 — P0: endpoint/ownership ICM не сохраняются между аварийными daemon lifecycle

`state.rs:168-180` выбирает новый ephemeral port на каждом startup и закрывает reserve listener до spawn. `supervisor.rs:145-153` при существующем PID file проверяет readiness текущего endpoint; прежний endpoint из PID file не восстанавливается. После daemon termination surviving old ICM не будет обнаружен такой проверкой, если порт сменился.

Управляющий lock хранится в supervisor, а не как durable verified process identity. Совместно с отсутствием SIGTERM graceful handler это создает риск нового long-lived процесса рядом со старым ICM для того же data root. Само наличие SQLite concurrency не делает такой lifecycle корректным.

Исправление: durable runtime manifest с PID, process-start identity, executable digest, DB identity, endpoint и owner generation; authenticated ownership handshake; отсутствие indiscriminate attach/kill. Либо связать child lifetime с owner на поддерживаемых ОС и надежно проверять остальное при старте. Ephemeral port reserve также имеет TOCTOU между drop и bind; ошибка должна вести к bounded new allocation, не вечному reuse занятого порта.

Acceptance: SIGTERM/crash/restart fixture не создает второго owner того же store, stale PID и reused PID не вызывают terminate чужого процесса.

### INT-4 — P0: graceful shutdown не соответствует Unix service lifecycle

`crates/hzr-daemon/src/main.rs:8` ожидает только `tokio::signal::ctrl_c()`. `server.rs:131-140` корректно останавливает ICM/index и sweeper, но этот путь зависит от полученного shutdown future. SIGTERM от service manager не связан с ним.

Исправление: SIGINT+SIGTERM на Unix, платформенный эквивалент на Windows, cancel in-flight с bounded grace, final receipt drain и owned child cleanup. Acceptance: shutdown signals и forced timeout отдельно; не полагаться только на Rust Drop при terminating signal.

### INT-5 — P1: health смешивает разные уровни доказательств

`api.rs:76-94`: RTK ready зависит от обнаруженного rewrite interface ForkCli. `api.rs:115-146`: grepai health основан на watcher registry. `api.rs:190-218`: Caveman health проверяет наличие bridge/package paths и сообщает Stopped. `api.rs:158-171` игнорирует on-demand stopped, что правильно, но итог ready не означает успешную связную retrieval/execution/delivery цепочку.

Исправление: независимые capability states installation_verified, process_ready, request_probe_passed, semantic_ready, accounting_complete, delivery_confirmed, task_evidence_available. Не добавлять на каждый health дорогое чтение store; кэшировать bounded read-only probes с timestamp/TTL.

Acceptance: malformed semantic result, scoped-path mismatch, missing receipt context, incompatible bridge и alive-unready ICM дают собственные actionable состояния; healthy process не скрывает broken edge.

### INT-6 — P1: Caveman codec и managed agent — разные интеграции

`hzr-codec/src/lib.rs:230-266` — структурная трансформация с удалением одинаковых абзацев. `hzr-daemon/src/api.rs:3499-3510` вызывает этот in-process codec; observability помечает engine=caveman на 3517, хотя внешняя Caveman runtime здесь не запускается. Это корректная реализация narrow codec, но название компонента не доказывает полную интеграцию исходного Caveman поведения.

`api.rs:3563-3605` измеряет compile/tool payload, а не факт замены будущего финального ответа внешнего host. Комментарий на 3560 прямо отделяет working transform от accounting failure. В host-managed режиме возврат content и его фактическая доставка — разные события.

Исправление: именовать structural codec честно; separate component identity/profile/version. Единственная цепочка transform_candidate → fidelity_validation → host_delivery_ack → accounting. Shadow и compile-only дают ноль host savings. Общую экономику считать по всему episode, включая полный исходный текст, переданный в codec.

## Что требуется от связанной архитектуры SoTA

- Один logical operation ID через host → hzrd → engine → receipt → delivery.
- Typed artifact identity и revision по всем retrieval источникам; path normalization в одном слое, результаты scoped query переводятся в canonical workspace coordinates ровно один раз.
- Явные modes supported / fallback / unavailable и цена планируемой цепочки, а не оптимальность по имени engine.
- Одно authoritative memory store и namespace enforcement, одно владение индексом на worktree, проверенный process lifecycle.
- Typed long-running execution handle, poll/cancel/selective output; нельзя сводить весь build/test к короткому request deadline.
- Bounded integration fixtures с failures на каждом переходе: daemon restart, ICM hung, grepai stale, scoped paths, RTK receipt failure, bridge protocol mismatch, host dropped delivery.
- Полезность доказать task outcomes и сравнением total context+retries+latency, а не только здравием компонентов.

## Проверка и ограничения

Выполнены runtime `hzr daemon status --json`, `hzr engines status --json`, bounded чтение актуального исходного кода. Не запускались mutation probes, новый suite, benchmark с моделями, установка или перезапуск процессов. Full gate находится в основном аудите.

Существование тестов в исходниках не приравнивается к их успешному прохождению. Утверждения про ICM hung/restart и SIGTERM — вывод из исполняемого control flow; отказоустойчивость на production процессах в этом подаудите не испытывалась. Ранее собранные основным аудитом orphan-process evidence и semantic scoped-path воспроизведения следует приложить к общему PRD отдельно.
