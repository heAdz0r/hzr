#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_OUTPUT_ROOT="${1:-${HZR_REPOSITORY_ROOT}/dist}"
HZR_ENGINE_OUTPUT="${HZR_OUTPUT_ROOT}/engines"
HZR_BINARY_OUTPUT="${HZR_OUTPUT_ROOT}/bin"
HZR_LICENSE_OUTPUT="${HZR_OUTPUT_ROOT}/licenses"
HZR_CAVEMAN_OUTPUT="${HZR_ENGINE_OUTPUT}/caveman-code"
HZR_PROVENANCE_OUTPUT="${HZR_OUTPUT_ROOT}/share/hzr"
HZR_BUILD_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-build.XXXXXX")"

cleanup_hzr_build() {
  if [[ -n "${HZR_BUILD_TEMP:-}" && -d "${HZR_BUILD_TEMP}" ]]; then
    rm -rf -- "${HZR_BUILD_TEMP}"
  fi
}
trap cleanup_hzr_build EXIT

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required build command: $1" >&2
    exit 1
  fi
}

clone_at_commit() {
  local repository="$1"
  local commit="$2"
  local destination="$3"

  git clone --quiet --filter=blob:none --no-checkout "${repository}" "${destination}"
  git -C "${destination}" checkout --quiet --detach "${commit}"
  local actual
  actual="$(git -C "${destination}" rev-parse HEAD)"
  if [[ "${actual}" != "${commit}" ]]; then
    echo "source pin mismatch for ${repository}: expected ${commit}, got ${actual}" >&2
    exit 1
  fi
}

verify_sha256() {
  local expected="$1"
  local artifact="$2"
  local actual

  actual="$(shasum -a 256 "${artifact}" | awk '{print $1}')"
  if [[ "${actual}" != "${expected}" ]]; then
    echo "source artifact digest mismatch: ${artifact}" >&2
    echo "expected: ${expected}" >&2
    echo "actual:   ${actual}" >&2
    exit 1
  fi
}

require_command cargo
require_command git
require_command go
require_command awk
require_command grep
require_command install
require_command kill
require_command ln
require_command mv
require_command node
require_command npm
require_command readlink
require_command shasum
require_command sleep

"${HZR_REPOSITORY_ROOT}/scripts/verify-fork-core.sh"

mkdir -p \
  "${HZR_ENGINE_OUTPUT}" \
  "${HZR_BINARY_OUTPUT}" \
  "${HZR_LICENSE_OUTPUT}" \
  "${HZR_PROVENANCE_OUTPUT}/fork-core" \
  "${HZR_PROVENANCE_OUTPUT}/patches/grepai" \
  "${HZR_PROVENANCE_OUTPUT}/patches/icm"

verify_sha256 \
  "55535352bc9f4837198c652b8c44ec54a0a7ef82fbd81e11b4ec11f4c4082991" \
  "${HZR_REPOSITORY_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch"
clone_at_commit \
  "https://github.com/yoanbernabeu/grepai" \
  "65c345ca32122c17a39a5bbec2780c2eea773a12" \
  "${HZR_BUILD_TEMP}/grepai"
git -C "${HZR_BUILD_TEMP}/grepai" apply --check \
  "${HZR_REPOSITORY_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch"
git -C "${HZR_BUILD_TEMP}/grepai" apply \
  "${HZR_REPOSITORY_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch"
(
  cd "${HZR_BUILD_TEMP}/grepai"
  go test ./cli
  go build -trimpath -ldflags "-s -w -X main.version=0.35.0" \
    -o "${HZR_ENGINE_OUTPUT}/grepai" ./cmd/grepai
)
"${HZR_ENGINE_OUTPUT}/grepai" version | grep -F "0.35.0" >/dev/null
"${HZR_ENGINE_OUTPUT}/grepai" watch --help | grep -F -- "--no-worktree-discovery" >/dev/null

verify_sha256 \
  "cd38e20e32f352bfde93a4ce297799ef8b5f984f8af928409ef0f3e47102e586" \
  "${HZR_REPOSITORY_ROOT}/patches/icm/0.10.61-refresh-workspace-lock.patch"
clone_at_commit \
  "https://github.com/rtk-ai/icm" \
  "c3a1bac7cfe401b55fd66af16dfc0c774c02167a" \
  "${HZR_BUILD_TEMP}/icm"
git -C "${HZR_BUILD_TEMP}/icm" apply --check \
  "${HZR_REPOSITORY_ROOT}/patches/icm/0.10.61-refresh-workspace-lock.patch"
git -C "${HZR_BUILD_TEMP}/icm" apply \
  "${HZR_REPOSITORY_ROOT}/patches/icm/0.10.61-refresh-workspace-lock.patch"
cargo build \
  --manifest-path "${HZR_BUILD_TEMP}/icm/Cargo.toml" \
  --locked --release --package icm-cli \
  --no-default-features --features "embeddings-static,backend-sqlite"
