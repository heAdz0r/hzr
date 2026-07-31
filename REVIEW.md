# HZR 0.1.0 — независимый review реализации и качества

> Исторический независимый review `v0.1.0`. Все G1–G7 из этого документа исправлены в development line 0.2.0; findings не переписаны задним числом. Реализация добавила regression tests и отдельную current-engine identity поверх неизменяемого `v0.1.0` baseline.

**Дата review:** 2026-07-31
**Reviewer:** независимая проверка против [PRD.md](PRD.md) и [FORK_PARITY.md](FORK_PARITY.md)
**Метод:** запуск всех gate самостоятельно + end-to-end прогон продукта из чистого data root, а не чтение отчётов
**Вердикт:** архитектурно проект реализован так, как спроектирован. Найдено **1 подтверждённое нарушение контракта PRD** и **2 значимых дефекта целостности gate**, не отражённых ни в `FORK_PARITY.md`, ни в отчёте реализующего агента

---

## 1. Итог одной таблицей

| Область | Статус | Основание |
|---|---|---|
| Fork-core contract (§1.1) | ✅ подтверждён | snapshot v2 верифицирован, 516 файлов, hash совпал |
| Архитектурные ownership boundaries (§5.1) | ✅ подтверждён | end-to-end прогон, ни одной reimplementation в runtime-пути |
| CLI + daemon surface (§6.8) | ✅ 15/15 команд, 12/12 routes | точное совпадение с PRD |
| Version/supply-chain pins (§8) | ✅ подтверждён | `engines.lock.toml` = §8 буквально |
| Security/privacy (§9) | ✅ подтверждён | проверено на живом процессе |
| Failure modes (§10) | ✅ подтверждён | fail-closed воспроизведён |
| Rust quality gates (§12.1) | ✅ зелёные | 160 passed, 0 failed |
| Codec (§6.6) | ❌ **нарушение контракта** | `shadow` мутирует контент — G1 |
| Protected spans (§6.6) | 🟡 слабее заявленного | 4 из заявленных классов не покрыты — G2 |
| Fork regression gate (§13) | 🟡 **недетерминирован** | race в fork-тесте — G3 |
| Economic KPI (§4.2) | ⚪ не измерено 0/9 | PRD это признаёт честно |

**Главный вывод:** «работает ли как спроектировано» — да, ядро работает именно так. Экономическая часть PRD (§4.2) остаётся полностью недоказанной, и это в PRD признано прямо. Но три технических пункта заявлены как готовые, а в реальности не готовы.

---

## 2. Что я подтвердил самостоятельно

Ниже — только то, что я воспроизвёл сам, а не принял из отчёта.

### 2.1 Quality gates и fork-core

```
cargo fmt --all --check                     → exit 0
cargo clippy --workspace ... -D warnings     → exit 0
cargo test --workspace --all-targets         → 160 passed; 0 failed
scripts/verify-fork-core.sh                  → snapshot v2 f4296ec4... verified
```

Проверено дополнительно:

- `find fork-core/rtk -type f -o -type l` → ровно **516** файлов, совпадает с `included_file_count = 516` в [SNAPSHOT.toml](fork-core/SNAPSHOT.toml);
- в `crates/` **ноль** `#[allow(clippy::unwrap_used)]` / `panic` / `todo!` / `unimplemented!` — workspace lints (`unwrap_used = deny`, `panic = deny`, `unsafe_code = forbid`) не обойдены локальными escape-hatch;
- CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) покрывает все §12.1 gates плюс `verify-fork-core.sh --test`, MSRV 1.85, `npm audit --omit=dev`, `bash -n`, и реально применяет grepai-патч к pinned commit и гоняет `go test ./cli`. Это сильнее, чем требует PRD.

### 2.2 Product surface — точное совпадение с PRD §6.8

`hzr --help` даёт ровно 15 команд из PRD: `init, doctor, daemon, engines, index, search, rgai, context, memory, exec, codec, agent, savings, migrate, rtk`.

[server.rs:22-33](crates/hzr-daemon/src/server.rs:22) даёт ровно 12 routes из PRD, без лишних и без недостающих. Расхождений с §6.8 нет ни в одну сторону.

