# Product Requirements Document — HZR Local Visualizer

**Status:** Implemented and locally verified  
**Date:** 2026-08-01  
**Product version:** HZR 0.3.x  
**Owners:** HZR control plane, daemon, index registry, and release bundle  
**Frontend:** Bun-built Vue 3 application  
**Runtime:** Served by the existing loopback-only `hzrd`; no second control plane

## Implementation record

The 0.3.0 implementation ships the typed dashboard protocol, bounded workspace registry,
read-only loopback API, Bun/Vue production bundle, `hzrd` static serving, installer/init
lifecycle integration, responsive project and service views, observed-versus-estimated
accounting, diagnostic help deck, and release-bundle checks described below.

![HZR visualizer desktop overview](screenshots/hzr-visualizer-overview.png)

The public README also includes the real registry and 390 px responsive captures. They were
recorded from an isolated three-project registry served by the assembled local dashboard,
so no production workspace paths or usage records are embedded in the repository.

## 1. Product summary

HZR needs one local visual surface that answers five questions within five seconds:

1. Is the HZR control plane healthy?
2. Which projects are registered with HZR, and which need attention?
3. What is the state of `hzrd`, RTK fork-core, ICM, and grepai?
4. What work and usage has HZR actually observed, and what efficiency values are estimates?
5. Which command should the operator run next when a component is degraded?

The visualizer is a read-only operator surface. Its Vue assets are built with Bun, shipped in the versioned release bundle, and served by the existing authenticated control-plane process on its loopback address. The browser dashboard must not spawn RTK, ICM, grepai, another daemon, another index, or another memory store.

## 2. Problem

HZR already exposes precise CLI diagnostics, health, engine pins, per-project index state, and a cumulative ledger. Those facts are distributed across commands and are difficult to scan while several repositories are active. An operator currently has to remember command names, compare JSON by hand, and distinguish on-demand services from failures without a unified view.

This creates three practical risks:

- a stopped on-demand grepai watcher can be mistaken for an unhealthy engine;
- an obsolete or unavailable project can remain invisible until a command fails;
- estimated avoided tokens can be confused with provider-observed usage if labels are weak.

## 3. Research and adopted patterns

The product uses established monitor patterns without copying their visual identity:

