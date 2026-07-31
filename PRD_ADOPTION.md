# PRD addendum §16 — HZR как default entry point (замена RTK-хуков)

**Статус:** release candidate 0.2.0; hook/control-plane, durable PATH placement, Claude/Codex instructions, MCP migration и production user service реализованы; live deployment фиксируется отдельным audit record
**Родитель:** [PRD.md](PRD.md) · закрывает adoption, client MCP ownership и background service lifecycle
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
- **Деградация видима, а не молчалива.** Каждый degraded вызов инкрементит счётчик, `hzr doctor` показывает `degraded_rewrites=N`, и `hzr stats` помечает период как частично неучтённый. Это соблюдает §4.2: неучтённое не выдаётся за учтённое.
- **Никакого silent fallback на stock RTK.** Fallback идёт в `hzr rtk --`, то есть в тот же pinned fork-core `0.44.1-fork.1`. Запрет §1.1.8 не нарушается.
- **Timeout хука жёсткий** (предлагаю 2 s на probe демона): просроченный probe немедленно уходит в degraded, а не висит в терминале.
- Release installer ставит и запускает production user service (`launchd`/`systemd --user`) на stable `current/bin/hzrd`; foreground `hzr daemon serve` остаётся development-режимом.

**Цена решения, зафиксированная явно:** если service недоступен, rewrite сохраняет fork-вердикт, но не попадает в SQLite ledger. `doctor` и `stats` обязаны показать неполный accounting; installer восстанавливает service idempotent-командами `hzr daemon service install|restart`.

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
hzr install [--dry-run] [--force] [--prefix DIR] [--binary PATH]
            [--allow-dev-path] [--keep-external-icm] [--skip-instructions]
