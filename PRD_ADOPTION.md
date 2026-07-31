# PRD addendum §16 — HZR как default entry point (замена RTK-хуков)

**Статус:** спецификация, не реализовано
**Родитель:** [PRD.md](PRD.md) · закрывает ⚪ пункт «HZR hook installer/background service manager» из [FORK_PARITY.md](FORK_PARITY.md)
**Целевая версия:** 0.2.0 (расширяет CLI surface §6.8 и mutation surface §14 — вне рамок 0.1.0)
**Принятые решения:** hybrid daemon→fork fallback · `HZR` = heAdz0r's Zero-Redundancy engine

---

## 16.1 Задача

Сегодня `~/.claude/settings.json` маршрутизирует хуки в `~/.claude/hooks/rtk-*.sh` и `~/.local/bin/icm hook *`. RTK — установленный по умолчанию перехватчик, HZR — нет. Требование: **HZR становится default entry point для всех новых проектов**, а RTK перестаёт вызываться напрямую, продолжая работать внутри как fork-core.

Это распространяется на три поверхности:

1. **Автоматический `hzr init`** в новом проекте — без ручного шага.
2. **Все запросы через HZR** — rewrite, memory-context, block-native-explore идут в HZR, а не в RTK напрямую.
3. **Инструкции агентов** (Claude Code и Codex) описывают HZR-поведение так же, как сейчас описывают RTK.

## 16.2 Непереговорный инвариант адопции

> **Ровно один перехватчик на событие.** RTK-хук и HZR-хук не могут быть зарегистрированы одновременно.

Это прямое следствие §4.1 («ноль дублирующих layers») и самого акронима. Двойной rewrite — двойной scan, двойное сжатие и конкурирующие вердикты. Установщик обязан **удалить или заменить** rtk-записи, а не добавиться рядом. `hzr doctor` обязан репортить сосуществование как `error`, а не `warning`.

## 16.3 Hook execution model — hybrid (принятое решение)

Хук срабатывает на **каждую** Bash-команду, поэтому он не имеет права зависеть от живого демона.

```text
hzr-rewrite.sh
  ├─ hzrd доступен?
  │    ├─ да  → POST /v1/exec/rewrite   (HZR policy + budget + ledger)  source=managed
  │    └─ нет → hzr rtk -- rewrite <argv> (прямой fork, daemon-free)    source=degraded
  └─ exit 0/1/2/3 — семантика fork сохраняется в обоих ветках
```

Правила:

- **Шелл не ломается никогда.** Отсутствие `hzrd` — не ошибка хука. Это ровно тот путь, который PRD §10 уже разрешает: «`hzrd` недоступен → exact compatibility `hzr rtk`/`bin/rtk` остаётся прямым process path».
- **Деградация видима, а не молчалива.** Каждый degraded вызов инкрементит счётчик, `hzr doctor` показывает `degraded_rewrites=N`, и `hzr savings` помечает период как частично неучтённый. Это соблюдает §4.2: неучтённое не выдаётся за учтённое.
- **Никакого silent fallback на stock RTK.** Fallback идёт в `hzr rtk --`, то есть в тот же pinned fork-core `0.44.1-fork.1`. Запрет §1.1.8 не нарушается.
- **Timeout хука жёсткий** (предлагаю 2 s на probe демона): просроченный probe немедленно уходит в degraded, а не висит в терминале.
- Автостарта демона нет. Он остаётся явной операцией (`hzr daemon serve`), как и решено в §14.

**Цена решения, зафиксированная явно:** пока демон не запущен, rewrite не попадает в ledger. Полный учёт требует либо запущенного `hzrd`, либо будущего background supervisor (§14 п.4).

## 16.4 Автоматический `hzr init`

`init` должен быть идемпотентным и **не-мутирующим для содержимого репозитория**.

| Условие | Поведение |
|---|---|
| Новый workspace, канонический data root есть | Регистрирует `(repository_id, worktree_id)`, готовит запись под `workspaces/`, ничего в репозитории не создаёт |
| `.grepai` отсутствует | Создаёт проверенный symlink на canonical store (§7) |
| `.grepai` — реальная директория | **Не трогает.** `migration_required`, ждёт явного `hzr migrate apply` (§11) |
| `.grepai` — чужой symlink | `error`, без мутации |
| Уже инициализирован | `already_initialized`, exit 0 |

Триггер автоматизации — `SessionStart` hook, вызывающий `hzr init --if-needed --quiet`. Это дешевле и безопаснее, чем автоинициализация внутри rewrite-пути: rewrite остаётся горячим путём без файловых мутаций.

Требование: `hzr init --if-needed` в уже инициализированном workspace обязан быть **чистым read-only no-op** — иначе он превращается в скрытую запись на каждый старт сессии.

## 16.5 Новый CLI surface (расширение §6.8)

