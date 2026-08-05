# HZR Live Observatory — Product Requirements Document

**Status:** implemented in HZR 0.3.1

**Surface:** local Bun + Vue application served by `hzrd`

**Default URL:** `http://127.0.0.1:47391/`

> Historical 0.3.1 requirements. The interactive record drill-down, quiet synchronization,
> and request-evidence requirements introduced for 0.3.3 are specified in
> [`PRD_HZR_SOTA_OBSERVATORY.md`](PRD_HZR_SOTA_OBSERVATORY.md). Project-only activation and
> workspace-scoped CLI accounting introduced in 0.3.5 are specified in
> [`PRD_HZR_PROJECT_ACTIVATION.md`](PRD_HZR_PROJECT_ACTIVATION.md).

## 1. Product intent

HZR needs one trustworthy view of the control plane it owns. In under ten seconds an
operator must be able to verify the daemon, pinned RTK fork-core, canonical ICM memory,
HZR-owned grepai watcher, semantic search, selected project, and recent routing efficiency.

The visualizer is an observability product, not a decorative status page. `Ready` requires
current evidence. Estimated output reduction and provider-observed usage remain separate.
Missing provider data must never be presented as an observed zero.

## 2. Users and jobs

### Operator

- Confirm that a new installation is healthy without reading logs.
- Identify a degraded subsystem and copy the exact diagnostic command.
- Verify that project memory and semantic search are useful, not merely running processes.
- Detect avoidable RAW use and see the optimized HZR replacement.

### Contributor

- Inspect generation fingerprints, watcher ownership, probe latency, and artifact age.
- Correlate project operations with baseline, delivered output, and net reduction estimates.
- Validate accessibility, responsive behavior, and release evidence.

### Integrator

- Consume a typed, read-only dashboard snapshot.
- Distinguish missing provider receipts from real zero usage.
- Confirm that agent sessions route through HZR-owned services.

## 3. Product principles

- **One control plane:** HZR owns daemon, index watcher, fork-core, and ICM lifecycle.
- **Evidence before color:** every positive state is supported by a probe or verified artifact.
- **Project scope first:** selected-project activity precedes global estimates.
- **Honest accounting:** RAW receives no savings credit; provider records are never inferred.
- **Privacy by construction:** graph payloads contain aggregate topic metadata and opaque IDs,
  never memory content, prompts, responses, secrets, or store paths.
- **Local and read-only:** the dashboard binds to loopback and exposes no lifecycle mutations.
- **Accessible interaction:** information cannot depend on animation, color, or mouse input.

## 4. Information architecture

1. Header: HZR and visualizer versions, live state, refresh, and help.
2. Project registry: selected worktree and index readiness.
3. Control-plane topology: `hzrd`, RTK fork-core, ICM, grepai.
4. ICM observatory: retrieval evidence, counts, topic graph, search, and selection.
5. grepai observatory: artifact and watcher readiness plus real ledger-backed routed searches.
6. Project activity: optimized/RAW split and recent operations.
7. Global estimated efficiency, explicitly outside the project accounting boundary.
8. Provider receipts or an explicit no-receipts state.
9. Help and diagnostics.

## 5. Functional requirements

### FR-1 — Startup and installation

- Production bundles contain prebuilt visualizer assets.
- `hzrd` serves the UI and public dashboard API from the same loopback endpoint.
- `hzr init`, bundle installation, and user-service installation make the UI available without
  a separate frontend process.
- Bun is required for development/build only, not for the installed runtime.

### FR-2 — Service evidence

- Required cards: HZR daemon, RTK fork-core, ICM memory, and grepai index.
- Each card includes version, state, explanatory evidence, and a diagnostic command.
- FTS5-only ICM is `Ready` when retrieval succeeds; disabled embeddings are a capability limit,
  not degradation.
- On-demand engines may be `Standby` only when intentionally idle and still usable.

### FR-3 — ICM health and memory graph

- HZR probes the supervised ICM typed transport.
- A canonical-store snapshot is read-only and positively filtered to the selected repository.
- It reports retrieval mode, observation time, latency, transport, source, total/visible/hidden
  counts, bounded topics, bounded edges, and truncation.
- Topic identifiers are opaque hashes. No memory body or database path reaches the browser.
- The graph supports text filtering, reset, keyboard focus, and pinned selection.
- Node size communicates memory count; edges communicate relationship count.

### FR-4 — grepai readiness

- Readiness evidence includes artifact presence, size and mtime, watcher state/PID/uptime, HZR
  ownership, and ready-marker observation.
- `Ready` requires usable artifacts and a live HZR-owned watcher. Search traffic is not a health
  prerequisite and the dashboard never generates a synthetic query.
- The routed-search card is populated only from an optimized `rgai` or `search` operation already
  present in the selected project's accounting ledger.
- A real observation reports the exact requested command, ledger identifier, timestamp, working
  directory, measured execution time, and available agent/session attribution.
