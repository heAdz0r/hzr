# HZR 0.7.0

HZR 0.7.0 restores end-to-end accounting for commands executed by Claude Code and makes
token-economy claims reflect what the host can actually show to the model. It also puts hard
bounds on previously unbounded read and find paths and repairs workspace ownership across MCP,
hooks, instructions, and installer smoke tests.

## Accounting integrity

- Hook rewrites register a correlation context with the daemon. A daemon-owned sweeper commits
  fork receipts from hook rewrites, direct `hzr exec run`, approved continuations, and daemon-owned
  fork operations such as search through one idempotent ledger writer, acknowledges journals only
  after the durable write, and retires empty denied-command contexts after 24 hours.
- Undrained fork receipts are part of the live coverage model. `doctor` warns while they exist,
  stats cannot claim complete coverage, and old 0.6.6 journals remain an explicit historical gap
  rather than being reconstructed into unverifiable operations.
- Stats report raw estimated tokens separately from host-visible capped estimates. Potential
  pricing is unavailable when an operation host has no validated output cap; raw totals are
  labelled upper bounds instead of billed savings.
- Passthrough `find` executions are bypassed and unmeasured, never optimized. Their visible output
  is limited to 200 lines and 16 KiB with an explicit exact-fidelity recovery command.
- `hzr read <file>` now has a smart default budget of 400 lines and 44 KiB. Explicit outlines,
  ranges, and exact-fidelity reads retain their existing semantics.

## Workspace and client safety

- Unpinned MCP servers resolve exactly one workspace at initialization: one client root,
  `CLAUDE_PROJECT_DIR`, then cwd. Filesystem root, home, multi-root, and uninitialized workspaces
  remain fail-closed; explicit `--workspace` pins still win.
- Global memory writes accept only `preferences`, `architecture-global`, and non-empty `user-*`
  topics. Recall is compact by default with explicit expansion, and the FTS5-only retrieval
  boundary is documented.
- Home and filesystem root cannot be initialized as projects. Managed instruction placement
  inherits an identical ancestor block instead of injecting duplicates, and doctor identifies
  legacy user-instruction conflicts.
- Legacy SessionStart hooks are migrated to one current owner. Client-config writes reject
  development binaries unless explicitly authorized, smoke tests isolate every real user config,
  and stale missing-workspace registrations get actionable repair guidance.
- The daemon now allocates its private ICM HTTP transport on an available loopback port. An
  isolated release smoke can therefore run beside the live user daemon without a fixed-port
  collision or a second writer to the same store.
- `hzr install --dry-run` remains a read-only preview when it detects hook, instruction, or
  project MCP changes; it no longer asks a nonexistent write transaction to receipt them.
- The new output paths continue to compile on the declared Rust 1.85 minimum toolchain.
- Instruction audits now time out with an explicit diagnostic when a cloud-backed contract cannot
  be opened, instead of blocking `doctor` and the rest of the registered workspace fleet.
- Aggregate release checksums use the standard two-space filename separator and verify with both
  GNU `sha256sum -c` and BSD `shasum -a 256 -c`.
- The bundled Caveman dependency lock upgrades `fast-uri` to 3.1.7, closing the high-severity
  host-confusion and SSRF advisories reported by the release audit.

## Routing and fork-core

- Top-level `hzr` commands are first-class native operations instead of recursively proxying
  through the fork. No-equivalent shell constructs are not labelled avoidable, and their recipe
  states that tracked fallback carries zero savings credit.
- Search hides the full generation digest unless verbose output is requested. Native `read` and
  `write` help now expose their important fork options.
- Empty `.rtk-lock` artifacts were removed from the current engine manifest. The immutable
  `v0.1.0` import snapshot is unchanged; the current fork identity was regenerated and verified.

## Release contract

HZR 0.7.0 publishes the same three native archives as 0.6.6: `linux-x64`, `linux-arm64`, and
`darwin-arm64`. CI builds each bundle, runs the platform smoke contract, publishes aggregate
`SHA256SUMS`, and marks the stable release Latest only after the complete source gate passes.
