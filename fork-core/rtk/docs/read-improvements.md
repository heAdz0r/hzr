# Read Improvements: архитектурный план и TODO

## Цель
Сформировать безопасный план развития `rtk read` с чистой архитектурой, учитывая уже внесенные улучшения в `/Users/andrew/Programming/rtk/src/read.rs`, и подготовить это как ТЗ для отдельного агента.

## Текущее состояние (учтено в плане)

### Что уже реализовано в `read.rs`
1. Binary-safe чтение + hex preview (`looks_binary`, `format_binary_preview`).
2. Line ranges `--from/--to` для файла и stdin.
3. Cat parity в `--level none` (вывод точных bytes через `write_all`).
4. CSV/TSV digest с sampling и numeric stats.
5. Вариант digest для `aggressive` (меньше analysis rows, без numeric stats).
6. Read cache для фильтрованных reads (`build_read_cache_key`, `load/store/prune`).
7. Range-aware streaming чтение из файла (`BufReader + read_until`).

### Технический долг, появившийся после улучшений
1. Размер `read.rs`: 1081 строка.
2. В одном модуле смешаны: orchestration, IO, digest, cache, render, heuristics.
3. Сложно безопасно добавлять `--outline`, `--symbols`, `--changed` без дальнейшего монолита.
4. Нет единого feature pipeline с четкими extension points.

## Архитектурные принципы
1. `read.rs` остается тонким orchestrator: route + graceful fallback + tracking.
2. Форматные фичи и режимы чтения выносятся в отдельные модули с trait-интерфейсами.
3. Tree-sitter **не удалять**: backend должен быть поддержан архитектурно наряду с regex.
4. Любая advanced-фича должна деградировать в безопасный fallback, а не ломать `rtk read`.
5. Новые режимы должны быть изолированы от существующего exact-path (`--level none`).

## Целевая модульная структура
1. `/Users/andrew/Programming/rtk/src/read.rs`:
   orchestrator, входные параметры, dispatch по mode.
2. `/Users/andrew/Programming/rtk/src/read_types.rs`:
   `ReadRequest`, `ReadMode`, `ReadContext`, `ReadOutput`, `ReadErrorPolicy`.
3. `/Users/andrew/Programming/rtk/src/read_source.rs`:
   bytes/text IO, line-range logic, stdin/file abstraction.
4. `/Users/andrew/Programming/rtk/src/read_cache.rs`:
   cache key, load/store/prune, versioning policy.
5. `/Users/andrew/Programming/rtk/src/read_digest.rs`:
   tabular + filename-based digest strategies.
6. `/Users/andrew/Programming/rtk/src/read_symbols.rs`:
   symbol model + extractor traits + backend router.
7. `/Users/andrew/Programming/rtk/src/read_changed.rs`:
   git diff provider + unified diff hunk parser + context slicing.
8. `/Users/andrew/Programming/rtk/src/read_render.rs`:
   text/json renderers, line-number policies, truncation utilities.
9. `/Users/andrew/Programming/rtk/src/symbols_regex.rs`:
   regex extraction (переиспользуемая логика для `smart` и `read`).
10. `/Users/andrew/Programming/rtk/src/symbols_treesitter.rs`:
    tree-sitter extractor implementation (или adapter-слой при наличии текущего кода).

## Режимы чтения (целевой контракт)
1. `full` (default): текущий behavior (`level + filters + digest/cache`).
2. `outline`: структурная карта файла с line spans.
3. `symbols`: machine-readable JSON index символов.
4. `changed`: только измененные hunks из git working tree.
5. `since`: измененные hunks относительно ревизии (`HEAD~N`, hash, tag).

## Важные решения по совместимости
1. Текущие флаги `--from/--to/--level/-n/--max-lines` сохранить.
2. `--outline|--symbols|--changed|--since` сделать mutually exclusive group.
3. Для `--symbols` вернуть стабильный JSON schema (versioned, с запасом для evolution).
4. Для `--changed|--since` при non-git окружении выводить объяснимую ошибку + exit code 1.

## Backend стратегия символов (regex + tree-sitter)
1. Ввести `SymbolExtractor` trait.
2. Реализации:
   - Regex extractor (быстрый, dependency-light).
   - Tree-sitter extractor (точные boundaries).
