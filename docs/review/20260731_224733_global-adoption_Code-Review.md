# Code Review: глобальная замена RTK на HZR

> Historical pre-fix audit. The machine state and scores below are intentionally preserved as an audit trail and must not be read as the current release verdict. See `PRD.md` §13.1–13.2 and `PRD_STATUS_0.2.0.md` for the active gate.

**Дата**: 2026-07-31 22:47:33 MSK
**Ревьюер**: IT Architect Agent
**Область**: `hzr install`, `hzr init --if-needed`, Claude/Codex awareness, hook dispatcher, HZR/RTK/ICM/grepai ownership, фактическая конфигурация рабочей машины

## Executive Summary

Целевой механизм частично реализован и локально работоспособен: изолированный E2E подтвердил идемпотентную установку двух Claude-hook'ов, автоматический `SessionStart -> hzr init --if-needed --quiet`, создание canonical `.grepai` symlink и read-only повторный init. Строгое требование «HZR глобально является единственным entry point для Claude и Codex во всех новых проектах» сейчас **не выполнено**.

На рабочей машине `hzr` отсутствует в `PATH`; Claude использует 3 RTK-hook'а и 4 прямых ICM-hook'а; `~/.claude/CLAUDE.md` требует `rtk`; Codex использует Caveman-специфичный `~/.codex/AGENTS.md` и прямой MCP `icm serve`. Текущий `hzr install` меняет только Claude `settings.json`: он не устанавливает bundle, awareness-файлы, Codex contract, daemon service и не централизует внешний ICM.

## Architectural Diagram

```mermaid
graph TD
    U["Новая сессия Claude или Codex"] --> I["HZR global installer"]
    I --> B["Стабильный versioned HZR bundle"]
    I --> C["Claude managed block"]
    I --> X["Codex managed block + MCP bridge"]
    I --> S["User service: hzrd singleton"]

    C --> SS["SessionStart: hzr init --if-needed"]
    C --> PH["Один HZR hook dispatcher"]
    X --> CI["Codex startup/init contract"]
    X --> CP["Все HZR tool paths"]

    SS --> W["Canonical workspace identity"]
    PH --> D["hzrd control plane"]
    CI --> W
    CP --> D
    D --> R["Private HZR-owned fork-core/rtk"]
    D --> G["Один canonical grepai store на worktree"]
    D --> M["Одна HZR-owned ICM DB/process"]
    D --> V["Caveman codec / managed agent"]

    OLD["Standalone RTK hooks, direct ICM MCP/hooks, legacy watchers"] --> Q["Migration inventory + explicit retirement"]
    Q --> I
```

## Requirements Compliance

| Требование | Статус | Доказательство |
|---|---|---|
| `hzr init` автоматически запускается в новом проекте | **PARTIAL** | Код регистрирует `SessionStart` (`adoption.rs:287-301`); isolated E2E дал `initialized`, затем `already_initialized` без изменения mtime. Глобально hook не установлен. |
| Все Claude Bash-запросы проходят через HZR | **PARTIAL** | Dispatcher покрывает `Bash|Agent|Task` и имеет daemon -> pinned fork fallback (`hook_runner.rs:20-87`). Фактически Claude продолжает вызывать `rtk-rewrite.sh`. |
| Все Claude tool/prompt/response пути проходят через HZR | **FAIL** | Нет HZR handlers для `Read`, `Grep`, `Edit`, `Write`, `UserPromptSubmit`, `PostToolUse`, `PreCompact` или response/Stop. Только Bash и Agent/Task. |
| Codex глобально использует HZR | **FAIL** | Есть только bundled markdown asset. Нет мутации `~/.codex/AGENTS.md`, Codex config/MCP bridge или startup-init механизма. |
| Инструкции Claude и Codex требуют HZR | **FAIL** | Claude глобально требует `rtk` и `@RTK.md`; Codex global `AGENTS.md` описывает JuliusBrussee/caveman. HZR assets не устанавливаются. |
| RTK доступен только как private engine/compat alias HZR | **PARTIAL** | Bundle правильно содержит `bin/rtk -> hzr` и private `engines/rtk`; на машине остаётся прямой `/usr/local/bin/rtk`, а `hzr` отсутствует в `PATH`. |
| ICM централизован HZR | **FAIL** | Installer намеренно сохраняет external ICM hooks (`adoption.rs:188-190,435-478`); Codex запускает прямой `icm serve`; обнаружено несколько процессов с внешней DB. |
| grepai/rgai используют canonical HZR store без дублей | **PARTIAL** | Текущий HZR workspace корректен; глобально работают legacy/project-local watchers, а `doctor` проверяет только один workspace. |
| Caveman экономит запросы/ответы Claude/Codex глобально | **FAIL** | Codec и bridge доступны в HZR daemon/managed `hzr agent run`, но внешние Claude/Codex запросы и ответы не перехватываются. |
| Глобальная установка воспроизводима и самодостаточна | **FAIL** | `build-bundle.sh` собирает bundle, но нет deploy/upgrade/rollback installer. `hzr install` записывает `current_exe()`, включая mutable `target/release/hzr` (`adoption.rs:262-268`). |
| Диагностика честно выявляет текущий drift | **PASS** | `hzr doctor` вернул unhealthy: hooks missing, ICM 0.10.57 вместо 0.10.61, Caveman runtime отсутствует, daemon auth mismatch. |

