# HZR 0.2.0 — текущий статус реализации и перепроверка исправлений

**Дата перепроверки:** 2026-08-01
**База:** [PRD.md](PRD.md) 0.2.0 · [PRD_ADOPTION.md](PRD_ADOPTION.md) §16 · [FORK_PARITY.md](FORK_PARITY.md)
**Предыдущий аудит:** [PRD_STATUS.md](PRD_STATUS.md) и [REVIEW.md](REVIEW.md) — исторический `v0.1.0` audit trail
**Метод:** повторный прогон **тех же самых repro-кейсов**, что дали G1–G7, end-to-end проверка adoption-поверхности в изолированном `HOME` и аудит self-contained release pipeline

> Source/release gates выполнялись в throwaway `HOME`, затем опубликованный Darwin ARM64
> artifact установлен глобально. Повторный live audit подтвердил bundle equality, active
> launchd service, отсутствие foreign engine owners и `hzr doctor --json` без errors.

---

## 1. Вердикт

**G1–G7 закрыты — все семь. Подтверждено запуском, не чтением диффов.** Ни одного «косметического» фикса: два из них исправлены полнее, чем я предлагал.

**Adoption-поверхность (§16) реализована полностью:** один dispatcher, durable PATH entry,
единый HZR instruction contract для Claude/Codex, HZR-owned MCP registrations и production
user service. Legacy ICM memory и platform RTK history имеют отдельные snapshot-first,
manifested, idempotent migrations.

