# Visualizer evidence, 2026-09-04

These screenshots show the modified local Vue preview at `http://127.0.0.1:47394` against the existing local daemon API. The daemon was not redeployed. Project names, paths, commands and memory contents retain the dashboard API's privacy redaction.

- `desktop-overview.png`, `desktop-projects.png`, `desktop-system.png`, `desktop-knowledge.png`, `desktop-unselected.png`: real live snapshots at 1440px viewport width.
- `mobile-overview.png`, `mobile-fold.png`, `mobile-projects.png`, `mobile-system.png`, `mobile-memory.png`: actual Chromium rendering at 390px viewport width.
- `loading.png`, `error.png`, `stale.png`: deliberately injected HTTP 503 responses used to verify UI error/recovery states; not production incident evidence.

The live counters changed during audit activity. They must not be compared with the separate frozen economic audit cohort or treated as demonstrated whole-task savings.

Browser plugin bootstrap succeeded but reported no browser backend; `agent.browsers.list()` returned an empty list. Visual QA therefore used the existing bundled Playwright package and locally cached Chromium 1228 explicitly. No browser download or deployment was performed.

## Checks

- No browser page errors in real-data navigation or error/recovery scenarios.
- Desktop: selected workspace, Overview, Projects, Memory & index, System, project search/reset, route filter/reset.
- Mobile: Overview, Projects, Memory & index, System; document width exactly 390px, no horizontal document overflow.
- Failure fixtures: loading skeleton, initial API error, Retry recovery, stale retained snapshot, stale Retry recovery, denied clipboard with explicit failure instead of success.
- Unit tests: 21 pass, 0 fail, 56 assertions across four files.
- Vue typecheck and production build passed.

Local QA harnesses used: `/tmp/hzr-ui-qa.mjs`, `/tmp/hzr-ui-states.mjs`. They are environment-specific audit helpers, not shipped test dependencies.
