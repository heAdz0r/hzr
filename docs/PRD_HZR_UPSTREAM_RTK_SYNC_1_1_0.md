# HZR ← upstream RTK v1.1.0 / develop: выборочная синхронизация fork-core — PRD

Статус: **утверждено владельцем 2026-11-05 (P0 + P1 + US-014), реализовано, не закоммичено.** §14 — что сделано
и где реализация отличается от плана; §15 — обновление grepai и ICM (просьба владельца).
Владелец: HZR control plane
HZR: 0.11.5 (`06b0e17`), fork-core `rtk 0.50.0-fork.1`
Upstream: `rtk-ai/rtk` tag `v1.1.0` (`753a3b0e`, 2026-10-25) + `develop` @ `690616d4` (2026-11-05, `1.2.0-rc.615`)
Предыдущая синхронизация: [`PRD_HZR_UPSTREAM_RTK_SYNC_0_50_0.md`](PRD_HZR_UPSTREAM_RTK_SYNC_0_50_0.md)
(0.11.0, upstream `v0.50.0` `1d87b8e`).

Это не merge. Как и в прошлый раз, каждый пункт — повторная реализация в плоском `src/*.rs`
нашего форка со своими тестами; тесты upstream переносятся как спецификация.

---

## 1. Итог

С `v0.50.0` upstream выпустил 0.51.0, 0.52.0, 1.0.0, 1.0.1, 1.1.0 и ещё 106 коммитов в `develop`:
**396 коммитов без merge, 559 файлов, +127k/−25k строк.** Большая часть объёма — новый
pipeline-движок, LLM-прокси и хосты агентов. Для нас это не нужно, а частично и юридически
недоступно (§2).

Выводы, которые определяют этот PRD:

1. **Permission gate fork-core пропускает deny-правила, а хук HZR превращает это в
   автоодобрение. Воспроизведено end-to-end через `hzr hooks dispatch`** (§3, P1–P9). При
   `deny: Bash(git push:*)` HZR отвечает `permissionDecision: "allow"` на `timeout 30 git push`,
   `env FOO=1 git push` и `ls -la $'\'' ; rm -rf /`. В режиме `bypassPermissions` то же самое
   происходит с `HUSKY=0 git push`, `nohup git push` и `bash -c 'git push'`. Хост получает
   переписанную команду (`… rtk git push`), которую его deny-правило уже не узнаёт. Upstream закрыл
   этот класс в 0.51–1.1: наш `permissions.rs` — 387 строк, у upstream — 2701.
2. **`cat` на не-UTF-8 и CRLF-файлах портит байты** (§3, F1–F2). Хук переписывает `cat` в
   `rtk read`, а тот выдаёт `\xe9` → `U+FFFD` и `\r\n` → `\n`. В 0.11.0 мы починили только
   `head`/`tail`. Upstream исправил это в `95452556` и `e5c79f48`.
3. **`--help` у обёрнутых команд показывает справку rtk, а не инструмента** (§3, F3). Агент,
   запросивший `git log --help`, получает «One-line commit history / Usage: rtk git log».
   Upstream: #4348 (`4f12b01`).
4. **Лицензия.** Начиная с 1.0.0 часть upstream распространяется под BUSL-1.1. Порт из этих
   каталогов запрещён (§2).

Рекомендуемый объём для **HZR 0.12.0 / fork-core `1.1.0-fork.1`**: P0 (US-001…US-006) +
P1 (US-007…US-009) + бамп версии (US-014). P2 — по отдельной отметке владельца.

---

## 2. Лицензионное ограничение (жёсткое)

| Каталог upstream | Лицензия | Что там | Решение |
|---|---|---|---|
| `src/`, `text/` | Apache-2.0 | CLI, фильтры, хуки, permission gate, lexer | Можно переносить с атрибуцией (NOTICE / `THIRD_PARTY_NOTICES.md`) |
| `engine/` (`rtk-engine`) | **BUSL-1.1**, Change Date 2030-09-09 | pipeline-движок, сжатие | **Не читать для реализации и не переносить** |
| `proxy/` (`rtk-proxy`) | **BUSL-1.1** | LLM-прокси-демон | **Не переносить** |
| `plugin/` (`rtk-plugin`) | **BUSL-1.1** | плагины | **Не переносить** |

Код в `src/` под `#[cfg(feature = "engine")]` (pipelines, `llm-proxy`, swap, pipeline-правила
хука) сам по себе Apache, но служит обвязкой BUSL-движка. Мы его тоже не берём (§7). Каждый
пункт PRD ссылается только на коммиты, которые меняют `src/`.

