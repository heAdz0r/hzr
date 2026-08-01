# PRD addendum §16 - HZR as default entry point (replacing RTK hooks)

**Status:** release candidate 0.2.0; hook/control-plane, durable PATH placement, Claude/Codex instructions, MCP migration and production user service are implemented; live deployment is recorded in a separate audit record
**Parent:** [PRD.md](PRD.md) · closes adoption, client MCP ownership and background service lifecycle
**Target version:** 0.2.0 (extends CLI surface §6.8 and mutation surface §14 - outside the scope of 0.1.0)
**Solutions accepted:** hybrid daemon→fork fallback · `HZR` = heAdz0r's Zero-Redundancy engine

---

## 16.1 Task

Today `~/.claude/settings.json` routes hooks to `~/.claude/hooks/rtk-*.sh` and `~/.local/bin/icm hook *`. RTK is the default interceptor, HZR is not. Requirement: **HZR becomes the default entry point for all new projects**, and RTK stops being called directly, continuing to work internally as a fork-core.

This applies to three surfaces:

1. **Automatic `hzr init`** in a new project - no manual step.
2. **All requests through HZR** - rewrite, memory-context, block-native-explore go to HZR, and not to RTK directly.
3. **Agent instructions** (Claude Code and Codex) describe HZR behavior in the same way as they currently describe RTK.

## 16.2 Non-negotiable invariant of adoption

> **Exactly one hook per event.** RTK-hook and HZR-hook cannot be registered at the same time.

This is a direct consequence of §4.1 (“zero duplicate layers”) and the acronym itself. Double rewrite - double scan, double compression and competing verdicts. The installer is required to **delete or replace** rtk entries, and not add them nearby. `hzr doctor` must report coexistence as `error`, not `warning`.

## 16.3 Hook execution model - hybrid (accepted decision)

The hook fires on **every** Bash command, so it has no right to depend on a living demon.

```text
hzr-rewrite.sh
  ├─ is hzrd available?
  │    ├─ yes → POST /v1/exec/rewrite     (HZR policy + budget + ledger)  source=managed
  │    └─ no  → hzr rtk -- rewrite <argv> (direct fork, daemon-free)      source=degraded
  └─ exit 0/1/2/3 — fork semantics are preserved in both branches
```

Rules:

- **The shell never breaks.** The absence of `hzrd` is not a hook error. This is exactly the path that PRD §10 already allows: “`hzrd` is not available → exact compatibility `hzr rtk`/`bin/rtk` remains the direct process path.”
- **Degradation is visible, not silent.** Each degraded call increments the counter, `hzr doctor` shows `degraded_rewrites=N`, and `hzr stats` marks the period as partially unaccounted for. This complies with §4.2: what is not accounted for is not passed off as accounted for.
- **No silent fallback on stock RTK.** Fallback goes to `hzr rtk --`, that is, to the same pinned fork-core `0.44.1-fork.1`. The prohibition §1.1.8 is not violated.
- **Hook timeout is strict** (I suggest 2 s per daemon probe): an expired probe immediately goes to degraded, rather than hanging in the terminal.
- Release installer installs and launches production user service (`launchd`/`systemd --user`) on stable `current/bin/hzrd`; foreground `hzr daemon serve` remains in development mode.

**The cost of the solution, fixed explicitly:** if the service is unavailable, rewrite saves the fork verdict, but does not get into the SQLite ledger. `doctor` and `stats` are required to show incomplete accounting; installer restores service idempotent commands `hzr daemon service install|restart`.

## 16.4 Automatic `hzr init`

`init` must be idempotent and **non-mutable for repository content**.

|Condition|Behavior|
|---|---|
|New workspace, canonical data root available|Registers `(repository_id, worktree_id)`, prepares an entry under `workspaces/`, does not create anything in the repository|
|`.grepai` is missing|Creates a verified symlink to the canonical store (§7)|
|`.grepai` - real directory|**Does not touch.** `migration_required`, awaits explicit `hzr migrate apply` (§11)|
|`.grepai` - someone else's symlink|`error`, without mutation|
|Already initialized| `already_initialized`, exit 0 |

The automation trigger is `SessionStart` hook, which calls `hzr init --if-needed --quiet`. This is cheaper and safer than auto-initialization inside the rewrite path: rewrite remains a hot path without file mutations.

Requirement: `hzr init --if-needed` in an already initialized workspace must be **pure read-only no-op** - otherwise it turns into a hidden entry for each session start.

## 16.5 New CLI surface (extension §6.8)

```text
hzr install [--dry-run] [--force] [--prefix DIR] [--binary PATH]
            [--allow-dev-path] [--keep-external-icm] [--skip-instructions]
hzr uninstall [--keep-data] [--dry-run] [--force] # hooks + instruction blocks; RTK is not restored
hzr hooks status [--json]              # hooks + instructions + PATH + foreign processes
hzr init --if-needed [--quiet]         # for SessionStart
```

