# HZR fork-core snapshot

This directory preserves the complete current `heAdz0r/rtk` worktree used as
HZR's inner execution and context engine. It is source code, not a downloaded
build artifact and not a partial compatibility port.

Snapshot identity:

- source: `https://github.com/heAdz0r/rtk.git`;
- branch: `feat/upstream-0.42-fork.1`;
- source HEAD: `5f403c465cbdbe148e9ca03e0ac8e856eef0bfee`;
- package version: `0.44.1-fork.1`;
- included files: 516;
- tracked deletions: 4;
- canonical snapshot v2 SHA-256: `f4296ec404f461d6fc03c966c0dc79caee6c3118a73d1ed1a078ded5529f0a16`;
- preserved v1 content-manifest SHA-256: `072a62adc754b728ec99a507d2c1a223d83077d067a9249a26d357eec890b4cc`.

`rtk/` is immutable. HZR integration belongs in sibling control-plane crates,
process/environment adapters, or an explicitly reviewed overlay. Do not format,
rename, prune, or mechanically update files under `rtk/`.

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

Verify bytes and run the original suite through a synthetic temporary Git
history (required by the fork's churn tests):

```bash
scripts/verify-fork-core.sh --test
```

Some snapshot files intentionally match the fork's own `.gitignore` patterns.
They must be force-added when creating the HZR repository's initial commit;
`SHA256SUMS` is the authoritative inclusion list.
