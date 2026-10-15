# HZR 0.11.3

Bypass mode no longer asks for approval on hook-managed commands such as `rm`.

## The hook no longer hides commands inside `<shell> -c`

When fork-core has no filter for a command (`rm`, `mv`, compound commands and the like) it
selects its tracked raw proxy. Until now that proxy wrapped the agent's command as
`rtk proxy /bin/zsh -c '<command>'`. Claude Code cannot inspect a `-c` script, so in
`bypassPermissions` mode it still stopped and asked — "This shell -c script runs rm and could
not be checked" — even though the HZR hook had answered `allow`. Without HZR the same command
ran without a prompt, so the prompt was a regression HZR introduced.

The proxy route now keeps the agent's command visible to the host:

- **A plain word list is proxied as argv** — `rm -f build/out.txt` becomes
  `rtk proxy rm -f build/out.txt`, so the raw output is still tracked.
- **Anything only the shell can interpret runs verbatim** after the engine exports:
  operators (`&&`, `;`, `|`), quoting, expansions, assignments (`FOO=1 cmd`) and builtins
  (`cd`, `exit`, `echo`, …). It runs in the same shell, exactly as written.
- Windows keeps the `cmd.exe /C` proxy; the prompt is specific to POSIX hosts.

Two regression tests guard it: an adapter test and an end-to-end `hooks dispatch` test in
`bypassPermissions` mode that fails if any managed command wraps the agent's command in `-c`.

## Upgrade

`hzr update` (or the installer). No data or configuration changes.