`hzr install` performs the entire adoption, in one confirmed action, in order
"binaries → hooks → instructions" so that the hook command and `CLAUDE.md` refer to the path,
which by this moment already exists:

1. **Durable binaries on PATH.** `hzr`/`hzrd` are copied to `--prefix` (by default `~/.local/bin`, where other engines already live), marked `0755`, and the absence of prefix in `PATH` is reported separate warning and `doctor`-check `hzr_on_path`. Alias ​​`rtk` to PATH is **not** created: this would be the second entry point.
2. **Backup** of each modified file with full-SHA in the name, the same proven pattern as migration (§11).
3. `--dry-run` - first-class mode: shows `before_sha256`/`after_sha256`, backup path and target hook binary without writing anything. Without `--force`, the entry is rejected with the instruction to watch the preview first.
4. **Idempotent on all three surfaces**: re-running gives `changed=false` for hooks, prefix and instructions, does not create a second backup and does not duplicate records.
5. **Detect and replace** rtk records (`rtk-rewrite.sh`, `rtk-mem-context.sh`, `rtk-block-native-explore.sh`, `rtk hook …`), saving the source in backup.
6. **Centralize memory ownership:** direct `icm hook …` records are deleted because they write to a store that HZR does not supervise - that is, the second durable memory layer against §6.5. Informed refusal - `--keep-external-icm`; then `doctor` continues to report the take as `error`.
7. **Unknown third-party handlers are never touched** - neither during install nor during uninstall.

**The hook binary is never taken naively from `current_exe()`.** Installing from `cargo run` or from a bundle unpacked into a temporary directory would bind the hook to `target/debug/hzr` or to a path that will disappear, breaking every Bash command. The hook therefore always names the durable copy in the prefix; `--binary` sets the path explicitly; `target/debug|release` is rejected with a diagnostic; and `--allow-dev-path` is reserved for development. The path is canonicalized, so a symlink or `..` cannot hide the build directory.

The implemented matcher covers modern Claude Code `Agent` and legacy `Task`: one `PreToolUse:Bash|Agent|Task` handler invokes the hidden `hzr hooks dispatch`. Fork exits `0/1/2/3` are not returned as hook process exits: the dispatcher converts them into typed `allow/ask/deny` JSON with exit 0, as required by the Claude hook protocol. The managed probe is limited to 2 s; fallback uses `PinnedRtkAdapter` at the same `0.44.1-fork.1` version.

### 16.5.1 Foreign engine processes

`doctor` and `hooks status` list `icm serve` and `grepai watch` running outside the HZR data root, and report them as `error`: multiple `icm serve` means multiple writers in the memory store, and a third-party `grepai watch` will rescan a tree that HZR already owns. Processes inside HZR data root are recognized as their own and are not considered foreign.

**HZR does not stop them.** Automatic termination of external processes is prohibited §4.3 and §11: an incorrectly killed watcher loses the in-flight state of the index. The report is mandatory, stopping is the user’s decision.

The implemented matcher covers modern Claude Code `Agent` and legacy `Task`: one `PreToolUse:Bash|Agent|Task` handler invokes the hidden `hzr hooks dispatch`. Fork exits `0/1/2/3` are not returned as hook process exits: the dispatcher converts them into typed `allow/ask/deny` JSON with exit 0, as required by the Claude hook protocol. The managed probe is limited to 2 s; fallback uses `PinnedRtkAdapter` at the same `0.44.1-fork.1` version.

## 16.6 Agent instructions

Fork-core already contains a ready-made sample of both surfaces: `hooks/rtk-awareness.md` (Claude), `hooks/rtk-awareness-codex.md` (Codex), `hooks/rtk-instructions.md`. HZR-analogues are made using the same division, **without copying the RTK-text verbatim** - the commands are different.