- When no recent routed search exists, the card says so explicitly and remains `Standby`; it never
  invents query text, hit counts, backend names, or activity timestamps.

### FR-5 — Live project activity and RAW accountability

- Operations use exact, separator-safe repository scoping.
- The UI reports operations, optimized operations, RAW operations, raw share, estimated baseline,
  delivered output, net avoided output, and execution time.
- RAW always has `baseline == delivered` and `net avoided == 0`.
- Avoidable RAW `rg`, `grep`, `ag`, `ack`, `sed`, `cat`, `head`, `tail`, and `nl` calls include
  a concrete HZR replacement and rationale.
- Exact build, test, Git, HTTP, and unsupported command output may remain RAW, with no savings claim.
- Live data refreshes without a full page reload.

### FR-6 — Accounting boundaries

- Project activity appears before global estimates.
- Global estimates name the deterministic estimator version.
- Provider receipts include only provider-attributed records accepted by HZR.
- When no receipt source is connected, the UI says `No provider receipts`; it does not show zero
  tokens or cost as though those were observed facts.
- Codex prompts and responses are not claimed as captured without an integrated provider receipt.

### FR-7 — Version and help

- The header displays utility and visualizer versions.
- Help includes doctor, service status, engine pins, stats, and full CLI help commands.
- Release documentation states the public URL and evidence semantics.

## 6. API contract

`GET /v1/dashboard` returns a typed, read-only snapshot containing:

- protocol, HZR, and visualizer versions;
- observation time, uptime, endpoint, overall state;
- service and project evidence;
- ICM and grepai observatories;
- project-local activity and global estimates;
- provider receipts, help, and accounting notes.

Repeated polling must not mutate service state. Health probes have bounded timeouts, and dashboard
polling must never issue search traffic or write self-generated activity into the ledger.

## 7. State model

- **Ready:** required evidence is current and usable.
- **Warming:** lifecycle is healthy but evidence is not yet sufficient.
- **Standby:** intentionally on-demand and inactive.
- **Degraded:** available with a material capability or ownership failure.
- **Failed:** the required service or probe cannot operate.

Overall state is derived from required service evidence. Text must explain every non-ready state.

## 8. Design and accessibility

- Visual language follows the README header: dark editorial canvas, warm orange signal,
  restrained glass surfaces, technical typography, and topology-inspired lines.
- Desktop uses an asymmetric, information-dense composition; mobile becomes one column without
  horizontal overflow.
- Graph nodes are focusable and work with Enter or Space.
- Live updates use an ARIA live region without stealing focus.
- Reduced-motion preference disables non-essential transitions.
- Decorative effects never obscure evidence or reduce contrast.

## 9. Non-functional requirements

- Loopback binding by default; no external network exposure.
- Dashboard requests are read-only and safe under two-second polling.
- Graph payload: at most 64 topics, 256 edges, and 256 sampled memories.
- Rust gates: fmt, clippy with warnings denied, all-target/all-feature tests.
- Vue gates: unit tests, typecheck, and production build.
- Responsive QA at 390, 768, and desktop widths with no horizontal overflow.

## 10. Acceptance criteria

1. A clean install exposes the UI at the daemon address without a separate command.
2. All four services show `Ready` when their evidence succeeds.
3. ICM renders real project topic counts and relationships without memory contents.
4. grepai shows HZR-owned watcher/artifact readiness and either a real ledger-backed routed search
   or an explicit no-observed-traffic state.
5. Avoidable RAW reads/searches receive zero credit and an actionable replacement.
6. Provider data is either attributable receipts or explicitly unavailable.
7. Utility version, visualizer version, help, and diagnostics are visible.
8. README desktop/mobile screenshots match the released implementation.
9. `/v1/dashboard` remains stable and read-only under repeated polling.
10. Rust, fork-core, and Vue release gates pass.

## 11. Success measures

- Time from load to a comprehensible health state.
- Project optimized-route share and avoidable RAW share.
- ICM probe latency and visible topic coverage.
- grepai watcher readiness and latency of real routed searches when present.
- Operator actions required to identify a degraded subsystem.

These are operational measures, not marketing benchmarks. Output reduction may be reported only
with its estimator or provider receipt source attached.

## 12. Explicit non-goals

- Exposing memory content in the browser.
- Treating estimated reduction as billed cost savings.
- Inventing provider usage that never entered an HZR receipt path.
- Starting secondary ICM or grepai processes from the UI.
- Remote multi-user administration in 0.3.1.

## 13. Release checklist

- Verify four live service probes, ICM graph privacy, grepai ownership, and ledger-backed search evidence.
- Exercise optimized and avoidable RAW commands and confirm accounting semantics.
- Run Rust, fork-core, and Vue gates.
- Capture sanitized desktop, observatory, and mobile screenshots.
- Update README, changelog, release notes, commit, annotated tag, and GitHub release.
