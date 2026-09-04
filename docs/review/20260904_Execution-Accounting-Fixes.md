# Execution and accounting fixes

Implementation report written 2026-09-05 for the 2026-09-04 audit.
Original audited source: `fbf204d48704160407e3f06035ac32b9cf0bc67a` (0.7.1).
Target: 0.8.0 working tree, before the subsequently authorized P2 branch integration.
No installed configuration, real user processes, release, or provider-billed experiment was changed by this subtask.

## Confirmed defects and implemented changes

### Durable command lifecycle

`crates/hzr-daemon/src/exec_jobs.rs` persists Running before dispatch and binds the canonical UUID to request hash and workspace. Reusing the same ID never launches another command; incompatible requests are rejected. Restart converts unresolved Running to Interrupted and never replays effects.

The review identified two additional defects:
- A completion write failure removed the active job while leaving its durable record Running forever.
- MCP cancellation could reach the daemon before the corresponding start request, return 404, then permit a late start to execute.

Inactive Running now produces Interrupted with an explicit unknown-completion error, even if the corrective record cannot be persisted. Cancel-before-start writes a durable cancellation tombstone scoped to the operation/workspace, so a later start with that ID remains Cancelled. Child execution exceptions are caught; cancellation awaits executor termination/reaping. The client drop guard checks HTTP failure status when forwarding MCP cancellation. Start failures retain the operation ID for recovery.

Job records use bounded reads on one opened file descriptor with no-follow on Unix, exact filename/operation identity checks, atomic writes and fsync. Record writes cannot exceed their read limit. New dispatch reserves worst-case record/capture capacity; durable history and owned output files have a 2 GiB aggregate ceiling and 20,000-entry ceiling. Capacity exhaustion returns explicit backpressure, retaining IDs rather than permitting accidental replay after deletion. Capture remains bounded to 64 MiB per stream. These ceilings can limit concurrency below the separate maximum of 32 active jobs.

This is bounded retention with backpressure, not automatic history pruning. Long-term maintenance/expiration policy is a remaining product choice. IDs and outputs are not silently evicted.

### Useful bounded output delivery

`ExecJobApiRequest` supports `after_revision` and `max_output_bytes`. A same-revision terminal poll does not repeat the outcome. A small API result budget explicitly reports omitted output and required serialized bytes. It never presents omitted output as complete. CLI wait defaults to the full supported record budget to preserve the original command's exit status.

`crates/hzr-daemon/src/exec_jobs/output.rs`, authenticated `/v1/exec/output`, CLI `hzr exec output`, and MCP `hzr_exec action=output` provide exact retained-output recovery:
- caller supplies operation ID, workspace, stdout/stderr, byte offset, optional SHA and a bounded maximum;
- no arbitrary file path can be supplied;
- spill files must belong to that exact operation's managed output directory;
- responses include retained/original byte counts, next offset, capture truncation, completeness and source hash;
- valid UTF-8 remains text; a trailing partial character is deferred to the next page; arbitrary binary bytes use lossless hex;
- file hashing and range extraction happen in one bounded-memory pass on a blocking worker, with length/mtime change checks;
- an intact capture is checked against the original receipt hash; continuation also validates the caller's expected hash;
- the returned source hash identifies retained bytes. If capture itself was truncated, original total bytes and capture_truncated preserve that distinction.

MCP run/start/wait/cancel now project results to bounded text and continuation metadata instead of numeric byte arrays or daemon-local spill paths. Repeated requested/executed command text is omitted from this projection and explicitly marked. A failed output retrieval preserves the fact that the command ran and directs recovery to the same operation; it does not prescribe executing the command again. CLI output recovery records standalone delivery overhead; MCP uses its existing delivery accounting.

The output API reads completed captures. It does not advertise live stdout streaming or interactive stdin. Per-page hashing rereads stored bytes to validate immutability; this is local I/O, not another model-context read. A large justified result can be recovered by exact pages without re-running the command.

### Direct fork receipts and producer completion

`crates/hzr-cli/src/fork.rs` now retains a supervisor around the owned fork child, preserving inherited stdin/stdout/stderr and recording a completion marker after the child finishes. It preserves exit codes and, on Unix, re-emits the child's signal status through a static safe exec path. No unsafe block or lint suppression remains.

Nonterminal children have a dedicated process group: INT/TERM are forwarded to that owned group, followed by bounded escalation and wait/reap. Interactive stdin retains foreground terminal semantics, and only the owned child is signaled; HZR never signals a process group shared with the user's shell. Non-Unix code retains the normal owned-child termination path. Non-Unix and actual interactive-terminal behavior have not been exercised here.

`AccountingReceiptContext` adds an explicit producer-completed timestamp and typed channel with backward-compatible HookCli default. MCP execution explicitly supplies Mcp; normal and approved execution propagate this channel, including fidelity records. Supervised daemon execution also marks producer completion.

The sweeper can ingest several receipt batches for one operation, replay a batch idempotently, and retain attribution between batches. Only explicit completion plus an empty drain retires the context. A 24-hour age alone no longer proves completion and no longer silently recovers missing accounting. Unknown legacy contexts remain unresolved; registration has bounded capacity rather than deleting unresolved identities.

A SIGKILL/host crash cannot run completion code. Such a context stays unresolved so late receipts retain attribution. This change does not claim universal orphan-process cleanup after an uncatchable signal.

### Isolated failures

Previously, one malformed accounting context aborted the whole sweep before later valid receipts could be processed. Processing is now isolated per context, including handle/drain/ledger/ack failures. Malformed identity data is quarantined only after a durable unattributed gap is recorded; its bytes are retained for inspection. Valid contexts continue. A rejected receipt batch is not treated as success.

`AccountingCoverageStore::ensure_missing` records an unresolved condition without incrementing missing-operation counts on every inspection. Existing `record_missing` continues counting distinct failure events.

## Tests and verification status

Added or updated:
- cancel-before-dispatch replay prevention;
- inactive Running recovery;
- result revision suppression and explicit output budget recovery;
- record symlink refusal and capacity reservation;
- spill retrieval, hash mismatch, foreign-workspace refusal and forged output-path refusal;
- binary preservation and UTF-8 continuation boundaries;
- direct fork exact stdout/stderr and exit 7;
- owned-group TERM forwarding followed by completion metadata;
- malformed context sorted before valid receipt processing;
- multi-batch receipt ingestion, replay and completed producer retirement;
- unresolved old context staying a gap rather than becoming green by age;
- repeated-condition accounting deduplication.

Selected rustfmt and scoped git diff --check passed repeatedly during implementation.

The shared centralized gate was still in progress at this report's freeze. Earlier centralized compiler issues (missing channel in a protocol fixture, forbidden unsafe blocks, test unwrap/must-use warnings) were corrected. Root reported an earlier daemon run with 84 passed, 10 failed and 1 ignored; several jobs/sweeper failures occurred at fake-engine version-probe timeout under high parallel fixture startup. That run is not claimed green. Root planned a bounded-parallelism rerun. The newest output-recovery regressions had not yet received a completed central verdict when source freeze was requested.

The 90-second execution probe remains an explicit ignored-by-default test and must be run by the release gate when required. No paid model calls, billed-token savings, accepted-task improvement, or real host-delivery success are inferred from these changes.
