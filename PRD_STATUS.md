# PRD: HZR 0.1.0 - implementation status by sections

> Historical audit `v0.1.0`; The summaries, gap registry and acceptance below are intentionally not rewritten retroactively. In the release candidate 0.2.0 G1–G7 and instruction-wiring gap A1 are closed, and one installer delivers a versioned self-contained bundle with full fork-core, patched grepai, ICM, caveman-code and Node.js 22.17.1. The current status, including platform-verification boundary and unmeasured economic KPI, is in [PRD_STATUS_0.2.0.md](PRD_STATUS_0.2.0.md); requirements - in [PRD.md](PRD.md) and [PRD_ADOPTION.md](PRD_ADOPTION.md).

**Audit date:** 2026-07-31
**Base:** [PRD.md](PRD.md) · cross with [FORK_PARITY.md](FORK_PARITY.md) · details in [REVIEW.md](REVIEW.md)
**Method:** self-launch gate + end-to-end product run from clean data root

## Legend

|Marker|Meaning|
|---|---|
| ✅ |Implemented and **tested by me by launch**, not by report|
| 🟢 |Implemented, verified by reading code/tests (without separate run)|
| 🟡 |Partially implemented or the guarantee is weaker than stated; there is a gap|
| ❌ |Stated in PRD, actual behavior is the opposite|
| ⚪ |Knowingly outside 0.1.0, recorded in PRD/FORK_PARITY|
| 📊 |Requires measurement; no data|

**Summary:** ✅ 34 · 🟢 21 · 🟡 5 · ❌ ​​1 · ⚪ 7 · 📊 9

---

## §1 Solution and fork-core contract

| # |Requirement|Status|Base|
|---|---|---|---|
| 1.1.1 |The source of truth is the actual worktree, including dirty/untracked| ✅ | `SNAPSHOT.toml`: `source_head=5f403c46`, `source_diff_sha256`, `source_status_sha256`, `selection = "git tracked ... plus untracked"` |
| 1.1.2 |The entire source/product surface has been transferred| ✅ |516 files on disk = `included_file_count = 516`|
| 1.1.3 |**Zero simplified reimplementations in the runtime path**| ✅ |Purposefully searched: search → fork `rgai`, plan → fork `memory plan`, exec → fork `rewrite`. Not found|
| 1.1.4 |Only thrown/secret exceptions| 🟢 |`exclusion_record_count = 32`, categories declared|
| 1.1.5 |Machine verifiable manifest| ✅ | `SNAPSHOT_V2.tsv` (79 KB), hex-encoded paths, per-file digest, 4 tracked deletions |
| 1.1.6 |Fork builds and passes its tests from HZR; CI rejects drift| 🟡 |Assembly and verify - ✅; regression suite **non-deterministic** → **G3**|
| 1.1.7 | Integration through auditable adapters | ✅ | `hzr-exec` is a process adapter with no independent rewrite table |
| 1.1.8 |Stock RTK does not fallback| ✅ |`engines.lock.toml`: `rtk-upstream-reference` with `runtime = false`, `source_kind = "reference-only"`|
| 1.x |One CLI/daemon/config root/protocol/budget/ledger| ✅ |Run: 15 commands, singleton `hzrd`, one data root, protocol 1|

## §2 Product contract

|Requirement|Status|Base|
|---|---|---|
|Formula `cost_per_accepted_task` as a function to be optimized| 🟢 |`EconomicInput::expected_value()` in [hzr-codec/src/lib.rs](crates/hzr-codec/src/lib.rs) implements `saved − overhead − p(retry)·cost`|
|Transformation only if expected_value is positive| 🟢 | `is_profitable()` + `test_economic_gate_charges_instruction_and_retry_cost` |
|Exact/lossless policy for code/JSON/paths/commands| 🟡 |`FidelityClass::Exact` is respected (checked), but the protected regex does not cover the 4 declared classes → **G2**|
|Raw / content-addressed reference when undefined| ✅ |`test_choose_never_worse_*`, `test_protected_duplicate_falls_back_to_raw`; in the raw run it is stored byte-for-byte|

