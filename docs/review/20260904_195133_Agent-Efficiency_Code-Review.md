# Архитектурное ревью: реальная агентская эффективность HZR

Дата: 2026-09-04. База: `fbf204d48704160407e3f06035ac32b9cf0bc67a`, HZR 0.7.1. Внутренний инженерный отчёт на русском.
Анализ выполнен заново по исполняемому коду, тестам и свежим наблюдениям. Старые PRD не использовались.

## Вердикт

**HZR имеет полезные технологические активы, но сейчас не доказывает снижение полной стоимости агентских задач и не заслуживает общего статуса SoTA.** Оркестрация реализована частично корректно: централизованное владение движками, типизированные интерфейсы, workspace binding и memory namespaces — правильная основа. Однако сквозная работа нарушается на границах accounting, путей поиска, длительных команд и завершения процессов.

Считать отдельный фильтр экономичным недостаточно. Продукт должен доводить модель до достаточного точного контекста за минимальную суммарную стоимость. Полное чтение небольшого или тесно связанного файла может быть оптимальным действием; серия формально «эффективных» команд может стоить дороже.

Основной [новый подробный PRD](../prd/20260904_AGENT_EFFICIENCY.md) содержит точные места исправлений, условия приёмки и план проверки. После завершения свежего аудита пользователь одобрил реализацию P0 и P1; P2 отложены. Отдельная доработка UI разрешена пользователем и выполняется независимо.

## Связи компонентов

~~~mermaid
graph TD
    Host[Claude / Codex / managed agent] --> Routing[Host adapter / MCP / CLI]
    Routing --> Daemon[hzrd: workspace + permissions + operation lifecycle]
    Daemon --> RTK[HZR RTK fork]
    Daemon --> Search[grepai: canonical index + graph]
    Daemon --> Memory[ICM: supervised process + scoped memory]
    Daemon --> Codec[Caveman codec / managed bridge]
    RTK --> Receipt[Producer receipts]
    Receipt --> Sweep[Accounting reconciliation]
    Sweep --> Ledger[Ledger]
    Search --> Planner[Context planner]
    Memory --> Planner
    RTK --> Planner
    Planner --> Delivery[Evidence delivery]
    Codec --> Delivery
    Delivery --> Host
    Host -. observed delivery and usage .-> Ledger
    Ledger --> UI[Visualizer / stats]
~~~

Сплошные стрелки показывают существующие логические связи, а не гарантию их полной надёжности. Прерывистая стрелка — необходимое, но неполностью наблюдаемое подтверждение фактической доставки и provider usage. UI не может восполнить отсутствующие данные.

| Связь | Сильная сторона | Проверенная проблема / граница |
| --- | --- | --- |
| CLI/MCP → hzrd | Единая политика, binding, явные ошибки | Не все CLI producer paths сохраняют полный accounting lifecycle; MCP не хватает длительных операций и полноты read |
| hzrd → RTK fork | Контролируемый fork с provenance и regression gates | Direct passthrough теряет context handle; test wrapper может скрыть причину падения |
| hzrd → grepai | Канонический индекс, управляемый watcher, реальный semantic lookup | Scoped semantic paths дублируют prefix; graph plan использует другую базу путей |
| hzrd → ICM | Центральный supervision, FTS5, namespace isolation | Старые smoke children осиротели; recovery `memory show` отсутствует в CLI |
| hzrd → Caveman | Bridge и защищённый derivative transform | Нет доказанной сквозной host replacement; текущий codec преимущественно удаляет дубликаты |
| engines → ledger → UI | Раздельные estimate/provider поля | Orphan receipts, потеря классификации exec, неполный observer; `healthy=true` не означает готовность экономики |

Подробная проверка владения, протоколов, состояния, ошибок и shutdown на каждой границе — в [component integration audit](20260904_195133_Component-Integration.md).

## Основные дефекты

| Приоритет | Наблюдение | Где исправлять | Условие приёмки |
| --- | --- | --- | --- |
| P0 | Direct read/write/test создают receipts без гарантированного registry context | `crates/hzr-cli/src/fork.rs:9-49`, `crates/hzr-exec/src/adapter.rs:523-550`, `crates/hzr-daemon/src/accounting_sweeper.rs:29-81` | Каждый producer имеет durable operation ID; crash/replay не теряет и не дублирует receipt |
| P0 | `exec run` ограничен 29.5 s, большой timeout отвергается | `crates/hzr-daemon/src/api.rs:3681-3725`, `server.rs:24,80-85` | 90-second job завершается через handle/wait/cancel без повторного запуска |
| P0 | Неверные пути реального semantic/graph результата | `crates/hzr-context/src/planner.rs:1036-1103,397-417,580-619` | Все returned paths проверяемы для root/nested/absolute scope и каждого engine |
| P0 для заявлений | 9.565% — локальная оценка, а не итоговая стоимость модели | `crates/hzr-core/src/ledger.rs`, `crates/hzr-cli/src/stats_output.rs` | Paired accepted tasks, реальные receipts, полная цена retries/overhead |
| P1 | Read/Grep deny заставляет модель повторять намерение | `crates/hzr-cli/src/hook_runner.rs:601-632`; policy registry | Оптимизация не добавляет deny/retry для разрешённой операции; native permission сохраняется |
| P1 | Живой daemon и предупреждения дают общий healthy | `crates/hzr-cli/src/diagnostics.rs:1969-1972`; process lifecycle | Отдельные liveness/readiness/retrieval/accounting/host coverage |
| P1 | MCP schema разрешает empty content, parser отвергает; не хватает batch/expand/lifecycle | `crates/hzr-cli/src/mcp.rs:1196-1226`, `crates/hzr-cli/src/mcp/arguments.rs:75-79` | Round-trip тесты реального parser и schema; typed completeness |
| P1 | Полный gate падает при запуске через сам HZR | `fork-core/rtk/tests/internal_env.rs:23-46`, `scripts/verify-fork-core.sh:261-306` | Раздельные standalone/managed fixtures и зелёный complete gate через штатный маршрут |

