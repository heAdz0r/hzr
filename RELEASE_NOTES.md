# HZR 0.4.3 — Efficient command utilization and accountable routing

HZR 0.4.3 completes the third command-utilization pass. It closes avoidable RAW and redundant
wrapper paths, makes recent misuse visible without recording sensitive command payloads, and
aligns the installed Claude/Codex contract with the command surface agents should actually use.

The frozen audit is an output-size estimate, not provider billing. At command id 53,511, RAW
accounted for 12,868 lifetime operations and 27,961,942 estimated delivered tokens. In the
preceding seven days it accounted for 1,684 operations and 7,319,643 estimated tokens, or 58.80%
of observed output. Existing dedicated HZR routes could already cover 75.3% of lifetime RAW
output and 83.2% of recent RAW output. The release therefore emphasizes route enforcement and
observable attribution instead of inventing a new savings claim.

## What changed

- Redundant managed RAW/proxy wrappers around an existing `hzr` command are removed without
  reconstructing its payload. The explicit `HZR_RAW_FIDELITY=1` path remains available when a
  bounded replacement cannot preserve byte-for-byte semantics.
- `npm test`, `npm run test`, and `pnpm test` enter existing typed families. npm run aliases no
  longer duplicate the `run` argument; unsupported execution forms stay conservative.
- Top-level `hzr read` and `hzr write` aliases forward arguments to the pinned fork-core command
  implementations. Instructions retain the compatibility spelling only where lifecycle context
  requires it.
- `hzr stats --since <N{h|d|w}>` applies a single inclusive cutoff and reports bounded top-12
  operation-family and operation-mode panels. JSON exposes the same typed aggregates.
- Search accounting records requested and effective mode, actual Grepai/Ripgrep/Files/Builtin
  backend, closed fallback codes, and internal-versus-final stage. Final-delivery rows do not
  inflate headline totals.
- New attribution remains privacy-safe: no query text, path, command arguments, file contents,
  secrets, or arbitrary error strings are stored in the added fields. Historical rows migrate
  as nullable and remain readable.
- Managed Claude and Codex blocks list all eight MCP tools, prefer semantic/auto discovery for
  unknown code, reserve exact mode for known literals, expose bounded generic filters, and
  accurately describe native file-tool observation as measurement-only.

## Deliberate boundaries

RAW is not banned when it is the only route that preserves the requested fidelity. Native `rg`,
shell grammar, and arbitrary binary reads can differ from bounded or filtered output, so the
explicit fidelity marker remains conservative. `read --max-tokens` and new tar, rustup, yarn,
dotnet, deno, podman, or fd filters remain candidates rather than being shipped without a proven
semantic contract.

## Upgrade impact

The active bundle, daemon, MCP registration, hooks, and managed instructions continue to resolve
through `~/.local/share/hzr/current`. Refresh a workspace with:

```bash
hzr update
hzr --version
hzr init --if-needed
hzr doctor
```

Restart open agent sessions after installation so they reload the current routing contract.

## Verification

The release is checked with:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets --all-features
scripts/verify-fork-core.sh --test
```

Dedicated acceptance gates cover no-RAW routing, exact/full-read density, instruction repair,
typed backend attribution, final-stage de-duplication, privacy, time-bounded stats, npm/pnpm
aliases, and top-level read/write forwarding.
