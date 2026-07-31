#!/usr/bin/env bash
#
# Write-path benchmark runner for write_core (durable/fast + unchanged/changed).
#
# Usage:
#   bash benchmarks/write/bench_write.sh
#
# Output:
#   benchmarks/write/results_raw.csv
#   benchmarks/write/results_env.txt
#
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CSV_OUT="$SCRIPT_DIR/results_raw.csv"
ENV_OUT="$SCRIPT_DIR/results_env.txt"

RUNS="${RUNS:-5}"
WRITE_TOOL="${WRITE_TOOL:-$PROJECT_DIR/target/release/write_bench_tool}"

echo "Building write_bench_tool..."
(cd "$PROJECT_DIR" && cargo build --release --bin write_bench_tool)

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

{
  echo "Date: $(date -u)"
  echo "Commit: $(cd "$PROJECT_DIR" && git rev-parse HEAD)"
  echo "runs: $RUNS"
  echo "write_tool: $WRITE_TOOL"
  echo "write_tool_version: $("$WRITE_TOOL" --help >/dev/null 2>&1 && echo ok || echo unavailable)"
  echo "OS: $(uname -a)"
} > "$ENV_OUT"

echo "=== Environment ==="
cat "$ENV_OUT"
echo ""

echo "scenario,size_label,file_size,tool,mode,run,latency_us,bytes_written,fsync_count,rename_count,skipped_unchanged" > "$CSV_OUT"

make_payload_files() {
  local size="$1"
  local base="$2"
  local changed="$3"
  python3 - "$size" "$base" "$changed" <<'PY'
import sys
from pathlib import Path

size = int(sys.argv[1])
base_path = Path(sys.argv[2])
changed_path = Path(sys.argv[3])

data = bytearray((i * 31 + 7) % 251 for i in range(size))
base_path.write_bytes(data)

changed = bytearray(data)
if size > 0:
    changed[size // 2] = (changed[size // 2] + 1) % 251
changed_path.write_bytes(changed)
PY
}

declare -a SIZES=(
  "small:1024"
  "medium:131072"
  "large:8388608"
)

declare -a CASES=(
  "unchanged:write_core:durable"
  "unchanged:write_core:fast"
  "changed:write_core:durable"
  "changed:write_core:fast"
  "changed:native_safe:durable"
)

for size_entry in "${SIZES[@]}"; do
  size_label="${size_entry%%:*}"
  size_bytes="${size_entry##*:}"
  echo "Running size=${size_label} (${size_bytes} bytes)"

  size_dir="$WORK_DIR/$size_label"
  mkdir -p "$size_dir"
  base_file="$size_dir/base.bin"
  changed_file="$size_dir/changed.bin"
  target_file="$size_dir/target.bin"

  make_payload_files "$size_bytes" "$base_file" "$changed_file"

  for case_entry in "${CASES[@]}"; do
    scenario="$(echo "$case_entry" | cut -d: -f1)"
    tool="$(echo "$case_entry" | cut -d: -f2)"
    mode="$(echo "$case_entry" | cut -d: -f3)"

    for run in $(seq 1 "$RUNS"); do
      cp "$base_file" "$target_file"
      content_file="$base_file"
      if [ "$scenario" = "changed" ]; then
        content_file="$changed_file"
      fi

      if [ "$tool" = "write_core" ]; then
        extra_args=()
        if [ "$scenario" = "changed" ]; then
          extra_args+=(--assume-changed)
        fi
        result_line="$("$WRITE_TOOL" --path "$target_file" --content-file "$content_file" --mode "$mode" --implementation write_core "${extra_args[@]}")"
      else
        result_line="$("$WRITE_TOOL" --path "$target_file" --content-file "$content_file" --mode "$mode" --implementation native_safe --assume-changed)"
      fi

      IFS=',' read -r latency_us bytes_written fsync_count rename_count skipped_unchanged <<< "$result_line"

      echo "$scenario,$size_label,$size_bytes,$tool,$mode,$run,$latency_us,$bytes_written,$fsync_count,$rename_count,$skipped_unchanged" >> "$CSV_OUT"
    done
  done
done

echo ""
echo "Benchmark complete."
echo "Raw CSV: $CSV_OUT"
echo "Env:     $ENV_OUT"
echo "Next:    python3 benchmarks/write/analyze_write.py"
