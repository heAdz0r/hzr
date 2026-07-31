# Requirements: RTK Memory Layer LLM Output Optimization

**Date**: 2026-02-18
**Scope**: `rtk memory explore/delta/refresh` output for LLM consumers (CLI text + JSON API).

## 1) Validation of Findings

### Confirmed
1. `type_graph` dominates compact/general payload and currently includes noisy `contains` edges.  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/renderer.rs:693`, `/Users/andrew/Programming/rtk/src/memory_layer/extractor.rs:222`.
2. `hot_paths` without delta is directory file-count, not true "hot" signal.  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/renderer.rs:260`.
3. `module_index` is alphabetically sorted and can prioritize `benchmarks/*` over `src/*`.  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/renderer.rs:391`.
4. `delta` JSON includes `old_hash/new_hash`, useful for internals but low value for LLM.  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/mod.rs:334`.
5. `stats` includes internal counters (`reused/rehashed/scanned`) that are low-value in LLM context.  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/renderer.rs:537`.
6. `top_imports` includes boilerplate (`std::*`, `super::*`, framework noise) with limited architecture signal.  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/renderer.rs:284`.
7. `test_map` may include marker files like `__init__.py` from test directories.  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/renderer.rs:714`.
8. Envelope has redundancy for LLM (`cache_status` + `cache_hit`, etc.).  
   Evidence: `/Users/andrew/Programming/rtk/src/memory_layer/mod.rs:396`, `/Users/andrew/Programming/rtk/src/memory_layer/mod.rs:402`.

### Clarification
- Some fields are redundant for LLM but still useful for machine compatibility/debug. We should avoid hard breaking existing JSON contracts by introducing an explicit output profile.

## 2) Updated Product Requirements

## R0. Dual Output Profiles (P0)
Add explicit response profile:
- `llm` (default for hook/API context consumption)
- `full` (backward-compatible diagnostics contract)

Applies to CLI + HTTP API. No breaking change for existing integrations that opt into `full`.

## R1. Type Graph Noise Control (P0)
In `llm` profile:
- Exclude `contains` relations in `compact` and `normal`.
- Keep `implements`, `extends`, `alias`.
- Keep full relation set only in `verbose` or `full` profile.
- Fix Rust field regex extraction to avoid bogus targets like `crate` from qualified types.

## R2. Module Index Ranking by Project Signal (P0)
In `llm` profile:
- Rank modules by `(primary_language_first, symbol_count_desc, path)`.
- De-prioritize known non-core roots (`benchmarks/`, `docs/`, `scripts/`) when primary source roots exist.

## R3. Delta Hash Suppression for LLM (P1)
In `llm` profile:
- Remove `old_hash/new_hash` from `delta.files[]`.
- Keep only `path`, `change`.
In `full` profile: keep hashes.

## R4. Hot Paths Semantics Fix (P1)
- If delta exists: keep current changed-dir behavior as `hot_paths`.
- If delta is empty: either omit `hot_paths` in `llm` profile or rename to `dir_layout` with explicit semantics.

## R5. Import Signal Filtering (P1)
For `llm` profile top imports:
- Exclude boilerplate prefixes: `super::`, `std::`, `anyhow::`, `serde::` (configurable denylist).
- Prefer project-internal imports (`crate::`, local module paths).

## R6. Test Map Hygiene (P2)
Exclude non-test marker files from `test_map` in `llm` profile:
- `__init__.py`
- empty files or files with zero detected test symbols (language-aware)

## R7. Stats Slimming (P2)
In `llm` profile keep only:
- `file_count`
- `total_bytes`
Keep internal counters only in `full` profile.

## R8. Envelope Slimming (P2)
In `llm` profile remove redundant envelope fields:
- `cache_hit` (derive from `cache_status`)
- `artifact_version`
- `command`
Keep in `full` profile.

## 3) Acceptance Criteria

1. Token reduction on `compact/general` (rtk repo reference run):
- At least **35%** reduction vs current baseline.
- Target band: **1400-1700 tokens saved** from current measurement.

2. Semantic quality:
- `module_index` must contain at least 70% entries from primary source roots for mixed repos.
- `type_graph` in `llm/compact` must contain 0 `contains` edges.
- `test_map` must contain 0 marker files (`__init__.py`).

3. Contract safety:
- Existing contract tests remain green for `full` profile.
- New contract tests cover `llm` profile shape and field omissions.

4. Performance guardrails preserved:
- No regression in p95 hot latency gate (`< 200ms` target; existing benchmark gates unchanged).

## 4) Test Plan Additions

1. Snapshot tests for `llm` vs `full` JSON shape.
2. Golden sample for mixed-language repo ensuring `module_index` prefers primary language modules.
3. Type graph regression: qualified Rust type parsing (`crate::foo::Bar`) should not produce target `crate`.
4. Delta serialization test ensuring hash fields hidden in `llm`, present in `full`.
5. Token-budget CI gate for `llm compact/general` payload size.
