#!/usr/bin/env bash
# Delivered-output benchmark after the external evaluation's method: raw vs old fork vs new fork vs upstream.
# Usage: HZR_BENCH_ROOT=… HZR_OLD_RTK=… HZR_NEW_RTK=… HZR_UPSTREAM_RTK=… bench.sh <out-dir>
set -u
S="${HZR_BENCH_ROOT:?set HZR_BENCH_ROOT to a scratch directory}"
OUT="$1"
OLD="${HZR_OLD_RTK:?path to the previous fork-core rtk binary}"
NEW="${HZR_NEW_RTK:?path to the current fork-core rtk binary}"
UP="${HZR_UPSTREAM_RTK:?path to an upstream rtk v0.50.0 binary}"
REPO="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
RED="$S/redmod"
export GOTOOLCHAIN=go1.25.1
export RTK_TELEMETRY_DISABLED=1 RTK_TRACKING_DISABLED=1
mkdir -p "$OUT"

# Red Go module: one compile failure, one failing test, one passing package.
mkdir -p "$RED/a" "$RED/b" "$RED/c"
printf 'module redmod\n\ngo 1.21\n' > "$RED/go.mod"
printf 'package a\n\nimport "testing"\n\nfunc TestOK(t *testing.T) {}\n\nfunc TestFail(t *testing.T) {\n\tgot := 2\n\tif got != 1 {\n\t\tt.Errorf("expected 1, got %%d", got)\n\t}\n}\n' > "$RED/a/a_test.go"
printf 'package b\n\nimport "testing"\n\nfunc TestB1(t *testing.T) {}\nfunc TestB2(t *testing.T) {}\n' > "$RED/b/b_test.go"
printf 'package c\n\nfunc Broken() int {\n\treturn "not an int"\n}\n' > "$RED/c/c.go"

CASES=(
  "go_test_red|$RED|go test ./... -count=1|RTK go test ./... -count=1"
  "go_vet_red|$RED|go vet ./...|RTK go vet ./..."
  "go_build_red|$RED|go build ./...|RTK go build ./..."
  "golangci_red|$RED|golangci-lint run ./...|RTK golangci-lint run ./..."
  "git_status|$REPO|git status|RTK git status"
  "git_log_30|$REPO|git log -30|RTK git log -30"
  "git_diff_head1|$REPO|git diff HEAD~1|RTK git diff HEAD~1"
  "git_diff_head5|$REPO|git diff HEAD~5|RTK git diff HEAD~5"
  "git_show_head|$REPO|git show HEAD~1|RTK git show HEAD~1"
  "find_rs|$REPO|find crates -name '*.rs' -type f|RTK find crates -name '*.rs' -type f"
  "ls_la|$REPO|ls -la crates/hzr-cli/src|RTK ls -la crates/hzr-cli/src"
  "wc_l|$REPO|wc -l crates/hzr-exec/src/*.rs|RTK wc -l crates/hzr-exec/src/*.rs"
  "grep_rn|$REPO|grep -rn never_worse fork-core/rtk/src|RTK grep -rn never_worse fork-core/rtk/src"
  "cat_n_code|$REPO|cat -n crates/hzr-exec/src/model.rs|RTK read -n crates/hzr-exec/src/model.rs"
  "cat_md_small|$REPO|cat SECURITY.md|RTK read SECURITY.md"
  "head_50|$REPO|head -50 crates/hzr-exec/src/adapter.rs|RTK read crates/hzr-exec/src/adapter.rs --max-lines 50"
)

strip_nag() { grep -v -e '^\[rtk\] /!\\ No hook installed' -e 'rtk init -g' ; }
run_case() { ( cd "$2" && bash -c "$3" 2>&1 ) | strip_nag > "$4"; }

printf "%-15s %9s %9s %9s %9s\n" case raw old_fork new_fork upstream
for entry in "${CASES[@]}"; do
  IFS='|' read -r name cwd raw rtkcmd <<< "$entry"
  run_case raw "$cwd" "$raw" "$OUT/$name.raw"
  run_case old "$cwd" "${rtkcmd//RTK/$OLD}" "$OUT/$name.old"
  run_case new "$cwd" "${rtkcmd//RTK/$NEW}" "$OUT/$name.new"
  run_case up  "$cwd" "${rtkcmd//RTK/$UP}"  "$OUT/$name.up"
  printf "%-15s %9d %9d %9d %9d\n" "$name" "$(wc -c < "$OUT/$name.raw")" "$(wc -c < "$OUT/$name.old")" "$(wc -c < "$OUT/$name.new")" "$(wc -c < "$OUT/$name.up")"
done
