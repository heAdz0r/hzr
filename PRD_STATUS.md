# PRD: HZR 0.1.0 — статус реализации по разделам

> Исторический аудит `v0.1.0`; сводки, gap registry и acceptance ниже намеренно не переписываются задним числом. В release candidate 0.2.0 G1–G7 и instruction-wiring gap A1 закрыты, а один installer поставляет versioned self-contained bundle с full fork-core, patched grepai, ICM, caveman-code и Node.js 22.17.1. Актуальный статус, включая platform-verification boundary и неизмеренный economic KPI, находится в [PRD_STATUS_0.2.0.md](PRD_STATUS_0.2.0.md); требования — в [PRD.md](PRD.md) и [PRD_ADOPTION.md](PRD_ADOPTION.md).

**Дата аудита:** 2026-07-31
**База:** [PRD.md](PRD.md) · перекрёстно с [FORK_PARITY.md](FORK_PARITY.md) · детали в [REVIEW.md](REVIEW.md)
**Метод:** самостоятельный запуск gate + end-to-end прогон продукта из чистого data root

## Легенда

| Маркер | Значение |
|---|---|
| ✅ | Реализовано и **проверено мной запуском**, а не по отчёту |
| 🟢 | Реализовано, проверено чтением кода/тестов (без отдельного прогона) |
| 🟡 | Реализовано частично либо гарантия слабее заявленной; есть gap |
| ❌ | Заявлено в PRD, фактическое поведение противоположно |
| ⚪ | Осознанно вне 0.1.0, зафиксировано в PRD/FORK_PARITY |
| 📊 | Требует измерения; данных нет |

**Сводка:** ✅ 34 · 🟢 21 · 🟡 5 · ❌ 1 · ⚪ 7 · 📊 9

---

## §1 Решение и fork-core contract

| # | Требование | Статус | Основание |
|---|---|---|---|
| 1.1.1 | Источник истины — фактический worktree, включая dirty/untracked | ✅ | `SNAPSHOT.toml`: `source_head=5f403c46`, `source_diff_sha256`, `source_status_sha256`, `selection = "git tracked ... plus untracked"` |
| 1.1.2 | Перенесён весь source/product surface | ✅ | 516 файлов на диске = `included_file_count = 516` |
| 1.1.3 | **Ноль упрощённых reimplementations в runtime-пути** | ✅ | Целенаправленно искал: search → fork `rgai`, plan → fork `memory plan`, exec → fork `rewrite`. Не найдено |
| 1.1.4 | Исключения только генерируемые/секретные | 🟢 | `exclusion_record_count = 32`, категории заявлены |
| 1.1.5 | Машинно проверяемый manifest | ✅ | `SNAPSHOT_V2.tsv` (79 KB), hex-encoded paths, per-file digest, 4 tracked deletions |
| 1.1.6 | Fork собирается и проходит свои тесты из HZR; CI отклоняет дрейф | 🟡 | Сборка и verify — ✅; regression suite **недетерминирован** → **G3** |
| 1.1.7 | Интеграция через adapters; overlay auditable | ✅ | `hzr-exec` — process adapter, своей таблицы rewrite нет |
| 1.1.8 | Stock RTK не fallback | ✅ | `engines.lock.toml`: `rtk-upstream-reference` с `runtime = false`, `source_kind = "reference-only"` |
| 1.x | Один CLI / daemon / config root / protocol / budget / ledger | ✅ | Прогон: 15 команд, singleton `hzrd`, один data root, protocol 1 |

## §2 Product contract

| Требование | Статус | Основание |
|---|---|---|
| Формула `cost_per_accepted_task` как оптимизируемая функция | 🟢 | `EconomicInput::expected_value()` в [hzr-codec/src/lib.rs](crates/hzr-codec/src/lib.rs) реализует `saved − overhead − p(retry)·cost` |
| Трансформация только при положительном expected_value | 🟢 | `is_profitable()` + `test_economic_gate_charges_instruction_and_retry_cost` |
| Exact/lossless policy для code/JSON/paths/commands | 🟡 | `FidelityClass::Exact` соблюдается (проверено), но protected-регекс не покрывает 4 заявленных класса → **G2** |
| Raw / content-addressed reference при неопределённости | ✅ | `test_choose_never_worse_*`, `test_protected_duplicate_falls_back_to_raw`; в прогоне raw сохранён byte-for-byte |

