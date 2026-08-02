# Changelog

All notable HZR changes are documented here. HZR follows semantic versioning while the public API is in `0.x` development.

## [Unreleased]

### Fixed

- The grepai observatory no longer runs a hard-coded semantic canary and presents it as live user
  traffic. Index readiness now comes from managed watcher and artifact evidence; the routed-search
  card is populated only by a real optimized HZR search in the selected project's recent ledger,
  with exact command, directory, latency, and available attribution. When no such request exists,
  the UI states that no routed search was observed. Volatile index generation and configuration
  hashes are no longer shown as operator-facing activity.
- **The reported `relevance` was a rank-only fusion weight, not a relevance estimate.**
  `BudgetPlanner::plan` scored every candidate as `source_weight / (60.0 + source_rank)` —
  reciprocal rank fusion with the k=60 constant calibrated for TREC runs over thousands of
  documents. Over HZR's lists of ten that pins the entire range between 1/61 and 1/70, so a
  live plan reported 0.0123–0.0164 for its best and worst evidence alike while discarding
  every engine's own magnitude. Scores are now normalized within their source, which is the
  only scale on which they are comparable, and fused as a weighted sum. Measured on the same
  question afterwards: 1.0000 / 0.7680 / 0.7477 / 0.4530.
- **Selection rewarded brevity over relevance.** `utility` divided by `sqrt(tokens)`, and with
  relevance pinned in a 15% band the expression degenerated into `source_boost / sqrt(tokens)`
  — a 30-token `Cargo.toml` outscored a 3000-token file that answered the question by an order
  of magnitude. That is the mechanism behind lockfiles and images being selected ahead of
  code. Length is now only a budget constraint, applied when filling the budget. Verified: a
  242-byte PNG that would have ranked near the top now ranks last.
- **Memory competed with code for one budget.** A memory body is prose and routinely an order
  of magnitude longer than a code candidate, so a single stale fact could consume the plan —
  observed at 10.9k of 12k tokens with the answering file not selected at all. Memory now has
  a minority share of the hard limit and is rejected with `memory_budget_share` beyond it.
- **`coverage` and `confidence` were structural constants printed beside real measurements.**
  `confidence` was `(exact + n*0.5)/n`, which is exactly 0.5 whenever no exact-mode candidate
  is present; `coverage` was source-kinds-present/4. A plan that missed the answer entirely
  still reported 0.50/0.50. `coverage` is now the share of selected evidence an agent can
  actually open, and `confidence` is how far the top candidate separates from the field —
  which is what distinguishes a pinpointed answer from a flat list of guesses.
- A code candidate with no locatable span — a PNG, a lockfile, a binary — is demoted rather
  than competing on equal footing, because it cannot answer a question about code. Memory is
  exempt: a durable fact legitimately has no line span.
- Candidates below a relevance floor are rejected with `relevance_floor` instead of padding the
  pack. A floor was previously impossible to express: with every score pinned between 0.0123
  and 0.0164 there was no threshold that separated signal from noise.

## [0.3.3] - 2026-08-02

### Fixed

- **The MCP surface and the CLI wrote to two different memory namespaces in the same
  repository.** The project namespace is derived from the directory the MCP *client*
  launched `hzr mcp serve` from, and clients choose it badly: the Claude desktop app
  launches from `/`, so every store through MCP landed in the namespace of the filesystem
  root while a CLI recall looked in the repository's namespace and found nothing. Both
  surfaces reported success. Reproduced symmetrically on a real machine — a sentinel stored
  over MCP was invisible to `hzr memory recall`, and one stored over the CLI was invisible
  to `hzr_memory_recall`. HZR now classifies the binding before using it: the filesystem
  root, the home directory and any ancestor of it can never own a project namespace, and a
  refused binding returns `isError` with the remediation instead of writing where nothing
  will read. `hzr_codec` needs no workspace and keeps working. The resolved binding is
  stated in the `initialize` handshake, and `hzr mcp config --workspace <dir>` pins it in
  the snippet the user pastes.