hzr uninstall [--keep-data] [--dry-run] [--force] # hooks + instruction blocks; RTK не восстанавливается
hzr hooks status [--json]              # hooks + instructions + PATH + foreign processes
hzr init --if-needed [--quiet]         # для SessionStart
```

`hzr install` выполняет адопцию целиком, в одном подтверждённом действии, по порядку
«binaries → hooks → instructions», чтобы hook-команда и `CLAUDE.md` ссылались на путь,
который к этому моменту уже существует:

1. **Durable binaries на PATH.** `hzr`/`hzrd` копируются в `--prefix` (по умолчанию `~/.local/bin`, где уже живут остальные движки), помечаются `0755`, и отсутствие prefix в `PATH` репортится отдельным warning и `doctor`-check `hzr_on_path`. Alias `rtk` на PATH **не** создаётся: это был бы второй entry point.
2. **Backup** каждого изменяемого файла с full-SHA в имени, тот же проверенный паттерн, что и migration (§11).
3. `--dry-run` — первоклассный режим: показывает `before_sha256`/`after_sha256`, backup path и целевой hook binary, не записывая ничего. Без `--force` запись отклоняется с указанием сначала посмотреть preview.
4. **Idempotent по всем трём поверхностям**: повторный запуск даёт `changed=false` для hooks, prefix и instructions, не создаёт второй backup и не дублирует записи.
5. **Обнаружить и заменить** rtk-записи (`rtk-rewrite.sh`, `rtk-mem-context.sh`, `rtk-block-native-explore.sh`, `rtk hook …`), сохранив исходник в backup.
6. **Централизовать memory ownership:** прямые `icm hook …` записи удаляются, потому что они пишут в store, который HZR не супервизирует, — то есть второй durable memory layer против §6.5. Осознанный отказ — `--keep-external-icm`; тогда `doctor` продолжает репортить дубль как `error`.
7. **Неизвестные сторонние handlers не трогаются никогда** — ни при install, ни при uninstall.

**Hook binary никогда не берётся из `current_exe()` наивно.** Установка из `cargo run` или из распакованного во временную директорию bundle привязала бы hook к `target/debug/hzr` или к пути, который исчезнет, — и тогда ломается каждая Bash-команда. Поэтому hook всегда именует durable копию в prefix; `--binary` задаёт путь явно; `target/debug|release` отклоняется с диагностикой, а `--allow-dev-path` оставлен только для разработки. Путь канонизируется, поэтому symlink или `..` не могут спрятать build-директорию.

Реализованный matcher учитывает современный Claude Code `Agent` и legacy `Task`: один `PreToolUse:Bash|Agent|Task` handler вызывает скрытый `hzr hooks dispatch`. Fork exit `0/1/2/3` не возвращается как process exit hook-а: dispatcher преобразует его в typed `allow/ask/deny` JSON при exit 0, как требует Claude hook protocol. Managed probe ограничен 2 s; fallback использует `PinnedRtkAdapter` той же версии `0.44.1-fork.1`.

### 16.5.1 Foreign engine processes

`doctor` и `hooks status` перечисляют `icm serve` и `grepai watch`, запущенные вне HZR data root, и репортят их как `error`: несколько `icm serve` означают несколько писателей в memory store, а сторонний `grepai watch` пересканирует дерево, которым HZR уже владеет. Процессы внутри HZR data root распознаются как свои и не считаются чужими.

**HZR их не останавливает.** Автоматическое завершение внешних процессов запрещено §4.3 и §11: неверно убитый watcher теряет in-flight состояние индекса. Отчёт — обязателен, остановка — решение пользователя.

Реализованный matcher учитывает современный Claude Code `Agent` и legacy `Task`: один `PreToolUse:Bash|Agent|Task` handler вызывает скрытый `hzr hooks dispatch`. Fork exit `0/1/2/3` не возвращается как process exit hook-а: dispatcher преобразует его в typed `allow/ask/deny` JSON при exit 0, как требует Claude hook protocol. Managed probe ограничен 2 s; fallback использует `PinnedRtkAdapter` той же версии `0.44.1-fork.1`.

## 16.6 Инструкции агентов

Fork-core уже содержит готовый образец обеих поверхностей: `hooks/rtk-awareness.md` (Claude), `hooks/rtk-awareness-codex.md` (Codex), `hooks/rtk-instructions.md`. HZR-аналоги делаются по тому же разделению, **без копирования RTK-текста дословно** — команды другие.

| Артефакт | Назначение |
|---|---|
| `HZR.md` | Канонический контракт инструмента: HZR-owned read/search/write/exec/memory/context paths, когда raw, когда exact |
| `integrations/claude-code/hzr-awareness.md` | Claude Code: приоритет `hzr` над нативными Read/Grep/Edit |
| `integrations/claude-code/hzr-awareness-codex.md` | Codex: тот же контракт в его формате инструкций |
| Ссылка в `~/.claude/CLAUDE.md` | ✅ реализовано: `install` вставляет managed-блок с `@<abs>/HZR.md` и снимает legacy `@RTK.md` |
| Блок в `~/.codex/AGENTS.md` | ✅ реализовано: тот же контракт в форме, которую Codex читает буквально (у него нет `@import`) |

**Как это устроено, чтобы не портить пользовательские файлы.** HZR владеет ровно одним
делимитированным блоком `<!-- hzr:begin … -->` / `<!-- hzr:end … -->` и никогда не
перезаписывает файл целиком. Пользовательский текст остаётся сверху, managed-блок
добавляется последним. Legacy-импорт снимается только при совпадении **всей строки**
(`@RTK.md`), поэтому проза, которая просто упоминает `RTK.md`, сохраняется. Незакрытый
маркер не приводит к обрезке содержимого. `uninstall` снимает блок и восстанавливает
исходное тело: оставленная инструкция продолжала бы требовать `hzr` после снятия hooks.
Путь до `HZR.md` абсолютный, потому что относительный `@`-импорт не выживает перемещение
bundle. Мутации идут через тот же backup/CAS/atomic путь, что и `settings.json`, но с
отдельным lock-файлом на каждый целевой файл.

Единый контракт для обоих агентов (замена текущей RTK-таблицы):

```text
Read     → hzr rtk -- read <file>
Grep     → hzr rgai "<intent>" | hzr search <pattern>
Edit     → hzr rtk -- write patch|replace|set ...
Bash     → перехватывается hzr hooks dispatch автоматически
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
- fallback уходит в pinned fork `0.44.1-fork.1`, а не в stock RTK (версионный ассерт);
- hook binary отклоняется при `target/debug|release` и при несуществующем пути; `--allow-dev-path` его разрешает;
- `install` в prefix idempotent, ставит exec-бит и не перезаписывает уже совпадающий байт-в-байт бинарь;
- `rtk` alias **не** появляется на PATH;
- managed-блок instruction-файла idempotent, снимает `@RTK.md` только как целую строку, сохраняет прозу с упоминанием `RTK.md`, не обрезает содержимое при незакрытом маркере, и `uninstall` восстанавливает исходное тело;
- Codex-поверхность использует литеральную ссылку, а не `@import`;
- foreign-scan отличает процессы внутри HZR data root от чужих и никогда не репортит собственный вызов.

