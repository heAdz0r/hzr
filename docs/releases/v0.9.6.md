# HZR 0.9.6

### Fixed

- Release notes are validated against the workspace version before native builds, preventing a late publication failure.

- Manual `doctor --reconcile-fleet --fix` records reference-state completion and immediately refreshes its report (#4, PR #9).
- Configured billing no longer displays the enable checklist when delivery evidence is missing; Fable 5 has an exact pricing row distinct from Fable 5.1 (#5, PR #9).
- Source observer installation checks Cargo before fetching and explains toolchain recovery; `--source-dir` is not presented as a toolchain-free option (#6, PR #9).
- Managed find retains absolute and relative search roots, preserves long grouped directory headers and cannot panic while slicing Unicode paths (#7, PR #10).
- Native Claude Explore workers pass through HZR without an optimization-only denial or permission grant (#11).

### Added

- Release bundles include pinned agtx 1.0.4 and its read-only observer, built from the same commit and patches. Observer installation works offline without Cargo, with bundle integrity and identity checks (#8).
- `hzr agents board --project <worktree> --agtx-data-dir <absolute-store-root>` opens the bundled interactive runtime explicitly; enrollment and read-only monitoring remain separate.
- Confirmed Apache-2.0 provenance, corrected derivative manifest licensing, source modification notices and shipped license texts for agtx.

Contributed by aleksandr-podmoskovniy; PR commits are preserved, with maintainer review corrections.

Upgrade with `hzr update`. Existing HZR data and billing selections are preserved.
Agent monitoring remains opt-in; launching `hzr agents board` explicitly creates an
agtx store when needed, and enrollment is a separate command.

No billing savings are inferred without host delivery evidence. Operating a board still requires upstream runtime prerequisites such as tmux and the selected coding agent; neither is needed merely to verify the bundled observer.
