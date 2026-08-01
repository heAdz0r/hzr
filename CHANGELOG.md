# Changelog

All notable HZR changes are documented here. HZR follows semantic versioning while the public API is in `0.x` development.

## [Unreleased]

### Added

- `hzr stats` reports an **OPTIMIZER BYPASS** panel directly beneath the headline
  reduction ratio: the share of operations and of delivered tokens that reached the shell
  unfiltered, and for each bypassed tool the first-class HZR command that replaces it,
  reconstructed from the costliest recorded invocation. A bypassed operation delivers
  exactly as many tokens as it consumed, so it cancels out of the reduction ratio instead
  of lowering it; the headline alone was therefore never a sufficient measure.
- The `PreToolUse` hook answers a bypassed read or search with the equivalent `hzr`
  command already filled in — `sed -n 120,180p f` becomes
  `hzr rtk -- read f --from 120 --to 180`. The decision is `Ask`, never `Deny`: raw stays
  one keystroke away because it is the correct tool for checksums, generated files and
  complete logs.
- `hzr_codec` is exposed over MCP, so the response-density codec is reachable by agents
  rather than only through `hzr codec compile`. Its `shadow` profile measures what
  compression would have saved without altering the text.
- Codec transforms are recorded in the efficiency ledger under a `codec` subsystem, so the
  capability is justified by measurement instead of assertion.
- `hzr search --path` accepts several directories.

### Changed

- One classifier in `hzr-core::operation` now answers "did this operation go through the
  optimizer?" for the ledger, the CLI, the dashboard and the hook. The answer used to be
  computed in three places with three different rules, and bypassed operations landed in
  the `execution` subsystem where their zero savings were indistinguishable from filtered
  commands. The SQL predicate is generated from the same marker list.
- Accounting coverage is an open gap rather than a lifetime tally. It was
  `line_count(degraded-rewrites.log) == 0` over an append-only file nothing truncated, so
  a single install performed while the daemon was down pinned `hzr stats` to `INCOMPLETE`
  permanently. A successful managed rewrite now folds the gap into a lifetime total and
  closes it; the history stays visible so closing a gap never looks like erasing it.
- `hzr daemon status` reports the managed agent runtime from what is on disk instead of a
  hardcoded state and a hardcoded version string. A missing bridge or package is reported
  as degraded with the repair command, rather than as "stopped, starts on demand".
- Engines that rest by design no longer colour the overall control-plane verdict.

### Documentation

- `HZR.md` documents the `hzr rtk -- read` flags that remove the need for `sed`, `nl`,
  `cat`, `head` and `tail`; states plainly that `--mode exact` is a ranked term search and
  not `grep`; and states the ledger cost of `raw`.

- `hzr update` checks GitHub Releases, downloads the matching native bundle and
  `SHA256SUMS`, verifies the archive, and installs the newer version through the bundled
  versioned installer. The idempotent project-start initialization caches release checks
  for 24 hours and reports a newer version without making offline startup fail.

## [0.3.0] - 2026-08-01

### Added

- Deterministic LLM utility evidence for self-describing bounded reads, byte-exact
  recovery, four single-write operations, ordered batch writes, dry-run,
  idempotency, and JSON schema v1 outcomes.
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

- Markdown digests now state that content was omitted, include bounded lead prose,
  report source and section coverage, and provide exact full/range recovery hints. The read-cache schema was
  advanced so an upgraded binary cannot replay the ambiguous legacy digest.
- README positioning now treats LLM-useful output and safe WRITE/BATCH WRITE as the
  primary product criterion; command-output size remains a separately qualified metric.
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