- **`hzr doctor` never read Claude Code's registrations.** Only Codex's `config.toml` and
  the desktop app's `claude_desktop_config.json` were audited, so a direct `icm` server in
  `~/.claude.json` — the one thing the contract forbids — passed `client_mcp_ownership`
  while spawning a second memory writer on every session start. Doctor reported only the
  resulting orphan processes and told the user to stop processes the client immediately
  respawns, so the ERROR named the symptom and never the cause. Claude Code is now audited
  (including per-project `projects.<path>.mcpServers`) and never written, and each client
  gets the remediation that actually applies to it.
- **An exact search could not be scoped to a single file.** `--path` never sent
  `--project-root` in exact mode, so the fork treated the search path as the project root
  and any file path failed with "project root is not a directory", surfaced as an opaque
  HTTP 503. The identical query in semantic mode worked, which made the failure look like a
  mode quirk rather than a missing argument.
- **A scoped search reported paths an agent could not open.** The fork reports hit paths
  relative to `--path`, so scoping to `src` reported `lib.rs`, which does not exist at the
  root, and scoping to a file reported the empty string, which normalized to `.`. Hits are
  now rebased onto the project root.
- **Semantic search emitted source truncated mid-token.** grepai chunks are byte windows,
  so a chunk's first line can begin mid-identifier; line 194 of `hook_runner.rs` came back
  as `en(Value::as_str) else {`. A fragment is now completed from the recorded line, and
  only when it provably occurs there, so an index older than the file keeps the engine's
  text rather than having an unrelated line substituted for it.
- ICM and grepai health in the visualizer is backed by supervised probes, repository-scoped
  snapshots, watcher ownership and a semantic canary instead of process-presence inference.
- The memory graph no longer overlaps labels in a decorative orbital SVG, and background
  refresh no longer resets selection, scroll, keyboard focus or graph viewport.
- Release synchronization updates only `workspace.package.version` in `Cargo.toml`
  and only source-free HZR workspace package entries in `Cargo.lock`. A dependency
  whose version happens to contain the previous HZR version can no longer be changed,
  and the subsequent `--locked` bundle build receives a synchronized lockfile.
- The context planner passes fork-core search options before `--` and the query after
  it, so exact terms such as `--outline` are never parsed as command options.
- Installation migrates stale global directives that described exact search as regex,
  batch writes as one all-files transaction, direct ICM MCP tools, or unconditional
  Bash mandates.
- Codex's managed bootstrap is executable without first assuming the contract it needs
  to load: it names the exact `hzr rtk -- read ... --level none` command.
- Instruction installation resolves the public binary symlink before locating bundle
  assets, so Claude and Codex reference the upgrade-stable `current/share/hzr/HZR.md`.
  `hzr doctor` now recognizes the executable Codex bootstrap and verifies that target.
- The macOS release smoke gate treats `/var/...` and its canonical `/private/var/...`
  spelling as the same stable installation path. It still rejects immutable `/versions/`
  references, but no longer fails a valid clean install because the runner and Rust resolve
  the system path alias differently.

### Added

- The loopback visualizer now has an operator-grade Cytoscape memory explorer with
  deterministic layout, pan/zoom/fit controls, synchronized keyboard topic navigation,
  and bounded topic-to-record drill-down through opaque project-scoped identifiers.
- Recent HZR ledger activity exposes inspectable request evidence: the requested and routed
  command, canonical working directory, route, latency, observed agent label and optional
  session identifier. Missing historical attribution remains explicitly `Unattributed`.
- A read-only `GET /v1/dashboard/memory/topics/{opaque_topic_id}` contract returns at most
  100 positively repository-filtered records with independently bounded detail fields.
