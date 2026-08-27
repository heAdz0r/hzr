# PRD — HZR 0.6.6 orchestrator correctness and single sources of truth

Status: **approved and implemented; release verification pending**  
Target: **0.6.6**  
Release rule: every requirement and acceptance gate in this document is release-blocking.

## 0. Why 0.6.6 exists

HZR 0.6.5 shipped the requested local instruction scope and live accounting visibility, but a
read-only adversarial audit found that several behaviors still have more than one executable
definition. Those copies already produce user-visible contradictions:

- `hzr stats` groups by private command text and only afterwards maps multiple rows to the same
  privacy-safe label, so the table and JSON contain apparent duplicates;
- fork-core can fail to write an executed operation while HZR continues to report accounting as
  complete;
- E11 exists in the HZR protocol and rewrite registry but is rejected by the fork ledger's older
  private whitelist;
- session identity, instruction scope, accounting-gap state, hook context and version identity are
  each recomputed in multiple places with different rules;
- a shared-to-local or local-to-shared instruction transition can leave the old managed block in
  force, while doctor checks only the newly desired paths;
- the release workflow can publish a tag without running the same complete Rust gate as CI;
- the separately approved bundle-build workstream must remove `darwin-x64` as a supported target,
  make expensive component builds safely reusable, slim the bundled Node/Caveman runtime and
  shorten release latency without weakening platform-specific proof.

This release is therefore not a feature accumulation release. It makes accounting, orchestration
and lifecycle state structurally single-owned, removes confirmed drift, and adds gates that fail
when a second source of truth is introduced.

### 0.1 Audit boundary

The audit traced current code and, for the stats defect, current live ledger output. It did not
modify product code or run mutating failure injection. Confirmed findings are requirements below;
one explicitly conditional cleanup item is marked as such.

The worktree already contained unrelated user changes in `install.sh`,
`scripts/install-bundle.sh`, and `scripts/smoke-install.sh`. They are outside this PRD write and
must be preserved during implementation.

### 0.2 Coordinated bundle-build workstream

The active task “Ускорить сборку бандлов” owns the first implementation of its approved P0-P1
changes in the same shared worktree. Its final handoff, including changed files and green gates, is
a required 0.6.6 input. Until that handoff arrives, this task must not edit or commit bundle/release
surfaces, bump/tag/push 0.6.6, or start the release. Shared dirty files are integration input, not
changes to overwrite or reimplement independently.

### 0.3 Severity

| Priority | Confirmed defect | Consequence |
|---|---|---|
| P0 | fork tracking swallows `Tracker::new` and `record_*` failures | executed operations disappear while accounting can still read complete and savings remain reassuring |
| P1 | MCP read records an included internal row and another included delivery row | operation and token totals are inflated |
| P1 | stats aggregates before a lossy privacy projection | duplicate labels with irreconcilable-looking calls, avoided tokens and ratios |
| P1 | raw command payload reaches SQLite before privacy scrub | sensitive bytes can remain in DB, WAL, SHM or free pages |
| P1 | E11 and accounting/rewrite contracts differ across fork and HZR | policy, operation ledger, evasion report and session scorecard disagree |
| P1 | daemon-down fallback discards typed evasion attribution | the same command changes class and avoidability only because the daemon is unavailable |
| P1 | instruction-scope transitions do not remove obsolete targets | global or local instructions remain active after the configured scope changes |
| P1 | fleet repair ignores activation state | `hzr doctor --reconcile-fleet` can rewire a disabled workspace |
| P1 | local excludes are outside lifecycle planning and rollback | dry-run can say healthy and rollback can leave a partial local installation |
| P1 | accounting coverage has two state machines and unlocked files | concurrent recovery can erase a gap; CLI/MCP loss is not shown live |
| P1 | project-only feedback/statusline lacks a canonical workspace gate | state can leak across enabled and disabled workspaces in the same host session |
| P1 | lifecycle mutations are not one serialized desired-state transaction | concurrent or failed enable/disable/install/uninstall can leave split state |
| P1 | release preflight is weaker than CI | a red commit can be tagged and published |
| P0-P1 delegated scope | bundle builds repeat safe-to-reuse work and ship an oversized runtime; platform support is distributed | slow releases, stale-cache risk and partial `darwin-x64` removal unless one manifest owns the graph |

## 1. Single-source-of-truth rule

For 0.6.6, SSoT does not mean “the copies happen to match.” It means:

1. one module or machine-readable artifact owns each semantic decision;
2. all runtime consumers use the owned typed value or generated artifact;
3. compatibility parsing is isolated to an explicitly named legacy migration boundary;
4. user-facing renderers do not reclassify persisted text;
5. source gates reject forbidden duplicate literals, enums, path lists and policy functions;
6. exhaustive construction or round-trip tests make a newly added enum variant impossible to
   implement on only one side;
7. docs describe the owner and link to it instead of copying mutable versioned values.