---

## 3. Доказательства (воспроизведено 2026-11-05 на установленном `rtk 0.50.0-fork.1` и `hzr 0.11.5`)

Фикстура: git-репозиторий с `.claude/settings.json` =
`allow: [ls:*, env:*, timeout:*, git status:*]`, `deny: [git push:*, rm -rf /]`.
Вход хука — `{"tool_name":"Bash","tool_input":{"command":…},"permission_mode":…}` →
`hzr hooks dispatch --native-mode steer`.

| # | Команда | Режим | Ожидается (хост / upstream) | HZR сейчас |
|---|---|---|---|---|
| P1 | `timeout 30 git push` | default | deny / ask | **allow** + `timeout 30 rtk git push` |
| P2 | `env FOO=1 git push` | default | deny / ask | **allow** |
| P3 | `ls; timeout 5 git push` | default | deny / ask | fork-core: `host_permission: allow` |
| P4 | `ls -la $'\'' ; rm -rf /` | default | ask (лексер не может это проверить) | **allow** |
| P5 | `HUSKY=0 git push` | bypass | deny | **allow** + `HUSKY=0 rtk git push` |
| P6 | `nohup git push` / `command git push` | bypass | deny | **allow** + `nohup rtk git push` |
| P7 | `bash -c 'git push'`, `env bash -c 'git push'` | bypass / default | deny / defer | **allow** + `bash -c 'rtk git push'` |
| P8 | `~/.claude/settings.json` с UTF-8 BOM и `deny git push` | любой | deny | правило молча пропущено → rewrite |
| P9 | `2>&1 git push --force`, `ls && { rm -rf / ; }` | default | deny | `default`; правило не применилось (сейчас спасает proxy/raw) |
| F1 | `cat latin.txt` (`caf\xe9`) | — | байты как есть | `caf\xef\xbf\xbd` |
| F2 | `cat crlf.txt` (`a\r\nb\r\n`) | — | байты как есть | `a\nb\n` |
| F3 | `git log --help`, `cargo test --help` | — | справка git / cargo | справка `rtk git log` / `rtk cargo test` |
| F4 | `ls -la  &&  ls` | — | пробелы автора сохранены | `rtk ls -la && rtk ls` (пробелы нормализованы) |

Не воспроизвелось (помечено **verify**, в план не входит): pytest E-lines / `--tb=no` /
setup errors (наш вывод на фикстуре полон), `bun test --coverage|--bail` (совпадает с
нативным), `ls` bare/long/rejected (совпадает с нативным), `git log --bogus` (ошибка git
сохранена).

Как воспроизвести: скрипты `probe.sh`, `p1.sh` и `p2.sh` из сессии 2026-11-05. При
реализации они переносятся в `scripts/upstream-parity-probe.sh` и
`crates/hzr-cli/tests/bash_hook_permissions.rs` (US-005).

---

## 4. Цели

- Ни одна команда, которую deny/ask-правило хоста остановило бы в исходном виде, не получает
  от хука HZR `allow` и не переписывается так, чтобы правило перестало её узнавать.
- `cat`/`rtk read` побайтово совпадает с нативным выводом на не-UTF-8 и CRLF.
- `-h`/`--help`/`help` обёрнутой команды возвращают справку самого инструмента.
- Не регрессировать то, в чём HZR впереди: anti-evasion E1–E10, typed attribution,
  host-verdict 0.9.4, bounded pipeline plan, окна `head`/`tail` с раскрытием.

## 5. User stories

### P0 — безопасность permission gate (всё воспроизведено)