- Docker Desktop groups related processes as applications, exposes lifecycle state in the list, and moves logs, inspection, and statistics into progressive-detail views. HZR adopts project-first grouping, a compact service overview, and copyable diagnostic commands. Source: [Docker Desktop Containers view](https://docs.docker.com/desktop/use-desktop/container/).
- Netdata distinguishes `Live`, `Stale`, and `Offline`, exposes collection errors and no-data states directly on charts, and lets operators narrow nodes by status. HZR adopts explicit state definitions, last-observed timestamps, and filterable project status. Sources: [Netdata node states](https://learn.netdata.cloud/docs/netdata-cloud/node-states-and-transitions) and [Netdata charts](https://learn.netdata.cloud/docs/dashboards-and-charts/charts).
- Grafana uses dashboard-level variables to change one context without duplicating dashboards, and exposes state history as a timeline. HZR adopts a single project selector/filter model rather than separate pages per repository; a timeline is deferred until HZR owns a trustworthy time-series source. Sources: [Grafana variables](https://grafana.com/docs/grafana/latest/visualizations/dashboards/variables/) and [Grafana alert state history](https://grafana.com/docs/grafana/latest/alerting/monitor-status/view-alert-state-history/).
- Mature monitors separate an overview from details. HZR keeps the first screen decisive, makes raw paths and IDs secondary, and provides exact CLI commands instead of unauthenticated browser mutations.

## 4. Goals

### 4.1 Functional goals

- Register the current workspace during `hzr init` and refresh its `last_seen` timestamp during idempotent initialization.
- List all valid HZR workspace registrations without scanning arbitrary user directories.
- Show HZR version, protocol version, dashboard build version, daemon endpoint, and refresh time.
- Show the states and pinned versions of `hzrd`, RTK fork-core, ICM, and grepai.
- Show project index readiness, Git/non-Git identity, last seen time, index artifact presence, and index size.
- Show provider-observed task/token/cost totals separately from UTF-8-byte estimate-based efficiency totals.
- Include concise help, copyable next-step commands, keyboard navigation, loading skeletons, empty states, and recoverable error states.
- Start serving the visualizer whenever the existing `hzrd` service starts. Release installation must install/start that service; project initialization must register the project so an already-running daemon displays it immediately.

### 4.2 Experience goals

- A healthy system must be recognizable in under five seconds without reading a table.
- A degraded subsystem must expose a reason and exact diagnostic command within one interaction.
- The primary desktop viewport must show overall state and all four service states without scrolling at 1440 × 900; metrics and projects must follow in one continuous scan with no navigation change.
- Mobile must preserve all facts and actions at 390 px width without horizontal page scrolling.

### 4.3 Quality goals

- First static UI response on a warm daemon: p95 below 100 ms on the local machine.
- Dashboard snapshot generation: p95 below 250 ms for 100 registered projects, excluding an explicitly surfaced filesystem stall.
- Initial production asset payload: below 400 KiB compressed excluding the existing README hero image; the hero image must be lazy/decorative and must not block dashboard content.
- WCAG 2.2 AA contrast, focus visibility, landmark structure, reduced-motion support, and screen-reader state labels.
- No network requests beyond the loopback HZR origin. No analytics, remote fonts, CDNs, or telemetry.

## 5. Non-goals

- Replacing `hzr doctor`, logs, or exact CLI diagnostics.
- Starting/stopping engines from the unauthenticated browser surface.
- Promising token or cost savings that are not backed by the current ledger.
- Adding a second daemon, database, semantic index, watcher owner, or memory process.
- Displaying raw memory contents, user prompts, credentials, bearer tokens, engine logs, or source file contents.
- Remote access, multi-user authorization, cloud synchronization, or mobile-native applications.
- A full time-series observability stack in the first release.

## 6. Users and jobs to be done

### 6.1 Primary user: local HZR operator

When starting work, the operator wants to confirm that HZR and its current project are ready, so agent work does not silently degrade or duplicate infrastructure.

### 6.2 Secondary user: HZR contributor

After changing lifecycle, indexing, or accounting code, the contributor wants a fast visual confirmation of component versions, registered project state, and typed API output before running full release gates.

### 6.3 Recovery user

When the system is degraded, the user wants the exact reason and safest next command without exposing mutation controls in the browser.

## 7. Information architecture

The initial release is one responsive page with anchored regions:

1. **Control header** — HZR wordmark, overall state, version, endpoint, refresh control.
2. **System pulse** — four service nodes connected by the HZR control-plane flow.
3. **Metrics** — observed usage and estimated efficiency shown in separate visual groups.
4. **Projects** — searchable/filterable registered workspace list with expandable detail.
5. **Help deck** — exact commands for doctor, service status, stats, index status, and help.
6. **Footer** — privacy/local-only statement, protocol version, UI build version, and documentation references.

The route contract is:

- `GET /` — visualizer application.
- `GET /assets/*` — immutable bundle assets.
- `GET /v1/dashboard` — read-only typed snapshot, loopback-only by daemon invariant.
- Existing mutating and sensitive `/v1/*` routes remain bearer-authenticated.

## 8. State model

### 8.1 System and service states

| State | Meaning | Visual treatment |
|---|---|---|
| `ready` | Component is available for its declared role. | green pulse and “Ready” text |
| `degraded` | Component is running or reachable but a capability is reduced. | amber pulse, reason shown |
| `rebuilding` | Component is starting, indexing, or warming. | blue animated pulse, progress-neutral copy |
| `standby` | Component is intentionally on demand and not currently running. | gray-blue pulse, never shown as failure |
| `stopped` | Component is expected but unavailable. | red pulse and diagnostic command |
| `unknown` | Snapshot cannot prove a state. | gray pulse and last observed time |

Color is never the only signifier: every state includes text, icon shape, and accessible label.

### 8.2 Project index states

- `ready`: configuration plus vector and symbol artifacts are present.
- `warming`: configuration exists but one or more index artifacts are not ready.
- `registered`: project is registered but grepai has not initialized its configuration.
- `unavailable`: registered root no longer resolves to a directory.
- `degraded`: duplicate/foreign placement or unreadable metadata was detected.

## 9. Project registry

Initialization writes one bounded `workspace.json` record under the existing HZR-owned workspace identity directory:

```text
<data-root>/workspaces/<repository-id>/<worktree-id>/workspace.json
```

Required fields:

```json
{
  "schema_version": 1,
  "root": "/absolute/exact/path",
  "repository_id": "sha256",
  "worktree_id": "sha256",
  "git_backed": true,
  "linked_worktree": false,
  "index_directory": "/absolute/exact/path",
  "registered_at_ms": 0,
  "last_seen_at_ms": 0
}
```

Requirements:

- atomic replacement, private file permissions, and a 64 KiB read cap;
- no lossy path conversion;
- no traversal outside `<data-root>/workspaces`;
- no following registration symlinks;
- invalid records are skipped and counted as registry warnings;
- records with the same canonical root are deduplicated by newest `last_seen_at_ms`;
- unavailable roots remain visible so the operator can understand stale state, but the dashboard never deletes them.

## 10. Typed dashboard API

`DashboardResponse` must live in `hzr-protocol`, not be assembled as ad-hoc frontend JSON. It contains:

- `protocol_version`, `hzr_version`, `visualizer_version`, `generated_at_ms`, `uptime_ms`;
- `overall_state` and a typed list of services;
- a typed project list and registry warning count;
- `LedgerSummary`-equivalent observed totals;
- `EfficiencySummary`-equivalent estimated totals;
- explicit measurement labels and notes;
- help commands as exact strings.

The API must not include bearer tokens, environment variables, raw prompts, source contents, memory contents, logs, or arbitrary config values.

## 11. Metrics contract

### 11.1 Provider-observed group

- recorded tasks;
- externally accepted tasks;
- actual input tokens;
- actual output tokens;
- provider-reported cost when present.

These values use the label **Observed**. A missing provider value is displayed as “Not reported,” not zero.

### 11.2 Direct efficiency estimate group

- filtered operations;
- estimated baseline tokens;
- estimated delivered tokens;
- estimated gross avoided tokens;
- estimated regression tokens;
- estimated net avoided tokens;
- estimate method `estimated_utf8_bytes_div_4_v1`.

These values use the label **Estimate** in the title, legend, tooltip, and accessible description. They are never added to provider-observed totals or translated into claimed billing savings.

## 12. Lifecycle and distribution

### 12.1 Development

- `visualizer/package.json` pins Vue, Vite, the Vue plugin, TypeScript, and the Bun package-manager version.
- `bun install --frozen-lockfile` installs dependencies.
- `bun run build` produces deterministic production assets.
- `bun run dev` is allowed for UI work and proxies only to a developer-selected loopback daemon.

### 12.2 Release bundle

- The bundle builder requires the pinned Bun release and builds the visualizer before packaging.
- Production assets are installed under `share/hzr/visualizer`.
- Bundle smoke tests verify required assets, manifest coverage, CSP-compatible rendering, and the dashboard API.
- Bun is a build tool, not a shipped runtime requirement. End users still need only system Git plus the installer’s standard POSIX utilities.

### 12.3 Initialization and installation

- `hzr init` registers the current project and prints the dashboard URL when not quiet.
- Idempotent `hzr init --if-needed` refreshes the registration without duplicating it.
- The release installer continues to install/start the single `hzrd` user service; the visualizer becomes available as part of the same readiness boundary.
- `HZR_INSTALL_SERVICE=0` remains authoritative and prevents service installation; the managed SessionStart hook carries `--skip-service` so a later project initialization cannot reverse the opt-out.
- Source/debug initialization never writes a production service definition implicitly; contributors use `hzr daemon serve`.
- Upgrades serve visualizer assets through the stable `current` release pointer.

## 13. Interaction requirements

- Refresh automatically every 5 seconds while the document is visible; pause while hidden.
- Manual refresh has default, hover, pressed, disabled, loading, and error feedback.
- Project search filters by exact visible name/path text locally; status chips filter by state.
- Project rows expand with index artifacts, IDs, last seen time, and an exact `hzr index status --workspace <path>` command.
- Copy actions show a visible “Copied” confirmation for 2.5 seconds and an ARIA live announcement.
- Errors retain the last successful snapshot, mark it stale, and show the failure without clearing the page.
- Empty registries show one action: `hzr init --if-needed` in the desired project.
- Motion follows `prefers-reduced-motion`; decorative pulses stop when reduced motion is requested.

## 14. Visual direction

The interface derives from `docs/assets/hzr-hero.png`: warm parchment, charcoal stone, ember orange, vermilion, and thin kintsugi-like energy lines. The hero art is a low-opacity atmospheric layer, never a content background. The functional UI remains a dark, high-contrast control surface so service state colors stay legible.

Core tokens:

```css
--ink-950: #0d0e10;
--ink-900: #141518;
--ink-850: #1b1c20;
--ink-800: #23242a;
--parchment-50: #fff8e8;
--parchment-200: #ead8b7;
--ember-300: #ffb15a;
--ember-500: #f36a21;
--ember-650: #cf3d18;
--success: #3ddc97;
--warning: #ffb454;
--error: #ff626d;
--info: #68a9ff;
--disabled: #727780;
```

The layout uses a 12-column desktop grid, 8-column tablet grid, 4-column mobile grid, a 4 px spacing system, 16–24 px card radii, restrained translucency, and no ornamental border grid.

## 15. Design Improvement PRD — 12-module audit

There is no existing visualizer, so each module records the current absence and the implementation target.

### M1: Signifiers and grouping

#### M1.1 — Group system, metrics, projects, and help

- **Current:** No visual grouping exists.
- **Problem:** Operators cannot form a scan path or distinguish system facts from project facts.
- **Target:** Four semantic regions with `background: rgba(27, 28, 32, 0.88)`, `border-radius: 20px`, and `padding: 24px`; related subgroups use 12 px gaps and unrelated sections use 32 px gaps.
- **Severity:** CRITICAL

#### M1.2 — Distinguish every state and interaction

- **Current:** No state signifiers exist.
- **Problem:** Color-only or absent state feedback would make health ambiguous.
- **Target:** Every state has icon, label, color, and detail; every button has hover, active, disabled, loading, and focus-visible states; icon-only buttons have tooltips.
- **Severity:** HIGH

### M2: Visual hierarchy and chromatic contrast

#### M2.1 — Establish a five-second scan order

- **Current:** No hierarchy exists.
- **Problem:** A flat data dump would force label-by-label reading.
- **Target:** Overall status at 24 px/750 weight, service pulse as the dominant visual anchor, metric numerals at 24 px/700, section titles at 16 px/700, metadata at 12 px/500. Ember is reserved for brand focus and primary actions; semantic colors are reserved for state.
- **Severity:** CRITICAL

#### M2.2 — Replace label/value dumps with data objects

- **Current:** CLI output is linear text.
- **Problem:** Repeating identical label/value pairs do not support rapid comparison.
- **Target:** Service nodes, metric cards with miniature bars/sparklines where history exists, and project rows with one dominant name, secondary path, state chip, and artifact meter.
- **Severity:** HIGH

### M3: Grid and negative space

#### M3.1 — Responsive, stable grid

- **Current:** No layout exists.
- **Problem:** Ad-hoc dashboard layouts collapse at intermediate widths.
- **Target:** 12 columns above 1200 px, 8 columns at 768–1199 px, 4 columns below 768 px; page gutter 32/24/16 px; maximum content width 1600 px; all gaps and paddings are multiples of 4 px.
- **Severity:** HIGH

### M4: Typography

#### M4.1 — Compact dashboard scale

- **Current:** No UI typography exists.
- **Problem:** Too many sizes reduce coherence in a dense monitor.
- **Target:** One local/system sans stack and six sizes only: 24, 18, 16, 14, 12, and 11 px. H1 uses 750 weight, 1.1 line-height, and `-0.025em`; H2/H3 use 650–700; body uses 1.5 line-height. Technical IDs use the same family with tabular numbers, not a second font.
- **Severity:** HIGH

### M5: Color and semantic palette

#### M5.1 — Brand ramp plus universal state colors

- **Current:** No palette exists.
- **Problem:** Reusing ember for both brand and success/error would corrupt meaning.
- **Target:** Charcoal/parchment/ember brand ramp; green success, red error, amber warning, blue info, gray disabled. Minimum text contrast 4.5:1 and UI component contrast 3:1.
- **Severity:** CRITICAL

### M6: Dark mode

#### M6.1 — Brightness expresses elevation

- **Current:** No mode exists.
- **Problem:** Visible borders and heavy shadows would create a wireframe appearance.
- **Target:** Page `#0d0e10`, surface `#141518`, cards `#1b1c20`, popovers `#23242a`; borders no brighter than `rgba(255,255,255,0.06)`; card depth uses brightness, not shadows. The visualizer ships dark-first in v1; a light theme is not required.
- **Severity:** HIGH

### M7: Shadows and depth

#### M7.1 — Reserve shadow for overlays

- **Current:** No depth system exists.
- **Problem:** Shadows on every card would compete with live status.
- **Target:** Cards use no shadow; tooltips/popovers use `0 16px 48px rgba(0,0,0,0.45)`; pressed buttons translate 1 px and remove the outer glow.
- **Severity:** MEDIUM

### M8: Icons

#### M8.1 — One icon geometry

- **Current:** No icon system exists.
- **Problem:** Mixed stroke weights and sizes appear unfinished.
- **Target:** Custom inline SVG icons use 1.75 px strokes, round caps/joins, 20 × 20 px in controls, 16 × 16 px in chips, and 6 px icon/text gaps. Decorative service glyphs are 24 × 24 px.
- **Severity:** MEDIUM

### M9: Button system

#### M9.1 — Complete control states

- **Current:** No controls exist.
- **Problem:** Refresh and copy actions need immediate, accessible feedback.
- **Target:** Standard button padding 10 px 20 px, compact padding 8 px 16 px, 10 px radius; primary ember fill, secondary translucent surface, ghost tertiary. Implement default, hover, pressed, disabled, loading, and focus-visible states.
- **Severity:** HIGH

### M10: Inputs and feedback

#### M10.1 — Search, filters, loading, and errors

- **Current:** No input or async feedback exists.
- **Problem:** Project lists become unusable at scale and an empty load can resemble no data.
- **Target:** Search field height 40 px, 12 px radius, blue 2 px focus ring plus 4 px translucent halo; loading skeletons preserve layout; errors use red icon/text plus recovery action; empty and no-match states are distinct.
- **Severity:** HIGH

### M11: Micro-interactions

#### M11.1 — Make state changes visible without distraction

- **Current:** No transitions or confirmations exist.
- **Problem:** Silent refresh/copy interactions leave completion uncertain.
- **Target:** 150 ms hover/press transitions, 240 ms data crossfade, 2.5 s copied toast, 5 s refresh cadence, and a stale-data ribbon after one failed refresh. All nonessential animation is disabled under reduced motion.
- **Severity:** MEDIUM

### M12: Image overlays

#### M12.1 — Use README art as atmosphere, not a readability hazard

- **Current:** README hero exists only in documentation.
- **Problem:** Text placed directly on the bright artwork would fail contrast and dominate the dashboard.
- **Target:** Hero image appears only in the header atmosphere at no more than 10% opacity with a charcoal linear overlay from 0.72 to 0.96 alpha and a progressive 6 px blur mask. All text sits on the overlay/surface, never directly on the image.
- **Severity:** MEDIUM

### Implementation priority

1. CRITICAL: typed API, state semantics, grouping, hierarchy, metrics separation, contrast.
2. HIGH: project registry, responsive layout, typography, dark elevation, button/input states.
3. MEDIUM: polished icons, overlay depth, transitions, copy confirmation, reduced motion.
4. LOW: optional historical sparklines after a trustworthy time-series source exists.

## 16. Accessibility

- Use `header`, `main`, `section`, `nav`, `table/list`, and `footer` landmarks with one H1.
- Overall and per-service state changes announce through a polite ARIA live region; repeated five-second refreshes do not announce unchanged values.
- All controls are reachable and operable by keyboard in visual order.
- Focus rings are always visible and are not removed for mouse users when `:focus-visible` applies.
- Decorative lines and the hero image use empty alternative text/`aria-hidden`.
- Numeric cards include full text descriptions; abbreviations such as `1.2M` retain exact values in accessible labels/tooltips.
- Minimum target size is 40 × 40 px; pointer hover is never required to access details.

## 17. Security and privacy

- The daemon continues to reject non-loopback binds.
- The dashboard API is GET-only, read-only, bounded, and excludes secrets and raw content.
- Authenticated execution, memory, context, and usage mutation routes remain bearer-protected.
- Responses set a restrictive CSP, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and frame denial.
- Static file serving is confined to the resolved visualizer bundle directory with no directory listing.
- The UI performs no cross-origin requests and stores no operational data in local storage.
- Project paths are local operator data. The interface states this explicitly and never embeds them in outgoing URLs.
- Error messages are typed/bounded and do not expose file contents, environment variables, or tokens.

## 18. Acceptance criteria

### 18.1 Registry and API

- Fresh `hzr init` creates or refreshes exactly one private registration for the current workspace.
- Repeated initialization does not duplicate the visible project.
- Git initialization after a non-Git registration results in one visible current project after deduplication.
- Invalid, oversized, and symlinked registration files do not escape the registry root and appear only as warning counts.
- `GET /v1/dashboard` returns a typed schema with HZR version, uptime, four required service entries, projects, observed totals, estimated totals, and help commands.

### 18.2 Runtime and installation

- Starting `hzrd` makes `/` and `/v1/dashboard` available on the configured loopback address.
- Release installation with the default service setting starts the same `hzrd` process that serves the UI.
- `HZR_INSTALL_SERVICE=0` does not install or start a user service.
- No `bun`, `vite`, `vue`, or frontend development dependency is required on the end-user machine.
- The bundle manifest covers all shipped visualizer assets.

### 18.3 UI

- Desktop, tablet, and mobile screenshots pass the 12-module target values.
- Ready/degraded/rebuilding/standby/stopped/unknown remain distinguishable without color.
- Actual and estimated metrics never share a total or unlabeled chart.
- Manual refresh, search, filters, row expansion, and copy confirmation work with mouse and keyboard.
- The UI remains useful with zero projects, zero ledger records, a degraded ICM, a standby grepai watcher, an unavailable project root, and a failed refresh.

### 18.4 Verification

- `bun run typecheck`
- `bun run build`
- frontend unit tests for state labels, formatting, metric separation, filtering, and stale refresh behavior;
- Rust protocol, registry, API authorization-boundary, static-path, and init idempotency tests;
- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- `cargo test --workspace --all-targets --all-features`;
- bundle and clean-install smoke gates, including the visualizer endpoint;
- `scripts/verify-fork-core.sh --test` only if fork-core changes are required. This feature must not require a fork-core change.

## 19. Rollout and remaining decisions

The first release is read-only and cumulative. Historical charts are deferred until HZR has a trustworthy bounded time-series query rather than reconstructed estimates. Browser-triggered service controls remain deferred until a separate authenticated, CSRF-resistant operator session is designed. A future “open dashboard” CLI convenience command may launch the default browser, but automatic browser launch during installation is intentionally excluded.

The implemented 0.3.0 rollout keeps those boundaries: the visualizer starts with the
existing installed user service, while source-only development remains explicit. Local
verification covers desktop and 390 px layouts, registry state variants, typed dashboard
data, frontend tests/type checking/build, Rust workspace gates, bundle attestation, and
clean installation. Publication of hosted release artifacts remains an operator action.
