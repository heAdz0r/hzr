# HZR ← upstream RTK selective sync — Product Requirements Document

Status: Draft for approval — no work scheduled until items are ticked in §11
Owner: HZR control plane
Date: 2026-08-22
HZR version probed: 0.4.6 (fork-core `rtk 0.44.1-fork.1`)
Upstream probed: `rtk-ai/rtk` — tags `v0.44.2`, `v0.45.0`, branch `develop` @ `f8d636d`
(pre-release `dev-0.45.1-rc.362`, 2026-08-20), plus 60 open pull requests as of 2026-08-22.

This document does not propose a merge. The two trees diverged structurally (§3) and a
merge is not expressible. Every item below is an **adaptation**: what upstream does, how it
does it there, what our tree does today with file/line evidence, and what the equivalent
change looks like in the HZR layout.

No token-savings number in this document is a provider-billed measurement. Upstream figures
are quoted as upstream reported them and are marked as such.

---

## 1. Executive result

Upstream shipped 59 non-merge commits between `v0.44.1` (our import base) and `develop`
(`f8d636d`), touching 32 files for +3,968/−377 lines. Of that delta the fork already carries
the multiline-rewrite family; the rest is not in our tree.

Four conclusions decide this PRD:

1. **HZR is ahead of upstream on the control plane, and behind it on the filter engine.**
   Our typed E1–E10 plan already covers shell wrappers, path-prefixed executables, quoted
   ranges and bounded pipelines — the exact things four open upstream PRs are still trying to
   fix. Meanwhile upstream has fixed output-fidelity defects in the *shared* filter code that
   we inherited verbatim and never touched.
2. **We carry three inherited defects that silently corrupt or truncate agent-visible output.**
   The worst is a single-line one: fork-core drops the entire remainder of a child's output at
   the first non-UTF-8 byte ([fork-core/rtk/src/stream.rs:89](fork-core/rtk/src/stream.rs:89)).
   That is a fidelity failure that our own accounting reports as a savings.
3. **We inherited a rewrite rule that inserts HZR into `sudo` elevation.**
   `sudo docker ps` → `sudo rtk docker ps` is an *asserted* behaviour in our test suite
   ([fork-core/rtk/src/discover/registry.rs:5314](fork-core/rtk/src/discover/registry.rs:5314)).
   An elevated fork-core writes the ledger DB, tee files and audit log as root inside a
   user-owned data root, against the daemon-ownership model in `FORK_PARITY.md`.
4. **The single largest untapped token saving upstream has found is `gh --json` / `gh api`.**
   Both are 0%-savings passthrough in our tree today. Open PR #3580 packs them losslessly with
   a runtime round-trip verification and a raw fallback. This is the only item here whose
   expected saving is large enough to move `hzr stats`.

Recommended scope for **HZR 0.4.7**: §4 P0 block (six items) + §5 P1 block (four items).
Everything else is parked behind an explicit decision in §11.

---

## 2. Method and reproducibility

```bash
git clone --filter=blob:none https://github.com/rtk-ai/rtk.git upstream-rtk
cd upstream-rtk && git checkout develop
git log --oneline --no-merges v0.44.1..develop        # 59 commits
git diff --stat v0.44.1..develop -- . ':!CHANGELOG.md' ':!Cargo.lock'
gh pr list -R rtk-ai/rtk --state open --limit 60
```

Comparison was done three ways, because a path-level diff is meaningless across the
restructure:

- **Inventory** — basename-level set difference between `upstream/src/**/*.rs` (121 files)
  and `fork-core/rtk/src/**/*.rs` (125 files).
- **Symbol presence** — for every function/const introduced upstream in the window, a direct
  grep for that symbol in the corresponding fork-core file.
- **Behavioural read** — the specific predicate or code path was read on both sides before an
  item was called a gap. Items that could not be confirmed this way are marked
  **`verify`** and are not scheduled.

---

## 3. Divergence map

| Axis | Upstream `develop` | HZR fork-core |
|---|---|---|
| Layout | `src/{core,cmds/<lang>,discover,hooks,filters,analytics,learn,parser}` | flat `src/*.rs` + `src/{discover,filters,learn,memory_layer,parser,bin}` |
| Version | `0.45.1-rc.362` | `0.44.1-fork.1` (imported 2026-07-31) |
| Base | continuous | `feat/upstream-0.42-fork.1` + selective P0/P1 sync onto 0.44.1 |
| Extra deps | `encoding_rs`, `codepage`, `oem_cp`, `windows-sys`, `ureq`, `quick-xml`, `automod` | `rayon`, `aho-corasick`, `memchr`, `xxhash-rust`, `notify`, `csv`, `toml_edit` |
| Filter TOMLs | 63 | 59 (missing 5× `pulumi-*`, extra `jest.toml`) |
| Native cmd modules absent on our side | mvn, gradlew, sbt, dotnet(+trx,+binlog), php suite (phpunit/phpstan/pest/paratest/pint/ecs/artisan), rake, rspec, rubocop, uv, glab, ecs | — |
| Modules absent upstream | `rgai_cmd`, `grepai`, `write_cmd`/`write_core`/`write_lock`, `read_digest`/`read_cache`/`read_changed`/`read_symbols`, `memory_layer/*`, `sqlite_cmd`, `tar_cmd`, `remote_logs_cmd`, `ssh_cmd`, `fidelity`, `session_stats`, `grep_cmd`, `ps_cmd`, `lsof_cmd` | — |

