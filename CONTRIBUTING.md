# Contributing to HZR

HZR is heAdz0r's zero-redundancy control plane. Changes must preserve its
one-owner architecture. Extend the inherited engine in `fork-core/rtk` when a
behavior belongs to command execution; use the HZR crates for orchestration,
lifecycle, policy, indexing, memory, accounting, and agent integration.

## Development setup

You need Rust 1.85 or newer, Git, Go, Bun 1.2.19, and a POSIX shell. Bun builds and
tests the Vue visualizer. Node is downloaded and verified by the bundle builder,
so a separate Node installation is not required to assemble a release bundle.

TDD is optional. Use `hzr tdd` when the user or repository requires it, or when
regression risk justifies test-first overhead. When token or time efficiency
matters, implementation may proceed without a RED phase, but the relevant tests
and repository quality gates still apply. If TDD is selected, preserve the
observed RED and GREEN commands for the final report. Passing tests added after
production code are regression coverage and must not be reported as test-first
TDD.

Run the fast checks while developing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
bun install --cwd visualizer --frozen-lockfile
bun --cwd visualizer test
bun run --cwd visualizer typecheck
bun run --cwd visualizer build
```

Before merging or tagging, run the same fail-closed source gate used by CI and the release
preflight:

```sh
scripts/complete-gate.sh
```

It runs the exact locked workspace format, Clippy and test commands, the complete fork parity
suite, the fork warning ratchet, shell syntax checks and the static release-workflow regression
gate. Do not replace it with name-filtered acceptance tests.

Changes to `fork-core/rtk` must also pass its complete regression gate and update
the current-engine identity:

```sh
scripts/refresh-current-engine.sh
scripts/verify-fork-core.sh --test
```

Before proposing a release or installer change, assemble and smoke-test the
self-contained bundle:

```sh
scripts/complete-gate.sh --bundle /absolute/path/to/hzr-dist
```

The bundle mode delegates to the canonical builder, which verifies the pinned components and
runs `scripts/smoke-bundle.sh` before returning success.

## Release notes

Three files describe change, and they answer different questions. Keeping them separate is
deliberate: a reader deciding whether to upgrade should not have to read a complete history
to find out what changed for them.

| File | Question it answers | Audience |
|---|---|---|
| `RELEASE_NOTES.md` | What did the current release change for me, and why does it matter? | Anyone deciding whether to upgrade |
| `CHANGELOG.md` | What changed in every version, exhaustively? | Anyone auditing history |
| `docs/releases/vX.Y.Z.md` | What did *that* release say at the time? | Anyone reading an old release |

`RELEASE_NOTES.md` always describes the newest released version and is rewritten in full
each release — it is not append-only. Explain the behaviour and the reason it changed, and
prefer a measured figure over an adjective. State the upgrade command, and say plainly when
existing data or configuration is affected. Copy the finished file to
`docs/releases/vX.Y.Z.md` in the same commit, so the historical statement is preserved
before the next release overwrites it.

`CHANGELOG.md` keeps its Keep-a-Changelog structure and stays append-only. Promote its
`[Unreleased]` heading to the released version rather than restating the entries.

## Publishing a release

`hzr release VERSION --force` synchronizes version surfaces, builds the bundle and installs
it **locally**. It does not tag and it does not publish — the GitHub Release exists only
because pushing a `v*` tag runs `.github/workflows/release.yml`. Skipping the tag is why the
release badge can sit several versions behind a repository that looks fully released.

```sh
# 1. CI must already be green: the tag workflow builds the same gates and will not publish
#    a release for a red commit.
# 2. Notes first — the workflow refuses to publish if RELEASE_NOTES.md does not name the tag.
git tag -a vX.Y.Z -m "HZR X.Y.Z"
git push origin vX.Y.Z
```

The workflow builds Linux bundles for x64 and ARM64 plus a macOS ARM64 bundle, attests build
provenance, and publishes a stable release marked as GitHub Latest whose description is
`RELEASE_NOTES.md` verbatim. Intel macOS is not a supported release platform. Do not write the
release description in the GitHub UI: it would immediately disagree with the repository.

The Linux x64 CI bundle and `.github/workflows/bundle-cache.yml` warm release Cargo dependencies
and verified native component caches on `main`. Tagged releases restore those caches, then re-check
every cached binary and pinned upstream licence before packaging it. Cache misses remain safe but
slower: `scripts/build-bundle.sh` rebuilds and verifies the component from its pinned source. Set
`HZR_WARM_COMPONENT_CACHE_ONLY=1` only for cache-warming jobs; it stops after grepai, ICM, and the
managed RTK fork have been built and verified.

## Pull requests

- Explain the ownership boundary affected by the change.
- Add regression coverage for behavior changes.
- Write public HZR documentation and user-facing text in English. Historical fork-core audit and provenance files retain their original language unless a dedicated migration updates current-engine identity and passes the complete fork regression gate.
- Keep the immutable `v0.1.0` import baseline unchanged.
- Do not introduce a second semantic index, memory store, command interceptor,
  or simplified fork-core implementation.
- Keep provider-reported usage separate from estimates.
- Update `engines.lock.toml` and third-party notices when dependencies change.

Small, focused pull requests are easiest to review. Generated build output,
runtime data, indexes, credentials, and local backups must never be committed.
