# HZR: task-level agent efficiency and host interception PRD

Date: 2026-09-04. Source baseline: fbf204d48704160407e3f06035ac32b9cf0bc67a, workspace and installed HZR 0.7.1.
Status: P0/P1 and visualizer implementation approved after the fresh audit. On 2026-09-05 the user also approved including the already implemented P2 branch `feat/prd-p2-agent-efficiency` (`9016b29`) in version 0.8.0. Historical findings below describe the audit baseline; implementation and final gates are tracked separately.
Authoritative evidence: current source, new local probes, current official host documentation, and the attached seven-day observed-usage audit. Previous PRDs were not used.

## 1. Product decision

Keep the engine assets and the single control plane. Change the optimization objective.

HZR should minimize the total cost of obtaining sufficient, correct evidence and completing an accepted task. It should not maximize the share of commands routed through HZR, the percentage of individually shortened outputs, the number of blocked native reads, or obedience to an instruction block.

Current verdict: useful components exist, but reliable end-to-end token savings and superior agent outcomes are not established. Current orchestration is partly correct, with material defects in receipt collection, scoped retrieval, long execution, and process cleanup. A successful doctor status is insufficient evidence of these properties.

This is a conditional investment decision: proceed with correctness and workflow repair, then evaluate against a competent native agent baseline. Do not publish a general savings percentage or claim SoTA before the gates in section 11 pass.

### Preserve

- HZR-owned RTK fork and immutable import provenance.
- Typed engine interfaces, one workspace identity and one grepai index owner per worktree.
- Central ICM supervision, project/global namespace separation, no hidden fallback memory store.
- Exact execution status, machine-protocol preservation, CAS writes and path confinement.
- Accounting that distinguishes local estimates, host delivery and provider receipts.
- Existing deterministic regression and distribution verification infrastructure.

### Stop treating as success

- A 200-line view that causes three more reads where one full read would have sufficed.
- A policy denial followed by an equivalent HZR call with extra reasoning and tool arguments.
- A compact result with an invalid path, omitted causal error, or unusable recovery command.
- A schema-valid operation that the runtime parser rejects.
- A local byte reduction credited as session-token or billed-money savings.
- A running engine credited as useful retrieval without testing result correctness.

## 2. Evidence and measurement limits

The frozen snapshots in ../review/20260904_195133_evidence/ were captured early during this audit. They include some audit activity and use a rolling seven-day window. They are observational runtime evidence, not randomized experiments, an immutable production-only period, or a complete inventory of host tool calls.

| Metric | HZR project | Global observed scope |
| --- | ---: | ---: |
| Measured operations | 391 | 660 |
| Total observed operations | 409 | 688 |
| Baseline estimated tokens | 637,076 | 867,654 |
| Delivered estimated tokens | 556,977 | 784,661 |
| Net avoided estimated tokens | 80,099 | 82,993 |
| Local reduction estimate | 12.573% | 9.565% |
| Detected repeat-output estimate | 5,672 | 29,695 |
| Net after existing repeat heuristic | 74,427 | 53,298 |
| Recorded accepted model tasks | 0 | 0 |
| Paired provider receipts | 0 | 0 |
| Host-visible coverage complete | false | false |

Measurement is estimated_utf8_bytes_div_4_v1. These are not tokenizer counts. The repeated-output adjustment is a pessimistic heuristic, not a causal estimate of filter-induced retries.

The independent fleet drilldown found two projects with observed activity among 85 existing registered workspaces: HZR and anonymous_bot. The other 83 returned zero observations, which cannot establish that they had no native agent activity. Nine registered paths no longer exist and cannot be queried through the current workspace-path validation.

For anonymous_bot, the frozen remainder is 269 measured / 279 observed operations, 2,894 net estimated tokens avoided, or 1.255%. Its repeat heuristic is 24,023 tokens and produces a negative adjusted estimate; this is a warning to investigate, not proof that HZR caused harm. Approximately 96.51% of positive global net savings is attributed to HZR's own development.

A route marked optimized is not necessarily smaller: 71 exact searches delivered 25,480 estimated tokens against a 4,866 baseline (net −20,614). Meanwhile 38 raw read_tail operations delivered 370,138 estimated tokens, 47.172% of global delivery. Aggregates cannot establish whether these reads were justified; task intent and evidence sufficiency must decide.

The initial doctor probe returned healthy=true while reporting 5,835 fork-producer missing operations and 5,121 unreconciled operations. These fields have different definitions; do not sum them or interpret both as one exact queue length. Later snapshots grew during ongoing work. Source examination establishes that the inventory counts orphan receipt journals lacking a registered context, while the sweeper visits registered contexts.

Live state also showed:

- RTK 0.44.1-fork.1, grepai 0.35.0, ICM 0.10.61; installed component hash checks passed.
- The active project's canonical index exists, with vectors, symbols and graph artifacts.
- A semantic query actually used fork_rgai_grepai, rather than only a healthy-looking config.
- ICM is ready with FTS5; embeddings are disabled.
- Seven old install-smoke ICM processes had PPID 1 and had survived for over a day. Their databases are separate test databases; this is a lifecycle leak, not evidence of seven writers to the production store.
- Claude Code 2.1.195 and codex-cli 0.146.0 were installed. New documentation capabilities were not proven active in these installed versions.