Строгий global-adoption gate: **1 PASS / 4 PARTIAL / 6 FAIL**. Это около **27%** при весе `PASS=1`, `PARTIAL=0.5`; процент относится только к данному acceptance scope, а не ко всему HZR 0.2.

## Проверенное фактическое состояние

- `command -v hzr` — отсутствует.
- `/usr/local/bin/rtk` — `0.44.1-fork.1` и остаётся публичным самостоятельным entry point.
- Claude hook status: `HZR=0`, `RTK=3`, `external-ICM=4`.
- Dry-run корректно планирует `HZR=2`, `RTK=0`, но сохраняет `external-ICM=4`; реальные settings не менялись.
- ICM в `PATH`: `0.10.57`, pin HZR: `0.10.61`.
- Активный `hzrd` запущен из `target/debug` с временным workspace/data context; основной client получает HTTP 401 из-за несовпадающего token/data root.
- Найдены несколько независимых `icm serve` и несколько legacy `grepai watch` в разных проектах/временных каталогах.
- HZR workspace имеет правильный managed `.grepai` symlink и не имеет вложенного duplicate index.

## Architectural Assessment

### Сильные стороны

- Hook replacement удаляет известные RTK handlers до добавления HZR и проходит идемпотентный тест.
- Настройки пишутся через lock, compare-and-swap, full-SHA backup, atomic persist и `0600`.
- Bash fallback использует HZR engine resolver и сохраняет fork decision semantics без зависимости от живого daemon.
- `init --if-needed` безопасно различает missing, managed, legacy и foreign `.grepai`.
- Bundle сохраняет 100% fork-core и делает `bin/rtk` compatibility alias, а не вторым control plane.

### Проблемы

1. **[P0] Installer имеет неверную границу ответственности.** Команда с названием `hzr install` устанавливает лишь Claude hooks. Она не делает HZR глобальным продуктом.
2. **[P0] Codex отсутствует в runtime adoption.** Markdown asset без установки не влияет на поведение агента.
3. **[P0] ICM ownership противоречит цели.** Прямые Claude hooks и Codex MCP сохраняются, поэтому после HZR adoption появляются два memory control plane и две DB.
4. **[P0] Instructions остаются RTK-owned.** Пользовательское поведение продолжает направлять модели мимо HZR.
5. **[P1] `current_exe()` создаёт хрупкие hooks.** Запуск installer из `target`, temp bundle или удаляемого release оставляет битый путь после cleanup/upgrade.
6. **[P1] Нет user-level supervisor.** На горячем пути почти всегда возможен degraded fallback; нет гарантии единственного daemon на фиксированном endpoint и data root.
7. **[P1] Coverage не равен «все запросы».** Нативные file tools, user prompts и внешние LLM responses находятся вне HZR.
8. **[P1] Global doctor отсутствует.** Проверяется текущий workspace и Claude settings, но не Codex, PATH precedence, все процессы, все active indexes и stale runtime.
9. **[P2] PRD adoption содержит устаревшие команды `hzr read/write`.** Реальный contract правильно использует `hzr rtk -- read|write`; addendum строки 89, 97-99 расходятся с CLI.
10. **[P2] Dry-run не показывает обещанный exact diff.** Вывод содержит hashes/status, но `rendered_settings` исключён из JSON и diff не печатается.

