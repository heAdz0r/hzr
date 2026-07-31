# HZR fork-core engine and import baseline

This directory began as the complete `heAdz0r/rtk` worktree used as HZR's inner
execution and context engine. Tag `v0.1.0` preserves that byte-for-byte import.
After the baseline, `rtk/` is HZR-owned source: fixes and extensions happen here,
not in `/Users/andrew/Programming/rtk`.

Snapshot identity:

- source: `https://github.com/heAdz0r/rtk.git`;
- branch: `feat/upstream-0.42-fork.1`;
- source HEAD: `5f403c465cbdbe148e9ca03e0ac8e856eef0bfee`;
- package version: `0.44.1-fork.1`;
- included files: 516;
- tracked deletions: 4;
- canonical snapshot v2 SHA-256: `f4296ec404f461d6fc03c966c0dc79caee6c3118a73d1ed1a078ded5529f0a16`;
- preserved v1 content-manifest SHA-256: `072a62adc754b728ec99a507d2c1a223d83077d067a9249a26d357eec890b4cc`.

The baseline metadata below is immutable provenance, not a permanent source
freeze. Changes under `rtk/` must preserve inherited functionality, remain
auditable in HZR Git history, update current-engine identity/parity metadata and
pass the complete deterministic regression suite. Never sync changes back to
the archived source repository automatically.

The import includes every tracked file present in the source worktree plus every
untracked non-ignored file. `.git`, ignored build/index/cache/data artifacts, and
ignored machine-local settings are excluded. Provenance artifacts:

- `SNAPSHOT.toml` — source identity, dirty-state hashes and aggregate v2 identity;
- `SNAPSHOT_V2.tsv` — canonical ordered paths, entry types, Git-portable modes, sizes and digests;
- `FORK_FILES` — complete included path set;
- `SHA256SUMS` — byte-level file checksums;
- `TRACKED_DELETIONS` — tracked files absent from the worktree;
- `TRACKED_CHANGES.patch` — binary-capable diff from source HEAD;
- `SOURCE_STATUS` — original porcelain worktree state.

Current-engine artifacts evolve after the baseline:

- `CURRENT_ENGINE.toml` — parent baseline, current manifest/content digests and ownership contract;
- `CURRENT_ENGINE_V1.tsv` — canonical current types, modes, sizes and content hashes;
- `CURRENT_FILES` — current HZR-owned engine path set;
- `CURRENT_SHA256SUMS` — current engine byte checksums.

After an intentional engine delta, run `scripts/refresh-current-engine.sh`,
review the generated identity change, update parity documentation, then run the
verifier. The refresh never modifies the archived `/Programming/rtk` source.

Verify bytes and run the original suite through a synthetic temporary Git
history (required by the fork's churn tests):

```bash
scripts/verify-fork-core.sh --test
```

Some snapshot files intentionally match the fork's own `.gitignore` patterns.
They must be force-added when creating the HZR repository's initial commit;
`SHA256SUMS` is the authoritative inclusion list.
