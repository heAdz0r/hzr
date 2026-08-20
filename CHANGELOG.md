# Changelog

All notable HZR changes are documented here. HZR follows semantic versioning while the public API is in `0.x` development.

## [Unreleased]

## [0.4.1] - 2026-08-21

### Added

- A High Critical release acceptance gate now fails publication when a command with a safe
  first-class HZR route can remain RAW in core classification, the hook, or daemon execution.
- The all-workspace RAW-utilization review records the frozen ledger snapshot, per-workspace
  exposure, highest-cost command families, safety boundary, and release criteria.

### Changed

- Managed Codex and Claude instructions now route ordinary shell work through
  `hzr exec run '<shell command>'`. RAW is an explicit byte-for-byte recovery path rather than
  the default wrapper for SSH, JSON, pipelines, or unfamiliar arguments.
- Unmarked managed RAW/proxy wrappers are removed byte-for-byte before fork-core rewrite, so
  existing `bun`, `cargo`, `ssh`, `git`, `gh`, `find`, `wget`, `ps`, and other
  first-class handlers remain the single command-support authority.
- Exact unfiltered recovery now requires the explicit `HZR_RAW_FIDELITY=1` marker; this preserves
  a real fidelity escape hatch without letting habitual RAW wrappers bypass managed policy.
- Managed execution carries the caller's `PATH` into the daemon envelope without forwarding
  unrelated environment variables, so launchd/systemd service environments can still resolve
  user toolchains.

### Fixed

- Safe text replacements for `nl`, ranged `sed`, and literal `rg` are applied
  automatically instead of leaving an avoidable RAW decision or tracked raw proxy for the agent.
- Automatic replacement refuses quoted arguments, regex/glob syntax, nested shell grammar,
  unsupported flags, and ambiguous multi-file commands rather than reconstructing them through
  whitespace tokenization.
- Automatic `cat` replacement selects `hzr rtk -- read --level none`, preserving exact Markdown
  content instead of applying the default digest.
- `hzr exec run` no longer loses tools such as `cargo` when the daemon was started with a
  restricted service `PATH`; approved rewrites preserve the same validated caller path.
- The managed Caveman runtime replaces vulnerable `extract-zip` 2.0.1 with a vendored compatible
  fork that rejects non-empty roots, escaping archive paths, and symlink components or targets
  before directory creation. Bundle builds attest the complete vendor tree, exercise a malicious
  symlink fixture, and retain the high-severity npm audit gate.
- Release deployment allows fifteen seconds for launchd/systemd to replace the daemon, avoiding a
  false failure after a successful bundle switch while retaining a bounded version-health gate.

## [0.4.0] - 2026-08-10

### Added

- `hzr stats --all` explicitly recovers the complete exact per-command history when an
  operator needs it; the default JSON and human reports now state how many rows were omitted
  and name that recovery command.
- `hzr doctor --fix` transactionally migrates one unambiguous legacy `.grepai`, retains
  the verified backup and manifest, then reports the post-repair health state. Duplicate or
  conflicting indexes fail without mutation.

### Fixed

- `rtk write` no longer treats the target file as an observed native Edit response. New and
  historical write rows are savings-neutral, so utilization cannot manufacture avoided tokens.
- `rtk read --changed` now compares a rendered diff with the real file baseline and falls back
  to the file when the diff would be larger, eliminating the marker-versus-diff regression.
- MCP operation accounting carries typed agent and inherited session attribution through the
  daemon into the canonical ledger.
- Search and context planning no longer surface HTTP 503 when a project has a legacy local
  index. HZR stays inside its single control plane and uses the fork-core builtin fallback
  without activating or modifying the legacy index.

### Changed

- Default `hzr stats` output is bounded to 12 command groups and 12 bypass-tool groups, redacts
  long, multiline, and raw command payloads, and describes runtime completeness as applying only
  to observed channels.
- Local output reduction remains explicitly separate from provider-billed usage. The release
  makes no economic-savings claim without paired provider receipts and accepted-task quality.

## [0.3.9] - 2026-08-08

### Changed

- TDD is now opt-in rather than automatically required for every agent implementation.
  Agents use `hzr tdd` when the user or repository requires it, or when regression risk
  justifies test-first overhead; token- or time-sensitive work may use proportionate
  verification instead.
- The `hzr tdd --json` contract adds `required: false` while retaining `strict: true`:
  selecting TDD still requires observed RED, identical-command GREEN, and refactoring
  while green. Skipping TDD does not waive repository-required quality gates.
- Managed Claude/Codex instructions, MCP initialization guidance, bundled awareness files,
  the canonical `hzr-tdd` skill, CLI help, README, and contributor guidance now describe
  the same opt-in policy.

## [0.3.8] - 2026-08-06

### Added