- **Plan candidates carry the symbols an agent can jump to.** A candidate used to be
  `{path, score, sources, estimated_tokens}` — nothing that could not be had from `ls` — so
  an agent opened every file anyway, which is the work the plan exists to save, while the
  budget went to memory bodies. `symbol`, `line_start` and `line_end` have been declared in
  the protocol since the first release and were never filled in. Each candidate now carries
  its symbol outline with line spans from the fork's own extractor
  (`rtk read <file> --symbols`), bounded to 24 symbols with the omitted count and the
  command that shows the rest. Outlines are best-effort: an unsummarisable file degrades to
  the path it was before rather than failing the plan.
- **`client_mcp_workspace` doctor check.** A registered MCP server with no `--workspace`
  takes its namespace from the client's launch directory. That is the cause of the split
  namespace above and was previously silent.
- `hzr mcp config --workspace <dir>` pins the project in the printed registration.

- `read --outline` now recognizes Markdown and emits its ATX heading hierarchy with
  original source spans. Supported code files continue to use their symbol extractor.
- Exact search accepts literals beginning with `-` through the conventional `--`
  separator, for example `hzr search --mode exact -- "--outline"`.

### Changed

- Dashboard synchronization runs quietly every five seconds while visible, permits only one
  in-flight request, preserves the graph camera and open inspectors, and never animates or
  disables the manual Refresh control during a background poll.
- RAW rows now contribute baseline equal to delivered output and exactly zero avoided,
  regression and net tokens in both global and project aggregates, even if a legacy or
  malformed row stored unequal counts.
- A subagent receives a brief instead of a JSON dump. The plan was prepended as a minified
  `ContextPlanApiResponse` with no glossary and no statement of what the entries were, so a
  subagent either ignored it or treated ranked guesses as findings. Leads are now named as
  `path:start-end` with their symbol, declared unverified, and paired with the commands that
  confirm them; an empty plan says it found nothing.
- The contract states what the hook covers. The `PreToolUse` matcher is `Bash|Agent|Task`,
  so nothing redirects a native `Read`, `Grep`, `Edit`, `Write` or `Glob` and nothing records
  one — those calls are absent from `hzr stats` on both sides of its ratio. The preference
  table is the agent's to follow, not something the hook enforces.
- The codec is described as what it is: removal of exact duplicate paragraphs under verified
  protected spans, not prose compression. Verified on 4 KB of deliberately verbose
  single-paragraph prose, which comes back byte-identical — a correct result, not a failure.
- `hzr rtk -- memory explore <dir>` and `read --symbols` are documented in the command
  table. The Explore-deny hook already pointed agents at `memory explore` while the contract
  omitted it.
- `mcp serve` is matched as an argument prefix in client configurations, so pinning a
  workspace no longer makes a correct registration report as unregistered.
- The fork-core integration fixture reports hit paths the way the real engine does, and fails
  when `--project-root` is absent. It previously modelled behaviour the engine never had,
  which is what let the file-scoping defect survive.

- `read -n` defaults to exact content and preserves source coordinates for full,
  ranged and tail reads instead of numbering filtered output from one.
- `read --max-lines N` is an exact head operation; it no longer substitutes a smart
  truncation marker for omitted lines.
- Global managed agent instructions now define only HZR tool routing, include all five
  MCP tools, and state the exact semantics of reads, per-file batch atomicity,
  read-only `mcp config`, and direct-spawn `raw`.
- Legacy imported memories remain available for explicit audit/migration but are
  quarantined from automatic project recall because they carry no trustworthy
  repository provenance.

### Documentation

- Added the 12-module Observatory 2.0 PRD, independent acceptance gate and sanitized public
  screenshots of service health, memory drill-down and per-request evidence.
- README, the canonical agent contract, Claude/Codex integration guides, fork docs and
  the parity ledger now agree on the corrected behavior and scope.

### Known gaps

- `hzr memory` still has no `forget` or `prune`, so cross-project rows written into a
  project namespace by the legacy import cannot be removed through the sanctioned control
  plane. The engine supports it; the HZR path does not exist yet.


