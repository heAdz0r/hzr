# HZR engineering rules

- HZR is a new product. Upstream RTK, ICM, grepai and Caveman are engines with preserved provenance.
- `fork-core/rtk` is the HZR-owned evolvable engine. Develop inherited RTK functionality here, never in `/Users/andrew/Programming/rtk`.
- Tag `v0.1.0` and snapshot v2 `f4296ec4...` are the immutable import baseline. Preserve their provenance while recording and testing every later engine delta.
- Never replace inherited engine behavior with a reduced reimplementation. Engine changes require parity documentation and the complete deterministic regression gate.
- Product version is 0.5.0; `v0.1.0` remains the immutable import baseline.
- Internal component communication uses typed protocol structures; never parse human CLI output when JSON is available.
- Exactly one canonical workspace and one grepai index owner are allowed per worktree.
- ICM is supervised centrally; adapters must not spawn independent long-lived instances.
- User intent, code, commands, JSON, paths, symbols, errors and security text are exact content.
- Semantic compression creates a versioned derivative and never overwrites canonical data.
- Provider usage and estimates are separate fields. Never present estimated counters as billed savings.
- Public HZR documentation and user-facing text are written in English. Preserve imported fork-core audit and provenance artifacts in their original form unless a dedicated migration updates current-engine identity and passes the complete fork regression gate.
- Production Rust must pass cargo fmt, cargo clippy --all-targets --all-features -- -D warnings, and cargo test --all-targets --all-features.
- TDD is optional. Use it when the user requests it or regression risk justifies the extra cycle; otherwise use proportionate verification to conserve tokens and time. Required quality gates still apply.
- Do not add placeholders, TODOs, dead abstractions or suppressed lints.

<!-- hzr:begin managed agent contract — do not edit inside -->

# HZR tool contract (managed)

`hzr` — heAdz0r's Zero-Redundancy engine — is the only control plane. Do not
invoke a separately installed `rtk`, `grepai` or `icm` binary: HZR owns those
engines internally, and a direct call creates the duplicate scan, duplicate
store and unaccounted usage this engine exists to remove.

This managed region defines tool routing only. Keep repository-specific roles,
source paths and test commands in that repository's root instruction file, not
in a user-global instruction file.

Read the full contract at `/Users/andrew/.local/share/hzr/current/share/hzr/HZR.md` only when a bounded lookup cannot resolve HZR-policy ambiguity.
Ordinary tasks must not import or read it in full. Start with `hzr read /Users/andrew/.local/share/hzr/current/share/hzr/HZR.md --outline`, then read only the relevant `--from`/`--to` range.

| Instead of | Use |
|---|---|
| `Read` | `hzr read <file>` uses the smart default; use `--outline` first for structure and ranges for exact evidence |
| `Grep` | `hzr rgai "<intent>"` or `hzr search "<intent>" --mode auto`; use `--mode exact` only for a known literal |
| `Edit`/`Write` | `hzr write patch\|replace\|set\|create\|batch ...` |
| memory | `hzr memory recall\|store` |
| context | `hzr context plan "<intent>"` |
| shell command | `hzr exec run '<shell command>'`; canonical policy selects the filtered route and preserves shell grammar |
| explicit unfiltered recovery | `HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr rtk -- raw <command...>`; reason must be `binary`, `checksum`, `machine_protocol`, `complete_log`, `full_patch`, or `verbatim_source` |
| optional TDD | `hzr tdd` only when user/repository policy or regression risk justifies test-first overhead |
| build this project | `hzr build <args>` (not `hzr release`, which rebuilds HZR) |

For agent-originated shell work, `hzr exec run` is the default. If
`hzr exec rewrite '<shell command>'` returns `allow_rewrite`, `raw` is forbidden.
Do not choose `raw` merely because a command uses SSH, JSON, pipes, redirects or
unfamiliar arguments. When no filter exists, `hzr exec run` performs the tracked
fallback without requiring the agent to select `raw`. POSIX shell launchers, env prefixes,
and simple Python file/JSON/subprocess wrappers do not bypass policy: HZR rewrites the
proven leaf or returns `Ask`. Opaque computation/migration remains tracked with zero
savings credit.

For a plain argv command whose output intent is known, the existing
`hzr rtk -- test`, `err`, `summary` and `log` routes provide bounded
generic filtering. Do not use them to reconstruct pipes, redirects or
other shell grammar; keep those commands on `hzr exec run`.

