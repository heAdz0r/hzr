# RTK - Rust Token Killer (Codex CLI)

RTK is a token-optimized CLI proxy for shell commands. Use it for supported
commands whose output goes into context; unsupported commands remain available
through the normal shell.

## Rule

Prefer RTK wrappers for verbose commands:

Examples:

```bash
rtk git status
rtk cargo test
rtk npm run build
rtk pytest -q
rtk rg 'pattern' src
```

## Meta Commands

```bash
rtk gain            # Token savings analytics
rtk gain --history  # Recent command savings history
rtk raw <cmd>       # Tracked execution without filtering
rtk proxy <cmd>     # Alias of raw
```

Use raw mode for generated files, checksums, counts, machine parsing, or any
pipeline whose exact output matters. Check `rtk --help` for current wrappers.

## Verification

```bash
rtk --version
rtk gain
which rtk
```