Consequence for every item below: **no cherry-pick is possible**. Each item is a re-implementation
against our own file, with our own tests, and must keep the current-engine manifest
(`fork-core/CURRENT_ENGINE.toml`) and the `scripts/verify-fork-core.sh` gate green.

---
## 4. Part A — merged upstream work absent from our tree

### A1 (P0) — Child output is truncated at the first non-UTF-8 byte

**Upstream:** `a3ba3c5` *fix(stream): decode lossily instead of dropping lines on invalid UTF-8*,
refined by `ae5d1ae`. Introduces `read_lines_lossy(reader) -> impl Iterator<Item = String>` in
`src/core/stream.rs`, a manual `read_until(b'\n')` loop that applies `String::from_utf8_lossy`
per line, strips `\r\n`, and stops only on a real I/O error.

**Our state:** [fork-core/rtk/src/stream.rs:89](fork-core/rtk/src/stream.rs:89) and
[:96](fork-core/rtk/src/stream.rs:96) use
`BufReader::new(stdout).lines().map_while(Result::ok)`. `Lines` yields `Err(InvalidData)` for a
non-UTF-8 line and `map_while` **terminates the iterator** — every subsequent line of that stream
is discarded. The child still exits 0, the filter runs on a truncated buffer, and
`TimedExecution::track` records the difference as a saving.

**Failure shape:** one `latin-1` filename, one `\xff` in a compiler diagnostic, one binary blob in
a test log ⇒ the rest of the build output never reaches the agent, and `hzr stats` reports the
loss as efficiency.

**Adaptation:** port `read_lines_lossy` into `fork-core/rtk/src/stream.rs` and use it in both
reader threads. ~40 lines, no new dependency (the Windows code-page half of the upstream work is
split out as A7 and is *not* required for this).

**Acceptance:** a regression test feeding a child that emits `ok\n\xff\xfe\nafter\n` must observe
`after` on the filtered path and in `raw_stdout`; existing stream tests unchanged.

**Effort:** S. **Risk:** low. **Gain:** fidelity, not tokens.

---

### A2 (P0) — `sudo`-prefixed commands are rewritten into HZR

**Upstream:** open PR #3569 *fix(rewrite): never rewrite sudo-prefixed commands* (merged behaviour
not yet in `develop`; listed here because the defect is ours today and the reasoning is upstream's).

**Our state:** [fork-core/rtk/src/discover/registry.rs:70](fork-core/rtk/src/discover/registry.rs:70)
strips `sudo` as an env prefix and the rewriter re-attaches it. The behaviour is *asserted*:
[registry.rs:5314](fork-core/rtk/src/discover/registry.rs:5314) expects
`sudo docker ps` → `sudo rtk docker ps`, and [:5873](fork-core/rtk/src/discover/registry.rs:5873)
expects `sudo noglob git status` → `sudo noglob rtk git status`.

**Why this is worse for HZR than for upstream RTK:** an elevated fork-core inherits
`RTK_DB_PATH`, `RTK_MEM_DB_PATH` and the private PATH/audit dirs that `ForkRuntimePaths`
([crates/hzr-exec/src/adapter.rs:18](crates/hzr-exec/src/adapter.rs:18)) points at the user-owned
HZR data root. Root-owned `history.db`, `-wal`, `-shm`, tee files and audit rows inside that root
break the daemon singleton/ownership model documented in `FORK_PARITY.md`, and can only be
repaired with `sudo rm`. The user granted elevation to *one* binary; we insert a second one.

**Adaptation:** classify a `sudo`/`doas`/`pkexec` prefix as *no-rewrite*, and — because HZR must
not simply lose the row — emit it as a typed tracked bypass rather than a silent proxy. The
natural class is a new `E11PrivilegedPrefix` in
[crates/hzr-core/src/ledger.rs:424](crates/hzr-core/src/ledger.rs:424), or `E10CapabilityGap`
with a reason if we do not want to grow the enum this release.

**Acceptance:** the two registry tests above invert; the 85-case anti-evasion fixture gains
`sudo <covered>` and `sudo env FOO=1 <covered>` cases resolving to no-rewrite + tracked; no
root-owned file appears under the data root in the smoke run.

**Effort:** S. **Risk:** low, but it is a **behaviour change with existing tests to flip** —
needs an explicit decision, not a silent fix.

---

### A3 (P0) — Owner-only permissions on data files

**Upstream:** `v0.44.2` — `9cf7a6d` *store history db, tee logs and audit log owner-only*,
`a1bbcaf` *create data files owner-only instead of chmod after write*, `18925c2` *tighten data
dirs that already exist*, `2ba02f4` follow-up review. Implemented as
`utils::create_private_dir` / `utils::restrict_file` / `utils::set_owner_only(path, mode)` plus
`tracking::restrict_db_files` + `tracking::db_sidecars`, which handle the SQLite `-wal`/`-shm`
siblings by `OsString` concatenation (not `PathBuf::push`, which would target `history.db/-wal`).

