# HZR 0.8.0

HZR 0.8.0 redesigns the dashboard and corrects execution, retrieval and accounting behavior
that could increase agent work or make local output reduction look like proven task savings.
Local installation and client coverage are verified separately from source checks.
Provider-billed savings require task-level evidence and are not claimed by this release.

## A dashboard for operational decisions

Overview, Projects, Memory & index and System separate service availability, local output
estimates and economic evidence. Project/activity filters, responsive navigation, loading,
error and stale states make failures and uncertainty visible. Unmeasured savings are shown
as not established.

## Commands that finish and evidence that stays exact

- Durable execution jobs expose start, bounded wait and cancellation with stable IDs.
  Repeated starts do not replay side effects; interrupted jobs stay explicit.
- Failed test wrappers retain error blocks, assertions and source locations. Unknown failure
  formats preserve captured output. Successful summaries retain compiler warning signals.
- Git diff checks, quiet mode and exit-code mode preserve native status and exact diagnostics.
  A failed Git validation cannot be reported as a successful HZR command.
- Explicit full reads remain valid when the whole file is needed. Typed reads add batch
  budgets, exact ranges, SHA-256 snapshot checks and completeness metadata.
- Interpreter reads without an equivalent filter no longer trigger optimizer-only approval.
  Read-only SQLite diagnostics use the managed bounded engine.
- Scoped semantic and graph paths resolve against the correct workspace. Context plans budget
  delivered evidence, expose optional memory retrieval and support exact memory expansion.

## One control plane, explicit ownership

ICM ownership survives daemon restarts through verified runtime records. SIGTERM follows graceful
shutdown; repeatedly unready owned children recover without killing attached or foreign processes.
Producer accounting and execution channels retain typed attribution. Fleet statistics read one
consistent time window, retain historical project IDs and provide private JSON export.

MCP adds typed read and execution lifecycle controls, accepts valid empty writes and adds exact
memory lookup. Host adapters preserve host permission decisions and avoid blocking native reads
merely to force a retry through HZR.

## Measured claims

The new paired-task harness requires task-bound provider usage, independent acceptance and
matched configurations. It tracks rereads and rejected-task cost. Offline fixtures validate the
measurement protocol; they do not establish economic superiority.

The earlier cache experiment now rejects aggregate counters as per-run evidence. Local bytes/4
estimates, provider usage, host delivery and completed-task quality remain distinct. No billed
or total-task savings percentage is claimed without a real paired evaluation.

## Scope and verification

The implementation follows the fresh September 4 audit and its approved P0/P1 scope. The user
also approved inclusion of the completed P2 branch `feat/prd-p2-agent-efficiency` (`9016b29`):
cache-prefix-aware placement, concise generation by default, tiered instructions and explicitly
uncalibrated ranking diagnostics. Calibration and a real paired economic evaluation remain
unproven; this release does not invent either result. The 0.8.0 verification report records
exact source and isolated installation checks. Existing integrations adopt the new runtime when
the installed bundle is updated; source verification alone does not reconfigure clients.

## Upgrade

Run `hzr update` to install the latest published bundle, then run `hzr doctor` in each active
workspace. Run `hzr doctor --reconcile-fleet --dry-run` to inspect managed configuration updates
and `hzr doctor --reconcile-fleet` to apply them across registered workspaces. Existing project
files and durable memory are retained. Reconnect running MCP clients after the upgrade so they
start the new binary.
