# PRD — HZR 0.6.4: Truthful stats surface and visible economics

Status: **approved 2026-08-26 — implemented; all gates green (see §0)**
Owner: HZR control plane
Target release: 0.6.4
Predecessor evidence: `docs/PRD_HZR_HONEST_ACCOUNTING_AND_BOUNDS.md`, `docs/PRD_HZR_TOKEN_ECONOMY_UTILIZATION.md`

---

## 0. Implementation status — 2026-08-27

Verified locally against this repository's live ledger.

`cargo fmt --check` clean · `cargo clippy --workspace --all-targets -D warnings` clean ·
`cargo test --workspace` **0 failures** · `./scripts/verify-fork-core.sh --test` exit 0.

The seven failures this tree carried at the start were not pre-existing drift to be tolerated:
they were one real defect, diagnosed and fixed (see the note at the end of §0).

Changing `fork-core/rtk` required `./scripts/refresh-current-engine.sh` and re-blessing the
Clippy ratchet in `scripts/verify-fork-clippy.py`. Both digests were **computed by the tooling**,
never transcribed from a message.

### Landed

| Req | What shipped | Evidence |
|---|---|---|
| R1 | Typed `Column`/`Cell`/`write_row` renderer; every cell truncated through its declared column; `Styled` cells carry ANSI without entering the width computation; all five tables migrated | `acceptance_gate_no_cell_can_exceed_its_column` feeds every table its longest possible value |
| R2 | `ZeroReductionCause` classification; in-panel scope-artifact disclosure; policy version + version scope + excluded count in `ACCOUNTING COVERAGE`, with the literal recovery command | `acceptance_gate_excluded_history_is_disclosed_with_its_recovery`, `..._a_zero_credit_scope_is_distinguished_from_excluded_history` |
| R3 | `stage_excluded_operations` / `..._delivered_tokens_estimated` on `EfficiencySummary`; `RAT` column on `OPERATION MODES`; stage legend; `total_observed_operations` doc corrected; stable short stage labels replacing mid-word ellipsis | `acceptance_gate_stage_excluded_rows_are_reported`, `..._mode_panel_is_stage_aware_and_bounded` |
| R4 | `ECONOMICS` block above the headline, two scopes always rendered; `Ledger::economic_scope_summary`; `net_avoided_tokens_scoped` sharing the headline's own SQL expression; `EconomicScopeSummary` alias; `POTENTIAL COST` removed and subsumed; `unavailable` + enable steps inline; `not measured` never a currency zero | `acceptance_gate_economics_renders_both_scopes_without_summing_evidence`, `..._unavailable_pricing_names_the_steps_that_enable_it` |
| R5 | Row conservation made total: `excluded_legacy_operations` no longer carries a stage filter, so the three counters partition every recorded row | `acceptance_gate_every_recorded_row_belongs_to_exactly_one_counter` |
| R6.1–R6.3 | `HostPermissionMode`, `HostExecutionGrant`, `HostGrantRejection` in `hzr-protocol`; `reconcile_host_grant` moved to `hzr-exec` beside `RewriteDecision`; `attach_host_grant` exports the grant onto the approved command | `acceptance_gate_a_grant_is_bound_to_its_session_and_its_moment` |
| R6.4 | Fail-closed on all four axes: mode, session digest, TTL (12 h), future skew (5 min); a malformed grant is refused, never partially trusted | same gate, one assertion per rejection cause |
| R6.5 | Deny survives a grant on every surface | preserved in `reconcile_host_grant` |
| — | **Bonus, same defect class:** `hzr exec run` and both MCP call sites read session identity from `CODEX_THREAD_ID`/`CLAUDE_SESSION_ID` only, ignoring the `HZR_SESSION_ID` the hook exports — so operations started *by* an approved command recorded no session. New `hzr_core::ambient_session_id` reads `HZR_SESSION_ID` first | `crates/hzr-core/src/host_grant.rs` |

Live verification, this repository:

```
hzr stats                          →  0 avoided, 0.0%  + "0.0% is a scope artifact, not a measurement."
                                                        + "76.8K operation(s) … recover them with:
                                                           hzr stats --accounting-version all"
hzr stats --accounting-version all →  252.66M avoided, 64.0%
ECONOMICS  this project      68.94M   USD  344.70   not measured
           global lifetime  252.66M   USD 1263.28   not measured
```

### Landed — approved backlog (2026-08-27)

Five items were approved after the initial scope. Four are complete; the fifth is complete as a
harness and cannot produce numbers without live provider runs.

