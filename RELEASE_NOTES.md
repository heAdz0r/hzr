# HZR 0.4.2 — Bounded reads and current instructions

HZR 0.4.2 closes a second High Critical utilization gap: agents could request exact search and
complete file output as a habit even when semantic discovery, outlines, changed hunks, or exact
ranges were sufficient. It also makes installed instruction freshness an executable lifecycle
invariant instead of trusting a managed-block marker.

The historical ledger cannot produce an exact abuse count because older rows did not preserve
search/read modes. The frozen audit therefore reports bounds, not billed savings: 11,632 generic
reads delivered 18,230,797 estimated tokens, including 798 reads above 4,096 tokens that produced
38.0% of that output. By comparison, 770 outline reads reduced 3,996,067 baseline tokens to
146,059. The HZR-owned full-contract bootstrap alone caused 114 complete reads and 384,950
estimated delivered tokens. These are UTF-8-size estimates, not provider billing.

## What changed

- Claude and Codex managed blocks no longer import or bootstrap the complete `HZR.md` on every
  task. They contain the ordinary routing contract, retain the canonical contract path, and
  direct HZR-policy investigations to an outline followed by only the relevant exact ranges.
- Unbounded managed `read --level none` is reduced to the format-aware smart default. Exact
  ranges, numbered/bounded reads, outlines, symbols, changed hunks, and the explicit
  `HZR_EXACT_FIDELITY=1` whole-file escape preserve their requested semantics.
- Search guidance now uses `auto` or semantic discovery for unknown implementations and reserves
  `--mode exact` for a known literal, symbol, error, key, or audit target. Exact literal recall is
  not heuristically weakened.
- New search/read ledger attribution records typed operation mode, accounting stage, bounds,
  filter level, result limit, include-content choice, and path-scope count without storing query
  text, paths, file contents, or secrets. `stats` exposes mode aggregates; old rows remain
  explicitly unattributed.
- `hzr init --if-needed` refreshes the managed instruction scope on every idempotent lifecycle
  pass. In all-project mode it also retires a detected local legacy RTK block that would override
  the global HZR contract, while leaving clean repository instruction files untouched.
- `hzr doctor` remains read-only and now verifies exact managed-policy currency, contract
  readability, activation scope, and conflicting local RTK/ICM directives. It directs stale
  instruction repair to `hzr init --if-needed`; `doctor --fix` remains limited to safe legacy
  index migration.
- The release workflow publishes stable tags explicitly as GitHub Latest. Existing `v0.4.1` was
  corrected from prerelease to stable without changing its tag or artifacts.

## Upgrade impact

The ledger migration only adds nullable attribution columns, so historical data remains readable.
The active bundle, hooks, daemon, MCP registrations, and instruction contract continue to resolve
through `~/.local/share/hzr/current`.

Upgrade and refresh the current workspace with:

```bash
hzr update
hzr --version
hzr init --if-needed
hzr doctor
```

Restart open agent sessions after installation so they reload the bounded routing contract.

## Verification

The release is checked with:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
scripts/verify-fork-core.sh --test
scripts/build-bundle.sh /absolute/path/to/hzr-dist
```

Dedicated acceptance gates cover stale instruction detection and repair, complete legacy RTK
block retirement, removal of forced full-contract bootstrap, exact-full fidelity escape,
automatic smart-read routing, typed search/read attribution, and stable GitHub Latest policy.
