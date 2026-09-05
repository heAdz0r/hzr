#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"

usage() {
  echo "usage: scripts/complete-gate.sh [--source | --bundle /absolute/path]" >&2
  exit 2
}

HZR_GATE_MODE="${1:---source}"
case "${HZR_GATE_MODE}" in
  --source)
    if [[ "$#" -gt 1 ]]; then
      usage
    fi
    cd "${HZR_REPOSITORY_ROOT}"
    scripts/target-hygiene.sh # 0.8.2: bound target/debug before the locked builds add to it
    python3 scripts/verify-release-gates.py --self-test
    bash -n scripts/*.sh
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo test --locked --workspace --all-targets --all-features
    scripts/verify-fork-core.sh --test
    ;;
  --bundle)
    if [[ "$#" -ne 2 || "$2" != /* ]]; then
      usage
    fi
    "${HZR_REPOSITORY_ROOT}/scripts/build-bundle.sh" "$2"
    ;;
  *)
    usage
    ;;
esac
