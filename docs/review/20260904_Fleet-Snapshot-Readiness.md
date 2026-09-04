# Fixed-window fleet evidence and readiness — 2026-09-04

Implementation scope: approved P1 W10 / F14 / F07. Source-only changes; no production state, installed hooks or daemon processes were changed.

## Fleet snapshot

```sh
hzr stats --fleet --since 7d --export fleet.json
hzr stats --fleet --since-unix 1788472800 --until 1788559200 --json
hzr stats --fleet --since-unix 1788472800 --until 1788559200 --project-id <sha256:64-digit-ledger-id> --json
```

The relative duration is anchored to the chosen end time once. The exact interval is `[since, until)`, in Unix seconds, and is emitted in every snapshot. `--until`, `--since-unix`, `--project-id` and `--export` require `--fleet`. The existing regular stats JSON contract remains unchanged.

The ledger is opened read-only without migrations. All fleet command, provider receipt and task aggregates share one SQLite read transaction. An absent ledger yields an explicitly absent snapshot and is not created. The ordinary `stats_collection` path now also shares a read transaction across its sections.

A single registry enumeration adds registered zero-activity projects. Deleted workspace directories remain selectable by their historical ledger IDs. Unregistered historical project IDs remain visible. Every operation belongs to its exact recorded project, so ancestor scopes do not double-count fleet totals. Registry metadata is sampled separately; it is not falsely presented as part of the database transaction.

The snapshot includes totals and grouping by project, host, family, and project/host/family/route. Host means the recorded agent ecosystem, such as Codex or Claude Code. The existing ledger does not identify physical machines, and the export explicitly states this limitation.

No command arguments, source text, memory contents, raw sessions, raw agent identifiers or workspace paths are exported. Command families and agent ecosystems pass through existing allowlists. Project IDs are validated hashes; missing attribution uses `unscoped`. `--export` writes owner-only JSON atomically; terminal output remains bounded.

An expression index supports the fixed timestamp range. Repeated-after-filter detection uses SQL window functions rather than a correlated repeated scan. It reports a bounded association with previous filtered commands, not a causal claim or a task-success result.

Measured token aggregates exclude native-unaccounted and final-delivery/control-plane rows from the savings ratio. Bypass and write operations receive no invented savings credit. Excluded legacy rows, excluded stages, native rows and unmeasured rows remain visible. Provider receipts and externally verified receipt counts remain separate. Provider task counts are `null` for historical-ID selection because their older table lacks the same project-hash column; unavailable data is not reported as zero.

`economic_claim_ready` stays false. Complete host delivery/input coverage and matched task-quality/causal evaluation remain unproven. A fleet export is auditable local measurement, not proof of provider-billed savings.

## Readiness

Doctor retains its compatibility `healthy` field (no Error checks) and adds independent dimensions:

- installation: installation/configuration checks;
- process: authenticated daemon and detected ownership;
- retrieval: index artifacts, semantic runtime and memory engine;
- accounting: known receipt-gap completeness;
- host delivery: actual replacement confirmation, independent of instructions.

Each dimension reports Ready, Degraded or Unknown, its supporting check names and its limited meaning. A missing probe stays Unknown. Warnings prevent the affected dimension from becoming Ready. Accounting completeness that cannot be established is no longer rendered as a pass merely because no specific missing-operation count was available.

The renderer states explicitly that the compatibility health flag is not proof of complete readiness or savings. Fleet reconciliation refreshes the readiness summary after adding its final check.

## Files and verification

Canonical implementation:
- `crates/hzr-core/src/ledger/fleet.rs`, `ledger/fleet/tests.rs`;
- `crates/hzr-core/src/ledger.rs`: module export, timestamp index, ordinary stats transaction;
- `crates/hzr-core/src/lib.rs`: typed public exports;
- `crates/hzr-cli/src/fleet_stats.rs`, `cli.rs`, stats dispatch in `main.rs`;
- `crates/hzr-cli/src/stats_output.rs`, `cli_subcommand_help.rs`;
- `crates/hzr-cli/src/diagnostics/readiness.rs`, `diagnostics.rs`, doctor renderer/fixture in `output.rs`.

Focused tests cover exact time boundaries, reconciliation of all dimensions, deleted/historical IDs, registry-only zero rows, secret redaction, owner-only atomic export, concurrent-writer snapshot isolation, absent-ledger behavior, invalid windows/IDs, CLI flag compatibility and warning/missing-probe readiness.

Source formatting and `git diff --check` passed. Compilation and tests were deliberately not launched in parallel with the coordinator's centralized Cargo gate. The tests above are authored, not reported as passed; the final gate result must be recorded by the coordinator.

No protocol or dashboard fields were removed or reinterpreted. No generated distribution mirrors were changed.

## Compatibility review after implementation

The fleet project-ID validator now follows the actual existing `privacy_identity_hash` representation, `sha256:<64 lowercase hexadecimal digits>`. The fleet regression creates IDs with that production helper and checks distinct project totals, historical-ID selection and registry enrichment; it would detect accidental collapse into `unscoped`.

The new fleet schema is a separate CLI/export contract. No dashboard endpoint was changed to return it. Existing `DashboardLocalActivity`, `DashboardProviderReceipts` and `DashboardSessionRoi` remain structurally unchanged, and the existing `StatsReport` remains unchanged. The visualizer's evidence overview labels estimates, unknown coverage and unestablished whole-task savings explicitly.

MCP `hzr_doctor` serializes `DoctorReport` directly, so readiness is additive there without replacing existing keys or relaxing its workspace binding. Doctor text output adds readiness before the existing check list. A fleet-reconciliation result refreshes readiness after adding its completion check.

Limit: the current dashboard separately calls `summaries_read_only`, `project_activity_read_only` and `session_roi_read_only`; these can observe different ledger moments. The new atomic fleet guarantee applies to fleet snapshots and the ordinary `stats_collection` transaction, not the dashboard's whole response. A UI that needs reproducible fleet comparisons must explicitly consume the new snapshot contract in a future API integration rather than assume its existing polling endpoint has changed.

