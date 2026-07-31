<!-- rtk-instructions v4 -->
# RTK - Token-Optimized Shell Commands

RTK filters supported shell-command output before it enters the agent context.
With the global Bash hook installed, unsupported commands pass through and rewrite
decisions delegate to `rtk rewrite`; native agent tools remain outside that hook.

## Operational Rules
- With the hook active, normal shell commands may be used; otherwise prefix
  supported commands with `rtk`.
- Use `rtk` explicitly for `gain`, `discover`, `raw`/`proxy`, `rgai`, and compact reads.
- Use `rtk raw <cmd>` when exact output is required; `raw` is an alias for `proxy`.
- Check `rtk --help` instead of relying on a copied command catalog.
- The hook rewrites independent multiline commands and safe display pipelines;
  producers stay native before parsing or transforming consumers.

## Exact Output

RTK output can contain summaries such as `[100 more lines]`. Hook bypass applies to
native commands; use `rtk raw` for source data, checksums, counts, or parsing.

```bash
rg pattern . > matches.txt          # Hook detects stdout redirection; stays native/raw
rtk raw rg pattern . > matches.txt  # Explicit raw mode with RTK usage tracking
rtk raw ssh host 'rg pattern /srv' > matches.txt
```

Use explicit raw mode for exact pipelines even when they do not redirect:

```bash
rtk raw rg pattern . | awk '{print $1}'
```

## Search

```bash
rtk rgai "intent query"   # Semantic discovery; grepai-backed when available
rtk rg <pattern> [path]   # Ripgrep flags and regex semantics
rtk grep <pattern> [path] # POSIX/BSD grep flags and regex semantics
```

`rtk rg` and `rtk grep` are distinct engines. Do not translate flags between
them. Prefer semantic search for intent and exact search for known symbols/text.

## Files

```bash
rtk read <file>                         # Automatic compact level
rtk read <file> --from <N> --to <M>    # Exact line range
rtk write patch <file> --old @old --new @new
rtk write replace <file> --from old --to new
rtk write set <file> --key a.b --value value
```

Range reads automatically preserve exact content. `rtk write` is useful for
atomic, idempotent transformations; native editing tools are still valid.

The never-worse guard emits raw output whenever filtering would use more
estimated tokens, so compression cannot make a supported result larger.
<!-- /rtk-instructions -->
