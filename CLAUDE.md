<!-- hzr:begin managed agent contract — do not edit inside -->

# HZR (managed)

`hzr` filters command output and owns `rtk`, `grepai`, `icm`; never call those binaries
directly. Hooks already route supported Bash commands; native Read, Grep, Glob, Edit and
Write stay the default.

| Need | Use |
|---|---|
| File outline or line range | `hzr read <file> --outline`; or `--from A --to B`; native Read stays the default for whole files. |
| Search by intent | `hzr search "<intent>"`; `--mode exact` for a known literal. |
| Atomic multi-file edit (optional) | `hzr write batch ...`; native Edit/Write stay the default. |
| Durable decisions and fixes | `hzr memory recall\|store`; project scope by default; `--scope global` only for user-wide preferences. |
| Unfamiliar or cross-cutting area | `hzr context plan "<intent>"`; bound with `--max-tokens N`. |
| Shell command outside the hook | `hzr exec run '<cmd>'`; hook-routed Bash is already managed. |
| Exact unfiltered output | `HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr exec run '<command>'`; reason: binary, checksum, machine_protocol, complete_log, full_patch or verbatim_source. |

Filtered or bounded output always names its recovery command; never treat it as
complete. Use MCP `hzr_*` tools only when the server reports this worktree. If enabled
in `hzr settings`, `hzr delegate` runs scoped tasks; review its results. Policy details:
`HZR.md` (read with `--outline` first).

Write concisely: lead with the result, skip restatement and tool recaps, keep code,
paths, errors and numbers exact.

<!-- hzr:end managed agent contract -->
