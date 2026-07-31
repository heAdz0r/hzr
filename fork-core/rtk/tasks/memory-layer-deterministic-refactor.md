# Memory Layer: Deterministic Refactor Plan

**Status**: Ready for implementation
**Принцип**: только объективные сигналы, никакой субъективной интерпретации

---

## Контекст и мотивация

Текущая реализация содержит компоненты с anchor bias:
- `episode.rs` — записывает `outcome`/`rationale` выводы прошлых агентов без инвалидации по коду
- `f_affinity_score` в `ranker.rs` — взвешивает "успехи" прошлых сессий без привязки к актуальному коду
- `f_semantic_score` в `ranker.rs` — зависит от grepai, не детерминировано
- `intent.confidence` → ranking — субъективная классификация влияет на ранжирование

Кроме того, `api.rs` не компилируется: 15+ ошибок `\!` (escaped bang — артефакт `rtk write`).

**Цель**: убрать субъективные signals, заменить детерминированными, починить сборку.

---

## Phase 0 — Починить сборку (блокер, делать первым)

**Файл**: `src/memory_layer/api.rs`

**Проблема**: `\!` вместо `!` в 15+ местах. Артефакт экранирования в `rtk write`.

```bash
# Проверить количество:
grep -n '\\!' src/memory_layer/api.rs | wc -l

# Исправить все вхождения:
rtk write replace src/memory_layer/api.rs --from '\!' --to '!' --all

# Верифицировать:
cargo build 2>&1 | grep "^error" | wc -l  # → 0
```

---

## Phase 1 — Убрать субъективные компоненты

### 1.1 episode.rs — срезать outcome/rationale

**Убрать** любые поля/методы связанные с:
- `outcome: Option<String>` — текстовый вывод агента об успехе/провале
- `rationale: Option<String>` — обоснование решения
- `task_file_affinity` — любой метод возвращающий f32 по прошлым исходам

**Оставить**:
- `EventType` enum (Read, Edit, GrepaiHit, Delta, Decision, Feedback) — факты
- `EpisodeEvent` с `event_type`, `file_path`, `symbol`, `epoch_secs` — без интерпретации
- `record_event()`, `start_session()`, `end_session()` — как debugging log

**Не менять**: SQLite-схему (обратная совместимость), просто перестать писать outcome/rationale.

### 1.2 ranker.rs — убрать f_affinity_score и f_semantic_score, добавить f_churn

В `FeatureVec`:
```rust
// УБРАТЬ:
pub f_affinity_score: f32,   // субъективно — episode task_file_affinity
pub f_semantic_score: f32,   // субъективно — зависит от grepai

// ДОБАВИТЬ:
pub f_churn_score: f32,      // git log frequency (объективно, кэш по HEAD sha)
```

В `RankingModel` + `impl Default` — убрать `w_affinity`/`w_semantic`, добавить `w_churn`.
Перераспределить веса (сумма = 1.0):
```
w_structural:          0.30   // был 0.25
w_churn:               0.25   // новый
w_recency:             0.20   // был 0.10
w_risk:                0.15   // был 0.10
w_test_proximity:      0.05   // без изменений
w_token_cost_penalty:  0.05   // без изменений
```

В методе `score()` заменить `f_affinity_score`/`f_semantic_score` на `f_churn_score`.

### 1.3 intent.rs — отвязать от ranking

`TaskIntent`, `IntentKind`, `task_fingerprint` — оставить (полезны как cache key).

**Убрать только**: любые места где `intent.confidence` или `intent.predicted` умножаются
на вес и попадают в `FeatureVec`. В коде заполняющем FeatureVec не должно быть:
```rust
features.f_semantic_score = intent.confidence * ...;  // УБРАТЬ
```

---

## Phase 2 — Новый модуль git_churn.rs

**Файл**: `src/memory_layer/git_churn.rs`

### Структура

```rust
use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;

/// Кэш инвалидируется при смене HEAD sha — не перечитываем git log при каждом запросе.
pub struct ChurnCache {
    pub head_sha: String,
    pub freq_map: HashMap<String, u32>,  // rel_path → change_count
}

pub fn load_churn(repo_root: &Path) -> Result<ChurnCache>;
pub fn churn_score(cache: &ChurnCache, rel_path: &str) -> f32;
```