Tests and immutable historical fixtures may repeat literals when repetition is the subject of the
test. Production modules, installers and release workflows may not become independent owners.

## 2. Canonical ownership registry

The implementation must establish this registry. Names may change during implementation, but an
equivalent single owner and the stated dependency direction are mandatory.

| ID | Concept | Canonical owner | Consumers that must stop redefining it |
|---|---|---|---|
| S1 | fork ↔ HZR rewrite and accounting wire, enums, policy version, evasion and fidelity taxonomies | small leaf `hzr-engine-contract` crate/schema usable by the workspace and excluded fork path dependency | fork rewrite/tracking/registry, `hzr-exec` adapter, hook, daemon, protocol conversion, ledger |
| S2 | HZR ledger schema, migrations and durable writes | `hzr-core` ledger writer API, reached through the daemon in HZR mode | fork DDL/DML against `hzr.sqlite`, CLI/MCP ad-hoc inserts |
| S3 | public privacy-safe operation key and savings inclusion policy | typed accounting contract plus `hzr-core` aggregation | SQL text predicates, CLI `classify_operation(rtk_cmd)`, terminal-only relabeling |
| S4 | accounting gap intervals and live/historical completeness | locked, versioned `AccountingCoverageStore` in `hzr-core` | degraded log, lifetime counter, daemon-unavailable counter, independent session boolean |
| S5 | ambient host/session identity precedence | `hzr_core::ambient_session_id` and one typed host-context resolver | manual environment chains in CLI, MCP, direct-bypass and fork adapters |
| S6 | requested/effective search accounting attribution | one typed builder in core/protocol | separate CLI and MCP mapping functions |
| S7 | instruction files, locality, obsolete targets and exclude entries | typed `InstructionDesiredState` built from canonical `InstructionTarget` records | main lifecycle branches, diagnostics path copies, exclude string, Codex bridge filename literal |
| S8 | authoritative hook workspace, activation and session namespace | one `HookContext` resolver from hook input | cwd-only dispatch/observe and ungated feedback/statusline |
| S9 | adoption mutation plan, lock, rollback/CAS and recovery hash | one normalized `AdoptionPlan` transaction | install, init, SessionStart, enable, disable, uninstall and fleet-specific mutation logic |
| S10 | hook identities and expected installed set | typed hook definitions emitted by adoption | status check `hzr_entries == 7` |
| S11 | product version and release surfaces | workspace `Cargo.toml` version plus explicit generated/validated surface registry | stale prose in `AGENTS.md`, immediate-previous-string replacement, scattered script constants |
| S12 | current engine identity and capability contract | `fork-core/CURRENT_ENGINE.toml` plus typed `contract --json`; immutable import baseline remains separate | `--version`/`--help` substring checks, hardcoded runtime semver, ambiguous `engines.lock.toml` fields |
| S13 | executable resolution | one cross-platform resolver using PATH semantics | eleven `Command::new("which")` copies |
| S14 | MCP tool registry and schemas | one lazily initialized typed registry | separate name/kind table and rebuilt raw JSON-schema vector |
| S15 | doctor adoption snapshot | one immutable read per invocation | repeated reads of mutable Claude settings |
| S16 | first-class rewrite policy | one typed policy function in `hzr-exec` or a leaf `hzr-policy` crate | hook and daemon copies |
| S17 | watcher lease and runtime-directory ownership | coordinator lifecycle state | observability reads that refresh activity and unowned temporary directories |
| S18 | supported release platforms and OS/architecture capabilities | one machine-readable release-platform manifest | installer, updater, build/package/smoke scripts, workflow matrices and mutable docs |
| S19 | bundle component inputs, outputs, cache identity and runtime allowlist | one versioned bundle build manifest consumed by the build pipeline | ad-hoc rebuild checks, copied component lists and independently curated Node/Caveman payloads |
| S20 | release smoke coverage plan | one capability-based matrix derived from S18/S19 | duplicate full smoke jobs and workflow-only platform assumptions |

## 3. R1 — an executed operation can never disappear silently

### Evidence

`fork-core/rtk/src/tracking.rs:1668-1833` intentionally keeps command execution fail-open but
discards every tracker-open and record error. Existing R7 coverage only sees rewrite-daemon and
selected CLI/MCP failures, so the successful command can be absent while status, stats and doctor
still call accounting complete.

The fix preserves command fail-open behavior. It changes visibility and credit, not daemon
supervision.

### Requirements

- R1.1: all producer write failures emit a payload-free typed `AccountingGapEvent` to S4 by a
  channel that does not depend on the failed ledger connection;
- R1.2: the event includes surface, operation family, session pseudonym, first/last failure time
  and count, never raw command text;
- R1.3: one failed operation receives zero savings credit and makes the affected session's live
  accounting state `DEGRADED` at the next prompt boundary;
- R1.4: recovery closes the interval once but never calls missing rows “reconciled” unless rows
  were actually backfilled;
