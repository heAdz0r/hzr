# HZR 0.11.5

`hzr exec` keeps accepting new commands when old execution history reaches its
20,000-record limit. Old operation IDs remain protected against duplicate execution.

## What broke

The daemon kept every full command result forever and counted output directories
toward the record limit. Once the history reached 20,000 entries, every new execution
failed with `HTTP 503 execution_store_capacity`, even with no commands running and
space left within the disk quota. Restarting the daemon did not reclaim anything.

## What changed

- Under record or disk pressure, the daemon reclaims terminal results older than one
  hour. It keeps the newest 1,000 full records when the disk budget permits; running
  commands, recent completions and live approval capabilities remain protected.
- Before removing any result or captured output, it commits a compact replay receipt
  containing the operation ID, workspace, request hash and terminal state. Retrying
  the same ID cannot execute the command again; a changed request is still rejected.
- A reclaimed result retains its existing terminal state and reports
  `execution_result_expired`. Captured-output requests return HTTP 410 with the same
  code and `recoverable=false`. Existing clients report the job error instead of
  rerunning the command or waiting indefinitely.
- Recovery handles interruption between committing a receipt and removing its files.
  Output readers hold a lease through the complete read and hash check, so reclamation
  cannot remove a file they are reading.
- Replay receipts use a private SQLite database capped at 256 MiB, counted within the
  existing 2 GiB execution-store budget. The existing worst-case active-command
  reservation remains in force. A full, missing, incompatible or damaged receipt
  store fails closed; HZR never deletes replay protection to accept a new command.

## Security

The managed runtime lockfile updates `zod` to 4.6.7 and `fast-uri` to 3.1.9,
fixing the new high-severity constrained-collection validation advisory and URI
normalization advisories reported by `npm audit`. Production dependency auditing
reports no vulnerabilities with this lockfile.

## Upgrade and data retention

Run `hzr update` or use the installer. Existing execution records are adopted without
rerunning commands; old result payloads are reclaimed only when capacity is needed.
Save output you need to retain independently of execution history. The accounting
ledger, memory, index and project files are unchanged.

After the first compaction, do not downgrade to a version before 0.11.5: older
daemons do not understand the compact replay receipts. Keep `replay.sqlite` and
`replay-required` together with the execution store; deleting either is not a safe
cleanup procedure.
