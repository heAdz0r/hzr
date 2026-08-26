# HZR Observatory 2.0 — Design Improvement PRD

**Status:** implemented for HZR 0.3.3  
**Product surface:** local Bun + Vue application served by the HZR daemon  
**Primary operator question:** what is working, what is happening now, and what evidence can I inspect?

## Outcome

Turn the dashboard from a periodically repainted status page into an operator-grade local observatory. The finished surface must preserve its state while live data changes, let an operator traverse project memory from topic to record, and make every routed output operation inspectable by route, command, working directory, and truthful agent attribution.

The visualizer remains read-only. It never starts a second ICM, grepai, or fork-core process, never mutates memory, and never credits estimated savings to RAW routes.

## Acceptance outcomes

1. Automatic synchronization never changes the manual Refresh button label, disabled state, icon animation, selection, scroll position, or graph camera.
2. A topic can be found, selected, and expanded into bounded redacted record metadata without leaving the public dashboard; content inspection requires the authenticated API.
3. The graph supports pan, zoom, fit, reset, keyboard selection, readable labels, stable layout, and a list fallback.
4. Memory details come from a positive repository filter and an opaque topic identifier. Responses are bounded and do not expose the ICM database path or repository token.
5. Recent routed operations expose the exact recorded command and working directory already present in the ledger. Agent and session fields are displayed only when they were observed; missing attribution is labeled `Unattributed`, never inferred as fact.
6. Every activity row opens a detail view with route rationale, token benchmark inputs and outputs, latency, directory, agent/session evidence, and a copyable command.
7. “Live” is defined in the UI as the latest successful HZR ledger snapshot. It is not presented as process liveness.
8. The UI remains usable at 1280 px, 1440 px, and narrow mobile widths, with keyboard focus, reduced-motion support, and no information encoded by color alone.

## Data and trust boundaries

- `/v1/dashboard` stays a bounded public loopback snapshot. It may expose commands and paths that HZR already records locally, but never provider secrets, environment variables, stdin, or captured output bodies.
- Public topic details use a dedicated read-only endpoint and a server-side opaque-ID lookup. A caller cannot submit a raw ICM topic or repository token, and summary, raw excerpt, keywords, and source data are redacted.
- Full bounded topic details use `/v1/memory/topics/{topic_id}?project={stable_worktree_id}` behind the daemon bearer token. Authentication is not treated as workspace authorization: the stable project identifier is mandatory, and the endpoint returns at most 100 positively project-filtered records while never returning the canonical database path or repository token.
- Agent attribution is evidence-based. The ledger stores an optional agent label and session identifier at record time. Historical rows remain unattributed.
- Background failures keep the last successful snapshot and show a compact stale indicator. No zero-value fallback is substituted for unavailable data.

## Interaction model

### Quiet live synchronization

- Initial load: skeleton only until the first snapshot resolves.
- Automatic update: every five seconds while the document is visible, with an immutable on-screen snapshot during the request.
- Manual update: the Refresh control alone may enter a busy state.
- Data changes: update values without entrance animation and preserve graph viewport, active topic, expanded records, open operation, and page scroll.
- Freshness: show `Live`, `Syncing quietly`, `Updated … ago`, or `Stale` as text with an icon.

### Memory explorer

1. Overview shows topic nodes sized by memory count and connected by observed relationships.
2. Search filters a synchronized topic list and graph highlights.
3. Selecting a topic centers it and opens its topic inspector.
4. Expanding a topic fetches bounded record details and adds memory nodes to the graph.
5. Selecting a memory on the public dashboard opens redacted metadata. Authenticated API clients may inspect summary, importance, weight, timestamps, access count, source, keywords, related record IDs, and a bounded raw excerpt when present.
6. Back, fit, zoom, and reset controls do not refetch data.

### Activity explorer

1. The stream is grouped by recent recorded operation, not animated on polling.
2. Primary row: time, route, operation, agent, directory basename, token delta, and latency.
3. Expanded drawer: full recorded command, canonical working directory, observed agent/session, route reason, replacement recommendation, baseline and delivered estimates, and measurement method.
4. RAW rows explicitly show zero savings credit and a first-class replacement when one exists.

## Twelve-module design audit

### 1. UX architecture

**Current:** one long dashboard with a decorative memory island and dense operation rows.  
**Problem:** monitoring, exploration, and evidence inspection have no explicit hierarchy.  
**Target:** retain the single-page overview but make Memory and Activity self-contained workspaces with overview → selection → detail disclosure.  
**Severity:** critical.

### 2. Content hierarchy

**Current:** large ornamental headings compete with operational evidence.  
**Problem:** the next useful action is visually weaker than decorative totals.  
**Target:** status and freshness first, then evidence, then explanation; selected entity details stay adjacent to the graph or stream.  
**Severity:** high.

### 3. Composition and layout

**Current:** orbital placement overlaps labels and wastes the center; activity columns collapse into an undifferentiated row.  
**Problem:** relationships and details cannot be scanned.  
**Target:** split-pane explorer on desktop, stacked inspector on mobile, bounded canvas height, resizable-feeling proportions, and aligned data columns.  
**Severity:** critical.

### 4. Typography

**Current:** heavy display type appears in dense operational regions and long labels collide.  
**Problem:** graph labels and command evidence lose readability.  
**Target:** display face only for section identity; UI sans for controls; monospace for commands, IDs, paths, and numbers; ellipsis only when the complete value is available in the inspector.  
**Severity:** high.

### 5. Color system