## 3. Operational definition of useful savings

For an accepted task, include all model-visible costs:

~~~text
TaskTokens =
  input instructions and actually exposed tool schemas
  + tool-call arguments
  + delivered tool results and recovery payloads
  + model output, including routing decisions and retries
  + later reprocessing according to actual provider usage

TaskCost =
  uncached_input * applicable_rate
  + cache_reads * applicable_rate
  + cache_writes * applicable_rate
  + output_and_reasoning_usage * applicable_rate
  + any additional model/tool service charges
~~~

Do not add the same token twice when a provider's usage fields already include it. Preserve provider-specific semantics and cache categories. Fixed subscription usage is not automatically equivalent to marginal API-dollar savings.

Report:

1. Local payload reduction, with a comparable observed baseline.
2. Actual model-visible payload reduction, with a versioned host renderer.
3. Total task-token difference, including extra calls and instructions.
4. Provider cost difference, where comparable receipts exist.
5. Accepted-task rate, exact evidence fidelity, recovery count, completion latency.
6. Coverage and missingness for each of the above.

Primary product metric: cost per accepted task at non-inferior quality. Secondary metric: total task tokens and completion latency. Routing share is a diagnostic dimension only.

An unknown task outcome or unobserved baseline is unknown, not zero-cost or failed quality. Never fill missing provider receipts with bytes/4 and label the result billed savings.

## 4. Findings with exact repair locations

### F01 — P0: direct HZR tools lose the accounting lifecycle

Locations:
- crates/hzr-cli/src/main.rs:664-675 dispatches test/read/write to passthrough.
- crates/hzr-cli/src/fork.rs:9-49 creates the child and execs it on Unix.
- crates/hzr-exec/src/adapter.rs:523-550 creates a new accounting handle and drops it in std_command_os.
- crates/hzr-daemon/src/accounting_sweeper.rs:29-81 visits context files only.
- crates/hzr-exec/src/accounting.rs:26-84 inventories receipt journals without those contexts.
- crates/hzr-cli/src/hook_runner.rs:2252-2325 folds inventory into coverage.

Impact: a sanctioned, useful command can produce an orphan journal that the regular sweeper cannot discover as a registered operation. Counting policy compliance therefore does not even imply complete accounting.

Requirement: register a durable typed operation envelope before producer execution. Retain workspace, session, operation ID, engine identity and attribution independently of the parent's survival. Drain idempotently, commit before acknowledge, and recover after restart. Decide explicitly how Unix exec passthrough transfers ownership.

Do not invent workspace attribution for historical orphans. Inventory, quarantine and report unresolved historical receipts; only migrate records with provable attribution. One malformed context must not starve every later context in a sweep.

Acceptance:
- Direct CLI read/write/test, hook rewrite, MCP and daemon-internal producers share the same lifecycle contract.
- Crash at each boundary leaves either an identifiable pending operation or one committed result, never a silently lost operation.
- Duplicate drain does not duplicate savings.
- A healthy daemon drains a completed registered operation within a proposed 5-second SLO.
- Historical unresolved data is separate from current live completeness.

### F02 — P0: ordinary long commands do not fit the managed execution path

Locations:
- crates/hzr-daemon/src/api.rs:3681-3725, ManagedExecutionBudget / managed_timeout_ms.
- crates/hzr-daemon/src/server.rs:24 and :80-85, shared HTTP timeout.
- crates/hzr-cli/src/mcp/tools.rs:6, MCP_EXEC_TIMEOUT_MAX_MS.
- crates/hzr-cli/src/client.rs and crates/hzr-exec/src/executor.rs: execution transport and ownership.

Reproduction:
- scripts/complete-gate.sh --source under hzr exec run ended with exit 124 during clippy.
- Requesting --timeout-ms 1800000 returned HTTP 400: execution timeout exceeds the daemon's 29500 millisecond managed limit.
- The inherited hzr rtk -- test route could run longer, but uses the incomplete direct accounting path in F01.

Requirement: separate operation duration from HTTP/MCP request duration. Introduce execution start, wait/poll with cursor, stdin where appropriate, cancel, and final result. Return an operation handle promptly. A poll never starts the command again. A cancelled client request must not be ambiguously interpreted as cancelled execution.

Acceptance:
- A deterministic 90-second command completes through CLI and MCP with one execution.
- Wait reconnect, host interruption, daemon restart and cancellation have explicit outcomes.
- Output chunks are delivered once; cursor replay is explicit and bounded.
- Exit code, signal, timeout, interruption, stdout/stderr identity and artifact recovery are retained.
- Side-effecting commands are never replayed solely to reconstruct lost output.

### F03 — P0: scoped retrieval gives the agent wrong file identities

Locations:
- crates/hzr-context/src/planner.rs:1036-1050, hit_relative_path.
- crates/hzr-context/src/planner.rs:1081-1103, normalize_search_hit.
- crates/hzr-context/src/planner.rs:397-417 and :580-619, plan candidates and outline lookup.
- crates/hzr-context/src/candidate.rs:108-165, normalize_plan.
- fork-core/rtk semantic-search and memory-plan output contracts.