**Our state:** zero occurrences of `set_owner_only`, `create_private_dir`, `restrict_file`,
`0o600` or `from_mode` in `fork-core/rtk/src/{utils,tracking,tee,hook_audit_cmd}.rs`. Files are
created at the process umask. The parent dir is created with `create_dir_all`, which leaves the
intermediate at umask even when the leaf is tightened.

**Adaptation:** port the four helpers into `fork-core/rtk/src/utils.rs` and call them from the
tracking DB open path, the tee writer and the hook audit log. Then audit the **HZR side** —
`crates/hzr-core/src/ledger.rs` and the daemon lock/socket — for the same property, since the
upstream fix does not cover files HZR creates itself.

**Acceptance:** a test asserting mode `0o600` on `history.db`, `history.db-wal`, `history.db-shm`,
tee output and the audit log on Unix; a no-op assertion on non-Unix.

**Effort:** S–M. **Risk:** low.

---

### A4 (P0) — `git log` argument model: flags vs. values, `--`, raw shapes

**Upstream:** eight commits, all after our import — `705a2f8`, `1a1b306`, `84169e2`, `9bbf55c`,
`ca89767`, `f8d636d`, `3cc80b2`, `40e4f3a`. Together they replace ad-hoc `args.iter().any(...)`
scanning in `run_log` with a small parsed model:

- `consumes_next_token_as_value(arg)` — closed list of ~30 options whose value is a separate
  token (`--grep`, `--author`, `--diff-algorithm`, `--diff-filter`, `--output`, …), so
  `git log --grep -p` no longer reads `-p` as the patch flag.
- `log_arg_tokens(args) -> Vec<LogArg>` + `flag_args_from_tokens` — tokenize once, share the
  flag/value classification between limit parsing and every flag-presence check.
- `requests_raw_diff_shape` / `requests_raw_log_output` — if the caller asked for `-p`,
  `--patch`, `--stat`, `--numstat`, `--shortstat`, `--raw`, hand the whole invocation to
  `run_passthrough` untouched instead of reformatting it.
- `args_utils::restore_double_dash(args)` — clap's `trailing_var_arg` swallows the literal `--`,
  so `git log -- -p` loses the separator and `-p` is misread as a flag rather than a pathspec.
- `-U`/`--unified`/`--expand-tabs`/`--max-parents`/`--min-parents` take **attached** values only.

**Our state:** none of `consumes_next_token_as_value`, `log_arg_tokens`, `flag_args_from_tokens`,
`real_flag_args`, `requests_raw_diff_shape`, `requests_raw_log_output`, `parse_limit_from_tokens`,
`LogArg` or `restore_double_dash` exists anywhere in `fork-core/rtk/src/`. Our `run_log` reaches
`filter_log_output` ([fork-core/rtk/src/git.rs:695](fork-core/rtk/src/git.rs:695)) for shapes
upstream now passes through.

**Failure shapes we carry today:** `git log --stat` reformatted instead of passed through;
`git log --grep "--pretty"` misclassified; `git log -- -p` losing its pathspec separator;
`git log -U5` treated as `-U` + limit `5`.

**Adaptation:** port the argument model into `fork-core/rtk/src/git.rs` and add
`restore_double_dash` to `fork-core/rtk/src/args_utils.rs`. This is the single largest
correctness item in the window (+574 lines upstream, roughly half of it tests).

**Acceptance:** one table-driven test per upstream commit's case; `git log --stat` byte-identical
to raw `git log --stat`; exit codes preserved.

**Effort:** M. **Risk:** medium (touches the most-used filter). **Gain:** correctness; a small
token *increase* on `--stat`-shaped calls, which is the correct outcome.

---

### A5 (P0) — Multiline rewrite: cross-line `[[ … ]]` is not detected

**Upstream:** `b09da2d` *fix(rewrite): bail on cross-line `[[ ]]`, share quote-state walker*.
Adds a `QuoteScan` iterator (`(offset, byte, in_single_before, in_double_before)`, skipping
backslash pairs outside single quotes) used by every line scanner, and
`line_has_unbalanced_test_brackets(code)`, which counts `[[`/`]]` **as words** and refuses the
multiline path when they do not balance within a line.

**Our state:** we carry the multiline family (`rewrite_multiline_block`, `BLOCK_KEYWORDS`,
`comment_start`, `line_has_unbalanced_grouping`, `ansi_c_quote_defeats_lexer`, `BASH_JOIN_RE`,
`tokenize_with_newlines` are all present in
[fork-core/rtk/src/discover/registry.rs](fork-core/rtk/src/discover/registry.rs)) but **not**
`QuoteScan` and **not** `line_has_unbalanced_test_brackets`. Bash allows
`[[ -f a &&` / `-f b ]]` across lines; we treat each line as independent and can rewrite the
first half of a conditional.

**Adaptation:** port `QuoteScan` (it also de-duplicates the three hand-rolled quote walkers we
already have) and `line_has_unbalanced_test_brackets`, and add the bail to
`rewrite_multiline_block`.

**Acceptance:** `[[ -f a &&\n-f b ]] && git status` passes through whole; existing multiline
tests unchanged.

**Effort:** S. **Risk:** low. **Gain:** prevents emitting a broken command.

