# HZR Honest Accounting and Bounded Output — Product Requirements Document

> [!IMPORTANT]
> **Status: historical / superseded.** This is the preserved HZR 0.3.7
> requirements and verification record, not the current product contract. Its unique W1–W8
> acceptance provenance remains authoritative for auditing that release; use the current owners
> below for new implementation and release decisions.

### Requirement migration map

| Historical requirement | Current owner |
|---|---|
| W1–W3 — accounting coverage, MCP attribution, native-tool blind spots | [`PRD_HZR_TOKEN_ECONOMY_UTILIZATION.md`](PRD_HZR_TOKEN_ECONOMY_UTILIZATION.md) and [`PRD_HZR_ANTI_EVASION_SOTA.md`](PRD_HZR_ANTI_EVASION_SOTA.md) |
| W4 and W6 — bounded recovery and effective search reporting | [`../HZR.md`](../HZR.md) and [`SOTA_RELEASE_GATES.md`](SOTA_RELEASE_GATES.md) |
| W5 — typed protocol fields must have real producers | [`../FORK_PARITY.md`](../FORK_PARITY.md) and [`SOTA_RELEASE_GATES.md`](SOTA_RELEASE_GATES.md) |
| W7 — observable progress and failure evidence | [`PRD_HZR_SOTA_OBSERVATORY.md`](PRD_HZR_SOTA_OBSERVATORY.md) |
| W8 — symlink-safe workspace identity | [`PRD_HZR_PROJECT_ACTIVATION.md`](PRD_HZR_PROJECT_ACTIVATION.md) |

**Historical implementation status:** implemented and release-verified for HZR 0.3.7

**Surface:** usage ledger, `hzr stats`, MCP gateway, `rtk read`, `hzr search`, `hzr-protocol`, `install.sh`, `scripts/build-bundle.sh`

**Origin:** a six-item retrospective on building an agent-side token optimizer. Each item is
treated here as a claim to verify against the current tree, not as a feature request. Four of
the six are already implemented in HZR; the value of this document is the residual set —
the places where the same failure mode is still reachable.

## 1. Product intent

HZR sells one number and one behavior: a defensible reduction estimate, and output that an
agent never has to re-read. Both are integrity properties, not features. A single unaccounted
channel makes the number fiction; a single silent truncation makes the agent pay twice. This
PRD closes the remaining paths to both, and deletes the protocol surface that promises data
nobody fills.

## 2. Verification summary

| # | Retrospective item | Verdict | Residual work |
|---|---|---|---|
| 1 | A bounded read must describe its own bounds | Mostly implemented | W4 — `--max-lines`/`--tail-lines`, long-line truncation, `memory explore` caps, `hzr_memory_recall` |
| 2 | Bypassed calls must sit in the baseline | Partially implemented | W1, W2, W3 — passthrough records `0/0`; MCP tools write no ledger rows; native host tools have no counter at all |
| 3 | `mv -f new current` follows a directory symlink | Implemented, with a smoke test | W8 — one inconsistent call site, one missing assertion |
| 4 | "Exact" must be literal | Implemented in the engine, with tests | W6 — the effective mode is not reported to the caller; literal query is trimmed |
| 5 | Never-filled protocol fields are worse than absent | Partially implemented | W5 — `ContextCandidate.symbol` is `None` on every path; `Envelope`/`Intent` are unreachable; MCP output schemas declare no properties |
| 6 | A silent download is indistinguishable from a hang | Implemented in `install.sh` | W7 — non-TTY installs, `build-bundle.sh`, MCP long calls, daemon startup, `hzr exec run` without a timeout |

## 3. Product principles applied

- **The ratio measures everything or it measures nothing.** Every channel that delivers tokens
  to an agent enters both sides of the ratio, or is reported as an explicit coverage gap.
- **Zero is a claim.** Recording `0` delivered tokens for work that delivered output is a
  stronger misstatement than recording no row at all.
- **A bound states its own escape.** Any output that cut content names the amount cut and the
  exact command that returns it.
- **A declared field is a promise.** A field that no code path fills is removed, filled, or
  paired with a typed reason for its absence.