Номера строк относятся к исходной версии до UI-изменений. Развёрнутые доказательства, оговорки и исправления F01–F16 — в PRD. Ссылки на конкретные MCP-модули дополнительно проверяются в независимом [MCP-аудите](20260904_195133_MCP-Audit.md).

## Что действительно показывают семь дней

[Fleet-аудит](20260904_195133_Fleet-Usage-7d.md) проверил 94 зарегистрированных пути: 85 существуют, 9 удалены. В 83 существующих нет наблюдений. Активные HZR и anonymous_bot дают в сумме 660 measured / 688 observed operations.

- Глобально: 867654 → 784661 estimated tokens, net +82993, 9.565%.
- HZR: +80099, 12.573%; anonymous_bot: +2894, 1.255%.
- 71 exact search: 4866 → 25480, net −20614. Метка optimized не гарантирует меньший вывод.
- 38 raw read_tail дают 370138 estimated tokens, 47.172% всего доставленного вывода.
- 453 raw exec скрыты за family `other`: по этому срезу нельзя обоснованно выбрать конкретные shell-команды для доработки RTK.
- После существующей эвристики повторов anonymous_bot получает −21129. Это сигнал для исследования, а не доказанный вред фильтрации.
- Recorded model tasks, accepted outcomes и provider receipts: ноль. Это отсутствие наблюдений, не нулевое качество или потребление.

Вывод: положительный локальный сигнал есть, но переносимость пользы почти не проверена. Нельзя ужесточать запреты на полный контекст на основании этих агрегатов.

## Как выбирать эффективный путь

1. Оценивать всю потребность в доказательствах: навигация, чтение, расширения, исправление ошибочного поиска, дальнейшее использование.
2. Для точного известного символа возвращать совпадение и достаточный контекст за один запрос.
3. Для архитектурного исследования использовать graph/semantic discovery только если реальные ссылки и качество оправдывают его стоимость.
4. При нескольких расширениях одного файла сравнивать накопленную стоимость с единым full read; объединять пересекающиеся диапазоны.
5. Разрешать exact/full read как нормальный экономический выбор, сохраняя обычные ограничения доступа.
6. Указывать source hash, точные диапазоны, полноту и пригодную recovery-команду. Не заставлять модель угадывать, что скрыто.
7. Фильтровать внутри инструмента до доставки модели; дополнительный LLM-call ради «сжатия» сам нуждается в экономическом обосновании.

Микропроверка одинакового 299-строчного файла: полный диапазон 11000 bytes, три диапазона 11272 bytes и два лишних вызова. Это небольшой, но непосредственно наблюдаемый пример, почему больше мелких чтений не всегда лучше. Не экстраполируем 2.47% на все задачи.

## Оценка архитектурной зрелости

Баллы — экспертная шкала для приоритизации этого ревью, а не измеренный benchmark или оценка производительности. Общий балл не рассчитывается: критические разрывы нельзя компенсировать хорошим средним.

| Категория | Балл / 100 | Обоснование |
| --- | ---: | --- |
| Code Quality | 69 | Много типизированного Rust и тестов; parser/schema drift и потеря path provenance |
| Extensibility / Modularity | 73 | Движки отделены и имеют provenance; host assumptions и routing растянуты по поверхностям |
| Security | 72 | Workspace binding, CAS, namespaces и fail-closed paths полезны; полной security-проверки не было |
| Optimization / Performance | 38 | Есть локальное сокращение; нет task/provider proof, встречаются повторные чтения и шумные diagnostics |
| Architecture & Visualization | 57 | Control plane сформирован; end-to-end ownership/accounting/health недостаточно согласованы |
| Deploy Cleanliness | 51 | Проверяемые bundle identities; orphan smoke processes и красный complete gate |

## Очерёдность

Сначала устранить потери receipts, ошибки путей, лимит исполнения и потерю диагностики. Затем дать LLM полноценные операции read/expand/batch и актуальные host adapters. После этого измерить реальные задачи с native baseline, RTK-only, +grepai, +ICM и полной системой.

Не следует начинать с новых запретов и очередного слоя декларативных инструкций. Инструмент должен выдавать более полезный ответ за меньшую полную стоимость, чтобы у модели не возникало практической причины обходить его.

Проверки: [Verification](20260904_195133_Verification.md). Производительность и масштабирование: [perf-report](../issues/20260904_195133_perf-report.md). UI: [отдельный дизайн-отчёт](20260904_UI-Design.md).