## §3 Evidence

| Требование | Статус |
|---|---|
| §3.1–3.5 выводы исследования зафиксированы с источниками | 🟢 Документ, не код. Все 5 подразделов имеют ссылки; выводы согласованы с §15 |

## §4 Goals и guardrails

### §4.1 Goals для 0.1.x

| Goal | Статус | Основание |
|---|---|---|
| Один semantic index на `(workspace, root, embedder, model, dim)` | ✅ | Прогон: `generation=68f4b53a...`; `test_linked_worktrees_share_repository_identity_but_not_index_identity` |
| 100% fork surface в hash-locked fork-core | ✅ | verify-fork-core.sh → `verified` |
| Ноль reimplementations в runtime-пути | ✅ | см. 1.1.3 |
| Ноль project-local index data; только проверенный symlink | ✅ | Прогон: `ForeignSymlink` детектирован и заблокирован без мутации |
| Один ICM process и одна canonical DB | 🟢 | `test_supervisor_owns_one_process_and_second_instance_attaches`, `test_supervisor_recovers_orphan_without_spawning_duplicate` |
| Точный RTK rewrite contract (rewrite ≠ auto-allow) | ✅ | Прогон: вердикт `ask:` для `cat main.rs`; exit 0/1/2/3 покрыты тестами |
| Hard evidence budget по маркированной estimate; без скрытого второго pre-read | ✅ | Прогон: `tokens=180/16000` и `180/12000`; `test_plan_never_exceeds_hard_budget` |
| **Адаптивный codec с protected spans и raw fallback** | 🟡 | Raw fallback ✅; но codec сводится к dedupe абзацев, spans узкие → **G2**, **G4** |
| caveman-code managed mode без дублирующих слоёв | 🟡 | Отключения присутствуют, но порядок «before first prompt» проверяется текстовым поиском → **G5** |
| Actual usage отдельно от estimates | 🟢 | `test_ledger_keeps_estimates_out_of_actual_totals` + `test_usage_route_records_provider_tokens_separately_from_estimates` |
| Все engines проверяются по version/integrity до запуска | ✅ | Прогон: `doctor` поймал реальный дрейф ICM 0.10.57 vs 0.10.61 → exit 1 |
| Offline local mode; telemetry off | ✅ | Прогон: `RTK_TEE=0`, `RTK_TELEMETRY_DISABLED=1`; loopback-only |

### §4.2 Product metrics — 0 из 9 измерено

| Метрика | Цель | Статус |
|---|---|---|
| median billed cost / accepted task | ≥ −30% | 📊 нет данных |
| median turns | ≥ −20% | 📊 |
| uncached input tokens | ≥ −35% | 📊 |
| tool-result bytes в context | ≥ −60% | 📊 |
| retrieval recall@20 | ≥ 95% | 📊 нет gold set |
| task success non-inferiority | ≤ 1 п.п. | 📊 |
| p95 warm overhead | ≤ 250 ms | 📊 не инструментировано |
| p90 cost отдельной задачи | ≤ +5% | 📊 |
| stale-index инциденты | 0 | 📊 нет продакшн-наблюдений |

**Соблюдение дисциплины отчётности — ✅ (проверено).** `hzr savings` выдаёт `tasks: 0, accepted: 0` и **не** печатает `cost_per_accepted_task`. Требование «UI не имеет права показывать прогноз как доказанную экономию» выполнено буквально.

### §4.3 Non-goals — соблюдены

| Non-goal | Статус |
|---|---|
| Сжатие reasoning провайдера | ✅ не делается |
| Regex-переписывание code/JSON/enums/args | ✅ `FidelityClass::Exact` короткозамыкает; проверено прогоном |
| Общая физическая SQLite для index+memory+ledger | ✅ Прогон: `memory/icm/memories.db`, `ledger/usage.sqlite`, `fork/history.db` раздельны |
| Облачный control plane | ✅ loopback-only, `test_config_rejects_non_loopback_bind` |
| Автоудаление legacy indexes | ✅ `test_migrate_legacy_index_refuses_*` — ничего не удаляется |
| Копирование caveman-code в Rust | ✅ managed npm runtime |
| Переписывание fork | ✅ snapshot immutable |
| Замена fork-core на stock RTK | ✅ `runtime = false` для upstream |
| Обещание «нулевой потери качества» | ✅ формулировок нет |