## §3 Evidence

|Requirement|Status|
|---|---|
|§3.1–3.5 research findings recorded with sources|🟢 Document, not code. All 5 subsections have links; conclusions are consistent with §15|

## §4 Goals and guardrails

### §4.1 Goals for 0.1.x

| Goal |Status|Base|
|---|---|---|
|One semantic index on `(workspace, root, embedder, model, dim)`| ✅ |Run: `generation=68f4b53a...`; `test_linked_worktrees_share_repository_identity_but_not_index_identity`|
|100% fork surface in hash-locked fork-core| ✅ | verify-fork-core.sh → `verified` |
|Zero reimplementations in the runtime path| ✅ |see 1.1.3|
|Zero project-local index data; only verified symlink| ✅ |Run: `ForeignSymlink` detected and blocked without mutation|
|One ICM process and one canonical DB| 🟢 | `test_supervisor_owns_one_process_and_second_instance_attaches`, `test_supervisor_recovers_orphan_without_spawning_duplicate` |
|Exact RTK rewrite contract (rewrite ≠ auto-allow)| ✅ |Run: verdict `ask:` for `cat main.rs`; exit 0/1/2/3 are covered by tests|
|Hard evidence budget according to the marked estimate; without hidden second pre-read| ✅ |Run: `tokens=180/16000` and `180/12000`; `test_plan_never_exceeds_hard_budget`|
|**Adaptive codec with protected spans and raw fallback**| 🟡 |Raw fallback ✅; but the codec is reduced to a dedupe of paragraphs, spans are narrow → **G2**, **G4**|
|caveman-code managed mode without duplicate layers| 🟡 |Disablements are present, but the order "before first prompt" is checked by text search → **G5**|
|Actual usage separate from estimates| 🟢 | `test_ledger_keeps_estimates_out_of_actual_totals` + `test_usage_route_records_provider_tokens_separately_from_estimates` |
|All engines are checked by version/integrity before launch| ✅ |Run: `doctor` caught real drift ICM 0.10.57 vs 0.10.61 → exit 1|
| Offline local mode; telemetry off | ✅ |Run: `RTK_TEE=0`, `RTK_TELEMETRY_DISABLED=1`; loopback-only|

### §4.2 Product metrics - 0 out of 9 measured

|Metrics|Target|Status|
|---|---|---|
| median billed cost / accepted task | ≥ −30% |📊 no data|
| median turns | ≥ −20% | 📊 |
| uncached input tokens | ≥ −35% | 📊 |
|tool-result bytes in context| ≥ −60% | 📊 |
| retrieval recall@20 | ≥ 95% |📊 no gold set|
| task success non-inferiority |≤ 1 p.p.| 📊 |
| p95 warm overhead | ≤ 250 ms |📊 not instrumented|
|p90 cost of a single task| ≤ +5% | 📊 |
|stale-index incidents| 0 |📊 no production observations|

**Reporting Compliance - ✅ (verified).** `hzr savings` produces `tasks: 0, accepted: 0` and **does not** print `cost_per_accepted_task`. The requirement “UI is not allowed to show the forecast as proven savings” is met literally.

### §4.3 Non-goals - met

| Non-goal |Status|
|---|---|
|Compression reasoning provider|✅ not done|
|Regex rewrite code/JSON/enums/args|✅ `FidelityClass::Exact` short-circuits; tested by run|
|Shared physical SQLite for index+memory+ledger|✅ Run: `memory/icm/memories.db`, `ledger/usage.sqlite`, `fork/history.db` are separate|
|Cloud control plane| ✅ loopback-only, `test_config_rejects_non_loopback_bind` |
|Auto-delete legacy indexes|✅ `test_migrate_legacy_index_refuses_*` - nothing is deleted|
|Copying caveman-code in Rust| ✅ managed npm runtime |
|Rewriting fork| ✅ snapshot immutable |
|Replacing fork-core with stock RTK|✅ `runtime = false` for upstream|
|"Zero Quality Loss" Promise|✅ no wording|

