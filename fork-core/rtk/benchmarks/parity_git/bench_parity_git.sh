#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CSV_OUT="$SCRIPT_DIR/results_raw.csv"
ENV_OUT="$SCRIPT_DIR/results_env.txt"

RTK_BIN="${RTK_BIN:-$PROJECT_DIR/target/release/rtk}"

echo "Building rtk (release)..."
(cd "$PROJECT_DIR" && cargo build --release --bin rtk)

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

{
  echo "Date: $(date -u)"
  echo "Commit: $(cd "$PROJECT_DIR" && git rev-parse HEAD)"
  echo "rtk_bin: $RTK_BIN"
  echo "OS: $(uname -a)"
} > "$ENV_OUT"

echo "scenario,kind,native_exit,rtk_exit,exit_match,side_effect_match,stderr_signal_match" > "$CSV_OUT"

seed_repo() {
  local repo="$1"
  mkdir -p "$repo"
  git -C "$repo" init -q
  git -C "$repo" config user.name "RTK Benchmark"
  git -C "$repo" config user.email "rtk-bench@example.com"
  printf 'seed\n' > "$repo/README.md"
  git -C "$repo" add README.md
  git -C "$repo" commit -m seed -q
}

repo_signature() {
  local repo="$1"
  local status cached stash_count tree subject
  status="$(git -C "$repo" status --porcelain=v1 --branch)"
  cached="$(git -C "$repo" diff --cached --name-status)"
  stash_count="$(git -C "$repo" stash list | wc -l | tr -d ' ')"
  tree="$(git -C "$repo" rev-parse HEAD^{tree} 2>/dev/null || echo none)"
  subject="$(git -C "$repo" log -1 --pretty=%s 2>/dev/null || echo none)"
  printf 'status=%s|cached=%s|stash=%s|tree=%s|subject=%s' "$status" "$cached" "$stash_count" "$tree" "$subject"
}

extract_signal() {
  local file="$1"
  awk 'BEGIN{IGNORECASE=1} /^[[:space:]]*(fatal:|error:)/ {print tolower($0); exit}' "$file"
}

contains_signal() {
  local signal="$1" text_file="$2"
  if [ -z "$signal" ]; then
    echo 1
    return
  fi
  if tr '[:upper:]' '[:lower:]' < "$text_file" | grep -F -- "$signal" >/dev/null 2>&1; then
    echo 1
  else
    echo 0
  fi
}

run_case() {
  local scenario="$1"
  local kind="$2"
  local native_repo="$3"
  local rtk_repo="$4"
  shift 4

  local native_args=()
  local rtk_args=()
  local split_seen=0
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "--" ] && [ "$split_seen" -eq 0 ]; then
      split_seen=1
      shift
      continue
    fi
    if [ "$split_seen" -eq 0 ]; then
      native_args+=("$1")
    else
      rtk_args+=("$1")
    fi
    shift
  done

  local nout nerr rout rerr rcomb
  nout="$(mktemp)"; nerr="$(mktemp)"; rout="$(mktemp)"; rerr="$(mktemp)"; rcomb="$(mktemp)"

  set +e
  (cd "$native_repo" && git "${native_args[@]}") >"$nout" 2>"$nerr"
  local native_exit=$?
  (cd "$rtk_repo" && "$RTK_BIN" git "${rtk_args[@]}") >"$rout" 2>"$rerr"
  local rtk_exit=$?
  set -e

  local exit_match=0
  if [ "$native_exit" -eq "$rtk_exit" ]; then
    exit_match=1
  fi

  local native_sig
  native_sig="$(extract_signal "$nerr")"
  cat "$rerr" "$rout" > "$rcomb"
  local stderr_match
  stderr_match="$(contains_signal "$native_sig" "$rcomb")"

  local side_effect_match=0
  local nstate rstate
  nstate="$(repo_signature "$native_repo")"
  rstate="$(repo_signature "$rtk_repo")"
  if [ "$nstate" = "$rstate" ]; then
    side_effect_match=1
  fi

  echo "$scenario,$kind,$native_exit,$rtk_exit,$exit_match,$side_effect_match,$stderr_match" >> "$CSV_OUT"

  rm -f "$nout" "$nerr" "$rout" "$rerr" "$rcomb"
}

run_failure_case() {
  local scenario="$1"
  local kind="$2"
  local nrepo="$WORK_DIR/${scenario}_native"
  local rrepo="$WORK_DIR/${scenario}_rtk"
  seed_repo "$nrepo"
  seed_repo "$rrepo"
  shift 2
  run_case "$scenario" "$kind" "$nrepo" "$rrepo" "$@"
}

run_add_success() {
  local scenario="add_success"
  local nrepo="$WORK_DIR/${scenario}_native"
  local rrepo="$WORK_DIR/${scenario}_rtk"
  seed_repo "$nrepo"
  seed_repo "$rrepo"
  printf 'x\n' > "$nrepo/new.txt"
  printf 'x\n' > "$rrepo/new.txt"
  run_case "$scenario" "success" "$nrepo" "$rrepo" add new.txt -- add new.txt
}

run_commit_success() {
  local scenario="commit_success"
  local nrepo="$WORK_DIR/${scenario}_native"
  local rrepo="$WORK_DIR/${scenario}_rtk"
  seed_repo "$nrepo"
  seed_repo "$rrepo"
  printf 'feat\n' > "$nrepo/feat.txt"
  printf 'feat\n' > "$rrepo/feat.txt"
  git -C "$nrepo" add feat.txt
  git -C "$rrepo" add feat.txt
  run_case "$scenario" "success" "$nrepo" "$rrepo" commit -m feat -q -- commit -m feat
}

run_stash_push_success() {
  local scenario="stash_push_success"
  local nrepo="$WORK_DIR/${scenario}_native"
  local rrepo="$WORK_DIR/${scenario}_rtk"
  seed_repo "$nrepo"
  seed_repo "$rrepo"
  printf 'changed\n' > "$nrepo/README.md"
  printf 'changed\n' > "$rrepo/README.md"
  run_case "$scenario" "success" "$nrepo" "$rrepo" stash push -m tmp -- stash push -m tmp
}

run_failure_case "add_missing_path_failure" "failure" add __missing__.txt -- add __missing__.txt
run_failure_case "commit_nothing_to_commit_failure" "failure" commit -m noop -- commit -m noop
run_failure_case "push_without_remote_failure" "failure" push -- push
run_failure_case "pull_without_remote_failure" "failure" pull -- pull
run_failure_case "branch_delete_missing_failure" "failure" branch -d __missing_branch__ -- branch -d __missing_branch__
run_failure_case "fetch_missing_remote_failure" "failure" fetch __missing_remote__ -- fetch __missing_remote__
run_failure_case "stash_drop_missing_failure" "failure" stash drop 'stash@{0}' -- stash drop 'stash@{0}'

# worktree remove missing path requires absolute path prepared per repo
nrepo="$WORK_DIR/worktree_remove_missing_native"
rrepo="$WORK_DIR/worktree_remove_missing_rtk"
seed_repo "$nrepo"
seed_repo "$rrepo"
missing_native="$nrepo/__missing_worktree__"
run_case "worktree_remove_missing_failure" "failure" "$nrepo" "$rrepo" worktree remove "$missing_native" -- worktree remove "$missing_native"

run_add_success
run_commit_success
run_stash_push_success

echo "Benchmark complete."
echo "CSV: $CSV_OUT"
echo "ENV: $ENV_OUT"
echo "Next: python3 benchmarks/parity_git/analyze_parity_git.py"