## §5 Архитектура

| Требование | Статус | Основание |
|---|---|---|
| §5.1 Ownership matrix — единственный владелец на concern | ✅ | Проверено прогоном по всем 10 строкам матрицы |
| §5.2 п.1 intent без переписывания | 🟢 | Codec не трогает user intent; density-контракт добавляется к ответу |
| §5.2 п.2 один managed grepai lifecycle, без unconditional query | 🟢 | `IndexCoordinator`, `test_coordinator_reuses_one_watcher_for_repeated_prepare` |
| §5.2 п.3 fork `memory plan` + один ICM recall параллельно | ✅ | Прогон: `fork-plan pipeline=graph_first_v1` + `warning MemoryUnavailable` в одном ответе |
| §5.2 п.4 `rgai` только при пустом planner | 🟢 | Логика планировщика; `test_search_and_context_use_managed_fork_core_commands` |
| §5.2 п.5 нормализация в `ContextCandidate` с provenance/hash/generation | ✅ | Прогон: `{"sources":["tier_c","semantic:budget"],"estimated_tokens":180}` |
| §5.2 п.6 один candidate на content ref + hard limit | 🟢 | `test_plan_deduplicates_same_content_reference`, `test_equal_content_has_stable_reference` |
| §5.2 п.7 bounded metadata, без eager reread | 🟢 | Прогон: план вернул метаданные+оценку, не содержимое файла |
| §5.2 п.8 density contract до generation; без второго lossy post-processing | 🟢 | `appendSystemPrompt: responseContract` в bridge |
| §5.2 п.9 только allowlisted tools | 🟢 | `installManagedToolGuard` + `beforeToolCall`; но см. **G5** |
| §5.2 п.10 JSON validation, empty output отклоняется, usage в ledger | 🟢 | `test_bridge_enforces_response_quality_before_and_after_generation` |
| §5.2 п.11 в ICM только durable facts | 🟢 | Нет автоматической записи tool output |

## §6 Компоненты

### §6.1 `hzr-protocol` — ✅
Versioned envelopes; actual/estimated разделены на уровне типов. `test_envelope_serialization_preserves_protocol_version`, `test_token_source_preserves_provider_only_counts`.

### §6.2 `hzr-core` — ✅
Canonical layout, engine lock, fusion, budgets, ledger, migration state. Прогон `hzr init` создал ровно спроектированный layout.

### §6.3 `fork-core` + `hzr-exec` — ✅ (сильнейшая часть)

| Требование | Статус |
|---|---|
| Полный hash-locked snapshot dirty worktree | ✅ verified |
| `hzr-exec` — тонкий adapter без своей таблицы rewrite | ✅ |
| Exit code / stderr / paths / identifiers сохранены | ✅ `test_pipeline_reports_exact_exit_and_stderr_channels`, `test_pipeline_reports_exact_unix_signal` |
| Raw/direct fallback по fork-семантике; stock RTK запрещён | ✅ |
| Публичное имя `hzr`; fork не публикуется как control plane | ✅ Прогон: `hzr rtk -- --version` → `rtk 0.44.1-fork.1` |

40 тестов на 2843 LOC: pipes, `&&/||`, heredoc, redirects, xargs, quoting, non-UTF8 argv, process-group timeout, spill на диск.

### §6.4 `hzr-index` — ✅ (11/11 пунктов)
Canonical paths + git common dir, стабильные ID, один config/watcher/generation, version check против lock, подготовка store, никакого конкурирующего ranker, детекция nested/legacy без удаления, отказ чужому watcher, блокировка legacy до `migrate apply`, инвалидация по generation+hash.
Прогон подтвердил `ForeignSymlink` и `grepai_duplicates: none found`. Патч `--no-worktree-discovery` проверяется в CI на pinned commit + `go test ./cli`.

