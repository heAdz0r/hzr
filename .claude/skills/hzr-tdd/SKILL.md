---
name: hzr-tdd
description: >
  Enforces evidence-backed Red-Green-Refactor for HZR Rust implementation,
  bug-fix, and refactoring work. Requires an observed failing test before
  production changes and the complete HZR Rust quality gate before completion.
---

# HZR TDD Workflow

<!-- hzr-managed-skill: hzr-tdd-v1 -->

Use this skill for Rust implementation, bug fixes, behavior changes, and
refactoring in HZR. Documentation-only and read-only review tasks do not require
a fabricated RED phase.

## Three laws

1. Do not write production code before a relevant test has failed.
2. Add only enough test code to demonstrate the missing or incorrect behavior.
3. Add only enough production code to make that test pass, then refactor while green.

An observed compilation failure caused by a deliberately missing API is valid
RED evidence. A test that already passes is regression coverage, not TDD.

## Required cycle

1. **RED** — add one focused behavioral test and run its narrowest deterministic command.
2. Record the command, non-zero exit status, and the failure that proves the test reached the intended behavior.
3. **GREEN** — implement the smallest production change and rerun the identical command successfully.
4. **REFACTOR** — remove duplication or improve names without changing scope; keep the focused test green.
5. Run the affected-area tests and then the complete HZR quality gate.

Do not treat an unrelated compile failure, missing dependency, flaky external
service, or pre-existing repository failure as RED evidence for the requested
behavior.

## HZR quality gate

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Changes under `fork-core/rtk` additionally require the complete deterministic
fork gate and an intentional current-engine identity update as documented in
`CONTRIBUTING.md`. Never refresh provenance merely to conceal unrelated drift.

## Final evidence

Report:

- the RED command and why its failure was relevant;
- the GREEN command and result;
- refactoring, or explicitly that none was needed;
- targeted and complete gate results;
- unrelated failures or checks not run.

See [testing patterns](references/testing-patterns.md) for HZR-specific examples.

## Upstream basis

Adapted from the official `rtk-ai/rtk` `rtk-tdd` skill at upstream revision
`e0ffd40ef7c450489aca4a50c0ab1358e4375691`. HZR keeps the upstream three-law
workflow while adding explicit RED evidence, full workspace/features gates,
fork provenance rules, and honest separation of regression coverage from
test-first development.
