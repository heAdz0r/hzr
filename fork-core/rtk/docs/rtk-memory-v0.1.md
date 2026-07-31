# RTK Memory Layer v0.1 Specification

## Status
- Version: `v0.1`
- Date: `2026-02-17`
- Scope: local shared-memory layer for repeated agent exploration across sessions.

## Problem Statement
Repeated agent sessions re-read the same codebase and rebuild identical context artifacts, causing unnecessary token spend and latency. v0.1 introduces deterministic artifact caching and incremental delta updates to return only valid, changed context.

## Goals
1. Reuse across sessions: produce once, reuse many times.
2. Incremental consistency: recompute only changed files/modules.
3. Low-noise output: compact, machine-oriented response payloads by default.

## Non-Goals (v0.1)
- Full cross-process daemon orchestration with push streams.
- AST-perfect semantic graph for all languages.
- Distributed cache replication.

## Architecture (v0.1)

### Component A: Artifact Cache Store
- Location: `~/.cache/rtk/memory`.
- Keying: project canonical path hash (`xxh3_64`) -> artifact file.
- Stored artifact fields:
  - project metadata (root, id, timestamps, version)
  - per-file records (rel path, size, mtime, hash, optional language/import hints)
- Eviction: LRU-like by file mtime, cap = `64` project artifacts.
- TTL: `24h`; stale artifacts trigger rebuild before use.

### Component B: Delta Indexer
- Walks project with ignore support and noise directory filters.
- Uses prior artifact map as baseline.
- Reuses unchanged file entries by `(size, mtime)`.
- Rehashes candidates only when metadata changed (or forced refresh).
- Emits delta summary:
  - `added`
  - `modified` (content hash changed)
  - `removed`

### Component C: Context Slice Builder
- Produces compact decision payloads:
  - entry points
  - hot paths
  - top imports
  - graph summary (`nodes`, `edges`)
- Supports detail levels: `compact` (default), `normal`, `verbose`.

### Component D: Watch Loop (daemon baseline)
- Poll-based watcher command (`memory watch`) for continuous delta emission.
- Intended as interim daemon mode before OS-native event backends.

## CLI Surface (implemented)
- `rtk memory explore [project] [--refresh] [--detail ...] [--format text|json]`
- `rtk memory delta [project] [--detail ...] [--format text|json]`
- `rtk memory refresh [project] [--detail ...] [--format text|json]`
- `rtk memory watch [project] [--interval N] [--detail ...] [--format text|json]`

## Planned Integration API (next)
Target transport for agent/tool integration:
- `POST /v1/explore`
- `POST /v1/delta`
- `POST /v1/context`
- `POST /v1/refresh`

Proposed request envelope:
```json
{
  "project_root": "/abs/path",
  "detail": "compact",
  "refresh": false
}
```

Proposed response envelope:
```json
{
  "command": "explore",
  "project_root": "/abs/path",
  "project_id": "f00dbabe...",
  "cache_status": "hit|miss|refreshed|stale_rebuild",
  "stats": {
    "file_count": 0,
    "reused_entries": 0,
    "rehashed_entries": 0
  },
  "delta": {
    "added": 0,
    "modified": 0,
    "removed": 0,
    "files": []
  }
}
```

## Consistency Policy
- Artifact validity is bounded by:
  - artifact version
  - file hash (`xxh3`)
  - size + mtime checks for incremental reuse
- Dirty/stale handling:
  - stale by TTL -> rebuild before serving
  - parse/version mismatch -> cold rebuild

## Output Noise Policy
- Default: `compact` detail.
- No explanatory prose in machine payloads.
- Deterministic keys and stable ordering for easy agent diffing.

## Security and Data Handling (v0.1)
- Artifacts are project-local metadata and lightweight code signatures.
- No secret masking logic yet for imported strings from source files.
- Next step: encryption-at-rest option + allow/deny patterns for sensitive paths.

## KPI Targets
- Exploration token reduction: `>= 50%` on stable repos.
- Cache-hit context latency: `< 200ms`.
- Medium delta latency: `< 2s`.
- Active project cache-hit ratio: `>= 80%`.

## Pilot Protocol
1. Repositories:
   - one monorepo
   - one medium service repo
2. Baseline period:
   - run repeated `explore` workloads without memory layer capture.
3. Treatment period:
   - run the same sequence via `rtk memory explore/delta`.
4. Record:
   - wall-clock latency
   - cache-hit ratio
   - changed-file ratio
   - token usage proxy (payload byte size / 4)
5. Compare baseline vs treatment and report regression deltas.

## Risks and Mitigations
- Stale artifact risk:
  - Mitigation: version+hash checks, TTL rebuild.
- Monorepo I/O overhead:
  - Mitigation: metadata-first reuse and selective rehash.
- Integration breakage with existing Deep Explorer wrappers:
  - Mitigation: additive CLI surface and opt-in usage.

## Next Iteration Backlog
1. Native FS events (`inotify`/`kqueue`/`ReadDirectoryChangesW`) behind unified trait.
2. HTTP/gRPC server mode with long-lived daemon process.
3. AST-segment granularity (file -> segment -> module graph) and dependency cascade refresh.
4. SDK bindings (`Rust`, `Python`, `Node`) over the transport API.
