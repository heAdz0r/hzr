# HZR 0.3.0 release status

**Status:** locally released and verified on macOS arm64 on 2026-08-01. This document
does not claim that GitHub artifacts or a public tag have been published.

## Product version and runtime

- The workspace product version, release metadata, Caveman bridge package, smoke
  assertions, and current public documentation declare HZR `0.3.0`.
- The global `current` pointer resolves to `v0.3.0-darwin-arm64`; `hzr --version` and
  the restarted daemon report `0.3.0`.
- Verified engine pins are RTK fork-core `0.44.1-fork.1`, grepai `0.35.0`, ICM
  `0.10.61`, caveman-code `0.65.2`, and Node.js `22.17.1`.
- References to `v0.1.0` and `v0.2.0` in provenance, changelog, and historical
  benchmark records are intentionally immutable rather than stale version fields.

## Accepted Rust quality work

The approved audit backlog in
[`docs/PRD_RUST_QUALITY_AUDIT_20260801_184239.md`](docs/PRD_RUST_QUALITY_AUDIT_20260801_184239.md)
is implemented: canonical semantic-index ownership, cancellation-safe Unix process
groups, exact memory scopes, generation-aware circuit transitions, serialized ICM and
watcher lifecycle, cached workspace discovery, reverse planner indexes, a bounded
single-owner SQLite writer, and a fork warning/provenance ratchet.

The accidental `crates/.grepai` was moved to Trash instead of being permanently
deleted. An old orphan watcher recreated it once; the orphan was terminated and the
recreated directory was also moved to Trash. A real path-filtered context plan and the
current doctor report confirm that the canonical project now has one index owner.

## Release-control changes

`hzr release VERSION` now validates strict SemVer, synchronizes canonical version and
lock surfaces, updates release documentation, refreshes fork provenance, synchronizes
the Caveman package-lock digest, builds and smoke-tests the bundle, generates and
verifies the complete bundle manifest, installs through the versioned layout, restarts
the installed service, and polls the daemon API until the requested version is live.
Pinned archives use a SHA-256-verified download cache. Any failed check aborts the
release instead of reporting partial success.

## Local visualizer

HZR 0.3.0 includes a Bun-built Vue operator dashboard served by the existing loopback-only
`hzrd`. It reports registered projects, daemon and pinned-engine state, project index
readiness, provider-observed usage, separately labeled deterministic efficiency estimates,
exact diagnostic commands, and HZR/protocol/UI versions. Initialization refreshes the
workspace registry; default release installation starts the same user service that serves
the UI, with `HZR_INSTALL_SERVICE=0` as the explicit opt-out.

The implementation contract and captured real-UI evidence are documented in
[`docs/PRD_HZR_VISUALIZER.md`](docs/PRD_HZR_VISUALIZER.md) and the public README.

## Verification

- Rust formatting and workspace Clippy with `-D warnings`: pass.
- Full Rust workspace tests with all targets and features: pass.
- Complete fork-core deterministic gate: pass; 1702 primary tests passed and one was
  ignored, with every additional suite passing.
- Fork inherited-warning ratchet: pass at exactly 141 warnings.
- Bundle build, manifest attestation, install smoke, engine-version checks, and daemon
  restart/version probe: pass.
- Visualizer unit tests, TypeScript validation, production build, desktop/mobile browser
  inspection, and assembled-bundle endpoint smoke: pass.
- `hzr doctor`: healthy; no nested project index and no foreign grepai/ICM owner.

## Known boundaries

- No Windows artifact is shipped in 0.3.0, so Unix process-group cancellation is not
  presented as Windows process-tree support.
- Workspace/planner Big-O improvements are covered by structural and regression tests,
  but no new 1k/10k/100k performance claim is published without a dedicated measured
  benchmark.
- ICM is intentionally configured for FTS-only/on-demand operation; the daemon may
  describe memory as degraded until it is requested even while `hzr doctor` is healthy.
- Public tag publication and multi-platform hosted CI are release-operator actions and
  were not performed by this local release command.
