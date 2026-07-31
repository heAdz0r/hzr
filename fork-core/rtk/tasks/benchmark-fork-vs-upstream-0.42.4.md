# Бенчмарк: rtk fork (0.42.0-fork.2) vs upstream (v0.42.4)

Дата: 2026-06-13. Площадка: репозиторий rtk (этот проект), идентичные команды, изолированные tracking-DB (`RTK_DB_PATH`), upstream — в чистом `HOME` (его integrity-check отказывался работать с форковским hook). Токены ≈ байты/4.

## 1. Экономия токенов на одинаковых командах

| Кейс | raw (tok) | fork | fork % | upstream | up % | Победитель |
|---|---|---|---|---|---|---|
| read src/main.rs | 25 298 | 24 487 | 3% | 25 298 | 0% | fork (авто-minimal) |
| read src/filter.rs | 3 408 | 2 742 | 20% | 3 408 | 0% | **fork** |
| read README.md | 5 131 | 199 | **96%** | 5 131 | 0% | **fork** (md-дайджест) |
| read Cargo.toml | 466 | 466 | 0% | 466 | 0% | паритет (config=none) |
| ls src | 2 127 | 663 | 69% | 651 | 69% | паритет |
| grep "fn run" src | 3 486 | 994 | **71%** | 3 428 | 2% | **fork** |
| find -name '*.rs' | 531 | 161 | 70% | 155 | 71% | паритет |
| git status | 163 | 32 | **80%** | 131 | 20% | **fork** |
| git log -30 | 5 134 | 593 | **88%** | 1 724 | 66% | **fork** |
| git diff HEAD~5 | 77 740 | 1 645 | **98%** | 6 623 | 91% | **fork** (4× компактнее) |
| git show HEAD | 4 173 | 1 013 | **76%** | 3 488 | 16% | **fork** |
| git branch -a | 2 398 | 2 252 | 6% | 179 | **93%** | **upstream** ⚠️ |
| cargo check (66 warn) | 4 010 | 1 154 | **71%** | 3 928 | 2% | **fork** (upstream льёт warnings raw) |
| cargo test (1136 tests) | 18 436 | 38 | 100% | 13 | 100% | паритет |
| **ИТОГО** | **153 136** | **36 613** | **76%** | **54 777** | **64%** | **fork: −33% токенов vs upstream** |

Ключевые причины победы форка:
- `read` по умолчанию: у upstream дефолт `--level none` (0% экономии, агент должен сам помнить про `-l minimal`); у форка авто-уровень по расширению. На равном `minimal` движки идентичны, **кроме Markdown**: у форка doc-дайджест (96%), upstream отдаёт md как есть.
- `cargo check/build`: upstream печатает все warnings verbatim; форк группирует/обрезает.
- `git status/log/show/diff`: у форка агрессивнее компрессия.
- ⚠️ Единственный проигрыш — `git branch -a`: upstream обрезает длинный список веток (93%), форк отдаёт почти raw. Стоит портировать.

## 2. Перехват «обычных» команд (hook / rewrite)

34 типовые команды через fork-hook (`rtk-rewrite.sh`) и upstream `rtk rewrite`:

- **Перехватывают оба** (22): git*, cargo*, ls, grep, rg, cat→read, find, docker, kubectl, curl, gh, pytest, ruff, pip, go test, tsc, head→read, npx vitest…
- **Только fork** (3): `npm test`, `bun run build`, `bunx tsc` — JS/Bun-стек (твой основной стек!).
- **Только upstream** (9): `pnpm install`, `mypy`, `rspec`, `mvn`, `dotnet`, `aws`, `psql`, `jest`, `glab` — у форка просто нет этих фильтров/правил.
- ⚠️ Семантический баг форка: `tail -100 app.log` → `rtk read --max-lines 100` (отдаёт ПЕРВЫЕ 100 строк). Upstream корректно: `--tail-lines 100`. У upstream есть флаг `--tail-lines` в read — портировать.
- Архитектура: у upstream `rtk rewrite` — single source of truth на Rust (+ `rtk hook` для Gemini/Copilot). У форка — bash-скрипт 21.6K (есть незакоммиченный src/rewrite_cmd.rs — правильное направление).

## 3. Эксклюзивные возможности

**Только в форке** (это «иная лига», не фильтры):
- `rtk write` (replace/patch/set/batch/file) — атомарные идемпотентные правки. У upstream правок нет вообще → агент использует нативный Edit/Write.
- `rtk rgai` — семантический поиск.
- `rtk memory` — memory layer + инъекция контекста в субагентов.
- `rtk bun`, `rtk ssh`, `rtk lsof`, `rtk ps`, `rtk build`.
- Блок-хуки нативных Read/Grep/Write/Explore.

**Только в upstream v0.42.4** (кандидаты на бэкпорт):
- `trust`/`untrust`/`verify` + **проектные TOML-фильтры** — расширяемость фильтров без перекомпиляции (самая интересная фича).
- `rtk pipe` — фильтрация stdin (Unix-пайпы).
- `rtk rewrite` + `rtk hook` — нативный Rust-движок перехвата, мультиагентный (Gemini CLI, Copilot).
- Фильтры: `aws`, `psql`, `glab`, `dotnet`, `jest`, `mypy`, `rake`, `rubocop`, `rspec`, `gt`, `gradlew`, `mvn`.
- `telemetry` (GDPR), `session` (адопшен по сессиям), `json --keys-only`.
- Integrity-check хука (агрессивный: отказывается работать при изменённом hook — у форка свой integrity.rs в работе).

## 4. Производительность
Старт: fork 4.8ms, upstream 4.5ms — паритет, оба в целевых <10ms.

## 5. Вердикт

**Форк развивать стоит.** На реальной рабочей нагрузке Claude Code форк экономит **76% vs 64%** у upstream — т.е. отправляет в LLM на **треть меньше токенов**. Решающие преимущества: авто-уровень `read` (upstream по умолчанию вообще не фильтрует чтение!), md-дайджест, компрессия git/cargo, перехват bun/bunx/npm. Плюс уникальный пласт: `write`, `rgai`, `memory` — у upstream этого нет, и именно они закрывают нативные Edit/Grep/Read-инструменты.

**Но upstream быстро двигается** (v0.42.4 — 13 фильтров/команд, которых нет у форка). Правильная стратегия — гибрид: продолжать форк + регулярный бэкпорт.

### Приоритеты бэкпорта/фиксов
1. 🐛 `tail -N` → `--tail-lines` (сейчас форк отдаёт первые строки вместо последних) + портировать флаг `--tail-lines` в read.
2. 🐛 `git branch` компрессия (6% vs 93% у upstream).
3. ⭐ TOML-фильтры (`trust`/`verify`) — расширяемость без релиза.
4. ⭐ `rtk rewrite` на Rust (src/rewrite_cmd.rs уже в работе) — заменить 21.6K bash.
5. `rtk pipe` (stdin-фильтрация).
6. Фильтры по потребности стека: `jest`, `pnpm install`-rewrite, `psql`, `aws` (заготовки aws_cmd.rs/psql_cmd.rs/mypy_cmd.rs уже лежат untracked в дереве).
