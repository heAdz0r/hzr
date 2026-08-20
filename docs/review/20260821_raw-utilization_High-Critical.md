# High Critical review: RAW utilization and first-class routing

**Date:** 2026-08-21

**Ledger snapshot:** command id `<= 51995`, through `2026-08-20T22:00:10.568480+00:00`

**Measurement:** estimated tool-output tokens from UTF-8 bytes divided by four

**Economic boundary:** `economic_claim_ready=false`; provider-billed task receipts: `0`

## Verdict

RAW/proxy was an agent default rather than an exceptional fidelity path. Across all observed
workspaces it delivered 27,878,506 estimated tokens, or 43.8128% of all observed tool output.
During the final seven-day window it delivered 7,236,207 tokens versus 4,442,871 through
optimized routes; 99.31% of that recent measured RAW output was attributed to Codex.

The primary cause was the managed instruction table: file, search, and write operations had
first-class routes, while the generic shell row sent agents directly to
`hzr rtk -- raw <command...>`. Codex does not receive Claude's `PreToolUse` rewrite, so the
instruction was its routing control. The result was especially costly for command families the
managed fork already filters.

## Workspace evidence

Values are `RAW operations / optimized operations; RAW delivered / optimized delivered`.
Temporary and descendant paths are normalized to their repository where possible.

| Workspace | Lifetime | Final seven days |
|---|---:|---:|
| `anonymous_bot` | `4,070 / 18,854; 13,375,667 / 16,466,452` | `1,206 / 4,020; 5,993,489 / 4,001,078` |
| `hzr` and descendants | `5,905 / 4,255; 9,757,293 / 3,957,005` | `20 / 67; 3,442 / 90,872` |
| `teo_tale` | `423 / 323; 396,398 / 491,517` | no activity |
| `pianomarvel` | `620 / 2,093; 268,809 / 3,609,989` | `202 / 370; 147,659 / 328,792` |
| `hzr-main-integration` | `175 / 43; 261,926 / 36,128` | no activity |
| `hzr-ui-sota` | `267 / 214; 234,289 / 231,310` | no activity |
| `easy_math` | `262 / 1,549; 133,944 / 569,775` | no activity |
| `rtk` | `115 / 2,366; 65,622 / 343,441` | no activity |
| `hzr-ui-real-search` | `78 / 110; 62,600 / 30,514` | no activity |
| `vezdehod_bot` | `112 / 82; 13,099 / 223,596` | no activity |
| `dkp_rag26` | `104 / 369; 5,359 / 2,143,856` | no activity |
| `hzr-observability` | `4 / 164; 3 / 15,766` | no activity |
| `inn_search` | `4 / 14; 0 / 3,367` | no activity |
| `gazprom_deals` | `0 / 19; 0 / 81,084` | no activity |

The ledger also contained 1,425 distinct `project_path` values. Of those, 1,377 were temporary
or test paths, accounting for 6,496 operations and 890,514 delivered tokens. A future attribution
change should store canonical workspace root separately from actual command cwd.

Codex task workspaces with material RAW exposure included `2026-08-02/new-chat`
(1,593,569 RAW delivered tokens), `2026-08-14/new-chat` (1,045,672),
`2026-08-12/files-mentioned-by-the-user-draft` (106,399), and
`2026-08-12/new-chat` (44,239). These are task directories rather than stable repository
identities and reinforce the attribution defect above.

## Highest-cost RAW families

| Family | Operations | Delivered | Existing efficient route |
|---|---:|---:|---|
| `bun` | 922 | 6,477,820 | managed fork `bun` |
| `ps` | 117 | 2,610,242 | managed fork `ps` |
| `nl` | 117 | 2,297,721 | `read -n` |
| `sed` | 1,166 | 1,488,173 | ranged `read` or typed write |
| `cargo` | 936 | 1,386,526 | managed fork `cargo` |
| `ssh` | 612 | 1,294,769 | managed fork `ssh` |
| `wget` | 6 | 1,254,936 | managed fork `wget` |
| `git` | 2,023 | 1,234,319 | managed fork `git` |
| `rg` | 974 | 1,032,467 | exact search or managed fork `rg` |
| `gh` | 511 | 786,107 | managed fork `gh` |
| `find` | 156 | 667,868 | managed fork `find` |
| `grep` | 669 | 173,072 | exact search |

Of lifetime RAW output, 15,905,222 tokens (57.05%) came from families already supported by the
fork but absent from HZR's narrow replacement map. Another 5,138,821 tokens (18.43%) came from
text tools already known to the map. The remaining 6,834,463 tokens (24.52%) were shell grammar,
unsupported tools, or operations without a safe general mapping. These are exposure figures, not
counterfactual savings claims.

## Corrective contract

1. Agent-originated shell commands use `hzr exec run '<shell command>'` by default.
2. `hzr exec rewrite '<shell command>'` is the read-only routing preview.
3. If policy or fork-core reports an effective first-class route, RAW is not an allowed result.
4. Unmarked managed RAW/proxy wrappers are removed without reconstructing the payload; fork-core
   receives the original payload and remains the single command-support authority.
5. Text replacements such as `nl`, ranged `sed`, and `rg` are automatic only when HZR can
   prove that reconstruction preserves command semantics.
6. Nested shell grammar is never reconstructed from `split_whitespace`; unsupported commands
   remain tracked fallback operations.
7. Explicit unfiltered recovery requires
   `HZR_RAW_FIDELITY=1 hzr rtk -- raw <command...>` and remains available only when exact fidelity
   is required and no effective filtered route can satisfy the task.

## Release acceptance gate

The release must fail unless all of these remain true:

- generated Codex and Claude contracts name `hzr exec run` as the default shell route and do
  not contain the former `exact/raw output -> raw` row;
- representative text families (`nl`, ranged `sed`, `rg`) never resolve to RAW when their
  safe first-class replacement is available;
- representative fork families (`bun`, `cargo`, `ssh`, `git`, `gh`, `find`, `wget`,
  `ps`) finish as `AllowRewrite` decisions after a managed wrapper is removed in both daemon and
  degraded-hook tests;
- quoted arguments, nested shell expressions, and explicit fidelity cases are not reconstructed
  unsafely;
- a clean installed bundle refreshes the managed contract and the installed `HZR.md`.

The gate verifies routing correctness. It does not turn local output estimates into a provider
cost claim; that still requires paired accepted tasks and provider receipts.
