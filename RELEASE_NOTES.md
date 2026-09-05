# HZR 0.8.1

HZR 0.8.1 is a doctor and lifecycle hotfix. It repairs the two findings that kept `hzr doctor`
red after the 0.8.0 upgrade, makes accounting recover from daemon-free intervals, and adds a
post-upgrade pass that brings every registered workspace, index and engine back to the reference
state without visiting each project.

## Doctor findings that could not be fixed

- `hzr_doctor` over MCP rejected its own output because the 0.8.0 `readiness` field was missing
  from the strict tool schema. The schema now declares every report field.
- ICM servers whose launching daemon had been killed, crashed or torn down by a release smoke
  fixture were reported as foreign duplicate owners with no supported remedy. Doctor now
  distinguishes `orphaned_engine_processes` (HZR installation removed, parent exited) from
  foreign processes, and `hzr doctor --fix` stops the orphans after re-verifying PID and argv.
  Foreign processes are still never signalled.
- The bundled ICM engine exits with its launcher when the daemon sets
  `ICM_EXIT_WITH_PARENT_PID`, so the orphan class cannot reappear from a killed daemon.

## Accounting that recovers

Hook rewrites made while the daemon was down produced fork receipts under a correlation that
never had a context file; those receipts stayed "undrained" indefinitely. The hook now registers
the context locally, and the daemon sweeper drains context-less journals older than ten minutes as
`unattributed` operations. Rejected batches are quarantined and recorded as a producer gap.

## Fleets that converge

- Registrations whose worktree directory was deleted are pruned during
  `hzr doctor --reconcile-fleet` instead of failing the fleet closure forever. A root whose
  parent directory is also absent is retained in case the volume is unmounted.
- The first `hzr init --if-needed` (the SessionStart hook) on a new version schedules one
  detached `hzr doctor --reconcile-fleet --fix`; `hzr update` runs it in the foreground. The
  marker `runtime/reference-state.json` and the `reference_state` doctor check show whether the
  pass completed, is still running, or ended with findings.

## Upgrade

```bash
hzr update
```

or

```bash
curl -fsSL https://raw.githubusercontent.com/heAdz0r/hzr/v0.8.1/install.sh | sh
```

After installing, open any HZR session or run `hzr doctor --reconcile-fleet --fix` once. The
reconciliation stops orphaned engines, prunes stale registrations and refreshes managed
instruction blocks and Codex pins in every registered workspace. Open MCP sessions must be
reconnected so clients pick up the new schema.