- **Fork-core parity is a cost, not a blocker.** Engine-level fixes are scoped separately so
  that product-level fixes are not gated on the full parity regression gate.

## 4. Workstreams

### W1 — Passthrough operations must dilute the ratio (P0, fork-core)

**Evidence.** `TimedExecution::track_passthrough` records `input_tokens = 0, output_tokens = 0`
(`fork-core/rtk/src/tracking.rs:1301-1323`) with the comment *"input_tokens=0, output_tokens=0
won't dilute savings statistics"*. It has eleven production call sites, including the raw
parse-failure fallback (`fork-core/rtk/src/main.rs:1824`), `npx` passthrough (`main.rs:1582`),
`search.rs:466`, `git.rs:1800`, `gt_cmd.rs:190`, `gh_cmd.rs:1125`, `curl_cmd.rs:40` and three in
`container.rs`. `Ledger::bypass_summary_scoped` sums `output_tokens` for both the bypass subset
and the total (`crates/hzr-core/src/ledger.rs:637,646`), so a `0/0` row is invisible on both
sides of the ratio.

**Why it matters.** A bypass is supposed to raise both sides and cancel out
(`HZR.md`, "The cost of `raw`"). A passthrough delivers real bytes to the agent and contributes
nothing to the denominator, so the headline reduction stays high precisely when the session
routed around the optimizer. This is item #2's failure mode, with an inverted comment sitting on
top of it.

**Requirements.**

- FR-W1.1 — Passthrough invocations that can be captured are captured and recorded with equal
  input and output token estimates, matching `Commands::Proxy` (`fork-core/rtk/src/main.rs:3174-3179`).
- FR-W1.2 — Passthrough invocations that stream to an inherited stdio and genuinely cannot be
  measured are recorded with a distinct `unmeasured` marker, counted in a new
  `unmeasured_bypass_operations` counter, and never recorded as `0` delivered tokens.
- FR-W1.3 — `hzr stats` renders the unmeasured count directly under `OPTIMIZER BYPASS`, with the
  same "the ratio measures a shrinking fraction" wording used for coverage gaps.
- FR-W1.4 — The comment at `tracking.rs:1310` is replaced by the accounting rule it currently
  contradicts.

**Acceptance.** A recorded session of `hzr rtk -- raw`, `npx`, and a git passthrough shows a
reduction estimate that decreases as passthrough volume grows; a regression test asserts a
passthrough row never has `output_tokens = 0` unless it is flagged unmeasured.

**Cost.** Touches the inherited engine, so it requires parity documentation and the complete
deterministic fork regression gate (`AGENTS.md`; `scripts/verify-fork-core.sh`).

### W2 — First-class MCP tools must write ledger rows (P0)

**Evidence.** Only `codec_compile` records an operation (`crates/hzr-daemon/src/api.rs:1540`
→ `record_codec_operation`, `api.rs:1550-1571`). The handlers `search` (`api.rs:1017`),
`context_plan` (`1038`), `memory_recall` (`1058`), `memory_store` (`1139`), `memory_forget`
(`1175`), `memory_update` (`1199`), `memory_prune` (`1240`), `exec_run` (`1341`) and `fork_run`
(`1431`) never call `LedgerWriter::record_operation`
(`crates/hzr-daemon/src/ledger_writer.rs:86`).

**Why it matters.** `HZR.md` tells agents to prefer MCP tools because "they reach the same single
store and index as the CLI, and their results are accounted". For everything except `hzr_codec`
that sentence is currently false. MCP calls do not pass through the `Bash` `PreToolUse` hook
either (`crates/hzr-cli/src/hook_runner.rs:33-37`), so an agent that follows the contract
perfectly produces a ledger that reflects almost none of its work — and `OPERATIONS` in
`LOCAL OUTPUT REDUCTION` undercounts the channel HZR most wants credit for.

**Requirements.**

- FR-W2.1 — Every MCP-reachable daemon handler records one operation row with a request-derived
  baseline estimate and a response-derived delivered estimate, reusing `estimated_tokens`
  (`api.rs:1576-1578`) rather than a third heuristic.
