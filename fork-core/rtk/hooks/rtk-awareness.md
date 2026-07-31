# RTK - Rust Token Killer

RTK is a token-optimized proxy for shell commands. The Claude Code Bash hook
uses `rtk rewrite` as its single source of truth and leaves unsupported commands
unchanged. Native agent tools such as Read, Grep, Edit, Write, and Task are not
replaced by the Bash hook. Independent multiline commands are rewritten line by
line; pipeline producers are rewritten only before plain `head`, `tail`, or `cat`.

## Exact Output

Filtered output may contain `[100 more lines]`. Hook bypass applies to native
commands; explicit `rtk rg ... > file` stays filtered. Use `rtk raw` for source
data or exact machine processing.

```bash
rg pattern . > matches.txt          # File redirection automatically bypasses rewrite
rtk raw rg pattern . > matches.txt  # Explicit, tracked raw execution
rtk raw rg pattern . | awk '{print $1}'
rtk raw ssh host 'rg pattern /srv' > matches.txt
```

Use `rtk raw` for exact output, checksums, counts, generated files, and parsers;
use filtered output when the result goes directly into agent context.

## Search

```bash
rtk rgai "intent query"  # Semantic search; delegates to grepai when available
rtk rg <pattern> [path]  # Ripgrep syntax and semantics
rtk grep <pattern> [path] # POSIX/BSD grep syntax and semantics
```

Use `rgai` for discovery and `rg`/`grep` for exact or regex matching. `rtk rg`
and `rtk grep` are separate engines; flags are passed to the selected engine.

## Reads And Writes

```bash
rtk read <file>                         # Automatic compact level
rtk read <file> --from <N> --to <M>    # Exact line range
rtk write patch <file> --old @old --new @new
rtk write replace <file> --from old --to new
rtk write set <file> --key a.b --value value
```

Range reads automatically preserve exact content. Use `rtk write` when its
atomic and idempotent operations fit the edit; native editing tools remain valid.

RTK's never-worse guard returns raw output when filtering would cost more tokens.
It does not guarantee completeness; use `rtk raw` for exact unfiltered output.
