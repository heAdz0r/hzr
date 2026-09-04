# Host integration fixes — 2026-09-04

Source baseline: `fbf204d48704160407e3f06035ac32b9cf0bc67a` (0.7.1).
Implementation target: 0.8.0, shared working tree; no release or installed-host claim.
This report follows current source and current official host documentation, not earlier PRDs.

## Implemented, code-proven

- `crates/hzr-cli/src/hook_runner.rs:50`: dispatch validates the explicit host and canonical event/input shape. Unknown shapes return no optimization response and preserve the host's normal permission flow. Codex is selected with `hzr hooks dispatch --host codex`; raw `exec_command` payloads are not assumed equivalent to the documented canonical Bash hook payload.
- Native Read/Grep/Glob/Edit/Write no longer receive optimization-only denial in observe, steer or strict compatibility modes. The old prescriptions discarded Read offset/limit and Grep expression/options; they have been deleted. Native writes also retain their original semantics instead of being forced through an unproven equivalent.
- `hook_runner.rs:631`: native observation is an unaccounted native operation with a non-avoidable capability gap and no savings credit. It stores size estimates without source/tool content. The observer remains failure silent. A missing accounting receipt remains a gap.
- `host_hooks.rs:74`: Claude argument rewrites remove the old unconditional permission allow. Updated arguments preserve all unchanged input fields and undergo normal host permission checks unless the host explicitly reports bypassPermissions. Security deny responses are retained.
- Codex requires allow with updatedInput. HZR therefore only rewrites when that host has explicitly granted bypassPermissions; otherwise native execution proceeds through the ordinary host permission flow. It never emits unsupported ask: any real policy ask reaching the adapter becomes a deny instead of being silently ignored by Codex. The dispatcher ordinarily avoids that branch by respecting the host grant before routing.
- No Codex PostToolUse block response is emitted. This avoids rejecting code-mode promises after a command's effects have already occurred.
- `host_hooks.rs:24`, CLI `hooks capabilities --host claude|codex --probe`: versioned `hzr_host_hooks_v1` metadata, documentation date, declared versus implemented features, and local shape/permission fixture checks. Installation, trust, observed dispatch and model delivery are deliberately not inferred from a successful fixture.
- `contracts/agent-capabilities.json` retains the new memory-get tool and describes current supported/manual host paths. The managed instruction generator uses task-cost-aware full reads, typed read budgets/hash continuation, and honest host coverage. It does not require an outline round trip before an already justified full read.
- `HZR.md`, awareness files and the README tool row are synchronized with the 14-tool surface. The actual canonical long contract is root `HZR.md`; there is no `contracts/HZR.md`.
- Doctor no longer warns that Observe must be changed to Steer for native Read/Grep optimization. All retained mode labels use nonblocking observation.

## Current host facts and deliberately unsupported paths

Claude documents PreToolUse updatedInput as replacing the complete argument object, with permission rules evaluated against rewritten input; allow additionally auto-approves. Current PostToolUse supports updatedToolOutput, but built-in output must match the tool's structured output shape. Additional context is not replacement. [Claude hooks reference](https://code.claude.com/docs/en/hooks)

Codex documents canonical Bash hooks for shell/unified exec and apply_patch hooks, but updatedInput requires allow. Ask is unsupported; PostToolUse block has promise-rejection semantics in code mode. HZR's implemented Codex adapter supports canonical Bash only. Apply-patch transforms and arbitrary nested tools remain pass-through. [Codex hooks reference](https://learn.chatgpt.com/docs/hooks)

No minimum installed host version has been verified. Documentation date and adapter version are not a tested host-version claim. No real Claude/Codex configuration was changed, no hook installed, and no paid model call made.

Native structured-output replacement now has a narrow opt-in Claude Bash/cargo-test implementation; see [Post-Tool-Output-Fixes](20260904_Post-Tool-Output-Fixes.md) for exact shape, recovery, tests and limitations. Native Read/Grep output replacement remains unimplemented. A generic truncation/summary of Read or Grep would lose exact semantics. Current useful transparent transformation is supported Bash input routing into the managed engine. A future native-output adapter requires concrete output fixtures, error/metadata retention, safe no-op for unknown shapes, and a trusted sentinel observed in the next model request before any delivered-output claim. Global final-answer replacement remains unavailable/instructed with zero economic credit.

## Verification

Completed locally:

- Selected Rust sources formatted successfully with rustfmt.
- Scoped `git diff --check`: exit 0.
- Five host-adapter unit regressions cover Claude permissions, Codex grant/ask behavior, unknown/native shapes, preserved security denial and honest fixture coverage.
- Native integration regression now exercises all three legacy modes against Read/Grep/Edit/Write including offsets, regex options and empty-write fields; it expects silent preservation instead of denial.
- Existing imported native fixtures remain historical engine evidence. Current host tests explicitly assert the new nonblocking policy rather than falsely claiming the old expected denials still describe the host.

Not yet passed at memo creation:

- First `hzr exec run 'cargo test -p hzr-cli host_hooks ...'` ended with legacy timeout 124 at roughly 29.5 seconds, with no test verdict.
- Retried through `hzr rtk -- test cargo test -p hzr-cli host_hooks ...`: compile exit 101, three E0277 errors in the concurrently edited hzr-cli; the wrapper returned only the final summary. The exact compiler diagnostics must be collected by the centralized gate.
- Workspace `cargo fmt --all --check` was red while multiple agents were editing unformatted source. Selected owned files were subsequently formatted; full-workspace green is not claimed.
- Per root coordination, additional cargo invocations are paused until the centralized build slot. Required follow-up: host_hooks, hook_runner, instructions, native_hook_policy, then full project gate.

No billed savings, accepted-task improvement, installed coverage, or real model-delivery result is inferred from these source changes.