Live reproduction:

~~~sh
hzr search "drain accounting receipt journal after producer crash" \
  --mode semantic --path crates --limit 5 --json

hzr context plan "sweep_once accounting receipts" \
  --path crates/hzr-daemon --memory-limit 1 --search-limit 5 --json
~~~

The semantic search returned crates/crates/hzr-exec/src/accounting.rs and analogous double-prefixed paths. A filesystem existence check failed. The plan combined project-relative exact evidence with scope-relative src/accounting_sweeper.rs and src/lib.rs. It reported unavailable outlines for four of five plan candidates. Cargo.toml also becomes ambiguous because a wrong rebasing can point to a different existing file.

Root issue: one uniform rebasing assumption is applied across producers that return different path bases.

Requirement: typed path_base plus canonical workspace-relative paths at the engine boundary. Validate existing readable targets, explicit deleted/stale states, containment and scope. Include source revision/content digest. Normalize before outlines, deduplication, provenance and ranking.

Acceptance matrix: exact / semantic / auto / fallback / graph plan × root / directory / file scope × spaces / Unicode / symlinks / worktree / missing or changed file. Every returned live canonical_ref must resolve to the cited content. Add a real pinned-engine contract test; fake engines alone did not catch the discrepancy.

### F04 — P0: optimization policy substitutes lexical prohibitions for task economics

Locations:
- crates/hzr-cli/src/main.rs:1050-1130, bounded_read_arguments.
- crates/hzr-cli/src/hook_runner.rs:601-632, native_pre_tool.
- fork-core/rtk/src/discover/registry.rs:594-698 and its interpreter/diagnostic classification helpers.
- contracts/agent-capabilities.json and crates/hzr-cli/src/instructions.rs:243-344.

Current behavior:
- Native Read/Grep in steer mode receives a denial and a prescription to call HZR.
- Unbounded --level none is rewritten unless a special marker or syntactically “bounded” flag is present.
- The bounded flag test includes --line-numbers, which by itself imposes no volume bound.
- Legitimate structured aggregation through a Python reader was rejected as E3 in this audit; read-only ledger diagnosis was rejected as E9 with a stats prescription that cannot expose all needed dimensions.

This is not a reason to remove authorization or path safety. It is a reason to distinguish authorization from optimization. A suboptimal output size is not, by itself, a permission violation.

Requirement: introduce a cost-aware read planner, described in section 6. Preserve an explicit exact/full operation. For a native tool that cannot be transparently transformed, allow it with measurement rather than forcing a corrective model round-trip by default. Report “no useful equivalent” when the equivalent cannot perform the user's requested computation.

Acceptance:
- Routine authorized work incurs no model-visible deny/retry solely to improve compression statistics.
- Full-file reads are selected when justified by completeness or expected total cost.
- Repeated ranges, overlap, wrappers and failed recovery count against the same task budget.
- Policy cannot be “improved” by adding a meaningless flag.
- Security and user approval rules remain authoritative and independent.

### F05 — P1: host integration capability is static and behind current contracts

Locations:
- contracts/agent-capabilities.json: harnesses.
- crates/hzr-cli/src/adoption.rs, instructions.rs and hook_runner.rs:84-204.
- crates/hzr-cli/src/client_config.rs and MCP registration.

HZR's Claude observer currently emits no replacement output and only measures native file tools. Codex is marked native_hook_routing=false and is instructed to route manually.

Current official documentation describes:
- Claude PostToolUse.updatedToolOutput replacing a matching tool-result shape; invalid built-in shapes retain original output.
- Codex PreToolUse rewriting supported local calls, including unified exec; supported paths have exceptions.
- Codex PostToolUse feedback semantics differ from Claude; blocking a nested code-mode tool can reject its promise.

See section 13. These are documented capabilities, not proof of behavior in the installed releases. Do not transplant a Claude JSON envelope into Codex unchanged.

Requirement: versioned, probe-backed host adapters. Record declared, installed, trusted, observed and verified capabilities separately. Use a synthetic sentinel fixture to demonstrate what reaches the next model request. Keep a fallback for unsupported hosts without claiming universal interception.

Global final-answer replacement and tool-result replacement are separate capabilities. Adding the latter does not establish the former.

### F06 — P1: process cleanup and diagnostics miss real lifecycle failures

Locations:
- scripts/smoke-install.sh:28-39 sends the default termination signal to the daemon.
- crates/hzr-daemon/src/main.rs:8-11 awaits ctrl_c only.
- crates/hzr-memory/src/supervisor.rs:304-342 owns the ICM child.
- crates/hzr-cli/src/foreign.rs:17 and :65-95 match literal “icm serve”.
- crates/hzr-cli/src/diagnostics.rs:1684-1732 and :1969-1972 derive health.

ICM is launched with options between the executable and serve. The literal process matcher misses that actual command shape. Seven orphan smoke ICM processes were observed; this matches a cleanup path where SIGTERM bypasses graceful shutdown. The exact historical exit cause was not recovered.

