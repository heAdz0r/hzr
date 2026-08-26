# HZR 0.6.0 deferred verification debt

Status: **open by explicit release decision**. The implementation was prioritized over repeated
acceptance runs. These checks are mandatory before 0.6.0 can be described as fully verified.

## Rust and fork gates

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-targets --all-features`
- `cargo +1.85.0 check --workspace --all-targets --all-features`
- full locked fork regression, clippy warning ratchet, parity manifests, and current-engine digest

## Product acceptance

- durable fidelity transitions, post-spawn unknown state, restart replay, cleanup failure, operator
  reconciliation, and public first-five/sixth/oversized allowance
- command-family completeness validators plus differential filtered/raw fixtures
- daemon-down hook fallback and anti-bypass fleet audit
- MCP stdio-to-daemon read/write/exec, schema parity/budget, cancellation, and exactly-once ledger
- ICM restart/backoff/shutdown/store and grepai TTL/LRU/tombstone/HTTP health
- init/install recovery matrix, sequential A-to-B ownership, and doctor desired state
- visualizer test/typecheck/build and isolated current-source A/B/C browser acceptance
- Caveman bridge/npm audit, release bundle smoke, and live host-to-policy-to-engines-to-ledger-to-UI flow
- fleet doctor and managed-contract reconciliation for applicable projects under `Programming/`

## Release integrity

- clean immutable SHA review
- tag/workflow/native asset and checksum verification
- installed `current` version parity for HZR, RTK fork-core, grepai, and ICM
- paired provider-billed benchmark; until then `economic_claim_ready` must remain false