- R1.5: live health and historical completeness are separate fields. A closed gap restores live
  health but preserves an incomplete historical interval and its duration;
- R1.6: corrupt or partially written coverage state reads `unknown`, never `complete`;
- R1.7: all journal mutation is locked and atomic; append racing recovery cannot erase or double
  count an event;
- R1.8: the current degraded log, lifetime file and daemon-unavailable counter cease to be
  independent authorities after migration.

## 4. R2 — truthful stats use a typed public aggregation key

### Evidence

`crates/hzr-core/src/ledger.rs:2576-2700` groups command economics by full `rtk_cmd`.
`crates/hzr-cli/src/stats.rs:600-615` then maps those distinct buckets through
`command_label`, which collapses many values to strings such as
`hzr search <arguments omitted>` without re-aggregation. Live `hzr stats --json --all --since 7d`
confirmed repeated search, execution and raw-shell labels.

The same queries infer raw/neutral treatment from command text even though a typed route exists.
The terminal renderer can clamp a negative command result to zero and round a value below 100% to
`100%`.

### Requirements

- R2.1: introduce S3, a stable privacy-safe key composed only of typed persisted dimensions needed
  for public distinction, for example operation family, route, mode, stage and capability class;
- R2.2: aggregate by S3 before limiting, sorting or rendering. Different private commands mapped
  to the same public key must become one row;
- R2.3: sum executions and all absolute token counters; recompute ratio from the summed numerator
  and denominator; compute duration as an execution-weighted aggregate;
- R2.4: JSON and terminal tables expose the same unique keys and conserved totals;
- R2.5: savings inclusion follows typed `OperationRoute` and `AccountingStage`, never
  `rtk_cmd` text. Text classification is permitted only in a versioned legacy migration;
- R2.6: negative net efficiency stays visibly negative. No `.max(0)` presentation clamp may turn
  measured regression into zero;
- R2.7: `100%` is printed only for an exact 100% value; lower values retain enough precision not
  to make the stronger claim;
- R2.8: rename the panel if its rows are route/family aggregates rather than commands. `--all`
  recovery wording must not promise private or “exact” command details that privacy policy never
  returns;
- R2.9: current and compatibility accounting versions share one public key. A version boundary
  may be shown as a typed dimension only when it changes comparability;
- R2.10: unavailable or incomplete accounting renders `unknown`/`partial`, never a reassuring
  zero.

## 5. R3 — one engine contract, one HZR ledger writer

### Evidence

The fork and HZR independently declare rewrite-plan types, accounting enums and string mappings:

- `fork-core/rtk/src/rewrite_cmd.rs:7-23` versus
  `crates/hzr-exec/src/adapter.rs:246-267`;
- `fork-core/rtk/src/tracking.rs:298-427` versus
  `crates/hzr-protocol/src/api.rs:809-1065`;
- `privacy_typed_v2` in both `crates/hzr-core/src/ledger.rs:28` and
  `fork-core/rtk/src/tracking.rs:909`.

This is already broken: the protocol and rewrite registry support E11, while the fork tracking
whitelist ends at E10 and silently drops E11 attribution. In HZR mode, the fork is also pointed at
`ledger/hzr.sqlite`, creates/alters HZR tables and inserts raw rows independently of `hzr-core`.

### Requirements

- R3.1: establish S1 and remove private production copies of rewrite decision/reason,
  attribution, operation kind/mode/route/stage, search strategy/fallback, filter level, evasion
  class/tier and fidelity reason/validation;
- R3.2: adding a new enum variant on the owner must force every exhaustive consumer to compile or
  fail; no string whitelist is accepted as type compatibility;
- R3.3: E11 must survive registry → hook/daemon → environment/side channel → fork execution →
  operation row with `avoidable=false` and its tier intact;
- R3.4: S2 is the sole schema/migration/write authority for the HZR ledger. The standalone RTK
  database remains supported and parity-tested, but HZR mode must not let fork code execute DDL or
  direct `commands` DML against `hzr.sqlite`;
- R3.5: fork execution returns a typed, privacy-safe accounting receipt to HZR through a bounded
  non-human side channel. HZR commits it through S2;
- R3.6: producer-side canonicalization removes raw command, project and session payloads before
  any HZR DB/WAL write. Post-hoc scrub remains a legacy migration only;
- R3.7: a receipt includes the current engine identity and contract version, not only a semver
  string;
- R3.8: S16 produces the same typed first-class decision for hook, daemon, `hzr exec rewrite`,
  `hzr exec run` and daemon-down fallback;
- R3.9: daemon-down fallback preserves the exact fork `RtkRewriteOutcome`, including evasion and
  avoidability. Only coverage health may differ from the daemon-up path;
- R3.10: MCP `hzr_read` final delivery uses the canonical stage policy and is excluded when the
  same operation already has an included engine stage. Search, read and future tools obtain their
  stages from that policy, not local comments;
