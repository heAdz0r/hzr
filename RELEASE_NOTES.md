# HZR 0.12.0

The Bash hook stops approving commands your deny rules forbid when they arrive
wrapped, and fork-core moves to upstream RTK v1.1.0. grepai moves to 1.2.0.

## What broke

HZR rewrites a Bash command into a managed form the host's own `Bash(...)` rules
cannot match, so fork-core's permission verdict is the only place an operator's deny
rule meets that command. The gate matched each segment only as written. With
`allow: Bash(timeout:*)` and `deny: Bash(git push:*)`, `timeout 30 git push` was
answered with `allow`; so were `env FOO=1 git push`, `bash -c 'git push'` and, under
`bypassPermissions`, `HUSKY=0 git push` and `nohup git push`. A quoted `$'\''` could
also hide `; rm -rf /` inside an allowed `ls`, and a settings file with a UTF-8 BOM
had its deny rules silently skipped.

## What changed

- Deny and ask rules match every reading of a segment the rewrite can look through:
  assignments, `env`, `timeout`, `nice`, `nohup`, `time`, `stdbuf`, keyword
  prefixes, `uv run`, your configured transparent prefixes, `sudo` and a bare
  `xargs`. Allow rules still match only the command as written, so no reading can
  widen an approval.
- A deny rule that matches inside a quoted `sh -c`, `bash -c` or `zsh -c` script
  denies the whole command. A wrapper is never auto-approved by an outer allow rule.
- Leading redirects, `{ … }` grouping, heredocs and ANSI-C quoting no longer hide a
  command. Constructs the gate cannot read are never auto-approved.
- A settings file that cannot be read or parsed asks for every command. A BOM is
  tolerated; `CLAUDE_CONFIG_DIR` is honoured.
- Under `bypassPermissions`, a command matching one of your ask rules runs
  unchanged so the host applies the rule itself. That run is unfiltered.
- `cat` returns the original bytes of non-UTF-8 and CRLF files, a wrapped tool's
  `--help` is the tool's own, and rewrites keep your spacing between commands.

## Engines

- fork-core `1.1.0-fork.1`: fixes re-implemented from upstream RTK v1.1.0's
  Apache-2.0 `src/` tree. Upstream's BUSL-1.1 engine, proxy and plugin crates are not
  used.
- grepai 1.2.0: watcher, crash-safety and security fixes, and only changed chunks are
  re-embedded. Existing indexes are adopted on first watch without re-indexing.
- ICM stays at 0.10.61.

## Upgrade

Run `hzr update` or use the installer. No data migration is needed. Commands that
used to be approved through a wrapper may now be denied or prompt; that is the rule
you wrote taking effect. Savings on those commands and on `cat` of non-UTF-8 files
fall, because they now run unfiltered or byte-exact.
