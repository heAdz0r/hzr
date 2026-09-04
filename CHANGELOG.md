# Changelog

All notable HZR changes are documented here. HZR follows semantic versioning while the public API is in `0.x` development.

## [Unreleased]

<<<<<<< HEAD
## [0.8.0] - 2026-09-04

### Added

- Durable execution start/wait/cancel operations with stable IDs and explicit interruption state.
- Typed batch/range reads with output budgets, snapshot hashes and completeness metadata.
- Exact memory lookup, optional context memory, and MCP execution lifecycle controls.
- Consistent time-bounded fleet statistics, privacy-safe export and separate readiness dimensions.
- A paired-task evaluation harness that validates per-request provider usage and independent
  acceptance; offline fixtures never count as economic proof.

### Changed

- Redesigned the dashboard around operational evidence, useful filters and visible unknown states.
- Preserved justified full reads and removed optimizer-only approval where no equivalent route exists.
- Budgeted delivered context evidence rather than the full files that might be read later.
- Kept host permission authority intact and stopped blocking native reads to force tool retries.
- Rejected cumulative ledger counters as evidence for an individual benchmark run.

### Fixed

- Corrected scoped semantic and graph path resolution.
- Preserved test failure diagnostics and compiler warning signals in managed output.
- Added durable ICM ownership, graceful SIGTERM and recovery for verified owned unready children.
- Corrected producer accounting lifecycle and command-family attribution.
- Accepted empty and whitespace-only MCP writes consistently with their schema.

This is the prepared source version. Release publication and billed/task-level savings are not
implied by these entries.
=======
### Changed

- Corrected the cache-aware filter placement model (PRD 2026-09-04, F13). `turn_boundary` no
  longer declines every mid-turn filter: a `PreToolUse` command rewrite shapes a tool result the
  host appends after the cached request prefix and cannot invalidate it, so it now filters at any
  turn position. Only transforms classified as `PrefixEffect::HistoryMutation` are deferred to the
  turn boundary. Session state counts both `placement_deferred_operations` and the new
  `placement_append_filtered_operations` so an arm comparison can size what each rule kept.
- Replaced the default response-codec round trip with concise generation (F15). Managed
  instructions, the Claude Code SessionStart notice, the `hzr_codec` MCP description and the
  contract documents now tell agents to write concisely and *not* to send generated prose back
  through `hzr_codec`; a transform applied after generation cannot refund tokens already emitted
  and only removes exact duplicate paragraphs. `hzr_codec` remains available on explicit request
  and for `shadow` counterfactual measurement. The capability contract records the policy as
  `default_prose_policy = concise_generation_no_post_hoc_transform`.
- Tiered the MCP catalog in every session prefix (W12). `contracts/agent-capabilities.json`
  marks each tool `core` or `specialist`; the managed instruction block keeps a table row only for
  core tools and names the six specialist tools once in one line. The compacted `hzr_codec`
  entry shrinks the `tools/list` result from 25,574 to 25,092 wire bytes and the managed block
  from 6,904 to 6,593 bytes (Claude) and 6,325 to 6,014 bytes (Codex); a new gate holds both
  below their pre-W12 sizes. These are payload bytes, not tokenizer or billed measurements.
- Labelled the context-plan `confidence` figure as an uncalibrated ranking diagnostic (W13,
  F09). `ContextPack` gains `ranking { method: "top_candidate_separation_v1", calibrated: false }`
  in the protocol, the MCP output schema and the CLI summary line; the numeric field keeps its
  name for wire compatibility. Calibration against held-out relevant-file labels remains
  blocked on the paired evaluation harness (W11) and is not claimed.
>>>>>>> feat/prd-p2-agent-efficiency

## [0.7.1] - 2026-09-04

### Fixed

- Prevented development and smoke binaries from writing real Codex or Claude Desktop
  configuration even when the fixture override is present; that override is now accepted only
  for configuration files inside the system temporary directory.
- Stopped global installation from retargeting a valid Claude Desktop workspace whenever HZR is
  installed from another repository. Missing, development, and temporary selections are still
  repaired, while deliberate selection remains available through `hzr mcp config --apply`.
- Rejected MCP initialization before exposing tools when an explicit workspace conflicts with the
  single root supplied by the client, preventing reads or memory calls in a foreign namespace.
- Prevented `init --if-needed` launched from the user home from treating the user-level Codex
  registration as a project file and reintroducing a home-directory workspace pin.
- Preserved Git-tracked shared `AGENTS.md` and `CLAUDE.md` contracts during automatic
  `init --if-needed`, fleet reconciliation, and cross-workspace health audits instead of treating
  repository-owned policy as obsolete managed state or poisoning every other workspace's doctor.

### Changed

- Codex's user-level HZR registration is now an unpinned dynamic fallback. Exact workspace pins
  remain in each repository's `.codex/config.toml`, while the fallback resolves the current client
  root instead of carrying whichever repository last ran the global installer.

## [0.7.0] - 2026-09-03

### Fixed

- Restored hook-command accounting with daemon-owned correlation contexts and receipt sweeping.
  Hook rewrites, direct `hzr exec run`, approved continuations, and daemon-owned fork operations
  such as search all register their correlation before execution. Undrained journals now make live
  coverage incomplete, denied-command contexts expire safely, and the 0.6.6 loss interval remains
  explicitly historical instead of fabricated.
- Bounded unsupported `find` passthrough at 200 lines and 16 KiB and records it as unmeasured
  bypass. Default `hzr read` output is bounded at 400 lines and 44 KiB with exact recovery.
- Replaced raw-output savings headlines with separately reported host-visible capped estimates;
  unknown host caps disable potential pricing and leave raw estimates labelled as upper bounds.
- Deduplicated legacy SessionStart hooks, fixed the historical-coverage false warning, protected
  real client configs from development/smoke writes, and added missing-workspace remediation.
- Allocated the daemon-owned ICM transport on a private loopback port so isolated release smokes
  can run alongside the user's live daemon without colliding with its memory service.
- Kept `hzr install --dry-run` transaction-free when hooks, instructions, or project MCP files
  would change; previews now report those changes without requiring a write transaction.
- Kept the new bounded output rendering compatible with the declared Rust 1.85 MSRV.
- Bounded instruction-file audits so an unavailable cloud-backed workspace contract cannot hang
  `doctor`, fleet reconciliation, or diagnostics for every other registered workspace.
- Updated the bundled Caveman dependency lock to `fast-uri` 3.1.7; the production release audit
  now reports zero high-severity vulnerabilities.
- Generated the aggregate release `SHA256SUMS` with standard two-space separators so both GNU
  `sha256sum -c` and BSD `shasum -a 256 -c` resolve every archive filename correctly.
- Refused project initialization at home or filesystem root, inherited identical managed
  instruction blocks, and diagnosed stale user-global HZR instruction conflicts.

### Changed

- Unpinned Codex and Claude Code MCP sessions can resolve one workspace from initialize roots,
  `CLAUDE_PROJECT_DIR`, or cwd; explicit pins remain authoritative and unsafe roots fail closed.