- FR-W2.2 — Rows carry a channel tag distinguishing `mcp` from hook-rewritten CLI traffic, so
  `hzr stats` can show the split instead of merging it into one opaque total.
- FR-W2.3 — Retrieval tools that legitimately receive no savings credit (context selection,
  memory recall) record equal baseline and delivered values, so they neither inflate nor deflate
  the ratio, consistent with the note already rendered at `crates/hzr-cli/src/stats.rs:219`.
- FR-W2.4 — When the daemon is unavailable and the row cannot be written, a
  `daemon_unavailable_operations` counter increments and surfaces in `ACCOUNTING COVERAGE`.

**Acceptance.** An MCP-only session (no `Bash` calls) produces a non-empty `hzr stats` with a
visible `mcp` channel; a test asserts each handler writes exactly one row per successful call and
none on a validation failure.

### W3 — Measure the native-tool blind spot (P0)

**Evidence.** The hook matches only `Bash`, `Agent` and `Task` (`hook_runner.rs:33-37`). The
previous generation of blocking hooks for native `Read`/`Grep`/`Write` is now actively removed on
every install (`fork-core/rtk/src/init.rs:87-92,183-193`). No `PostToolUse` registration exists
anywhere in the tree. `HZR.md` states the consequence honestly — "a session that reads files
natively shows a high reduction over a small measured fraction of what it actually spent" — but
ships no instrument that measures how small that fraction is.

**Why it matters.** This is the largest remaining distance between the headline number and
reality, and it is the one number a user cannot estimate for themselves.

**Requirements.**

- FR-W3.1 — Installation registers a `PostToolUse` observer for `Read`, `Grep`, `Glob`, `Edit`
  and `Write` that mutates nothing, blocks nothing, and returns no steering payload.
- FR-W3.2 — The observer records a row on a third route, `OperationRoute::NativeUnaccounted`
  (`crates/hzr-core/src/operation.rs:45-160`), never folded into `Bypass` or `Optimized`.
- FR-W3.3 — `hzr stats` reports measured coverage as a share: accounted operations versus
  accounted plus native-observed operations, so the headline ratio is always read next to the
  fraction of traffic it covers.
- FR-W3.4 — The observer is failure-silent and adds no measurable latency to a host tool call;
  a hook error never surfaces as a tool error.
- FR-W3.5 — `HZR.md` replaces its prose disclaimer with a pointer to the reported coverage share.

**Acceptance.** A session using only native file tools reports a coverage share near zero
instead of an unqualified high reduction.

**Open decision.** Whether the observer is installed by default or opt-in. Default-on maximizes
honesty; opt-in avoids writing a second hook entry into user settings. Recommendation: default-on,
because a coverage number that is only present when requested is the same failure as the missing
baseline.

### W4 — Every bound states its own escape (P1)

**Already done.** Markdown digest omission notice with two recovery commands
(`fork-core/rtk/src/read_digest.rs:635,663-685`); generated-file and tabular digests
(`read_digest.rs:67,204,420,626-631`); column omission count (`read_digest.rs:285-288`);
`hzr search` `+N more` notes plus a literal `tail -n +N` recovery command
(`fork-core/rtk/src/search.rs:663-682`, `tee.rs:273-280`); typed `total_hits`/`shown_hits`
(`crates/hzr-cli/src/mcp/tools.rs:279-285`); `outline_omitted` + `outline_recovery`
(`crates/hzr-context/src/candidate.rs:87-100`); capped warning lists with an explicit
`WarningsTruncated` marker (`crates/hzr-context/src/planner.rs:1104-1116`).

**Residual gaps.**

- FR-W4.1 — `--max-lines` / `--tail-lines` (`keep_head_lines`/`keep_tail_lines`,
  `fork-core/rtk/src/read.rs:438-453`, applied at `read.rs:213-217`) append a footer stating the
  file's total line count, the number of lines omitted, and the exact `--from N --to M` command
  for the remainder. The existing `Lines: X -> Y` notice is written to stderr and computed before
  the window is applied, so it does not satisfy this.