- R3.11: fidelity reasons and accounting policy versions are serialized and parsed by S1. Unknown
  values fail closed at the boundary.

## 6. R4 — one session and search attribution path

- R4.1: all CLI, MCP, exec, policy and delivery records obtain identity through S5;
- R4.2: precedence remains `HZR_SESSION_ID`, `CODEX_THREAD_ID`, `CLAUDE_SESSION_ID`, then
  `CURSOR_TRACE_ID`, with blank values skipped and bounded normalization applied once;
- R4.3: direct environment reads are confined to S5, the hook payload exporter and a thin fork
  boundary adapter generated/tested from the same matrix;
- R4.4: replace the independent CLI and MCP requested/effective search-mode functions with S6;
- R4.5: S6 accepts typed response strategy and delivery metadata; it does not parse human output;
- R4.6: search, raw/proxy, context, memory, MCP and direct-bypass policy events for one host session
  must use the same session pseudonym.

## 7. R5 — instruction scope is a real desired-state transition

### Evidence

Current init/install logic calculates targets for the new scope but does not consistently remove
the old targets. Local-to-shared can leave `AGENTS.override.md` or `CLAUDE.local.md`; shared-to-local
can leave global managed blocks. Doctor audits only the current desired set. Local exclude entries
are written outside init/SessionStart rollback and are not part of dry-run change detection.

### Requirements

- R5.1: S7 returns `desired`, `obsolete`, `excluded` and `untouched` targets for the transition from
  the persisted old state to the requested new state;
- R5.2: default scope remains the current shared/global behavior. Local use remains explicitly
  configurable;
- R5.3: `init`, `install`, SessionStart repair, `enable`, `disable`, `uninstall`, doctor and fleet
  reconcile all consume S7 instead of branching on filenames themselves;
- R5.4: a transition installs exactly one current managed block at each desired surface and removes
  only HZR's managed block at every obsolete surface; all user-authored bytes remain;
- R5.5: local filenames, surfaces, bridge targets and `.git/info/exclude` entries derive from
  canonical `InstructionTarget` records. Filename literals are forbidden elsewhere in production;
- R5.6: exclude changes participate in dry-run, plan hashing, backup, CAS, rollback and cleanup on
  local-to-shared/uninstall;
- R5.7: fleet reconcile checks activation before mutation, reports `skipped_disabled`, and never
  rewires disabled registrations;
- R5.8: local targets are confined to the canonical workspace and remain ignored without changing
  shared `.gitignore`;
- R5.9: direct lifecycle writes and fleet writes use one no-follow regular-or-absent file primitive.
  Final-component and parent symlinks are rejected without changing the link or target;
- R5.10: doctor evaluates desired and obsolete targets from the same snapshot and cannot report
  healthy while an opposite-scope managed block is still active.

## 8. R6 — hooks are workspace-bound, bounded and corruption-honest

- R6.1: S8 resolves canonical workspace and activation from authoritative hook input, then derives
  a workspace-bound session namespace;
- R6.2: dispatch, observe, feedback and statusline all use S8. In a disabled or unregistered
  workspace they emit no HZR output and mutate no HZR session or ledger state;
- R6.3: the same host session moving between workspaces cannot carry corrections, scorecard or
  accounting state from one workspace into another;
- R6.4: corrupt session feedback is quarantined and rendered `UNKNOWN`/`PARTIAL`; it is never reset
  through `unwrap_or_default()` to a healthy state;
- R6.5: S10 owns exact hook identities. Status compares sets and reports missing, duplicate and
  altered identities by name; magic entry counts are forbidden;
- R6.6: an upstream user statusline has a fixed deadline, bounded stdout/stderr capture and
  kill-and-reap behavior. HZR's accounting fragment must still render after timeout or overflow;
- R6.7: the live gap transition is emitted once, remains in the statusline while open, and recovery
  is emitted once. Closed historical gaps remain visible in stats/evasion with start, end and
  duration;
- R6.8: CLI and MCP accounting failures enter the same session state as producer and hook failures;
  messaging names the correct surface rather than always saying “MCP”.

## 9. R7 — lifecycle mutations are serialized desired-state transactions

- R7.1: S9 normalizes the complete desired state before hashing, including effective instruction
  scope, activation, hooks, client/MCP config, service intent and excludes;
- R7.2: install, init, SessionStart repair, enable, disable, uninstall and fleet mutation share one
  adoption lock or a proven compatible lock order;
- R7.3: concurrent enable of workspaces A and B retains both changes; install and uninstall cannot
  race;
- R7.4: every mutated file participates in backup, fingerprint/CAS and rollback. Concurrent user
  edits are preserved and reported as conflicts;
- R7.5: retrying a recovery journal with a different normalized desired state fails before mutation;
  an identical retry remains idempotent;
- R7.6: every config change, including scope-only change, receives the advertised exact preimage
  backup;