- Global memory stores accept only user-wide topic classes. Recall is compact by default with
  explicit expansion, and FTS5-only retrieval is now a documented capability boundary.
- Top-level `hzr` commands bypass recursive fork proxying. No-equivalent shell constructs are not
  called avoidable, exact search output hides generation hashes unless verbose, and read/write
  help exposes the supported native options.
- Removed empty `.rtk-lock` files from the current fork manifest without changing the immutable
  import snapshot.

## [0.6.6] - 2026-08-28

### Fixed

- Fork-core accounting now crosses one typed, payload-free receipt contract and is committed
  idempotently by HZR's daemon-owned ledger writer. Producer failures become durable live coverage
  gaps; bypassed and unmeasured operations cannot claim savings.
- Public stats aggregate by the privacy-safe typed operation key before ranking and rendering, so
  different private commands no longer appear as duplicate public rows. Totals, negative results
  and ratios below 100% remain truthful.
- Instruction-scope changes, local excludes, activation, hooks, project MCP state and uninstall are
  serialized desired-state transactions with rollback. Local mode keeps HZR instructions out of
  repository-shared `AGENTS.md` and `CLAUDE.md`; shared mode remains the default.
- Hook feedback, observation and statusline use the authoritative workspace from hook input.
  Accounting state cannot leak between enabled and disabled workspaces sharing a host session.
- Watcher status reads no longer refresh leases, daemon-owned runtime directories are reclaimed,
  and executable lookup no longer depends on an external `which` binary.
- CI and release publishing now share one complete fail-closed source gate.

### Changed

- Intel macOS (`darwin-x64`) is no longer supported. Native release assets are exactly
  `linux-x64`, `linux-arm64` and `darwin-arm64`.
- Bundle builds reuse content-addressed, checksum-verified component outputs. The measured
  same-machine component build improved from 372.92 seconds cold to 16.27 seconds warm (22.9x).
- Bundled Node contains only the executable and license; Caveman installs omit optional packages.
  The measured macOS arm64 bundle is 354204 KiB and its archive is 101056087 bytes.
- Bundle preflight runs independent checks in parallel, CI warms/restores verified component
  caches, and the smoke matrix keeps OS-specific coverage without repeating every full install.

## [0.6.5] - 2026-08-27

### Fixed

- Default stats now include aggregate-compatible typed v1 rows emitted by the current fork, while
  new fork rows carry `privacy_typed_v2`. The exact production fixture that previously collapsed
  435 rows and 44,189 avoided tokens to zero now reports 44,189; its 64 read rows report 27,334
  transform-avoided tokens.
- A healthy grepai watcher is no longer killed because a large initial index has not emitted its
  ready marker within 120 seconds. Startup proves process stability; readiness remains separately
  observable, and the 15-minute idle lifecycle is unchanged.
- Fail-open accounting records when the open gap first began, how long it has remained open, and
  how many session operations were affected. The scorecard and `stats --evasion` expose this
  without adding retry or daemon-supervision behavior.

### Added

- `[instructions] scope = "local"` and `--instruction-scope local` wire HZR through
  `CLAUDE.local.md` and `AGENTS.override.md`. Shared `CLAUDE.md`, `AGENTS.md`, and `.gitignore`
  remain byte-identical; `.git/info/exclude` keeps the local surfaces out of commits. The default
  remains `shared`.
- Exact acceptance gates cover both public GitHub issues, local instruction isolation, and
  first-transition accounting-gap telemetry.

## [0.6.4] - 2026-08-27

0.6.3 made HZR's efficiency measurable. 0.6.4 fixes the part that mattered most and was easiest
to miss: **the surface that reports it.** Upgrading to 0.6.3 moved the accounting-policy version
forward, which silently pushed every previously recorded operation out of the default view. On a
workspace holding 252.65 million avoided tokens, `hzr stats` rendered `0 TOKENS AVOIDED / 0.0%`
and explained nothing.

Nothing was lost and nothing was miscounted. The evidence existed the whole time and the renderer
never mentioned it. That is the class of defect this release closes, in both directions: numbers
that exist and are not shown, and answers that exist and are not carried to the process that
needs them.

### Fixed — a zero now explains itself

- **A scope-narrowed view says so.** When the selected accounting-policy scope excludes recorded
  history, `hzr stats` states the excluded count and prints the exact recovery command
  (`hzr stats --accounting-version all`) inside the panel showing the zero. A `0.0%` produced by
  a scope boundary is labelled a scope artifact, never a measurement.
- **Three zeros are three different claims.** A scope artifact, a scope in which every operation
  earns no savings credit by policy, and an empty ledger are now distinguished and worded
  differently. A clean zero says it is clean.
- **The panels reconcile.** `OPERATION MODES` counted delivery and control-plane stages that the
  reduction ratio deliberately excludes, so a table reporting 202 searches sat under a headline
  claiming 10 operations with nothing connecting them. Each mode row now carries whether it is
  inside the ratio, the excluded totals are reported, and the reason — a delivery stage must not
  double-count the row that measured it — is stated where the numbers disagree.
- **Every recorded row belongs to exactly one counter.** `excluded_legacy_operations` carried a
  stage filter, so a row that was both earlier-policy and delivery-stage appeared in no counter
  at all and the figures could not be reconciled against the ledger. The three counters now
  partition every row in scope, enforced by a property gate.

### Fixed — tables that stay inside their columns

- Table cells are bounded by their own column by construction. The mode panel formatted its
  family cell without truncation, so a 13-character `observability` shifted every column after it
  out of the frame. Columns are now declared once with a width and an alignment, and every cell
  is rendered through that declaration — the alignment gate exists for all five tables and is fed
  the longest value each column can receive.
- Closed vocabularies render as stable short labels instead of mid-word ellipsis.
  `standalone_delive…` told a reader less than `standalone` and looked like a fault.

### Added — money, for the two scopes an operator compares

- **A new `ECONOMICS` block opens the output.** It reports estimated avoided input tokens, their
  preliminary public-list value, and imported provider receipts — for **this project** and for
  **global lifetime**, always both rows.
- Potential and billed are adjacent columns and never a sum. A scope with no receipt reads
  `not measured`, never a currency zero.
- When pricing is not configured, the block states the reason **and** the exact steps that enable
  it, and still reports the token figure, which does not depend on pricing.
- The money rows share the headline's own SQL expression for avoided tokens, so the two can never
  state different totals. Both scopes are read from one snapshot.
- The standalone `POTENTIAL COST` section is subsumed by `ECONOMICS`. No JSON field was removed.

### Fixed — one intent, one verdict

HZR evaluated the same command twice, in two processes, with two different amounts of
information. The `PreToolUse` hook received the harness permission mode and allowed a command;
the `hzr exec run` that the approval had just launched re-derived the verdict without it and
refused. An operator running in `bypassPermissions` was prompted for approvals they had already
granted, and approved commands exited 77.

- **The host's execution grant is now typed and propagated.** `HostExecutionGrant` travels to
  every descendant of an approved command over the same channel that already carries session
  identity, so the hook, `hzr exec run`, `hzr exec rewrite` and the daemon all read one answer.
