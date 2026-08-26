# HZR 0.6.1 — the deferred verification, actually run

0.6.0 shipped with its full-suite, MSRV, fork-parity and fleet acceptance runs deferred by an
explicit release decision, recorded in [`docs/TEST_DEBT_0.6.0.md`](docs/TEST_DEBT_0.6.0.md).
0.6.1 is what happened when those gates were finally executed against the immutable 0.6.0
source: they found eight real defects, and this release fixes them.

No gate was relaxed and nothing was reclassified as passing. Every failure below was reproduced
first on the untouched 0.6.0 commit.

## What was actually broken

**The advertised minimum Rust version did not build.** The workspace declares
`rust-version = 1.85` and the README promises "Rust 1.85+", but twenty conditions across
`hzr-daemon` and `hzr-cli` used `let` chains, stabilized only in Rust 1.88. Anyone building from
source on the supported toolchain got a compile error.

**Every MCP read and write was invisible to `hzr stats`.** `hzr_read` and `hzr_write` were
recorded with `accounting_stage = final_delivery`, a stage the efficiency summary deliberately
excludes so that the last hop of a multi-stage pipeline is not counted twice. An MCP tool call
has no earlier stage, so the exclusion removed the only record that existed. Agents working
through MCP produced a ledger reading of zero operations.

**fork-core could hand back a summary where the caller needed exact bytes.** The never-worse
guard trimmed its input before testing for git porcelain, removing the two leading status
columns that define the format. Porcelain was therefore never recognized as a machine protocol
and lost its exactness guarantee.

**The same guard silently disabled every digest a user explicitly asked for.** Any valid JSON
counted as an exact machine protocol, so `rtk json` and the `rtk read` CSV/JSON digests always
fell back to raw. A schema view that never renders is not a bounded read.

**One global MCP pin made every other project permanently wrong.** HZR audited only Claude
Code's user-global `mcpServers` and ignored the per-project `projects[<path>].mcpServers` scope
that Claude Code actually launches, so `hzr doctor` reported a workspace mismatch that no
correct configuration could satisfy.

**The managed agent contract described a surface that does not exist.**
`harnesses.managed_agent.tool_names` carried the 13-tool MCP inventory instead of the 11 tools
the Caveman bridge implements, so the managed runtime refused to start against its own contract.

**Doctor's fidelity remediation had lost its safety wording.** An unknown execution may already
have been billed. The guidance says so again, and names idempotent replay explicitly.

**The suite was flaky under its own parallel load.** ICM liveness gates allowed two seconds for
a process spawn, which is not enough on a machine running the whole workspace suite.

## Fleet exemptions are now declared, not inferred

A hardcoded path heuristic silently exempted one directory from fleet instruction policy, which
is indistinguishable from a hidden bypass. A project that genuinely cannot route a directive
through HZR — a benchmark whose measured subject *is* the engine — now declares that in
`.hzr/policy.toml`:

```toml
schema_version = 1

[[exemption]]
rule = "direct-rtk"
reason = "benchmark-subject"
justification = "This checkout is the upstream RTK baseline that HZR measures itself against."
```

`hzr doctor` honours only the rule named, requires an auditable justification, and reports the
waiver under `fleet_instruction_exemptions` rather than passing it in silence. Execution routes
are not waivable: no file on disk can buy an exception to replacement-capable bypass.

## Compatibility

No protocol, config, ledger or CLI surface changed. `agent-capabilities.json` now lists the
managed-agent tools the bridge actually implements, so its bundle digest changes with this
release. Bundles remain native and self-contained for Linux x86_64/ARM64 and macOS Apple
Silicon/Intel; Windows is not published.

## Verification status

The full CI gate set runs green: `cargo fmt --all --check`, workspace clippy with `-D warnings`,
the full locked workspace test suite, `cargo +1.85.0 check --locked --workspace --all-targets
--all-features`, the locked fork-core parity and digest gate with its test suite, the Caveman
bridge tests and npm audit, and the visualizer test, typecheck and build.

`economic_claim_ready` remains **false**. A paired provider-billed benchmark has still not been
run, so HZR continues to report estimated local reduction and never presents it as billed
savings. The remaining deferred items — that benchmark and a multi-day engine soak — are tracked
in [`docs/TEST_DEBT_0.6.1.md`](docs/TEST_DEBT_0.6.1.md).