- R7.7: uninstall has an ownership-checked service removal path that stops/unloads the HZR service
  and removes only its owned definition;
- R7.8: data is retained by default. Replace the unused/ambiguous `--keep-data` behavior with honest
  CLI semantics; deletion requires an explicit `--purge-data` and the standard destructive-action
  boundary;
- R7.9: a second uninstall is idempotent, and a foreign service definition is refused rather than
  overwritten or removed.

## 10. R8 — resource lifetime and read-only observability

- R8.1: watcher `status` is read-only and does not refresh the idle lease;
- R8.2: only real prepare/search/use events update `last_used`;
- R8.3: every owned watcher runtime directory is removed after child reap; bounded diagnostic tail
  may be retained separately on failure;
- R8.4: S13 replaces all production `Command::new("which")` subprocess probes with a cross-platform
  PATH resolver;
- R8.5: resolution works when the executable is present but an external `which` program is absent,
  and builds on Windows;
- R8.6: doctor loads mutable adoption settings once into S15 and derives all findings from that
  immutable snapshot.

## 11. R9 — release and engine identity have enforceable owners

- R9.1: workspace `Cargo.toml` is the product-version SSoT. Mutable prose such as the currently
  stale `AGENTS.md` “0.6.3” statement must be removed or generated, not manually bumped;
- R9.2: the release-surface registry validates every declared marker after synchronization and
  fails on a surface that is two or more releases stale. Silent “previous string not found” is not
  success;
- R9.3: S12 separates immutable import baseline identity from current-engine identity. Names in
  `engines.lock.toml`, `CURRENT_ENGINE.toml`, parity docs and runtime receipts must make that
  distinction explicit;
- R9.4: fork exposes a typed `contract --json` containing interface version, accounting schema,
  capabilities and current-engine identity. Adapter validation uses it rather than human
  `--version`/`--help` substrings;
- R9.5: a fake binary with matching human strings but the wrong typed identity is rejected;
- R9.6: release preflight depends on or invokes the exact complete required CI gates: fmt, clippy,
  full workspace all-target/all-feature tests, fork parity and platform bundle smoke;
- R9.7: publish is unreachable after any ordinary unit-test or clippy failure, not only a test whose
  name begins `acceptance_gate_`;
- R9.8: current `FORK_PARITY.md` and engine lock metadata must describe the actual 0.6.6 current
  engine delta and identity while preserving the immutable v0.1.0 provenance.

## 12. R10 — remove drift-prone Rust and registry slop

- R10.1: S14 lazily builds one typed MCP registry containing name, kind, input schema, output schema
  and metadata. Validation borrows it; `tools/list` clones only the response it must return;
- R10.2: the registry has a name/kind/schema bijection and cannot contain duplicate names;
- R10.3: remove the duplicated CLI/MCP search builders, repeated `ForkRuntimePaths` environment
  lists, fidelity string mappings, privacy classifiers and first-class steering functions as their
  SSoT owners land;
- R10.4: split the confirmed mega-functions only along the new typed ownership boundaries. Do not
  add generic builders, speculative traits or unrelated refactors;
- R10.5: remove changelog comments such as `// changed:` and `// fix #` from HZR-authored production
  code; comments must explain current invariants. Preserve immutable upstream provenance artifacts;
- R10.6: remove confirmed redundant clones/collects when touched by these workstreams and enable
  focused Clippy lints without suppressions;
- R10.7: production HZR code may not add `#[allow(clippy::too_many_arguments)]`, dead-code permits,
  placeholders or TODO abstractions to complete this release;
- R10.8: for `memory_layer/episode.rs`, `memory_layer/ollama.rs` and `write_semantics.rs`, compare
  against the immutable import baseline. If they are HZR-added and still have no runtime consumer,
  delete them. If inherited, retain provenance and add no new HZR claim or dead-code allowance;
- R10.9: if episode persistence is retained and wired, identifiers must be collision-free for two
  sessions started in the same second and behavior must be covered by a real runtime test;
- R10.10: use the existing fork warning ratchet. Its audited pre-change baseline is 141 warnings,
  digest `61cb04477b8ec5d6d734ea86aee855c69cf13ae41b0f4b91c208f10e8f365c83`;
  0.6.6 must not normalize new warnings into that baseline.

## 13. R11 — bundle builds are smaller, reusable and platform-single-owned

This requirement incorporates the approved P0-P1 scope from the coordinated “Ускорить сборку
бандлов” task. Its implementation is integrated only after the final handoff described in §0.2.

- R11.1: remove `darwin-x64` completely from supported release behavior, including installer,
  updater, build, package, smoke, workflow and current documentation surfaces. Removing one matrix
  row while another surface still advertises, selects or accepts it is a release failure;
- R11.2: S18 is the only current supported-platform list. Linux x64, Linux arm64 and Darwin arm64
  are the 0.6.6 native targets; unsupported Darwin x64 requests fail with a precise actionable
  error rather than selecting a stale archive;
