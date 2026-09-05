# HZR 0.8.3

HZR 0.8.3 removes three accumulations that grew without bound on a working installation: build
artefacts in the developer's `target/` directory, lock files and idle registrations in the
daemon's fork directory, and one closed "gap" per successful command in the accounting coverage
state. Nothing about what is measured changes; what changes is that the daemon and the
repository stop consuming disk, CPU and I/O for work that was already finished.

## The daemon sweeper does only the work that exists

The accounting sweeper listed the fork directory twice a second and ran a full drain attempt for
every registered context whether or not it had receipts: a lock file, two journal probes and a
rotation probe each. Every drain and every producer append also created a lock file that nothing
removed, so one fork directory had accumulated 18 500 of them. A rejected receipt batch was retried
every second forever, and each retry rewrote and fsynced the coverage state. macOS flagged `hzrd`
at 70 % CPU with every sample in `write(2)` and at 8.6 GB written in 27 minutes.

The sweeper now lists the directory once, drains only correlations that have a journal, retires
completed registrations and registrations that produced nothing for 24 hours together with their
lock files, removes stale locks (500 per sweep, after the 600 s orphan grace), quarantines a
rejected batch once as `.jsonl.rejected` with one closed gap, and sweeps every five seconds
instead of every second while no producer is active.

## Successful commands are not gaps

Every command registers a fork-producer context and recovers it when its receipts drain. Until
0.8.2 the recovery closed the registration's interval instead of removing it, so every successful
command remained in `accounting-coverage.json` as a historical gap: 2 067 of 2 071 intervals on
one installation, a 714 KB file rewritten twice per command. A registration recovered inside the
pending grace is now removed, intervals written the old way are pruned on the next write, and a
repeated inspection of one open condition rewrites the state at most once a minute. Lifetime
missing-operation totals drop by the number of successful commands they had absorbed; real gaps
are unchanged.

## The approved command carries no policy JSON

The PreToolUse hook used to return every rewritten command as a script that began with
`export HZR_INTERNAL_EVASION_JSON='{"class":"e5_pipeline_or_redirect",…,"avoidable":true,…}'`
before the engine environment and the command itself. A host permission classifier that reads
the tool input judged that text, and in one Claude Code auto-mode session it refused a large
share of ordinary commands, `df -h | tail -1` among them. The classification is consumed only
after the command has run, so it now travels with the accounting registration and the daemon
attaches it to the receipts; the approved command carries the engine environment, which now
opens with a comment naming what it is, and nothing else. Only the T4 fidelity hatch, which has
no registration, still exports its attribution.

## Build artefacts are bounded

Cargo never removes superseded artefacts, and with full debug info for every dependency variant
the workspace's `target/debug` had reached 58 GB. The dev profile now emits line tables only and
no debug info for dependencies (`CARGO_PROFILE_DEV_DEBUG=2` restores it for one build), and
`scripts/target-hygiene.sh`, run first by `scripts/complete-gate.sh`, removes artefacts idle for
more than seven days and resets a debug directory that still exceeds 20 GB. Release artefacts and
the component cache are never touched.

## Upgrade

`hzr update` installs 0.8.3 and runs the reference-state pass; the first session on the new
version schedules it otherwise. The first write to the coverage state prunes the settled
registrations described above; existing lock files and abandoned registrations are retired over
the first sweeps. Reconnect open MCP sessions after the upgrade. Full details:
`docs/releases/v0.8.3.md` and `CHANGELOG.md`.
