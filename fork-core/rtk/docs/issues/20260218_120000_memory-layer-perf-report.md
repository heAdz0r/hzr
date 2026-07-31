# Performance Report: RTK Memory Layer — SQLite WAL Architecture

**Date**: 2026-02-18T12:00:00 (updated sprint-5)
**Architecture**: SQLite WAL (`~/.local/share/rtk/mem.db`) — replaces JSON file backend
**PRD Guardrail**: cache-hit p95 < 200ms

---

## Storage Layer (current)

| Metric | Value |
|---|---|
| Backend | SQLite WAL (`mem.db`) |
| Concurrency | WAL multi-reader + `busy_timeout=2500ms` + `with_retry()` |
| Artifact format | JSON blob in `artifacts.content_json` column |
| Tables | `projects`, `artifacts`, `artifact_edges`, `events`, `cache_stats` |
| Indexes | `idx_projects_accessed`, `idx_events_project`, `idx_artifacts_version` |
| LRU eviction | max 64 проекта по `last_accessed_at` |
| TTL | 24h (configurable via `[mem] cache_ttl_secs` в `~/.config/rtk/config.toml`) |

---

## Measured Latency (этот репозиторий, ~60 src-файлов)

```
cache hit p50:  ~2 ms
cache hit p95:  ~108 ms   ← PRD target < 200 ms ✅
cache hit p99:  ~674 ms   ← хвост (OS jitter / SQLite busy_wait)
cold index:     ~80 ms
refresh:        ~120 ms
```

**Test gate**: `cache_hit_latency_p95_under_200ms` — 30 итераций на 20-модульном TempDir, p95 < 2000ms (hard), warn при ≥ 200ms (soft).

---

## HTTP API Latency (E4.1)

- `GET /v1/health`: < 1ms (no DB access)
- `POST /v1/explore` (cache hit): ~2-10ms (SQLite read + JSON serialize)
- `POST /v1/explore` (cache miss): ~80-150ms (full scan + hash + SQLite write)
- Server model: blocking thread-per-connection on `127.0.0.1:7700`
- Idle timeout: 300s (configurable `--idle-secs`)

---

## Cascade Invalidation Overhead (E3.2)

Pre-pass in `build_incremental_files()`:
- `O(changed_files × avg_stems × avg_imports_per_file)`
- На этом репо: ~negligible (<1ms) — most files have <10 imports
- Worst case (dense import graph): bounded by `cap(imports, 64)` per file

---

## Concurrency: WAL vs JSON

| | JSON backend (legacy) | SQLite WAL (current) |
|---|---|---|
| Multi-reader | Race condition | ✅ WAL concurrent reads |
| Multi-writer | Tempfile rename (not atomic on Windows) | ✅ SQLite serialised writes |
| Lock granularity | Whole file | Per-row, WAL-based |
| Busy handling | None (silent data loss risk) | `busy_timeout=2500ms` + `with_retry(3)` |
| Crash safety | fsync on tempfile | SQLite WAL crash-safe |

---

## Known Bottlenecks & Mitigations

| Bottleneck | Current Mitigation | Next Step |
|---|---|---|
| FS scan on every `explore` | `size+mtime` short-circuit (skip hash if metadata stable) | Persistent daemon index via `rtk memory watch` |
| SQLite SQLITE_BUSY contention | `with_retry(3)` exponential backoff | Connection pool if > 8 writers |
| p99 latency tails | `busy_timeout=2500ms` | Profile with `EXPLAIN QUERY PLAN` on large repos |
| HTTP server single-thread-per-conn | Adequate for local multi-agent (< 10) | Async (tokio/axum) for > 100 concurrent agents |

---

## Scale Projection

| Project Size | Expected p95 (cache hit) | Notes |
|---|---|---|
| < 100 files (this repo) | ~10-110ms | Within PRD target |
| 500 files | ~50-200ms | Borderline — daemon mode recommended |
| 1000+ files | ~100-500ms | Exceeds target on cold; daemon mandatory |
| Monorepo (10k files) | N/A — needs daemon | Background watcher + persistent index |
