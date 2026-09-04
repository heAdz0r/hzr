# Fork warning identity verification — 2026-09-05

The warning baseline change is entirely explained by source-line movement relative to commit `b21d0c7`. This verification is read-only with respect to source and warning baseline; it does not run Cargo.

Authoritative input: `/tmp/hzr-080-fork-warnings.jsonl`, supplied to a computation-only Python process through `hzr read`. Old source came from `git show b21d0c7:fork-core/rtk/src/git.rs` and the equivalent `tests/git_cli_parity.rs` path. Current source came from exact whole-file managed reads. These are the only fork files changed relative to that commit.

The parser exactly follows `scripts/verify-fork-clippy.py`: retain compiler-message events at warning level; take the first primary span; form `code|message|file_name|line_start`; sort the rows; hash their newline-joined UTF-8 bytes with SHA-256. Duplicate target diagnostics remain separate rows.

For each changed file, `difflib.SequenceMatcher(None, old.splitlines(), current.splitlines(), autojunk=False).get_matching_blocks()` mapped each unchanged current line back to its old line number. No changed or unmatched line was guessed, deleted, or normalized. Only mapped line numbers were substituted; codes, messages and source paths were untouched.

| Check | Result |
| --- | --- |
| Warning count | 141 |
| Current raw row SHA-256 | `5b013a3c862ea687ad6e8c12a9ce9be2370113b7620a60018c81a90fcd19bcd8` |
| SHA-256 after mapping unchanged lines back | `3aa3353d899a0fa403f018310b913a5919095d746744b68e6ac0af65ab8fbfc8` |
| Existing expected SHA-256 | `3aa3353d899a0fa403f018310b913a5919095d746744b68e6ac0af65ab8fbfc8` |
| Exact old hash recovered | true |
| Unmapped warning rows | 0 |
| Shifted warning rows | 2 |

Both shifted rows are the same target-duplicated diagnostic: `clippy::needless_return`, `src/git.rs`, current line 1229 → old line 1226. `tests/git_cli_parity.rs` introduces no changed warning identity. All remaining rows remain identical under the ratchet's identity definition. No new or removed warning identity is present in this evidence.

An initial direct interpreter file-read attempt was rejected by installed HZR policy before execution. The successful proof instead supplied source and log bytes through managed readers and `git show`, retaining only the compact hash/count/movement result in model context. No policy override was used.

The release owner may now review the new positional hash against this proof. This report does not itself update `EXPECTED_SHA256` or declare any other gate passed.