install -m 0755 "${HZR_BUILD_TEMP}/icm/target/release/icm" "${HZR_ENGINE_OUTPUT}/icm"
"${HZR_ENGINE_OUTPUT}/icm" --version | grep -F "0.10.61" >/dev/null

HZR_FORK_TARGET="${HZR_BUILD_TEMP}/fork-core-target"
CARGO_TARGET_DIR="${HZR_FORK_TARGET}" cargo build \
  --manifest-path "${HZR_REPOSITORY_ROOT}/fork-core/rtk/Cargo.toml" \
  --locked --release
install -m 0755 "${HZR_FORK_TARGET}/release/rtk" "${HZR_ENGINE_OUTPUT}/rtk"
"${HZR_ENGINE_OUTPUT}/rtk" --version | grep -Fx "rtk 0.44.1-fork.1" >/dev/null

HZR_CAVEMAN_STAGE="${HZR_BUILD_TEMP}/caveman-code"
mkdir -p "${HZR_CAVEMAN_STAGE}"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/caveman-code/bridge.mjs" \
  "${HZR_CAVEMAN_STAGE}/bridge.mjs"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/caveman-code/package.json" \
  "${HZR_CAVEMAN_STAGE}/package.json"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/caveman-code/package-lock.json" \
  "${HZR_CAVEMAN_STAGE}/package-lock.json"
npm ci --omit=dev --prefix "${HZR_CAVEMAN_STAGE}"
npm audit --omit=dev --audit-level=high \
  --prefix "${HZR_CAVEMAN_STAGE}"
node --check "${HZR_CAVEMAN_STAGE}/bridge.mjs"
if [[ -e "${HZR_CAVEMAN_OUTPUT}" ]]; then
  echo "refusing to merge a managed Caveman runtime into existing output: ${HZR_CAVEMAN_OUTPUT}" >&2
  exit 1
fi
mv -- "${HZR_CAVEMAN_STAGE}" "${HZR_CAVEMAN_OUTPUT}"

cargo build \
  --manifest-path "${HZR_REPOSITORY_ROOT}/Cargo.toml" \
  --locked --release --workspace
install -m 0755 "${HZR_REPOSITORY_ROOT}/target/release/hzr" "${HZR_BINARY_OUTPUT}/hzr"
install -m 0755 "${HZR_REPOSITORY_ROOT}/target/release/hzrd" "${HZR_BINARY_OUTPUT}/hzrd"
ln -s hzr "${HZR_BINARY_OUTPUT}/rtk"
install -m 0644 "${HZR_REPOSITORY_ROOT}/LICENSE" "${HZR_LICENSE_OUTPUT}/HZR-Apache-2.0.txt"
install -m 0644 "${HZR_REPOSITORY_ROOT}/THIRD_PARTY_NOTICES.md" \
  "${HZR_LICENSE_OUTPUT}/THIRD_PARTY_NOTICES.md"
install -m 0644 "${HZR_REPOSITORY_ROOT}/fork-core/rtk/LICENSE" \
  "${HZR_LICENSE_OUTPUT}/rtk-fork-core-MIT.txt"
install -m 0644 "${HZR_BUILD_TEMP}/grepai/LICENSE" \
  "${HZR_LICENSE_OUTPUT}/grepai-MIT.txt"
install -m 0644 "${HZR_BUILD_TEMP}/icm/LICENSE" \
  "${HZR_LICENSE_OUTPUT}/ICM-Apache-2.0.txt"
install -m 0644 "${HZR_REPOSITORY_ROOT}/licenses/caveman-code-MIT.txt" \
  "${HZR_LICENSE_OUTPUT}/caveman-code-MIT.txt"
install -m 0644 "${HZR_REPOSITORY_ROOT}/engines.lock.toml" \
  "${HZR_PROVENANCE_OUTPUT}/engines.lock.toml"
for HZR_FORK_PROVENANCE in \
  SNAPSHOT.toml \
  SNAPSHOT_V2.tsv \
  SHA256SUMS \
  FORK_FILES \
  TRACKED_DELETIONS \
  SOURCE_STATUS \
  TRACKED_CHANGES.patch; do
  install -m 0644 \
    "${HZR_REPOSITORY_ROOT}/fork-core/${HZR_FORK_PROVENANCE}" \
    "${HZR_PROVENANCE_OUTPUT}/fork-core/${HZR_FORK_PROVENANCE}"
done
install -m 0644 \
  "${HZR_REPOSITORY_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch" \
  "${HZR_PROVENANCE_OUTPUT}/patches/grepai/0.35.0-disable-worktree-discovery.patch"
install -m 0644 \
  "${HZR_REPOSITORY_ROOT}/patches/icm/0.10.61-refresh-workspace-lock.patch" \
  "${HZR_PROVENANCE_OUTPUT}/patches/icm/0.10.61-refresh-workspace-lock.patch"

"${HZR_REPOSITORY_ROOT}/scripts/smoke-bundle.sh" "${HZR_OUTPUT_ROOT}"

echo "HZR bundle built at ${HZR_OUTPUT_ROOT}"