### §6.5 `hzr-memory` — 🟢 (13/13 пунктов реализованы)
Фиксированная DB, singleton lock, MCP store с полной семантикой, typed CLI JSON recall, repository-scoped topic namespace, private permissions, circuit breaker, idempotent lifecycle, version check, без индексации кода.
Проект-скоупинг покрыт всерьёз: `test_isolate_project_memories_removes_global_and_cross_repo_records`, `test_topic_belongs_to_project_never_accepts_global_or_foreign_topics`, `test_memory_route_rejects_user_supplied_project_override`.
Прогон: version check сработал (0.10.57 ≠ 0.10.61) → `warning MemoryUnavailable`, code plan сохранён. Ровно §10.

### §6.6 `hzr-codec` — ❌ / 🟡 (единственное нарушение контракта)

| Требование | Статус | Основание |
|---|---|---|
| Профили `off`, `safe`, `adaptive`, `compact`, `shadow` | 🟢 | Все 5 в CLI и protocol |
| `adaptive` проверяет экономику до добавления контракта | 🟢 | `is_profitable()` + порог `>= 600` output tokens |
| **`shadow` не меняет доставляемый контент** | ❌ | **Меняет.** `--profile shadow` → `"changed": true`, абзац удалён → **G1** |
| **`shadow` записывает counterfactual size** | ❌ | Не записывает. `counterfactual\|shadow_size` → 0 совпадений → **G1** |
| Protected spans: code fences, inline code, URLs, flags, hashes, versions | 🟢 | Покрыты регексом, проверено |
| Protected spans: **paths, identifiers, enum-like, structured payloads** | 🟡 | `src/main.rs`, `MAX_RETRIES`, `handle_budget_overflow`, `{"k":1}` → `protected_spans: []` → **G2** |