---

### A6 (P1) — `tee` recovery-hint slugs are unbounded

**Upstream:** `fd2f942` *hash long recovery-file slugs to prevent collisions and shorten hints*,
`9ee1532` refactor. A slug longer than the threshold collapses to a readable prefix + `short_hash`
(first 6 hex of SHA-256). Upstream's own rationale is token cost: the slug usually embeds a path
the model already issued.

**Our state:** [fork-core/rtk/src/tee.rs:16](fork-core/rtk/src/tee.rs:16) has `sanitize_slug` only —
no shortening, no hash. We do have the path-quoting fixes (`991ed97`, `5de188b`, `b1f35f6`).

**Adaptation:** ~25 lines in `tee.rs`. `sha2` is already a fork dependency.

**Effort:** S. **Risk:** low. **Gain:** small, per-tee-hint token reduction.

---

### A7 (P3) — Console code-page decoding for child output

**Upstream:** `5bd410e`, `945c3a5`, `de38f44`, `b35ff3a`, `f496f59`, `32dc612` — `decode_process_output`
/ `decode_mixed` / `decode_line` / `output_codepage` in `src/core/utils.rs`, three new dependencies
(`encoding_rs`, `codepage`, `oem_cp`) plus `windows-sys`, and a centralisation of every child-output
decode onto `exec_capture`.

**Our state:** absent. **Recommendation: do not adopt now.** The Windows artifact is an open ⚪ gate
in `FORK_PARITY.md`; adopting three dependencies for a platform we do not ship is cost without
return. The *centralisation* half (one decode path, signal diagnostic preserved on capture —
`32dc612`) is worth taking **with A1** and costs nothing.

---

## 5. Part B — open upstream PRs worth adopting

Ranked by expected value to HZR, not by upstream's ordering. Line counts are upstream's diff size.

### B1 (P1) — PR #3580 · lossless CSV+schema packing for `gh --json` / `gh api` — +1821/−17, 9 files

The only item here with a material savings ceiling.

**What it does:** a new `core/jsonpack` module re-encodes JSON **losslessly**, removing only
repetition. Top-level arrays of objects become a declaration line plus CSV rows
(`[3]{id:int,author.login:string,title:string,tag:string?}`); `gh api` envelopes stay valid JSON
with dense inner arrays rendered as `{"_cols":[…],"_rows":[[…]]}`. Uniform nested objects flatten
into dotted columns with a hard 512-column ceiling. `--jq`/`--template`/`-q`/`-t` and file
redirects keep full passthrough. `pack()` **decodes its own output and requires exact value
equality with the parsed input before emitting a byte** — any mismatch returns the raw bytes.
The packed paths use a byte-exact runner (inherited stdin/stderr, verbatim bytes, >32 MiB streams
through untouched) rather than the shared line-oriented capture.

**Our state:** `gh <cmd> --json …` and `gh api …` are passthrough in
[fork-core/rtk/src/gh_cmd.rs](fork-core/rtk/src/gh_cmd.rs) (`run_passthrough` at :21/:24/:31/:44,
:135, :139, :442, :448) and therefore tracked at 0% savings.

**Why it fits HZR specifically:** the runtime self-verification plus raw fallback is exactly our
"never worse, never wrong" contract, and it composes with `guard::never_worse` already in the tree.
The worst case is today's behaviour.

**Adaptation notes:** upstream's port is ~500 lines with **zero new dependencies** (it explicitly
rejected the `headroom-core` crate dependency). We would additionally need to decide how the
packed form is declared to the model — HZR's density contract is injected pre-generation by the
Caveman bridge, so the declaration line must be described there once rather than re-explained per
response. `serde_json` needs the `float_roundtrip` feature.

**Open risk to state plainly:** this emits a format the model did not ask for. Upstream flags the
same trade-off and notes the gate is one line if we want to pack `gh api` only and leave `--json`
alone. **Recommendation: adopt `gh api` first, `--json` behind a config flag, promote after one
observation window.**

**Effort:** L (the code is large and the review must be adversarial). **Risk:** medium, bounded by
the verify-before-emit design.

---

### B2 (P1) — PR #3579 · cap raw retention in search — +71/−1, 2 files

**What it does:** routes the path-based search capture through `run_streaming`'s `CaptureOnly`,
which already caps retention at `RAW_CAP` (10 MiB) and warns on truncation. Upstream measured, on
a 64 MiB match set, peak allocation dropping from >90 MiB to the cap and wall time from 2m25s to
25s (debug) / 10s (release), because the parse pass now runs over 10 MiB instead of everything.
The reported production symptom was a 30 GB log tree producing 4.6 GB of matches, rtk peaking at
~9 GB, and the **calling agent being OOM-killed**.

**Our state:** identical code path. `engine_capture`
([fork-core/rtk/src/search.rs:300](fork-core/rtk/src/search.rs:300)) calls `exec_capture_stdin`,
which is an unbounded `.output()`. Our own `grep_cmd.rs` is worse:
[:63](fork-core/rtk/src/grep_cmd.rs:63) `.output()` then
[:67](fork-core/rtk/src/grep_cmd.rs:67) `from_utf8_lossy` — a full second copy — and it has no
upstream counterpart, so nobody else will fix it.