## 16.7.1 Что сознательно не делается: глобальный перехват codec

Требование «Caveman codec перехватывает глобальные запросы и ответы Claude/Codex»
**не реализуется**, и не из-за объёма работы:

1. **Технически невозможно через hooks.** Claude Code не предоставляет hook, который
   переписывает payload, уходящий провайдеру, или ответ модели. `UserPromptSubmit` умеет
   *добавлять* контекст, `Stop` умеет блокировать остановку — ни один не даёт mutable
   provider request/response. Перехват существует только там, где запрос действительно
   проходит через HZR, то есть в `hzr agent run`.
2. **Противоречит собственной доказательной базе PRD.** §3.4 фиксирует, что input
   compression в среднем **повышает** стоимость и снижает accuracy (CAVEWOMAN), а §4.3
   прямо объявляет non-goal сжатие reasoning провайдера. Глобальное сжатие входа Claude
   было бы реализацией того, что PRD уже отверг на данных.

Реализуемая и реализованная часть — **instruction-level density contract**: managed-блок
в `CLAUDE.md`/`AGENTS.md` задаёт плотность ответа до генерации, что и есть тот же приём,
которым managed bridge пользуется через `appendSystemPrompt`. Это влияет на output-токены
без lossy-переписывания и без второго прохода.

## 16.8 Риски

| Риск | Митигация |
|---|---|
| Двойной rewrite RTK+HZR | §16.2 инвариант + `doctor` error + замена записей при install |
| Хук ломает терминал при мёртвом демоне | hybrid fallback (§16.3) + жёсткий timeout |
| Молчаливо неполный ledger | Счётчик degraded + отражение в `doctor`/`savings` (§4.2) |
| Повреждение `settings.json` | Full-SHA backup + `--dry-run` + diff-подтверждение (паттерн §11) |
| Рост mutation surface против §14 | Ограничить mutations transaction-safe adoption, client-config migration и platform service lifecycle; каждый шаг имеет dry-run/backup либо idempotent service semantics |
| Дубль memory-слоя с внешним `icm hook` | `install` централизует ICM по умолчанию; `--keep-external-icm` — явный opt-out, который остаётся видимым в `doctor` |

## 16.9 Delivery status

1. ✅ `hzr hooks dispatch` + hybrid daemon→pinned-fork path.
2. ✅ `hzr install/uninstall/hooks status` с backup, dry-run, centralized ICM и заменой RTK-записей.
3. ✅ `hzr init --if-needed` + регистрация `SessionStart`.
4. ✅ `HZR.md` + Claude/Codex managed blocks с backup/CAS/atomic mutation.
5. ✅ Contract tests §16.7 и `doctor`-проверка ownership/conflicts.
6. ✅ Direct ICM registrations в Codex/Claude Desktop транзакционно заменяются на `hzr mcp serve` с backup/CAS.
7. ✅ Production user service устанавливается на stable `current/bin/hzrd`; lifecycle доступен через `hzr daemon service`.
8. ✅ Родительский PRD и release documentation синхронизированы с 0.2.0.

Standalone adoption по-прежнему начинается с `hzr install --dry-run` и требует явный `--force`. Repository-level release installer выполняет эту подтверждённую стадию по умолчанию; `HZR_INSTALL_HOOKS=0` устанавливает bundle без изменения hooks/instructions.
