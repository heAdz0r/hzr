# Audit evidence index

Captured during the fresh 2026-09-04 audit of HZR 0.7.1, commit fbf204d48704160407e3f06035ac32b9cf0bc67a. Previous PRDs were not used.

- `stats-project-7d.json`, `stats-global-7d.json`: frozen early rolling-window estimates. Some audit activity is included. Local bytes/4 accounting, not provider usage.
- `fleet/snapshot.json`: independent per-project reconciliation, safe aggregates and project inventory; see the fleet report for snapshot timing and missing directories.
- `search-summary.json`: selected path/strategy metadata from a real scoped semantic query; source snippets excluded.
- `plan-summary.json`: candidate/path/budget metadata from context planning; memory content excluded.
- `source-gate-timeout.log`: the first bounded source-gate attempt, exit 124.
- `bridge-tests.log`: the eight passing bridge tests.
- `internal-env-failure.txt`, `internal-env-isolated.txt`: bounded diagnostic excerpts from the reproduced test and isolated rerun; warning noise excluded.

The full source gate subsequently failed with exit 101 in fork `internal_env`. Its filtered terminal result, source sequencing and targeted reproductions are documented in [Verification](../20260904_195133_Verification.md). A timeout and an explained test failure remain incomplete/red validation, respectively.

No raw process argv, credentials, memory text, private source contents, or historical PRDs are included. Absence of an observation is not proof of absence of activity.