**Adaptation:** one-line change in `search.rs` mirroring upstream, plus the same treatment for
`grep_cmd.rs` (ours). `| head -N` is not an escape hatch on this path: nothing is written until
the child exits.

**Acceptance:** a memory regression test under `ulimit -v`; grouped-form output byte-identical
below the cap; the `N matches in M files` header documented as a floor above the cap.

**Effort:** S. **Risk:** low. **Gain:** removes an agent-OOM class and a large latency cliff.

---

### B3 (P1) — PR #3577 · never-worse **exit-code** guard across all tool filters — +913/−57, 11 files

**What it does:** filters currently render all-green summaries when the child exited non-zero —
pytest collection errors as "No tests collected", ruff parse errors as "All files formatted
correctly", mypy as "No issues found", vitest/jest ignoring `numFailedTestSuites`, tsc missing
position-less global errors, go vet / go test -json / golangci-lint dropping exit 1, and a lint
fallback that matches "0 error". The PR gates each summary on the child's exit status.

**Our state:** we have `guard::never_worse` (byte-identical to upstream at
[fork-core/rtk/src/guard.rs](fork-core/rtk/src/guard.rs)) and we apply it at **more** sites than
upstream — but `cargo_cmd.rs` has **zero** `never_worse` call sites while upstream has three
(including the `cargo test` summary added in `13bd8f2`/`d003920`). The exit-code half of the
problem is unaddressed on our side across the same filter set.

**Why this is a P1 for HZR and not just upstream hygiene:** `FORK_PARITY.md` claims "Generic
test/error routes preserve argv and exact child exit status, so failure-first filtering cannot
change a failing verification command into success." That claim is true for the *generic* routes
and false for the *specific* tool filters. The gap is exactly where an agent's verification step
lives.

**Adaptation:** apply the existing `filter_go_build_with_exit` pattern per filter; port
`compute_test_summary` extraction (`d003920`) into `cargo_cmd.rs` and add the three
`never_worse` sites.

**Effort:** M. **Risk:** low per site, wide blast radius. **Gain:** removes a class of false-green
verification.

---

### B4 (P1) — PR #3550 · `ignore_dirs` / `ignore_files` are dead config — +314/−29, 8 files