| Item | Req | What shipped | Evidence |
|---|---|---|---|
| 1 — live degradation signal | R7 | Edge-triggered `systemMessage` on the tool call that detects the flip, a `hzr hooks statusline` fragment that holds `ACCOUNTING: DEGRADED` while true, a once-per-recovery notice, and — added in this pass — `additionalContext` injection at the **prompt boundary**, which is where the operator actually reads | `acceptance_gate_accounting_transition_notices_are_edge_triggered`, `..._degraded_scorecard_withholds_partial_totals` |
| 2 — paired billed-input benchmark | R8 | `benchmarks/hzr-billed-input-prefix-cache-v0.6.4/`: the same 14 cases, metric changed from delivered bytes to **provider-billed input** with cache-read/cache-write reported separately, run across both placement arms. Refuses to emit a comparison without a paired receipt for every case in both arms, lists every unmeasured case with its reason, and exits non-zero | smoke-run against this repo returns `status: not_measured` with all 28 cases itemized — the intended behaviour, not a failure |
| 3 — cache-aware filter placement | R9 | `FilterPlacement { Anywhere, TurnBoundary }` as a first-class policy dimension in `[policy] filter_placement`; turn position tracked in session state; a mid-turn filter is declined under `TurnBoundary` and the forgone reduction counted in `placement_deferred_operations`. Default is the shipped behaviour, so arm A is also a regression check | `acceptance_gate_filter_placement_defers_mid_turn_only_when_policy_says_so` |
| 4 — must-keep contract | R10 | `MustKeep`, `CompletenessContract` and an exhaustive `COMPLETENESS_CONTRACTS` table in `hzr-protocol`, plus executable proof in `fork-core/rtk/tests/must_keep_contract.rs` that runs the real routes against output carrying every undroppable class | structural gates + 4 executable gates |
| 5 — filter-induced re-run detection | R11 | `filter_induced_rerun_operations` / `..._tokens_estimated` on `EfficiencySummary`, bounded to `RERUN_DETECTION_WINDOW_OPERATIONS`, de-duplicated via `EXISTS`, rendered as a `RERUN TAX` line that states the pessimistic net alongside the headline | `acceptance_gate_rerun_tax_is_bounded_deduplicated_and_filter_scoped`, `..._rerun_tax_is_measured_rather_than_assumed` |

**Two real defects were found by the new gates, not by inspection:**

1. `rtk summary` returned exit `0` for a child that exited `3`. A filter that turns a red run
   green is worse than no filter, because the summary is believed. `must_keep_exit_status_…`
   caught it; `fork-core/rtk/src/summary.rs` now propagates the child's status like `run_test`
   and `run_err` always did.
2. The `write` route's contract listed `ExitStatus` and `ChangedFiles` but not `Failures`, so a
   refused patch could be presented as a completed one. `acceptance_gate_failure_capable_routes_…`
   rejected the table until it was fixed.

**A third, unrelated to the backlog, was fixed in passing:** the seven pre-existing test failures
in this tree were one defect of the same family. `wrap_prose` wraps managed prose *after*
interpolation, which is correct — but `referenced_contract` parsed the contract path per line and
expected the phrase and the quoted path on the same line. A long install path put them on
different lines, so `hzr doctor` reported no contract at all: a presentation layer silently
destroying a machine-readable reference. The parser now matches across the wrap.

### Remaining

| Req | Work | Note |
|---|---|---|
| R8 | **Numbers** for the paired billed-input benchmark | The harness is complete and refuses to fabricate. Producing figures needs real provider-billed runs in both arms; no local substitute exists, and a modelled billed-input number is exactly what the benchmark replaces. |
| R9 | A recorded per-operation `filter_placement` dimension on the ledger row | The policy dimension and its deferral counter ship; attributing an individual historical row to an arm needs a new column, which R8's analysis will want and nothing yet reads. |

One local config change was made to verify R4 end to end: `[billing]` in the operator's
`config.toml` was switched to `public_estimate_enabled = true` /
`claude_code / anthropic / claude-opus-5 / standard / input`. The previous file is preserved at
`config.toml.pre064.bak`.

---

## 1. Problem statement

After upgrading to 0.6.3, `hzr stats` on a workspace with 76,725 recorded operations
renders a headline of **0 tokens avoided / 0.0%**, a bypass panel claiming 26.4% of
delivered tokens received zero filtering, a mode table whose columns visibly break
alignment, and a `POTENTIAL COST` section that says only `unavailable`.

Every one of those numbers is individually defensible. Together they tell the operator
that HZR stopped working. It did not. The stats surface is lying by omission.