3. Ввести выбор backend:
   - `auto` (по умолчанию): tree-sitter если поддерживается язык, иначе regex.
   - `regex`.
   - `tree-sitter` (strict mode, с понятной ошибкой если unsupported).
4. В `auto` режиме ошибки tree-sitter не должны ломать read, fallback на regex.

## TODO: этапы реализации

### Phase 1: Разделение монолита без изменения поведения
1. Вынести `read cache` из `/Users/andrew/Programming/rtk/src/read.rs` в `/Users/andrew/Programming/rtk/src/read_cache.rs`.
2. Вынести line-range + file/stdin bytes logic в `/Users/andrew/Programming/rtk/src/read_source.rs`.
3. Вынести tabular digest в `/Users/andrew/Programming/rtk/src/read_digest.rs`.
4. Вынести formatting helpers (`format_with_line_numbers`, truncation helpers) в `/Users/andrew/Programming/rtk/src/read_render.rs`.
5. Оставить в `read.rs` только orchestration.

Definition of done:
1. Поведение CLI не изменилось.
2. Текущие тесты `read::tests` проходят.
3. Добавлены unit tests по новым модулям.

### Phase 2: `--outline` и `--symbols`
1. Добавить флаги в `/Users/andrew/Programming/rtk/src/main.rs`.
2. Добавить data model символов (`Symbol`, `SymbolKind`, `Visibility`, `Span`).
3. Реализовать regex extractor в `/Users/andrew/Programming/rtk/src/symbols_regex.rs`.
4. Интегрировать tree-sitter extractor через `/Users/andrew/Programming/rtk/src/symbols_treesitter.rs`.
5. Добавить renderer:
   - outline (human-readable, stable formatting);
   - symbols JSON (machine-readable, versioned).
6. Добавить backend selector `--symbol-backend auto|regex|tree-sitter`.

Definition of done:
1. `rtk read <file> --outline` работает на Rust/TS/Python минимум.
2. `rtk read <file> --symbols` выдает валидный JSON.
3. `auto` fallback работает: tree-sitter -> regex.

### Phase 3: `--changed` и `--since`
1. Добавить `DiffProvider` trait в `/Users/andrew/Programming/rtk/src/read_changed.rs`.
2. Реализация `GitDiffProvider` через `git diff --no-color --unified=<N> ...`.
3. Парсер hunk headers `@@ -a,b +c,d @@` -> ranges.
4. Renderer changed blocks: line numbers + +/- context.
5. Поддержка:
   - working tree (`--changed`),
   - relative rev (`--since HEAD~3`).
6. Edge cases:
   - untracked files,
   - rename,
   - binary diff,
   - not-a-git-repo.

Definition of done:
1. `--changed` показывает только измененные блоки.
2. `--since` корректно отрабатывает диапазон ревизий.
3. Никаких silent-fail: ошибки объяснимы и тестируются.

### Phase 4: Спецформаты и экономия токенов
1. Ввести registry стратегий digest по filename/path pattern.
2. Добавить стратегии:
   - `*.lock`, `pnpm-lock.yaml`, `yarn.lock`, `Cargo.lock`;
   - `package.json` (scripts + deps summary);
   - `Cargo.toml` (deps/features summary);
   - `tsconfig*.json`, `biome*.json` (schema-like digest);
   - `.env*` (keys only, masked values);
   - `Dockerfile` (key instructions only);
   - `*.generated.*`, `*.g.ts` (generated marker);
   - `*.md` (headers + section preview).
3. Добавить long-line truncation policy (minimal/aggressive only).

Definition of done:
1. Для спецформатов стабильно срабатывает digest path.
2. Есть явный fallback к обычному read при ошибке parser/strategy.
3. Truncation не применяется в `--level none`.

### Phase 5: Опциональный dedup repetitive blocks
1. Реализовать отдельной опцией (не default).
2. Добавить conservative thresholds и heuristic safety checks.
3. Добавить regression тесты, чтобы не скрывать важные отличия.

## План в разрезе PR

### PR-1: Safety Net + подготовка к рефакторингу
Scope:
1. Зафиксировать текущее поведение `read` golden/integration тестами.
2. Добавить минимальные типы-заготовки (`ReadMode`, `ReadRequest`) без смены behavior.
3. Подготовить feature flags/arg groups в `main.rs` только как скрытые (`hide = true`) или без wiring.

