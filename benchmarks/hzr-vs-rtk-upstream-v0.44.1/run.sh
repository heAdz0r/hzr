#!/usr/bin/env bash
set -euo pipefail

HZR_BENCHMARK_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
HZR_REPOSITORY_ROOT="$(cd -- "${HZR_BENCHMARK_DIR}/../.." && pwd -P)"
HZR_UPSTREAM_COMMIT="36591fb00d650bf987b57483c0b3a395a35a8dc1"
HZR_RUN_ID="${HZR_BENCHMARK_RUN_ID:-$(date -u +%Y-%m-%dT%H%M%SZ)}"
HZR_OUTPUT_ROOT="${HZR_BENCHMARK_OUTPUT:-${HZR_BENCHMARK_DIR}/runs/${HZR_RUN_ID}}"
HZR_REPETITIONS="${HZR_BENCHMARK_REPETITIONS:-5}"
HZR_TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/hzr-rtk-benchmark.XXXXXX")"

cleanup_hzr_benchmark() {
  rm -rf -- "${HZR_TEMP_ROOT}"
}
trap cleanup_hzr_benchmark EXIT

for HZR_REQUIRED_COMMAND in cargo git python3 rustc; do
  command -v "${HZR_REQUIRED_COMMAND}" >/dev/null 2>&1 || {
    echo "missing required command: ${HZR_REQUIRED_COMMAND}" >&2
    exit 1
  }
done

git clone --quiet --no-tags \
  https://github.com/rtk-ai/rtk.git \
  "${HZR_TEMP_ROOT}/upstream"
git -C "${HZR_TEMP_ROOT}/upstream" checkout --quiet "${HZR_UPSTREAM_COMMIT}"

CARGO_TARGET_DIR="${HZR_TEMP_ROOT}/upstream-target" \
  cargo build --quiet --locked --release --bin rtk \
  --manifest-path "${HZR_TEMP_ROOT}/upstream/Cargo.toml"
CARGO_TARGET_DIR="${HZR_TEMP_ROOT}/fork-target" \
  cargo build --quiet --locked --release --bin rtk \
  --manifest-path "${HZR_REPOSITORY_ROOT}/fork-core/rtk/Cargo.toml"
CARGO_TARGET_DIR="${HZR_TEMP_ROOT}/hzr-target" \
  cargo build --quiet --locked --release --bin hzr \
  --manifest-path "${HZR_REPOSITORY_ROOT}/Cargo.toml"

python3 "${HZR_BENCHMARK_DIR}/benchmark.py" \
  --fixture "${HZR_TEMP_ROOT}/upstream" \
  --upstream-binary "${HZR_TEMP_ROOT}/upstream-target/release/rtk" \
  --hzr-binary "${HZR_TEMP_ROOT}/hzr-target/release/hzr" \
  --hzr-engine "${HZR_TEMP_ROOT}/fork-target/release/rtk" \
  --repo-root "${HZR_REPOSITORY_ROOT}" \
  --output "${HZR_OUTPUT_ROOT}" \
  --work-root "${HZR_TEMP_ROOT}/work" \
  --repetitions "${HZR_REPETITIONS}"

echo "benchmark evidence: ${HZR_OUTPUT_ROOT}"