Unbounded `read --level none` defeats the smart default and is automatically reduced.
Prefer `--outline` for structure and `--from`/`--to` for exact evidence. Use
`HZR_EXACT_FIDELITY=1 hzr read <file> --level none` only when the whole file
is authoritative input that cannot be bounded. Search defaults to `--mode auto` for
discovery; `--mode exact` remains the escape hatch for a known symbol, error, key, or
audit literal.

TDD is opt-in, not the default. When token or time efficiency matters, skip it
and use proportionate verification; repository-required quality gates still apply.

`read -n` defaults to exact content and preserves source coordinates, including
ranged and tail reads. `--max-lines N` is the exact head equivalent. `--outline`
returns Markdown headings or heuristic symbols for Rust, Python, TypeScript,
JavaScript, Go and Java. For several files, use
`hzr read --batch --max-tokens N <files...>`; it preserves order and coordinates and
emits exact recovery ranges for omitted content.

Batch writes are atomic and idempotent per file; independent file groups can fail separately,
so inspect every operation result. Batch is not an all-files transaction.

## Memory scopes

One store, two namespaces. `--scope project` (the store default) is for facts about
*this repository*. `--scope global` is for facts about the **user** — a preference or
a standing rule that should apply in every repository, so it does not have to be
restated per project. Recall defaults to project + global; another repository's
memory is never reachable from any scope.

## MCP tools

If the `hzr` MCP server is registered, prefer its tools over the CLI — same
single store and index, and the calls are accounted:

| Tool | Use it for |
|---|---|
| `hzr_context_plan` | Build bounded graph-first evidence for unfamiliar or cross-cutting work. |
| `hzr_search` | Find code by intent (`mode: semantic`) or exactly (`mode: exact`). |
| `hzr_memory_recall` | Recall decisions, resolved errors and prior context before re-reading files. |
| `hzr_memory_store` | Persist a decision, resolved error or finished work. Not ephemeral state. |
| `hzr_memory_update` | Replace one superseded memory after namespace ownership is verified. |
| `hzr_memory_forget` | Delete one invalid memory after namespace ownership is verified. |
| `hzr_memory_prune` | Preview or remove low-weight memories in one namespace; preview is the default. |
| `hzr_codec` | Apply or shadow-measure protected response-density transforms. |

MCP inputs are strictly validated and results include typed `structuredContent`.
`isError: true` means no success was confirmed and no fallback engine or store
was used. If a store transport fails after dispatch, recall before retrying because
completion may be unknown.
MCP is client-managed stdio: `hzr init` never starts it. Run `hzr install --force`
once to register it, and `hzr mcp status` to audit native client launch state.
`hzr mcp config --client codex\|claude-desktop --workspace <dir> --apply` writes a pinned registration; omit `--apply` to print a paste snippet. Never
register `icm`, `grepai` or `rtk` as your own MCP server: each direct launch adds
another writer to the store HZR supervises and leaks orphans when the session dies.

`hzr rtk -- raw <command> <args...>` directly spawns the first argument and receives
zero savings credit. It is an explicit fidelity escape hatch, not the default shell
wrapper; normal agent shell work goes through `hzr exec run '<shell command>'`. The
fidelity marker without one of the closed reasons above returns `Ask`; even a valid
reason cannot override a managed equivalent, deny, or ambiguous-policy decision. The
per-session allowance is five operations or 100,000 estimated delivered tokens; an
oversized local read or unmeasurable remote exact stream asks before execution.

The installed `PreToolUse` hook routes Bash through the managed daemon and
falls back to the same pinned fork-core when the daemon is down. A degraded
rewrite keeps command policy but is absent from the usage ledger; `hzr doctor`
and `hzr stats` report that incomplete accounting rather than hiding it.

The failure-open `PreToolUse` hook sees native `Read`, `Grep`, `Glob`, `Edit` and
`Write`. In `steer` mode it prescribes `hzr read`/`hzr search`; `Glob` and native
edits remain allowed. `strict` additionally prescribes `hzr write`, while `observe`
retains measurement-only compatibility for existing installations. The `PostToolUse`
observer stores no tool content and grants no savings credit. In `steer`/`strict`,
policy-allowed native calls are accounted as typed E10 bypasses, not hidden as
`native_unaccounted`.

<!-- hzr:end managed agent contract -->
