# HZR Project-Only Activation — Product and Architecture Contract

**Status:** implemented in HZR 0.3.5

## 1. Problem

HZR already isolated indexes and memory by repository/worktree identity, but installation was
user-global. `hzr install --force` wrote global agent instructions and a Claude `SessionStart`
hook that initialized every opened directory. `hzr stats` also showed one lifetime ledger. That
made “HZR in project A, untouched baseline in project B” impossible to prove even though the data
stores themselves did not leak.

## 2. Required outcome

An operator can select one or more projects. Outside that set:

- opening a project creates no HZR workspace artifact;
- the global hook returns no decision or injected context;
- no global managed instruction tells an agent to use HZR;
- project-scoped MCP calls fail before daemon dispatch;
- project statistics do not include sibling projects.

Disabling a project is reversible and does not delete its index, memory, registration, or ledger
history.

## 3. User contract

```bash
hzr install --project-only --dry-run
hzr install --project-only --force
hzr enable --workspace /path/to/project
hzr disable --workspace /path/to/project
hzr stats --workspace /path/to/project
```

Release installation supports the equivalent `HZR_PROJECT_ONLY=1` environment override.

The default remains `all` for backward compatibility. `hzr enable` switches the persistent mode to
`selected`; returning to all-project activation requires a confirmed `hzr install --force`.

## 4. Architecture

```text
global config
  activation.mode = all | selected
  activation.enabled_workspaces[] = repository_id + worktree_id + canonical root
          |
          +--> SessionStart: init --if-enabled --> no-op or workspace refresh
          +--> PreToolUse: identity gate --> no-op or one HZR dispatcher
          +--> MCP binding: root safety + initialized placement + activation gate
          +--> local AGENTS.md / CLAUDE.md only for enabled projects

single global hzrd
  workspaces/<repository>/<worktree>/index/grepai
  memory/icm (positive repository namespace filter)
  ledger/hzr.sqlite -- filtered by canonical project path for scoped stats
```

The service remains global because process ownership is not product activation. One supervised
daemon, ICM process, and grepai owner prevent duplicate infrastructure; project activation only
controls whether a workspace may route through them.

## 5. Identity and fail-closed rules

1. Git repositories use the git common directory plus canonical worktree root.
2. Plain directories use the canonical path until `git init` migrates them.
3. Enabled entries contain both SHA-256 identities and the display root. Invalid or relative entries
   make configuration validation fail.
4. Hook identity-discovery failure means no-op, never global fallback.
5. MCP additionally requires the verified managed `.grepai` placement. A plausible directory name
   is insufficient.
6. The filesystem root, home directory, home ancestors, uninitialized workspaces, and unselected
   workspaces cannot own a project MCP namespace.

## 6. Integration ownership

The Claude hook remains global because Claude has one user hook file. In selected mode it is an
identity gate, not evidence that the current project is enabled.

Managed instruction blocks move from user-global files to the enabled project roots. Switching to
project-only removes only marker-owned global blocks and preserves all unrelated user content.

HZR-owned Codex and Claude Desktop MCP registrations are removed in project-only mode. Those clients
store MCP registrations at client scope, so keeping one would expose a fixed project tool in
baseline sessions. Operators may print and paste a workspace-pinned registration into a separate
client profile; HZR still enforces initialization and activation when it starts.

## 7. Accounting

`hzr stats` remains the global lifetime view. `hzr stats --workspace DIR` filters command rows by the
canonical project root and separator-safe descendants. The headline, command ranking, subsystem
totals, and optimizer-bypass denominator use the same filter.

Provider receipts that carry a workspace identity are labelled `project_matched` under
`hzr stats --workspace DIR` and count only matching rows (exact root or separator-safe
descendants). Older receipts without `project_path` stay unscoped: they appear in the global
lifetime view and are omitted from project-scoped provider totals so HZR never invents
attribution. Degraded-hook coverage remains process-local and is not project-filtered.

## 8. Acceptance criteria

- CLI parsing covers install, enable, disable, internal `init --if-enabled`, and scoped stats.
- Selected activation distinguishes two sibling directories deterministically.
- Project-only hooks contain `init --if-enabled`, are recognized as HZR-owned, and remain
  idempotently replaceable.
- Switching modes removes only HZR-owned MCP registrations and managed instruction blocks.
- MCP refuses uninitialized and unselected workspaces before a daemon request.
- Scoped efficiency and bypass summaries include the project root and descendants, but not a sibling
  with a common string prefix.
- The complete Rust, visualizer, bridge, bundle, and release smoke gates remain green.
