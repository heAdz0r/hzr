# HZR 0.3.6 — Reliable notifications and resilient index startup

HZR 0.3.6 fixes the path that tells users and agents a newer published version exists. The updater
itself already downloaded, verified, and atomically installed release bundles correctly; discovery
was unreliable because a pre-release “nothing newer” result stayed cached for 24 hours and only one
agent surface invoked the check.

It also restores canonical search in repositories that still contain dormant nested `.grepai`
directories created by older grepai/RTK invocations. Those stores remain preserved and visible to
diagnostics, but no longer turn a hygiene finding into a search outage.

No update is installed automatically. HZR reports availability and waits for explicit approval.

## What changed

- A check that finds no newer version expires after one hour instead of 24 hours. A release
  published after an earlier same-day check is therefore discovered within the next hourly window.
- A known newer version remains cached for 24 hours, avoiding unnecessary GitHub API traffic while
  keeping the notice available to later sessions.
- Claude's managed SessionStart hook returns structured output with both `systemMessage` for the
  visible interface and `additionalContext` for the model.
- Codex checks during the mandatory installed `HZR.md` bootstrap. The update notice is written
  separately from fork-core stdout, so an exact contract read remains exact.
- Agent-facing notices explicitly require telling the user once and prohibit running `hzr update`
  without approval.
- Network errors remain silent and fail-open: workspace initialization and tool execution continue.
- A dormant nested `.grepai` is now a non-blocking doctor warning. HZR takes only the canonical
  owner lock, launches only the canonical watcher, and leaves every nested byte untouched.
- A nested `index.gob.lock` held by an active legacy writer is still a hard error, preventing two
  grepai writers from running against one repository.
- Explicit migration remains fail-closed when several stores make the intended source ambiguous;
  this availability fix does not guess, merge, delete, or silently archive user data.

## Why 0.3.5 could stay silent

If HZR 0.3.5 checked GitHub before a release was published, it cached `latest_version: null` for a
full day. Every session in that interval classified the cache as current and skipped the network.
In addition, plain Claude SessionStart stdout entered model context but was not a guaranteed visible
user notification, while Codex's bootstrap did not call the update path at all.

## Upgrade impact

The upgrade does not rewrite indexes, memory, ledger rows, or project activation. Dormant nested
stores remain in place until an operator deliberately handles them. The installer
rewires the marker-owned Claude SessionStart command to request structured hook output and preserves
unrelated hooks and settings through the existing transactional backup and compare-and-swap path.

Upgrade and verify with:

```bash
hzr update
hzr --version
hzr hooks status
hzr doctor --workspace .
```

Restart already-open agent sessions after upgrading so they reload the managed hook and contract.

## Verification

The release gate includes focused RED/GREEN regression tests for negative-cache expiry, structured
SessionStart output, Codex contract-bootstrap detection, and canonical index availability in the
presence of a byte-preserved dormant nested store, followed by:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
rustup run 1.85.0 cargo check --locked --workspace --all-targets --all-features
scripts/verify-fork-core.sh --test
cd integrations/caveman-code && npm ci --ignore-scripts && node --test bridge.test.mjs
cd visualizer && bun install --frozen-lockfile && bun test && bun run typecheck && bun run build
```

The bundle smoke gate additionally verifies clean installation, same-version re-attestation,
project-only adoption, hook rewiring, upgrade behavior, public entrypoints, and all four pinned
engines before publication.
