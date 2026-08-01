# HZR 0.3.2 — Honest accounting and live evidence

The reduction ratio HZR reported was measuring a shrinking fraction of the traffic it
claimed to cover. This release makes the uncovered part visible, gives agents the commands
that close it, fixes the search mode whose name promised something it did not do, and makes
the memory/index evidence behind `Ready` directly inspectable.

The command-output example below is a point-in-time ledger snapshot. It uses HZR's documented
UTF-8-byte estimator, not provider billing; live values necessarily change as agents work.

> **Already on 0.3.1?** Your runtime already contains the behaviours described below because
> their implementation commits landed before `v0.3.1` was tagged. `0.3.2` synchronizes the
> distribution metadata and publishes the complete upgrade narrative. If you are upgrading
> from `0.3.0` or earlier, the rest of these notes describes the user-visible change.

## The number the headline was hiding

A command routed through `hzr rtk -- raw` reaches the shell unfiltered. It delivers exactly
as many tokens as it consumed, so it raises the numerator and the denominator of the
reduction ratio by the same amount and **cancels out** instead of lowering it. A workspace
could send half its tool output straight to the model and still read "87.3% avoided".

`hzr stats` now prints the counterweight directly beneath the headline:

```
OPTIMIZER BYPASS  estimated · these operations skipped HZR
╭─────────────────────────────────────────────────────────────────────╮
│  3.2K of 8.4K operations (37.6%) reached the shell unfiltered       │
│  6.87M of 13.93M delivered tokens (49.3%) received zero filtering   │
╰─────────────────────────────────────────────────────────────────────╯

   sed             719 calls ·    984.0K delivered
     → hzr rtk -- read install.sh --from 1 --to 80
   rg              324 calls ·    294.7K delivered
     → hzr search 'release artifact' --mode exact --path crates
   cargo           511 calls ·    788.2K delivered
     → no first-class equivalent; raw is correct here
```

Each replacement is reconstructed from the costliest recorded invocation of that tool, so
it is a command you can copy — not a category name. Where no equivalent exists, the panel
says so instead of inventing one.

## Agents are told, not just measured

Measurement changes nothing on its own. The `PreToolUse` hook now answers a bypassed read
or search with the equivalent command already filled in:

```
sed -n 120,180p crates/hzr-core/src/ledger.rs
  → hzr rtk -- read crates/hzr-core/src/ledger.rs --from 120 --to 180
```

The decision is **Ask**, never Deny. Raw stays one keystroke away, because it remains the
correct tool for checksums, parsers, generated files, complete logs and machine-readable
data. Commands with no first-class equivalent — `cargo`, `git`, `docker` — are never
interrupted.

## `--mode exact` is finally exact

`hzr search "fn record_degraded_rewrite" --mode exact` used to return
`fork-core/TRACKED_CHANGES.patch` as its top hit and never return the definition. Exact
mode delegated to a ranked term model that lowercases the query, splits it on
non-alphanumeric characters, drops stop words, stems what remains and ORs the survivors
into one regex — so it matched every file containing `fn`.

This is the single behaviour most likely to make an agent abandon `hzr search` for `raw rg`
and never come back. Exact mode is now a literal, case-sensitive lookup.

| Query | Before | After |
|---|---|---|
| `fn record_degraded_rewrite` | 21 files | **1 file** |

`--path` also accepts several directories now. `--path crates fork-core/src` used to fail
with `error: unexpected argument`, which is exactly the moment an agent gives up on the
command.

## Accounting coverage can return to COMPLETE

Coverage was `line_count(degraded-rewrites.log) == 0` over an append-only file that nothing
ever truncated. One installation performed while the daemon was down pinned `hzr stats` to
`▲ INCOMPLETE` permanently — which teaches an operator to ignore the field entirely, the
opposite of what an integrity signal is for.

Coverage is now an *open gap*. The next managed rewrite folds it into a lifetime total and
closes it. The history stays visible, so closing a gap never looks like erasing one.

## The density codec is reachable and measurable

`hzr-codec` existed but nothing called it: not the hook path, not the planner, not the MCP
surface. It is now the `hzr_codec` MCP tool, and its transforms are recorded in the
efficiency ledger under a `codec` subsystem — so the capability is justified by measurement
rather than by assertion. Its `shadow` profile reports what compression *would* have saved
without altering the text.

## Engine health is read, not asserted

`hzr daemon status` reported `caveman-code` with a hardcoded state and a hardcoded version
string, so an installation missing the runtime entirely looked identical to a working one.
The version now comes from `engines.lock.toml`, and a missing bridge or package is reported
as degraded with the command that repairs it. Engines that rest by design no longer colour
the overall verdict.

## The live observatory proves memory and search

The loopback visualizer at `http://127.0.0.1:47391/` now answers the operational questions
behind a green badge:

- ICM `Ready` requires a successful supervised probe and a read-only snapshot of the canonical
  store. FTS5-only retrieval is a supported ready capability; a missing database is unavailable,
  never a synthetic empty project.
- The current project's memory is rendered as a privacy-safe topic graph with opaque node IDs,
  aggregate counts and bounded relationships. Memory bodies and store paths never enter the API.
- grepai reports its full generation and configuration fingerprints, artifact freshness, watcher
  PID/uptime, HZR ownership and ready marker. A cached semantic canary must return visible hits
  before semantic search is `Ready`, and the canary cannot credit its own ledger.
- Project-local activity shows optimized and RAW operations separately. RAW always receives zero
  avoided-token credit, while avoidable reads/searches carry a copyable HZR replacement.
- Provider tokens and cost appear only from accepted provider receipts. Without a receipt source,
  the UI says `No provider receipts` instead of displaying invented zero usage.

Utility and visualizer versions, help commands and per-service diagnostics are part of the same
typed `/v1/dashboard` snapshot. The UI ships inside the daemon bundle and starts with the normal
HZR user service; Bun is not required on the installed runtime.

## One rule behind all of it

"Did this operation go through the optimizer?" used to be answered in three places — the
ledger, a hand-written SQL predicate, and the CLI — with three different answers. It is now
one classifier in `hzr-core::operation`, and the SQL predicate is generated from the same
marker list, so the terminal, the dashboard and the ledger cannot drift apart again.

## Upgrading

```bash
hzr update
```

Nothing in your ledger is rewritten. The bypass panel appears on the next `hzr stats`, and
coverage closes on the next rewrite the daemon serves.

## Verification

The source release gate is:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
scripts/verify-fork-core.sh
cd visualizer && bun test && bun run build
```

The GitHub tag workflow additionally builds Linux and macOS bundles for x64 and ARM64, packages
each archive, proves a clean install without external engines, publishes aggregate checksums and
attests build provenance. The live API must report separate project estimates and provider
receipts, an HZR-owned grepai watcher, a successful semantic canary and a non-synthetic ICM
snapshot before the observatory is accepted as `Ready`.