## §5 Architecture

|Requirement|Status|Base|
|---|---|---|
|§5.1 Ownership matrix - the only owner on concern| ✅ |Checked by running through all 10 rows of the matrix|
|§5.2 clause 1 intent without rewriting| 🟢 |Codec doesn't touch user intent; density contract is added to the answer|
|§5.2 p.2 one managed grepai lifecycle, without unconditional query| 🟢 | `IndexCoordinator`, `test_coordinator_reuses_one_watcher_for_repeated_prepare` |
|§5.2 p.3 fork `memory plan` + one ICM recall in parallel| ✅ |Run: `fork-plan pipeline=graph_first_v1` + `warning MemoryUnavailable` in one answer|
|§5.2 p.4 `rgai` only with an empty planner| 🟢 |Scheduler logic; `test_search_and_context_use_managed_fork_core_commands`|
|§5.2 p.5 normalization to `ContextCandidate` with provenance/hash/generation| ✅ |Run: `{"sources":["tier_c","semantic:budget"],"estimated_tokens":180}`|
|§5.2 p.6 one candidate per content ref + hard limit| 🟢 | `test_plan_deduplicates_same_content_reference`, `test_equal_content_has_stable_reference` |
|§5.2 p.7 bounded metadata, without eager reread| 🟢 |Run: plan returned metadata+score, not file content|
|§5.2 clause 8 density contract up to generation; without second lossy post-processing| 🟢 |`appendSystemPrompt: responseContract` in bridge|
|§5.2 clause 9 only allowlisted tools| 🟢 |`installManagedToolGuard` + `beforeToolCall`; but see **G5**|
|§5.2 clause 10 JSON validation, empty output rejected, usage in ledger| 🟢 | `test_bridge_enforces_response_quality_before_and_after_generation` |
|§5.2 clause 11 in ICM only durable facts| 🟢 |No automatic recording tool output|

## §6 Components

### §6.1 `hzr-protocol` — ✅
Versioned envelopes; actual/estimated are separated at the type level. `test_envelope_serialization_preserves_protocol_version`, `test_token_source_preserves_provider_only_counts`.

### §6.2 `hzr-core` — ✅
Canonical layout, engine lock, fusion, budgets, ledger, migration state. The run `hzr init` created a smoothly designed layout.

### §6.3 `fork-core` + `hzr-exec` — ✅ (strongest part)

|Requirement|Status|
|---|---|
|Full hash-locked snapshot dirty worktree| ✅ verified |
|`hzr-exec` - thin adapter without its own rewrite table| ✅ |
|Exit code/stderr/paths/identifiers saved| ✅ `test_pipeline_reports_exact_exit_and_stderr_channels`, `test_pipeline_reports_exact_unix_signal` |
|Raw/direct fallback based on fork semantics; stock RTK is prohibited| ✅ |
|Public name `hzr`; fork is not published as control plane|✅ Run: `hzr rtk -- --version` → `rtk 0.44.1-fork.1`|

40 tests for 2843 LOC: pipes, `&&/||`, heredoc, redirects, xargs, quoting, non-UTF8 argv, process-group timeout, spill to disk.

### §6.4 `hzr-index` — ✅ (11/11 points)
Canonical paths + git common dir, stable IDs, one config/watcher/generation, version check against lock, store preparation, no competing ranker, nested/legacy detection without deletion, refusal of someone else's watcher, legacy locking up to `migrate apply`, generation+hash invalidation.
The run confirmed `ForeignSymlink` and `grepai_duplicates: none found`. Patch `--no-worktree-discovery` is being checked in CI on pinned commit + `go test ./cli`.

