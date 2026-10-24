# Execution history capacity and replay protection

HZR 0.11.5 source review, 2026-10-24.

## Failure and ownership

The old execution store retained every full terminal result indefinitely. Its
20,000-entry limit also counted captured-output directories. A store with 19,985
JSON records and 15 output directories rejected all new commands despite having
no running jobs. Restarting preserved the same exhausted store.

The existing daemon-owned execution store now owns both full results and compact
SQLite replay receipts. This adds no second dispatcher, memory store or execution
path. The accounting ledger is unchanged.

## Contracts

- Capacity admission counts JSON records, while byte accounting includes results,
  output and the replay registry. Existing worst-case active-job reservations stay
  unchanged within the 2 GiB store budget.
- Pressure reclaims terminal payloads older than one hour, retaining the newest
  1,000 full records when the byte budget permits. Running jobs, fresh completions
  and live approval capabilities are protected; a busy approval lock is conservative.
- A batch of compact receipts commits with SQLite `synchronous=EXTRA` and DELETE
  journaling before any payload unlink. The journal directory entry is synchronized
  before a committed receipt becomes the only protection for an operation ID.
- Receipts retain operation ID, canonical workspace, request hash, terminal state
  and revision. They never expire. Their database has a 256 MiB page quota; failure
  to retain an ID rejects admission rather than allowing replay.
- Missing, truncated, foreign or incompatible registries fail closed. Application
  ID, schema version and table definition are validated on every open. Private
  files use mode 0600 and no-follow opens; the execution directory uses mode 0700.
- Existing terminal states remain unchanged. Reclaimed snapshots carry
  `execution_result_expired`; output retrieval returns HTTP 410, non-recoverable.
  A new command under an old bound ID is rejected. Pre-dispatch cancellation IDs
  remain cancellation tombstones even without a request hash.
- Output retrieval holds the same store lease as reclamation through the complete
  spill read and hash verification. Restart prefers an intact full result after
  interruption and removes orphaned output only when a durable receipt exists.

Downgrading to a daemon older than 0.11.5 after compaction is unsupported: it would
not recognize the replay registry. Removing the database or its required marker
is not a safe cleanup method.

## Regression evidence

The added 20,000-record regression first failed with the original
`HTTP 503 execution_store_capacity`, then passed with the fix. The final execution
suite passed 21 tests. Coverage includes directory counting; protected running,
recent and approved operations; byte pressure with sparse bounded spill files;
duplicate IDs and changed request/workspace binding; cancellation and interruption;
receipt-commit failure; restart at both compaction boundaries; changed error wording;
expired output; reader leases; SQLite quota exhaustion; and live or startup registry
replacement, truncation, schema/version changes and symlink rejection.

The complete source gate exited 0 with 3,116 Rust tests passing, 0 failing and 3
ignored. The ignored 90-second execution lifecycle probe was also run explicitly
and passed. The remaining ignored tests are historical manual gates, not claimed
as executed. Workspace strict Clippy and formatting passed; the fork warning ratchet
preserved its reviewed 137-warning baseline. Shell and source-gate regression checks
also passed.

Exact source command:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 scripts/complete-gate.sh --source
```

Disabling debug information and incremental compilation bounds local build output;
it does not change the required test set. The complete source log SHA-256 is
`a51648f1fdc1e39442bee72f8b02449d1f454c46e868ad6541723226ec59ecd4`.

Fresh production dependency installation and auditing with bundled Node 22.17.1
passed: 21 bridge tests and zero npm vulnerabilities. Only compatible `zod` 4.6.7
and `fast-uri` 3.1.9 lockfile updates were needed for newly reported advisories.

The full gate exposed an unrelated loopback-test port race. The test now keeps
its original listener instead of dropping and rebinding it. Production engine
behavior is unchanged; current-engine manifest
`93cdc8c804b8995fbe386a9ac4836b46b294aef256dbfa3fee6ad340c493ce7d`
records the test-source delta, while the immutable import baseline is preserved.

These are local source-review results on macOS ARM64. Native bundle builds,
isolated installation and publication remain separate release gates; this review
does not substitute local Rust tests for Linux execution or artifact checksums.