`engines.lock.toml` совпадает с таблицей §8 буквально по всем 6 компонентам, включая `rtk-upstream-reference` с `runtime = false` и `source_kind = "reference-only"` — то есть запрет «stock RTK как fallback» выражен в машинно-проверяемой форме, а не только в прозе.

### 2.3 End-to-end прогон из чистого data root

Поднял `hzrd` с изолированным `HOME` и отдельным git-workspace. Работает как спроектировано:

**`hzr init`** → создал canonical layout (`ledger/`, `memory/icm/`, `runtime/`, `workspaces/`, `engines/`, `config.toml`); `fork/` создаётся лениво при первом обращении. `hzrd.token` — права `0600`.

**`hzr doctor`** — не косметический. Обнаружил **реальный дрейф на машине**: `engine_icm: expected 0.10.61, got icm 0.10.57`, и корректно распознал `ForeignSymlink` для `.grepai`, указывающего в другой data root. Возвращает **exit 1** при `healthy: false` — то есть годен как скриптовый gate.

**`hzr exec rewrite`** — fork-окружение инжектируется точно по §9:

```
RTK_TEE=0
RTK_TELEMETRY_DISABLED=1
RTK_MEM_DB_PATH=<data>/fork/mem.db
RTK_DB_PATH=<data>/fork/history.db
RTK_TEE_DIR=<data>/fork/tee    RTK_AUDIT_DIR=<data>/fork/audit
PATH=<private engines>:$PATH
```

Вердикт `ask:` для `cat main.rs` — то есть approval-lifecycle (exit 3) живой, и предложенная команда не исполняется без явного решения.

**`hzr search budget`** → `main.rs [ForkRgaiAdaptive score=0.6139]` + `generation=68f4b53a...`. Поиск идёт через fork `rgai`, generation-хэш присутствует.

**`hzr context plan`** — самый показательный результат:

```
context selected=1 rejected=0 tokens=180/16000 coverage=0.25 confidence=0.50
fork-plan pipeline=graph_first_v1 candidates=1/1 tokens=180/12000
warning MemoryUnavailable: ICM version mismatch: expected 0.10.61, found 0.10.57
```

Это буквально §5.2 + §10: fork IMG planner (`graph_first_v1`) отработал, hard budget промаркирован (`180/16000`), а недоступный ICM **деградировал с явным warning, не уронив code plan**. Ровно спроектированное поведение.

**`hzr savings`** → `tasks: 0, accepted: 0` и **не** выдаёт `cost_per_accepted_task`. §4.2 соблюдён честно: продукт не притворяется, что экономия доказана.

**Fail-closed без daemon** — `search`, `memory`, `codec` блокируются с actionable remediation; `hzr rtk -- --version` при этом продолжает работать (`rtk 0.44.1-fork.1`). Это в точности матрица §10.

### 2.4 Contract tests §12.2 — покрытие реальное

Имена тестов ложатся на §12.2 почти один-к-одному, это не декоративный suite:

| Требование §12.2 | Тест |
|---|---|
| estimates never increment actual | `test_ledger_keeps_estimates_out_of_actual_totals`, `test_usage_route_records_provider_tokens_separately_from_estimates` |
| token estimates ≤ hard limit | `test_plan_never_exceeds_hard_budget`, `test_fuse_returns_only_content_selected_within_hard_limit` |
| ICM singleton/stale PID/breaker | `test_supervisor_owns_one_process_and_second_instance_attaches`, `test_supervisor_recovers_orphan_without_spawning_duplicate`, `test_breaker_opens_at_threshold_and_recovers_half_open` |
| duplicate index detection | `test_workspace_uses_git_root_and_reports_nested_indexes`, `test_managed_discovery_blocks_foreign_symlink_without_mutation` |
| stdout/stderr/exit preservation | `test_pipeline_reports_exact_exit_and_stderr_channels`, `test_pipeline_reports_exact_unix_signal` |
| daemon auth/loopback/limits | `test_all_routes_require_bearer_authentication`, `test_config_rejects_non_loopback_bind`, `test_payload_limit_reserves_json_envelope_space` |
| project-scoped memory | `test_isolate_project_memories_removes_global_and_cross_repo_records`, `test_memory_route_rejects_user_supplied_project_override` |
| migration безопасность | 6 тестов `test_migrate_legacy_index_*`, включая idempotent replay и refuse-on-escaping-symlink |

