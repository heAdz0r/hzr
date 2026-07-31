# HZR engineering rules

- HZR is a new product. Upstream RTK, ICM, grepai and Caveman are engines with preserved provenance.
- Product version is 0.1.0 until an explicit release change.
- Internal component communication uses typed protocol structures; never parse human CLI output when JSON is available.
- Exactly one canonical workspace and one grepai index owner are allowed per worktree.
- ICM is supervised centrally; adapters must not spawn independent long-lived instances.
- User intent, code, commands, JSON, paths, symbols, errors and security text are exact content.
- Semantic compression creates a versioned derivative and never overwrites canonical data.
- Provider usage and estimates are separate fields. Never present estimated counters as billed savings.
- Production Rust must pass cargo fmt, cargo clippy --all-targets --all-features -- -D warnings, and cargo test --all-targets --all-features.
- Do not add placeholders, TODOs, dead abstractions or suppressed lints.
