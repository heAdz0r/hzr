# MCP and retrieval implementation — approved P0/P1

This report records changes against the fresh 2026-09-04 audit. It does not establish billed savings or accepted-task improvements. The integration owner runs the complete workspace and fork gates after all concurrent changes settle.

## Implemented

- **Scoped retrieval provenance:** `crates/hzr-context/src/planner.rs` now distinguishes grepai workspace-relative hits from lexical scope-relative hits. Graph-plan candidates are rebased to the workspace before symbol outlines and normalization. Path resolution does not guess based on whether competing filenames exist. The integration fixture now matches the real graph planner's scope-relative output.
- **Evidence cost:** `candidate.rs` charges delivered candidate evidence, rather than the greater of the evidence and the whole source file's estimated future reading cost. Whole-file estimates remain explicit evidence metadata.
- **Caller control:** `hzr context plan --max-tokens N --no-memory` and matching MCP fields reach the daemon and context planner. The configured maximum remains authoritative. No-memory avoids the ICM recall call entirely. This limit describes selected evidence; it is not a provider tokenizer or a billed session budget.
- **Exact writes:** MCP patch replacement and file creation accept empty content and preserve whitespace. Empty patch search text remains rejected, matching the fork's patch contract. Strings publish UTF-8 byte ceilings in `x-maxUtf8Bytes`, and validation applies them before dispatch. Read bounds publish the parser's 100000 maximum.
- **Exact memory recovery:** `hzr memory get ID --scope project|global` and `hzr_memory_get` retrieve one exact ID from the canonical ICM SQLite store. Parameterized read-only lookup and the existing namespace predicates prevent foreign or legacy memories from being returned. This does not scan a capped list of 10000 memories and does not require another semantic query. Context truncation recovery now names this executable command.
- **Typed reads:** `hzr --json read FILE [--from N --to N --max-lines N --max-tokens N --expected-sha256 HASH]` and `hzr --json read --batch FILE... --max-tokens N` use the same daemon endpoint as non-outline MCP reads. MCP accepts exactly one of `path` or `paths`. Each result has source SHA-256, byte and line counts, delivered range, `complete`, and `next_line`. Remaining batch paths are explicit. An expansion can reject a changed source using `expected_sha256`.
- **Read fidelity:** the confined fork produces the actual content. The daemon checks byte equality against the requested source range and checks the source hash again after execution; transport success alone does not imply source completeness. Source limits, malformed ranges, path escapes, invalid hashes and a single line exceeding the budget fail explicitly.
- **Budgeted batches:** the read endpoint estimates the serialized UTF-8 response, including JSON escaping and metadata, before selecting line ranges. The budget is shared across files. A file that fits is read in full without requiring an outline or several partial requests. This remains an estimate based on UTF-8 bytes divided by four, not model tokenizer billing.
- **Durable execution:** MCP `hzr_exec` accepts `action: run|start|wait|cancel`. Start requires a stable caller operation ID; wait/cancel use the same ID and pinned workspace. The execution deadline supports 30 minutes; waits are bounded to 10 seconds. The root implementation owns the durable daemon jobs, CLI execution surface and cancellation-on-drop integration.

## Source ownership and integration

Main edits are in `hzr-context`, `hzr-cli/src/mcp*`, `read_cli.rs`, the context/read/memory sections of CLI and daemon API, protocol request/response types, and `hzr-memory/src/snapshot.rs`. The daemon uses already-workspace-owned `sha2` and `hex` dependencies for read identity. The MCP inventory adds one tool (14 total); the host-integration owner regenerates the managed instruction projections from `contracts/agent-capabilities.json`.

The complete serialized MCP schema limit is now 32 KiB, with a regression guard, because typed read completeness and durable execution require additional contracts. This is a deliberate cost increase with useful capability, not a token-saving claim. Critical fields were not removed to preserve an old size threshold.

## Verification

- Exact memory read regression: **1 passed**, including preserved whitespace/empty raw excerpt, project/global isolation, legacy exclusion, missing ID, and SQL-shaped ID input.
- MCP targeted run initially: **31 passed, 1 failed**. The sole remaining assertion assumed every output variant used a top-level `additionalProperties` instead of strict alternatives; it was corrected to validate either representation. The previously failing size, inventory, parser, and representative output-schema tests passed in that run.
- Context all-targets run after retrieval fixes: **25 unit tests and 1 fork integration test passed**. A subsequent explicit no-memory/budget assertion was added to that integration fixture for the final workspace gate.
- New read integration coverage: full exact content, range/next-line, hash-pinned expansion, changed-source rejection, escaped/Unicode response budgeting, and traversal rejection. Final integration results are recorded by the main implementation owner.
- Earlier combined compiles encountered concurrent root-owned job-test compilation defects and a managed 29.5-second command timeout. Those runs are not counted as successful tests.

## Practical limits

Typed reads currently cover UTF-8 files up to 4 MiB and response budgets from 1024 to 48000 estimated tokens. They preserve complete source lines; a minified line larger than the maximum response budget requires tracked exact CLI recovery. Outline reads retain the existing structural fork result and do not claim complete source content. Source identity checks prevent silent mixing of revisions but do not provide a filesystem-wide transaction across a batch. Existing non-JSON inherited CLI reads remain available.

No paid model evaluation, installed client configuration change, deployment, or claim of improved accepted-task cost is part of this subtask. Evaluate complete agent episodes, including added schema bytes, retries and follow-up calls, before promoting an economic claim.