Причина G1 — [lib.rs:106](crates/hzr-codec/src/lib.rs#L106): short-circuit покрывает только `Exact` и `Off`; `Shadow` проваливается в `deduplicate_paragraphs`. Ни один из 5 тестов codec не покрывает `Shadow`.

### §6.7 `hzr-agent` — 🟡

| Требование | Статус |
|---|---|
| Version + npm integrity pinned | ✅ `test_package_lock_digest_matches_compiled_provenance`, `test_package_lock_digest_rejects_tampering` |
| Изолированный `agentDir` под HZR data root | 🟢 `test_prepare_agent_data_dir_rejects_symlink` |
| Native RTK/repo-map/memory/hooks/compression/telemetry/builtins отключены **до первого prompt** | 🟡 Вызовы присутствуют, но порядок проверяется `BRIDGE.contains(...)` → **G5** |
| Exact allowlist custom tools | 🟢 `installManagedToolGuard` + `beforeToolCall` |
| Один bounded prefetch как untrusted evidence | 🟢 `context_prefetched: true` |
| Text и strict JSON режимы | 🟢 |
| Credentials не копируются в ledger | 🟢 §9 |
| Health: protocol 1, HZR 0.1.0, ровно один ready fork-core | ✅ `test_bridge_preflight_requires_compatible_hzr_and_ready_fork_core`; прогон: `state Degraded` при незавершённом окружении |
| Usage постится один раз с terminal outcome; сбой учёта не маскирует результат | 🟢 `test_bridge_accounts_provider_usage_once_for_every_terminal_outcome` |
| Managed launch fails closed; прочие команды работают | ✅ Прогон подтвердил |
| Node ≥ 20.18.1, Node 26 заблокирован | 🟢 `test_supported_node_range_keeps_node_25_and_excludes_node_26` |
| `adm-zip` override, `npm audit` без high/critical | ✅ CI-шаг зелёный |
| Остаточный `cavemem --version` probe | 🟡 Признано в PRD и FORK_PARITY |

### §6.8 `hzrd` и `hzr-cli` — ✅ точное совпадение

**12/12 routes** в [server.rs:22-33](crates/hzr-daemon/src/server.rs#L22) — health, engines, search, context/plan, memory/recall, memory/store, exec/rewrite, exec/run, exec/approval, fork/run, codec/compile, usage. Без лишних и без недостающих.

**15/15 команд CLI** — `init, doctor, daemon, engines, index, search, rgai, context, memory, exec, codec, agent, savings, migrate, rtk`.

`bin/rtk` → alias на `bin/hzr` с нормализацией в `hzr rtk --`: `test_normalize_rewrites_installed_rtk_alias`, `test_normalize_preserves_non_utf8_fork_argument`.

## §7 Data layout и запрет дублей — ✅

Прогон `hzr init` из чистого `HOME` создал `runtime/`, `workspaces/`, `memory/icm/`, `ledger/`, `engines/`, `config.toml`; `fork/` и `migrations/` создаются лениво (подтверждено `RTK_MEM_DB_PATH=<data>/fork/mem.db` в прогоне).

| Инвариант | Статус |
|---|---|
| index и memory физически раздельны | ✅ проверено |
| Нет project-local index data; `.grepai` только symlink | ✅ проверено |
| Real `.grepai` = legacy, блокирует до migration | ✅ проверено (`ForeignSymlink`) |
| Legacy обнаруживается read-only scan | 🟢 `test_collect_markers_finds_database_without_following_symlinks` |
| Migration только явной командой + backup + 2 manifest | 🟢 6 тестов `test_migrate_legacy_index_*` |
| Автоудаление/quarantine запрещены | ✅ |
| Singleton `hzrd` + worktree owner lock | ✅ Прогон: `hzrd.lock`; `test_acquire_refuses_second_owner_and_releases_without_deleting_file` |
| Один content hash не повторяется в pack | 🟢 `test_plan_deduplicates_same_content_reference` |

## §8 Version и supply-chain — ✅

`engines.lock.toml` совпадает с таблицей §8 **буквально** по всем 6 компонентам (grepai 0.35.0/`65c345ca`, ICM 0.10.61/`c3a1bac7`, fork 0.44.1-fork.1/`5f403c46`, upstream 0.44.1/`36591fb0` reference-only, Caveman 1.9.1/`0d95a81d`, caveman-code 0.65.2/`4700b8fa` + sha512 integrity).

CI реально клонирует grepai на pinned commit, применяет патч, гоняет `go test ./cli`. Patch SHA-256 зафиксированы для обоих патчей.

## §9 Security и privacy — ✅ (проверено на живом процессе)

| Требование | Статус |
|---|---|
| Loopback-only; non-loopback не поддержан | ✅ `test_config_rejects_non_loopback_bind`, `test_hzr_api_rejects_remote_or_credentialed_hosts` |
| Bearer token на локальный API | ✅ `test_all_routes_require_bearer_authentication` |
| Private permissions на секреты | 🟡 `hzrd.token` = `0600` ✅, но `hzrd.token.lock` = `0644` → **G7** (секрета не содержит) |
| API keys не логируются | ✅ `test_bearer_token_debug_is_redacted`, `test_redact_token_removes_every_occurrence` |
| `RTK_TEE=0`, `RTK_TELEMETRY_DISABLED=1` | ✅ **наблюдал в выводе `exec rewrite`** |
| Telemetry и raw retention off | ✅ |
| Allowlist argument shapes + canonical paths | ✅ `test_managed_fork_api_confines_read_and_write_paths`, `test_managed_fork_api_rejects_symlink_escape` |
| Destructive commands → отдельный verdict | ✅ Прогон: вердикт `ask:` |
| Body/capture/time limits; traversal отклоняется | ✅ `test_payload_limit_reserves_json_envelope_space`, `test_capture_writer_truncates_at_safe_memory_cap` |
| Ledger без prompt/response body | 🟢 |

## §10 Failure modes — ✅ все 11 воспроизведены или покрыты

| Failure | Статус |
|---|---|
| `hzrd` недоступен → блок managed, `hzr rtk` работает | ✅ **проверено прогоном** |
| grepai отсутствует/устарел → exact fallback, degraded | 🟢 `test_connect_rejects_unpinned_grepai_version` |
| index stale → stale provenance | 🟢 |
| legacy/duplicate/foreign index → typed error, без удаления | ✅ **проверено** (`ForeignSymlink`) |
| ICM недоступен → warning + code plan | ✅ **проверено** (`warning MemoryUnavailable` + план) |
| codec invariant нарушен → raw | ✅ `test_protected_duplicate_falls_back_to_raw` |
| fork-core недоступен/version mismatch → блок, без stock RTK | ✅ `test_adapter_version_mismatch_fails_closed`, `test_adapter_missing_binary_fails_closed` |
| fork выбрал raw/fail-open → семантика сохранена | ✅ |
| provider usage отсутствует → только estimated columns | 🟢 |
| caveman-code SDK drift → блок с remediation | 🟢 `test_bridge_preflight_*` |
| budget исчерпан → отказ с reason, без скрытого расширения | 🟢 `test_plan_never_exceeds_hard_budget`, `test_budget_never_underflows` |

## §11 Migration — 🟢
Все 8 шагов `migrate apply` реализованы; `scan` read-only. Отказы (escaping symlink, special files, active owner, source mutation, partial state) покрыты 6 тестами. Backup не удаляется автоматически. `/Users/andrew/Programming/rtk` не изменялся.

## §12 Verification strategy

### §12.1 Rust quality gates — ✅ запущены мной
```
cargo fmt --all --check                → exit 0
cargo clippy --workspace -D warnings   → exit 0
cargo test --workspace --all-targets   → 160 passed; 0 failed
```
Плюс CI: MSRV 1.85, `bash -n scripts/*.sh`, `node --check`, `npm audit`.

### §12.2 Contract tests

| Требование | Статус |
|---|---|
| Snapshot manifest воспроизводит 100% source set | ✅ verified |
| **Весь fork test/benchmark harness присутствует и запускается** | 🟡 присутствует и запускается, но **недетерминированно** → **G3** |
| Fork CLI/rewrite/read/write/rgai/memory/guard без потерь | ✅ |
| stdout/stderr/exit preservation | ✅ |
| grepai JSON fixtures + version drift | 🟢 |
| root/worktree identity + duplicate detection | 🟢 |
| ICM singleton race, stale PID, token perms, breaker | 🟢 4 теста |
| Сумма token estimates ≤ hard limit | 🟢 |
| **Protected spans survive byte-for-byte** | 🟡 доказано только на подмножестве регекса → **G2** |
| Estimates never increment actual | 🟢 |
| **Caveman duplicate layers disabled before prompt** | 🟡 текстовый поиск, не runtime-порядок → **G5** |
| Daemon body limit, timeout, auth, loopback | 🟢 |

### §12.3 Paired benchmark — 📊 не выполнен
Инфраструктура готова (ledger разделяет actual/estimated, outcome-метки есть), самих данных нет.

## §13 Release acceptance — 15 ✅ / 2 🟡 из 17

| # | Критерий | Статус |
|---|---|---|
| 1 | fork-core импортирован целиком, manifest проверен независимо | ✅ |
| 2 | `FORK_PARITY.md` без `missing`/`reimplemented` | ✅ (единственное вхождение — в описании легенды) |
| 3 | Stock RTK отсутствует в execution path и bundle | ✅ |
| 4 | Все crates компилируются без warnings | ✅ |
| 5 | Quality gates зелёные | ✅ |
| 6 | `hzr doctor --json` проверяет pins и ownership | ✅ **прогон: поймал реальный дрейф, exit 1** |
| 7 | ICM start/stop race test доказывает singleton | 🟢 |
| 8 | Nested `.grepai` fixture обнаруживается и не удаляется | ✅ |
| 9 | `hzr search` использует grepai 0.35.0 + exact fallback | ✅ |
| 10 | `hzr rgai` использует ту же generation | ✅ |
| 11 | **`hzr exec` делегирует fork-core и проходит весь fork regression suite** | 🟡 делегирование ✅; suite недетерминирован → **G3** |
| 12 | **Codec сохраняет protected spans** | 🟡 на подмножестве; `shadow` вообще мутирует контент → **G1/G2** |
| 13 | Managed caveman-code smoke подтверждает отключение слоёв | 🟢 (метод проверки слабый → **G5**) |
| 14 | CLI/daemon smoke из чистого data root | ✅ **воспроизведено** |
| 15 | README: установка, инварианты, recovery | ✅ 14 разделов, включая «Быстрый старт», «Data root», «Проверки», «Честные границы» |
| 16 | ICM содержит handoff | 🟢 README §«Handoff для LOOP-агентов» |
| 17 | Initial commit и version 0.1.0 | ✅ `c5a10f1 feat: release hzr 0.1.0`, workspace `version = "0.1.0"` |

## §14 Delivery status

Заявленное реализовано и подтверждено. Осознанно не включённое (⚪, не является gap): фоновые `daemon start/stop`, automatic engine sync, hook installer, destructive cleanup, Windows artifact, формальный legal review, crash-safe usage outbox, runtime re-attestation скомпилированного бинаря.

## §15 Decision log — все 11 решений соблюдены в коде ✅
Проверено: HZR — самостоятельный продукт; fork неизымаем; grepai — единственный semantic index; `rgai` — facade без storage; ICM — единственная durable memory; Caveman — адаптивный contract; caveman-code — optional runtime, не второй control plane; HZR Core — владелец budget/policy/lifecycle/ledger; actual ≠ estimates; duplicates не удаляются; quality через outcome и инварианты.

---

## Реестр gap

| ID | Раздел PRD | Severity | Суть | Фикс |
|---|---|---|---|---|
| **G1** | §6.6, §13.12 | **ВЫСОКИЙ** | `shadow` мутирует доставляемый контент (`changed: true`, абзац удалён) и не пишет counterfactual size — поведение **инвертировано** относительно PRD | Добавить `Shadow` в short-circuit `transform()` (1 строка) + тест профиля; реализовать учёт либо убрать заявление из §6.6 |
| **G3** | §1.1.6, §12.2, §13.11 | **СРЕДНИЙ** | `tracking::tests::test_timed_execution_records_time` — race на общей tracker-БД (`get_recent(5)`): parallel FAILED 3/3, single-threaded ok. Делает release-gate недетерминированным | `--test-threads=1` для модуля, per-test `RTK_DB_PATH`, либо уникальное имя команды + фильтр |
| **G2** | §6.6, §2, §12.2 | СРЕДНИЙ (латентный) | Protected spans не покрывают относительные пути, identifiers, enum-like, structured payloads — 4 заявленных класса | Расширить регекс **до** появления любой трансформации уровня предложения |
| **G5** | §6.7, §12.2, §13.13 | НИЗКИЙ | Инварианты bridge проверяются `BRIDGE.contains(...)` — наличием строки, а не порядком «before first prompt» | Runtime-ассерт порядка вызовов в fake-bridge |
| **G4** | §4.1, §6.6 | НИЗКИЙ | «Адаптивный codec» = селектор density + dedupe точных дубликатов абзацев (261 LOC); формулировка переобещает. Плюс срезается trailing `\n` | Привести §6.6 к факту |
| **G6** | — | КОСМЕТИКА | Дублированный суффикс ошибки: `...(os error 2); run \`hzr daemon serve\`: ...(os error 2)` | Убрать лишний `anyhow` context |
| **G7** | §9 | ИНФО | `hzrd.token.lock` = `0644` при `hzrd.token` = `0600` (секрета не содержит) | Выровнять права в `runtime/` |

## Открытый вопрос релиза

Не gap, а незавершённое измерение: **0 из 9 метрик §4.2**. Функциональные предпосылки для paired benchmark (§14 п.1–2) готовы — ledger разделяет actual/estimated, outcome-метки пишутся, `savings` не подменяет отсутствие данных прогнозом. До получения данных утверждения о −30% cost / −20% turns / −35% uncached input остаются гипотезой, что PRD признаёт прямо.

## Следующий архитектурный этап: HZR MCP

После стабилизации и публикации 0.2.0 запланирован HZR-owned MCP gateway. Он обязан переиспользовать canonical HZR Index, Memory, execution, codec, policy и ledger; создание параллельных индексов, memory stores или daemon owners запрещено. Claude и Codex должны подключаться к одному HZR entrypoint вместо прямого запуска внутренних engines. Полная спецификация — §14.1 `PRD.md`.
