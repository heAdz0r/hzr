# Reddit feedback backlog — 2026-08-27

Ideas from the r/ClaudeCode, r/codex and r/AI_Agents threads that survive our own
guidelines (measure, never assume; no claim the ledger cannot support). Kept here so
they are not lost in a comment thread.

## 1. Live degradation signal at the prompt boundary

Source: r/ClaudeCode, fail-open thread.

Today a daemon that dies mid-session is visible only in `hzr doctor` and the end-of-session
card. The accounting state is already known at the moment it flips; surface it at the next
prompt boundary and in the status line instead of at the end. Hook-side change, not a
supervision rewrite. Explicitly *not* doing: making `hzrd` fault tolerant for this
(k8s-native territory, wrong layer).

## 2. Paired provider-billed benchmark, prefix-cache aware

Source: r/AI_Agents, prefix-cache comment.

Byte-derived counts cannot see a cache miss. Run the same 14 cases on provider-reported
billed input tokens, twice: filter firing mid-session vs at a turn boundary. Publish the
result including the case where cached raw beats filtered.

## 3. Filter placement is part of the contract

Follows from 2. Where the filter fires (turn boundary vs mid-turn) trades prefix-cache
stability against delivered bytes. Should be a stated policy with a measurement behind it,
not an implementation detail.

## 4. Declared must-keep contract per command family

Source: r/AI_Agents, non-droppable outputs.

Failures, assertion text with `file:line`, exit status and totals are always kept today, but
that set is implicit in codec code. Make it a declared per-family contract with tests that
fail if a must-keep field becomes droppable, so a new codec cannot quietly widen what may
be dropped.

## 5. Cross-workspace rollup and recovery-pull rate per family

Source: r/codex, "is this pattern happening elsewhere" comment.

`hzr stats` answers per workspace. Two gaps:

- a rollup across registered workspaces (same privacy-safe aggregates) so a family that
  leaks everywhere is visible as one number, with a trend over the window;
- recovery pulls and raw-fidelity requests reported per family. The re-run tax already
  measures repeats; a family that keeps forcing exact recovery is the same signal one step
  earlier, and it is the closest honest proxy for "the model worked around the filter".

## Already implemented — do not re-file

- Re-run tax: `hzr-core/src/ledger.rs::filter_induced_reruns` — same `command_hash` in the
  same `session_hash` within `RERUN_DETECTION_WINDOW_OPERATIONS` (8) of a filtered run,
  printed as its own line plus net-after-rerun-tax.
- Per-command-family savings: `CommandSavings` in `hzr-cli/src/stats.rs` with baseline,
  delivered, gross, regression, net, average ratio and time; rendered as TOP OPERATION
  ROUTES / OPERATION FAMILIES / OPERATION MODES.
- Raw vs filtered needs no second run: `fork-core/rtk/src/tracking.rs` records
  `baseline_tokens` from the full captured output and `delivered_tokens` from the payload in
  the same receipt. Content rewriting (response codec) has the `shadow` profile for the
  counterfactual.
