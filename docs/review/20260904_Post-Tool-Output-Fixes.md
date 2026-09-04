# W07 supported Claude tool-output interception and exact-read policy

Date: 2026-09-04. Target source version: 0.8.0. Authorized P1 implementation.
This adds to `20260904_Host-Integration-Fixes.md`; its statement that all PostToolUse output replacement is unimplemented is superseded only for the narrow path below.

## Implemented behavior

- `crates/hzr-cli/src/host_hooks/post.rs` validates the documented Claude built-in Bash response object: string stdout/stderr and false interrupted/isImage. Every unrelated metadata member survives unchanged. Codex and unknown/event/failure/image shapes pass through.
- Explicit opt-in: `hzr hooks observe --native-mode observe --replace-output`. Default observer behavior remains unchanged. The source-generated PostToolUse matcher now includes Bash. To opt in on a Claude version supporting built-in `updatedToolOutput`, append ` --replace-output` to the existing managed observe command in its PostToolUse entry, preserving its executable and mode. HZR recognizes the resulting observer ownership. An install regeneration can restore the default observer command, so recheck this explicit opt-in after an upgrade. No actual user configuration was changed.
- Only plain `cargo test` commands and 4–128 KiB successful stdout matching the closed standard test-run grammar are eligible. Custom stdout, diagnostics, shell chains, managed HZR commands and incomplete schemas are untouched. Stderr remains exact even when it contains warnings.
- Filtering invokes the authenticated daemon's existing managed fork API with exactly `pipe --filter cargo-test` and stdin containing already produced stdout. No observed command is executed again. The API accepts no other pipe family, additional path, passthrough flag or shell text.
- Replacement uses `hookSpecificOutput.updatedToolOutput`, not duplicated `additionalContext`. Original per-suite summary lines remain exact alongside the inherited filter summary. The complete serialized replacement envelope and recovery command must beat the original response by more than 256 bytes.
- Before emitting a proposal, exact original stdout is durably saved to private `<data_dir>/hook-output/cargo-test-v1-<sha256>.txt` using lifecycle path validation, a locked private directory and atomic no-clobber persistence; Unix directory/file permissions are 0700/0600. A versioned content hash and executable exact-read recovery appear in the replacement. Already transformed output is skipped; repeated identical originals share the content-addressed artifact.
- Timeouts, daemon errors, malformed/truncated filter results and artifact-write failures leave the original host output intact.
- The contract reports source implementation, opt-in and zero economic credit. A supported JSON shape is not proof of an installed host version, actual replacement or provider savings. Host minimum verified version remains null and delivery remains unverified. Original artifacts have a 128 MiB / 2,048-file quota under a process lock. Capacity, a busy lock, symlinks or corrupted content-addressed files cause pass-through without replacement. Existing originals are not evicted or overwritten, so advertised recovery remains available.

## Fork parity delta

Canonical fork changes: `fork-core/rtk/src/pipe_cmd.rs` and `fork-core/rtk/tests/hzr_accounting_receipt.rs`. The inherited pipe filter selection, never-worse guard, stdout bytes and passthrough behavior remain unchanged. The command now records its previously missing receipt via the existing TimedExecution producer: successful filtering reports an estimated internal_transport transform; passthrough remains unmeasured with zero token counts. It does not label a transform as delivered. The new `stdin_pipe_emits_only_an_internal_estimated_receipt` integration fixture verifies receipt generation, stage, privacy and absence of a parallel history database in managed receipt mode. Parent owns current-engine manifest regeneration and the complete deterministic fork gate.

The typed read test engine now emits a correlated valid unmeasured internal-transport receipt rather than incorrectly relying on an empty journal. This repairs a fixture defect exposed by fail-closed daemon accounting; production receipt validation was not weakened.

## Explicit full reads

Removed the obsolete `efficient_route_replacement` full-file downgrade from core operation policy, both hook and daemon steering, and legacy ledger inference. An explicit `hzr read FILE --level none` no longer needs a fidelity environment marker and is not rewritten to a smaller semantic view. Genuine engine ownership/security checks remain in place. Hook regression: `explicit_full_read_is_preserved_without_a_fidelity_marker`.

The typed read handler now budgets actual newline-bearing source slices and reads from bounded file descriptors (4 MiB + one sentinel byte) both before and after fork execution. RTK already preserves CRLF and unterminated EOF using byte range reads and direct stdout write; its implementation required no normalization change. The test engine fixture now uses byte offsets plus dd instead of newline-appending awk output. Added `exact_read_preserves_crlf_and_unterminated_eof`. Parent owns subsequent read-cost advisory additions.

CLI help now documents explicit exact reads correctly and fixes replace syntax to `replace PATH --from PATTERN --to REPLACEMENT`.

## Verification and remaining evidence

New deterministic tests:

- `host_hooks::post::tests::replacement_preserves_shape_status_stderr_and_suite_totals`
- `host_hooks::post::tests::unsupported_or_sensitive_output_is_not_replaced`
- `host_hooks::post::tests::envelope_overhead_must_break_even`
- `host_hooks::post::tests::original_recovery_is_quota_bounded_and_never_overwrites_corruption`
- `host_hooks::post::tests::recovery_rejects_symlinks_and_uses_private_permissions`
- `host_hooks::post::tests::busy_recovery_quota_lock_preserves_original_response`
- daemon `post_tool_filter_accepts_only_closed_stdin_argv`
- daemon `exact_read_preserves_crlf_and_unterminated_eof`
- existing full/ranged read and batch/confinement tests remain applicable.

`git diff --check`: passed after the changes. Cargo execution is delegated to the parent's single integration gate to avoid competing workspace builds; no claim of passing Rust tests is made here until that gate returns. No paid calls, real host installation, active-session interception or provider-billed measurement was performed. Source fixtures prove shape/preservation behavior only. This narrow adapter is useful coverage, not universal interception across native tools or final responses.

Official source checked: [Claude Code hooks — PostToolUse output](https://code.claude.com/docs/en/hooks#posttooluse-decision-control). The reference states that built-in replacement must match the tool's structured output schema; incompatible shapes may be ignored. Codex remains unsupported for this replacement path.
