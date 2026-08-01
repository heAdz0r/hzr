# Contributing to HZR

HZR is heAdz0r's zero-redundancy control plane. Changes must preserve its
one-owner architecture. Extend the inherited engine in `fork-core/rtk` when a
behavior belongs to command execution; use the HZR crates for orchestration,
lifecycle, policy, indexing, memory, accounting, and agent integration.

## Development setup

You need Rust 1.85 or newer, Git, Go, Bun 1.2.19, and a POSIX shell. Bun builds and
tests the Vue visualizer. Node is downloaded and verified by the bundle builder,
so a separate Node installation is not required to assemble a release bundle.

Before Rust implementation, bug fixes, behavior changes, or refactoring, run
`hzr tdd` and preserve the observed RED and GREEN commands for the final report.
Passing tests added after production code are useful regression coverage, but
must not be reported as test-first TDD.

Run the fast checks while developing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
bun --cwd visualizer install --frozen-lockfile
bun --cwd visualizer test
bun --cwd visualizer run typecheck
bun --cwd visualizer run build
```

Changes to `fork-core/rtk` must also pass its complete regression gate and update
the current-engine identity:

```sh
scripts/refresh-current-engine.sh
scripts/verify-fork-core.sh --test
```

Before proposing a release or installer change, assemble and smoke-test the
self-contained bundle:

```sh
scripts/build-bundle.sh /absolute/path/to/hzr-dist
```

## Pull requests

- Explain the ownership boundary affected by the change.
- Add regression coverage for behavior changes.
- Write public HZR documentation and user-facing text in English. Historical fork-core audit and provenance files retain their original language unless a dedicated migration updates current-engine identity and passes the complete fork regression gate.
- Keep the immutable `v0.1.0` import baseline unchanged.
- Do not introduce a second semantic index, memory store, command interceptor,
  or simplified fork-core implementation.
- Keep provider-reported usage separate from estimates.
- Update `engines.lock.toml` and third-party notices when dependencies change.

Small, focused pull requests are easiest to review. Generated build output,
runtime data, indexes, credentials, and local backups must never be committed.
