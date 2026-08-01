# HZR 0.3.2 — Honest accounting

HZR reported a reduction ratio that was measuring a shrinking fraction of the traffic it
claimed to cover. This release makes the uncovered part visible, hands agents the commands
that close it, and fixes the search mode whose name promised something it did not deliver.

**Reference point:** comparisons below are against upstream RTK `v0.44.1`, the engine HZR
forks. Where a row says *upstream*, that behaviour is unchanged in `rtk` today.

> Already on `0.3.1`? The implementation landed before that tag, so your runtime already has
> it. `0.3.2` adds the MSRV fix that turned CI green and publishes these notes as the release
> description.

## Optimizer bypass is measured and named

- A command routed through `raw` reaches the shell unfiltered. It delivers exactly as many
  tokens as it consumed, so it raises **both sides** of the reduction ratio and cancels out
  instead of lowering it — a workspace could send half its output to the model and still read
  "87.3% avoided".
- `hzr stats` prints the bypass share directly beneath the headline: operations, delivered
  tokens, and each bypassed tool ranked by cost.
- Every bypassed read or search carries a **copyable** replacement, reconstructed from its
  costliest recorded invocation — `sed -n 120,180p f` becomes
  `hzr rtk -- read f --from 120 --to 180`.
- Where no equivalent exists (`cargo`, `git`, `ps`, `tar`), the panel says so instead of
  inventing one.
- *Upstream:* RTK reports savings per command and has no notion of a bypassed route.

## Agents are corrected, not only measured

- The `PreToolUse` hook answers a bypassed read or search with the equivalent command already
  filled in.
- The decision is **Ask**, never **Deny** — raw stays one keystroke away, because it remains
  correct for checksums, parsers, generated files and complete logs.
- Commands with no first-class equivalent are never interrupted.

## `--mode exact` is finally exact

- Exact mode delegated to a **ranked term model**: the query was lowercased, split on
  non-alphanumerics, stripped of stop words, stemmed, and the survivors OR-ed into one regex,
  so `hzr search "fn record_degraded_rewrite" --mode exact` matched every file containing `fn`.
- It is now a literal, case-sensitive lookup — `rtk rgai --literal` in fork-core, wired to
  HZR's exact mode.
- `--path` accepts several directories. `--path crates fork-core/src` used to fail with
  `error: unexpected argument`.

| Query `fn record_degraded_rewrite` | Files returned |
|---|---|
| RTK upstream `v0.44.1` — `rgai` | 21 (ranked terms) |
| HZR `0.3.1` — `--mode exact` | 21 (same model) |
| HZR `0.3.2` — `--mode exact` | **1** |

## Accounting coverage can return to COMPLETE

- Coverage was `line_count(degraded-rewrites.log) == 0` over an append-only file that nothing
  truncated, so one install performed while the daemon was down pinned `hzr stats` to
  `▲ INCOMPLETE` **permanently**.
- It is now an *open gap*: the next managed rewrite closes it, and the lifetime count stays
  visible, so closing a gap never looks like erasing one.

## The density codec is reachable and measurable

- `hzr-codec` existed but nothing called it — not the hook path, not the planner, not MCP.
- It is now the `hzr_codec` MCP tool, and its transforms are recorded in the efficiency ledger
  under a `codec` subsystem.
- The `shadow` profile reports what compression *would* have saved without altering the text.

## Engine health is read, not asserted

- `caveman-code` was reported with a hardcoded state and a hardcoded version string, so an
  installation missing the runtime looked identical to a working one.
- The version now comes from `engines.lock.toml`; a missing bridge or package is reported as
  degraded together with the command that repairs it.
- Engines that rest by design no longer colour the overall verdict.

## The live observatory proves memory and search

- ICM `Ready` requires a successful supervised probe and a read-only snapshot of the canonical
  store. FTS5-only retrieval is a supported ready capability; a missing database is
  unavailable, never a synthetic empty project.
- Project memory is a privacy-safe topic graph — opaque node IDs, aggregate counts, bounded
  relationships. Memory bodies and store paths never enter the API.
- grepai reports generation and configuration fingerprints, artifact freshness, watcher
  PID/uptime and ownership. A cached semantic canary must return visible hits before semantic
  search is `Ready`, and the canary cannot credit its own ledger.
- Provider tokens and cost appear only from accepted receipts. Without a receipt source the UI
  says `No provider receipts` rather than displaying an invented zero.

## One rule behind all of it

- "Did this operation go through the optimizer?" was answered in three places — the ledger, a
  hand-written SQL predicate and the CLI — with three different answers.
- It is now one classifier in `hzr-core::operation`, and the SQL predicate is generated from
  the same marker list, so the terminal, the dashboard and the ledger cannot drift apart again.

## Fixed in 0.3.2

- A `let` chain in `hzr-core::operation` compiled locally but not on the declared MSRV
  (`rust-version = "1.85"`), failing the `msrv` CI job and blocking the release workflow.
- The GitHub Release description is taken from this file instead of an auto-generated commit
  list, so the published release and the repository state the same thing.

## Upgrading

```bash
hzr update
```

Nothing in your ledger is rewritten. The bypass panel appears on the next `hzr stats`, and
coverage closes on the next rewrite the daemon serves.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
rustup run 1.85.0 cargo check --locked --workspace --all-targets --all-features
scripts/verify-fork-core.sh --test
cd visualizer && bun test && bun run typecheck && bun run build
```

The tag workflow additionally builds Linux and macOS bundles for x64 and ARM64, attests build
provenance and publishes the pre-release.