Отдельно отмечу качество `hzr-exec` (40 тестов на 2843 LOC): shell-семантика проверена всерьёз — pipes, `&&/||`, heredoc, redirects, xargs, quoting, non-UTF8 argv, process-group timeout, spill на диск. Это самая сильная часть кодовой базы.

---

## 3. Найденные дефекты

### G1 — `shadow` профиль нарушает контракт PRD §6.6 (ВЫСОКИЙ)

PRD §6.6 утверждает:

> `shadow` records counterfactual size **without changing delivered content**.

Фактически `shadow` **изменяет доставляемый контент и ничего не записывает**.

Воспроизведение (вход — 3 абзаца, 1-й и 3-й идентичны):

```bash
hzr codec compile --profile shadow --json --file dup.txt
```

```json
{
  "content": "The budget is exhausted.\n\nSecond distinct paragraph here.",
  "changed": true,
  "profile": "shadow",
  "protected_spans": []
}
```

Третий абзац **удалён**, `changed: true`.

Причина — в [lib.rs:106](crates/hzr-codec/src/lib.rs:106) short-circuit покрывает только `FidelityClass::Exact` и `CodecProfile::Off`:

```rust
if fidelity == FidelityClass::Exact || profile == CodecProfile::Off {
```

`Shadow` в `transform()` не обрабатывается вовсе и проваливается в `deduplicate_paragraphs`. Во всём workspace `Shadow` встречается 4 раза, и ни один — не в `transform()`. Поиск по `counterfactual|shadow_size|shadow_bytes` даёт **ноль** совпадений: counterfactual-учёт не реализован.

Проскочило потому, что **ни один из 5 тестов codec не покрывает `Shadow`**.

Минимальный фикс — добавить `Shadow` в short-circuit и либо реализовать запись counterfactual-размера, либо убрать заявление из §6.6:

```rust
// было
if fidelity == FidelityClass::Exact || profile == CodecProfile::Off {
// нужно
if fidelity == FidelityClass::Exact
    || matches!(profile, CodecProfile::Off | CodecProfile::Shadow)
{
```

### G2 — protected spans не покрывают 4 заявленных класса (СРЕДНИЙ, латентный)

PRD §6.6 заявляет покрытие: «code fences, inline code, **paths**, URLs, flags, hashes, versions, **identifiers**, **enum-like values** and **structured payloads**».

Проверка:

```bash
hzr codec compile --json --file sp.txt
# вход: Edit src/main.rs and set MAX_RETRIES for handle_budget_overflow with {"k":1}.
```

```json
{ "changed": false, "protected_spans": [] }
```

**Ни одного** protected span. Не покрыты:

- **относительный путь** `src/main.rs` — регекс в [lib.rs:9](crates/hzr-codec/src/lib.rs:9) требует префикс `./`, `../` или `/`, поэтому обычная запись пути в прозе не защищена;
- **enum-like** `MAX_RETRIES`;
- **identifier** `handle_budget_overflow`;
- **structured payload** `{"k":1}`.

Сейчас это **латентный** риск: единственная трансформация работает на границе абзацев и физически не может испортить идентификатор внутри абзаца. Но gate §12.2 «protected spans survive codec byte-for-byte» доказан только на том подмножестве, которое ловит регекс. Любая будущая трансформация уровня предложения сразу получит незащищённые идентификаторы и пути.

### G3 — fork regression gate недетерминирован (СРЕДНИЙ, целостность gate)

`FORK_PARITY.md` помечает «Fork regression suite» как ✅, отчёт реализующего агента говорит «1699 passed, 1 ignored». Фактически в suite есть тест с race на общем состоянии.

Диагностика — детерминированная, не «мигание»:

```
cargo test --locked --bin rtk tracking::tests                    → FAILED 3/3 (10 passed; 1 failed)
cargo test --locked --bin rtk tracking::tests -- --test-threads=1 → ok      (11 passed)
```

Полный suite:

```
full-suite-run1..3: FAILED. 1698 passed; 1 failed; 1 ignored
```