Separately, the same operator running Claude Code in `bypassPermissions` is still being
prompted for approval — because HZR evaluates one command twice, in two processes, and the
second evaluation cannot see that the first already accepted the host's grant (§6). Both
problems share a root shape: **state that exists is not carried to the surface that needs
it.**

### 1.1 Observed output (verbatim, 2026-08-26, HZR 0.6.3)

```
TOOL OUTPUT BEFORE    DELIVERED TO MODEL    OPERATIONS
33.4K                 33.4K                 9
0 TOKENS AVOIDED                                 0.0% of tool output
```

```
│ context  │ context_plan      │ standalone_delive… │      5 │     24.5K │
│ observability │ observability_sn… │ control_plane      │      1 │       150 │
│ search   │ search_exact      │ final_delivery     │    201 │     37.3K │
```

Two things are wrong on the face of it: the `observability` row is 5 characters wider
than its column and shifts every column after it; and a panel reporting 201 search calls
sits directly beneath a headline claiming 9 operations total.

---

## 2. Evidence — what the ledger actually contains

All figures below were read from the live ledger of `/Users/andrew/Programming/hzr`
on 2026-08-26 via `hzr stats --json`. No number in this section is inferred.

| Query | Operations | Baseline tokens | Delivered tokens | Net avoided | Reduction |
|---|---:|---:|---:|---:|---:|
| `hzr stats` (default, `--accounting-version current`) | 10 | 41,251 | 41,251 | **0** | **0.0%** |
| `hzr stats --accounting-version all` | 73,300 | 394,485,063 | 141,830,187 | **252,654,876** | **64.0%** |
| `hzr stats --workspace .` (default scope) | 7 | 22,596 | 22,596 | 0 | 0.0% |

`excluded_legacy_operations` in the default global view: **76,682**.
`excluded_legacy_operations` in the default project view: **26,691**.

**252.65 million avoided tokens exist in the ledger and the default view renders `0`.**

### 2.1 Root cause A — a silent accounting-policy version bump

`crates/hzr-core/src/ledger.rs:28`

```rust
pub const CURRENT_ACCOUNTING_POLICY_VERSION: &str = "privacy_typed_v2";
```

`accounting_policy_predicate` (`ledger.rs:749`) filters every efficiency, bypass, family,
mode, coverage and read-pipeline query to that exact string unless
`--accounting-version all` is passed. 0.6.3 moved the constant from `privacy_typed_v1`
to `privacy_typed_v2`, so on upgrade the entire recorded history left the default view in
one step.

`EfficiencySummary.excluded_legacy_operations` is computed for exactly this reason
(`ledger.rs:2561`), is carried into `StatsReport` (`crates/hzr-cli/src/stats.rs:443`), and is
serialized into `hzr stats --json`.

**`crates/hzr-cli/src/stats_output.rs` never renders it.** Neither
`excluded_legacy_operations`, nor `accounting_version_scope`, nor
`accounting_policy_version` appears anywhere in the terminal output. The daemon surface
does disclose it (`crates/hzr-daemon/src/api.rs:453`); the CLI — the surface an operator
actually reads — is the only one that stays silent.

That is the defect. A view may narrow its scope. It may not narrow its scope by 76,682
operations and print `0.0%` as though that were a measurement.

### 2.2 Root cause B — the surviving rows are zero-credit by policy

Ten rows carry `privacy_typed_v2`. Every one of them is, by construction, incapable of
showing a reduction:

| Rows | Mode | Stage | Why savings are structurally zero |
|---:|---|---|---|
| 5 | `context_plan` | `standalone_delivery` | Generative operation; `baseline == delivered`. Policy grants no credit without a measured counterfactual. |
| 1 | `memory_recall` | `standalone_delivery` | Same. |
| 4 | `native` | (bypass) | A bypassed operation delivers what it consumed, by definition. |

So the default headline is `0` for two independent reasons stacked on top of each other,
and the renderer explains neither.

### 2.3 Root cause C — two panels, two undisclosed denominators

`efficiency_summary_scoped` (`ledger.rs:2351`) and every coverage query exclude
`accounting_stage IN ('final_delivery', 'control_plane')`.
`operation_modes_summary` (`ledger.rs:2578`) applies **no stage filter at all**.

Result, from the same snapshot:

- headline: 10 operations
- `OPERATION MODES`: 205 further operations (202 `search_exact`, 2 `search_semantic`,
  1 `observability_snapshot`) that the headline cannot see

Both queries are correct. Neither panel says which denominator it used. This is the
"неконсистентность данных" the operator perceived, and the perception is justified.