**Current:** orange glow is used for nodes, background ornament, warning states, and primary actions.  
**Problem:** semantic state is ambiguous and the graph looks decorative.  
**Target:** ember remains brand accent; green means verified ready, amber means attention, red means failure, blue means selection/relationship, neutral layers carry structure. Each state also has text and an icon.  
**Severity:** high.

### 6. Visual depth and surfaces

**Current:** repeated glass cards and halos flatten evidence into the same visual weight.  
**Problem:** important inspectors do not feel attached to selected data.  
**Target:** quiet base surface, raised selected inspector, one-pixel graph grid, restrained shadows, and no glow around every node.  
**Severity:** medium.

### 7. Controls and affordances

**Current:** graph nodes expose only a neighborhood toggle; automatic polling animates a manual control.  
**Problem:** operators cannot navigate, drill down, or distinguish manual from background work.  
**Target:** explicit search, fit, zoom, reset, back, expand, reveal, copy, and close controls with labels/tooltips and predictable disabled states.  
**Severity:** critical.

### 8. Feedback and motion

**Current:** Refresh visibly flips every two seconds and components replay transitions.  
**Problem:** the dashboard feels unstable and distracts from real changes.  
**Target:** background sync is silent, camera and selection are stable, motion is limited to direct manipulation, and `prefers-reduced-motion` disables nonessential transitions.  
**Severity:** critical.

### 9. Data visualization

**Current:** hand-authored SVG orbital layout, overlapping labels, no pan/zoom, and no record layer.  
**Problem:** topology is not explorable and large topics dominate the visual.  
**Target:** Cytoscape.js with deterministic constrained layout, compound topic/record semantics, incremental element updates, min/max zoom, selected-neighborhood emphasis, readable labels, and a synchronized accessible list.  
**Severity:** critical.

### 10. Accessibility

**Current:** SVG groups are keyboard-focusable but navigation is not discoverable and detail content does not exist.  
**Problem:** keyboard and assistive-technology users cannot complete the exploration task.  
**Target:** semantic controls, roving topic list, visible focus, ARIA live only for meaningful state changes, no poll announcements, list fallback for the graph, focus return after closing inspectors, and 44 px targets.  
**Severity:** high.

### 11. Responsive behavior

**Current:** wide orbital SVG and fixed activity grid rely on desktop width.  
**Problem:** labels and columns overflow or become illegible.  
**Target:** graph and inspector split above 1100 px, stacked below; activity summary becomes a compact card row; commands and paths wrap safely.  
**Severity:** high.

### 12. Performance and implementation quality

**Current:** the entire snapshot is replaced every two seconds and graph geometry is recomputed reactively.  
**Problem:** unnecessary visual churn and no durable view state.  
**Target:** five-second visible-tab polling, one in-flight request, incremental Cytoscape updates, cached topic details per snapshot generation, bounded API payloads, abort on unmount, unit-tested state transforms, Rust typed contracts, and zero lint suppression.  
**Severity:** high.

## Functional requirements

### FR-1 — Snapshot synchronization

- Distinguish `initialLoading`, `backgroundSyncing`, and `manualRefreshing`.
- Prevent concurrent requests with one in-flight promise or abort controller.
- Never disable or relabel Refresh during background synchronization.
- Pause the interval in hidden tabs and refresh once on return.

### FR-2 — Topic detail API

- Route: `GET /v1/dashboard/memory/topics/{opaque_topic_id}`.
- Select the same registered project as `/v1/dashboard`.
- Resolve the opaque ID only after positive repository filtering.
- Return topic summary plus bounded redacted record metadata and `truncated` metadata.
- Keep summary, raw excerpt, keywords, and source data redacted on this public route; expose full
  bounded details only through authenticated, explicitly project-scoped
  `GET /v1/memory/topics/{opaque_topic_id}?project={stable_worktree_id}`.
- Reject malformed identifiers and unknown topics with explicit 4xx errors.

### FR-3 — Memory graph

- Use Cytoscape.js, not custom orbital SVG geometry.
- Preserve viewport unless the user explicitly selects Fit or Reset.
- Add records only for the expanded topic; do not fetch all memory bodies on dashboard load.
- Keep an accessible list synchronized with graph selection.

### FR-4 — Operation evidence

- Extend typed activity records with `recorded_command`, `working_directory`, optional `agent`, and optional `session_id`.
- Show only recorded facts. Historical nulls render as `Unattributed` or `Not recorded`.
- Do not parse human-formatted command output to create attribution.

### FR-5 — Honest accounting

- Label baseline, delivered, avoided, and regression fields as estimates.
- RAW operations always receive zero avoided-token credit even if input/output happen to differ.
- Provider receipts remain visually and semantically separate.

## Quality gate

- Rust: focused RED → GREEN tests, `cargo fmt --all --check`, workspace clippy with warnings denied, workspace tests for all targets and features.
- Vue: `bun install --frozen-lockfile`, unit tests, typecheck, production build.
- Browser: verify initial load, quiet automatic update, manual busy state, topic expansion, memory selection, operation expansion, keyboard focus, 1280/1440/mobile layouts, console errors, and reduced motion.
- Acceptance audit: an independent subagent maps every acceptance outcome and all twelve modules to evidence, with no critical or high unresolved findings.

## Non-goals

- Editing or deleting memories from the dashboard.
- Starting independent engine processes.
- Claiming that a recent agent-attributed operation proves the agent process is still alive.
- Capturing provider prompts, responses, stdin, environment variables, or raw tool output.
- Presenting deterministic UTF-8 estimates as provider-observed billing.
