## What changed

Describe the user-visible behavior and the HZR owner affected.

## Zero-redundancy contract

- [ ] This does not create a second index, memory store, interceptor, daemon owner, or additive accounting path.
- [ ] Changes to the inherited engine preserve the `v0.1.0` baseline and update current-engine identity.
- [ ] Actual provider usage remains separate from estimates.

## Verification

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-targets --all-features`
- [ ] Relevant bundle, fork-core, clean-install, upgrade, or adoption gate

Include exact commands and summarize the result.

## Documentation and provenance

- [ ] User-facing documentation is updated where needed.
- [ ] Dependency pins, checksums, notices, and licenses are updated where needed.