The stage exclusion itself is deliberate and stays: `final_delivery` rows exist to make a
delivery stage visible without double-counting the `internal_transport` row that measured
it (gate `acceptance_gate_final_delivery_is_stage_visible_but_not_double_counted`,
`ledger.rs:4618`). The exclusion is right; hiding it is not.

A related contradiction ships today: `EfficiencySummary.total_observed_operations` is
documented as *"Every row observed across measured, unmeasured, and host-native channels"*
(`ledger.rs:73`) while its query excludes two stages. `ACCOUNTING COVERAGE` therefore
prints `covers 10 of 10 observed operations (100.0%)` while 205 observed operations sit
outside the denominator.

### 2.4 Root cause D — column cells are not bounded by their column width

`write_operation_modes` (`stats_output.rs:170`) formats the family cell as `{:<8}` and
never truncates it:

```rust
mode.operation.as_str(),          // NOT truncated — "observability" is 13 chars
truncate(mode.mode.as_str(), 17), // truncated
truncate(mode.stage.as_str(), 18) // truncated
```

`write_subsystems` (`stats_output.rs:479`) has the identical defect on
`subsystem.subsystem`. The `assert_aligned` helper (`stats_output.rs:852`) would catch
both, but no fixture in the suite renders a family or subsystem name long enough to
overflow. The gate exists; the fixture does not.

### 2.5 Root cause E — money is unreachable in practice

`POTENTIAL COST` (`stats_output.rs:53`) prices exactly one input:
`report.direct_savings.net_avoided_tokens_estimated` (`stats.rs:237`). In the default
scope that value is `0`, so even with `public_estimate_enabled = true` the section would
report `USD 0.000000` — a correct arithmetic result on a scope-mutilated input.

There is no project-vs-global money view at all. `SessionEconomicSummary`
(`billing.rs:191`) is session-scoped and reachable only from the `Stop` hook and the
dashboard; `provider_economic_receipts` already carries `project_hash` and is already
indexed by `(project_hash, observed_at_ms)` (`ledger.rs:1429`), but nothing aggregates it
per project or globally.

For scale: 252,654,876 avoided input tokens priced against the catalog entry
`claude_code/anthropic/claude-opus-5` (`input = $5.00 / 1M`) is a preliminary potential of
**≈ USD 1,263.27**. That is the number the product exists to show, and today it shows
`unavailable`.

---

## 3. Goals

**G1.** No stats panel may render a cell wider than its column, for any input.
**G2.** A default view that excludes recorded history must say so, with the exact recovery
command, on the same screen as the headline.
**G3.** Two panels shown together must be reconcilable: if they use different denominators,
the output must say which and why.
**G4.** Estimated monetary value must be visible per project and globally, in one block, at
the top of the output.
**G5.** Nothing above may weaken the estimated/actual separation that 0.6.3 established.
**G6.** One intent yields one policy verdict. Two HZR processes evaluating the same command
must never reach different decisions because one of them knows less than the other.

### Non-goals

- Changing what counts as a saving. The `final_delivery` / `control_plane` exclusion, the
  zero-credit rule for generative operations, and the bypass-cancels-out rule are correct
  and stay exactly as they are. 0.6.4 discloses them; it does not re-score them.
- Migrating `privacy_typed_v1` rows into `v2`. The version boundary is real evidence about
  what producer wrote a row. It is disclosed, not erased.
- Promoting a public-list estimate to invoice truth. It stays labelled preliminary and
  potential, in its own column, never summed with a receipt.

---

## 4. Requirements

### R1 — Column-safe table rendering  *(answers operator item 1)*

R1.1 Introduce a single row writer in `stats_output.rs`. A column is declared once as
`(width, alignment)`; the writer truncates every cell to its declared width. Adding a
column without a width becomes impossible to express.

R1.2 A pre-styled cell (the ANSI progress bar in `WHERE IT WAS AVOIDED`) declares its
display width explicitly so ANSI bytes never enter the width computation.

R1.3 Widen `OPERATION MODES` `FAMILY` to fit the longest member of the closed
`AccountingOperationKind` taxonomy (`observability`, 13 chars) rather than truncating a
short closed enum into ambiguity.

R1.4 Gate: `assert_aligned` fixtures gain a maximal-length family, subsystem, mode, stage
and command in every table, and `assert_aligned` is applied to every rendering test rather
than a subset.

### R2 — Disclose the accounting scope  *(answers operator item 2)*