Requirement:
- Handle SIGTERM and SIGINT with the same bounded owned-child cleanup.
- Track child PID, creation identity, executable identity, data root and owner; avoid ownership by substring alone.
- Test abnormal parent exit and installation-smoke cleanup.
- Report operational_health, accounting_health, retrieval_health and host_coverage separately.
- Missing daemon or incomplete receipts must not be buried in a generic healthy=true.

No automatic termination of unrelated or historical processes. Cleanup must prove ownership; manual historical cleanup is a separate action.

### F07 — P0 for claims: local accounting is not task or provider economics

Locations:
- crates/hzr-core/src/ledger.rs:2574-2619, repeat heuristic.
- crates/hzr-core/src/ledger.rs:3002-3075, host-visible estimate.
- crates/hzr-cli/src/stats.rs:286-410 and :733-810.
- crates/hzr-cli/src/mcp.rs:785 and :1395-1400.
- crates/hzr-core/src/billing.rs.

The host-visible estimator caps every claude-code row at 512 estimated tokens from a assumed 2 KiB preview. It does not identify whether that particular result was spilled, whether the tool was Bash, whether follow-up reading occurred, or what the host actually put into context. The repeat detector uses nearby identical command hashes, with no task intent or source-change proof.

The current economic_claim_ready=false is appropriately conservative, but is hard-coded rather than a completed release-readiness derivation.

Requirement: an event chain linking invocation → native result → transform → host delivery → model request → provider receipt → accepted task. Keep privacy-safe identities and aggregate defaults. Make unmatched intervals and uncertainty first-class. Do not rewrite historical estimates into “actual” values.

Every displayed number needs scope, window, measurement method, denominator and completeness. Separate reduction of repeated identical output from new evidence produced after an edit.

### F08 — P1: MCP covers names, not all essential workflow operations

Sources and detailed audit: ../review/20260904_195133_MCP-Audit.md.

Confirmed:
- Thirteen registered tools and strict schemas are a good base.
- hzr_write cannot accept empty or whitespace-only replacement content through its parser, although the schema permits it. This blocks ordinary deletion and empty file creation.
- Read lacks batch/token-budget/typed source completeness; explicit full ranges are possible but poorly expressed.
- Search lacks pagination; context and memory lack per-request delivery budgets.
- Schema range/Unicode limits drift from parser limits.
- Full JSON appears in text and structuredContent. This is a compatibility design, not proof that every host charges double.
- Some result objects are opaque to the agent.

Requirement: make the existing core tools complete before adding many specialist tool names. Add compact discriminated results and progressive expansion. Long execution requires operation handles, not a large arbitrary timeout. Keep diagnostics and maintenance tools discoverable on demand where hosts support deferred tool loading.

Acceptance: empty delete/create/whitespace edits; schema-parser boundary parity; typed read completeness; stable search cursor; source-hash-bound expansion; task budget; correct cancellation; host-specific delivery measurement.

### F09 — P1: context budgets and confidence do not represent evidence value

Locations:
- crates/hzr-context/src/candidate.rs:143-155 charges max(evidence_tokens, whole-file estimated_tokens).
- crates/hzr-context/src/planner.rs:259-329 always joins code planning with memory recall.
- crates/hzr-context/src/planner.rs:580-619 serially executes per-path outline subprocesses.
- crates/hzr-context/src/planner.rs:1380-1419 budgets candidates but serializes metadata afterwards.
- crates/hzr-core/src/budget.rs:56-172 and :208-264.

Live scoped plan: declared used=7,351 estimated tokens, compact JSON=8,927 bytes, content bodies=2,415 bytes, confidence=0.7503118, locatability coverage=0.2857143. Most file candidates had no usable outline. These are different measurement domains; neither the declared budget nor confidence is a measured probability of a correct answer.

Requirement:
- Budget the actual model-facing representation including headers, metadata and recovery.
- Separate pointer/outline delivery cost from optional full-source expansion cost.
- Make memory optional, explicitly disableable, and restricted by the task's freshness requirement.
- Batch/cached outlines with bounded concurrency, keyed by source hash.
- Rename uncalibrated confidence to a ranking diagnostic or calibrate on held-out relevant-file/span labels.
- Validate references before ranking them as useful evidence.

An arbitrary external memory hit must not be treated as current code truth. Its provenance and stale/superseded state remain visible.

### F10 — P1: recovery must be executable

Location: crates/hzr-context/src/candidate.rs:339-365.

Large memory summaries prescribe hzr memory show <id>; the current CLI has no show subcommand. A live help probe returned unrecognized subcommand 'show'.

Requirement: implement scoped exact memory retrieval by ID, or emit an actually supported retrieval operation. Recovery references must be typed, content-bound and tested through the shipped CLI/MCP. Do not force the agent to formulate a second search that might not return the same memory.

FTS-only is a valid low-overhead mode. Enabling embeddings alone is not a SoTA upgrade. Compare recall@k, useful span precision, stale-memory errors and accepted-task impact before choosing semantic retrieval.

### F11 — P1: failure filtering discards the diagnostic needed for the next action

Locations:
- fork-core/rtk/src/runner.rs:72-106 and :165-259.
- fork-core/rtk/src/guard.rs:71-94.