### §6.5 `hzr-memory` — 🟢 (13/13 points implemented)
Fixed DB, singleton lock, MCP store with full semantics, typed CLI JSON recall, repository-scoped topic namespace, private permissions, circuit breaker, idempotent lifecycle, version check, no code indexing.
The scoping project is seriously covered: `test_isolate_project_memories_removes_global_and_cross_repo_records`, `test_topic_belongs_to_project_never_accepts_global_or_foreign_topics`, `test_memory_route_rejects_user_supplied_project_override`.
Run: version check worked (0.10.57 ≠ 0.10.61) → `warning MemoryUnavailable`, code plan saved. Exactly §10.

### §6.6 `hzr-codec` - ❌ / 🟡 (the only breach of contract)

|Requirement|Status|Base|
|---|---|---|
|Profiles `off`, `safe`, `adaptive`, `compact`, `shadow`| 🟢 |All 5 in CLI and protocol|
|`adaptive` checks the economy before adding a contract| 🟢 |`is_profitable()` + threshold `>= 600` output tokens|
|**`shadow` does not change the delivered content**| ❌ |**Changes.** `--profile shadow` → `"changed": true`, paragraph deleted → **G1**|
|**`shadow` writes counterfactual size**| ❌ |Doesn't record. `counterfactual\|shadow_size` → 0 matches → **G1**|
| Protected spans: code fences, inline code, URLs, flags, hashes, versions | 🟢 |Regex coated, verified|
| Protected spans: **paths, identifiers, enum-like, structured payloads** | 🟡 | `src/main.rs`, `MAX_RETRIES`, `handle_budget_overflow`, `{"k":1}` → `protected_spans: []` → **G2** |