- Root `hzr --help` groups commands (Setup, Runtime, Search & Memory, Agent tools,
  Distribution, Legacy), fills global/common flag help, adds an Examples footer, and
  applies light HZR orange styling.
- High-traffic subcommands (`update`, `stats`, `enable`/`disable`, `doctor`, `agent`,
  `mcp`, `memory` leaves) gain `long_about` text and Examples footers.
- `hzr update --check` reports whether a newer GitHub release is available without
  downloading or installing (exit 0 when the check succeeds; non-zero only on failure).
- `hzr activation status` lists project-only activation mode and enabled workspace roots
  (human and `--json`).
- `hzr install` and `hzr mcp config --apply` pin MCP registrations with
  `mcp serve --workspace <dir>` so Desktop/Codex memory is not bound to `/`.
- `hzr doctor` warns when Claude Code has no HZR MCP registration and prints the
  `claude mcp add` remediation.
- `hzr doctor` human output separates permanent host limits (`*_global_codec`) as NOTE and
  lists actionable remediations under **Next actions:** (JSON schema unchanged).
- `hzr doctor` reports `index_readiness` when the semantic index is missing or not yet
  warm, with remediation to `hzr index init` or wait for the hzrd watcher.

### Changed

- Provider usage receipts may carry a workspace `project_path`. `hzr stats --workspace`
  labels matching receipts `project_matched` and omits older unscoped rows from project
  provider totals; the global lifetime view still includes every receipt. Estimated
  savings remain separate from billed provider counters.
- `hzr stats` always shows the MCP channel in coverage, including `mcp=0` when there was
  no MCP traffic.

### Documentation

- Retired stale “no forget/prune” known-gap text that no longer matches the current CLI.
  README basic commands list `hzr memory forget|update|prune` alongside recall/store/status,
  plus `enable`/`disable` where relevant.

## [0.3.7] - 2026-08-05

### Added

- Operation accounting now records typed `channel`, `measurement`, and `route` dimensions. MCP,
  hook/CLI, and native-host traffic are reported separately; successful MCP tools contribute one
  neutral coverage row without receiving unmeasured savings credit.
- Claude installs a failure-silent `PostToolUse` observer for native `Read`, `Grep`, `Glob`,
  `Edit`, and `Write` calls. It records response-size estimates and attribution, never tool
  content, and never mutates or blocks a host result.
- Context candidates expose a typed reason when no symbol can be resolved; search candidates use
  their symbol outline and line span to select the smallest enclosing symbol.
- Long MCP search/context calls emit progress notifications, foreground `hzrd` announces startup,
  and long `hzr agent run` calls emit periodic liveness heartbeats.

### Changed

- `hzr stats` computes reduction only from measured non-native rows and prints the observed
  coverage share, explicitly unmeasured bypass count, daemon-unavailable gaps, and channel split.
  Estimated output reduction remains separate from provider-billed usage.
- Bounded reads, ranged reads, long-line shortening, memory exploration, memory recall and search
  now state totals or omitted volume and name the exact recovery command or detail level.
- MCP output schemas declare nested properties and required fields for every emitted structured
  result; representative outputs are checked against those contracts.
- The dashboard distinguishes optimized, measured RAW, native-observed, and unmeasured traffic so
  operations outside the reduction ratio cannot look optimized.
- Installer and bundle assembly show non-TTY transfer/stage progress and bound downloads with an
  overall timeout.

### Fixed

- Inherited-stdio passthrough no longer writes a misleading `0/0` token row. It is explicitly
  unmeasured and cannot carry invented token counts; captured RAW output remains neutral with
  equal baseline and delivered estimates.
- Exact search preserves leading/trailing whitespace and reports its effective mode, fallback,
  scan diagnostics, and next step even when it finds no matches.
- Public symlink swaps use the portable replacement helper, and an identical
  `hzr install --force` preserves bundle entry-point symlinks.
- `hzr doctor` recognizes the canonical three-hook installation, including the default-on
  `PostToolUse` observer, instead of reporting a clean install as conflicting.

## [0.3.6] - 2026-08-05

### Fixed

- A successful “no update” result now expires after one hour instead of suppressing a release
  published later that day for 24 hours. Cached positive notices retain the 24-hour TTL, and
  network failures remain fail-open.
- Claude SessionStart update notices now use structured hook output that reaches both the visible
  UI and agent context. The agent context requires one user notification and forbids installation
  without explicit approval.
- Codex now checks the cached/published version during its mandatory installed `HZR.md` bootstrap.
  The notice is emitted separately from fork-core stdout, preserving byte-exact `hzr rtk -- read`
  output while making the update visible to the model.
- Dormant nested `.grepai` directories left by older grepai/RTK invocations no longer disable the
  canonical HZR index. HZR reports them as a doctor warning, never activates or mutates them, and
  keeps explicit migration strict when multiple stores make source selection ambiguous.