- **One reconciliation authority.** The logic moved out of the hook and next to the decision type;
  a private copy in one caller is what allowed two callers to disagree.
- **Fail-closed on every axis.** A grant is honoured only for the session it names, within a
  bounded lifetime, from a mode that actually grants execution. A malformed grant is refused, not
  partially trusted. A value copied into a script or left in a stale shell approves nothing.
- **Routing, denial and evidence all survive.** A grant removes the prompt, never the managed
  route. An explicit deny stands. A grant-approved operation is recorded as such and still counts
  as avoidable leakage with zero savings credit.
- **Drift is a check, not a document.** `hzr doctor` fails when a session shows hook-allowed
  operations beside `exec run` approval refusals for the same command.
- An attachment guard matched a bare variable name rather than the export it writes, so a command
  that merely *mentioned* the variable was treated as already carrying it.

### Fixed — session identity reached the ledger

`hzr exec run` and both MCP surfaces resolved the session from the harness-native variables only
and ignored the `HZR_SESSION_ID` the hook exports. Operations started *by* an approved command
therefore recorded no session, so per-session figures read zero for exactly the traffic HZR had
just routed. Session resolution is now one helper that reads the exported value first.

### Added — fail-open is visible while it matters

Bypass was already typed, priced at zero, and reported. What was missing was timing: if the
daemon stopped at minute 20, the operator learned it from `hzr doctor` or the end-of-session card
— after the session whose numbers were now partial.

- The transition from complete to unknown accounting is announced in-session, once, when it
  happens, with the recovery command. Once per transition, not once per command.
- A status-line fragment keeps the state on screen for as long as it is true.
- Recovery is announced once as well.
- A session that spent time degraded says so and withholds partial totals rather than presenting
  them as complete.

This is a visibility change. Daemon fault tolerance is deliberately out of scope: `hzrd` is one
supervised local process, and real fault tolerance would be a different architecture rather than
more supervision patches. 0.6.4 detects the flip and says so; it does not try to prevent it.

### Added — completeness contracts are now executable

- **`MustKeep` and `CompletenessContract`.** "Command families need explicit completeness
  contracts" was documented prose, and prose does not fail. Every filtered route HZR owns now
  declares what it will never drop — exit status, failure lines, warnings, changed files — in a
  compiled-in table, and a route that reaches an agent without an entry fails its gate.
- **The contract is proved by running the filters.** `fork-core/rtk/tests/must_keep_contract.rs`
  feeds the real routes output that carries every undroppable class, plus noise a filter is
  expected to remove, and asserts each class survives.
- This immediately caught two live defects. `rtk summary` returned exit `0` for a child that
  exited `3` — a filter that turns a red run green is worse than no filter, because the summary
  is believed; it now propagates the child's status as `rtk test` and `rtk err` always did. And
  the `write` route promised to keep its exit status and changed-file list but not its failure
  lines, so a refused patch could read as a completed one.

### Added — the re-run tax is measured instead of assumed

- A filtered result can make the model re-issue the command it just ran, and those tokens are a
  real cost of filtering that nothing counted: the second run looked like ordinary traffic.
  `RERUN TAX` now reports repeats of an already-filtered command within a bounded window of the
  same session, de-duplicated, alongside the pessimistic net the headline would show if the cost
  were subtracted.
- The headline keeps its definition. Silently redefining a metric that shipped in 0.6.3 would be
  a worse fault than the one being fixed, and a repeat has causes other than filtering — so the
  figure sits next to the headline and `hzr stats --json` carries
  `net_avoided_after_rerun_tax_estimated` for the pessimistic reading.

### Added — filter placement is a policy dimension

- Delivered bytes and provider-billed input are different axes. A harness that caches the request
  prefix bills a cached read far below a fresh one, and a filter firing mid-turn rewrites content
  the prefix already carries. `[policy] filter_placement` makes *where a filter may fire* explicit:
  `anywhere` (the shipped default, unchanged) or `turn_boundary`.
- Under `turn_boundary` a mid-turn filter is declined and the reduction given up is counted, so
  protecting the cached prefix reads as a trade rather than a free improvement.

### Added — a benchmark that measures what is actually paid

- `benchmarks/hzr-billed-input-prefix-cache-v0.6.4/` runs the same 14 cases with the metric
  changed from delivered bytes to **provider-billed input**, cache-read and cache-write reported
  separately, across both placement arms.
- It refuses to emit a comparison without a paired provider receipt for every case in both arms,
  lists every unmeasured case with its reason, and exits non-zero. A modelled billed-input figure
  is precisely what this benchmark exists to replace, so it will not produce one.
- No numbers are claimed yet. The hypothesis under test is stated and falsifiable: mid-turn
  filtering may raise billed input even while delivered bytes fall sharply.

### Fixed — a wrapped path is still a path

- Managed prose is word-wrapped after interpolation, which keeps rendered width independent of how
  long an install path happens to be. The contract-path parser read one line at a time and needed
  the introducing phrase and the quoted path together, so a long absolute path split across the
  wrap made `hzr doctor` report no contract at all — a presentation layer silently destroying a
  machine-readable reference. It now matches across the wrap.

### Fixed — the session scorecard tells the same truth as `hzr stats`

- A zero on the card explains its cause instead of printing a bare `0 tokens (0.0%)`.
- The `Saved` line carries the priced value inline, with the same rule that potential and billed
  never merge.
- `Measured commands` no longer sits beside a larger hook-event count with nothing accounting for
  the difference; the card itemizes stage-excluded, unmeasured/native, earlier-policy and
  hook-only events, and the five figures partition the total.
- An ask recorded in a session that carries a host grant is reported as a propagation failure
  rather than as ordinary policy activity, because under a grant the expected count is zero.

## [0.6.3] - 2026-08-26

0.6.3 turns HZR's efficiency evidence into a product loop: choose the efficient route, see what
the session saved, understand which command families created the value, and — when explicitly
enabled — translate potentially avoided model-input tokens into a preliminary public-list price
estimate. The same release closes the operational gaps that could make healthy-looking state
untrustworthy across a fleet.

This is the qualitative change: HZR no longer asks an agent to follow an efficiency convention
and leaves the operator to infer the result. The control plane enforces the route, attributes the
executed work, exposes the benefit, and diagnoses drift against the same workspace identity.

### Added — visible session economics

- **Session ROI now has an optional money lens.** HZR prices estimated potentially avoided
  model-input tokens against an exact harness/provider/model/method entry. The result is labelled
  preliminary and potential, keeps its currency and catalog identity, and never masquerades as a
  provider bill.
- **A versioned public catalog ships with the product.** It records dated first-party price
  sources for popular OpenAI, Anthropic, Google, DeepSeek, Qwen, Mistral, and xAI API models,
  including method-specific and expiry boundaries where the provider requires them.
- **Pricing is operator-owned.** Estimates are disabled by default. `hzr billing catalog` shows
  the evidence, and an absolute JSON `pricing_file` merges strict overrides by exact pricing
  key. Unknown, ambiguous, unsupported, expired, or mixed-currency selections fail closed;
  HZR performs no FX conversion and never borrows a nearby price.
