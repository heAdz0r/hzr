# HZR 0.3.3 — Exact agent contracts

HZR 0.3.3 fixes a class of failures that looked small in a terminal or dashboard but were
costly for an agent: a flag promised one shape of evidence while the runtime returned another,
or a green tile had no useful drill-down behind it. Markdown outlines, line coordinates, exact
heads, leading-hyphen searches, memory provenance, live observability and the installed
instructions now agree with their public contract.

No benchmark percentage changes in this release. The improvements below are deterministic
behavioral contracts covered by focused regression tests.

## Markdown `--outline` now means an outline

```bash
hzr rtk -- read README.md --outline
```

Markdown previously went through the source-code symbol extractor and returned
`(no symbols found)`. HZR now emits the ATX heading hierarchy (`#` through `######`) with
original source spans. Rust, Python, TypeScript, JavaScript, Go and Java keep their existing
symbol-oriented outline.

The default Markdown read remains a bounded, self-described digest. Use `--outline` when the
heading tree is the evidence you need, `--level none` for exact full content, or `--from` and
`--to` for an exact range.

## Line-oriented flags preserve exact evidence

- `read -n` now defaults to exact content and prints original source coordinates.
- Ranged and tail reads no longer restart numbering at line 1.
- `read --max-lines N` returns exactly the first N lines. It no longer replaces the
  remainder with a smart truncation marker.

These semantics make the output safe to cite in a later edit or review without translating
coordinates back to the source file.

## Exact search accepts option-shaped literals

Exact search is still literal and case-sensitive. The planner now puts search options before
`--` and the query after it, so a literal beginning with `-` cannot be parsed as a fork-core
option:

```bash
hzr search --mode exact -- "--outline"
```

## Legacy memory is retained without leaking across projects

Old imported memory rows do not contain trustworthy repository provenance. Earlier HZR
versions assigned every such row to the repository that happened to run the import, which
could make another project's facts reachable there.

HZR 0.3.3 keeps those records in the canonical store for audit and explicit migration, but
quarantines them from automatic project recall. Current project and global memories are
unchanged; no database rows are deleted or rewritten by the upgrade.

## The live observatory is now an inspection surface

Open the same daemon-owned loopback UI after installation:

```text
http://127.0.0.1:47391/
```

- ICM `Ready` is backed by a supervised probe and a positively repository-filtered snapshot.
  Select a topic in the deterministic Cytoscape graph to load at most 100 bounded memory
  records, then inspect one record's summary, importance, weight, timestamps, source,
  keywords and opaque identifier. The database path and repository token never enter the API.
- grepai `Ready` shows its HZR-owned watcher, generation and configuration fingerprints,
  artifact freshness and a successful semantic-search canary with visible hits.
- Every recent routed operation can be opened to see the requested command, routed command,
  working directory, route, latency, estimate inputs, observed agent and optional session.
  Historical rows without evidence stay `Unattributed`; recent ledger evidence is explicitly
  not presented as proof that an agent process is still alive.
- Five-second background synchronization is quiet. It preserves scroll, graph camera,
  selected topic, open memory and operation detail; only a manual refresh changes the Refresh
  control's busy state.
- RAW receives zero savings credit in both project and global totals even if an imported or
  malformed row contains unequal counters. Provider receipts remain separate from deterministic
  UTF-8 estimates, and missing receipts still mean no data rather than zero usage.

The graph has pan, zoom, fit and reset controls, synchronized Arrow/Home/End keyboard
navigation, focus return, reduced-motion support and 44 px interaction targets. The public
README captures are sanitized and use synthetic memory detail content.

## Installed agent instructions describe the real commands

The HZR-managed block now:

- gives Codex an executable bootstrap command for reading the canonical contract;
- describes Markdown digests, source line coordinates, exact heads and format-aware outlines;
- states that batch atomicity is per file, not an all-files transaction;
- states that `hzr mcp config` prints a snippet while `hzr install --force` writes native
  client registration;
- lists `hzr_codec` alongside the other four MCP tools;
- states that `hzr rtk -- raw` directly spawns argv and does not interpret shell grammar;
- reserves the global managed region for HZR tool routing and directs repository-specific
  roles, paths and test commands to that repository's root instruction file.

The installer resolves the public `hzr` binary symlink before locating bundle assets, so both
clients reference the upgrade-stable `current/share/hzr/HZR.md`. `hzr doctor` recognizes the
Codex bootstrap form and verifies that the referenced contract is readable.

Reinstallation migrates known stale directives outside the managed region without replacing
unrelated user-authored prose. Full-SHA backups and compare-and-swap writes remain in place.

## Release construction stays locked

The version synchronizer now edits only `workspace.package.version` in `Cargo.toml` and the
source-free HZR workspace packages in `Cargo.lock`. It cannot rewrite an unrelated dependency
whose version happens to resemble the previous HZR version, and it keeps the bundle's
`cargo build --locked` input synchronized. This was found by the 0.3.3 clean release build;
the lock was not bypassed or weakened.

## Upgrading

```bash
hzr update
```

Restart already-open Codex or Claude Desktop sessions after upgrading so a client that was
launched with an old direct `icm serve` registration cannot keep that child alive. Confirm the
new native registration and process ownership with:

```bash
hzr mcp status
hzr doctor
```

## Verification

The release gate is:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
rustup run 1.85.0 cargo check --locked --workspace --all-targets --all-features
scripts/verify-fork-core.sh --test
cd visualizer && bun test && bun run typecheck && bun run build
```

The bundle gate additionally verifies the manifest, clean installation, same-version
re-attestation, public `hzr` and compatibility `rtk` entrypoints, all four pinned engines,
and the installed 0.3.3 behavior before publication.
