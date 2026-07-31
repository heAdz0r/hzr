# PRD: Upstream PR Backports — Bug Fixes & Quality Improvements

## Introduction

Наш форк rtk накопил несколько известных багов и UX-проблем, которые уже исправлены в upstream open PRs. Цель — систематически портировать эти исправления, адаптируя их к нашей кодовой базе (которая опережает upstream по ряду модулей: bun, go, python, write-cmd, memory-layer).

Источник: анализ 43 открытых PR в rtk-ai/rtk (2026-02-25).

---

## Goals

- Устранить 3 критических бага, которые ломают реальное использование (`find`, playwright, `gh --json`)
- Исправить 3 заметных дефекта снижающих надёжность (`git show blob`, `find` native flags, Clap fallback)
- Улучшить 2 области UX/robustness (`git` global options, `proxy` streaming)
- Не сломать существующие 105+ unit-тестов и 69 smoke-тестов
- Каждое изменение покрыто тестами (TDD: Red → Green → Refactor)

---

## User Stories

### US-001: Fix `fi` shadowing `find` in registry (PR #246)
**Description:** As an rtk user, I want `rtk find` и `rtk discover` корректно обрабатывали команды `find`, чтобы они не игнорировались из-за совпадения префикса `"fi"`.

**Root cause:** `"fi"` и `"done"` в `IGNORED_PREFIXES` как bare-строки. `"find ...".starts_with("fi") == true` → все find-команды классифицируются как `Ignored`.

**Files:** `src/discover/registry.rs:427,430`

**Acceptance Criteria:**
- [ ] `"fi"` и `"done"` перемещены из `IGNORED_PREFIXES` в `IGNORED_EXACT`
- [ ] `classify_command("find . -name foo")` возвращает `Supported`, а не `Ignored`
- [ ] `classify_command("fi")` возвращает `Ignored` (bare keyword)
- [ ] `classify_command("done")` возвращает `Ignored` (bare keyword)
- [ ] 3 новых regression-теста в `registry.rs`
- [ ] `cargo test` — all pass

---

### US-002: Fix Playwright JSON parser (PR #193)
**Description:** As an rtk user, I want `rtk playwright test` корректно парсил реальный JSON-вывод Playwright, чтобы фильтрация результатов работала вместо постоянного EXIT 1.

**Root cause (3 бага):**
1. `duration: u64` → должно быть `f64` (Playwright шлёт `3519.703`)
2. `PlaywrightSuite.tests` → должно быть `specs: Vec<PlaywrightSpec>`
3. `playwright --reporter=json test` → должно быть `playwright test --reporter=json`

**Files:** `src/playwright_cmd.rs:18-55`

**Acceptance Criteria:**
- [ ] `PlaywrightStats.duration` типизирован как `f64`
- [ ] `PlaywrightSuite` содержит `specs: Vec<PlaywrightSpec>` вместо `tests`
- [ ] `PlaywrightSpec` содержит `ok: bool` и `tests: Vec<PlaywrightExecution>`
- [ ] `--reporter=json` вставляется после первого аргумента (субкоманды)
- [ ] Тест с реальной JSON-структурой Playwright проходит
- [ ] Тест с `duration: f64` (float) проходит
- [ ] `cargo test playwright` — all pass

---

### US-003: Fix `gh pr view --json` passthrough (PR #217 + #196)
**Description:** As an rtk user, I want `rtk gh pr view --json fields` и `rtk gh pr view --web` корректно пробрасывались напрямую в `gh`, чтобы не получать ошибку `Unknown JSON field: "--json"`.

**Root cause:** `view_pr()`, `view_issue()` безусловно берут `args[0]` как PR/issue number. При `--json` как первом аргументе он становится "номером", потом rtk добавляет свой `--json` → двойной флаг → ошибка gh.

**Files:** `src/gh_cmd.rs:132, 447, 512`

**Acceptance Criteria:**
- [ ] Добавлена функция `has_output_format_flags(args)` (аналог `should_passthrough_run_view`)
- [ ] Детектирует: `--json`, `--jq`, `--template`, `--web` как первый arg или любой arg
- [ ] `view_pr()` вызывает passthrough если `has_output_format_flags` → true
- [ ] `view_issue()` вызывает passthrough аналогично
- [ ] `rtk gh pr view 42` — по-прежнему использует RTK-фильтрацию
- [ ] `rtk gh pr view --json number,title` — passthrough к gh
- [ ] `rtk gh pr view 42 --json number,title` — passthrough к gh
- [ ] 5+ новых unit-тестов для `has_output_format_flags`
- [ ] `cargo test gh` — all pass