- **Receipts and estimates remain different evidence.** A CLI-imported receipt is user-supplied,
  not verified provider evidence; only a trusted adapter may upgrade provenance. Public-list
  arithmetic is never promoted to invoice truth. Replay is idempotent, and attribution stays
  bound to the canonical project/session.
- **The value story stays privacy-safe.** End-of-session output combines estimated net token
  reduction, measured commands, top command families, prevented bypasses, and avoidable leakage
  without revealing paths, queries, arguments, or raw session identifiers.

### Changed — the efficient path is the rewarded path

- An effective managed replacement still makes RAW execution forbidden, but 0.6.3 connects that
  policy to the session story: bypasses receive zero efficiency credit, named replacements are
  actionable, and a clean zero means **no proven avoidable leakage**, not “HZR saved nothing”.
- Wrapped shell forms remain subject to the same route decision. An agent cannot recover credit
  by hiding a managed command behind a shell, environment wrapper, or neighbouring instruction.
- Explicit fidelity remains available for real exactness requirements, with one closed reason,
  a budget, and an auditable receipt. It is a controlled capability, not an escape hatch.

### Fixed — doctor now tests reality

- **Semantic readiness is executable evidence.** Doctor performs a typed runtime probe instead
  of declaring grepai healthy merely because index files exist. Complete fork runtime
  configuration is accepted, incompatible schema is surfaced, and stale state cannot present a
  dead semantic route as ready.
- **Global install recovery is global only after proof.** A completed journal is accepted from
  another workspace only after its schema, exact terminal stage receipts, and state validate.
  Incomplete recovery remains tied to the workspace that owns it.
- **Waivers mean the same thing locally and across the fleet.** Current-workspace doctor honours
  the same narrow, auditable RTK instruction waiver as fleet inspection; malformed or broader
  exemptions remain failures.
- **ICM and index ownership remain centralized.** Health, supervision, canonical worktree
  identity, and cache/index readiness are evaluated as one control-plane state instead of
  allowing an orphan process or old files to stand in for a live engine.

### Added — fleet reconciliation that completes the job

- `hzr doctor --reconcile-fleet` now creates missing managed Claude/Codex instruction surfaces
  and Codex project MCP pins for registered workspaces in addition to refreshing existing blocks.
  Claude Code `.mcp.json` remains an explicit project registration.
- Fleet writes require canonical registered-root identity and reject escaping symlink paths.
  User-authored text outside managed blocks is preserved and reported.
- `--migrate-legacy-indexes` transactionally handles an unambiguous root `.grepai` while nested
  or ambiguous duplicates remain explicit operator decisions.
- `hzr migrate archive-index --dry-run` previews the exact nested source, content hash, backup,
  and manifest; `--force` applies it. Active writers are refused, backups are retained, and a
  source recreated at the same path produces an explicit conflict without mutation instead of
  being hidden by an old idempotency record.

### Fixed — truthful client and codec coverage

- Codex and Claude Code prefer project-scoped MCP pins. Linked worktrees use their exact project
  scope without rewriting shared local state.
- Claude Desktop's one global selected workspace is modelled honestly: every other project is
  `unavailable_for_this_workspace` with a retarget-or-CLI action, never a false green fleet pin.
- Generated MCP snippets serialize paths safely rather than interpolating executable or
  workspace strings into TOML/JSON.
- Codec coverage is derived from audited activation surfaces and reports `applied`,
  `shadow_measured`, `instructed`, or `unavailable`. Only a payload HZR actually transforms can
  receive estimated codec credit. Codex and Claude cannot currently expose trusted global final
  response replacement, so that path stays visible with zero economic credit.

## [0.6.2] - 2026-08-26

0.6.2 turns session accounting into visible product value. HZR already had the data to prove that
an agent used smaller outputs and to identify bypass pressure, but its Stop scorecard led with a
misleading zero: `recoverable-tokens=0`. That number meant only "no proven avoidable bypass was
allowed to execute". It did **not** mean that HZR saved zero tokens or prevented nothing.

The result was technically defensible and experientially wrong. An effective control plane
should make its benefit obvious at the moment the work finishes.

### Added — session ROI

- **The `hzr stats` efficiency model is now session-scoped.** Each session receives the same
  baseline, delivered, gross avoided, regression, and net avoided token arithmetic as the global
  report, filtered by HZR's keyed private session identity and current accounting policy.
- **The Stop card leads with measured value.** It reports estimated net tokens saved, reduction
  percentage, baseline-to-delivered movement, measured command count, and the three most-used
  privacy-safe command families.
- **Session ROI is a clean future billing seam.** Model-priced provider receipts can later join
  the same private session scope and add dollar savings without relabelling local token estimates
  as billed usage.

### Fixed — zero no longer hides success

- `ops` is no longer presented as though it were a savings denominator. It was the hook event
  count; the card now names it `hook events` and reports measured ledger executions separately.
- `recoverable-tokens=0` is gone. Proven avoidable output that actually escaped the efficient
  route is named **avoidable leakage**. Zero is explained as the desired outcome: no proven
  avoidable bypass executed.
- Prevented commands are shown as policy wins, with native denials and approval asks separated.
  Their unexecuted output is explicitly `not estimated`; HZR does not manufacture counterfactual
  savings to make the card look stronger.
- If the ledger cannot be opened or queried, savings and leakage now read `unknown`, not zero.
  Missing evidence can no longer masquerade as a clean session.
- `E10` is rendered as `e10-capability-gap`, so the dominant class is understandable without
  memorising the evasion taxonomy.
- **Instruction drift can no longer wait silently for a manual doctor run.** SessionStart audits
  every Claude/Codex instruction surface owned by the active all-project or project-only policy
  with the same canonical audit used by doctor. If it repaired a managed block or still finds an
  unhealthy surface, it tells the agent to run `hzr doctor` before continuing and to preview
  fleet repair with `hzr doctor --reconcile-fleet --dry-run`.

### Acceptance gate

- The session slice is compared against global `hzr stats` on a one-session ledger, preventing
  the two token formulas from silently diverging.
- Regression cases cover non-zero avoidable leakage, zero leakage with prevented work, missing
  ledger evidence, session attribution, privacy-safe top-command output, and a SessionStart drift
  alert that remains intact when an update notice is emitted at the same time.

## [0.6.1] - 2026-08-26

0.6.1 is the verification hotfix for 0.6.0. The deferred acceptance runs from the 0.6.0 release
were executed against the immutable 0.6.0 source, and every gate that
failed is fixed here. Nothing was reclassified as passing: the defects below were found by
running the gates, not by lowering them.

### Fixed

- **The declared MSRV did not build.** `hzr-daemon` and `hzr-cli` used `let` chains, which Rust
  stabilized in 1.88, while the workspace advertises `rust-version = 1.85` and the README
  promises "Rust 1.85+". Twenty conditions are now nested plainly, and `cargo +1.85.0 check
  --locked --workspace --all-targets --all-features` passes.