The test-summary parser selects framework behavior from the command string. For scripts/complete-gate.sh it falls back to the last five lines, despite the script containing Cargo test failures. During this audit it retained the test target name but dropped the assertion and SQLite cause, requiring a rerun for diagnosis.

A “still contains an error word” guard is weaker than retaining actionable error evidence.

Requirement: recognize diagnostic structure from output as well as declared tool family. Preserve failed test names, assertion location, causal error/stack excerpt, exit status and exact-log reference. Handle mixed stdout/stderr without moving unrelated compiler warnings after the useful failure. Unknown formats should return a bounded error-preserving view, not an arbitrary tail.

Acceptance: wrapper scripts, mixed-language suites, failure followed by warnings, multiple failures, no newline, signals, malformed JSON and long single lines. Agent diagnosis should not require rerunning a side-effecting command to recover the original error.

### F12 — P1: the current economics benchmark does not establish paired task savings

Location: benchmarks/hzr-billed-input-prefix-cache-v0.6.4/benchmark.py:151-207.

The runner executes CLI cases, ignores their return status, takes delivered-count deltas, but reads cumulative observed_model_usage from the after snapshot as the case's receipt. It does not establish a per-case provider request/task identity. Reusing prior aggregate usage can therefore make cases appear measured without the intended causal pairing.

Requirement: immutable case/arm/run IDs, exact environment/commit, isolated runtime data, recorded exit status, provider request IDs and per-task accepted outcomes. Unmatched or absent receipts are not measured. The experiment must run an actual agent task; a shell command alone cannot measure a prompt-cache effect in an LLM.

Do not run paid experiments without the required cost authorization.

### F13 — P2: cache-aware policy needs the correct causal model

Location: crates/hzr-cli/src/hook_runner.rs:885-916.

turn_boundary disables mid-turn filtering with the claim that this preserves the request prefix. Transforming a newly appended tool result does not by itself rewrite an already cached prefix. Rewriting older history, changing tool schemas or instructions, and compaction can affect prefix reuse.

Requirement: classify append versus history mutation. Measure actual cache read/write receipts and total cost. Do not disable useful filtering solely because the agent is beyond its first operation. Preserve stable prefixes without treating all new tool output as a cache invalidation.

### F14 — P1: command telemetry cannot identify where RTK work will pay off

Locations:
- crates/hzr-core/src/ledger.rs:884-892 and :3275-3281.
- crates/hzr-cli/src/stats.rs and fleet workspace selection.

Typed operation kind overwrites safe command family, turning exec into other under the whitelist. In the frozen global snapshot, 453 raw other executions cannot be split into cargo/git/gh/docker/etc from the available aggregate. A maintainer therefore cannot rank missing filters using the data.

Requirement: retain separate safe command_family, operation_kind, producer, route, transform, host and reason_no_transform dimensions. Do not store raw commands by default. Add one bounded project-grouped usage export, stable project IDs for deleted worktrees, fixed from/to window and audit-session exclusions.

A zero-credit route can still be useful; report “no comparable baseline” separately from “avoidable raw output”.

### F15 — P2: post-generated response compression has the wrong default economics

Locations: crates/hzr-codec/src/lib.rs:127-174 and :230-284; instructions.rs codec guidance.

The implemented transform primarily removes exact duplicate paragraphs while retaining protected spans. It is not a semantic summarizer. Asking the model to generate a long answer, put it in a codec call, read it back and generate the final answer cannot refund tokens already emitted.

Requirement: make concise generation a lightweight instruction where appropriate; use deterministic deduplication on incoming tool payloads before model ingestion. Keep explicit codec/shadow tooling for evaluation and specific user needs. Do not automatically ask every agent to send long prose back through a codec.

Acceptance: count codec arguments, result and subsequent model output. Unique prose should incur no transform call by default. Protected technical evidence and requested detail remain intact.

### F16 — P1: the deterministic gate inherits incompatible engine accounting state

The complete source gate was run through the supported managed RTK test route and failed in `host_grant_marker_is_consumed_and_persisted_on_the_operation`: SQLite reported `no such table: commands`. A focused reproduction failed 1 of 2 tests; clearing the internal accounting environment made both pass. The full gate was not rerun to green.

Repair locations: `fork-core/rtk/tests/internal_env.rs:23-46` sets `RTK_DB_PATH` but does not isolate inherited receipt mode; `fork-core/rtk/src/tracking.rs:1815` selects receipt accounting from the inherited journal variable; `scripts/verify-fork-core.sh:261-306` launches the deterministic fork suite without a clean standalone-engine environment.

Separate standalone local-DB tests from managed receipt integration tests. Explicitly clear inherited internal variables in standalone fixtures, then deliberately install them in receipt fixtures. Do not weaken accounting or production isolation to make the test pass. Acceptance: the entire source gate passes when launched through HZR, plus both modes are independently tested. This is a reproduced test-environment compatibility defect, not proof of a production host-grant leak.

### F17 — P0/P1: ICM process identity and recovery do not survive all daemon lifecycles

The independent [component integration audit](../review/20260904_195133_Component-Integration.md) establishes two additional source-level paths. In `crates/hzr-daemon/src/state.rs:168-180`, each startup allocates a new ICM endpoint. The existing PID-file check in `crates/hzr-memory/src/supervisor.rs:145-153` probes this new endpoint rather than a durable previous endpoint, so a surviving child can be missed. This is P0 ownership correctness; duplicate production writers were not reproduced.