- R11.3: S19 defines component dependency inputs, target/profile/features/toolchain identity,
  expected outputs and checksums. Reuse is allowed only after the complete manifest matches and
  every output verifies;
- R11.4: cache identity must include all semantic inputs that can change a component result,
  including relevant sources, lockfiles, build scripts/configuration, target triple and pinned
  toolchain/runtime versions. Modification time alone is never sufficient;
- R11.5: component publication is atomic. Interrupted, partial, missing, corrupt or mismatched
  cache entries are rejected and rebuilt without contaminating the final bundle;
- R11.6: a clean build and a cache-reuse build from the same inputs produce equivalent component
  manifests and final bundle contents. Reuse must be visible in structured build output;
- R11.7: the bundled Node/Caveman runtime follows S19's explicit runtime allowlist and contains only
  files reachable by shipped commands. It remains self-contained and does not fall back to host
  Node, npm state, network downloads or an external Caveman checkout;
- R11.8: preflight checks without data dependencies run in parallel, but all failures remain
  mandatory and deterministic. Aggregation must report every failed branch and prevent packaging;
- R11.9: CI warms and restores component caches using S19 identities. A cold/missed cache remains a
  supported path; a hit cannot bypass checksum, provenance or bundle verification;
- R11.10: S20 reduces duplicate smoke work by capability, not by deleting OS proof. Every supported
  OS-specific installer/update/service/path branch runs at least once, and every produced archive
  is verified on its native architecture for executable identity, manifest and offline dependency
  closure;
- R11.11: build timing is reported separately for cold, warm and cache-restored paths. The PRD does
  not turn one runner observation into a universal performance claim;
- R11.12: final integration preserves unrelated and handoff-owned changes in `install.sh`,
  `scripts/install-bundle.sh` and `scripts/smoke-install.sh`; conflicts are resolved from the
  handoff evidence, never by replacing whole files.

## 14. Acceptance gates

Each gate must fail on the pre-0.6.6 defect, pass on the implementation, and run in release
preflight. Source scans supplement behavior tests; they do not replace them.

### A1 — producer accounting failure is live and durable

Against locked, corrupt and unwritable temporary ledger paths, run a filtered command. Its payload
and exit status remain fail-open, but the next prompt/statusline says `ACCOUNTING: DEGRADED` with a
stable first-failure time and missing-operation count. Stats says `unknown`/`partial`; the missing
operation gets no credit. One successful later write emits one recovery edge and preserves the
closed interval.

### A2 — gap journal cannot lose evidence

Use deterministic barriers to interleave `missing_operation` with `recover`, and to run two
recoveries. Assert exact lifetime/open/closed counts, no lost event, no double count, stable first
timestamp and persisted duration. Corrupt journal bytes must produce unknown/integrity finding,
never complete.

### A3 — one public stats row per key

Seed distinct private commands and compatible accounting versions that map to one public key.
Assert unique labels in JSON and terminal output; exact conservation of executions, baseline,
delivered, gross, regression and net totals; recomputed ratio; weighted duration; identical row
ordering for equal totals.

### A4 — stats never strengthens evidence

Seed one negative row and ratios immediately below and exactly at 100%. Assert the negative value
stays negative, the below-100 value never prints `100%`, exact 100 does, and unavailable accounting
prints unknown rather than zero. `--all` recovery text must describe the privacy-safe data actually
returned.

### A5 — typed route owns savings

Seed a compatibility row with `route=bypassed`, optimized-looking `rtk_cmd` and input greater than
output. It receives zero savings credit and all headline/family/bypass panels reconcile. The new
typed insert API must make an equivalent contradictory new row unrepresentable.

### A6 — one MCP read, one included operation

One `hzr_read` produces one included internal row and one excluded final-delivery row. Headline
operations and tokens count it once, and `stage_excluded_operations` increases by one.

### A7 — engine contract is exhaustive

Round-trip every shared rewrite/accounting/evasion/fidelity variant across fork and HZR. E11 must
reach the operation row with `avoidable=false` and tier intact. A compile fixture adding an owner
variant without updating a consumer must fail. A source gate rejects private wire enums and policy
version literals outside S1, tests and legacy fixtures.

### A8 — no raw payload reaches HZR storage

Execute an operation containing a unique sentinel, then inspect SQLite DB, WAL and SHM bytes before
any HZR reopen or scrub. The sentinel and raw session/project values are absent; typed family and
pseudonyms remain queryable.

### A9 — daemon availability does not change policy attribution

Run the anti-evasion corpus through daemon-up and daemon-down hook paths. Typed decision, source,
class, tier and avoidability are identical; only coverage differs.

### A10 — session identity matrix

Run CLI search, context/memory standalone, exec, MCP and direct-bypass paths with each single session
variable, blanks, and all variables set to distinct values. Every row/event follows S5 precedence
and normalization. A source gate permits the environment names only in the canonical resolver,
hook exporter, generated fork adapter and tests.