- **MCP reads and writes reached no ledger at all.** `hzr_read` and `hzr_write` were recorded
  with `accounting_stage = final_delivery`, a stage the efficiency summary excludes so that the
  last hop of a multi-stage pipeline is not counted twice. An MCP tool call has no earlier
  stage, so every MCP read and write was silently absent from `hzr stats`. They are recorded as
  standalone deliveries now, like the memory and context tools beside them.
- **fork-core could replace `git status --porcelain` with a summary.** The never-worse guard
  trimmed its input before testing for porcelain, which removed the two leading status columns
  that define the format, so the protocol was never recognized and the exact bytes were not
  preserved.
- **The same guard neutered every explicitly requested digest.** Any valid JSON counted as an
  exact machine protocol, so `rtk json` and the `rtk read` CSV/JSON digests always fell back to
  raw — a schema view that never rendered. An explicitly requested summary now uses its own
  guard, which still refuses to lose content, a failure signal, or size.
- **MCP ownership was not workspace-aware.** HZR audited only Claude Code's user-global
  `mcpServers` and ignored the per-project `projects[<path>].mcpServers` scope that Claude Code
  actually launches, so one global pin left every other project permanently mismatched. The
  project scope now takes precedence, mirroring the existing Codex behaviour.
- **The capability contract described the wrong surface.** `harnesses.managed_agent.tool_names`
  carried the 13-tool MCP inventory instead of the 11 tools the Caveman bridge implements, so
  the managed runtime refused to start against its own contract.
- **Doctor's fidelity remediation lost its safety wording.** An unknown execution may already
  have been billed; the operator guidance says so again, and names idempotent replay.
- **The integration suite was flaky under full parallel load.** ICM liveness gates waited two
  seconds for a process spawn, which is not enough on a machine running the whole suite.
- **System posture read `Rebuilding` on a healthy idle daemon.** `dashboard_overall_state`
  scored every registered workspace, so any project that had never been indexed counted as a
  rebuild in progress — 75 of 82 here, which pinned the chip to `Rebuilding` permanently. The
  posture now describes this control plane and the project in view; fleet progress keeps its own
  reading in `projects_index_ready / projects_total`, and an unselected daemon reports
  `standby` labelled "No project selected".

### Changed

- **Fleet exemptions are declared, not inferred.** A hardcoded path heuristic silently exempted
  one directory from fleet instruction policy. A project now declares a waiver in
  `.hzr/policy.toml` with the rule it covers and an auditable justification, and `hzr doctor`
  reports it under `fleet_instruction_exemptions` instead of passing it in silence. Only
  instruction-directive rules are waivable; no file on disk can waive an execution route.

### Changed

- **Bundle provenance stops restating digests it can compute.** Thirteen artifacts HZR ships
  out of its own tree were verified against transcribed SHA-256 constants, so every legitimate
  edit to a contract, patch or licence broke the release build at the last step with a magic
  number to hand-copy. The bundle copy is now compared directly against the reviewed
  repository source, which states the same property — "the bundle carries exactly what was
  reviewed" — and cannot go stale. Digests stay pinned only where the file comes from outside
  this repository: the vendored extract-zip snapshot and the grepai and ICM upstream licences,
  where the pin is the supply-chain control rather than a restated diff.
- The Caveman bridge manifest restates the product version, and the managed runtime refuses to
  start when it drifts. A one-second test now asserts it against `Cargo.toml`, instead of the
  drift surfacing three minutes into a bundle build.

### Added

- **`hzr doctor --reconcile-fleet` repairs the drift doctor reports.** Doctor could name 158
  stale managed contract blocks but not fix them, so the only remedy was opening 79 workspaces
  by hand — which is how a fleet drifts in the first place. One command now refreshes every
  registered workspace that reports a stale block, and `--dry-run` lists the exact files first.
  It never adopts a workspace that has no managed block, never rewrites a user-authored
  directive, and names each file where such a directive survives so a refresh cannot read as
  "this workspace is clean". It also refuses to run from a source checkout: a contract path
  inside a developer's tree is unreadable from every other project, and writing it across the
  fleet would replace real drift with worse drift.
- `hzr init --dry-run --if-needed` is now a legal combination, and the plan carries a top-level
  `changes_required`. Reconciling a fleet no longer means writing to a repository to discover
  whether it needed writing.

### Documentation

- The README carries the control-plane banner again, and its architecture diagram is redrawn
  with HZRD at the centre as the control plane rather than as one box among the engines.

## [0.6.0] - 2026-08-26

### Why this is a qualitative leap

0.5.x made individual routes safer and cheaper. 0.6.0 changes the class of the product: HZR is
no longer a collection of adjacent wrappers around RTK, grepai, ICM, and Caveman. It is one
owned control plane with a shared project identity, lifecycle, policy boundary, typed protocol,
accounting model, diagnostic surface, and local observatory.

Before 0.6.0, a filter could be correct while a neighbouring path still bypassed it; a watcher
could die while the UI looked idle; a cache could outlive its project; an uncertain execution
could be retried; and an estimated reduction could look like billed savings. The components were
useful, but correctness still depended on them behaving independently in compatible ways.

0.6.0 closes that loop:

```text
agent intent → HZR policy → managed engines → typed validation → ledger → doctor / traces / UI
```

#### Closing the agent incentive gap

The previous gap was behavioural as much as technical: even when an efficient command existed,
an LLM could still prefer a familiar native tool, RAW wrapper, shell indirection, or generic
fallback. That shortcut was easier to request, difficult to distinguish from legitimate exact
work, and could disappear from the denominator while making the remaining savings ratio look
better.

0.6.0 changes those incentives instead of relying on prompt obedience. `hzr exec run` is the one
easy route and selects the managed implementation itself. Policy recursively inspects common
shell/environment wrappers; `allow_rewrite` makes RAW forbidden; direct RAW/proxy entry points are
rejected; and legitimate byte-exact recovery requires a closed reason plus a durable bounded
allowance. A bypass that is still observed receives equal baseline and delivered cost, zero
savings credit, a typed E10 classification, and a concrete first-class replacement in stats or
the agent scorecard.

The effect is deliberate: following HZR requires less agent judgment than bypassing it, while a
real unsupported command remains usable through a tracked no-equivalent fallback. HZR therefore
discourages avoidable bypass without pretending every command already has an effective filter.

That architectural shift gives users materially stronger guarantees:

- **One owner instead of competing processes.** A worktree has one grepai index owner and HZR
  supervises one durable ICM process; lifecycle failures remain visible and recoverable.
- **No invisible escape hatch.** A replacement-capable command that bypasses managed filtering is
  a policy failure. Explicit exact-output recovery is bounded, durable, and receives zero savings
  credit.
- **Truth before headline savings.** Provider receipts, byte-based estimates, excluded traffic,
  native unobserved traffic, regressions, and unknown replacement capability remain distinct.
- **Failures become state, not folklore.** Post-spawn uncertainty survives restart, interrupted
  adoption has a forward-recovery journal, and doctor reports concrete desired-state gaps.
- **The system explains itself.** The same typed events drive MCP receipts, lifecycle diagnostics,
  traces, memory/index topology, accounting posture, and the local UI.