При этом один из моих запусков `scripts/verify-fork-core.sh --test` **прошёл**, а другой — упал. То есть «1699 passed» — это удачный прогон, а не свойство gate.

Корень — [fork-core/rtk/src/tracking.rs:1334](fork-core/rtk/src/tracking.rs:1334):

```rust
timer.track("test cmd", "rtk test", "raw input data", "filtered");
let recent = tracker.get_recent(5).expect("Failed to get recent");
assert!(recent.iter().any(|r| r.rtk_cmd == "rtk test"));
```

Тест пишет запись в **общую** tracker-БД и читает окно только из **5** последних записей. Соседние `tracking::tests` параллельно пишут свои записи и вытесняют `rtk test` из окна. Изоляция `RTK_DB_PATH` не помогает — гонка внутри самого модуля тестов.

Это унаследованный дефект `heAdz0r/rtk`, а не привнесённый HZR. Но он делает release-gate §13 («проходит весь fork regression suite») недетерминированным. Фиксы по возрастанию правильности: `--test-threads=1` для модуля, per-test `RTK_DB_PATH`, либо уникальное имя команды + фильтр вместо окна из 5.

### G4 — «adaptive codec» в §4.1/§6.6 переобещан (НИЗКИЙ, точность документации)

`hzr-codec` — 261 LOC и 5 тестов. По существу это: селектор density-контракта (3 варианта) + удаление **точных дубликатов абзацев** + валидатор protected spans. Никакой иной компрессии текста нет.

Это не дефект реализации — §5.2 п.8 и `FORK_PARITY.md` описывают механику корректно (плотность задаётся контрактом до генерации, codec — отдельный explicit transform). Но формулировки §4.1 («адаптивный codec с protected spans») и §6.6 создают впечатление представленческого кодека, которого в 0.1.0 нет. Стоит привести §6.6 в соответствие с фактом.

Побочно: codec **срезает завершающий `\n`** (`"...here."` вместо `"...here.\n"`). Для `LosslessStructural` это спорно.

### G5 — инварианты Caveman-bridge проверяются текстовым поиском (НИЗКИЙ)

[hzr-agent/src/lib.rs:21](crates/hzr-agent/src/lib.rs:21) `test_bridge_contains_fail_closed_ownership_invariants` — это 23 ассерта вида `BRIDGE.contains("session.setRepomapEnabled(false)")` по исходнику `bridge.mjs`.

Проверяется **наличие строки в файле**, а не то, что вызов произошёл, и не то, что он произошёл **до первого prompt** — тогда как §6.7 требует именно «disabled **before first prompt** and rechecked throughout generation». Порядок вызовов таким тестом недоказуем; рефакторинг, который сохранит строку но переставит вызов после prompt, пройдёт gate.

Смягчающее: `tests/fake_bridge.rs` даёт реальный процессный прогон (timeout, termination дескендантов, JSONL-захват), и `installManagedToolGuard` + `beforeToolCall` дают runtime-защиту. Но именно порядок «до первого prompt» покрыт только текстово. `FORK_PARITY.md` помечает 🟡 за `cavemem --version` probe, но слабость метода проверки не отмечена.

### G6 — дублированный суффикс в сообщении об ошибке (КОСМЕТИКА)

```
hzr: failed to read daemon token <path>: No such file or directory (os error 2); run `hzr daemon serve`: No such file or directory (os error 2)
```

Причина ошибки напечатана дважды — лишний `anyhow` context над уже отформатированной ошибкой.

### G7 — несогласованные права lock-файла (ИНФО)

`hzrd.token` → `0600` ✅, `hzrd.lock` → `0600`, но `hzrd.token.lock` → `0644`. Секрета в lock-файле нет, так что это не уязвимость, но §9 («runtime secrets имеют private permissions») читается как единое правило для всего `runtime/`.

---

## 4. Оценка качества кода

**Сильные стороны**