**Our state:** [fork-core/rtk/src/config.rs:72-73](fork-core/rtk/src/config.rs:72) declares
`ignore_dirs` and `ignore_files` with defaults, and a repo-wide grep finds **zero consumers**
outside `config.rs`. A user (or HZR's managed config) can set them and nothing happens.

**Adaptation:** merge configured values on top of the hardcoded noise defaults in the system
commands (`ls`, `find`, `tree`, `wc`, search). Cheap, and it makes a documented knob real.

**Effort:** S. **Risk:** low. **Gain:** direct token reduction in noisy repos.

---

### B5 (P1) — PR #3603 · `find` never-worse against the *capped* listing + tee tail hint — +128/−10

The guard compared against the full match list, so on small capped runs the summary format cost
more than the plain paths it replaced (upstream's own benchmark negative on `find --max 10`).
Also adds a tee tail hint when RTK truncates on its own initiative, and passes through `find`
invocations with flags it cannot filter (`-not`, `-exec`) instead of erroring.

**Our state:** [fork-core/rtk/src/find_cmd.rs:295](fork-core/rtk/src/find_cmd.rs:295) has the
same single-comparison guard. The third part matters most for us: an error on an unfilterable
`find` is unrecoverable for the agent once the hook has rewritten the command.

**Effort:** S. **Risk:** low.

---

### B6 (P2) — PR #3538 · `pass_through_if_args` in the declarative TOML engine — +191/−90

Adds a general capability to `toml_filter`: a filter may opt out of filtering for specific
invocation shapes. `du.toml` uses it for `-s`, `--summarize`, `-d`, `--max-depth`, because every
row of `du -s` is an independent total and the 40-line cap silently dropped the largest consumers
(upstream: a 312-row survey returned 42 rows, missing the 4.4 GB entry, exit 0).

**Our state:** `pass_through_if_args` absent from
[fork-core/rtk/src/toml_filter.rs](fork-core/rtk/src/toml_filter.rs); `du.toml` present with the
shape-blind cap. The engine capability is reusable by all 59 of our filters, which is the real
argument for taking it.

**Effort:** S–M. **Risk:** low.

---

### B7 (P2) — PRs #3559 / #3560 · declarative filter correctness — +52/−8 and +74/−3

Seven independent over-broad or lossy filters, all shipped in our tree with the same content:
`turbo` (failing run loses its pass/fail tally), `quarto-render`, `bundle-install`, `gcc`,
`spring-boot` (any `java -jar *.jar` activates Spring compaction), `liquibase`, `ssh`. These are
TOML-only changes — the lowest-risk, highest-certainty batch in this document.

**Effort:** S. **Risk:** very low.

---

### B8 (P2) — PR #3621 / #3561 · silence informational stderr for agents

`RTK_QUIET=1` / `[hooks] quiet = true`, cached in a `OnceLock` so no config I/O lands on the
command-resolution critical path. Suppresses "failed to resolve via PATH, falling back to direct
exec" notes for commands that **succeeded** — a diagnostic that costs more tokens than the filter
saved. HZR's managed path already sets `RTK_TEE=0` and `RTK_TELEMETRY_DISABLED=1`; this is the
same family and should be set the same way.

**Effort:** S. **Risk:** low.

---

### B9 (P2) — PR #3606 · stream `npm`/`npx` output instead of retaining until exit — +208/−18

`rtk npm` uses `FilterMode::CaptureOnly`, so a long-running npm script's readiness line is
invisible to the parent until the child exits — it breaks dev servers, watchers and any agent
orchestration that starts a background service. Our [npm_cmd.rs](fork-core/rtk/src/npm_cmd.rs) has
no `FilterMode` reference at all and is 79 non-test lines against upstream's 169; the same
class of defect is likely present. **`verify` before scheduling.**

**Effort:** S–M. **Risk:** low.

---

### B10 (P2) — small correctness batch, confirmed applicable to our files

| PR | Change | Our file | Confirmed |
|---|---|---|---|
| #3573 | `curl -sS` instead of bare `-s`, so failures keep curl's error message | [curl_cmd.rs:10](fork-core/rtk/src/curl_cmd.rs:10) injects bare `-s` | yes |
| #3600 | multiline split on a lone `\r` separator (our dispatch only enters on `\n`) | `discover/registry.rs` | yes |
| #3601 | preserve the `--` pathspec separator in `git commit` | no `restore_double_dash` anywhere | yes (bundle with A4) |
| #3631 | announce a failed commit instead of ending on pre-commit's `...Passed` line | `git.rs` commit path | yes |
| #3607 | unbreak `git diff` when an external diff driver (difftastic) is configured | no `no-ext-diff`/external handling in `git.rs`/`diff_cmd.rs` | yes |
| #3571 | prettier `--check` writes to stderr; filter both reads stdout only and skips `[warn]` | [prettier_cmd.rs:65](fork-core/rtk/src/prettier_cmd.rs:65) skips `[warn]` | yes |
| #3540 | report how many noise dirs `rtk ls` omitted; keep `(empty)` honest | [ls.rs:164](fork-core/rtk/src/ls.rs:164), [:186](fork-core/rtk/src/ls.rs:186) | yes |
| #3563 (part 2) | `exclude_commands` must apply to `head`/`tail` | [registry.rs:1457](fork-core/rtk/src/discover/registry.rs:1457) `rewrite_line_range(cmd)` takes no exclusion list | yes |
| #3566 | prisma migration counts reported as `0 applied, 0 pending` | [prisma_cmd.rs:304](fork-core/rtk/src/prisma_cmd.rs:304) | `verify` |
| #3567 | `rtk lint src` reads a bare path as a linter name and invents a verdict | [lint_cmd.rs:58](fork-core/rtk/src/lint_cmd.rs:58) has the same heuristic | `verify` |
| #3572 | space-separated `--reporter list` leaves `list` as a positional filter | no `--reporter` handling found in `playwright_cmd.rs` | `verify` |
| #3551 | gradle rewrites lose the invoked executable (`./gradlew` → `gradle`) | we have `gradle.toml`, no `gradlew_cmd.rs` | partial |

**Effort:** S each. **Risk:** low. Schedule as one batch after the P0 block.

---
## 6. Part C — explicitly **not** adopted, because HZR is already ahead

Recording these matters as much as the adoption list: they are the items a future sync would
otherwise re-import and regress.

| Upstream item | Why we skip it |
|---|---|
| PR #3583 *rewrite commands with shell wrappers* | Covered by `E2ShellWrapper` with `MAX_SHELL_WRAPPER_DEPTH = 3` and `shell_wrapper_is_ambiguous` ([registry.rs:692](fork-core/rtk/src/discover/registry.rs:692), [:753](fork-core/rtk/src/discover/registry.rs:753)) and by the 85-case fixture. Upstream is catching up to us. |
| PRs #3590 / #3591 / #3592 *path-prefixed commands, `RTK_BIN`* | Covered by basename normalisation + `absolute_matches_path_executable` ([registry.rs:398-416](fork-core/rtk/src/discover/registry.rs:398)) and `E4ExecutablePath`. |
| PR #3581 *rewrite producers before display pipelines* | Covered by `E5PipelineOrRedirect` and the bounded-pipeline rewrite plan shipped in 0.4.6. |
| PR #3554 *don't drop commands chained after a pipeline* | Fixes `rtk discover` accounting. HZR does not use `rtk discover` for accounting; the HZR ledger is the source of truth. Not applicable. |
| PR #3596 *preserve argv in `rtk test`* | Already done — `FORK_PARITY.md`: "Generic test/error routes preserve argv and exact child exit status." |
| PR #3563 part 1 *`head -n N` verbatim prefix* | Already ahead: our `--max-lines` is an exact head (`keep_head_lines`, [read.rs:771](fork-core/rtk/src/read.rs:771)), with no `smart_truncate` in the tree. Only the `exclude_commands` half applies (B10). |
| PR #3629 *sqlite3 TOML filter* | HZR 0.4.6 already ships a native SELECT-only SQLite route (`sqlite_cmd.rs`), which is stricter. |
| PR #3542 *dedupe `gh pr checks --watch`* | No `format_pr_checks` in our `gh_cmd.rs`. Not applicable. |
| PR #3555 *AWS EKS query-shaped responses* | No `eks` handling in our `aws_cmd.rs`. Not applicable. |
| PR #3568 *rake non-Minitest passthrough* | No `rake_cmd.rs`. Not applicable. |
| Telemetry work (`telemetry.rs` +275 lines upstream) | HZR runs with `RTK_TELEMETRY_DISABLED=1` by design. |
| Copilot self-heal (`d1f7139`, `db31da9`, #3630), Mistral Vibe hooks (`d480f1e`), Windows hook path (#3619, #3614, #3582) | HZR owns its own hook installer, dispatcher and integrity model. Adopting upstream host logic would create a second installer. Host *coverage* is a separate product decision — see §7. |

---

## 7. Coverage expansion — decisions required, not recommendations

These are capability gaps, not defects. Each is real work with no correctness argument behind it,
so none is scheduled without an explicit answer.

**C1 — Native command modules upstream has and we do not:** `mvn`, `gradlew`, `sbt`, `dotnet`
(+`dotnet_trx`, `dotnet_format_report`, `binlog`), the PHP suite (`phpunit`, `phpstan`, `pest`,
`paratest`, `pint`, `ecs`, `artisan`), Ruby (`rake`, `rspec`, `rubocop`), `uv`, `glab`, AWS `ecs`.
Roughly 12k lines upstream including tests and fixtures.
*Assessment:* the working stack in this environment is Rust + bun/React. The only two with a
plausible near-term return are **`uv`** (Python tooling is common in adjacent repos) and
**`glab`** (PR #3533 additionally routes `mr note` sub-commands and compresses note lists; the
`jsonpack` from B1 would apply to `glab api` unchanged). The rest is speculative surface area.
**Recommend: `uv` + `glab` only if asked; skip the JVM/.NET/PHP/Ruby suites.**

**C2 — Five `pulumi-*` TOML filters** (`preview`, `up`, `destroy`, `refresh`, `stack`). Pure data,
no code. Cheap to take, worthless if pulumi is not used. **Recommend: take with B7 if pulumi is in
scope, otherwise skip.**

**C3 — Additional agent hosts:** Codex direct command rewriting (PR #3552, +951/−149), Mistral Vibe
(merged `d480f1e`), Prime Agent extension (PR #3587, +703/−17). These are *host* integrations, and
HZR's hook installer already owns the Claude Code path. Adopting any of them means teaching the
HZR installer a new host, not merging upstream's file. **Recommend: only if Codex is a target host.**

**C4 — Windows track:** PRs #3613, #3615, #3616 (native `ls`/`tree`/`wc`, +1503/−165), #3617, #3618
(pick the right shell instead of hardcoding `cmd /C`), #3619, #3620 (PowerShell cmdlet
classification), #3622, #3623 (installer + winget/scoop/chocolatey), plus A7. `FORK_PARITY.md`
lists the Windows artifact as an open ⚪ gate. This is a coherent ~4k-line body of work that only
pays off if that gate is opened. **Recommend: defer as one block; do not cherry-pick pieces.**
Note that #3616's *pattern* — implementing `ls`/`tree`/`wc` natively instead of spawning a child —
is a genuine latency win on every platform and could be lifted independently.

---

## 8. Architectural item — upstream's streaming filter framework

Upstream's `src/core/stream.rs` is 1,259 lines against our 248. Beyond `read_lines_lossy` (A1) it
carries a reusable framework we do not have:

- `BlockHandler` + `BlockStreamFilter` — block-oriented streaming with `emit_block`/`feed_line`/`flush`
- `LineHandler` + `LineStreamFilter` — line-oriented streaming with `should_skip`/`observe_line`
- `RegexBlockFilter` — a declarative builder (`new(tool, start_pattern).skip_prefixes(&[…])`)
- `StdinFilter`, `CaptureResult::combined()`/`success()`

The value is not the code; it is that a filter written against these traits is **streaming by
construction** — bounded memory and first-byte latency — instead of `CaptureOnly`. B2 and B9 are
both symptoms of filters that were written buffered because writing them streaming was harder.

**Recommendation:** do **not** port the framework in 0.4.7. Land A1 + B2 + B9 first, then decide in
0.4.8 whether to adopt the traits as the default authoring surface for new filters. Porting a
framework and its consumers in the same release as six correctness fixes is how a parity gate goes
red.

---

## 9. Proposed sequencing

**0.4.7 — fidelity and privilege (no new surface)**

1. A1 lossy per-line decode + the `32dc612` signal-diagnostic half of the capture centralisation
2. A2 `sudo` no-rewrite + typed tracked bypass class
3. A3 owner-only data files, fork-core **and** HZR ledger/daemon audit
4. A5 `QuoteScan` + cross-line `[[ ]]` bail (+ B10/#3600 lone `\r`)
5. A4 `git log` argument model (+ B10/#3601 `restore_double_dash`)
6. B2 bounded search/grep capture
7. B3 exit-code guard across tool filters

**0.4.7.x or 0.4.8 — savings**

8. B4 `ignore_dirs`/`ignore_files` wiring
9. B5 `find` guard + tee tail hint + unfilterable passthrough
10. B7 declarative filter corrections (TOML only)
11. A6 tee slug shortening
12. B6 `pass_through_if_args`
13. B8 quiet informational stderr
14. B10 remaining batch (after `verify` on the four marked items)

**Gated on approval, separate release**

15. B1 `gh api` packing → observation window → `gh --json` behind a flag
16. §7 decisions
17. §8 framework

Rationale for the order: every 0.4.7 item either prevents output loss or prevents a false claim.
None of them increases savings, and A4 will slightly *reduce* measured savings on `git log --stat`
— which is the correct direction, and must be stated in the release notes so the ledger delta is
not read as a regression.

---

## 10. Acceptance gates

Beyond the standing gate in `FORK_PARITY.md` (`cargo fmt`/`clippy -D warnings`/full test suite/
`scripts/verify-fork-core.sh --test`/bundle smoke):

1. **Fidelity probe.** A child emitting invalid UTF-8 mid-stream loses nothing on any route
   (A1). Asserted per `FilterMode`.
2. **Privilege probe.** No file under the HZR data root is root-owned after a session containing
   `sudo <covered-command>`; the row is present in the ledger with a typed class (A2, A3).
3. **`git log` shape matrix.** For `--stat`, `--numstat`, `--shortstat`, `-p`, `--raw`,
   `--grep <flag-lookalike>`, `-- -p`, `-U5`: byte-identical to raw git, exit code preserved (A4).
4. **Exit-code matrix.** For each filter touched by B3: non-zero child exit never yields an
   all-green summary; exit status propagates verbatim.
5. **Memory bound.** Search over a 64 MiB match set under `ulimit -v` completes; peak retention at
   `RAW_CAP` (B2).
6. **Anti-evasion fixture** grows from 85 to ≥89 cases and stays green.
7. **Current-engine identity** re-recorded: `CURRENT_ENGINE.toml`, `CURRENT_ENGINE_V1.tsv`,
   `CURRENT_FILES`, `CURRENT_SHA256SUMS`, and the `FORK_PARITY.md` matrix updated with the new
   inherited-Clippy ratchet count.
8. **Ledger delta explained.** The release notes state which items move measured savings and in
   which direction, before the numbers are published.

---

## 11. Approval checklist

Tick to schedule. Nothing below is started without a tick.

**P0 — recommended, 0.4.7**

- [ ] A1 — lossy per-line child-output decoding (`stream.rs`)
- [ ] A2 — never rewrite `sudo`-prefixed commands; typed tracked bypass instead
- [ ] A3 — owner-only data files (fork-core + HZR ledger/daemon)
- [ ] A4 — `git log` argument model + `restore_double_dash`
- [ ] A5 — `QuoteScan` + cross-line `[[ … ]]` bail + lone-`\r` split
- [ ] B2 — bounded search/grep capture (agent-OOM class)
- [ ] B3 — never-worse **exit-code** guard across tool filters

**P1 — recommended, 0.4.7.x**

- [ ] B4 — wire `ignore_dirs` / `ignore_files`
- [ ] B5 — `find` guard against the capped listing + tee tail hint + unfilterable passthrough
- [ ] B7 — declarative filter corrections (turbo, quarto, bundle-install, gcc, spring-boot, liquibase, ssh)
- [ ] A6 — tee slug shortening

**P2 — optional**

- [ ] B6 — `pass_through_if_args` in the TOML engine
- [ ] B8 — quiet informational stderr
- [ ] B9 — stream `npm`/`npx` (after `verify`)
- [ ] B10 — small correctness batch (curl `-sS`, git commit `--`/hook failure/external diff driver, prettier stderr, `ls` hidden count, `head`/`tail` exclusions, + 4 `verify` items)

**Gated — separate decision**

- [ ] B1 — `gh api` lossless packing (then observation window, then `gh --json` behind a flag)
- [ ] C1 — `uv` and/or `glab` native modules
- [ ] C2 — five `pulumi-*` TOML filters
- [ ] C3 — Codex / Vibe / Prime Agent host coverage
- [ ] C4 — Windows block (incl. A7 code-page decoding)
- [ ] §8 — streaming filter framework as the authoring surface (0.4.8+)

---

## 12. Honest boundaries

- Upstream's measured figures (B1 savings table, B2 memory and wall-clock) are **upstream's**,
  taken on their fixtures. They are not reproduced here and must not be quoted as HZR results.
- Four items in B10 are marked `verify`. They are listed for completeness and are not schedulable
  until the predicate is read on our side.
- This comparison covers `src/` and `src/filters/`. Upstream's `docs/`, `hooks/`, `Formula/`,
  `openclaw/` and `scripts/benchmark.sh` were reviewed only far enough to classify them; HZR owns
  those surfaces independently.
- Open PRs are unreviewed upstream work. #3580 in particular is 1,821 lines from an outside
  contributor with an adversarial-review history in the thread. Adopting it means owning it —
  the recommendation in B1 is deliberately staged for that reason.
- No claim is made that adopting every item would close the gap permanently. Upstream merges
  roughly 15–20 commits a week; the maintainable posture is a recurring bounded sync of this
  shape, not a one-time catch-up.
