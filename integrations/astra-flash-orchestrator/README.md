# HZR integration of astra-flash-orchestrator

HZR adapts the upstream orchestration pattern: a user-selected parent scopes and
reviews tasks, while an explicitly selected worker executes bounded work. The
unchanged MIT source is retained in `upstream/` with its pinned inventory.

The shipped implementation uses `hzr settings`, `hzr delegate`, the bundled
managed-agent SDK and HZR's private Node runtime. It needs no external Router,
Git, npm, Python, uv, pip, or additional runtime download on the user's machine.
Only the configured provider API is contacted for inference.

```sh
hzr settings login --provider opencode-go
hzr settings delegation --provider opencode-go --model deepseek-v4.1-flash --enabled true
hzr delegate --file task.md
hzr settings delegation --enabled false
```

The parent may be any model in Codex or Claude. It retains planning and final
acceptance. The worker receives the bounded task and HZR context; it does not
receive the full parent transcript or a credential in its prompt. Keys are stored
privately per user and never included in configuration exports or bundles.

Open the existing HZR visualizer at `?view=delegation` in Codex's browser panel
to see actual provider/model, tool activity, worker state and provider usage.
These are HZR-managed workers, not native Codex child threads. Native Codex UI
identity cannot be claimed for CLI-managed execution.

The upstream native Router adapter was evaluated in a live acceptance test.
It is not part of the HZR runtime or installer: that separate adapter required
external dependency installation and would add another service and lifecycle.
The HZR adaptation keeps one tool control plane and uses existing bundle assets.
Do not run the archived upstream installer to configure HZR.

Build-time Python verifies provenance; it is not an end-user dependency.