- **Fail-closed по умолчанию, последовательно.** Version drift, foreign symlink, отсутствие daemon, недоступный ICM — каждый случай даёт типизированный отказ или явный warning, но никогда молчаливый fallback. Это самое ценное свойство проекта, и оно реально держится в рантайме, а не только в тестах.
- **Ownership boundaries соблюдены.** Я специально искал «упрощённую реимплементацию» в runtime-пути (это главный запрет §1.1 п.3) и не нашёл: search уходит в fork `rgai`, план — в fork `memory plan`, exec — в fork `rewrite`. `hzr-exec` действительно adapter, а не своя таблица rewrite.
- **Отделение actual от estimated** проведено на уровне протокола и БД, а не соглашением — и покрыто тестами с обеих сторон (ledger + route).
- **Migration-код заметно осторожнее среднего:** full-SHA backup, два immutable manifest, idempotent replay, отказ при escaping-symlink и активном писателе. Ни одной автоматической деструктивной операции.
- **Workspace lints** (`unsafe_code = forbid`, `unwrap_used/panic/todo = deny`) не обойдены ни одним локальным `allow`.

**Слабые места**

- **Перекос покрытия.** `hzr-exec` — 40 тестов на 2843 LOC; `hzr-codec` — 5 тестов на 261 LOC, и именно там нашлось нарушение контракта (G1). Тестами закрыты те места, где авторы ожидали риск, а не те, где заявлены самые сильные гарантии.
- **Проверка инвариантов внешнего JS-рантайма текстовым поиском** (G5) — самая хрупкая часть gate: она ломается бесшумно.
- **Документация местами опережает код** (G1, G2, G4). `FORK_PARITY.md` в целом честен и сам помечает 🟡/⚪ там, где границы есть, но три пункта помечены готовыми преждевременно.

**Экономическая часть.** Из 9 product metrics §4.2 измерено **0**. Это в PRD и `FORK_PARITY.md` признано прямо и последовательно (`hzr savings` не выдаёт `cost_per_accepted_task`, UI не показывает прогноз как доказанную экономию). Претензии здесь нет — но и утверждать, что «спроектированная экономия достигнута», сейчас нельзя ни в какую сторону: −30% cost / −20% turns / −35% uncached input остаются гипотезой до paired benchmark.

---

## 5. Рекомендации по приоритету

**Блокеры для 0.1.1**

1. **G1** — починить `Shadow` в `transform()` (одна строка) и добавить тест на профиль; либо реализовать counterfactual-учёт, либо убрать это утверждение из §6.6. Сейчас документированное поведение инвертировано.
2. **G3** — сделать fork regression gate детерминированным. Пока он проходит через раз, строка «проходит весь fork regression suite» в §13 не имеет силы.

**Важное, но не блокирующее**

3. **G2** — расширить protected-регекс на относительные пути, `UPPER_SNAKE`, идентификаторы и JSON-подобные payload **до** того, как появится любая трансформация уровня предложения.
4. **G5** — добавить runtime-ассерт порядка «disabled before first prompt» (например, счётчик вызовов в fake-bridge), а не только текстовый поиск.
5. **G4** — привести §6.6 в соответствие с фактической функциональностью codec.

**Гигиена**

6. G6 (двойной суффикс ошибки), G7 (права `hzrd.token.lock`), trailing-newline в codec.

**Следующий этап по существу** — ровно тот, что уже записан в PRD §14: paired baseline-vs-HZR benchmark. Все функциональные предпосылки для него готовы; именно он превращает §4.2 из гипотезы в результат.

---

## 6. Ответ на исходный вопрос

> Работает ли он как я того запроектировал?

**Да — в части архитектуры и инвариантов.** Я проверил не отчёты, а продукт: fork-core импортирован целиком и верифицируется хэшем, ownership boundaries держатся в рантайме, fail-closed работает на реальном дрейфе версий, context plan honours hard budget и деградирует с warning вместо тихого отказа, actual/estimated не смешиваются, `savings` не притворяется. Спроектированная дисциплина «единый владелец на каждый concern» действительно реализована, а не имитирована.

**Три вещи заявлены готовыми преждевременно:** `shadow`-профиль ведёт себя противоположно спецификации (G1, доказано запуском), fork regression gate проходит недетерминированно (G3, доказано 3/3 падениями), protected spans узкее заявленного (G2). Ни одна из них не подрывает архитектуру — все три локальны и починяемы малыми правками.

**Экономическая цель PRD не проверена вовсе** — 0 из 9 метрик §4.2. Это честно зафиксировано в самих документах, и именно это, а не код, остаётся главным открытым вопросом релиза.