R2.1 A new `ACCOUNTING SCOPE` line group renders, always:
policy version, version scope, and — when non-zero — `excluded_legacy_operations` with the
literal recovery command `hzr stats --accounting-version all`.

R2.2 When `net_avoided_tokens_estimated == 0` **and** `excluded_legacy_operations > 0`, the
headline panel carries an explicit in-panel line stating that `0.0%` is a scope artifact,
not a measurement, naming the excluded count.

R2.3 When every counted operation is a zero-credit class (generative
`standalone_delivery` and/or bypass), state that in one line. An operator must never have
to infer "the only rows in scope cannot show a saving" from a table.

### R3 — Reconcile the panels  *(answers operator item 3, first half)*

R3.1 Add `stage_excluded_operations` and `stage_excluded_delivered_tokens_estimated` to
`EfficiencySummary`, computed under the same project/window/version predicates as the
headline, counting exactly the rows the headline drops for stage reasons.

R3.2 `OPERATION MODES` gains an `IN RATIO` column: `yes` for stages inside the reduction
ratio, `no` for `final_delivery` / `control_plane`.

R3.3 `ACCOUNTING COVERAGE` states the stage-excluded count and the reason
(`delivery and control-plane stages are visible but never double-counted`).

R3.4 Correct the `total_observed_operations` doc comment to match its query, or the query
to match the doc. Chosen: correct the doc and state the stage scope in the field name's
documentation. The ratio keeps its current meaning; the field stops overclaiming.

### R4 — `ECONOMICS` block, per-project and global  *(answers operator items 3 and 4)*

R4.1 A new `ECONOMICS` panel renders **immediately after the header, above
`LOCAL OUTPUT REDUCTION`**.

R4.2 Two rows, always both present:

| Scope | Avoided tokens | Potential saved | Billed (actual) |
|---|---:|---:|---:|
| `this project` | project-scoped estimated net avoided input tokens | public-list estimate, native currency | sum of project-attributed receipts |
| `global lifetime` | global estimated net avoided input tokens | public-list estimate | sum of all receipts |

R4.3 Potential and billed occupy separate columns and are never summed. A scope with no
receipt renders `not measured`, never `0.00`.

R4.4 When pricing is opt-in-disabled or fails closed, the block renders the reason and the
exact enabling steps inline, and still renders the avoided-token column — the token figure
does not depend on pricing being configured.

R4.5 The block states the pricing identity once:
`harness / provider / model / method · basis=… · catalog=… retrieved=…`.

R4.6 New ledger surface `economic_scope_summary(project_path: Option<&str>)` aggregating
`provider_economic_receipts` by `project_hash` (or globally when `None`), reusing
`aggregate_economic_rows` so mixed-currency and partial-pair rules are inherited unchanged.

R4.7 `hzr stats` computes the project scope from the current worktree when `--workspace` is
absent, so the block is populated without extra flags. When the cwd is not a registered
worktree, the project row renders `no project scope for this directory`.

R4.8 `POTENTIAL COST` is removed as a separate section; its content is subsumed by
`ECONOMICS`. `hzr stats --json` keeps `raw_public_estimate` and gains
`economics` — no JSON field is removed.

### R5 — Verified recalculation after upgrade  *(answers operator item 3, second half)*

R5.1 Acceptance run on this repository proving the three scopes reconcile:
`hzr stats`, `hzr stats --accounting-version all`, `hzr stats --workspace <root>`.

R5.2 Property gate: for a fixed snapshot,
`headline_operations + stage_excluded_operations + excluded_legacy_operations`
equals the total row count in scope. A regression that silently drops rows fails the gate.

R5.3 Gate: a report with `excluded_legacy_operations > 0` and `reduction_pct == 0.0` must
render the R2.2 disclosure. Asserted on the rendered string.

### R6 — One intent, one verdict: propagate the host execution grant

#### 6.1 The defect is structural, not editorial

HZR evaluates the same command **twice, in two processes, with two different amounts of
information**, and the second evaluation cannot see what the first one decided.

Reproduced live on 0.6.3 (2026-08-26). One command, `permission_mode = bypassPermissions`:

**Evaluation 1 — `hzr hooks dispatch` (host-aware).** Receives `permission_mode` on stdin,
applies `honor_host_permission_mode` (`crates/hzr-cli/src/hook_runner.rs:709`, wired at
`:216`), and answers:

```json
{"permissionDecision":"allow","permissionDecisionReason":"fork-core selected tracked raw proxy execution",
 "updatedInput":{"command":"export HZR_SESSION_ID='p';\nexport HZR_INTERNAL_EVASION_JSON='{…}';\n…\nrtk proxy /bin/sh -c 'hzr exec run …'"}}
```

