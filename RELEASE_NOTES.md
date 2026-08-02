# HZR 0.3.3 — One store, exact contracts, evidence an agent can use

Two lines of work land in this release, and both are about the same thing: a contract that
did not match the runtime behind it.

The first was silent. The MCP surface and the CLI wrote to different memory namespaces in
the same repository while both reported success, and a context plan handed agents ranked
file paths with no code in them — so the agent opened every file anyway, which is the work
the plan exists to save.

The second was visible but wrong in shape: a flag promised one kind of evidence while the
runtime returned another. Markdown outlines, line coordinates, exact heads, leading-hyphen
searches, memory provenance and the installed instructions now agree with their public
contract.

Everything below was reproduced before it was changed, and verified after. No benchmark
percentage changes in this release.

## The one-store guarantee held everywhere except through MCP

The project memory namespace is derived from the directory the MCP **client** launched
`hzr mcp serve` from. Clients choose that badly:

| Client | Launch directory observed | Namespace it bound |
|---|---|---|
| Claude desktop app | `/` | the filesystem root |
| Codex | `~/Documents/Codex/<date>/<chat>` | a fresh one per chat |
| Claude Code | the project | correct |

So a memory stored through MCP landed in the root's namespace while a CLI recall looked in
the repository's namespace and found nothing. Reproduced symmetrically: a sentinel stored
over MCP was invisible to `hzr memory recall`, and one stored over the CLI was invisible to
`hzr_memory_recall`. Both writes reported success — the fake-success failure this adapter was
built to prevent, arriving through the one input it does not control.

- The binding is classified before it is used. The filesystem root, the home directory and
  any ancestor of it can never own a project namespace.
- A refused binding returns `isError` with the remediation rather than writing where nothing
  will read. `hzr_codec` needs no workspace and keeps working.
- The `initialize` handshake states the resolved binding, so an agent can tell which project
  its memory belongs to instead of inferring it from a hash.
- `hzr mcp config --client <c> --workspace <dir>` pins the project in the snippet you paste.
  An unpinned snippet says why that matters.
- A directory that is merely not a git repository still binds, because HZR supports projects
  before `git init` everywhere else.

## Doctor names the cause, not the symptom

`client_mcp_ownership` audited Codex's `config.toml` and the desktop app's
`claude_desktop_config.json`. Nothing read `~/.claude.json`, so a direct `icm` server
registered there — the one thing the contract forbids — passed the ownership check while
spawning a second memory writer on every session start. Doctor reported the resulting orphan
processes and told the user to stop processes the client immediately respawns.

- Claude Code is audited, including per-project `projects.<path>.mcpServers`, and never
  written: it is the user's session state, not an HZR file. Writing it is refused rather than
  skipped, so the rule stays checked.
- Each client gets the remediation that applies to it — `claude mcp remove icm` for the file
  HZR must not touch, the config snippet for the two it may.
- New `client_mcp_workspace` check: a registered server with no `--workspace` takes its
  namespace from the client's launch directory. That is the cause of the split namespace
  above, and it was previously silent.

## A plan candidate now contains evidence

`symbol`, `line_start` and `line_end` have been in the protocol since the first release and
the planner never filled them in. A candidate was `{path, score, sources, estimated_tokens}`
— nothing that could not be had from `ls`.

Measured on the intent *"how does the bash hook decide to rewrite a command"*: 11809/16000
tokens spent, coverage 0.50, relevance spread 0.0123–0.0161 (no ranking signal), and the file
that actually answered it was not among the candidates.

- Every candidate carries its symbol outline with line spans, from the fork's own extractor
  (`rtk read <file> --symbols`) rather than a second, weaker one grown inside HZR.
- Verified live: a candidate that reported only `src/lib.rs` now reports `src/lib.rs:4-12`
  with `decide_rewrite(function, 4-12)`.
- Bounded to 24 symbols, with the omitted count and the command that shows the rest, so a
  large generated file cannot spend the plan's budget on its own symbol list.
- Best-effort by design: an unreadable, binary or unsupported-language file contributes no
  outline and degrades to the path it was before. A plan must not fail over one lead.

## Search stops lying about where and what it found

- **A file scope failed entirely.** `--path` never sent `--project-root` in exact mode, so
  the fork treated the search path as the project root and any file path failed with
  "project root is not a directory" — delivered as an opaque HTTP 503. The same query in
  semantic mode worked, which made it look like a mode quirk rather than a missing argument.
- **A scoped hit reported an unusable path.** The fork reports hit paths relative to
  `--path`: scoping to `src` reported `lib.rs`, which does not exist at the root, and scoping
  to a file reported the empty string, which normalized to `.`. Hits are rebased onto the
  project root.
- **Semantic mode emitted source truncated mid-token.** grepai chunks are byte windows, so a
  chunk's first line can begin mid-identifier — line 194 of `hook_runner.rs` came back as
  `en(Value::as_str) else {`, the tail of `.and_then(Value::as_str) else {`. Source that looks
  real and does not parse is worse than no source. A fragment is now completed from the
  recorded line, and only when it provably occurs there, so an index older than the file
  keeps the engine's text rather than having an unrelated line substituted for it.

## The contract says what the code does

- **Hook coverage.** The `PreToolUse` matcher is `Bash|Agent|Task`. Nothing redirects a
  native `Read`, `Grep`, `Edit`, `Write` or `Glob`, and nothing records one — those calls are
  absent from `hzr stats` on both sides of its ratio, so a session that reads files natively
  shows a high reduction over a small measured fraction of what it spent. The preference
  table is the agent's to follow, not something the hook enforces.
- **The codec.** It was described as compressing prose. Its only transform is removing exact
  duplicate paragraphs under verified protected spans. Verified on 4 KB of deliberately
  verbose single-paragraph prose: it comes back byte-identical, and that is a correct result,
  not a failure.
- **The repo map.** `hzr rtk -- memory explore <dir>` gives entry points, hot paths and API
  surface, and the Explore-deny hook already pointed agents at it while the command table
  omitted it. Documented, along with `read --symbols`.

## A subagent gets a brief, not a JSON dump

The plan was prepended to a subagent's prompt as a minified `ContextPlanApiResponse` — no
glossary, no statement of what the entries were, no instruction. A subagent either ignores
that or treats ranked guesses as findings; the second is worse. Leads are now named as
`path:start-end` with their symbol, declared unverified with the commands that confirm them,
and an empty plan says it found nothing rather than reading as "nothing relevant exists".

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

## Known gap

`hzr memory` has no `forget` or `prune`. Legacy-imported rows are now quarantined from
automatic project recall, so they no longer surface as if they described this repository — but
they still occupy the namespace and cannot be removed through the sanctioned control plane.
The engine supports deletion; the HZR path does not exist yet.

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