## [0.3.2] - 2026-08-02

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
- `rtk rgai --literal` matches a query verbatim and case-sensitively instead of ranking its
  terms, and implies `--builtin` because a literal lookup is never a semantic one.

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
- `hzr search --mode exact` is finally exact. It sent `--builtin` to fork-core, which
  handed the query to the ranked term model: lowercased, split on non-alphanumerics,
  de-stopped, stemmed and OR-ed, so `hzr search "fn record_degraded_rewrite" --mode exact`
  top-hit a patch file on the token `fn` and never returned the definition. Exact mode now
  sends `--literal`. Measured on this repository, the same query returns 1 file instead of
  21.

### Fixed

- Release version synchronization is restricted to an explicit list of HZR-owned surfaces and
  the two root version fields in the npm lockfile. A transitive package with the same numeric
  version can no longer be rewritten into a nonexistent dependency release.
- The complete workspace builds on the declared Rust 1.85 MSRV without unstable let-chain syntax.
  Fork-core retains its separate pinned compiler and deterministic warning ratchet.
- Public upgrade guidance uses `hzr update`; `hzr release` remains a maintainer command for
  constructing and switching a source release.
- The GitHub Release description is `RELEASE_NOTES.md` verbatim instead of an auto-generated
  commit list, and the tag workflow refuses to publish when that file does not name the tag.
  The published release and the repository can no longer describe the same version
  differently.
- GitHub workflows use Node 24-compatible official actions and skip Go dependency caching in
  jobs that clone their `go.sum` only after setup, removing deprecation and false cache-failure
  annotations from otherwise successful CI runs.

### Documentation

- `HZR.md` documents the `hzr rtk -- read` flags that remove the need for `sed`, `nl`,
  `cat`, `head` and `tail`; separates the literal and ranked search modes; and states the
  ledger cost of `raw`.
- Release documentation now has three enforced responsibilities: `RELEASE_NOTES.md` explains the
  current upgrade, `CHANGELOG.md` keeps exhaustive history, and `docs/releases/vX.Y.Z.md` preserves
  the immutable statement published with an older release.

## [0.3.1] - 2026-08-01

### Added

- Deterministic LLM utility evidence for self-describing bounded reads, byte-exact
  recovery, four single-write operations, ordered batch writes, dry-run,
  idempotency, and JSON schema v1 outcomes.
- `hzr update` checks GitHub Releases, downloads the matching native bundle and
  `SHA256SUMS`, verifies the archive, and installs the newer version through the bundled
  versioned installer. The idempotent project-start initialization caches release checks
  for 24 hours and reports a newer version without making offline startup fail.
- Interactive, privacy-safe current-project ICM topic graph with verified FTS5-only Ready
  semantics and snapshot freshness evidence.
- Typed grepai artifact, generation, watcher PID/uptime, and semantic-canary observability.
- Live project-scoped output accounting with explicit optimized/RAW routes, zero RAW savings
  credit, and first-class HZR replacement suggestions for avoidable bypasses.

### Fixed

- Same-minor release synchronization now keeps the stable `0.3.x` release line.
- Semantic observability canaries no longer manufacture ledger activity.
- Missing provider receipts render as unavailable evidence rather than numeric zero usage.

### Changed

- Markdown digests now state that content was omitted, include bounded semantic lead prose,
  report source and section coverage, skip HTML navigation/media noise, and provide exact
  full/range recovery hints. The read-cache schema was advanced so an upgraded binary cannot
  replay an ambiguous legacy digest.
- README positioning now treats LLM-useful output and safe WRITE/BATCH WRITE as the
  primary product criterion; command-output size remains a separately qualified metric.

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

[0.3.3]: https://github.com/heAdz0r/hzr/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/heAdz0r/hzr/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/heAdz0r/hzr/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/heAdz0r/hzr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/heAdz0r/hzr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/heAdz0r/hzr/releases/tag/v0.1.0