---

### US-004: Fix `git show rev:path` duplicate output (PR #248)
**Description:** As an rtk user, I want `rtk git show HEAD:src/file.rs` выводил содержимое файла ровно один раз без дублирования, чтобы можно было корректно пайпить вывод в `grep`.

**Root cause:** Blob-запросы (`rev:path`) не детектируются и попадают в diff-логику, которая выводит содержимое дважды. `println!("{}", stdout.trim())` также обрезает trailing newlines.

**Files:** `src/git.rs` (функция `run_show`)

**Acceptance Criteria:**
- [ ] Добавлена функция `is_blob_show_arg(arg: &str) -> bool` (содержит `:`, не начинается с `-`)
- [ ] При наличии blob-аргумента — строгий passthrough (`print!("{}", stdout)` без trim)
- [ ] `rtk git show HEAD:src/main.rs | grep fn` — ровно одно совпадение на строку
- [ ] Trailing newlines файла сохраняются точно
- [ ] Unit-тесты для `is_blob_show_arg`: ветки, пути, флаги форматирования
- [ ] `cargo test git` — all pass

---

### US-005: Fix `rtk find` native flag syntax (PR #211)
**Description:** As an rtk user, I want `rtk find . -name "*.rs" -type f` работал без ошибок, чтобы не переключаться на нативный `find` для стандартных операций.

**Root cause:** `Find` в `main.rs:249` использует именованные Clap-аргументы. `-name` раскладывается в short flags `-n,-a,-m,-e` → `unexpected argument '-n'`.

**Files:** `src/main.rs:249-261`, `src/find_cmd.rs`

**Acceptance Criteria:**
- [ ] `Find` переключён на `trailing_var_arg = true, allow_hyphen_values = true`
- [ ] Добавлена `parse_find_args()` детектирующая native vs RTK синтаксис
- [ ] `rtk find . -name "*.rs"` — работает, возвращает compact output
- [ ] `rtk find . -name "*.rs" -type f` — работает
- [ ] `rtk find . -maxdepth 2` — работает
- [ ] `rtk find . -iname "*.RS"` — case-insensitive match
- [ ] RTK-синтаксис `rtk find *.rs src -m 50` — по-прежнему работает (обратная совместимость)
- [ ] 15+ unit-тестов в `find_cmd.rs`
- [ ] `cargo test find` — all pass

---

### US-006: Graceful Clap parse fallback (PR #200)
**Description:** As an rtk user, I want неизвестные или нестандартные команды прозрачно проксировались напрямую, а не падали с Clap-ошибкой, чтобы rtk был всегда безопасным fallback.

**Root cause:** Clap не может разобрать некоторые команды (нестандартные флаги) → rtk возвращает ошибку вместо выполнения.

**Files:** `src/main.rs` (top-level error handling), `src/tracking.rs`

**Acceptance Criteria:**
- [ ] При Clap parse failure → выполняется raw command как passthrough
- [ ] RTK meta-команды (`gain`, `discover`, `init`, `proxy` и др.) защищены — их ошибки показываются пользователю
- [ ] Fallback-команды логируются в tracking с пометкой parse_failure
- [ ] Время выполнения fallback-команд корректно записывается в `rtk gain --history`
- [ ] `rtk git -C /path status` → выполняется как `git -C /path status`
- [ ] `rtk gain --badtypo` → показывает Clap error (не passthrough)
- [ ] `cargo test` + smoke tests — all pass

---

### US-007: git global options support (PR #192)
**Description:** As an rtk user, I want `rtk git --no-pager log`, `rtk git -C /path status` и аналогичные команды с глобальными git-опциями работали, чтобы Claude Code мог использовать любой стандартный git-вызов.

**Root cause:** Clap-определение `GitCommand` не знает о глобальных опциях git (`--no-pager`, `--git-dir`, `--work-tree`, `-C`, `-c`, `--no-optional-locks`, `--bare`, `--literal-pathspecs`).

**Files:** `src/main.rs` (GitCommand enum), `src/git.rs`