Files:
1. `/Users/andrew/Programming/rtk/src/read.rs`
2. `/Users/andrew/Programming/rtk/src/main.rs`
3. `/Users/andrew/Programming/rtk/tests/read_cli.rs` (новый)

Acceptance:
1. Полностью зеленые текущие тесты.
2. Поведение команд `rtk read` не изменилось.

### PR-2: Декомпозиция монолита `read.rs` без функциональных изменений
Scope:
1. Вынести source/range logic в `read_source`.
2. Вынести cache logic в `read_cache`.
3. Вынести render helpers в `read_render`.
4. Вынести tabular digest в `read_digest`.
5. Оставить `read.rs` как orchestration-only слой.

Files:
1. `/Users/andrew/Programming/rtk/src/read.rs`
2. `/Users/andrew/Programming/rtk/src/read_source.rs` (новый)
3. `/Users/andrew/Programming/rtk/src/read_cache.rs` (новый)
4. `/Users/andrew/Programming/rtk/src/read_render.rs` (новый)
5. `/Users/andrew/Programming/rtk/src/read_digest.rs` (новый)
6. `/Users/andrew/Programming/rtk/src/read_types.rs` (новый)

Acceptance:
1. Поведение CLI идентично baseline.
2. Нет дублирования логики между новыми модулями.
3. Покрытие новых модулей unit тестами.

### PR-3: `--outline` + `--symbols` (Regex backend)
Scope:
1. Добавить публичные флаги `--outline` и `--symbols`.
2. Добавить regex extractor на shared API.
3. Реализовать text renderer для outline и JSON renderer для symbols.
4. Переиспользовать extraction в `local_llm` через общий модуль.

Files:
1. `/Users/andrew/Programming/rtk/src/main.rs`
2. `/Users/andrew/Programming/rtk/src/read.rs`
3. `/Users/andrew/Programming/rtk/src/read_symbols.rs` (новый)
4. `/Users/andrew/Programming/rtk/src/symbols_regex.rs` (новый)
5. `/Users/andrew/Programming/rtk/src/local_llm.rs`

Acceptance:
1. `rtk read <file> --outline` работает стабильно для Rust/TS/Python.
2. `rtk read <file> --symbols` выдает валидный versioned JSON.
3. Нет регрессии в `full` режиме.

### PR-4: Tree-sitter backend (без удаления regex)
Scope:
1. Ввести `SymbolExtractor` backend routing: `auto|regex|tree-sitter`.
2. Подключить tree-sitter extractor адаптером.
3. Обеспечить fallback в `auto` режиме на regex при ошибках/unsupported language.

Files:
1. `/Users/andrew/Programming/rtk/src/read_symbols.rs`
2. `/Users/andrew/Programming/rtk/src/symbols_treesitter.rs` (новый)
3. `/Users/andrew/Programming/rtk/src/main.rs`
4. `/Users/andrew/Programming/rtk/Cargo.toml` (если нужны зависимости/feature gating)

Acceptance:
1. Tree-sitter backend selectable из CLI.
2. Regex backend остается рабочим и поддерживаемым.
3. `auto` никогда не ломает read при сбое tree-sitter.

### PR-5: `--changed` и `--since`
Scope:
1. Добавить read-mode для diff-aware чтения.
2. Реализовать `GitDiffProvider` и hunk parser.
3. Добавить `--context N` для количества соседних строк.
4. Корректно обработать non-git/untracked/rename/binary cases.

Files:
1. `/Users/andrew/Programming/rtk/src/main.rs`
2. `/Users/andrew/Programming/rtk/src/read.rs`
3. `/Users/andrew/Programming/rtk/src/read_changed.rs` (новый)

Acceptance:
1. `--changed` показывает только релевантные hunks.
2. `--since <rev>` корректно работает для диапазонов ревизий.
3. Ошибки объяснимые, без silent fallback в некорректные данные.

### PR-6: Спецформаты + long-line truncation
Scope:
1. В `read_digest` добавить registry filename/path-based strategies.
2. Добавить первые стратегии: lock files, generated files, package/cargo/env/tsconfig/biome/dockerfile/md.
3. Добавить policy truncation длинных строк для minimal/aggressive.

Files:
1. `/Users/andrew/Programming/rtk/src/read_digest.rs`
2. `/Users/andrew/Programming/rtk/src/read_render.rs`
3. `/Users/andrew/Programming/rtk/src/read.rs`