#### US-001: deny/ask видят команду за присваиваниями и обёртками (P1–P3, P5, P6)
Upstream: `b465deff` (#4281), часть `05e7f801`.
- Для deny и ask проверять все «прочтения» сегмента (`matching_readings`): каждое `VAR=x`,
  `env [VAR=x…]`, `timeout N`, `time`, `nice`, `nohup`, `stdbuf`, `command`, `builtin`,
  `noglob`, `exec`, голый `xargs`, а также наши `transparent_prefixes`. Снимать по одному
  префиксу и хранить каждое промежуточное прочтение.
- Allow по-прежнему проверяется только на исходном сегменте (иначе `{ git status` унаследует
  `git:*`).
- Если префиксов больше 32 и существует хоть одно deny/ask-правило, вердикт не мягче Ask.
- Таблица только для сопоставления (`stdbuf`, `xargs`) отделена от таблицы перезаписи:
  узнать команду не значит переписать её.
- **Критерии:** P1, P2, P3, P5, P6 → `Deny`. `git -C . push` остаётся `git` (аргументы
  программы не разбираются). Тесты upstream из `b465deff` перенесены как спецификация.

#### US-002: deny внутри `sh|bash|zsh -c '…'` (P7)
Upstream: permission-часть `7cc2699a`, `4d37aa8b`, `c2c75959`, `b4bf59c2`.
- Перезапись `-c`-скриптов у нас уже есть (E2). Не хватает гейта: deny-правила должны
  проверяться на сегментах внутреннего скрипта на любой глубине префиксов, которую достигает
  перезапись (включая `uv run`, `env`, `direnv exec`).
- Если внутренний deny срабатывает → `Defer`: команда не переписывается, решения нет, хост
  видит её в исходном виде. Это поведение upstream.
- Переписанная обёртка никогда не получает `Allow`; сильнейший вердикт — `Ask`, потому что
  `*`-allow не должен одобрять скрипт, который хост не разбирал.
- **Критерии:** `bash -c 'git push'`, `env bash -c 'git push'`, `bash -c 'timeout 5 git push'`
  дают defer или deny и никогда allow ни в каком режиме.

#### US-003: дыры сегментации лексера (P4, P9)
Upstream: `3992e0ab`, `3c2b4d0a`.
- ANSI-C `$'…\'…'`: если `ansi_c_quote_defeats_lexer` срабатывает, это должно вести к
  `contains_unattestable_construct` → вердикт не мягче Ask. `QuoteScan` уже есть (0.5.0).
- Ведущий редирект (`2>&1 git push`): перешагивать редирект и его файловый операнд до
  первого слова команды.
- Остатки грамматики `{ } ! ( )`: deny и ask проверяют сегмент с этими словами, снятыми в
  начале; allow — нет.
- Heredoc (`<<`, `<<-`, `3<<`) никогда не освобождается правилом `/dev/null`.
- **Критерии:** P4 → Ask, P9 → Deny. Тест из `3c2b4d0a` (7 форм heredoc).

#### US-004: настройки — fail closed (P8)
Upstream: `e27b47d1`, `455ea7fa`, плюс `CLAUDE_CONFIG_DIR` (`f4404cb`, 0.47).
- Файл настроек существует, но не читается или не парсится → добавить catch-all Ask и
  предупреждение, а не пропускать файл.
- BOM снимать до разбора; `NotADirectory` и пустой файл не считать ошибкой.
- Путь пользовательских настроек: `$CLAUDE_CONFIG_DIR` раньше `~/.claude` (сейчас
  `permissions.rs:125` знает только home; HZR в остальных местах уже учитывает переменную).
- **Критерии:** P8 → Deny; файл `{bad` → Ask на любую команду; пустой файл → без эффекта.

#### US-005: страховка на стороне HZR и сквозная регрессия
Без порта upstream: это наш слой.
- `hook_runner.rs`: при `host_grants_execution` (bypass) переписанная команда с `allow` уходит
  только если fork-core вернул `Allow` или `Default` и **в настройках нет ни одного
  deny/ask-правила, которое совпало бы с любым прочтением** (вердикт fork-core после
  US-001…US-004). При `Defer` — raw без решения.
- Новый сквозной набор в `crates/hzr-cli/tests/bash_hook_permissions.rs`: матрица
  P1–P9 × {`default`, `acceptEdits`, `bypassPermissions`}; утверждение — «нет `allow` и нет
  переписанной формы для любой команды, которую deny-правило узнаёт в исходном виде».
- Набор `scripts/smoke-install.sh` — обязательно (меняется поведение хука).

#### US-006: `cat`/`rtk read` — сырые байты для не-UTF-8 и CRLF (F1, F2)
Upstream: `95452556`, `e5c79f48`.
- Plain-чтение (без фильтра/окна) невалидного UTF-8 → вывод исходных байтов. Stdin проходит
  через тот же байтовый гейт.
- CRLF сохраняется побайтово в plain-режиме (сейчас теряется и на валидном UTF-8).
- `-n` (нумерация) на не-UTF-8 → нативный `cat -n` (`passthrough`), а не lossy.
- Учёт: такие чтения записываются как passthrough 0/0, а не как «экономия».
- **Критерии:** F1, F2 и `cat - < latin.txt` побайтово равны нативным (`od -c | md5`); кейсы
  добавлены в `upstream-parity-probe.sh`.

### P1 — корректность

#### US-007: справка принадлежит инструменту (F3)
Upstream: #4348 (`4f12b01` и связанные).
- `-h`, `--help` и `help` после обёрнутой подкоманды (`rtk git log --help`, `rtk cargo test
  --help`, `rtk go test -h`, `rtk pytest --help`, …) исполняют нативный инструмент с исходным
  argv (passthrough, учёт 0/0). Справка самого rtk — только для `rtk --help` и
  `rtk <cmd>` без аргументов инструмента, если такой случай вообще нужен.
- Реализация — одна проверка в диспетчере (`main.rs`) до разбора clap для модулей-обёрток, а
  не 18 файлов, как у upstream.
- **Критерии:** вывод `--help` для git/cargo/go/pytest/bun/npm/gh совпадает с нативным;
  `Cli::command().debug_assert()` проходит.

#### US-008: перезапись не трогает текст между сегментами (F4)
Upstream: `05e7f801`, `5e92ea47` (#3197).
- Сегмент выводится как свой ведущий пробел + перезапись + свой хвостовой пробел; оператор
  копируется из исходника байт в байт (включая `;;`, `;&`, `;;&`). То же для префиксов обёрток
  (`uv run  ls`) и для `|` в pipeline-перезаписях.
- Сейчас `case` уходит в proxy (безопасно, но без экономии). После правки он переписывается,
  а терминаторы остаются целыми.
- **Критерии:** 7 примеров из сообщения `05e7f801` дают ту же форму; `case x in a) ls;; esac`
  переписывается и проходит `bash -n`.

#### US-009: отдельные проверки (verify → port только при воспроизведении)
Каждый пункт сначала воспроизводится; если не воспроизвёлся, фиксируем это в §14.
- `git` в управляемых worktree Claude Code (`2fdcaa7d`, #3864): воспроизвести в сессии с
  `isolation: worktree` (`<repo>/.claude/worktrees/<name>`). Если guard Claude Code
  отклоняет управляемую форму, исключить `git` из перезаписи для такого cwd.
- `git log`: уведомление о капе только на реальном обходе, а не на ошибке использования
  (`5db646b3`, `16a20a2e`, git 2.56). Наша заметка о капе из 0.11.0 — сверить на git 2.52 и 2.56.
- `go test` в pipe-режиме: события в кавычках, отчёты не в JSON (`139618d8`, `37fcaa67`,
  `fb79b81d`).
- `cargo nextest`: только последняя попытка повторённых тестов, таймауты, slow
  (`6a6a9900`, `aff0fcc3`, `57fc55c1`) — HZR сам на Rust; nextest не установлен, нужна фикстура.

### P2 — экономия и новые возможности (решение владельца)

#### US-010: свёртка общего префикса путей в списках файлов
Upstream: #4170 (`feac25d1`), `3223a80`. `rg --files`, `rg -l`, `grep -rl` — частые вызовы
агентов; сейчас это passthrough. Если строка не является простым путём, вывод остаётся
дословным. Ожидание upstream — без чисел HZR; замерить на `hzr` и `anonymous_bot`.

#### US-011: `git ls-tree --name-only` — свёртка по каталогам
Upstream: `618ca7f6` (+`41537ad0`). Сейчас эта команда уходит в proxy. 1394 строки у upstream;
брать, только если ledger показывает частые вызовы.

#### US-012: `rtk json --keys-only`
Upstream: `db57e126`, `dadc712f`, `0e596d13`. У нас флага нет (clap-ошибка → raw).
Кандидат для `jq`-маршрута, если владелец хочет схему JSON вместо значений.

#### US-013: fish `-c` и fish-скрипты
Upstream: `4d2184c9`, `32b2ec44` + раунды ревью (`03a57826`, `1f4d1107`). В стеке владельца
fish не установлен. **Не брать**, если нет запроса.

### US-014: бамп версии fork-core → `1.1.0-fork.1`
- `Cargo.toml`, `CURRENT_ENGINE.toml` через `scripts/refresh-current-engine.sh` (дайджесты
  только вычисляются), `FORK_PARITY.md` (раздел «1.1.0 upstream sync delta»),
  `THIRD_PARTY_NOTICES.md` (явно: перенесено только из Apache-2.0 `src/`).
- Неизменяемый baseline (`SNAPSHOT*`, `FORK_FILES`, `SHA256SUMS`, `TRACKED_*`) не трогаем.

---

## 6. Функциональные требования

- FR-1: deny/ask проверяют каждое прочтение сегмента после префиксов; allow — только сегмент
  как он написан (US-001).
- FR-2: deny во внутреннем `-c`-скрипте → `Defer`; переписанная обёртка не получает `Allow`
  (US-002).
- FR-3: непроверяемые конструкции (ANSI-C с `\'`, heredoc) → вердикт не мягче Ask (US-003).
- FR-4: нечитаемые/битые настройки → catch-all Ask; BOM снимается; `CLAUDE_CONFIG_DIR`
  учитывается (US-004).
- FR-5: хук HZR не выдаёт `allow` с переписанной формой для команды, которую deny/ask-правило
  узнаёт, ни в одном режиме разрешений (US-005).
- FR-6: plain `rtk read` побайтово равен `cat` для любого входа (US-006).
- FR-7: `-h|--help|help` обёрнутой команды → нативный инструмент (US-007).
- FR-8: перезапись сохраняет байты между сегментами (US-008).

## 7. Не берём

| Upstream | Почему нет |
|---|---|
| Pipeline-движок, `rtk pipeline`, swap, pipeline-правила хука, `rtk-engine` (`62e9047`, `5d8102a`, `380a804`, все `fix(pipeline)` в develop) | **BUSL-1.1** (§2); кроме того, HZR сам является control plane |
| `rtk llm-proxy`, `rtk-proxy`, автостарт прокси, `RTK_PROXY_BIND` (`8420c11`, `f53e603`, `496a8c7`, `49087db`) | **BUSL-1.1**; HZR не проксирует LLM-запросы |
| Переименование `rtk proxy` → `rtk bypass`, удаление `--ultra-compact` (1.0.0 BREAKING) | HZR эмитит `rtk proxy` в маршрутах; переименование ничего не даёт. `--ultra-compact` у нас уже только длинный флаг |
| Хосты: OpenCode V2/Desktop, Antigravity, Hermes, OpenClaw, Mistral Vibe, `permissions_opencode.rs` | Установщиком и диспетчером владеет HZR |
| `rtk gain`, analytics, ccusage, tracking (`fab1f8cb`, `f3e6130c`, `2ba9956`, `7a5eded7`, …), telemetry, discover-аналитика | Учёт ведёт ledger HZR |
| Recall store (64-битный хэш `6c1af5f`) | Восстановлением владеет HZR (tee + `hzr exec` raw) |
| Hook integrity для переписанных команд (`1b47098`, `01738da`), `rtk hook check` | Целостностью владеет HZR (0.4.x) |
| `rtk test`/`run`: границы argv, implicit shell (0.51 BREAKING `c298de6`) | Уже есть (#3596, 0.5.0) |
| `curl` stdin, один завершающий перевод строки, `diff` exit 2 | Уже взято в 0.11.0 (#4085, #4154, нативный `diff`) |
| Angular `ng`, dotnet, sqlfluff, mvn, ruff-раунды, Windows/MinGW | Вне стека владельца или не воспроизводится |

## 8. Технические соображения

- **Объём P0 у upstream:** permission gate — около 2,3 тыс. строк изменений (`3992e0ab` +434,
  `b465deff` +486, `4d37aa8b` +542, `e27b47d1` +117, `7cc2699a` +783, частично). Порт
  делаем в наш `permissions.rs` и `discover/lexer.rs`, без структуры `src/hooks/` upstream.
  Оценка: 1,2–1,6 тыс. строк, из них около половины тесты.
- **Хук.** US-001…US-005, US-008 меняют решения перезаписи → `scripts/smoke-install.sh`
  локально обязателен (память: *smoke-install gates hook behaviour*).
- **Anti-evasion.** E2 (shell wrapper) и E5 (pipeline/redirect) затронуты US-002/US-003/US-008;
  фикстура 97 кейсов должна остаться зелёной. Новые кейсы P1–P9 добавить туда же.
- **Ledger delta.** US-005 и US-006 переводят часть вызовов в raw/defer, поэтому измеренная
  экономия на них упадёт. Это правильное направление; объяснить в release notes до публикации
  цифр.
- **Clippy ratchet** 137 не растёт; новый код без предупреждений.
- **Общее дерево.** Параллельные сессии правят checkout → `git add` только явными путями
  (память: *shared tree*). Сейчас в дереве есть чужие незакоммиченные правки
  (`CHANGELOG.md`, `service.rs`, `build-bundle.sh`, `smoke-install.sh`); их не трогаем.

## 9. Порядок реализации

1. US-004 → US-003 → US-001 → US-002 (каждый шаг сужает вердикт; тесты upstream как спецификация).
2. US-005: сквозная матрица (сначала красная на текущем коде, потом зелёная).
3. US-006, US-007, US-008.
4. US-009: только воспроизведение; порт по результату.
5. US-014 + gate (§10). P2 — только по отметке.

## 10. Метрики успеха и gate

- Матрица P1–P9 × 3 режима: 0 случаев `allow` с переписанной формой для команды под deny.
- `scripts/upstream-parity-probe.sh`: 22 → не меньше 27 кейсов (F1, F2, stdin, F3 ×2), все ok.
- `scripts/verify-fork-core.sh --test` зелёный; `cargo clippy -D warnings` для `crates/*`;
  ratchet fork-core 137.
- `scripts/smoke-install.sh` зелёный.
- Ревью `hzr-rust-reviewer` перед релизом.

## 11. Чек-лист утверждения

| Блок | Пункты | Рекомендация |
|---|---|---|
| P0 безопасность | US-001, US-002, US-003, US-004, US-005 | **Взять** — воспроизведённый обход deny |
| P0 верность | US-006 | **Взять** |
| P1 | US-007, US-008 | **Взять** |
| P1 verify | US-009 | Воспроизвести; порт по результату |
| P2 | US-010 (свёртка путей) | Взять, если владелец хочет экономию на `rg --files`/`grep -l` |
| P2 | US-011, US-012 | По желанию |
| P2 | US-013 (fish) | Не брать |
| Версия | US-014 | **Взять** вместе с P0 |

## 12. Открытые вопросы

- Q1: US-005 в режиме bypass. Вариант «raw без решения» при любом совпавшем deny/ask безопаснее,
  но стоит экономии на этих командах. Рекомендация — да: deny-правило важнее экономии.
- Q2: Версионировать как `1.1.0-fork.1` (точка отсчёта — тег) или `1.2.0-rc-fork.1` (берём фиксы
  из develop)? Рекомендация — `1.1.0-fork.1` с явным списком коммитов develop в
  `FORK_PARITY.md`.
- Q3: Выпускать P0 отдельным security-релизом HZR 0.11.6 раньше P1/P2? Рекомендация — да.

## 13. Честные границы

- Отсутствие P9-эксплойта в HZR сейчас случайно: команда уходит в proxy/raw, и решение
  принимает хост. После US-008 (больше перезаписей) это станет реальным путём, поэтому US-003
  должен войти не позже US-008.
- Поведение Claude Code при `allow` из хука против собственных deny-правил хоста здесь не
  проверялось. Анализ исходит из худшего случая: переписанная форма `… rtk git push` не
  совпадает с `git push:*`, независимо от того, применяет ли хост deny поверх `allow`.
- Цифры экономии upstream (US-010, US-011) — заявления upstream, не замеры HZR.

---

## 14. Реализация и корректировки (2026-11-05)

Утверждённый объём: P0 (US-001…US-006), P1 (US-007…US-009), US-014. P2 не брали.
Ответы на §12: Q1 — да (raw при ask-правиле оператора в bypass), Q2 — `1.1.0-fork.1`, Q3 — решает
владелец при релизе.

Gate: `scripts/verify-fork-core.sh --test` зелёный (база `f4296ec4…`, текущий движок пересчитан
`refresh-current-engine.sh`), 1895 unit + все интеграционные наборы fork-core, новый
`tests/permission_gate_matrix.rs` (6), `scripts/upstream-parity-probe.sh` 22 → 27/27,
`cargo clippy --workspace --all-targets -D warnings` чистый. Clippy ratchet fork-core 137 → 134:
три dead_code ушли, потому что гейт теперь использует `split_for_permissions` и
`contains_unattestable_construct` лексера. Новых предупреждений нет.

| US | План | Сделано | Почему иначе |
|---|---|---|---|
| US-001 | Прочтения upstream (`matching_readings`) | `permissions.rs`: присваивания, `env [-i/-u/--]`, `execution_prefix` (timeout/nice/nohup/time/stdbuf), keyword-префиксы, **настроенные `transparent_prefixes` и `uv run`**, плюс `sudo` и голый `xargs` (только для сопоставления); 32 снятия, после — Ask | upstream не снимает transparent-префиксы, «потому что хост их не снимает». У HZR управляемая форма непрозрачна для правил хоста, поэтому всё, через что смотрит перезапись, обязан видеть и гейт |
| US-002 | Внутренний deny → `Defer` | Внутренний deny → **`Deny`**; обёртка лишь ограничивает Allow до Ask, `Default` остаётся хосту | `Defer` в HZR означал бы raw без решения, а в bypass — автоодобрение. Отдельно от upstream: обёртка без совпавшего правила не поднимается до Ask, иначе auto-режим получал бы навязанные промпты |
| US-003 | — | Как в плане; сегментатор гейта заменён на `lexer::split_for_permissions` (раньше был свой, только по кавычкам: не делил по `\n`, `&`, `(`) | — |
| US-004 | — | Как в плане | — |
| US-005 | Защита в bypass + матрица | `keep_operator_ask_rules`: bypass + `host_permission = ask` → raw без решения. Матрица — `tests/permission_gate_matrix.rs` (реальный бинарь) + обновлённый `bash_hook_permissions.rs` | После US-001…US-004 deny приходит из гейта как `Deny`; оставшаяся дыра — ask-правило, которое bypass превращал в allow |
| US-006 | Сырые байты для plain-чтения | Гейт `needs_exact_bytes`: невалидный UTF-8 **или CR** → исходные байты (файл и stdin); `-n` → формат `cat -n` побайтово; окно по умолчанию для больших файлов сохраняется, подсказка уходит в stderr | upstream чинил только `--level none`; наш `cat` → `rtk read` идёт через minimal |
| US-007 | Проверка в диспетчере | `help_handoff` в `main.rs`: `--help`, `help` сразу после подкоманды, `-h` (кроме tar/psql/sqlite3/ssh) → `rtk proxy <tool> …` для 24 подкоманд, которые запускают программу напрямую | Обёртки пакетных раннеров (`tsc`, `vitest`, …) не трогаем: их программу ищет сам rtk |
| US-008 | — | `splice()` в `rewrite_compound` и в обоих pipeline-переписчиках: пробелы, переводы строк и операторы (`;;`, `&&\n`) копируются из исходника | Тест `cross_line_operators_join_before_rewrite` закреплял нормализацию — ожидание обновлено, как у upstream |
| US-009 | Воспроизвести | `git log --bogus` = нативный. Worktree-guard Claude Code, `go test` в pipe-режиме, nextest локально не воспроизводимы (нужна сессия с `isolation: worktree`, `cargo-nextest` не установлен) | Порт не делался |
| US-014 | Бамп | `1.1.0-fork.1` везде, где версия активна (контракт HZR, скрипты, lock, тестовые стабы); история (CHANGELOG, старые PRD) не тронута | — |

**Bundle и установка (2026-11-05, решение владельца: ICM отложить, собрать):** `build-bundle.sh`
(Bun 1.2.19 из официального релиза, SHA-256 сверен, глобальный Bun не тронут) прошёл все 10 этапов
со встроенным bundle-smoke; движки `rtk 1.1.0-fork.1`, `grepai 1.2.0`, `icm 0.10.61`.
`package-release.sh` → `scripts/smoke-install.sh … full`: clean-install smoke и upgrade smoke
пройдены. `npm audit --omit=dev --audit-level=high` для caveman-bridge чистый (3 moderate).

**Не связано с синхронизацией, найдено по ходу:** 16 тестов workspace (`hzr-core::billing`,
`hzr-cli::hook_runner` про цены, `hzr-daemon::api::dashboard_session_roi…`) падают по часам:
записи прайс-каталога `anthropic-claude-opus-5*` получены 2026-09-26 при `max_age_days=30`.
Требуется обновить каталог; в этой работе не трогалось.

---

## 15. grepai и ICM (просьба владельца 2026-11-05: «только то, что фиксит, чинит, ускоряет, улучшает»)

Оба движка собираются из закреплённого тега с нашими патчами, поэтому «взять только фиксы»
означает: тег с фиксами + проверка, что новые возможности инертны в конфигурации HZR, +
перебазирование или удаление наших патчей. Лицензии не менялись: grepai — MIT, ICM — Apache-2.0.

### 15.1 grepai 0.37.0 → 1.2.0 (`d86397d5`)

| Что приходит | Для HZR |
|---|---|
| 0.38: watcher не падает на всплеске событий (#326), без лишнего хвостового чанка (#333), батчи Ollama по 64 | Наш патч `event-queue-backpressure` вошёл в upstream — **удалён** |
| 0.39: правки во время скана не теряются (#328), Qdrant не переэмбеддит неизменённое | Фикс |
| 1.0: при правке файла переэмбеддятся только изменённые чанки (#342), индекс не устаревает после краша (#311), `index-meta.json` защищает от смешения векторов разных моделей (#139), boost `/src/` начал работать, `init --model` для ollama больше не игнорируется | Ускорение и фиксы. `index-meta.json` отсутствует у старых индексов, поэтому `CheckIndexMeta` их пропускает и при первом watch принимает без переиндексации (проверено) |
| 1.1: watcher видит dotfiles и custom extensions (#367), workspace не теряет перечанкованные чанки | Фиксы |
| 1.2: **security** — ключи провайдеров из окружения уходят только на официальный HTTPS-endpoint; `node_path` из конфига проекта больше не исполняет файл репозитория; один watch-демон на индекс (#357) | Фиксы. HZR по умолчанию работает с Ollama без ключа, так что миграция ключей нас не касается |
| AST-чанкинг (experimental, тег сборки `treesitter`), `grepai index`, MCP-поля | Не включаются: HZR собирает без `treesitter` и вызывает только `init/watch/search/version` |

Наш патч `disable-worktree-discovery` перебазирован как `1.2.0-disable-worktree-discovery.patch` и
покрывает теперь ещё и TUI-путь watch. Флаг остаётся, хотя upstream с 0.36 умеет то же через
`watch.discover_worktrees`: HZR не пишет в `.grepai/config.yaml` пользователя.

Контракт HZR проверен на собранном 1.2.0: `search --json` даёт те же поля
(`file_path/start_line/end_line/score/content`) и тот же хит, что 0.37.0 на одном индексе. Индекс,
построенный 0.37.0, 1.2.0 принял и дописал только изменённый файл (1 файл, 12 чанков) без полного
ре-эмбеддинга. `go test ./cli ./watcher` зелёный.

### 15.2 ICM 0.10.61 → 0.11.5 — проверено, **отложено** решением владельца до обновления CommandLineTools (SDK macOS 14.4+)

| Что приходит | Для HZR |
|---|---|
| 0.10.64: dedup больше не затирает разные воспоминания (потеря данных), FTS AND→OR, голодание recall под фильтром topic/project/keyword | Фиксы |
| 0.11.0: recall engine v2 (rank fusion, фильтры до отсечения, глубина ≥ 100) по умолчанию; store открывается без write-lock | Фикс + улучшение. Перекрывает наш патч `http-recall-filter-headroom` |
| 0.11.2: `Cargo.lock` синхронен с версией | Перекрывает наш патч `refresh-workspace-lock` |
| 0.11.4: security-обновления зависимостей; 0.11.5: MCP отвечает на заявленной версии протокола, скачивание моделей с Xet CDN | Фиксы |
| fastembed 4→6, модель по умолчанию → `multilingual-e5-large` (было `multilingual-e5-base`); извлечение фактов из разговора (opt-in); 3D-граф, Vibe/OpenCode init | Инертно для HZR: `icm_embeddings = false` → `--no-embeddings`; extraction opt-in; init HZR не вызывает |
| 0.11.0 BREAKING: `icm_memory_consolidate` требует `ids` | HZR consolidate не вызывает |

Из трёх наших патчей остаётся один: `exit-with-parent`, перебазирован как
`patches/icm/0.11.5-exit-with-parent.patch`.

**Сравнение на копии реальной БД HZR** (758 воспоминаний, `--no-embeddings`, оба бинаря на своих
копиях, скрипт сравнения из scratchpad сессии):

- Контракт HZR цел: `/health`, `/stats`, `/recall`, `/store`, CLI `recall/list/update/forget/store`
  отдают все поля, которые десериализует `MemoryRecord`.
- Never-worse по выдаче: на 8 запросах новый recall — надмножество старого (7/8 на первом:
  ранжирование), чаще находит больше: `topic=context-hzr-…` 0 → 5, `keyword=release` 0 → 1,
  `grepai watcher worktree` 2 → 8. Время 32 → 38 мс на 8 запросов.
- 0.11.5 мигрирует схему (`user_version` 0 → 1, новые таблицы); **откат безопасен** — 0.10.61 на
  мигрированной БД делает recall и list (759).

**Блокер локальной сборки:** `embeddings-static` в 0.11.5 тянет новый `ort`, которому нужен CoreML
API (`MLComputePlan`, `MLOptimizationHints`) из SDK macOS 14.4+. На этой машине CommandLineTools
с clang 14.0.3 и SDK максимум 13.3, поэтому линковка падает. CI (`macos-15`) соберёт. Без
эмбеддингов (`http-api,backend-sqlite`) 0.11.5 собирается и прошёл сравнение выше.

Если эмбеддинги в HZR когда-нибудь включат, модель стоит закрепить на `multilingual-e5-base`:
иначе 0.11.5 возьмёт e5-large и переэмбеддит память. Сейчас эмбеддинги выключены, и это не
делалось.