- FR-W4.2 — `truncate_long_lines` (`read_digest.rs:801-829`) reports how many lines were shortened
  and names `--level none` as the recovery, instead of leaving a bare `…` per line.
- FR-W4.3 — `hzr rtk -- memory explore` states `N of M shown` for each capped list and names
  `--detail` as the lever (`fork-core/rtk/src/memory_layer/renderer.rs:481-511,532-669`).
- FR-W4.4 — `hzr_memory_recall` returns a typed `total_matches` alongside `count`, so a caller can
  tell a complete result set from one capped at `limit` (`crates/hzr-cli/src/mcp.rs:604-616`,
  `mcp/tools.rs:49-57`).
- FR-W4.5 — `bounded_memory_content` (`candidate.rs:307-325`) states the omitted byte count and the
  memory id needed to fetch the full record.
- FR-W4.6 — `hzr_search` `structuredContent` carries a `next_step` recovery string in the shape
  already established by `outline_recovery`.
- FR-W4.7 — A `--from/--to` read that stops before EOF reports the file's total line count, so a
  caller can distinguish a clipped range from a range that reached the end
  (`fork-core/rtk/src/read_source.rs:35-68`).

**Acceptance.** For each path above, a test asserts the recovery command printed in the footer,
when executed verbatim, returns the omitted content.

**Cost.** FR-W4.1 through FR-W4.3 and FR-W4.7 are engine changes and carry the fork parity gate.
FR-W4.4 through FR-W4.6 are product-level.

### W5 — No dead protocol fields (P1)

**Evidence.**

- `ContextCandidate.symbol` (`crates/hzr-protocol/src/lib.rs:247`) is assigned `None` at every
  production site — `candidate.rs:147`, `:220`, `:279` — and `Some(_)` nowhere in the tree. The
  line-span half of the original defect was fixed (`candidate.rs:136-137,202-213`); the symbol half
  was not. A search hit that matched inside one function still never says which one.
- An outline miss is silent: when `candidate_outlines` (`planner.rs:431-458`) cannot produce an
  outline — unsupported language, unreadable file, engine failure — the candidate degrades to a
  bare path, which is exactly the pre-fix behavior, and `ContextWarningCode`
  (`crates/hzr-protocol/src/api.rs:97-107`) has no variant to say so.
- `Envelope<T>`, `Intent`, `TokenBudget` and `Provenance` (`lib.rs:183-230`) are constructed only in
  the crate's own unit test (`lib.rs:368-383`). A typed public request envelope that no handler
  feeds is the same defect at API scale.
- MCP output schemas declare nested objects and arrays as bare `{"type": "object"}` /
  `{"type": "string"}` with no `properties` and no `required`
  (`crates/hzr-cli/src/mcp/tools.rs:54,281,343-347`), so a client cannot even discover that a
  candidate has a `symbol` field, and the class of defect is unverifiable from the contract.

**Requirements.**

- FR-W5.1 — `symbol` is resolved from the already-fetched outline and the candidate's line span
  and filled for every candidate whose span falls inside a known symbol.
- FR-W5.2 — When no symbol can be resolved, the candidate carries a typed reason rather than a bare
  `None`; a candidate is never emitted with an unexplained empty `symbol`.
