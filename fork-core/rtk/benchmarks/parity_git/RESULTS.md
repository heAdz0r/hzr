# Git Mutating Parity Benchmarks

## Repro
```bash
bash benchmarks/parity_git/bench_parity_git.sh
python3 benchmarks/parity_git/analyze_parity_git.py
python3 -m unittest discover -s benchmarks/parity_git/tests -p 'test_*.py'
```

## Environment
```text
Date: Tue Feb 17 11:04:21 UTC 2026
Commit: bd240dc00f4302e2d5e36990b0ada8783359898d
rtk_bin: /Users/andrew/Programming/rtk/target/release/rtk
OS: Darwin MacBook-Pro-Andy.local 25.2.0 Darwin Kernel Version 25.2.0: Tue Nov 18 21:09:56 PST 2025; root:xnu-12377.61.12~1/RELEASE_ARM64_T6041 arm64
```

## Threshold Gates
- [PASS] exit_code_match_rate = 100% (100.0%)
- [PASS] side_effect_match_rate = 100% (100.0%)
- [PASS] stderr_key_signal_match_rate >= 99% (100.0%)

## Aggregate
- rows: 11
- exit_match_rate: 100.0%
- side_effect_match_rate: 100.0%
- stderr_signal_match_rate: 100.0%

## Scenario Rows
| scenario | kind | native_exit | rtk_exit | exit_match | side_effect_match | stderr_signal_match |
|---|---|---:|---:|---:|---:|---:|
| add_missing_path_failure | failure | 128 | 128 | 1 | 1 | 1 |
| commit_nothing_to_commit_failure | failure | 1 | 1 | 1 | 1 | 1 |
| push_without_remote_failure | failure | 128 | 128 | 1 | 1 | 1 |
| pull_without_remote_failure | failure | 1 | 1 | 1 | 1 | 1 |
| branch_delete_missing_failure | failure | 1 | 1 | 1 | 1 | 1 |
| fetch_missing_remote_failure | failure | 128 | 128 | 1 | 1 | 1 |
| stash_drop_missing_failure | failure | 1 | 1 | 1 | 1 | 1 |
| worktree_remove_missing_failure | failure | 128 | 128 | 1 | 1 | 1 |
| add_success | success | 0 | 0 | 1 | 1 | 1 |
| commit_success | success | 0 | 0 | 1 | 1 | 1 |
| stash_push_success | success | 0 | 0 | 1 | 1 | 1 |
