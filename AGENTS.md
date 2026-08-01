# HZR engineering rules

- HZR is a new product. Upstream RTK, ICM, grepai and Caveman are engines with preserved provenance.
- `fork-core/rtk` is the HZR-owned evolvable engine. Develop inherited RTK functionality here, never in `/Users/andrew/Programming/rtk`.
- Tag `v0.1.0` and snapshot v2 `f4296ec4...` are the immutable import baseline. Preserve their provenance while recording and testing every later engine delta.
- Never replace inherited engine behavior with a reduced reimplementation. Engine changes require parity documentation and the complete deterministic regression gate.
- Product version is 0.3.0; `v0.1.0` remains the immutable import baseline.
- Internal component communication uses typed protocol structures; never parse human CLI output when JSON is available.
- Exactly one canonical workspace and one grepai index owner are allowed per worktree.
- ICM is supervised centrally; adapters must not spawn independent long-lived instances.
- User intent, code, commands, JSON, paths, symbols, errors and security text are exact content.
- Semantic compression creates a versioned derivative and never overwrites canonical data.
- Provider usage and estimates are separate fields. Never present estimated counters as billed savings.
- Public HZR documentation and user-facing text are written in English. Preserve imported fork-core audit and provenance artifacts in their original form unless a dedicated migration updates current-engine identity and passes the complete fork regression gate.
- Production Rust must pass cargo fmt, cargo clippy --all-targets --all-features -- -D warnings, and cargo test --all-targets --all-features.
- Do not add placeholders, TODOs, dead abstractions or suppressed lints.