**Evaluation 2 — `hzr exec run`, launched *by that very approval* (host-blind).** It has no
`permission_mode`, no grant, no memory of evaluation 1. It re-derives the verdict from
scratch and refuses:

```
approval required without executable proposal: t2 ask e9 (direct HZR diagnostic access):
fork-core canonical policy requires approval; use `hzr stats` … instead of reading HZR state directly
```
→ `ExecutionOutcome::NotStarted { ApprovalRequired }`, exit 77
(`crates/hzr-cli/src/output.rs:296`).

The host approved. HZR's own child then refused the thing the host approved. That is the
desync, and it is not fixable by documentation: `hzr doctor --reconcile-fleet` rewrites the
managed instruction blocks in `CLAUDE.md` / `AGENTS.md` on every reconciliation pass, so
any note placed there is transient by design. The fix belongs in the decision path.

Note the mechanism that is *already* there and merely under-used: the hook injects typed
state into the approved command's environment — `HZR_SESSION_ID` via
`attach_session_attribution` (`hook_runner.rs:415`) and `HZR_INTERNAL_EVASION_JSON` via
`attach_hook_evasion`. Session identity propagates. The host's execution grant does not.

#### 6.2 Requirements

R6.1 **Typed carrier, not a loose string.** Introduce `HostExecutionGrant` in
`hzr-protocol`: a closed `HostPermissionMode` enum plus `granted_for_session_hash`,
`granted_at_ms`, and `source`. No policy code may branch on a raw environment string.

R6.2 **Single reconciliation authority.** Move `honor_host_permission_mode` out of
`hook_runner.rs` into a shared crate as the one function that reconciles a
`RewriteDecision` with a `HostExecutionGrant`. Today the hook owns a private copy; that
privacy *is* the bug. Every policy surface — `hzr hooks dispatch`, `hzr exec run`,
`hzr exec rewrite`, the daemon `/v1/exec/rewrite` route — calls that one function.

R6.3 **Propagation over the existing channel.** `attach_host_grant` exports the typed grant
into the approved command exactly as `attach_session_attribution` exports the session, so
every descendant process — `hzr exec run`, the pinned engine, a nested agent — inherits it
without a new transport.

R6.4 **Fail closed.** A grant is honored only when its `granted_for_session_hash` matches
the ambient `HZR_SESSION_ID` **and** `granted_at_ms` is inside a bounded TTL. A grant
copied into a shell script, committed to a repo, or left exported in a stale terminal
manufactures no standing approval. An unparseable or mismatched grant is ignored, never
trusted-by-default.

R6.5 **Deny is unaffected.** An explicit deny is a rule, not an absent one — identical to
the hook's current behavior and its existing gate
(`acceptance_gate_bypass_permissions_is_not_re_litigated`, `hook_runner.rs:2026`).

R6.6 **Accounting loses nothing.** An operation auto-approved through a grant is recorded
with the grant applied and still counts as avoidable leakage with zero savings credit.
Removing a prompt must not remove the evidence that a bypass happened.

R6.7 **Drift is a doctor check, not a doc.** `hzr doctor` gains
`host_permission_grant_propagation`, which fails when a session shows hook-allowed
operations alongside `exec run` approval-refusals for the same command hash. Because
managed prose is regenerated, the only durable place to state an invariant is a check that
can fail.

R6.8 **Wording stays in the single authority.** The E9 prescription is emitted by
`EvasionClass::prescription()` (`crates/hzr-protocol/src/api.rs`), which is already the one
wording authority; it gains the fact that this is an HZR policy decision surfaced through
the harness approval channel rather than a harness permission. No message text is authored
in a Markdown file.

---

### §6a — R7: fail-open must be loud in-session, not at the end of it

#### 7.1 What is already true

Bypass is not a silent gap today. Policy-allowed native calls land as typed E10 findings with
their own class (`e10_capability_gap` and siblings); the end-of-session card prints avoidable
leakage on its own line, separate from savings; a bypass earns zero savings credit, so it cannot
inflate the ratio; and `hzr stats --evasion` breaks it down by class. When the daemon is down the
hook fails open — the command still runs and command policy still applies through the pinned
fork-core — but that run is absent from the usage ledger, and doctor and stats report incomplete
accounting rather than folding it into savings. **An unavailable ledger already reads `unknown`,
never a reassuring zero.**

#### 7.2 The actual gap

Nothing tells the operator, in-session, when the daemon dies at minute 20. They find out from
`hzr doctor`, or from the end-of-session card — both of which arrive after the session whose
numbers are now partial. The evidence is correct and its delivery is too late to act on.

