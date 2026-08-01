# HZR 0.1.0 - independent review of implementation and quality

> Historical independent review `v0.1.0`. All G1–G7 from this document are corrected in development line 0.2.0; findings are not rewritten retroactively. The implementation added regression tests and a separate current-engine identity on top of the immutable `v0.1.0` baseline.

**Review date:** 2026-07-31
**Reviewer:** independent review against [PRD.md](PRD.md) and [FORK_PARITY.md](FORK_PARITY.md)
**Method:** launching all gates yourself + end-to-end running the product from a clean data root, rather than reading reports
**Verdict:** Architecturally, the project was implemented as designed. Found **1 confirmed violation of contract PRD** and **2 significant gate integrity defects** not reflected in either `FORK_PARITY.md` or the implementing agent's report

---

## 1. Summary in one table

|Region|Status|Base|
|---|---|---|
| Fork-core contract (§1.1) |✅ confirmed|snapshot v2 verified, 516 files, hash matched|
|Architectural ownership boundaries (§5.1)|✅ confirmed|end-to-end run, no reimplementation in the runtime path|
| CLI + daemon surface (§6.8) |✅ 15/15 teams, 12/12 routes|exact match with PRD|
| Version/supply-chain pins (§8) |✅ confirmed|`engines.lock.toml` = §8 literally|
| Security/privacy (§9) |✅ confirmed|tested in live process|
| Failure modes (§10) |✅ confirmed|fail-closed reproduced|
| Rust quality gates (§12.1) |✅ green| 160 passed, 0 failed |
| Codec (§6.6) |❌ **breach of contract**|`shadow` mutates content - G1|
| Protected spans (§6.6) | 🟡 weaker than stated | 4 declared classes are not covered — G2 |
| Fork regression gate (§13) |🟡 **non-deterministic**|race in fork test - G3|
| Economic KPI (§4.2) |⚪ not measured 0/9|PRD admits this honestly|

**Main conclusion:** “does it work as designed” - yes, the kernel works exactly like that. The economics of PRD (§4.2) remain completely unproven, and this is explicitly acknowledged in PRD. But three technical points are declared as ready, but in reality they are not ready.

---

## 2. What I confirmed myself

Below is only what I reproduced myself, and did not accept from the report.

### 2.1 Quality gates and fork-core

```
cargo fmt --all --check                     → exit 0
cargo clippy --workspace ... -D warnings     → exit 0
cargo test --workspace --all-targets         → 160 passed; 0 failed
scripts/verify-fork-core.sh                  → snapshot v2 f4296ec4... verified
```

Additionally checked:

- `find fork-core/rtk -type f -o -type l` → exactly **516** files, matches `included_file_count = 516` in [SNAPSHOT.toml](fork-core/SNAPSHOT.toml);
- in `crates/` **zero** `#[allow(clippy::unwrap_used)]` / `panic` / `todo!` / `unimplemented!` - workspace lints (`unwrap_used = deny`, `panic = deny`, `unsafe_code = forbid`) are not bypassed by local escape-hatch;
- CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) covers all §12.1 gates plus `verify-fork-core.sh --test`, MSRV 1.85, `npm audit --omit=dev`, `bash -n`, and actually applies the grepai patch to the pinned commit and runs `go test ./cli`. This is stronger than PRD requires.

### 2.2 Product surface - exact match with PRD §6.8

`hzr --help` gives exactly 15 commands from PRD: `init, doctor, daemon, engines, index, search, rgai, context, memory, exec, codec, agent, savings, migrate, rtk`.

[server.rs:22-33](crates/hzr-daemon/src/server.rs:22) gives exactly 12 routes from PRD, without extra ones and without missing ones. There are no discrepancies with §6.8 in either direction.

`engines.lock.toml` matches table §8 in literally all 6 components, including `rtk-upstream-reference` with `runtime = false` and `source_kind = "reference-only"` - that is, the prohibition “stock RTK as fallback” is expressed in machine-verifiable form, and not just in prose.

### 2.3 End-to-end run from clean data root

Raised `hzrd` with isolated `HOME` and separate git-workspace. Works as designed:

**`hzr init`** → created canonical layout (`ledger/`, `memory/icm/`, `runtime/`, `workspaces/`, `engines/`, `config.toml`); `fork/` is created lazily on the first call. `hzrd.token` - rights `0600`.

**`hzr doctor`** - not cosmetic. Detected **real drift on the machine**: `engine_icm: expected 0.10.61, got icm 0.10.57`, and correctly recognized `ForeignSymlink` for `.grepai` pointing to a different data root. Returns **exit 1** at `healthy: false` - that is, it is suitable as a script gate.

**`hzr exec rewrite`** - the fork environment is injected exactly according to §9:

```
RTK_TEE=0
RTK_TELEMETRY_DISABLED=1
RTK_MEM_DB_PATH=<data>/fork/mem.db
RTK_DB_PATH=<data>/fork/history.db
RTK_TEE_DIR=<data>/fork/tee    RTK_AUDIT_DIR=<data>/fork/audit
PATH=<private engines>:$PATH
```

Verdict `ask:` for `cat main.rs` - that is, approval-lifecycle (exit 3) is alive, and the proposed command is not executed without an explicit decision.

**`hzr search budget`** → `main.rs [ForkRgaiAdaptive score=0.6139]` + `generation=68f4b53a...`. The search is done through fork `rgai`, the generation hash is present.

**`hzr context plan`** - the most significant result:

```
context selected=1 rejected=0 tokens=180/16000 coverage=0.25 confidence=0.50
fork-plan pipeline=graph_first_v1 candidates=1/1 tokens=180/12000
warning MemoryUnavailable: ICM version mismatch: expected 0.10.61, found 0.10.57
```

This is literally §5.2 + §10: fork IMG planner (`graph_first_v1`) worked, hard budget was marked (`180/16000`), and inaccessible ICM **degraded with an explicit warning, without dropping the code plan**. Well-designed behavior.

**`hzr savings`** → `tasks: 0, accepted: 0` and **not** returns `cost_per_accepted_task`. §4.2 is kept fairly: the product does not pretend that the savings are proven.

**Fail-closed without daemon** - `search`, `memory`, `codec` are blocked with actionable remediation; `hzr rtk -- --version` continues to work (`rtk 0.44.1-fork.1`). This is exactly the matrix of §10.

### 2.4 Contract tests §12.2 - real coverage

The names of the tests fall on §12.2 almost one-to-one, this is not a decorative suite:

|Requirement §12.2|Test|
|---|---|
| estimates never increment actual | `test_ledger_keeps_estimates_out_of_actual_totals`, `test_usage_route_records_provider_tokens_separately_from_estimates` |
| token estimates ≤ hard limit | `test_plan_never_exceeds_hard_budget`, `test_fuse_returns_only_content_selected_within_hard_limit` |
| ICM singleton/stale PID/breaker | `test_supervisor_owns_one_process_and_second_instance_attaches`, `test_supervisor_recovers_orphan_without_spawning_duplicate`, `test_breaker_opens_at_threshold_and_recovers_half_open` |
| duplicate index detection | `test_workspace_uses_git_root_and_reports_nested_indexes`, `test_managed_discovery_blocks_foreign_symlink_without_mutation` |
| stdout/stderr/exit preservation | `test_pipeline_reports_exact_exit_and_stderr_channels`, `test_pipeline_reports_exact_unix_signal` |
| daemon auth/loopback/limits | `test_all_routes_require_bearer_authentication`, `test_config_rejects_non_loopback_bind`, `test_payload_limit_reserves_json_envelope_space` |
| project-scoped memory | `test_isolate_project_memories_removes_global_and_cross_repo_records`, `test_memory_route_rejects_user_supplied_project_override` |
|migration security|6 tests `test_migrate_legacy_index_*`, including idempotent replay and refuse-on-escaping-symlink|

I would especially like to note the quality of `hzr-exec` (40 tests for 2843 LOC): shell semantics have been seriously tested - pipes, `&&/||`, heredoc, redirects, xargs, quoting, non-UTF8 argv, process-group timeout, spill to disk. This is the strongest part of the codebase.

---

## 3. Defects found

### G1 - `shadow` profile violates contract PRD §6.6 (HIGH)

PRD §6.6 states:

> `shadow` records counterfactual size **without changing delivered content**.

In fact, `shadow` **changes the delivered content and does not write anything**.

Reproduction (input - 3 paragraphs, 1st and 3rd are identical):

```bash
hzr codec compile --profile shadow --json --file dup.txt
```

```json
{
  "content": "The budget is exhausted.\n\nSecond distinct paragraph here.",
  "changed": true,
  "profile": "shadow",
  "protected_spans": []
}
```

Third paragraph **deleted**, `changed: true`.

The reason is in [lib.rs:106](crates/hzr-codec/src/lib.rs:106) short-circuit only covers `FidelityClass::Exact` and `CodecProfile::Off`:

```rust
if fidelity == FidelityClass::Exact || profile == CodecProfile::Off {
```

`Shadow` in `transform()` is not processed at all and fails in `deduplicate_paragraphs`. In the entire workspace `Shadow` occurs 4 times, and not a single one in `transform()`. Search for `counterfactual|shadow_size|shadow_bytes` gives **zero** matches: counterfactual accounting is not implemented.

It slipped because **none of the 5 codec tests cover `Shadow`**.

The minimum fix is ​​to add `Shadow` to the short-circuit and either implement a counterfactual size entry, or remove the statement from §6.6:

```rust
// before
if fidelity == FidelityClass::Exact || profile == CodecProfile::Off {
// required
if fidelity == FidelityClass::Exact
    || matches!(profile, CodecProfile::Off | CodecProfile::Shadow)
{
```

### G2 - protected spans do not cover 4 declared classes (MEDIUM, latent)

PRD §6.6 states coverage: “code fences, inline code, **paths**, URLs, flags, hashes, versions, **identifiers**, **enum-like values** and **structured payloads**.”

Examination:

```bash
hzr codec compile --json --file sp.txt
# input: Edit src/main.rs and set MAX_RETRIES for handle_budget_overflow with {"k":1}.
```

```json
{ "changed": false, "protected_spans": [] }
```

**None** protected span. Not covered:

- **relative path** `src/main.rs` - the regex in [lib.rs:9](crates/hzr-codec/src/lib.rs:9) requires the prefix `./`, `../` or `/`, so the usual path notation in prose is not protected;
- **enum-like** `MAX_RETRIES`;
- **identifier** `handle_budget_overflow`;
- **structured payload** `{"k":1}`.

Now this is a **latent** risk: the only transformation works on the border of paragraphs and physically cannot corrupt the identifier inside the paragraph. But gate §12.2 “protected spans survive codec byte-for-byte” has been proven only on the subset that the regex catches. Any future proposal-level transformation will immediately receive unprotected identifiers and paths.

### G3 - fork regression gate non-deterministic (MEDIUM, integrity gate)

`FORK_PARITY.md` marks the “Fork regression suite” as ✅, the implementing agent report says “1699 passed, 1 ignored”. In fact, the suite has a test with race on the general state.

Diagnostics - deterministic, not “blinking”:

```
cargo test --locked --bin rtk tracking::tests                    → FAILED 3/3 (10 passed; 1 failed)
cargo test --locked --bin rtk tracking::tests -- --test-threads=1 → ok      (11 passed)
```

Full suite:

```
full-suite-run1..3: FAILED. 1698 passed; 1 failed; 1 ignored
```

One run of `scripts/verify-fork-core.sh --test` **passed**, while another failed. Therefore, “1699 passed” describes one successful run, not a reliable gate property.

Root - [fork-core/rtk/src/tracking.rs:1334](fork-core/rtk/src/tracking.rs:1334):

```rust
timer.track("test cmd", "rtk test", "raw input data", "filtered");
let recent = tracker.get_recent(5).expect("Failed to get recent");
assert!(recent.iter().any(|r| r.rtk_cmd == "rtk test"));
```

The test writes a record to the **general** tracker database and reads the window only from the **5** latest records. Neighboring `tracking::tests` write their notes in parallel and push `rtk test` out of the window. Isolating `RTK_DB_PATH` does not help - there is a race within the test module itself.

This is an inherited defect `heAdz0r/rtk`, and not an introduced one HZR. But it makes release-gate §13 (“goes through the entire fork regression suite”) non-deterministic. Fixes in ascending order of correctness: `--test-threads=1` for the module, per-test `RTK_DB_PATH`, or a unique command name + filter instead of a window of 5.

### G4 - “adaptive codec” in §4.1/§6.6 overpromised (LOW, documentation accuracy)

`hzr-codec` - 261 LOC and 5 tests. Essentially this is: density contract selector (3 options) + removal of **exact duplicate paragraphs** + protected spans validator. There is no other text compression.

This is not an implementation defect - §5.2 clause 8 and `FORK_PARITY.md` describe the mechanics correctly (density is specified by the contract before generation, codec is a separate explicit transform). But the wording of §4.1 (“adaptive codec with protected spans”) and §6.6 gives the impression of a presentational codec, which is not present in 0.1.0. It is worth bringing §6.6 into line with the facts.

Side note: codec **cuts off the trailing `\n`** (`"...here."` instead of `"...here.\n"`). For `LosslessStructural` this is debatable.

### G5 - Caveman-bridge invariants are checked by text search (LOW)

[hzr-agent/src/lib.rs:21](crates/hzr-agent/src/lib.rs:21) `test_bridge_contains_fail_closed_ownership_invariants` are 23 assertions of the type `BRIDGE.contains("session.setRepomapEnabled(false)")` based on the source code `bridge.mjs`.

It checks **the presence of a line in the file**, and not that the call occurred, and not that it occurred **before the first prompt** - whereas §6.7 requires exactly “disabled **before first prompt** and rechecked throughout generation”. The order of calls is not provable by such a test; refactoring, which will save the line but move the call after prompt, will pass gate.

Mitigating: `tests/fake_bridge.rs` gives real process run (timeout, termination of descendants, JSONL-capture), and `installManagedToolGuard` + `beforeToolCall` give runtime protection. But it is the order “until the first prompt” that is covered only textually. `FORK_PARITY.md` flags 🟡 for `cavemem --version` probe, but the weakness of the probe method is not flagged.

