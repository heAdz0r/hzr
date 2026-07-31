# hzr-exec

`hzr-exec` is HZR's typed process boundary around the complete bundled
`heAdz0r/rtk` fork. It does not implement RTK routing, parsing, filtering,
permissions, read/write, search, or memory behavior. Those stay in the immutable
fork-core binary; this crate verifies and invokes that binary.

## Hard fork-core contract

- The only accepted runtime is `rtk 0.44.1-fork.1` with both the fork `rewrite`
  and `proxy` CLI contracts. Stock RTK `0.44.1` is rejected.
- Missing runtime paths, missing binary, version drift, capability drift,
  timeout, invalid UTF-8, empty rewrite output, and unknown exit status produce
  a typed `Deny`. Managed execution never falls back to stock RTK or directly to
  the raw command.
- The exact requested shell text, including compound and multiline syntax, is
  submitted to fork-core `rewrite`. HZR neither parses it nor validates it
  against a reduced argv model.
- Fork exit statuses remain inside the adapter: `0` is an approved fork rewrite,
  `1` becomes exact-binary `rtk proxy` execution, `2` is `Deny`, and `3` is
  `Ask` with the complete fork-produced command retained as the proposal.
- Rewritten shell commands prepend the verified binary directory to `PATH`, so
  every emitted `rtk` resolves to the probed fork before any other installation.
  Direct fork invocations use the verified absolute binary as exact argv.

## Centralized fork state

`ForkRuntimePaths::from_data_root` gives fork-core one HZR-owned state boundary:

```text
<data-root>/fork/mem.db
<data-root>/fork/history.db
<data-root>/fork/tee/
<data-root>/fork/audit/
```

Every probe and invocation receives `RTK_MEM_DB_PATH`, `RTK_DB_PATH`,
`RTK_TEE_DIR`, `RTK_AUDIT_DIR`, and `RTK_TELEMETRY_DISABLED=1`. Runtime
directories are created with private Unix permissions. `HOME` and the rest of
the inherited environment remain intact so Git, npm, SSH, and registry
credentials keep working; only the verified fork directory is prepended to
`PATH`.

## Shared runner

`PinnedRtkAdapter::detect(ForkCoreConfig)` performs the fail-closed probe.
`adapter.runner()` returns the only public direct fork launcher:

- `ForkCoreRunner::execute(ForkCoreInvocation)` runs exact fork argv with typed
  cwd, timeout, bounded stdout/stderr capture, cancellation, and optional stdin;
- `ForkCoreRunner::envelope` exposes the same invariant to callers that need to
  customize the typed execution envelope;
- `ForkCoreRunner::std_command` and `std_command_os` return an exact,
  environment-configured `std::process::Command` for CLI TTY passthrough or
  Unix `exec`; the `OsString` form preserves non-UTF-8 Unix argv.

Search, context, daemon, and CLI integrations must use this runner rather than
launching `rtk` themselves.

## Capture and policy envelope

- `ExecutionPipeline::start` exposes bounded live stdout/stderr events and an
  independently captured completion result.
- stdout and stderr remain separate byte streams. Exit code, Unix signal,
  duration, timeout, and cancellation are separate typed fields.
- capture stays inline to its memory limit, then spills to a unique file or
  truncates at the declared cap. Total byte count and SHA-256 still cover the
  complete observed stream.
- `Ask` and `Deny` never spawn a process.
- only an HZR-native `RewriteSource::HzrPolicy` may use the legacy pre-spawn raw
  fallback. `RewriteSource::Rtk` is always fail-closed.
- on Unix each captured command owns a process group; timeout and cancellation
  terminate the group.

`choose_never_worse` remains a deterministic byte-view selector. It never
replaces the canonical captured result or fork-core's command semantics.