This is a **visibility** requirement, not a resilience one. Making `hzrd` fault-tolerant is an
explicit non-goal: it is one supervised local process, and real fault tolerance would be a
different architecture rather than more patches to the current supervision. 0.6.4 detects the
flip and says so immediately; it does not try to prevent it.

#### 7.3 Requirements

R7.1 **The flip is announced once, when it happens.** The first degraded rewrite in a session —
the moment accounting coverage goes from complete to unknown — emits a visible in-session notice
through the hook's own response channel, stating that the ledger is no longer recording this
session's operations and naming the recovery (`hzr daemon service status`). Once per transition,
not once per command: a notice on every call is noise that trains the operator to ignore it.

R7.2 **The state is continuously visible, not just announced.** HZR ships a status-line fragment
reporting accounting coverage for the running session, so `ACCOUNTING: DEGRADED` is on screen for
as long as it is true rather than scrolling away with the message that announced it.

R7.3 **Recovery is announced too.** The next successfully reconciled managed rewrite clears the
state and says so once. A banner that only ever appears is a banner operators learn to dismiss.

R7.4 **Degraded never reads as zero.** The in-session surfaces inherit the rule the ledger
already follows: unmeasured is `unknown`, and a session that spent time degraded says so on its
end card rather than presenting a partial total as a complete one.

R7.5 **No resilience work.** No supervision changes, no retry loops, no daemon restarts triggered
by this requirement. Detection and visibility only.

### §6b — R2.4 / R4.9: the session scorecard has the same three defects

The `Stop` scorecard reproduces, at session scope, exactly what §2 found at ledger scope:

```
Saved (estimated net): 0 tokens (0.0%; gross 0, regression 0; 47674 -> 47674)
Potential public-list savings: unavailable (opt-in disabled; not an invoice)
Measured commands: 8 | Top: native x8
Policy: prevented 30 (0 native denial); asked 21; avoidable leakage 0 ops / 0 tokens
Evidence: prevented output not estimated | top evasion e10-capability-gap | hook events 59
```

R2.4 **Explain the zero.** The card reuses `ZeroReductionCause` rather than printing a bare
`0 tokens (0.0%)`, so a session whose rows are all zero-credit says so.

R4.9 **Show the money.** The `Saved` line carries the priced value inline — estimated tokens
*and* their preliminary public-list value — instead of relegating money to a separate
`unavailable` line. The two-scope discipline of §R4 applies: potential and billed never merge.

R2.5 **Reconcile the denominators.** `Measured commands: 8` beside `hook events 59` is the same
undisclosed-denominator problem as `OPERATION MODES` beside the headline. The card states what
the 8 measures and where the other 51 went.

R2.6 **`asked 21` is a regression signal.** Under a host grant the expected count is zero; a
non-zero ask count in a granted session is direct evidence that §6 propagation failed somewhere,
and the card names it as such rather than reporting it as ordinary policy activity.

---

## 5. Acceptance gates

| # | Gate | Evidence |
|---|---|---|
| A1 | Every framed line is exactly 74 display columns for maximal-length inputs in all five tables | `assert_aligned` over new fixtures |
| A2 | `excluded_legacy_operations > 0` always renders the count and `hzr stats --accounting-version all` | rendered-string assertion |
| A3 | `reduction_pct == 0.0` with excluded history renders the scope-artifact disclosure | rendered-string assertion |
| A4 | `OPERATION MODES` marks each row in/out of the reduction ratio | rendered-string assertion |
| A5 | Row conservation: headline + stage-excluded + legacy-excluded == rows in scope | ledger property test |
| A6 | `ECONOMICS` renders both scopes with pricing disabled and enabled | two rendered-string assertions |
| A7 | Potential and billed never appear in one summed figure | rendered-string assertion |
| A8 | A scope with zero receipts renders `not measured`, never a currency zero | rendered-string assertion |
| A9 | `cargo fmt` clean, `cargo clippy -D warnings` clean, full suite green | CI |
| A10 | `./scripts/verify-fork-core.sh --test` exit 0 | CI |
| A11 | Same command + same grant yields the same verdict from the hook and from `hzr exec run` | cross-surface property test over the anti-evasion fixture |
| A12 | The live repro of §6.1 executes end to end under `bypassPermissions` instead of exiting 77 | integration test driving `hooks dispatch` → the returned command |
| A13 | A grant whose session hash mismatches, or whose age exceeds the TTL, is ignored | unit test per rejection cause |
| A14 | Deny survives a grant on every surface | extends `acceptance_gate_bypass_permissions_is_not_re_litigated` to `exec run` |
| A15 | A grant-approved bypass still books avoidable leakage with zero savings credit | ledger assertion |
| A16 | `hzr doctor` fails on a synthesized propagation-drift fixture | doctor test |
| A17 | The first degraded rewrite in a session emits exactly one in-session notice; the tenth emits none | hook response assertion |
| A18 | Recovery emits exactly one clearing notice | hook response assertion |
| A19 | A session that spent any time degraded says so on its end card and never presents a partial total as complete | scorecard assertion |
| A20 | The scorecard's `Saved` line carries the priced value inline, and potential is never summed with billed | scorecard assertion |
| A21 | A zero on the scorecard states its cause | scorecard assertion |
| A22 | A non-zero ask count in a granted session is reported as propagation failure, not as ordinary policy activity | scorecard assertion |