In `state.rs:253-285` and `supervisor.rs:113-125`, a live but unready owned child returns to readiness/backoff without replacement. A dead child is restarted; a hung live child is not. This is P1 recovery correctness.

Extend W08: persist verified process/DB/endpoint ownership, refuse unsafe second ownership, handle SIGTERM gracefully, and restart only verified owned children after a bounded failure threshold. Attached/foreign processes must never be killed as recovery. Test stale/reused PID, live-unready, transient health failure, SIGTERM, abrupt owner death and restart in isolated fixtures. Do not clean up existing user processes as a side effect of implementation.

## 5. Target architecture

~~~mermaid
graph TD
    A[Agent host and native permissions] --> B[Versioned host adapter]
    B --> C[Typed invocation and task budget]
    C --> D[HZR operation supervisor]
    D --> E[RTK execution and deterministic renderers]
    D --> F[Canonical grepai retrieval]
    D --> G[Central ICM retrieval]
    E --> H[Exact result and content-addressed artifact]
    F --> H
    G --> H
    H --> I[Cost and evidence selector]
    I --> J[Host-specific model-facing result]
    J --> A
    H --> K[Durable receipts and provenance]
    J --> K
    A --> L[Provider usage and accepted task outcome]
    L --> K
    K --> M[Honest local and task economics]
~~~

One control plane does not require one synchronous HTTP request to contain a whole job. Engines remain replaceable behind typed interfaces. The adapter maps host permissions and outputs; it does not impersonate user approval.

### Host interception contract

Before execution:
1. Identify host/version/workspace/task and whether this tool path is supported.
2. Preserve native permission checks.
3. Select an equivalent route only when argument, cwd, environment, stdin, timeout and output semantics are preserved.
4. Create the durable operation record before side effects.

After execution:
1. Store the exact result under scoped local artifact policy.
2. Classify output as exact machine data, source evidence, diagnostic text or eligible prose.
3. Choose passthrough, lossless structural rendering, failure-focused rendering, exact spans, outline, or explicitly requested summary.
4. Apply only when useful and when the host supports the intended delivery mechanism.
5. Record actual host-visible delivery; otherwise label only produced/transformed.
6. Never re-execute a completed command as the recovery operation.

Host adapters must not stack their transform with an equivalent already-applied HZR wrapper. Carry an idempotent transform ID, not only a textual command prefix.

## 6. Cost-aware full-file and partial-read policy

The decision unit is an evidence episode within a task, keyed by canonical file identity and source hash. It is not the spelling of the shell command.

Let:
- F be the cost of one exact full read in the actual host.
- P be immediate partial-result cost.
- R be expected cost of further calls, repeated spans, recovery and model decisions.
- Q be the expected quality penalty of insufficient context.

Select partial only when P + R + Q is lower than F at the required confidence. These are initially transparent estimates; calibrate them with task data. Never present Q or an expected cost as measured savings.

### Default decision rules

| Situation | Preferred action |
| --- | --- |
| Small file, substantial understanding needed | One exact full read |
| Known symbol/location, local fix | Exact enclosing semantic region with needed imports/types |
| Unfamiliar large subsystem | One bounded plan and batch of high-value evidence |
| Audit, exact quote, instruction file or globally coupled module | Exact full read when it fits the available budget; otherwise explicit complete partitions |
| Second read of unchanged file, limited missing evidence | Only missing spans from the same content snapshot |
| Repeated partial reads approach full-read cost | Switch once to complete remaining evidence |
| Filter caused an unresolved error or missing dependency | Expand exact original output immediately |
| Binary or machine protocol | Exact payload/artifact; no prose transformation |
| Unknown useful equivalent | Transparent execution and honest measurement |

The user can request an exact full file without a special “evasion” justification. Filesystem access and secret handling still follow host policy.

Do not enforce an inflexible “always outline first” rule. For a 50-line function module, outline plus read can cost more than the entire file.

### Measured small example from this audit

crates/hzr-daemon/src/accounting_sweeper.rs, 299 lines:
- One exact full-range read: 11,000 bytes including emitted streams.
- Three exact consecutive ranges: 3,978 + 3,797 + 3,497 = 11,272 bytes.
- Difference: 272 bytes, approximately 2.47%, before two extra calls, arguments or reasoning.

This only demonstrates wrapper overhead for equivalent complete coverage. It does not mean every task needs the whole file or that bytes equal provider tokens.

### Proposed compact read result

~~~json
{
  "status": "ok",
  "source": {"path": "src/module.rs", "hash": "sha256:...", "total_lines": 299},
  "view": "exact_ranges",
  "complete": false,
  "shown_ranges": [[1, 80], [130, 190]],
  "omitted_ranges": [[81, 129], [191, 299]],
  "content": "...exact source with original line identities...",
  "recovery": {"cursor": "opaque-scoped-token", "estimated_remaining_tokens": 1200},
  "cost": {"method": "model_tokenizer_or_labeled_estimate", "delivered": 900}
}
~~~