Reason G1 - [lib.rs:106](crates/hzr-codec/src/lib.rs#L106): short-circuit only covers `Exact` and `Off`; `Shadow` falls into `deduplicate_paragraphs`. None of the 5 codec tests cover `Shadow`.

### §6.7 `hzr-agent` — 🟡

|Requirement|Status|
|---|---|
| Version + npm integrity pinned | ✅ `test_package_lock_digest_matches_compiled_provenance`, `test_package_lock_digest_rejects_tampering` |
|Isolated `agentDir` under HZR data root| 🟢 `test_prepare_agent_data_dir_rejects_symlink` |
|Native RTK/repo-map/memory/hooks/compression/telemetry/builtins disabled **before first prompt**|🟡 Calls are present, but the order is checked `BRIDGE.contains(...)` → **G5**|
| Exact allowlist custom tools | 🟢 `installManagedToolGuard` + `beforeToolCall` |
|One bounded prefetch as untrusted evidence| 🟢 `context_prefetched: true` |
|Text and strict JSON modes| 🟢 |
|Credentials are not copied to ledger| 🟢 §9 |
|Health: protocol 1, HZR 0.1.0, exactly one ready fork-core|✅ `test_bridge_preflight_requires_compatible_hzr_and_ready_fork_core`; run: `state Degraded` with incomplete environment|
|Usage fasts once with terminal outcome; accounting failure does not mask the result| 🟢 `test_bridge_accounts_provider_usage_once_for_every_terminal_outcome` |
|Managed launch fails closed; other commands are working|✅ Run confirmed|
|Node ≥ 20.18.1, Node 26 blocked| 🟢 `test_supported_node_range_keeps_node_25_and_excludes_node_26` |
|`adm-zip` override, `npm audit` without high/critical|✅ CI step green|
|Residual `cavemem --version` probe|🟡 Recognized in PRD and FORK_PARITY|

### §6.8 `hzrd` and `hzr-cli` — ✅ exact match

**12/12 routes** in [server.rs:22-33](crates/hzr-daemon/src/server.rs#L22) - health, engines, search, context/plan, memory/recall, memory/store, exec/rewrite, exec/run, exec/approval, fork/run, codec/compile, usage. No extra ones and no missing ones.

**15/15 commands CLI** - `init, doctor, daemon, engines, index, search, rgai, context, memory, exec, codec, agent, savings, migrate, rtk`.

`bin/rtk` → alias to `bin/hzr` with normalization to `hzr rtk --`: `test_normalize_rewrites_installed_rtk_alias`, `test_normalize_preserves_non_utf8_fork_argument`.

## §7 Data layout and prohibition of duplicates — ✅

Running `hzr init` from pure `HOME` created `runtime/`, `workspaces/`, `memory/icm/`, `ledger/`, `engines/`, `config.toml`; `fork/` and `migrations/` are created lazily (confirmed by `RTK_MEM_DB_PATH=<data>/fork/mem.db` in the run).

|Invariant|Status|
|---|---|
|index and memory are physically separate|✅ verified|
|No project-local index data; `.grepai` symlink only|✅ verified|
|Real `.grepai` = legacy, blocks until migration|✅ verified (`ForeignSymlink`)|
|Legacy is detected by read-only scan| 🟢 `test_collect_markers_finds_database_without_following_symlinks` |
|Migration only with explicit command + backup + 2 manifest|🟢 6 tests `test_migrate_legacy_index_*`|
|Auto-delete/quarantine prohibited| ✅ |
| Singleton `hzrd` + worktree owner lock |✅ Run: `hzrd.lock`; `test_acquire_refuses_second_owner_and_releases_without_deleting_file`|
|One content hash is not repeated in pack| 🟢 `test_plan_deduplicates_same_content_reference` |

## §8 Version and supply-chain — ✅

`engines.lock.toml` coincides with table §8 **literally** for all 6 components (grepai 0.35.0/`65c345ca`, ICM 0.10.61/`c3a1bac7`, fork 0.44.1-fork.1/`5f403c46`, upstream 0.44.1/`36591fb0` reference-only, Caveman 1.9.1/`0d95a81d`, caveman-code 0.65.2/`4700b8fa` + sha512 integrity).

CI actually clones grepai on pinned commit, applies the patch, runs `go test ./cli`. Patch SHA-256 are fixed for both patches.

## §9 Security and privacy — ✅ (tested on a live trial)

|Requirement|Status|
|---|---|
|Loopback-only; non-loopback is not supported| ✅ `test_config_rejects_non_loopback_bind`, `test_hzr_api_rejects_remote_or_credentialed_hosts` |
|Bearer token to local API| ✅ `test_all_routes_require_bearer_authentication` |
|Private permissions for secrets|🟡 `hzrd.token` = `0600` ✅, but `hzrd.token.lock` = `0644` → **G7** (does not contain a secret)|
|API keys are not logged| ✅ `test_bearer_token_debug_is_redacted`, `test_redact_token_removes_every_occurrence` |
| `RTK_TEE=0`, `RTK_TELEMETRY_DISABLED=1` |✅ **observed in the output `exec rewrite`**|
|Telemetry and raw retention off| ✅ |
| Allowlist argument shapes + canonical paths | ✅ `test_managed_fork_api_confines_read_and_write_paths`, `test_managed_fork_api_rejects_symlink_escape` |
|Destructive commands → separate verdict|✅ Run: verdict `ask:`|
|Body/capture/time limits; traversal deviates| ✅ `test_payload_limit_reserves_json_envelope_space`, `test_capture_writer_truncates_at_safe_memory_cap` |
|Ledger without prompt/response body| 🟢 |

## §10 Failure modes — ✅ all 11 played or covered

| Failure |Status|
|---|---|
|`hzrd` is not available → managed block, `hzr rtk` is working|✅ **tested by run**|
|grepai is missing/deprecated → exact fallback, degraded| 🟢 `test_connect_rejects_unpinned_grepai_version` |
| index stale → stale provenance | 🟢 |
|legacy/duplicate/foreign index → ​​typed error, no deletion|✅ **verified** (`ForeignSymlink`)|
|ICM not available → warning + code plan|✅ **verified** (`warning MemoryUnavailable` + plan)|
|codec invariant is broken → raw| ✅ `test_protected_duplicate_falls_back_to_raw` |
|fork-core unavailable/version mismatch → block, no stock RTK| ✅ `test_adapter_version_mismatch_fails_closed`, `test_adapter_missing_binary_fails_closed` |
|fork chose raw/fail-open → semantics preserved| ✅ |
|provider usage missing → estimated columns only| 🟢 |
|caveman-code SDK drift → block with remediation| 🟢 `test_bridge_preflight_*` |
|budget exhausted → refusal with reason, no hidden extension| 🟢 `test_plan_never_exceeds_hard_budget`, `test_budget_never_underflows` |

## §11 Migration — 🟢
All 8 steps of `migrate apply` are implemented; `scan` read-only. Failures (escaping symlink, special files, active owner, source mutation, partial state) are covered by 6 tests. Backup is not deleted automatically. `/Users/andrew/Programming/rtk` has not been changed.

## §12 Verification strategy

### §12.1 Rust quality gates — ✅ launched by me
```
cargo fmt --all --check                → exit 0
cargo clippy --workspace -D warnings   → exit 0
cargo test --workspace --all-targets   → 160 passed; 0 failed
```
Plus CI: MSRV 1.85, `bash -n scripts/*.sh`, `node --check`, `npm audit`.

### §12.2 Contract tests

|Requirement|Status|
|---|---|
|Snapshot manifest reproduces 100% source set| ✅ verified |
|**The entire fork test/benchmark harness is present and running**|🟡 is present and runs, but **non-deterministically** → **G3**|
|Fork CLI/rewrite/read/write/rgai/memory/guard lossless| ✅ |
| stdout/stderr/exit preservation | ✅ |
| grepai JSON fixtures + version drift | 🟢 |
| root/worktree identity + duplicate detection | 🟢 |
| ICM singleton race, stale PID, token perms, breaker |🟢 4 tests|
|Amount of token estimates ≤ hard limit| 🟢 |
| **Protected spans survive byte-for-byte** |🟡 proved only on a subset of the regex → **G2**|
| Estimates never increment actual | 🟢 |
| **Caveman duplicate layers disabled before prompt** |🟡 text search, not runtime order → **G5**|
| Daemon body limit, timeout, auth, loopback | 🟢 |

### §12.3 Paired benchmark — 📊 not completed
The infrastructure is ready (ledger separates actual/estimated, there are outcome labels), the data itself is missing.

## §13 Release acceptance — 15 ✅ / 2 🟡 out of 17

| # |Criterion|Status|
|---|---|---|
| 1 |fork-core imported in its entirety, manifest verified independently| ✅ |
| 2 |`FORK_PARITY.md` without `missing`/`reimplemented`|✅ (the only occurrence is in the description of the legend)|
| 3 |Stock RTK is missing from execution path and bundle| ✅ |
| 4 |All crates are compiled without warnings| ✅ |
| 5 |Quality gates green| ✅ |
| 6 |`hzr doctor --json` checks pins and ownership|✅ **run: caught a real drift, exit 1**|
| 7 |ICM start/stop race test proves singleton| 🟢 |
| 8 |Nested `.grepai` fixture is detected and not removed| ✅ |
| 9 |`hzr search` uses grepai 0.35.0 + exact fallback| ✅ |
| 10 |`hzr rgai` uses the same generation| ✅ |
| 11 |**`hzr exec` delegates to fork-core and goes through the entire fork regression suite**|🟡 delegation ✅; suite is non-deterministic → **G3**|
| 12 |**Codec keeps protected spans**|🟡 on a subset; `shadow` generally mutates the content → **G1/G2**|
| 13 |Managed caveman-code smoke confirms disabling layers|🟢 (weak verification method → ​​**G5**)|
| 14 |CLI/daemon smoke from clean data root|✅ **reproduced**|
| 15 |README: installation, invariants, recovery|✅ 14 sections, including “Quick start”, “Data root”, “Checks”, “Fair boundaries”|
| 16 |ICM contains handoff|🟢 README §"Handoff for LOOP-agents"|
| 17 |Initial commit and version 0.1.0| ✅ `c5a10f1 feat: release hzr 0.1.0`, workspace `version = "0.1.0"` |

## §14 Delivery status

What was declared has been implemented and confirmed. Deliberately not included (⚪, not a gap): background `daemon start/stop`, automatic engine sync, hook installer, destructive cleanup, Windows artifact, formal legal review, crash-safe usage outbox, runtime re-attestation of the compiled binary.

## §15 Decision log - all 11 decisions are followed in the code ✅
Tested: HZR - stand-alone product; fork is irrevocable; grepai is the only semantic index; `rgai` — facade without storage; ICM is the only durable memory; Caveman - adaptive contract; caveman-code - optional runtime, not second control plane; HZR Core - owner of budget/policy/lifecycle/ledger; actual ≠ estimates; duplicates are not removed; quality through outcome and invariants.

---

## Registry gap

| ID | PRD section | Severity | Summary | Fix |
|---|---|---|---|---|
| **G1** | §6.6, §13.12 |**HIGH**|`shadow` mutates the delivered content (`changed: true`, paragraph removed) and does not write counterfactual size - the behavior is **inverted** relative to PRD|Add `Shadow` to short-circuit `transform()` (1 line) + profile test; implement accounting or remove the statement from §6.6|
| **G3** | §1.1.6, §12.2, §13.11 |**AVERAGE**|`tracking::tests::test_timed_execution_records_time` — race on a common tracker database (`get_recent(5)`): parallel FAILED 3/3, single-threaded ok. Makes release-gate non-deterministic|`--test-threads=1` for the module, per-test `RTK_DB_PATH`, or a unique command name + filter|
| **G2** | §6.6, §2, §12.2 |MEDIUM (latent)|Protected spans do not cover relative paths, identifiers, enum-like, structured payloads - 4 declared classes|Expand the regex **to** the appearance of any sentence-level transformation|
| **G5** | §6.7, §12.2, §13.13 | LOW | Bridge invariants are checked by `BRIDGE.contains(...)`: string presence does not prove the required “before first prompt” ordering | Add a runtime assertion for call order in the fake bridge |
| **G4** | §4.1, §6.6 | LOW | “Adaptive codec” means selector density plus exact duplicate-paragraph deduplication (261 LOC), so the wording overpromises. It also removes the trailing `\n` | Align §6.6 with the implementation |
| **G6** | — |COSMETICS|Duplicate error suffix: `...(os error 2); run \`hzr daemon serve\`: ...(os error 2)`|Remove extra `anyhow` context|
| **G7** | §9 |INFO|`hzrd.token.lock` = `0644` with `hzrd.token` = `0600` (does not contain a secret)|Align rights in `runtime/`|

## Open release issue

Not a gap, but an incomplete measurement: **0 of 9 metrics §4.2**. The functional prerequisites for paired benchmark (§14 p. 1–2) are ready - ledger separates actual/estimated, outcome labels are written, `savings` does not replace the absence of data with a forecast. Until data is received, the statements about −30% cost / −20% turns / −35% uncached input remain a hypothesis, as PRD explicitly acknowledges.

## Next architectural stage: HZR MCP

After stabilization and publication of 0.2.0, HZR-owned MCP gateway is planned. He must reuse canonical HZR Index, Memory, execution, codec, policy and ledger; creation of parallel indexes, memory stores or daemon owners is prohibited. Claude and Codex should connect to the same HZR entrypoint instead of running the internal engines directly. Full specification - §14.1 `PRD.md`.
