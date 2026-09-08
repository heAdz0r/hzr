# HZR 0.9.0

Makes the dashboard readable, and stops an optional component from failing HZR's
health.

## Things are named, not numbered

A registry of a hundred `Project 6a1be071` rows, a memory graph of
`Memory topic 14` nodes whose every leaf read "content is redacted", and an
agtx board of `Task 7ac3` cards could not be used to find anything. Withholding
those names protected nothing on a loopback dashboard showing the operator their
own machine; it removed the only way to tell one row from another.

By default HZR now shows:

- each registered workspace by its own directory, with the home-relative path
  beside it (`hzr` · `~/Programming/hzr`), and project search matches that path;
- each ICM topic by its topic name (`context-hzr`, `errors-resolved`) and each
  memory by what it says;
- each observed agtx task by its title and branch, bounded and stripped of
  control characters at the source;
- each observability trace by what it did (`optimized · rtk → grepai`) and the
  latest attributed session by its command families and operation count.

Every one of them keeps its pseudonymous identity published beside the name, so
cross-project isolation, session attribution and support workflows are
unchanged, and identity hashes remain the thing to quote. An install whose
loopback port other people can reach restores the pseudonymous view:

```toml
[privacy]
publish_workspace_names = false
publish_memory_content = false

[integrations.agtx]
publish_task_titles = false
```

## An absent optional component is not a failure

`hzr doctor` inspects every pinned engine binary, which turned the opt-in agtx
observer — absent on every ordinary install — into a mandatory health check and
failed all three 0.8.7 release bundle jobs at the clean-install smoke. Pins
marked `runtime = false` are now skipped, and the observer reports through its
own check: no component and no enrollment passes, an enrollment whose component
is missing warns and names the command that completes it, and neither state can
fail HZR.

## Measured

The agtx Observatory's resource budgets were verified at the acceptance fixture
scale of 1 000 tasks and 4 985 dependency references, against the real pinned
helper and a real SQLite store:

| Budget | Limit | Measured |
| --- | --- | --- |
| Full snapshot traversal | 3 000 ms | 59 ms (5 pages) |
| Peak helper RSS | 128 MiB | 8.7 MiB |
| Response page | 2 MiB | 192 KB |
| Warm dashboard p95 over 100 requests | 250 ms | 11.6 ms board, 7.9 ms events, 7.3 ms economics |
| Durable growth over unchanged polls | 1 KiB | 0 bytes |

Zero growth across roughly twenty unchanged polling cycles is the idempotency
rule working: an identical snapshot bumps no revision, writes no event and
touches no projection.

## Unchanged

The observer remains an optional local build. No prebuilt adapted agtx binary is
published while its pinned LICENSE/manifest provenance discrepancy is
unresolved. Polling observes transitions, not every message between agents.
Reported amounts, current-catalog API estimates and HZR output estimates stay
separate, and an unobserved value renders as an em dash with a reason rather
than a zero. Windows acceptance for the observer is still unverified; the
component reports itself unavailable there.
