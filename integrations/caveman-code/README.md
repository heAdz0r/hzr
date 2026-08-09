# HZR managed Caveman Code bridge

This directory pins `@juliusbrussee/caveman-code` 0.65.2 and adapts its SDK into
HZR's single-owner runtime. Build or packaging must run `npm ci`; HZR refuses to
launch if the runtime bridge differs byte-for-byte from the bridge embedded in
the Rust build, or if `package-lock.json` differs from the compiled SHA-256 pin
`f35932ef45e5b218ec3c065660b12db4e800ed85b356f636cd3b64dbbb2c3e98`.
The exact lock digest covers every transitive version and registry integrity,
while preflight separately verifies the installed Caveman Code manifest.

The certified runtime range is Node.js `>=20.18.1 <26`. The lower bound comes
from `undici` in the exact lock. The upper bound is fail-closed because
[caveman-code issue #46](https://github.com/JuliusBrussee/caveman-code/issues/46)
reports a Node 26 installation failure in the upstream `better-sqlite3` 11.10.0
chain. Node 25 remains supported by preflight. The npm override pins
`adm-zip` 0.6.0 to remove the `<0.6.0` high-severity DoS advisory; the production
dependency audit is expected to report zero vulnerabilities.

Managed mode disables Caveman's RTK hook, cave output/tool/ML compression,
telemetry, hooks, repo map, memory, automatic checkpoints, sessions, extensions,
skills, prompt templates, and subagents. This prevents a second execution,
retrieval, memory, or compression authority. Caveman remains the provider-aware
agent loop. Context, search, read, edit, write, recall, memory writes, and shell
execution are HZR tools over authenticated loopback routes. File operations use
the exact bundled fork-core `read` and atomic `write` commands; Caveman's native
file tools are not registered.

Before creating the session, the bridge retrieves one bounded HZR context plan
for the original intent. The plan is produced by the preserved fork memory
planner and centralized ICM, then injected once as explicitly untrusted data.
The bridge also appends a short, cache-stable response-density contract to the
system prompt before generation. This can reduce billed output; post-generation
deduplication alone cannot. JSON responses get an exact compact-JSON contract,
while text responses preserve code, identifiers, commands, paths, errors,
numbers, evidence, and caveats. The bridge fails closed if upstream removes any
required setter, changes an invariant, enables a forbidden tool, or emits
malformed JSONL.

After a completed model run, the bridge reads Caveman's aggregate provider token
counters and records input, output, cache-read and cache-write usage through
`POST /v1/usage`. Caveman's locally calculated dollar figure is deliberately not
stored as billed cost. A ledger failure is attached to the terminal result as a
bounded warning and never discards an otherwise valid model answer.

The integration relies on these public 0.65.2 APIs: `SettingsManager.inMemory`,
`DefaultResourceLoader`, `SessionManager.inMemory`, `createAgentSession`, session
subscriptions and prompts, the `read`/`edit`/`write` tool exports, and the public
memory/repo-map/checkpoint setters. HZR additionally inspects the three private
boolean fields behind those setters because Caveman does not expose getters for
repo-map and checkpoint state. That deliberate compatibility tripwire may need
an adapter update on an upstream release; managed mode will stop instead of
silently enabling a duplicate subsystem.

The bearer token is accepted only through `HZR_DAEMON_TOKEN`, never as an
argument or JSON field. `HZR_DAEMON_URL` must be an HTTP loopback origin. The
launcher accepts the exact 64-character `hzrd` token contract and redacts it
from captured diagnostics. On Unix, both the Node version probe and every agent
run get a dedicated process group. Normal completion, timeout, or cancellation
terminates any remaining descendants so provider subprocesses cannot leak past
the managed run.

## Bridge contract

The process reads one JSON object from stdin with `request_id`, `prompt`,
`response_format` (`text` or `json`), and `max_turns`. It writes contiguous
JSONL events with the same request ID: one `ready`, zero or more `agent_event`
records, and either `result` or `error`. JSON mode accepts only a model response
that parses as one JSON value. Output is bounded by the Rust launcher and a
nonzero bridge exit is always an error. The run deadline covers stdin delivery
as well as model execution, and output-drain tasks are cancelled if a detached
process keeps a pipe open after the managed process group is terminated.

Provider credentials remain Caveman SDK credentials or provider environment
variables. Persistent Caveman settings and sessions are not loaded. Auth files,
if used, live only under the HZR-provided `HZR_AGENT_DIR`.

All daemon responses are streamed into a byte counter and rejected above
512 KiB before UTF-8 decoding. The initial context request has a separate
30-second deadline and the enclosing Rust process deadline remains authoritative
for the complete run.