## Целевая реализация

1. Разделить `hzr install` на транзакционный global installer с фазами `plan -> apply -> verify -> rollback`: versioned bundle, stable symlink, service, Claude, Codex, memory/index migration.
2. Никогда не писать hook на `current_exe()`. Hook должен ссылаться на stable path, например `~/.local/bin/hzr`, который атомарно переключается на immutable release directory.
3. Добавить managed marker-блоки и CAS-backups для:
   - `~/.claude/CLAUDE.md` + установленного `~/.claude/HZR.md`;
   - `~/.codex/AGENTS.md` + установленного `~/.codex/HZR.md`;
   - Codex MCP: HZR proxy/adapter вместо прямого `icm serve`.
4. Мигрировать известные прямые ICM handlers/MCP в HZR-owned routes. Неизвестные hooks сохранять, известные ICM entries выводить в plan и заменять только с подтверждением/backup.
5. Добавить user service (`launchd` на macOS; systemd user на Linux; Windows service/task позже), который запускает ровно один `hzrd` с явными `--config`/data root и versioned engine directory.
6. Ввести `hzr doctor --global`: stable binary, bundle attestation, Claude/Codex managed blocks, direct RTK precedence, ICM MCP/hooks/processes/DB, grepai watchers/index roots, daemon endpoint/token owner.
7. Для Claude расширить policy на нативные tools через поддерживаемые matchers либо честно оставить instruction-driven routing; request/response codec оформлять отдельным opt-in contract, поскольку Bash hook не перехватывает LLM transport.
8. Для Codex считать instructions soft enforcement. Где platform hook отсутствует, HZR должен давать MCP/tools и startup bootstrap; doctor обязан различать enforced и instructed coverage.

## Quality Scores

| Критерий | Оценка | Обоснование |
|---|---:|---|
| Code Quality | 84/100 | Безопасная запись settings, typed JSON hook results, хорошие isolated tests; adoption module слишком узко назван и классифицирует ownership по строковым suffix. |
| Extensibility/Modularity | 70/100 | Control-plane границы сильные, но installer жёстко привязан к Claude settings и не имеет provider abstraction/transaction plan. |
| Security | 76/100 | CAS, lock, backups и `0600` хороши; mutable `current_exe`, отсутствие binary attestation при adoption и конфликт daemon token/data root требуют исправления. |
| Optimization/Performance | 68/100 | Горячий hook ограничен 2 s и имеет fallback; отсутствие supervisor даёт лишний timeout/degraded path, внешние ICM hooks дублируют работу. |
| Architecture & Visualization | 72/100 | Внутренний HZR control plane согласован, но global ownership boundary не реализован. |
| Deploy Cleanliness | 28/100 | Bundle собирается, но нет глобального deploy/upgrade/service/awareness/Codex lifecycle. |
| **Итого** | **66/100** | Хорошее ядро 0.2, но global replacement ещё не продуктовый installer. |

## Critical Issues (Must Fix)

1. [CRITICAL] Реализовать полноценный stable global bundle installer и запретить hooks на build/temp executable.
2. [CRITICAL] Добавить Codex adoption и заменить глобальные RTK-инструкции Claude/Codex на HZR managed blocks.
3. [CRITICAL] Централизовать прямые Claude/Codex ICM integrations через HZR; duplicate memory control plane должен быть release-blocking error.
4. [CRITICAL] Добавить управляемый singleton `hzrd` service и global ownership/attestation gate.

## Recommendations (Should Fix)

1. [SHOULD] Добавить machine-wide inventory/migration для legacy RTK hooks, direct binary, ICM processes/DB и grepai watchers/indexes.
2. [SHOULD] Исправить PRD commands и сделать dry-run полноценным diff-plan.
3. [SHOULD] Добавить E2E fixtures Claude + Codex + upgrade + rollback + stale binary/service.

## Minor Suggestions (Nice to Have)

1. [NICE] Отдельно показывать `coverage=enforced|instructed|unsupported` для каждого agent surface.
2. [NICE] Выдавать `hzr adoption status --json` с per-provider state вместо одного общего счётчика hooks.