Acceptance:
1. Для поддержанных форматов включается digest path.
2. В `--level none` не происходит truncation/digest и сохраняется exact mode.
3. Есть fallback на обычный read при ошибке strategy parser.

### PR-7: Опциональный dedup repetitive blocks + финализация
Scope:
1. Реализовать dedup как opt-in флаг.
2. Добавить safety thresholds и regression тесты на ложные сжатия.
3. Обновить docs/README/ARCHITECTURE.

Files:
1. `/Users/andrew/Programming/rtk/src/read_render.rs`
2. `/Users/andrew/Programming/rtk/src/read.rs`
3. `/Users/andrew/Programming/rtk/README.md`
4. `/Users/andrew/Programming/rtk/ARCHITECTURE.md`

Acceptance:
1. Dedup выключен по умолчанию.
2. Включенный dedup не скрывает критичные отличия в тестовых кейсах.
3. Документация синхронизирована с CLI и реализацией.

## TODO: изменения по файлам
1. Обновить `/Users/andrew/Programming/rtk/src/main.rs`:
   новые args, groups, conflicts, help-text.
2. Создать `/Users/andrew/Programming/rtk/src/read_types.rs`.
3. Создать `/Users/andrew/Programming/rtk/src/read_source.rs`.
4. Создать `/Users/andrew/Programming/rtk/src/read_cache.rs`.
5. Создать `/Users/andrew/Programming/rtk/src/read_digest.rs`.
6. Создать `/Users/andrew/Programming/rtk/src/read_symbols.rs`.
7. Создать `/Users/andrew/Programming/rtk/src/read_changed.rs`.
8. Создать `/Users/andrew/Programming/rtk/src/read_render.rs`.
9. Создать `/Users/andrew/Programming/rtk/src/symbols_regex.rs`.
10. Создать `/Users/andrew/Programming/rtk/src/symbols_treesitter.rs`.
11. Рефакторить `/Users/andrew/Programming/rtk/src/local_llm.rs` на shared extraction API.
12. Обновить `/Users/andrew/Programming/rtk/README.md` (новые read modes).
13. Обновить `/Users/andrew/Programming/rtk/ARCHITECTURE.md` (новая read architecture).

## Тест-план

### Unit tests
1. `read_source`: range semantics для text и bytes (inclusive bounds, EOF, no trailing newline).
2. `read_cache`: key stability, prune policy, mismatch handling.
3. `read_digest`: CSV/TSV планы для minimal/aggressive, format strategies.
4. `read_symbols`: regex/tree-sitter extractors, backend fallback behavior.
5. `read_changed`: hunk parser, context window logic.
6. `read_render`: line-number formatting, JSON schema rendering.

### Integration tests
1. CLI backward compatibility для существующих флагов.
2. New modes:
   - `--outline`;
   - `--symbols`;
   - `--changed`;
   - `--since`.
3. Non-git and binary edge cases.
4. Cache hit/miss сценарии.

## Риски и mitigation
1. Риск: cache stale output.
   - Mitigation: robust key (path + size + mtime + inode + options + cache version).
2. Риск: tree-sitter dependency complexity.
   - Mitigation: backend abstraction + regex fallback + feature-gated integration.
3. Риск: регрессии existing read behavior.
   - Mitigation: phase-1 refactor без функциональных изменений + golden tests.
4. Риск: line numbers теряют связь с исходником в outline/changed.
   - Mitigation: для структурных режимов хранить source spans отдельно от filtered output.

## Критерии приемки (DoD)
1. `read.rs` становится orchestration-first, heavy logic вынесена в модули.
2. Старое поведение сохранено и покрыто тестами.
3. Новые режимы работают и документированы.
4. Tree-sitter backend сохранен и поддерживается как опция.
5. Ошибки в advanced paths не ломают базовый `rtk read`.

## Примечание по текущему состоянию
Этот план уже учитывает параллельные улучшения в `/Users/andrew/Programming/rtk/src/read.rs`:
1. read cache,
2. aggressive-specific tabular digest profile,
3. byte-level stdin/file handling,
4. `--from/--to` routing в `main.rs`.

Следующий исполнитель должен рефакторить поверх этих улучшений, а не откатывать их.