### A11 — scope transitions remove obsolete state

In two isolated Git workspaces, exercise shared → local and local → shared via both init and install.
After each transition, desired files have exactly one current HZR block; obsolete files have none;
user bytes are unchanged; local files are ignored; doctor is healthy; a second run is byte-idempotent.

### A12 — local exclude is transactional

Remove only HZR's exclude block and verify dry-run reports a required named change. Inject failure
after instruction writes and assert config, instruction files and exclude exact preimages. A
concurrent user edit survives. Shared transition and uninstall remove only HZR's exclude block.

### A13 — disabled means untouched

Register enabled workspace A and disabled workspace B in selected/local mode. Fleet reconcile
changes A only, reports B as `skipped_disabled`, leaves B files/MCP byte-identical, keeps A local
files ignored and leaves shared `.gitignore` unchanged.

### A14 — all hook surfaces share workspace context

Use the same host session ID in enabled A and disabled B. Create degraded/correction state in A;
invoke dispatch, observe, feedback and statusline in B and assert zero output and mutation; return to
A and assert its state is intact. Force process cwd and hook input cwd to disagree; authoritative
input wins.

### A15 — statusline is bounded and honest

Wrap one upstream statusline that sleeps forever and one that emits more than 2 MiB. The wrapper
returns within its deadline, kills/reaps the child, keeps capture bounded and still emits HZR's
fragment. Corrupt session state renders unknown, not complete.

### A16 — adoption transaction and recovery identity

With barriers, concurrently enable A and B and retain both; race install/uninstall and prove
serialization. Inject failure after each mutation stage and recover exact config/hooks/instructions/
MCP/exclude/service preimages. Retry a failed shared plan as local and require plan-mismatch with
zero mutation; identical retry succeeds idempotently. Scope-only config change creates an exact
preimage backup.

### A17 — uninstall owns service, not user data

With stub `launchctl`/`systemctl`, uninstall stops/unloads and removes the owned definition, refuses
a foreign definition, retains data by default, purges only with explicit authorization, and is
idempotent.

### A18 — no-follow lifecycle writes

Test final-component and parent symlinks for config, instructions and project Codex config through
init, install, SessionStart and enable. Every operation rejects the target and leaves link and target
bytes unchanged.

### A19 — watcher observation is not use

With paused time, perform one real prepare and poll status every half TTL. The idle watcher still
reaps at TTL from the last real use. One hundred start/reap cycles leave no live runtime directories
and only bounded failure diagnostics.

### A20 — current engine identity is machine-verifiable

A fake binary with matching human version/help strings but wrong contract identity is rejected. An
assembled fork reports a typed contract exactly matching `CURRENT_ENGINE.toml`; a new operation row
contains that identity. Baseline and current identity fields cannot be confused.

### A21 — version surfaces cannot silently drift

Seed a declared surface two releases stale. Synchronization must update it structurally or fail and
name it. After sync, every declared marker equals workspace Cargo version; no undeclared production
literal claims another current product version.

### A22 — MCP registry initializes once

Perform 100 input/output validations. Schema construction occurs once; tool names and kinds are
unique and bijective; `tools/list` and validation use the same definitions.

### A23 — executable resolution is cross-platform

Use a temporary PATH containing a target executable but no external `which`; every migrated command
finds it. Run Windows compile coverage. A source gate forbids production
`Command::new("which")`.

### A24 — doctor reads one adoption snapshot

A counting settings loader proves one read per doctor invocation. A mutation between would-be checks
cannot produce contradictory hook/native-mode findings because all projections use the captured
snapshot.

### A25 — runtime environment and targeted slop have exact gates

Capture the environment produced for the asynchronous command runner, `std::process::Command` and
the pure environment-map adapter. The three maps must be byte-for-byte equal for every canonical
fork runtime key, and a source gate permits the key list only in its owner and fixtures. Diff the
three speculative fork modules against the immutable import baseline and enforce R10.8's delete or
provenance outcome. A source gate rejects HZR-authored production `// changed:`/`// fix #` comments,
new lint suppressions and a changed fork-warning digest without an explicit audited ratchet update.

### A26 — release cannot publish red code

Release preflight must invoke the shared complete gate. Workflow tests introduce, separately, a
failing ordinary non-acceptance unit test and a Clippy warning and prove that native bundle and
publish jobs are unreachable. The static workflow gate verifies this dependency cannot be removed
silently.

### A27 — `darwin-x64` is absent from the product contract

S18 enumerates exactly `linux-x64`, `linux-arm64` and `darwin-arm64`. Installer detection, explicit
update selection, bundle build, packaging, smoke planning and workflow generation consume it.
Repository and assembled-bundle source scans find no live `darwin-x64`, `x86_64-apple-darwin` or
macOS Intel support branch outside historical release notes and a negative unsupported-platform
fixture. An explicit Darwin x64 install/update request fails before download with the documented
unsupported-platform error.

