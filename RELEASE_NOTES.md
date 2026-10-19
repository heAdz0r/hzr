# HZR 0.11.4

`hzr delegate` no longer fails everywhere because one index watcher died, and the watcher
no longer dies on a burst of file changes.

## What broke

A new repository started three Claude Code agents. Claude Code keeps agent worktrees under
`.claude/worktrees/` inside the repository, so the grepai watcher indexed three more full
checkouts, and creating them was a burst of file events. Upstream grepai 0.37.0 delivers
debounced events into a 100-slot queue with a non-blocking send and treats a full queue as
fatal: the watcher exited with `file event queue is full`.

HZR kept the dead watcher as a tombstone for its 15-minute idle TTL, and grepai health — which
aggregates every workspace — reported `degraded`. The delegation bridge refused to start a
worker while grepai was anything but `ready` or `stopped`, so every `hzr delegate` in every
repository failed with `HZR grepai is not available on demand: degraded`, including a fresh
worktree created only for the worker. `hzr doctor` said only `state Degraded`, and
`hzr doctor --reconcile-fleet --fix` could not finish clean either: it reported four
"identity mismatch" errors for temporary workspaces a cleaner had emptied.

## What changed

- **The watcher survives bursts.** A new grepai patch
  (`patches/grepai/0.37.0-event-queue-backpressure.patch`) makes delivery wait for queue
  space, in order, until the watcher stops, instead of exiting.
- **Agent worktrees stay out of the index.** HZR adds `.claude/worktrees` to the managed
  grepai `ignore:` list — for new indexes at initialization and for existing ones when their
  watcher next starts. Your own entries are kept.
- **A degraded index does not block delegation.** The worker starts with a warning; HZR
  restarts the workspace's watcher on the worker's first search.
- **`hzr doctor --fix` clears failed watchers** through a new daemon route, so the daemon is
  healthy at once instead of after the TTL. The `daemon` check now names each degraded engine
  with its detail and remedy.
- **No more `watcher_maintenance_failed` noise.** Reaping a watcher that already exited no
  longer asks grepai to stop it.
- **Fleet reconciliation prunes vacated roots.** A registered root whose repository was removed
  and which holds only HZR's `.grepai` link is pruned like a deleted root, link included.

## Dependencies

The managed agent runtime's transitive dependencies were refreshed (`undici` 7.30.1 among
them) and `basic-ftp` is overridden to 6.2.3, clearing every high-severity `npm audit`
advisory that had turned CI red since 0.11.3.

## Upgrade

`hzr update` (or the installer), then `hzr doctor --fix` once to clear any failed watcher the
old daemon still holds. Existing index configs gain one `ignore:` entry the next time their
watcher starts; nothing else in your data or configuration changes.
