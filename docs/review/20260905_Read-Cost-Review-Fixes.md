# Read-cost review closure

Date: 2026-09-05. Target: 0.8.0. Scope: four P1 defects identified during independent review of the initial read-episode implementation.

## Changes

1. `crates/hzr-daemon/src/read_cost.rs` now counts the serialized cost advice itself and an allocated share of the entire response envelope. A fixed point includes decimal counter widths. Selected and hypothetical full-file results use comparable overhead. The method is versioned `produced_utf8_bytes_div_4_advisory_v2`; these are UTF-8/4 estimates, not provider tokens or acknowledged host delivery. Envelope allocation reserves the maximum estimated-token counter width, at most four conservative bytes per response.
2. The daemon finalizes every path and fork result before mutating any episode. Failed batches have no coverage. During selection it reserves 512 bytes for advice for each accepted file; a maximal-scalar regression validates that bound. Advice never suppresses explicit content.
3. Full-source JSON escaping is counted in one allocation-free byte pass outside the global mutex. Under the lock, repeat detection scans only returned content and advances through sorted coverage once. The previous full-source scan, repeated per-line serialization and per-line scan of all stored spans are removed.
4. MCP read coordinates now support 4,194,304 lines, matching the maximum possible line count in a 4 MiB source. Schema and both exact/outline parsers agree. Continuations beyond 100,000 lines are callable.

The former 900-byte/100-line case recommended a full read after six small calls because advice overhead was omitted. The new deterministic test requires the third-call crossover and verifies cumulative estimated bytes against actual serialized file-plus-advice results.

## Regression checks added

- `read_cost::tests::advice_and_envelope_cost_trigger_crossover_after_three_ranges`
- `read_cost::tests::source_byte_counter_matches_json_escaping_without_line_allocations`
- `read_cost::tests::scalar_advice_always_fits_the_reserved_response_budget`
- `api::read::tests::failed_batch_does_not_commit_read_episode_coverage`
- `api::read::tests::exact_continuation_crosses_one_hundred_thousand_lines`
- MCP `read_continuations_above_one_hundred_thousand_lines_are_callable`

Existing repeated-read, epoch-reset, source-hash, exact CRLF/EOF and shared-budget tests remain applicable.

The separate `crates/hzr-cli/tests/post_tool_output.rs` adds `post_tool_output_requires_accounted_rpc_and_durable_original_before_replacement`. It runs the real CLI observer and authenticated daemon with a correlated fixture engine, validates stdin-only invocation, receipt, exact recovery artifact and structured replacement, then validates missing-receipt pass-through. Everything uses temporary local configuration; no paid model call, host installation or user config modification occurs.

`git diff --check` passed. Rust execution is delegated to the parent's centralized integration gate; this report does not predeclare those tests passing.
