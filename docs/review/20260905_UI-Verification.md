# HZR 0.8.0 UI verification — 2026-09-05

Source: working tree based on `44cd601f3373b2d2c7879eab29267a948749e369`, including the current uncommitted stage-accounting integration. `visualizer/package.json` reports version `0.8.0`.

## Scope and outcome

The current visualizer test command, TypeScript/Vue check, and production build all completed successfully through managed HZR execution. This is source/build verification, not deployment or new browser screenshot acceptance. No Cargo command was run by this reviewer.

| Command | Exit status | Observed result |
| --- | --- | --- |
| `hzr exec run 'bun run --cwd visualizer test'` | 0 | Managed result reports success, warnings 0, errors 0. Individual test count was not exposed by this filter. |
| `hzr exec run 'bun run --cwd visualizer typecheck'` | 0 | Managed result reports success, warnings 0, errors 0. |
| `hzr exec run 'bun run --cwd visualizer build'` | 0 | Managed result reports success, warnings 0, errors 0. The script runs `vue-tsc --noEmit && vite build`. |
| `hzr exec run 'git diff --check -- visualizer'` | 0 | No whitespace errors. |

Do not transfer the earlier UI test count or screenshots to this exact working tree as though they were repeated here.

## Accounting label review

- Overview and session cards identify the reduction and input/output comparison as producer estimates.
- Explicit adapter delivery appears independently and renders `Unknown` when evidence is absent. The UI explicitly says not to add it to producer output and states that complete host delivery and causal linkage are unproven.
- Whole-task savings remains `Not established`; provider receipts do not establish a comparison or answer quality.
- The final review found remaining stale labels in `App.vue` and `LiveActivity.vue`. Only their copy was corrected: global totals now say `Producer operations`, `Producer reduction`, and `Produced tokens`; activity details and the accessible bar description now say produced/producer. No layout or calculation changed.
- Pricing assumptions and user-supplied claims remain distinguished from proof of billed savings. The corresponding backend stage-accounting gate is outside this UI verification run.

## Verification limitation

An additional attempt to obtain an unfiltered test transcript with `HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=complete_log hzr exec run 'bun run --cwd visualizer test'` was rejected before execution with exit 77: `T4 fidelity reason is contradicted by the requested command`. The earlier managed test command had already passed. This failed diagnostic attempt does not establish an additional test run; no bypass or approval was attempted. Exact per-test counts are therefore intentionally not claimed.

`git status --short -- visualizer` listed only the five intended source files (`App.vue`, `EvidenceOverview.vue`, `LiveActivity.vue`, `SessionRoi.vue`, `types.ts`), with no tracked generated build output. No real agent configuration, installed bundle, daemon lifecycle, deployment, or provider calls were changed during this verification.