- **New capabilities compose instead of creating another side channel.** CLI and MCP share the
  daemon-owned schema, confinement, validation, and accounting boundary.

The result is not merely stronger compression. It is a safer and more operable mechanism for
earning token reduction without hiding incomplete output, duplicate work, cross-project state,
or missing economic evidence.

### Added

- A typed local observability plane now connects daemon lifecycle, RTK routing, grepai watcher
  health, ICM supervision, MCP activity, memory topology, latency, and accounting exclusions to
  one project-scoped dashboard and trace API.
- The MCP surface grows to 13 schema-validated tools, including confined daemon-owned read/write,
  managed execution, observability, and doctor. One canonical contract now drives discovery,
  dispatch, validation, and accounting.
- Fidelity operations now use a crash-safe durable state machine with pre-execution reservation,
  explicit executing/unknown state, idempotent ledger replay, quarantine, bounded allowances, and
  operator-visible remediation.
- Project-local Codex MCP registration, transactional init/install journals, desired-state doctor
  checks, and forward recovery make adoption reproducible across multiple workspaces.

### Changed

- Command bypass is treated as a policy failure instead of an invisible fast path. Managed
  equivalents are enforced, RAW receives zero savings credit, and incomplete/native traffic is
  first-class in statistics.
- grepai and ICM now run under one bounded supervisor with singleton ownership, restart/backoff,
  TTL/LRU cache limits, failed-state tombstones, and project-safe health reporting.
- RTK fork configuration is typed and fingerprinted; consumers invalidate stale configuration
  without duplicate loads. Structured-output fidelity now falls back to raw when completeness
  cannot be established.
- Public telemetry uses stable keyed pseudonyms and project aliases rather than commands, paths,
  queries, session identifiers, or memory content.
- The README and documentation set were rebuilt around the unified architecture; superseded
  records are clearly archived and stale false-ready reports/screenshots were removed.

### Fixed

- Cross-project memory detail and UI request races can no longer leak stale A/B/C project state.
- Failed grepai watchers remain observable until bounded expiry or explicit recovery instead of
  being mistaken for standby, and status polling cannot keep the cache alive forever.
- Post-spawn execution failures no longer refund fidelity allowance or invite unsafe retry; the
  uncertain operation survives restart for reconciliation.
- Sequential multi-workspace installation no longer keeps the first workspace as the global owner.
- MCP rejects outside-workspace reads/writes and refuses success when its typed output contract or
  accounting receipt is incomplete.
- `hzr update` could not find its own installer when invoked through `~/.local/bin/hzr` — the
  symlink the installer itself puts on PATH. `current_exe()` can return that symlink rather than
  its target, so the bundle root derived from it was `~/.local/share/hzr`, which holds no
  installer; the only remaining candidate was the source tree at the compiled-in path, which
  exists on a build machine and nowhere else. Every install built by CI therefore failed to
  self-update, with `hzr update` reporting the installer as absent. The resolved target is now
  tried as well.

### Evidence boundary

The four native release bundles passed clean-install verification, including the daemon, managed
engines, memory readiness, project search, stats, MCP, and atomic current-version switching. This
is still not a provider-billed proof of economic savings: `economic_claim_ready` remains `false`,
and the deferred full workspace, browser, fleet, and fault-injection gates were not presented as
completed.

## [0.5.1] - 2026-08-23

Post-release audit of 0.5.0 for correctness, races and efficiency headroom.

### Fixed

- `sudo` was still rewritten when it was not the first word. The prefix regex folds
  `sudo`, `env` and `VAR=value` into one run, so `SUDO_ASKPASS=... sudo docker ps` and
  `env sudo docker ps` reached the rewriter with the elevation already stripped into the
  prefix — re-creating the root-owned-state problem 0.5.0 closed. The guard now skips the
  assignment/`env` run the way the shell would and stops at the first real command word,
  so an elevator name appearing only as an argument still rewrites.