---

## 6. Implementation plan

| Step | File | Change |
|---|---|---|
| 1 | `crates/hzr-core/src/ledger.rs` | `stage_excluded_operations` + delivered tokens on `EfficiencySummary`; `economic_scope_summary`; doc correction on `total_observed_operations` |
| 2 | `crates/hzr-core/src/billing.rs` | no change to pricing rules; reuse `price_avoided_input_tokens` per scope |
| 3 | `crates/hzr-cli/src/stats.rs` | dual-scope collection (project + global), `EconomicsReport` assembly, scope-artifact classification |
| 4 | `crates/hzr-cli/src/stats_output.rs` | `Column`/`write_row` renderer; `ECONOMICS` panel; scope disclosure; `IN RATIO` column; remove standalone `POTENTIAL COST` |
| 5 | `crates/hzr-protocol/src/api.rs` | `HostPermissionMode`, `HostExecutionGrant`, E9 prescription wording |
| 6 | `crates/hzr-cli/src/hook_runner.rs` | `honor_host_permission_mode` moved out; `attach_host_grant` added beside `attach_session_attribution` |
| 7 | `crates/hzr-exec/src/adapter.rs`, `crates/hzr-cli/src/main.rs` | `exec run` / `exec rewrite` read and apply the grant through the shared authority |
| 8 | `crates/hzr-daemon/src/api.rs` | `/v1/exec/rewrite` accepts the grant; grant application recorded |
| 9 | `crates/hzr-cli/src/diagnostics.rs` | `host_permission_grant_propagation` doctor check |
| 10 | tests | fixtures for A1–A16 |
| 11 | `crates/hzr-cli/src/hook_runner.rs` | R7 transition notices; scorecard zero-cause, inline money, denominator reconciliation, granted-session ask signal |
| 12 | status-line fragment | R7.2 continuous coverage state |
| 13 | `CHANGELOG.md`, `RELEASE_NOTES.md`, version bump | 0.6.4 |

Steps 1–4 (stats surface) and steps 5–9 (grant propagation) are independent and can land in
either order; they share only the release.

### Version-bump checklist (inherited process facts)

- `Cargo.toml` workspace version → `0.6.4`
- `PACKAGE_LOCK_SHA256` in `crates/hzr-agent/src/preflight.rs` — **derived by build, never transcribed**
- digests in `scripts/smoke-bundle.sh` — derived, never transcribed
- `./scripts/refresh-current-engine.sh` only if `fork-core/rtk` changes (it does not here)

---

## 7. Risk

| Risk | Mitigation |
|---|---|
| Widening `FAMILY` shifts total table width past 74 | Width is rebalanced within the existing 74-column budget; A1 enforces it |
| Dual-scope collection doubles ledger queries per `hzr stats` | Both scopes read from one read-only connection in one snapshot; `stats_concurrency` test covers contention |
| `ECONOMICS` at the top pushes the reduction headline below the fold | The block is 8 lines; the headline stays on the first screen at 24 rows |
| Disclosing 76,682 excluded rows reads as an HZR failure | Wording states it as a producer-version boundary with a one-flag recovery, which is what it is |
| An exported grant becomes a standing approval an operator never revisits | R6.4 binds it to a session hash and a TTL; a stale or foreign grant is ignored, and A13 tests each rejection cause |
| Honoring the grant weakens enforcement | Enforcement is routing, not prompting. The managed replacement is still substituted, Deny still stands (R6.5), and the bypass is still booked as avoidable leakage (R6.6) |
| Moving `honor_host_permission_mode` between crates changes hook behavior | The existing gate moves with it and is extended, not replaced (A14) |