## [0.3.5] - 2026-08-05

### Added

- First-class project-only activation through `hzr install --project-only`, `hzr enable`,
  `hzr disable`, and the release-installer `HZR_PROJECT_ONLY=1` override. Enabled workspaces are
  keyed by validated repository/worktree identities; disabling preserves index, memory, workspace
  registration, and ledger history.
- `hzr stats --workspace <dir>` applies one separator-safe project filter to the headline,
  subsystem and command totals, and optimizer-bypass denominator. Provider receipts and degraded
  hook coverage remain explicitly global because those records do not carry project attribution.

### Changed

- Selected activation keeps one global Claude dispatcher but makes `SessionStart` and `PreToolUse`
  no-ops outside enabled workspaces. Managed instructions move to project-root `CLAUDE.md` and
  `AGENTS.md`; HZR-owned client-global MCP registrations are removed with transactional backups.
- MCP project tools now require a safe, initialized HZR workspace and, in selected mode, an enabled
  repository/worktree identity. Uninitialized and unselected bindings fail before daemon dispatch.

### Fixed

- The dashboard reports a ready on-disk index with its on-demand watcher at rest as `Standby`
  instead of claiming that a rebuild is active indefinitely after a daemon restart.
- Project data isolation no longer masquerades as project-level activation: the default remains
  all-project, while the selected mode now gates instructions, hooks, MCP, and statistics as one
  auditable contract.

## [0.3.4] - 2026-08-02

### Added

- The installer reports five bounded progress steps, shows interactive transfer progress for the
  release bundle, and finishes with the installed locations, dashboard URL, conditional PATH
  guidance, health command, and next-use examples.

### Fixed

- Installer cleanup now removes a pending atomic `current` symlink if installation exits between
  creating the temporary link and replacing the active-version pointer.
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
  only scale on which they are comparable, and fused as a weighted sum. Deterministic tests
  preserve large relevance gaps and reject the weak tail below the configured floor.
- **Selection rewarded brevity over relevance.** `utility` divided by `sqrt(tokens)`, and with
  relevance pinned in a 15% band the expression degenerated into `source_boost / sqrt(tokens)`
  — a 30-token `Cargo.toml` outscored a 3000-token file that answered the question by an order
  of magnitude. That is the mechanism behind lockfiles and images being selected ahead of
  code. Length is now only a budget constraint, applied when filling the budget; a deterministic
  regression test requires relevant locatable code to outrank a similar-scoring opaque artifact.
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
- High and irreversible codec requests now force exact fidelity in the production daemon path;
  the request risk field is no longer ignored.
- Managed agents load bounded root `AGENTS.md` and `CLAUDE.md` instructions while keeping the
  Caveman SDK's duplicate tools, hooks, memory, and repo map disabled. Prefetched context is a
  compact, bounded evidence brief rather than raw JSON.
- Context planning now reserves configured output and safety tokens, caps memory to a minority
  budget share, bounds each long memory while preserving its latest tail, and adds exact symbol
  search when the intent names an identifier.
- `hzr memory update`, `forget`, and namespace-scoped `prune` are available through the CLI,
  daemon API, MCP, and managed-agent bridge. Destructive selection uses the same positive
  project/global namespace filter as recall; API, CLI, and MCP prune default to dry-run, and
  threshold pruning preserves high and critical memories regardless of weight.
- MCP tool calls run concurrently and honor `notifications/cancelled`. Managed provider-usage
  receipts that cannot reach the daemon are persisted as private per-event outbox files and
  replayed exactly once after the daemon recovers.
- Public dashboard memory details redact content-bearing fields. Full bounded details moved to
  a bearer-authenticated endpoint.

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

### Known gaps (as of 0.3.3; closed in 0.3.4)

- As of 0.3.3, `hzr memory` had no `forget` or `prune` on the HZR control plane, so
  quarantined legacy-import rows could not be removed through sanctioned commands. Closed
  in 0.3.4: use `hzr memory forget`, `hzr memory update`, and `hzr memory prune` on current
  releases.


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

[0.4.1]: https://github.com/heAdz0r/hzr/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/heAdz0r/hzr/compare/v0.3.9...v0.4.0
[0.3.9]: https://github.com/heAdz0r/hzr/compare/v0.3.8...v0.3.9
[0.3.8]: https://github.com/heAdz0r/hzr/compare/v0.3.7...v0.3.8
[0.3.7]: https://github.com/heAdz0r/hzr/compare/v0.3.6...v0.3.7
[0.3.6]: https://github.com/heAdz0r/hzr/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/heAdz0r/hzr/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/heAdz0r/hzr/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/heAdz0r/hzr/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/heAdz0r/hzr/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/heAdz0r/hzr/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/heAdz0r/hzr/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/heAdz0r/hzr/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/heAdz0r/hzr/releases/tag/v0.1.0
