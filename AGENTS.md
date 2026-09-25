# HZR engineering rules

- HZR is a new product. Upstream RTK, ICM, grepai and Caveman are engines with preserved provenance.
- `fork-core/rtk` is the HZR-owned evolvable engine. Develop inherited RTK functionality here, never in `/Users/andrew/Programming/rtk`.
- Tag `v0.1.0` and snapshot v2 `f4296ec4...` are the immutable import baseline. Preserve their provenance while recording and testing every later engine delta.
- Never replace inherited engine behavior with a reduced reimplementation. Engine changes require parity documentation and the complete deterministic regression gate.
- The workspace `Cargo.toml` owns the current product version; `v0.1.0` remains the immutable import baseline.
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

# HZR (managed)

`hzr` filters command output and owns `rtk`, `grepai`, `icm`; never call those binaries
directly. HZR installs no Codex hooks, so run shell commands as `hzr exec run '<cmd>'`
to get filtered output; native file tools stay the default.

| Need | Use |
|---|---|
| File outline or line range | `hzr read <file> --outline`; or `--from A --to B`; native Read stays the default for whole files. |
| Search by intent | `hzr search "<intent>"`; `--mode exact` for a known literal. |
| Atomic multi-file edit (optional) | `hzr write batch ...`; native Edit/Write stay the default. |
| Durable decisions and fixes | `hzr memory recall\|store`; project scope by default; `--scope global` only for user-wide preferences. |
| Unfamiliar or cross-cutting area | `hzr context plan "<intent>"`; bound with `--max-tokens N`. |
| Shell command outside the hook | `hzr exec run '<cmd>'`; hook-routed Bash is already managed. |
| Exact unfiltered output | `HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr exec run '<command>'`; reason: binary, checksum, machine_protocol, complete_log, full_patch or verbatim_source. |

Filtered or bounded output always names its recovery command; never treat it as
complete. Use MCP `hzr_*` tools only when the server reports this worktree. If enabled
in `hzr settings`, `hzr delegate` runs scoped tasks; review its results. Policy details:
`HZR.md` (read with `--outline` first).

Write concisely: lead with the result, skip restatement and tool recaps, keep code,
paths, errors and numbers exact.

<!-- hzr:end managed agent contract -->