This is a proposed schema, not a claim that these fields are implemented. Keep the full provenance in structured data or a separately expandable audit record when duplicating it would dominate the useful content.

Session “already seen” state is advisory: after compaction, fork or resume the host may no longer retain that content. Require a context epoch or delivery acknowledgement before suppressing it. A local cache hit alone does not prove the model remembers the cached bytes.

## 7. Engine-specific work

### RTK fork

Prioritize error-preserving wrapper-script output, typed source completeness, batch/range reads and safe family-specific reducers identified by real traffic. Preserve code and structured-data fidelity. Never reduce source semantics merely to satisfy a size target.

Use the existing never-worse guard as a local baseline safeguard, then add complete-evidence and task-level checks. Retain the immutable import snapshot; record deltas through current-engine manifests and complete deterministic fork gates.

### grepai

Repair path provenance first. Test warm and cold index behavior, index generation consistency after edits, stale-hit detection and real engine fallback. Keep one owner per worktree and bounded watcher count/idle lifecycle.

On a miss or stale hit, return a useful exact fallback and diagnostic reason in the same call if cost-effective. Do not force the model through “semantic empty → exact search → outline → read” when a single bounded evidence operation can provide the answer.

### ICM

Retain centralized transport and scopes. Add exact ID retrieval, verified expansion, source linkage, supersession/expiry and a task-controlled no-memory mode. Memory facts should be useful and verifiable, not mandatory context injected into every query.

FTS and semantic retrieval need an ablation, including stale-fact failures and retrieval overhead. Do not start another long-lived memory instance per adapter or task.

### Caveman / managed agent

Keep the bridge and runtime pins, but measure the managed-agent workflow independently from Claude/Codex host integrations. Shorter final writing is not sufficient evidence of cheaper or more correct tasks. Keep technical content and requested detail exact.

## 8. Implementation work packages

| ID | Priority | Deliverable | Main files | Depends on |
| --- | --- | --- | --- | --- |
| W01 | P0 | Durable operation/receipt lifecycle for every producer | fork.rs, adapter.rs, accounting_sweeper.rs, ledger_writer.rs, protocol | none |
| W02 | P0 | Canonical path-base contract and pinned-engine regressions | planner.rs, candidate.rs, fork rgai/memory planner | none |
| W03 | P0 | Long-running execution start/wait/cancel | daemon/api.rs, server.rs, executor.rs, CLI/MCP/protocol | W01 |
| W04 | P0 | Error-preserving RTK test/wrapper output and raw recovery | fork runner.rs, guard.rs, capture.rs | W01 |
| W05 | P1 | MCP parser parity and complete edits | mcp.rs, arguments.rs, tools.rs | none |
| W06 | P1 | Cost-aware exact/batch/partial read and typed completeness | CLI main.rs, MCP, fork read modules, budget.rs | W01, W02, W05 |
| W07 | P1 | Versioned Claude/Codex interception adapters and probes | contracts, adoption, instructions, hook_runner | W01, W03, W04 |
| W08 | P1 | Process ownership, SIGTERM cleanup, split health | daemon main/state, memory supervisor, foreign, smoke scripts | W01 |
| W09 | P1 | Per-task retrieval budget, verified expansion, optional memory | planner/candidate/budget, memory APIs | W02, W05, W06 |
| W10 | P1 | Host-delivery receipts and privacy-safe fleet diagnostics | ledger, billing, stats, MCP output | W01, W07 |
| W11 | P1 | Paired end-to-end agent evaluation harness | new benchmark runner and held-out fixtures | W03-W10 |
| W12 | P2 | Reduce instructions/catalog overhead; defer specialist tools | capabilities, instructions, MCP registry | W07, W10, W11 |
| W13 | P2 | Calibrated ranking and optional retrieval/codec enhancements | budget, grepai/ICM adapters, codec | W09, W11 |

W05 can be a small independent correctness change. Do not gate an obvious parser fix behind a full architecture rewrite. W01-W04 are necessary before trusting production measurements. W12-W13 must be justified by measured marginal gains.

Implementation estimates are deliberately omitted: repository code complexity and engine parity obligations require work-package sizing after approval. The sequence and acceptance tests are concrete; calendar promises would be speculative.

## 9. Testing strategy

Three separate layers:

1. Deterministic contracts: native behavior, exact output, namespaces, schemas, ownership, accounting idempotency, cancellation, parser edge cases.
2. Installed-engine and host integration: real pinned RTK/grepai/ICM, isolated data roots, real host versions and trusted hook fixtures. Test return values rather than merely emitted hook JSON.
3. Agent-task utility: accepted solution, total token/cost/latency, retries and retrieval precision.

Required regressions include:
- Every receipt producer, including direct read/write/test.
- Scoped semantic and exact retrieval with the real path conventions of each engine.
- Empty and whitespace edit content, Unicode byte boundaries, empty file reads.
- Full read versus fragments with equal evidence requirements.
- Repeated expansion after compaction and source changes.
- A 90-second command; interruption before/after spawn; reconnect without replay.
- A wrapper script with a failed assertion followed by warning noise.
- SIGTERM/INT cleanup and abrupt death in isolated process groups.
- Unknown/malformed hook payload, unsupported result shape, multiple hook ordering.
- Stats fixed windows, zero observations, orphan receipt isolation and deleted workspaces.
- Dirty worktrees and preservation of unrelated user configuration.