- `playwright` no longer strips `-r` and its value. Playwright has no `-r` alias for
  `--reporter` (that is mocha's `--require`), so an unrelated short flag was being removed
  from the invocation along with its argument.

### Changed

- A bounded read stops at the bound. `hzr read <file> --max-lines N` loaded the whole file
  to keep N lines and then read it again to count newlines for the "N of M" notice. The read
  now stops where the bound is, the file total streams through a fixed buffer, and the filter
  path borrows the content instead of cloning it twice. Output is byte-identical; on a 20 MB
  log, `--max-lines 20` drops from 51.7 MB to 10.5 MB peak and no longer scales with file
  size, and a filtered whole-file read drops from 199 MB to 137 MB.
- `jsonpack` collects columns in one pass with a name index. It previously scanned the column
  vector linearly for every leaf of every row and then re-walked every path per column to
  decide nullability — quadratic in the payload on a wide `gh api` page.

## [0.5.0] - 2026-08-22

Selective adoption of upstream `rtk-ai/rtk` work published after the `v0.44.1` import base
(59 commits to `develop` @ `f8d636d`, plus reviewed unmerged proposals). The trees diverged
structurally, so every item is a re-implementation against `fork-core`, not a merge. Analysis and
the per-item decision record live in `docs/PRD_HZR_UPSTREAM_RTK_SYNC_0_4_7.md`.

### Security

- HZR is no longer inserted into a privilege elevation. `sudo docker ps` was rewritten to
  `sudo rtk docker ps`, so an elevated engine wrote the history DB, its SQLite siblings, tee files
  and the audit log as root inside a user-owned data root. `sudo`, `doas` and `pkexec` prefixes —
  bare or via an absolute path — now stop the rewrite; the command runs verbatim and is accounted
  under the new `e11_privileged_prefix` class, marked not avoidable so it cannot consume the
  avoidable-bypass budget.
- Data files are created owner-only instead of being chmod-ed after the write, which left a
  window where content was readable under a permissive umask. Covers the engine history DB and
  its `-wal`/`-shm` siblings, tee output, the telemetry salt and the memory cache, plus the HZR
  ledger database and the daemon runtime directory.
- A lone `\r` is no longer a command separator. Bash runs `git status\rrm -rf ~` as a single
  mangled git command; treating the `\r` as a boundary over-segmented the permission split
  relative to the command that actually executed.

### Fixed

- A single non-UTF-8 byte no longer truncates the rest of a command's output. The reader chained
  `BufRead::lines()` with `.map_while(Result::ok)`, which stops the iterator at the first `Err`,
  so one stray byte discarded every remaining line of that stream — and the loss was recorded as
  a saving. Lines are now split on the raw byte and decoded lossily.
- `git log` gained a real argument model. Requests for a raw diff shape (`-p`, `--patch`,
  `--stat`, `--numstat`, `--shortstat`, `--raw`) pass through untouched; a closed list of
  value-taking options keeps `--grep --pretty` from reading the search term as a format flag;
  `restore_double_dash` puts back the `--` pathspec separator clap consumes, for `log` and
  `commit`; and `-U`/`--unified`/`--expand-tabs`/`--max-parents`/`--min-parents` take attached
  values only.
- A multi-line `[[ … ]]` conditional is no longer split in half by the multiline rewriter. The
  four hand-rolled quote-state walkers behind the independence checks are now one shared
  `QuoteScan`.
- Filters no longer report success on a failing child. The never-worse guard is extended from
  output size to exit status across `cargo test`/`nextest`, `pytest`, `ruff`, `mypy`, `tsc`,
  `vitest`/`jest`, `go vet`, `go test -json`, `golangci-lint` and the lint fallback.
- `prettier --check` reads its file list from stderr, where prettier has written it since 1.19,
  instead of discarding every `[warn]` line and reporting "All files formatted correctly" beside
  exit 1.
- Path-based search no longer buffers the whole result set twice; capture goes through the
  streaming path that caps retention at 10 MiB and reports truncation. The same fix applies to
  `grep_cmd`, which has no upstream counterpart.
- `[filters].ignore_dirs` and `ignore_files` reach the system commands. They had no readers
  anywhere in the engine.
- `find` compares its guard against the plain listing at the same cap rather than the full match
  list, adds a tee tail hint when it truncates on its own initiative, and passes through
  invocations carrying flags it cannot filter (`-not`, `-exec`) instead of failing.
- `du -s` and `du -d N` keep every row. Each is an independent total, so the 40-line cap dropped
  the largest consumers while reporting a confident answer.
- `turbo`, `quarto-render`, `bundle-install`, `gcc`, `spring-boot`, `liquibase` and `ssh`
  declarative filters no longer match commands they should never touch or discard their own
  pass/fail tally.
- `rtk lint <path>` no longer reads a bare path as a linter name and reports a verdict for a run
  that never happened.
- `playwright --reporter list` no longer leaves `list` behind as a positional test filter.
- `prisma migrate status` counts migrations instead of occurrences of the word "applied" in
  prose, and falls back to the raw text rather than asserting "0 applied, 0 pending".
- `curl` uses `-sS`, so a failure keeps curl's own error message. `git commit` announces its own
  failure instead of ending on a pre-commit hook's `...Passed` line, and `git diff` works with an
  external diff driver configured.
- `ls` reports how many noise directories it omitted, and `head`/`tail` honour
  `exclude_commands`.
- Signal deaths keep their stderr diagnostic when output is captured; both capture helpers route
  through one function so a `128 + signal` code is never reported without saying why.

### Added

- `jsonpack`: lossless re-encoding of JSON that removes the field names repeated on every row and
  nothing else. A top-level array of objects becomes a declaration line plus CSV rows; an
  envelope object stays valid JSON with dense inner arrays rendered as `{"_cols":…,"_rows":…}`.
  `pack()` decodes its own output and requires exact equality with the parsed input before
  emitting a byte, so an encoder bug or adversarial data returns the raw bytes unchanged.
- `gh api` uses it in place of a preview that truncated strings past 200 characters and arrays to
  five items. `gh <cmd> --json` stays exact passthrough unless `RTK_GH_PACK_JSON=1` or
  `[gh] pack_json = true` is set; `--jq` and `--template` are never touched.
- `pass_through_if_args` in the declarative TOML filter engine, so any filter can opt out of
  filtering for specific invocation shapes.
- `RTK_QUIET=1` / `[hooks] quiet = true` suppresses informational stderr about commands that
  succeeded. Real errors and truncation warnings are never suppressed.
- `npm`/`npx` stream filtered output instead of retaining it until the child exits.
- Tee recovery-hint filenames collapse a long slug to a short prefix plus hash.

### Changed

- The `hzr stats` evasion panel lists every class that occurred instead of the top ten. The
  taxonomy has eleven entries as of this release, and a capped list reads as a complete one.
- **Measured savings move in both directions.** Passing `git log --stat` through unchanged is the
  correct answer and produces more tokens than mangling it did, so those invocations get worse on
  paper. `gh api` improves while also no longer discarding content. A ledger delta across this
  boundary is a change of behaviour, not a regression.

## [0.4.6] - 2026-08-21

### Fixed

- HZR no longer prompts for commands the host has already authorized. It derives its own verdict
  from the settings file, so an operator running Claude Code in `bypassPermissions` with no
  `permissions` block was asked to approve every rewritten command — a prompt answering a
  question they had already answered. The hook now reads the reported permission mode; an
  explicit `deny` rule still stands, and every decision is still recorded.
- Environment carried to the recording process is now exported rather than assigned. The managed
  command is a script whose first line is already a run of assignments, so a bare `VAR=value`
  prefix became one more assignment in that run and never crossed the process boundary — the
  script's own `export` statement lists only the RTK variables.
- Per-session evasion accounting reported zero however much a session actually bypassed. Two
  independent breaks produced one symptom, and both are closed here.
  - Hook-approved commands now carry `HZR_SESSION_ID` into the process that records them. The
    hook receives the session on stdin, but the engine it approves runs as a fresh process that
    could only learn the session from its environment, so executed operations landed with a null
    session while policy events carried one.
  - `/v1/exec/rewrite` now returns the classification alongside the decision, and the hook
    forwards it as `HZR_INTERNAL_EVASION_JSON` for every approved command rather than only on the
    fidelity path. The daemon knew the evasion class, the host ran the command and the engine
    wrote the row, so the `avoidable` column was never set for ordinary traffic.
- Attribution failures no longer change a decision. For a fidelity request the attribution stays
  a precondition and a failure remains an Ask; for every other command it is accounting, and
  accounting must not turn a working command into a prompt. The agent-facing source and reason of
  an approved decision now survive attribution instead of being replaced by the fidelity wording.

## [0.4.5] - 2026-08-21

### Added

- Every Ask and Deny names its evasion class, the construct that triggered it, and the route to
  use instead. `EvasionClass` carries `construct()` and `prescription()` as the single authority
  for that wording, and an approved Ask states the managed form approval will run.
- The acceptance matrix asserts what a decision tells the agent, not only its verdict. Ask and
  Deny cases require `expect_reason_contains`, the loader rejects an empty expectation, and a
  gate ties every expectation to a prescription the taxonomy actually emits.

### Changed

- Ask and Deny decisions report the running session avoidable-bypass count alongside corrections.
  Only corrections increment it: an E10 Ask is a capability gap and must not consume the budget.

### Fixed

- The Stop scorecard's shadow budget reported a constant `tokens=0/250000`: it read a
  `SessionFeedback` field that no code path ever wrote. Both halves now come from the ledger's
  per-session avoidable operations and tokens, so the shadow window can calibrate the threshold
  it exists to calibrate, and the label states that T3 is measured rather than enforced.
- The installed-bundle smoke check asserted that the dashboard reports a literal workspace path.
  Project identity became a keyed pseudonym in 0.4.4, so the check asserted the absence of the
  privacy guarantee it should have been proving; it now requires a registered project whose root
  is redacted. This failed both the `assembled-bundle` CI job and every release bundle job.
- The `ps` route tests pinned a permission verdict that depends on the caller's home directory,
  so they passed on a developer host that allows `ps` and failed on a clean runner. They now run
  against a temporary home and assert the default verdict's documented rewrite and exit code.
- `hzr init` no longer fails when the pinned index engine is not installed.
 Warming the index is
  optional; the workspace registration is not. A source checkout, a partially installed host and
  the `rust` CI job all lack the engine, and the hard failure took the whole command — and the
  SessionStart hook — down with it. A genuinely broken engine still surfaces.
- `hzr stats` bypass rows merge on the privacy-safe label they display instead of the ledger tool
  identity, so `rg`, `grep`, `rgai` and `search` no longer appear as several identically named
  rows with their traffic split.
- A bypassed HZR subsystem is no longer reported as having no first-class route. The per-row flag
  records whether a replacement could be reconstructed from one recorded invocation, which is a
  different question from whether the family has a route; `read`, `search`, `write`, `memory` and
  `codec` have one by construction.

## [0.4.4] - 2026-08-21

### Added

- A canonical typed anti-evasion plan normalizes POSIX shell launchers, environment and utility
  prefixes, quoted ranges, absolute executable paths and bounded pipelines before policy. A
  committed 85-case E1–E10 matrix gates the fork, hook, daemon, degraded and native-tool paths.
- Privacy-safe policy events record Ask, Deny and Correction without inflating command counts,
  delivered tokens or savings. `hzr stats --evasion` reports bounded class/session aggregates,
  producer and accounting-policy versions, and a separate fidelity allowance.
- Compact first-class routes cover grouped `git blame`, ordered budgeted batch reads, SELECT-only
  SQLite, tar listings and bounded remote Docker logs. `hzr test` forwards exact test argv.
- Native file-tool policy exposes `observe`, `steer` and `strict`; new installations use `steer`,
  while legacy upgrades retain `observe` and receive a doctor warning.

### Changed

- RAW fidelity requires a compatible closed reason and is limited to five operations or 100,000
  estimated tokens per session. Missing, unknown, contradictory or provably replaceable requests
  Ask before execution and never echo the supplied value.
- `hzr init` enables repository-graph indexing and installs managed project-local `CLAUDE.md` and
  `AGENTS.md` regions while preserving user-authored text. Doctor reports contract drift, native
  mode and fidelity allowance.
- Telemetry persists only closed dimensions and keyed pseudonyms. Commands, paths, queries, SQL,
  heredocs, environment values, raw agent IDs and session IDs are scrubbed before persistence;
  unbounded `stats --json --all` is refused.
- Read accounting now separates source selection from delivery transforms, and current-policy
  reporting excludes unattributed legacy rows instead of mixing incompatible accounting models.

### Fixed

- Quoted `sed`/`nl`, `/bin/sh -c`, nested shell wrappers, safe prefixes and known interpreter read
  idioms can no longer fall through to silent proxy merely because their spelling changed.
- Temp-log laundering, private HZR-ledger reads and malformed/deep wrappers receive typed policy
  responses; genuine computation without an equivalent remains unpunished E10.
- The macOS `ps aux` formatter now parses logical fields rather than header byte offsets, keeping
  complete PIDs and command text.
- Generic `hzr test` and `rtk test|err` routes now execute the supplied argv without shell
  reconstruction and return the child's exact nonzero exit code instead of masking failures.
- Policy attribution reaches the existing fork tracking row through a validated, consumed
  internal value, avoiding both payload leakage and duplicate operation accounting.

## [0.4.3] - 2026-08-21

### Added

- `hzr stats --since <N{h|d|w}>` applies one inclusive time window to provider,
  efficiency, bypass, family, and mode summaries. Human and JSON output now include bounded,
  privacy-safe operation-family and stage-aware mode aggregates.
- Typed requested/effective search modes, actual backend strategy, closed fallback code, and
  accounting stage distinguish internal transport from final delivery without storing queries,
  paths, command arguments, file contents, or arbitrary error text.
- Top-level `hzr read` and `hzr write` aliases expose the existing typed fork-core operations
  without requiring agents to spell the compatibility route.
- A final High Critical command-utilization audit and acceptance matrix cover redundant HZR
  wrappers, typed npm/pnpm test aliases, instruction drift, accounting de-duplication, and
  privacy-safe diagnostics.

### Changed

- Redundant managed RAW/proxy wrappers around an already first-class `hzr` command are removed
  byte-for-byte. Explicit `HZR_RAW_FIDELITY=1` remains a byte-fidelity escape when no equivalent
  bounded route can preserve the requested output.
- `npm test`, `npm run test`, and `pnpm test` use the existing typed command families. Unknown
  npm execution forms remain conservative proxy operations unless a specialized rule matches.
- Managed Claude and Codex instructions prefer top-level read/write aliases, list every shipped
  MCP tool, expose bounded generic output filters, and describe native tool observation as
  measurement-only rather than redirection or savings.

### Fixed

- Direct and MCP search accounting now records the backend actually used instead of inferring it
  from the requested mode, and final-delivery rows no longer double-count headline totals.
- npm run aliases no longer produce a duplicated `run` argument.
- The inherited fork-core Clippy ratchet retains the reviewed 141-warning set after the typed
  accounting and command-routing line shifts.

## [0.4.2] - 2026-08-21

### Added

- Typed, non-sensitive read/search attribution records operation mode, accounting stage, bounds,
  filter level, result limit, include-content choice, and path-scope count. Stats exposes mode
  aggregates while legacy rows remain unattributed.
- Acceptance gates cover bounded managed reads, explicit whole-file fidelity, removal of forced
  full-contract bootstrap, stale instruction detection/repair, legacy RTK block retirement, and
  stable GitHub Latest publication.

### Changed

- Managed Claude and Codex instructions use the embedded ordinary-work contract and direct
  HZR-policy investigations to an outline followed by relevant ranges instead of importing or
  reading the complete `HZR.md` on every task.
- Unbounded managed `read --level none` uses the smart format-aware view. Ranges, numbered reads,
  structural modes, changed hunks, and `HZR_EXACT_FIDELITY=1` preserve exact semantics.
- Search guidance uses semantic/auto discovery for unknown implementations and reserves exact
  mode for known literals, symbols, errors, keys, and audits.
- `hzr init --if-needed` refreshes the active managed instruction scope and migrates detected
  delimited legacy RTK blocks without changing clean project files.

### Fixed

- `hzr doctor` now rejects a stale managed region or a conflicting local RTK/ICM directive rather
  than treating marker presence as current policy.
- GitHub tag releases are published as stable and explicitly marked Latest; `v0.4.1` metadata was
  corrected without replacing its tag or assets.

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
[0.4.3]: https://github.com/heAdz0r/hzr/compare/v0.4.2...v0.4.3
[0.6.3]: https://github.com/heAdz0r/hzr/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/heAdz0r/hzr/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/heAdz0r/hzr/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/heAdz0r/hzr/compare/v0.5.1...v0.6.0
[0.8.0]: https://github.com/heAdz0r/hzr/compare/v0.7.1...v0.8.0
[0.7.1]: https://github.com/heAdz0r/hzr/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/heAdz0r/hzr/compare/v0.6.6...v0.7.0
[0.6.6]: https://github.com/heAdz0r/hzr/compare/v0.6.5...v0.6.6