### G6 - duplicate suffix in error message (COSMETICS)

```
hzr: failed to read daemon token <path>: No such file or directory (os error 2); run `hzr daemon serve`: No such file or directory (os error 2)
```

The reason for the error was printed twice - an extra `anyhow` context above an already formatted error.

### G7 - inconsistent lock file rights (INFO)

`hzrd.token` → `0600` ✅, `hzrd.lock` → `0600`, but `hzrd.token.lock` → `0644`. There is no secret in the lock file, so this is not a vulnerability, but §9 (“runtime secrets have private permissions”) reads as a single rule for the entire `runtime/`.

---

## 4. Code quality assessment

**Strengths**

- **Fail-closed by default, consistently.** Version drift, foreign symlink, absence of daemon, inaccessible ICM - each case gives a typed failure or an explicit warning, but never a silent fallback. This is the most valuable property of the project, and it really lasts in runtime, and not just in tests.
- **Ownership boundaries are respected.** I specifically looked for “simplified re-implementation” in the runtime path (this is the main prohibition of §1.1 p. 3) and did not find it: search goes to fork `rgai`, plan - to fork `memory plan`, exec - to fork `rewrite`. `hzr-exec` is really an adapter, and not its own rewrite table.
- **The separation of actual from estimated** is carried out at the protocol and database level, and not by agreement - and is covered by tests on both sides (ledger + route).
- **Migration code is noticeably more careful than average:** full-SHA backup, two immutable manifests, idempotent replay, failure with escaping-symlink and active writer. Not a single automatic destructive operation.
- **Workspace lints** (`unsafe_code = forbid`, `unwrap_used/panic/todo = deny`) are not bypassed by any local `allow`.

**Weaknesses**

- **Coating skew.** `hzr-exec` — 40 tests for 2843 LOC; `hzr-codec` - 5 tests on 261 LOC, and this is where the contract violation was found (G1). The tests covered those places where the authors expected a risk, and not those where the strongest guarantees were stated.
- **Checking invariants of external JS runtime using text search** (G5) is the most fragile part of the gate: it breaks silently.
- **Documentation is ahead of code in some places** (G1, G2, G4). `FORK_PARITY.md` is generally honest and marks 🟡/⚪ where there are boundaries, but three points are marked ready prematurely.

**Economic part.** Out of 9 product metrics §4.2 measured **0**. This is recognized directly and consistently in PRD and `FORK_PARITY.md` (`hzr savings` does not report `cost_per_accepted_task`, the UI does not show the forecast as proven savings). There are no complaints here - but it is now impossible to say that “the designed savings have been achieved” in any direction: -30% cost / -20% turns / -35% uncached input remain a hypothesis until the paired benchmark.

---

## 5. Priority recommendations

**Blockers for 0.1.1**

1. **G1** — fix `Shadow` in `transform()` (one line) and add a test to the profile; either implement counterfactual accounting or remove this statement from §6.6. The documented behavior is now inverted.
2. **G3** — make the fork regression gate deterministic. As long as it goes through once, the line “goes through the entire fork regression suite” in §13 has no effect.

**Important, but not blocking**

3. **G2** - extend the protected regex to relative paths, `UPPER_SNAKE`, identifiers and JSON-like payloads **before** any clause-level transformation appears.
4. **G5** - add a runtime assertion of the order “disabled before first prompt” (for example, a call counter in fake-bridge), and not just text search.
5. **G4** - bring §6.6 into line with actual codec functionality.

**Hygiene**

6. G6 (double error suffix), G7 (permissions `hzrd.token.lock`), trailing-newline in codec.

**The next stage essentially** is exactly what is already written in PRD §14: paired baseline-vs-HZR benchmark. All functional prerequisites for it are ready; it is he who turns §4.2 from a hypothesis into a result.

---

## 6. Answer to original question

> Does it work as I designed it to?

**Yes - in terms of architecture and invariants.** I checked not the reports, but the product: fork-core is imported in its entirety and is verified by hash, ownership boundaries are maintained in runtime, fail-closed works on real version drift, context plan honors hard budget and degrades with warning instead of silent failure, actual/estimated do not mix, `savings` does not pretend. The designed discipline of “single owner per concern” is actually implemented, not imitated.

**Three things are declared ready prematurely:** `shadow` profile behaves opposite to the specification (G1, proven by launch), fork regression gate runs non-deterministically (G3, proven by 3/3 crashes), protected spans are narrower than stated (G2). None of them undermine the architecture - all three are local and can be fixed with small edits.

**The PRD's economic goal is entirely unverified** — 0 of 9 metrics in §4.2. The documents state this honestly. This missing evidence, rather than an implementation defect, remains the release's principal open issue.
