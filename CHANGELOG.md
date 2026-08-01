# Changelog

All notable HZR changes are documented here. HZR follows semantic versioning while the public API is in `0.x` development.

## [Unreleased]

## [0.3.0] - 2026-08-01

### Added

- Bun-built Vue local visualizer, served by the existing loopback `hzrd`, with typed
  service/project health, versions, help commands, and strictly separated observed
  usage versus direct-efficiency estimates.
- Private atomic workspace registrations refreshed by `hzr init`, allowing one dashboard
  to list Git, non-Git, and linked-worktree projects without scanning arbitrary paths.
- Typed `hzr_context_plan` MCP tool and path/content controls for repository search.
- Read-only `hzr mcp status` reporting native client registrations and the
  client-managed stdio lifecycle.
- Native `hzr tdd` text/JSON contract and bundled `hzr-tdd` skill, adapted from
  upstream RTK with explicit RED evidence and HZR's complete Rust quality gate.
- `hzr release VERSION` synchronizes current product/version surfaces, refreshes
  fork-core provenance, builds the bundle, switches the global install, and verifies
  the installed public binary plus every pinned engine. Pinned source archives use a
  checksum-verified local download cache for repeatable same-version rebuilds.

### Changed

- Installed-bundle `hzr init` and confirmed `hzr install` now ensure the single
  daemon/visualizer user service is running; `--skip-service` and
  `HZR_INSTALL_SERVICE=0` preserve controlled opt-out behavior.
- MCP now negotiates stable protocol `2025-11-25` with compatible older clients,
  publishes strict JSON Schema 2020-12 input/output contracts, and returns
  `structuredContent` alongside backward-compatible text.
- Invalid MCP tool arguments fail explicitly instead of silently using defaults.
- Init output now states that MCP is launched by the registered client on
  connection; `hzr init` never spawns a persistent MCP process.
- Semantic path filters can no longer create nested grepai indexes; the canonical
  workspace root remains the sole index owner.
- Execution cancellation owns the complete process group, memory recall preserves exact
  project/global scopes, and ICM/circuit/watcher lifecycle transitions reject stale races.
- Workspace discovery, planner graph expansion, and usage-ledger writes now reuse cached
  indexes and single-owner structures instead of repeated whole-tree/whole-graph/DDL work.

## [0.2.0] - 2026-08-01

First public HZR release.

### Added

- One self-contained, versioned installer bundle for supported macOS and Linux platforms.
- Global Claude and Codex adoption contracts with a single HZR dispatcher.
- HZR-owned MCP gateway for memory and search without additional durable stores.
- Canonical workspace identity, one semantic index generation, and centralized memory lifecycle.
- Managed agent path with protected response-density contracts.
- Global cumulative `hzr stats` accounting with actual provider usage kept separate from estimated local effects.
- Release provenance, engine pins, internal bundle manifest, checksums, clean-install smoke, and native artifact workflow.

### Changed

- The complete inherited engine now evolves inside HZR while the `v0.1.0` import remains immutable provenance.
- Public product naming, documentation, hooks, instructions, and compatibility entrypoints use HZR ownership.

### Security

- Loopback-only authenticated daemon API.
- Fail-closed archive, checksum, symlink, path, and engine-version validation.
- Private vulnerability reporting and documented supply-chain provenance.

## [0.1.0] - 2026-07-31

- Established the independent HZR repository and immutable baseline of the complete proven engine.

[0.3.0]: https://github.com/heAdz0r/hzr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/heAdz0r/hzr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/heAdz0r/hzr/releases/tag/v0.1.0