The pre-change baseline is recorded in ../review/20260904_195133_Verification.md. Integrated 0.8.0 implementation status, passing source gates and remaining rollout limits are recorded in [the release verification report](../review/20260905_0.8.0-Verification.md). A red complete gate remains red even if the failure is explained or a targeted isolated rerun passes.

## 10. End-to-end evaluation protocol

Baselines:
A. Competent native agent with the same model, task prompt and repository state.
B. RTK filtering only.
C. HZR execution/read layer.
D. HZR plus grepai.
E. HZR plus ICM.
F. Full proposed host interception.

Use ablations to attribute benefits. Do not compare HZR against intentionally unbounded “cat the entire repository” behavior when the native agent would use rg, narrow reads or host-native truncation.

Initial design target: at least 60 held-out tasks across six repository/language/workflow strata, including projects other than HZR. Run at least three randomized paired trials per arm; refine sample size from a pilot power/variance analysis. These are proposed evaluation requirements, not completed results.

Task strata: local bug fix, unfamiliar architecture navigation, cross-file refactor, failing test diagnosis, configuration/migration work, and source-faithful review. Include tasks where the correct action is a full file read and tasks where only a tiny span is needed.

Freeze model/version, reasoning budget, context policy, repository snapshot, tool versions and acceptance evaluator. Randomize arm ordering and record cache warm/cold state. Use separate sessions and artifact stores. Preserve failed and timed-out runs; never calculate savings only on convenient successful examples.

Evaluate hidden functional tests where meaningful, plus blinded evidence-based review for audit tasks. Report paired distributions and confidence intervals, not only aggregate token sums. Analyze retrieval regressions and missing evidence separately.

## 11. Proposed product gates

The following are engineering targets to validate, not current accomplishments or borrowed benchmark claims:

- Zero semantic changes to protected machine outputs, commands, source spans, permissions and exit status in the deterministic fixture corpus.
- Zero broken canonical references in the supported pinned-engine scope matrix.
- Zero lost/duplicated committed receipts across the defined crash/replay corpus.
- Zero optimizer-only model deny/retry cycles on supported ordinary workflows.
- Exact/full reads and recovery are first-class operations.
- No orphan owned processes after isolated normal exit, SIGINT or SIGTERM.
- Schema/parser agreement on the full boundary corpus.
- Long jobs complete and resume without duplicate execution.
- Lower 95% confidence bound on paired accepted-task cost improvement is positive for the supported target workload.
- Proposed launch target: at least 15% median total task-token reduction at an accepted-task-rate non-inferiority margin no worse than 2 percentage points.
- No task stratum with a material unmitigated quality regression; publish negative results.
- p95 added fast-path interception overhead target <=50 ms when warm, measured separately from execution and index cold-start. Adjust only with explicit evidence and user-impact rationale.
- Net lifetime receipts, host coverage and accepted-task evidence must back public claims; economics readiness is derived from these gates.

A supported workload can pass while another remains experimental. Do not call all hosts or all projects SoTA from one passing arm.

## 12. Release and rollout

Stage 1: correctness repair and shadow measurement on deterministic fixtures. Stage 2: opt-in supported-host pilot. Stage 3: task-level paired trial. Stage 4: bounded rollout with automatic disabling of a regressing transform, preserving normal authorized execution.

Per-transform rollback must retain the same permission boundary and return exact data; it must not execute commands again. Preserve old receipt schema readers and provenance during migration.

Distribution includes canonical contracts and generated instruction surfaces, CLI/MCP schema registrations, managed agent bridge, bundled engine identities, platform smoke tests and checksums. Update only affected release surfaces. Do not edit immutable import manifests to hide engine changes.

The user approved P0 and P1 implementation after this audit, and separately approved immediate visualizer UI work. On September 5, the user also approved including the already implemented P2 branch `feat/prd-p2-agent-efficiency` (`9016b29`) in 0.8.0. This includes prefix-effect-aware placement, concise generation defaults, instruction tiers and explicit uncalibrated ranking metadata; it does not establish real deferred server-catalog loading or calibrated retrieval quality. Source preparation and isolated bundle verification are included. Publication, cleanup of existing processes, global configuration migration and paid model benchmarks remain separate. UI scope and validation are recorded in ../review/20260904_UI-Design.md.

## 13. Current primary references

Checked 2026-09-04. Host support is version-sensitive and must be probed locally before activation.

- [Claude Code hooks reference](https://code.claude.com/docs/en/hooks): tool-result replacement shape and distinction from side effects already executed.
- [Codex hooks](https://learn.chatgpt.com/docs/hooks): supported local-tool interception, trust, argument rewrite, and distinct PostToolUse/code-mode feedback behavior.
- [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching): matching rendered prefixes, changing history and actual cache accounting.
- [Anthropic advanced tool use](https://www.anthropic.com/engineering/advanced-tool-use): deferred tool discovery and programmatic execution that can keep intermediate results out of model context.

These references motivate architectural options; their published measurements are not HZR measurements.