|Artifact|Purpose|
|---|---|
| `HZR.md` |Tool canonical contract: HZR-owned read/search/write/exec/memory/context paths when raw, when exact|
| `integrations/claude-code/hzr-awareness.md` |Claude Code: priority `hzr` over native Read/Grep/Edit|
| `integrations/claude-code/hzr-awareness-codex.md` |Codex: the same contract in its instructions format|
|Link in `~/.claude/CLAUDE.md`|✅ implemented: `install` inserts a managed block with `@<abs>/HZR.md` and removes legacy `@RTK.md`|
|Block in `~/.codex/AGENTS.md`|✅ implemented: the same contract in a form that Codex reads literally (it doesn't have `@import`)|

**How HZR avoids corrupting user files.** HZR owns exactly one
delimited block `<!-- hzr:begin … -->` / `<!-- hzr:end … -->` and never
overwrites the entire file. The custom text remains on top, managed block
added last. Legacy import is removed only if **the entire line** matches
(`@RTK.md`), so prose that simply mentions `RTK.md` is retained. Unclosed
The marker does not cause content to be cropped. `uninstall` removes the block and restores
original body: the left instruction would continue to require `hzr` after removing the hooks.
Path to `HZR.md` is absolute because relative `@`-import does not survive relocation
bundle. Mutations go through the same backup/CAS/atomic path as `settings.json`, but with
a separate lock file for each target file.

Single contract for both agents (replacing the current RTK table):

```text
Read     → hzr rtk -- read <file>
Grep     → hzr rgai "<intent>" | hzr search <pattern>
Edit     → hzr rtk -- write patch|replace|set ...
Bash     → intercepted automatically by hzr hooks dispatch
Memory   → hzr memory recall|store    (the only durable store, §6.5)
Context  → hzr context plan "<intent>"
```

Requirement for the text of instructions: it must describe the **degraded branch** too. The agent who received `source=degraded` should know that the ledger is incomplete, and not assume that accounting is in progress.

## 16.7 Verification (extension §12.2)

New contract tests, mandatory before the release of 16.x:

- `install` idempotent: double launch does not create a second record;
- `install` creates a full-SHA backup `settings.json` and restores from it;
- rtk-record and hzr-record **cannot coexist**; `doctor` gives `error` when coexisting;
- the hook returns **exit 0/1/2/3 identically** in managed and degraded branches on the same command body (this is a key test: degradation should not change the verdict);
- the demon's probe respects the timeout and goes to degraded without blocking;
- degraded calls are counted and visible in `doctor`/`savings`;
- `init --if-needed` on the initialized workspace - byte-by-byte no-op on the file system;
- `init` does not touch the real `.grepai` and requires migration;
- `uninstall` removes all records and leaves no broken paths;
- fallback goes to pinned fork `0.44.1-fork.1`, and not to stock RTK (version assertion);
- the hook binary is rejected at `target/debug|release` and when the path does not exist; `--allow-dev-path` permits it;
- `install` in prefix idempotent, sets the exec bit and does not overwrite an already matching byte-to-byte binary;
- `rtk` alias **not** appears on PATH;
- managed block of the idempotent instruction file, removes `@RTK.md` only as a whole line, saves the prose with the mention of `RTK.md`, does not cut off the contents when the marker is not closed, and `uninstall` restores the original body;
- Codex surface uses literal reference rather than `@import`;
- foreign-scan distinguishes processes inside HZR data root from foreign ones and never reports its own call.

## 16.7.1 What is deliberately not done: global codec interception

Requirement "Caveman codec intercepts global Claude/Codex requests and responses"
**not implemented**, and not because of the amount of work:

1. **Technically not possible via hooks.** Claude Code does not provide a hook that
rewrites the payload going to the provider, or the model's response. `UserPromptSubmit` can
*add* context, `Stop` can block stopping - none gives mutable
provider request/response. Interception only exists where the request is valid
passes through HZR, that is, to `hzr agent run`.
2. **Contradicts its own evidence base PRD.** §3.4 records that input
compression on average **increases** cost and reduces accuracy (CAVEWOMAN), and §4.3
directly declares the provider's non-goal compression reasoning. Global Claude input compression
would be the implementation of what PRD had already rejected on the data.

Implemented and implemented part - **instruction-level density contract**: managed block
in `CLAUDE.md`/`AGENTS.md` sets the density of the response before generation, which is the same technique,
which managed bridge uses via `appendSystemPrompt`. This affects output tokens
no lossy rewrite and no second pass.

## 16.8 Risks

|Risk|Mitigation|
|---|---|
|Double rewrite RTK+HZR|§16.2 invariant + `doctor` error + replacing records during install|
|Hook breaks the terminal when the demon is dead|hybrid fallback (§16.3) + hard timeout|
|Silently incomplete ledger|Counter degraded + reflection in `doctor`/`savings` (§4.2)|
|Damage `settings.json`|Full-SHA backup + `--dry-run` + diff confirmation (pattern §11)|
|Growth mutation surface vs §14|Limit mutations transaction-safe adoption, client-config migration and platform service lifecycle; each step has dry-run/backup or idempotent service semantics|
|Double memory layer with external `icm hook`|`install` centralizes ICM by default; `--keep-external-icm` is an explicit opt-out that remains visible in `doctor`|

## 16.9 Delivery status

1. ✅ `hzr hooks dispatch` + hybrid daemon→pinned-fork path.
2. ✅ `hzr install/uninstall/hooks status` with backup, dry-run, centralized ICM and replacement of RTK records.
3. ✅ `hzr init --if-needed` + registration `SessionStart`.
4. ✅ `HZR.md` + Claude/Codex managed blocks with backup/CAS/atomic mutation.
5. ✅ Contract tests §16.7 and `doctor` - ownership/conflicts check.
6. ✅ Direct ICM registrations in Codex/Claude Desktop are transactionally replaced with `hzr mcp serve` from backup/CAS.
7. ✅ Production user service is installed on stable `current/bin/hzrd`; lifecycle is available through `hzr daemon service`.
8. ✅ Parent PRD and release documentation are synchronized with 0.2.0.

Standalone adoption still starts with `hzr install --dry-run` and requires an explicit `--force`. The repository-level release installer performs this confirmed stage by default; `HZR_INSTALL_HOOKS=0` installs the bundle without changing hooks/instructions.