**Acceptance Criteria:**
- [ ] Value-taking global opts: `-C <path>`, `-c <key=val>`, `--git-dir <dir>`, `--work-tree <dir>`
- [ ] Boolean global opts: `--no-pager`, `--no-optional-locks`, `--bare`, `--literal-pathspecs`
- [ ] Глобальные опции prepend перед субкомандой при выполнении
- [ ] `rtk git --no-pager --no-optional-locks status` парсится корректно
- [ ] `rtk git -C /tmp log --oneline -5` — работает
- [ ] Unit-тесты для Clap parsing и `git_cmd()` с глобальными флагами
- [ ] `cargo test git` — all pass

---

### US-008: Streaming output in `rtk proxy` (PR #268)
**Description:** As an rtk user, I want `rtk proxy cargo build` показывал вывод в реальном времени, а не казался зависшим пока команда выполняется.

**Root cause:** `rtk proxy` использует `Command::output()` — буферизует весь вывод до завершения процесса. При долгих командах (build, test) — терминал молчит.

**Files:** `src/main.rs` или выделенный proxy-модуль

**Acceptance Criteria:**
- [ ] Proxy переключён с `Command::output()` на `Command::spawn()` + chunked read
- [ ] stdout и stderr форвардируются инкрементально (chunk за chunk)
- [ ] Каждый chunk сразу флашится в stdout/stderr
- [ ] Captured output для tracking/analytics сохраняется (join chunks)
- [ ] Exit code дочернего процесса сохраняется и проксируется
- [ ] `rtk proxy cargo build` — вывод появляется по мере компиляции
- [ ] `cargo test` — все существующие тесты pass

---

## Functional Requirements

- FR-1: `classify_command("find . -name foo")` → `Supported` (не `Ignored`)
- FR-2: `rtk playwright test` корректно парсит JSON с `f64` duration и `specs`-структурой
- FR-3: `rtk gh pr view --json fields` → passthrough к `gh` без ошибки
- FR-4: `rtk git show HEAD:file.rs` → single output без дублирования, trailing newlines сохранены
- FR-5: `rtk find . -name "*.rs" -type f` → compact output (не ошибка Clap)
- FR-6: Неизвестные команды → graceful passthrough (не падение rtk)
- FR-7: `rtk git --no-pager log` → корректное выполнение с глобальными git-опциями
- FR-8: `rtk proxy <cmd>` → streaming вывод в реальном времени

## Non-Goals

- Не портируем PR #241 (`rtk rewrite`) — наш хук сложнее и несовместим с упрощённой моделью
- Не портируем PR #234 (git exit codes) — уже исправлено в нашем форке
- Не портируем PR #128 (per-project gain) — уже реализовано нами
- Не добавляем новые языки/тулчейны (Gradle, Maven, .NET, RSpec) — вне нашего стека
- Не трогаем memory-layer, write-cmd и другие наши кастомные модули

## Technical Considerations

- **Порядок внедрения**: US-001 → US-002 → US-003 (критические), затем US-004 → US-005 → US-006, затем US-007 → US-008
- **Merge strategy**: cherry-pick логики, не применять патчи механически — наш `playwright_cmd.rs` и `gh_cmd.rs` расходятся от upstream
- **Test gate**: перед каждым PR-фиксом запускать `cargo test <module>`, после — `cargo test` полностью
- **registry.rs**: у нас `IGNORED_PREFIXES` и `IGNORED_EXACT` уже разделены (строки 382, 437) — фикс #246 тривиален
- **playwright_cmd.rs**: наша версия использует `parser` framework (FormatMode, OutputParser) — нужна адаптация структур, не copy-paste
- **gh_cmd.rs**: `should_passthrough_run_view` уже существует как образец для `has_output_format_flags`
- **Clap fallback (#200)**: самый рискованный — затрагивает `main.rs` и meta-команды, требует защитного списка

## Success Metrics

- `cargo test` — 0 регрессий на всех 105+ unit-тестах
- `bash scripts/test-all.sh` — 0 регрессий на 69 smoke-тестах
- `rtk find . -name "*.rs"` → работает
- `rtk playwright test` → парсит JSON без EXIT 1
- `rtk gh pr view --json number,title` → возвращает JSON
- `rtk git show HEAD:Cargo.toml | wc -l` → ровно столько строк, сколько в файле (не x2)

## Open Questions

- US-006 (Clap fallback): нужно ли сохранять parse_failure в отдельную таблицу или достаточно обычного tracking? PR #200 создаёт отдельную таблицу.
- US-008 (proxy streaming): нужно ли объединять stdout+stderr в один поток или выводить раздельно? PR #268 выводит раздельно.