```text
hzr install [--dry-run] [--force]      # идемпотентно: хуки + инструкции + миграция rtk-записей
hzr uninstall [--keep-data]            # полное снятие; RTK-записи не восстанавливаются молча
hzr hooks status [--json]              # что зарегистрировано, кем, и есть ли конфликт с rtk
hzr init --if-needed [--quiet]         # для SessionStart
```

`hzr install` обязан:

1. Сделать **backup** `settings.json` с full-SHA в имени, как это делает migration (§11) — тот же проверенный паттерн, не новый.
2. Показать точный diff и требовать подтверждения; `--dry-run` — обязательный первоклассный режим.
3. Быть idempotent: повторный запуск даёт `already_installed`, не дублирует записи.
4. **Обнаружить и заменить** rtk-записи (`rtk-rewrite.sh`, `rtk-mem-context.sh`, `rtk-block-native-explore.sh`), сохранив их в backup.
5. Никогда не трогать `icm hook *` записи, если ICM управляется вне HZR — но пометить их в `hooks status` как потенциальный дубль memory-слоя (HZR уже владеет ICM по §6.5).

## 16.6 Инструкции агентов

Fork-core уже содержит готовый образец обеих поверхностей: `hooks/rtk-awareness.md` (Claude), `hooks/rtk-awareness-codex.md` (Codex), `hooks/rtk-instructions.md`. HZR-аналоги делаются по тому же разделению, **без копирования RTK-текста дословно** — команды другие.

| Артефакт | Назначение |
|---|---|
| `HZR.md` | Канонический контракт инструмента: `hzr read/grep/rgai/write/exec`, когда raw, когда exact |
| `hooks/hzr-awareness.md` | Claude Code: приоритет `hzr` над нативными Read/Grep/Edit |
| `hooks/hzr-awareness-codex.md` | Codex: тот же контракт в его формате инструкций |
| Ссылка в `~/.claude/CLAUDE.md` | Одна строка `@HZR.md`, устанавливается `hzr install` |

Единый контракт для обоих агентов (замена текущей RTK-таблицы):

```text
Read     → hzr read <file>            (или hzr rtk -- read при отсутствии демона)
Grep     → hzr rgai "<intent>" | hzr search <pattern>
Edit     → hzr write patch|replace|set
Bash     → перехватывается hzr-rewrite.sh автоматически
Memory   → hzr memory recall|store    (единственный durable store, §6.5)
Context  → hzr context plan "<intent>"
```

Требование к тексту инструкций: он обязан описывать **degraded-ветку** тоже. Агент, получивший `source=degraded`, должен знать, что ledger неполон, а не считать, что учёт идёт.

## 16.7 Verification (расширение §12.2)

Новые contract tests, обязательные до релиза 16.x:

- `install` idempotent: двойной запуск не создаёт вторую запись;
- `install` создаёт full-SHA backup `settings.json` и восстановим из него;
- rtk-запись и hzr-запись **не могут сосуществовать**; `doctor` даёт `error` при сосуществовании;
- хук возвращает **exit 0/1/2/3 идентично** в managed и degraded ветках на одном корпусе команд (это ключевой тест: деградация не должна менять вердикт);
- probe демона соблюдает timeout и уходит в degraded, не блокируя;
- degraded вызовы считаются и видны в `doctor`/`savings`;
- `init --if-needed` на инициализированном workspace — байт-в-байт no-op по файловой системе;
- `init` не трогает реальную `.grepai` и требует migration;
- `uninstall` снимает все записи и не оставляет битых путей;
- fallback уходит в pinned fork `0.44.1-fork.1`, а не в stock RTK (версионный ассерт).

## 16.8 Риски

| Риск | Митигация |
|---|---|
| Двойной rewrite RTK+HZR | §16.2 инвариант + `doctor` error + замена записей при install |
| Хук ломает терминал при мёртвом демоне | hybrid fallback (§16.3) + жёсткий timeout |
| Молчаливо неполный ledger | Счётчик degraded + отражение в `doctor`/`savings` (§4.2) |
| Повреждение `settings.json` | Full-SHA backup + `--dry-run` + diff-подтверждение (паттерн §11) |
| Рост mutation surface против §14 | Ограничить его ровно четырьмя командами §16.5; автостарта демона нет |
| Дубль memory-слоя с внешним `icm hook` | `hooks status` помечает; HZR владеет ICM по §6.5 |

## 16.9 Порядок работ

1. `hzr-rewrite.sh` + hybrid-логика и её exit-code-эквивалентность (ядро; без него остальное бессмысленно).
2. `hzr install/uninstall/hooks status` с backup, dry-run и заменой rtk-записей.
3. `hzr init --if-needed` + регистрация `SessionStart`.
4. `HZR.md` + awareness для Claude и Codex.
5. Contract tests §16.7 и `doctor`-проверка сосуществования.
6. Обновить §6.8 (CLI), §7 (layout), §12.2 (tests), §14 (delivery status) в PRD.md.

Рекомендую делать 1–2 и остановиться на проверке: до записи в `~/.claude/settings.json` нужен ваш явный прогон `--dry-run`, потому что это глобальный конфиг, а не файл проекта.