### Алгоритм load_churn

```bash
git -C <repo_root> log --all --format="" --name-only
```

- Строки без пустых = пути файлов → `freq_map[path] += 1`
- HEAD sha: `git -C <repo_root> rev-parse HEAD`

### Нормализация churn_score (логарифмическая — важно)

```rust
pub fn churn_score(cache: &ChurnCache, rel_path: &str) -> f32 {
    let count = *cache.freq_map.get(rel_path).unwrap_or(&0) as f32;
    let max = cache.freq_map.values().copied()
        .map(|x| x as f32).fold(0.0_f32, f32::max);
    if max == 0.0 || count == 0.0 { return 0.0; }
    count.ln() / max.ln()  // log-normalized в (0,1]
}
```

Лог-нормализация обязательна: Cargo.lock меняется в 5× чаще src-файлов — без log доминирует.

### Тесты в git_churn.rs

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_score_zero_for_unknown_file() { ... }  // файл не в freq_map → 0.0

    #[test]
    fn test_score_one_for_max_churn_file() { ... } // файл с max count → 1.0

    #[test]
    fn test_log_normalization_ordering() { ... }   // A(10 changes) > B(1 change) → score_A > score_B
}
```

### Интеграция в mod.rs

Добавить `pub mod git_churn;` в `src/memory_layer/mod.rs`.

В функции `build_plan_context()` (или эквивалентной):
```rust
let churn = git_churn::load_churn(&repo_root)?;
// для каждого кандидата:
candidate.features.f_churn_score = git_churn::churn_score(&churn, &candidate.rel_path);
```

---

## Phase 3 — Token budget в API и CLI

### 3.1 API: добавить token_budget в PlanRequest

В `api.rs`, в структуру запроса `/v1/plan-context`:
```rust
/// Maximum tokens in response context. 0 = default (4000).
#[serde(default)]
pub token_budget: u32,
```

Передавать в `budget::select_candidates(candidates, effective_budget)`.
Если `token_budget == 0`, использовать 4000 как дефолт.

### 3.2 CLI: --token-budget

В `main.rs`, в sub-command `rtk memory plan`:
```
rtk memory plan --token-budget 2000
```

### 3.3 Убрать Ollama из critical path

В `handle_plan_context()`:
- Ollama rerank только при явном `"ml_mode": "full"` в запросе
- Дефолт: Stage-1 linear ranker + budget.rs, без HTTP к ollama
- Убирает потенциальный таймаут ~200ms из каждого запроса

---

## Итоговые файлы к изменению

| Файл | Действие |
|------|----------|
| `src/memory_layer/api.rs` | Phase 0: исправить `\!`→`!`; Phase 3: добавить token_budget |
| `src/memory_layer/episode.rs` | Phase 1.1: убрать outcome/rationale/affinity |
| `src/memory_layer/ranker.rs` | Phase 1.2: заменить f_affinity/f_semantic → f_churn; обновить веса |
| `src/memory_layer/intent.rs` | Phase 1.3: убрать propagation confidence → FeatureVec |
| `src/memory_layer/git_churn.rs` | Phase 2: создать новый модуль |
| `src/memory_layer/mod.rs` | Phase 2: `pub mod git_churn;` + вызов в plan pipeline |
| `src/main.rs` | Phase 3.2: добавить `--token-budget` |

## Что НЕ трогать

- `budget.rs` — работает корректно
- `cache.rs`, `renderer.rs`, `extractor.rs`, `manifest.rs` — без изменений
- `ollama.rs` — оставить как `--ml-mode full` опцию
- `intent.rs` — `task_fingerprint`, `extracted_tags` оставить
- SQLite-схема — не менять

---

## Верификация

```bash
# Сборка
cargo build 2>&1 | grep "^error" | wc -l     # → 0

# Тесты
cargo test 2>&1 | tail -5

# git_churn unit
cargo test memory_layer::git_churn::

# API без Ollama (запустить в фоне)
rtk memory serve &
curl -s -X POST http://127.0.0.1:9193/v1/plan-context \
  -H 'Content-Type: application/json' \
  -d '{"project_root":".","task":"fix auth bug","token_budget":1000}' \
  | jq '.candidates | length'
```