- FR-W5.3 — Outline unavailability emits an aggregate `ContextWarning` ("outline unavailable for
  N of M candidates") through the existing `push_warning` path used by `ContentUnavailable`
  (`candidate.rs:193-199,253-260`).
- FR-W5.4 — `Envelope`, `Intent`, `TokenBudget` and `Provenance` are either wired into the request
  path they were designed for or removed from the public crate; they do not ship as an unreachable
  API for a third release.
- FR-W5.5 — Every MCP output schema declares the properties and `required` sets its handler
  actually emits, and a test compares emitted `structuredContent` against the declared schema for
  each tool.

**Acceptance.** No production construction of `ContextCandidate` leaves `symbol` empty without an
attached reason; the schema-conformance test fails if a handler emits an undeclared field or omits
a required one.

### W6 — The caller must see which search actually ran (P2)

**Already done.** `--mode exact` builds a whole, case-preserved literal
(`fork-core/rtk/src/rgai_cmd.rs:1469-1476`), scores by case-sensitive containment
(`rgai_cmd.rs:1355-1364,1416-1422`), never shells to a regex engine on the default path
(`rgai_cmd.rs:134,191,215-228`), escapes metacharacters when `rg` is used for `--files`
(`rgai_cmd.rs:683-694`), withholds `-i` (`rgai_cmd.rs:826-830`), and is covered by six unit tests
(`rgai_cmd.rs:2274-2350`) plus a dispatch test asserting only `Exact` sends `--literal`
(`crates/hzr-context/src/planner.rs:1272-1296`).

**Residual gaps.**

- FR-W6.1 — `SearchApiResponse` gains an explicit `effective_mode`, and `print_search`
  (`crates/hzr-cli/src/output.rs:82-119`) prints it. `strategy`
  (`ForkRgaiAdaptive`/`ForkRgaiBuiltin`) is an implementation label and is not a documented mode
  indicator.
- FR-W6.2 — `fallback_reason` is printed in text output. It is set when a `semantic`/`auto` request
  is silently forced to `Exact` because the grepai lifecycle is unavailable
  (`planner.rs:151-171`) and is currently visible only under `--json`
  (`crates/hzr-protocol/src/api.rs:66`).
- FR-W6.3 — The zero-hit branch prints the same diagnostics as the hit branch —
  `scanned_files`, `skipped_large`, `skipped_binary`, `index_generation`, `fallback_reason`.
  Today `no matches` (`output.rs:104`) hides the fact that a file was skipped for size, which is
  indistinguishable from a genuine absence.
- FR-W6.4 — The literal query is not trimmed. `build_literal_query_model` applies `query.trim()`
  (`rgai_cmd.rs:1470`) and `score_line` compares against `line.trim()` (`rgai_cmd.rs:1348`), so
  `"foo "` matches `foobar`. Trimming is a display concern only.
- FR-W6.5 — `HZR.md` states that literal matching is per-line, so a multi-line signature cannot be
  matched literally (`analyze_file`, `rgai_cmd.rs:1169`).

**Acceptance.** Every search result — including the empty one — names the mode that ran, and a
degraded ranked search is never silently reported as a normal one. FR-W6.4 carries the fork parity
gate.

### W7 — Work must be distinguishable from a hang (P2)

**Already done.** `hzr_step()` numbering with an accurate five-step total across both branches
(`install.sh:43-50,204-296`); progress explicitly enabled for the large archive download
(`install.sh:212`); a closing summary naming every install location (`install.sh:330-376`);
bounded `hzr update` network timeouts and a 2 GiB archive cap
(`crates/hzr-cli/src/update.rs:19-23,188-197`); a bounded default `hzr agent run` timeout
(`crates/hzr-cli/src/cli.rs:512-517`, `crates/hzr-agent/src/runner.rs:127-133`); MCP
`notifications/cancelled` (`crates/hzr-cli/src/mcp.rs:334`).

**Residual gaps.**

- FR-W7.1 — Non-TTY installs emit periodic transfer progress. `[ -t 1 ]` gates the progress bar
  (`install.sh:110,118`), so a CI log shows `[1/5] Downloading …` followed by silence for the whole
  transfer — the original failure mode, unchanged, in the environment least able to tolerate it.
- FR-W7.2 — Step `5/5` narrates its three sub-operations. `hzr init --quiet`, `hzr install --force`
  and `hzr daemon service install` (`install.sh:275-296`) run with suppressed output
  (`crates/hzr-cli/src/main.rs:1158`), so a stalled daemon start looks like a stalled script.
- FR-W7.3 — `scripts/build-bundle.sh` numbers its stages and reuses one download helper. It has
  roughly nine multi-minute stages and prints one line of its own (`build-bundle.sh:309`); its git
  clone is `--quiet` (`:34`) and `download_cached` is `--silent` (`:62-85`), a second silent
  implementation not shared with `install.sh:105-129`.
- FR-W7.4 — `download_cached` gains an overall `--max-time`. It sets only `--connect-timeout 20`, so
  a transfer that stalls after connecting hangs without bound.
- FR-W7.5 — The MCP gateway accepts `progressToken` and emits `notifications/progress` for
  `hzr_context_plan` and `hzr_search`. No progress support exists in the tree; a long plan over a
  large repository is indistinguishable from a hung server, and the client's only lever is
  cancellation.
- FR-W7.6 — `hzrd` in the foreground prints a startup line naming its bind address and index state.
  The daemon crate has no logging dependency and no startup output, so a healthy quiet daemon and a
  stuck one look identical.
- FR-W7.7 — `hzr exec run` applies a default timeout. `ExecArgs.timeout_ms` has no default
  (`crates/hzr-cli/src/cli.rs:472-483`) and the untimed branch is a bare `child.wait().await`
  (`crates/hzr-exec/src/executor.rs:430`), so a hung wrapped command blocks forever with no output.
- FR-W7.8 — `hzr agent run` streams or heartbeats during a run. Output is captured in memory
  (`crates/hzr-agent/src/runner.rs:108-133`) and nothing is emitted until completion, so a
  twenty-five-minute turn is silent inside a thirty-minute timeout.

**Acceptance.** For each long operation, a user observes output at least once per fixed interval,
or the operation is bounded by a timeout that names what expired.

### W8 — Symlink swap consistency (P2)

**Already done.** `replace_hzr_symlink()` probes `-fh` then `-fT` with a loud last-resort fallback
(`install.sh:73-100`), used for the `current` swap (`install.sh:253`) and reused verbatim by
`scripts/install-bundle.sh:60-61,81,89`. The Rust paths use `NamedTempFile::persist`, i.e.
`rename(2)`, which replaces the final path component without following it
(`crates/hzr-cli/src/adoption.rs:533-561`, `crates/hzr-cli/src/update.rs:397-411`). Stale resolved
paths are actively migrated back through `current`
(`crates/hzr-core/src/config.rs:57-70,344-381`; `crates/hzr-cli/src/service.rs:68-119` with two unit
tests). `scripts/smoke-install.sh:406-492` performs a real upgrade and asserts `current` repoints
and no `/versions/` path leaks into `hzr doctor`.

**Residual gaps.**

- FR-W8.1 — `install_hzr_link` uses `replace_hzr_symlink` instead of a bare `mv -f`
  (`install.sh:255-266`). It is safe today only because the target is never a directory symlink;
  `install-bundle.sh:89` already routes the same operation through the helper. This is consistency
  insurance against a regression of the exact defect the helper exists to prevent.
- FR-W8.2 — A smoke assertion confirms that the public `hzr` entry point is still a symlink after
  `hzr install --force` runs on top of a bundle install. `prefix::install` writes real bytes
  (`crates/hzr-cli/src/main.rs:688` → `crates/hzr-cli/src/prefix.rs:104`), which would silently opt
  a PATH entry out of all future `current`-based upgrades.

## 5. Prioritization

| Priority | Workstreams | Rationale |
|---|---|---|
| P0 | W1, W2, W3 | The reduction estimate is the product claim. Today it omits passthrough volume, all MCP traffic, and every native tool call. |
| P1 | W4, W5 | Directly causes the double-payment and useless-plan failures the retrospective describes. |
| P2 | W6, W7, W8 | Honesty and operability hardening on paths that are already correct. |

Recommended sequencing: W2 and W3 first (product-level, no engine gate, and together they define
the coverage denominator), then W5, then W1 and W4 as one fork-core change set sharing a single
parity gate, then W6 through W8.

## 6. Release gates

- `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets --all-features` (`AGENTS.md`).
- `scripts/verify-fork-core.sh` and parity documentation for W1, W4.1–W4.3, W4.7 and W6.4.
- `scripts/smoke-install.sh` extended for FR-W8.2 and FR-W7.1.
- New ledger tests: a passthrough row is never silently `0`; each MCP handler writes exactly one
  row; native-observed rows never enter the `Optimized` or `Bypass` route.
- New bounds tests: for each truncating path, the printed recovery command returns the omitted
  content when executed verbatim.
- New schema test: emitted `structuredContent` conforms to each declared MCP output schema.

All gates above passed on the 0.3.7 release candidate. The packaged clean-install and upgrade
smoke also verifies that `doctor` accepts the installed dispatcher, SessionStart initializer, and
default-on `PostToolUse` observer as the canonical three-hook layout.

## 7. Explicitly out of scope

- Reinstating blocking hooks for native file tools. They were deliberately removed
  (`fork-core/rtk/src/init.rs:183-193`); W3 measures the channel instead of policing it.
- Replacing the `bytes / 4` estimator. The estimate/billed separation is already enforced
  (`estimated_utf8_bytes_div_4_v1`, `crates/hzr-cli/src/stats.rs:189-222`); a better tokenizer is a
  separate decision from covering all channels.
- Any change to the `v0.1.0` import baseline or snapshot provenance.

## 8. Decisions required before implementation

1. **W3 default.** Ship the `PostToolUse` observer enabled by default, or opt-in?
2. **Fork-core appetite.** Accept W1 plus the W4 engine items as one parity-gated change set in
   0.3.7, or defer all engine work to 0.3.8 and ship only the product-level workstreams now?
3. **W5.4 disposition.** Wire `Envelope`/`Intent` into a real request path, or remove them from the
   public crate?
4. **W6.4 compatibility.** Dropping the literal `trim()` changes existing result sets for queries
   with trailing whitespace. Treat as a bug fix in 0.3.7, or as a documented behavior change?

## 9. Verified implementation outcome

The implementation audit found no recoverable partial commit from the interrupted agent: the
only artifact on `main` was this untracked PRD. The work below was implemented from the current
0.3.6 tree with regression-first tests.

| Workstream | Outcome in 0.3.7 | Verification note |
|---|---|---|
| W1 | Implemented | Captured raw output remains a neutral baseline; inherited-stdio passthrough is stored as `measurement=unmeasured`, `route=bypassed`, and cannot carry invented token counts. |
| W2 | Implemented | Every successful MCP tool response produces one `channel=mcp` row. Codec reuses its existing daemon row; validation failures produce none; a failed accounting write increments the local daemon-unavailable gap counter. |
| W3 | Implemented, default-on | Claude installation registers a failure-silent `PostToolUse` observer for `Read`, `Grep`, `Glob`, `Edit`, and `Write`. Native observations are excluded from the reduction ratio and included in its coverage denominator. |
| W4 | Implemented | File windows, ranged reads, shortened lines, memory exploration, MCP recall, bounded memories, and search results expose totals or recovery instructions. Recovery paths are shell-quoted. |
| W5 | Implemented with corrected scope | Search candidates resolve the smallest enclosing symbol; all empty symbols carry a typed reason; outline misses aggregate into warnings; dead `Envelope`, `Intent`, and `TokenBudget` types were removed. `Provenance` was retained because production candidates use it. MCP output schemas now declare nested properties and required fields and are checked against representative structured content. |
| W6 | Implemented | Text and structured search output report the effective mode, fallback, scan diagnostics, and next step, including zero-hit results. Exact matching preserves trailing whitespace and is documented as per-line. |
| W7 | Implemented with one corrected premise | Installer and bundle work is staged and bounded; MCP search/planning sends progress; foreground daemon startup and long agent runs emit liveness output. `hzr exec run` needed no change: the daemon already applies a request budget when the CLI timeout is omitted. |
| W8 | Implemented | All public symlink swaps use the portable helper. An identical `hzr install --force` is proven to preserve bundle symlinks rather than replacing them with copies. |

Resolved decisions: the observer is default-on; fork-core changes ship together behind the full
parity gate; unreachable protocol types are removed; preserving trailing whitespace is treated as
an exact-mode correctness fix.

The accounting terminology is intentionally strict:

- `operations` in the reduction summary means measured, non-native rows only;
- `total_observed_operations` includes measured, unmeasured, and native-observed rows;
- `accounted_operations` records control-plane visibility, while the displayed coverage share uses
  only rows eligible for the reduction ratio;
- estimated output reduction remains separate from provider-billed usage.