**Self-contained distribution опубликован:** [`v0.2.0`](https://github.com/heAdz0r/hzr/releases/tag/v0.2.0) ставит один versioned platform bundle с full fork-core, patched grepai, ICM, exact caveman-code production tree и bundled Node.js 22.17.1. Same-version roots проходят повторную полную аттестацию; production `hzrd` управляется launchd/systemd user service. [Release workflow 30676586828](https://github.com/heAdz0r/hzr/actions/runs/30676586828) подтвердил native package/install/upgrade smoke для `darwin-arm64`, `darwin-x64`, `linux-arm64` и `linux-x64` до публикации artifacts.

| Область | Было (0.1.0) | Сейчас (0.2.0) |
|---|---|---|
| G1 codec `shadow` | ❌ инвертировано | ✅ исправлено полностью |
| G2 protected spans | 🟡 4 класса не покрыты | ✅ все 4 покрыты |
| G3 fork regression gate | 🟡 недетерминирован | ✅ детерминирован 4/4 |
| G4 codec overpromise | 🟡 | ✅ формулировка приведена к факту, newline сохранён |
| G5 bridge invariants | 🟡 текстовый поиск | ✅ runtime-тест в CI |
| G6 дублированная ошибка | 🟡 | ✅ исправлено |
| G7 права lock-файла | 🟡 `0644` | ✅ все `0600` |
| Adoption / hooks / instructions (§16) | ⚪ не реализовано | ✅ реализовано полностью |
| Self-contained release installer | ⚪ отсутствовал | ✅ public `v0.2.0`; native 4-platform matrix green |
| Economic KPI (§4.2) | 📊 0/9 | 📊 **0/9 — без изменений** |

---

## 2. Перепроверка G1–G7

### G1 — codec `shadow` ✅ исправлено полностью

Тот же вход, что дал исходное нарушение (3 абзаца, 1-й и 3-й идентичны):

```json
{
  "content": "The budget is exhausted.\n\nSecond distinct paragraph here.\n\nThe budget is exhausted.\n",
  "changed": false,
  "counterfactual": { "input_bytes": 84, "output_bytes": 58, "saved_bytes": 26, "would_change": true }
}
```

Оба требования PRD §6.6 теперь выполнены одновременно:

- **контент не изменён** — байт-в-байт равен входу, включая завершающий `\n`;
- **counterfactual size записан** — чего раньше не было вовсе (0 совпадений по `counterfactual|shadow_size`).

Фикс в [hzr-codec/src/lib.rs:116](crates/hzr-codec/src/lib.rs#L116) сделан **сильнее** моей рекомендации: `Shadow` короткозамыкается *первым*, до проверки `Exact|Off`, и вычисляет `candidate_transform` только чтобы измерить, ничего не отдавая наружу. Я предлагал одну строку в short-circuit — реализован полноценный `CounterfactualSize`.

### G2 — protected spans ✅ исправлено

Тот же вход, что раньше давал `protected_spans: []`:

| Фрагмент | Раньше | Сейчас |
|---|---|---|
| `src/main.rs` | не покрыт | ✅ `path` (5–16) |
| `MAX_RETRIES` | не покрыт | ✅ `enum` (25–36) |
| `handle_budget_overflow` | не покрыт | ✅ `identifier` (41–63) |
| `{"k":1}` | не покрыт | ✅ `structured` (69–76) |

Регекс расширен относительными путями (`(?:[\w.@%+~-]+/)+[\w.@%+~-]+`), snake/UPPER_SNAKE идентификаторами, `{...}` и `[...]`. Добавлена таксономия `kind`, которой раньше не было.

**Честная граница:** идентификаторы распознаются по наличию `_`. Одиночный CamelCase (`MyStruct`) и однословный `budget` по-прежнему не защищены. Это осознанное сужение, а не недоделка — при текущем paragraph-level трансформе оно не эксплуатируется.

### G3 — fork regression gate ✅ детерминирован

Причина устранена по существу: [tracking.rs:1365](fork-core/rtk/src/tracking.rs#L1365) теперь создаёт `tempfile::tempdir()` + `Tracker::open(isolated)` + `track_with(&tracker, …)` вместо записи в общую БД и чтения окна `get_recent(5)`.

```
tracking::tests, параллельно ×5        → ok 11 passed  (было FAILED 3/3)
scripts/verify-fork-core.sh --test ×4  → exit 0, 0, 0, 0  (было flaky)
```

**Важное уточнение, чтобы не создать ложной тревоги:** `cargo test` **напрямую внутри** `fork-core/rtk` по-прежнему падает — но на другом тесте, `memory_layer::git_churn::tests::test_load_churn_real_repo`. Он требует настоящий `.git` в `CARGO_MANIFEST_DIR`, а вложенная история запрещена snapshot-правилом §1.1.4 (проверено: `.git` в `fork-core/rtk` отсутствует). Официальный gate создаёт synthetic Git history в temp-копии, поэтому проходит. Это документированное устройство из `FORK_PARITY.md`, а не регрессия — но поддерживаемая команда именно `scripts/verify-fork-core.sh --test`, и прямой `cargo test` в этой директории использовать нельзя.

### G4 — формулировки codec ✅ приведены к факту

PRD §14 теперь говорит «protected **paragraph** codec с shadow counterfactual» вместо расплывчатого «адаптивный codec». Это ровно то, что код и делает. Trailing `\n` больше не срезается (видно в выводе G1). Существо не изменилось — трансформация остаётся дедупликацией абзацев — но документация больше не переобещает.

### G5 — bridge invariants ✅ заменены runtime-тестом

Появился [bridge.test.mjs](integrations/caveman-code/bridge.test.mjs) — настоящий JS-тест с mock-объектами вместо `BRIDGE.contains(...)`:

```
✔ bridge import is side-effect free
✔ production preparation owns tools and disables duplicate subsystems in order
```

Проверяется **фактическое состояние после исполнения**, а не наличие строки в файле: `prepared.session._repomapEnabled === false`, `prepared.session.memoryEnabled === false`, `getRtkEnabled() === false`, `getDisableAllHooks() === true`, `sessionOptions.tools === []`, пустые extensions/skills/prompts/agentsFiles, реально выставленные env. Порядок доказан отдельно: `assert.deepEqual(order, ["/v1/health", "/v1/context/plan", "create_session"])` — health и context plan происходят **до** создания сессии.

Включено в CI: `node --test integrations/caveman-code/bridge.test.mjs` ([ci.yml:68](.github/workflows/ci.yml#L68)).

Это именно то, чего требовал G5. Остаточная граница честна: тест использует fakes, то есть проверяет контракт bridge, а не поведение настоящего SDK.

### G6 — сообщение об ошибке ✅ исправлено

```
hzr: failed to read daemon token <path>; run `hzr daemon serve`: No such file or directory (os error 2)
```

Причина печатается один раз (была дважды).

### G7 — права файлов ✅ исправлено

`hzrd.token`, `hzrd.lock`, `hzrd.token.lock`, `config.toml` — все `-rw-------` (`0600`). Ранее `hzrd.token.lock` был `0644`.

---

## 3. Quality gates 0.2.0

| Gate | Результат |
|---|---|
| `cargo fmt --all --check` | ✅ exit 0 |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | ✅ exit 0 |
| `cargo test --workspace --all-targets --all-features` | ✅ **224 passed, 0 failed** (свежий прогон) |
| `rustup run 1.85.0 cargo check` (MSRV) | ✅ green |
| `node --test bridge.test.mjs` | ✅ 2/2 |
| `scripts/verify-fork-core.sh --test` | ✅ baseline `f4296ec4…` + current engine `a8512845…`; 1699 passed, 1 documented ignored, остальные suites green |
| `FORK_PARITY.md` без `missing`/`reimplemented` | ✅ (единственное вхождение — легенда) |
| assembled bundle smoke | ✅ versions, ownership, provenance, daemon и bundled Node 22.17.1 покрыты |
| outer `package-release.sh` + `smoke-install.sh` | ✅ clean runtime, reinstall, tamper/missing/symlink rejection и upgrade green на 4 native runners |

**Отмечу архитектурное решение 0.2.0:** verifier теперь ведёт **двойную identity** — неизменяемый baseline `v0.1.0` (`f4296ec4…`) и evolvable current engine (`a8512845…`). Это корректный способ разрешить развитие fork-core, не потеряв provenance: §1.1 больше не читается как «snapshot заморожен навсегда», но исходный импорт остаётся доказуемым.

---

## 4. Новая adoption-поверхность (§16) — проверено end-to-end

CLI surface из PRD §6.8 реализован; дополнительно доступны production service lifecycle и
явные `migrate history|memory` с preview/confirmation contract.

| Требование §16 / §13 | Проверка | Статус |
|---|---|---|
| §16.2 Ровно один перехватчик; RTK-записи **заменяются** | 3 rtk-записи → `rtk_entries: 0`, `hzr_entries: 2` | ✅ |
| §16.2 Сосуществование = `error`, не warning | `ERROR hook_ownership: HZR=2 RTK=1; exactly two HZR handlers and zero RTK handlers are allowed`, exit 1 | ✅ |
| §16.3 **Managed и degraded дают один вердикт** | вывод dispatcher при живом и убитом `hzrd` — **байт-идентичен**, exit 0 в обоих | ✅ |
| §16.3 Шелл не ломается без демона | degraded ветка отвечает валидным hook-JSON, а не ошибкой | ✅ |
| §16.3 Деградация видима | `hzr stats`: `degraded rewrites: 1` + incomplete accounting для daemon-free rewrite; `doctor`: `WARN degraded_rewrites` | ✅ |
| §16.3 Fallback в pinned fork, не stock RTK | тот же `0.44.1-fork.1` adapter | ✅ |
| §16.4 `init --if-needed` — filesystem no-op | рекурсивный SHA-256 data root до/после **идентичен** | ✅ |
| §16.5 `--dry-run` не пишет | SHA settings.json до/после идентичен; отдаёт `before_sha256`/`after_sha256`/`backup_path` | ✅ |
| §16.5 Подтверждение обязательно | без `--force`: «inspect `hzr install --dry-run`, then rerun with `--force`» | ✅ |
| §16.5 Idempotent | 2-й `install --force` → `changed: false`, второй backup не создаётся | ✅ |
| §16.5 Full-SHA backup (паттерн §11) | `settings.json.hzr-backup-<full-sha256>` | ✅ |
| §16.5 Centralized ICM ownership | известные external `icm hook` entries снимаются по умолчанию; `--keep-external-icm` оставляет их явно | ✅ |
| §16.5 Unknown handlers сохраняются | записи, не принадлежащие HZR/RTK/ICM, не переписываются | ✅ |
| §6.8 matcher `Bash\|Agent\|Task`, один handler | подтверждено в установленном конфиге | ✅ |
| §6.8 `SessionStart` → `init --if-needed --quiet` | подтверждено | ✅ |
| §6.8 CAS под filesystem lock | `settings.json.hzr.lock` присутствует | ✅ |
| §16.5 `uninstall` не восстанавливает RTK молча | HZR-записи сняты, подложенная rtk-запись и ICM остались как были | ✅ |
| Rewrite реально работает через HZR | `cat main.rs` → `permissionDecision: "ask"` + `updatedInput: … rtk read main.rs` | ✅ |

Фактический вердикт dispatcher — валидный Claude hook protocol: `hookSpecificOutput.permissionDecision` при exit 0, то есть fork-овые exit `0/1/2/3` преобразуются в typed decision, а не протекают наружу как process exit. Это соответствует §6.8.

### 4.1 Agent instructions — A1 закрыт

`hzr install` теперь одной подтверждённой операцией:

- атомарно размещает durable `hzr`/`hzrd` в PATH prefix;
- устанавливает один managed block в Claude `CLAUDE.md` и Codex `AGENTS.md`;
- ссылается на canonical bundled [HZR.md](HZR.md), а не копирует расходящийся prompt;
- снимает только machine-owned legacy `@RTK.md` imports и сохраняет пользовательский текст;
- применяет тот же `--dry-run` → `--force`, full-SHA backup, filesystem lock и compare-and-swap contract, что и settings installer;
- удаляет managed blocks при `hzr uninstall`, не восстанавливая RTK молча.

Тем самым агент получает не только shell interception, но и явный contract использовать HZR как единственный control plane для read/search/write/memory/context.

### 4.2 Self-contained release distribution

Release pipeline теперь включает четыре отдельных уровня:

1. `build-bundle.sh` собирает HZR, полный fork-core, patched grepai, patched ICM, exact caveman-code production dependencies и checksum-pinned Node.js 22.17.1;
2. `smoke-bundle.sh` проверяет versions, engine ownership, current/baseline provenance, bundled Node, daemon auth/singleton и compatibility alias;
3. `package-release.sh` создаёт platform archive и внутренний `BUNDLE_MANIFEST.sha256`;
4. `install.sh` проверяет release `SHA256SUMS` и manifest, ставит отдельный `versions/v0.2.0-<platform>` root и атомарно переключает `current`; `smoke-install.sh` доказывает clean install с урезанным `PATH` без внешних Node/RTK/grepai/ICM.

Artifact mappings реализованы для macOS/Linux arm64/x64. Release workflow собрал каждый artifact на native runner и выполнил тот же outer package/install smoke до публикации. Матрица полностью green в [run 30676586828](https://github.com/heAdz0r/hzr/actions/runs/30676586828); release содержит четыре platform archives, единый `SHA256SUMS` и GitHub build-provenance attestations. Windows artifact в 0.2.0 отсутствует.

### 4.3 Финальный live audit опубликованного `v0.2.0` — 2026-08-01

- release tag указывает на commit `551e91f2445cd51f52ff22052cc1d5592fd96475`;
  source CI [30675757188](https://github.com/heAdz0r/hzr/actions/runs/30675757188) и native
  release matrix [30676586828](https://github.com/heAdz0r/hzr/actions/runs/30676586828) — green;
- опубликованный `darwin-arm64` artifact установлен в `~/.local/share/hzr/current`;
  Claude/Codex hooks, instructions и MCP registrations используют только stable
  `~/.local/bin/hzr` и `~/.local/share/hzr/current/...`, без `versions/`/`target/` paths;
- launchd service активен и использует только stable `current/bin/hzrd`;
- bundle attestation: `hzr`, `hzrd`, RTK, grepai, ICM, Node, Caveman bridge и `HZR.md`
  — все `pass`; installed RTK SHA совпадает с current-engine release binary;
- `hzr doctor --json`: `healthy=true`, `errors=[]`; остаются только явные warnings для
  FTS-only memory, 59 исторических daemon-free rewrites и двух host-global codec paths,
  помеченных `unintercepted` без начисления savings;
- legacy ICM: 141 memories / 148 durable rows импортированы в repository namespace;
  legacy RTK source: 23 228 operations + 725 parse failures; финальный delta-import добавил
  246 ранее отсутствовавших commands, после чего повторный run стал no-op;
  оба повторных migration run вернули нулевой импорт и `changed=false`;
- process audit после production restart: ровно один `hzrd`, один HZR-owned `icm serve`
  и один managed `grepai watch`; doctor не находит foreign owners, wrappers или duplicate indexes;
- centralized memory содержит critical handoff `hzr-release-0-2-0` для следующих LOOP-агентов;
- cumulative `hzr stats`: 24 156 operations, 188 782 166 estimated net avoided tokens
  (`89.6355%`); provider-observed tasks/accepted остаются `0/0`, поэтому
  `economic_claim_ready=false` и cost reduction не заявляется как измеренный результат.

---

## 5. Реестр текущих gap

| ID | Раздел | Severity | Суть |
|---|---|---|---|
| **A3** | §6.6 / RB-08 | ПРИНЯТАЯ ГРАНИЦА | Claude/Codex не дают безопасный global response hook. Doctor маркирует путь `unintercepted`, managed codec остаётся в `hzr agent`, а savings для host-global ответов не начисляются |
| **A4** | §11 / §16.5 | НИЗКИЙ | Content-addressed backups изменяемых `settings.json`, `CLAUDE.md` и `AGENTS.md` сохраняются; автоудаление запрещено верно, но долгосрочная политика ротации не определена |
| **A5** | §6.6 | НИЗКИЙ | Идентификаторы защищены только при наличии `_`; CamelCase и однословные — нет. Латентно безопасно при paragraph-level трансформе |
| **KPI** | §4.2 | **ОТКРЫТО** | **0 из 9 product metrics измерено — без изменений с 0.1.0** |

R1 закрыт публичной native matrix. A3/A4/A5 — документированные ограничения,
KPI — незавершённое измерение; ни один из них не разрешает публиковать
прогноз экономии как результат.

---

## 6. Что дальше

`v0.2.0` опубликован и глобально принят как active bundle. Следующий release LOOP начинается с
paired provider benchmark, расширения MCP schema/accounting coverage и доступного только через
официальные host hooks request/response interception; exact/shadow safety и запрет двойного
accounting остаются непереговорными инвариантами.

Главный открытый продуктовый вопрос не изменился: §4.2 остаётся 0/9. Функциональные предпосылки готовы, а `hzr stats` честно разделяет actual usage, estimated local effect и неполный accounting. Превращать целевые −30% cost / −20% turns / −35% uncached input из гипотезы в результат можно только после paired benchmark из §14.