### A28 — component reuse is content-safe and atomic

Build every reusable component cold, repeat unchanged, then mutate each semantic input class one at
a time: source, lockfile, build configuration, target, features and pinned toolchain/runtime. The
unchanged run must report verified reuse; every mutation must invalidate only its affected closure.
Delete, truncate and corrupt individual cached outputs and interrupt publication before rename;
each case must rebuild cleanly and never expose a partial component to packaging. Clean and reused
final manifests/checksums must be equivalent.

### A29 — slim runtime is self-contained

The packaged Node/Caveman tree must equal the S19 allowlist plus generated integrity metadata, with
no development dependencies, caches, tests or unreachable source payload. In an isolated temporary
home with network disabled and host Node/npm/Caveman removed from PATH, exercise every shipped
Caveman entrypoint used by HZR and verify identical command registration plus checksum-clean
install/update behavior.

### A30 — parallel preflight preserves failure semantics

Instrument independent preflight branches with barriers and prove overlap. Inject a failure into
each branch separately and several simultaneously; the aggregate result names every failed branch,
returns failure deterministically and makes component publication/package/release jobs unreachable.
The successful parallel result must be semantically identical to the serial reference fixture.

### A31 — CI cache restore cannot become authority

Exercise cold, warm local and CI-restored builds using S19 keys. A matching restored cache passes
checksum and provenance validation before reuse; wrong-target, stale, partial and poisoned entries
are rejected and rebuilt. Disabling the cache still produces the same final manifest. Workflow
tests prove cache warming happens before dependent native builds and cache outage cannot skip any
required gate.

### A32 — reduced smoke matrix retains OS-specific coverage

Generate the smoke plan from S20 and assert every supported OS-specific capability is covered at
least once. Run full install/update/service/path smoke on Linux and Darwin arm64 representatives;
run every produced archive on its native architecture for `hzr --version`, executable architecture,
internal manifest/checksums and offline no-external-engine/runtime proof. The coverage report must
fail if removing any selected job leaves an OS-specific branch or archive invariant uncovered.

### A33 — coordinated handoff is integrated before release mutation

Record the source task handoff with its exact changed files, commit/base identity if present and
green gates. Before editing a shared surface, compare its current bytes and diff to that handoff.
Integration must preserve all handoff-owned and pre-existing user hunks. Until this evidence exists,
tests must treat bundle/release integration, version bump, tag and publish as blocked rather than
complete.

### A34 — full release gate

The final 0.6.6 commit must pass:

```text
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets --all-features
scripts/verify-fork-core.sh --test
scripts/verify-fork-clippy.py
bash -n scripts/*.sh
all A1-A33 regression tests
three supported native bundles and the S20 isolated smoke plan
```

The release report must also verify commit, annotated tag, GitHub checks, three archives, aggregate
checksums, provenance attestations and an installed `hzr --version` of 0.6.6. A delegated or merely
started release is not complete.

## 15. Implementation order

1. Land S1/S2/S3/S4/S5 foundations and migration readers without changing public rendering.
2. Route fork receipts through the sole HZR writer; add producer-side privacy and gap events.
3. Switch stats, session card, evasion, doctor and statusline to S3/S4; remove legacy authorities.
4. Land S7/S8/S9/S10 and migrate every lifecycle/hook entrypoint.
5. Land S11/S12 release and engine identity enforcement.
6. Land S13/S14/S15/S16/S17 and targeted slop cleanup.
7. Wait for and verify the coordinated bundle-build handoff; then integrate S18/S19/S20 without
   replacing shared installer or smoke-script hunks.
8. Delete compatibility copies only after their consumers and migration tests use the canonical
   owners.
9. Run A1-A34, bump only declared release surfaces, update changelog/release notes, then commit,
   tag, push and verify the 0.6.6 release.

No step may temporarily write a second HZR ledger row “for compatibility.” Migration is read-side
or one-time, and new writes use only the canonical path.

## 16. Non-goals

- no hzrd clustering, restart loop or k8s-style fault tolerance;
- no weakening of fail-open command execution;
- no exposure of private command arguments to make stats labels unique;
- no rewrite of immutable v0.1.0 provenance;
- no compatibility promise, archive or hidden fallback for `darwin-x64` after 0.6.6;
- no broad style refactor unrelated to a named SSoT boundary;
- no independent reimplementation of the active bundle-build task before its final handoff;
- no implementation, commit, push, issue closure or release before explicit owner approval of this
  PRD.

## 17. Approval boundary

Approval authorizes implementation of all R1-R11 requirements, A1-A34 gates and the 0.6.6 release
workflow in the stated order. Any need to weaken a gate, preserve a confirmed duplicate authority,
store raw payload, or broaden destructive data handling requires a new explicit decision rather
than an implementation-time assumption.
