#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_OUTPUT_ROOT="${1:-${HZR_REPOSITORY_ROOT}/dist}"
HZR_ENGINE_OUTPUT="${HZR_OUTPUT_ROOT}/engines"
HZR_BINARY_OUTPUT="${HZR_OUTPUT_ROOT}/bin"
HZR_LICENSE_OUTPUT="${HZR_OUTPUT_ROOT}/licenses"
HZR_RUNTIME_OUTPUT="${HZR_OUTPUT_ROOT}/runtime"
HZR_CAVEMAN_OUTPUT="${HZR_ENGINE_OUTPUT}/caveman-code"
HZR_PROVENANCE_OUTPUT="${HZR_OUTPUT_ROOT}/share/hzr"
HZR_VISUALIZER_OUTPUT="${HZR_PROVENANCE_OUTPUT}/visualizer"
HZR_BUILD_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-build.XXXXXX")"
HZR_BUILD_STAGE=0
HZR_BUILD_STAGE_TOTAL=9

hzr_build_stage() {
  HZR_BUILD_STAGE=$((HZR_BUILD_STAGE + 1))
  printf '[%s/%s] %s\n' "${HZR_BUILD_STAGE}" "${HZR_BUILD_STAGE_TOTAL}" "$1"
}

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

  git clone --progress --filter=blob:none --no-checkout "${repository}" "${destination}"
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

  actual="$(sha256_file "${artifact}")"
  if [[ "${actual}" != "${expected}" ]]; then
    echo "source artifact digest mismatch: ${artifact}" >&2
    echo "expected: ${expected}" >&2
    echo "actual:   ${actual}" >&2
    exit 1
  fi
}

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

download_cached() {
  local url="$1"
  local expected="$2"
  local name="$3"
  local destination="$4"
  local cache_root="${HZR_DOWNLOAD_CACHE:-${HZR_REPOSITORY_ROOT}/target/hzr-download-cache}"
  local cached="${cache_root}/${name}"

  mkdir -p "${cache_root}"
  if [[ -f "${cached}" && "$(sha256_file "${cached}")" == "${expected}" ]]; then
    cp -- "${cached}" "${destination}"
    return
  fi

  curl --fail --progress-bar --show-error --location --proto '=https' --tlsv1.2 \
    --connect-timeout 20 --max-time 1800 --retry 3 --retry-all-errors \
    "${url}" --output "${destination}"
  verify_sha256 "${expected}" "${destination}"

  local cache_stage
  cache_stage="$(mktemp "${cache_root}/.${name}.XXXXXX")"
  install -m 0644 "${destination}" "${cache_stage}"
  mv -f -- "${cache_stage}" "${cached}"
}

require_command cargo
require_command curl
require_command git
require_command go
require_command awk
require_command bun
require_command cp
require_command grep
require_command install
require_command kill
require_command ln
require_command mv
require_command readlink
require_command shasum
require_command sleep
require_command tar

HZR_BUN_VERSION="1.2.19"
if [[ "$(bun --version)" != "${HZR_BUN_VERSION}" ]]; then
  echo "unsupported Bun build runtime: expected ${HZR_BUN_VERSION}, got $(bun --version)" >&2
  exit 1
fi

hzr_build_stage "Verifying fork-core provenance and parity"
"${HZR_REPOSITORY_ROOT}/scripts/verify-fork-core.sh"

mkdir -p \
  "${HZR_ENGINE_OUTPUT}" \
  "${HZR_BINARY_OUTPUT}" \
  "${HZR_LICENSE_OUTPUT}" \
  "${HZR_RUNTIME_OUTPUT}" \
  "${HZR_PROVENANCE_OUTPUT}/fork-core" \
  "${HZR_PROVENANCE_OUTPUT}/skills/hzr-tdd/references" \
  "${HZR_PROVENANCE_OUTPUT}/patches/grepai" \
  "${HZR_PROVENANCE_OUTPUT}/patches/icm"

HZR_NODE_VERSION="22.17.1"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    HZR_NODE_PLATFORM="darwin-arm64"
    HZR_NODE_SHA256="a983f4f2a7b71512b78d7935b9ccf6b72120a255810070afd635c4146bca7b31"
    ;;
  Darwin-x86_64)
    HZR_NODE_PLATFORM="darwin-x64"
    HZR_NODE_SHA256="b925103150fac0d23a44a45b2d88a01b73e5fff101e5dcfbae98d32c08d4bee3"
    ;;
  Linux-aarch64 | Linux-arm64)
    HZR_NODE_PLATFORM="linux-arm64"
    HZR_NODE_SHA256="f53510706998cf044f634190416f0588e7e1937aecea938768952e0f0ac1f41b"
    ;;
  Linux-x86_64)
    HZR_NODE_PLATFORM="linux-x64"
    # SHA-256 of node-v22.17.1-linux-x64.tar.gz. The previous value was the .tar.xz
    # digest while HZR_NODE_ARCHIVE downloads .tar.gz, so every linux-x64 bundle build
    # failed verification. Keep these paired with the .tar.gz suffix below.
    HZR_NODE_SHA256="cfb6ac0cf339825fe36efd1f18a79016b02aca19fbfa6c9547c57e27dc09f6ea"
    ;;
  *)
    echo "unsupported Node.js bundle platform: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac
HZR_NODE_ARCHIVE="node-v${HZR_NODE_VERSION}-${HZR_NODE_PLATFORM}.tar.gz"
hzr_build_stage "Downloading and staging the pinned Node.js runtime"
HZR_NODE_DOWNLOAD="${HZR_BUILD_TEMP}/${HZR_NODE_ARCHIVE}"
download_cached \
  "https://nodejs.org/download/release/v${HZR_NODE_VERSION}/${HZR_NODE_ARCHIVE}" \
  "${HZR_NODE_SHA256}" \
  "${HZR_NODE_ARCHIVE}" \
  "${HZR_NODE_DOWNLOAD}"
verify_sha256 "${HZR_NODE_SHA256}" "${HZR_NODE_DOWNLOAD}"
HZR_NODE_ROOT="${HZR_RUNTIME_OUTPUT}/node"
mkdir -p "${HZR_NODE_ROOT}"
tar -xzf "${HZR_NODE_DOWNLOAD}" -C "${HZR_NODE_ROOT}" --strip-components=1
HZR_NODE_BINARY="${HZR_NODE_ROOT}/bin/node"
HZR_NPM_BINARY="${HZR_NODE_ROOT}/bin/npm"
"${HZR_NODE_BINARY}" --version | grep -Fx "v${HZR_NODE_VERSION}" >/dev/null
ln -s ../runtime/node/bin/node "${HZR_ENGINE_OUTPUT}/node"

verify_sha256 \
  "55535352bc9f4837198c652b8c44ec54a0a7ef82fbd81e11b4ec11f4c4082991" \
  "${HZR_REPOSITORY_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch"
hzr_build_stage "Building the pinned grepai engine"
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
hzr_build_stage "Building the pinned ICM engine"
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
hzr_build_stage "Building the managed RTK fork-core"
CARGO_TARGET_DIR="${HZR_FORK_TARGET}" cargo build \
  --manifest-path "${HZR_REPOSITORY_ROOT}/fork-core/rtk/Cargo.toml" \
  --locked --release
install -m 0755 "${HZR_FORK_TARGET}/release/rtk" "${HZR_ENGINE_OUTPUT}/rtk"
"${HZR_ENGINE_OUTPUT}/rtk" --version | grep -Fx "rtk 0.44.1-fork.1" >/dev/null

HZR_CAVEMAN_STAGE="${HZR_BUILD_TEMP}/caveman-code"
hzr_build_stage "Installing the pinned Caveman runtime"
mkdir -p "${HZR_CAVEMAN_STAGE}"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/caveman-code/bridge.mjs" \
  "${HZR_CAVEMAN_STAGE}/bridge.mjs"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/caveman-code/package.json" \
  "${HZR_CAVEMAN_STAGE}/package.json"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/caveman-code/package-lock.json" \
  "${HZR_CAVEMAN_STAGE}/package-lock.json"
PATH="${HZR_NODE_ROOT}/bin:${PATH}" "${HZR_NPM_BINARY}" ci --omit=dev \
  --prefix "${HZR_CAVEMAN_STAGE}"
PATH="${HZR_NODE_ROOT}/bin:${PATH}" "${HZR_NPM_BINARY}" audit \
  --omit=dev --audit-level=high \
  --prefix "${HZR_CAVEMAN_STAGE}"
"${HZR_NODE_BINARY}" --check "${HZR_CAVEMAN_STAGE}/bridge.mjs"
if [[ -e "${HZR_CAVEMAN_OUTPUT}" ]]; then
  echo "refusing to merge a managed Caveman runtime into existing output: ${HZR_CAVEMAN_OUTPUT}" >&2
  exit 1
fi
mv -- "${HZR_CAVEMAN_STAGE}" "${HZR_CAVEMAN_OUTPUT}"

HZR_VISUALIZER_STAGE="${HZR_BUILD_TEMP}/visualizer"
hzr_build_stage "Testing and building the visualizer"
mkdir -p "${HZR_VISUALIZER_STAGE}"
for artifact in bun.lock index.html package.json tsconfig.json vite.config.ts; do
  cp -- \
    "${HZR_REPOSITORY_ROOT}/visualizer/${artifact}" \
    "${HZR_VISUALIZER_STAGE}/${artifact}"
done
cp -R -- "${HZR_REPOSITORY_ROOT}/visualizer/src" "${HZR_VISUALIZER_STAGE}/src"
cp -R -- "${HZR_REPOSITORY_ROOT}/visualizer/public" "${HZR_VISUALIZER_STAGE}/public"
(
  cd "${HZR_VISUALIZER_STAGE}"
  bun install --frozen-lockfile
  bun test
  bun run build
)
mkdir -p "${HZR_VISUALIZER_OUTPUT}"
cp -R -- "${HZR_VISUALIZER_STAGE}/dist/." "${HZR_VISUALIZER_OUTPUT}/"

hzr_build_stage "Building HZR binaries and assembling provenance"
cargo build \
  --manifest-path "${HZR_REPOSITORY_ROOT}/Cargo.toml" \
  --locked --release --workspace
install -m 0755 "${HZR_REPOSITORY_ROOT}/target/release/hzr" "${HZR_BINARY_OUTPUT}/hzr"
install -m 0755 "${HZR_REPOSITORY_ROOT}/target/release/hzrd" "${HZR_BINARY_OUTPUT}/hzrd"
ln -s hzr "${HZR_BINARY_OUTPUT}/rtk"
install -m 0644 "${HZR_REPOSITORY_ROOT}/LICENSE" "${HZR_LICENSE_OUTPUT}/HZR-Apache-2.0.txt"
install -m 0644 "${HZR_REPOSITORY_ROOT}/NOTICE" "${HZR_LICENSE_OUTPUT}/NOTICE"
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
install -m 0644 "${HZR_NODE_ROOT}/LICENSE" \
  "${HZR_LICENSE_OUTPUT}/Node.js-MIT-and-dependencies.txt"
install -m 0644 "${HZR_REPOSITORY_ROOT}/engines.lock.toml" \
  "${HZR_PROVENANCE_OUTPUT}/engines.lock.toml"
for HZR_FORK_PROVENANCE in \
  SNAPSHOT.toml \
  SNAPSHOT_V2.tsv \
  SHA256SUMS \
  FORK_FILES \
  CURRENT_ENGINE.toml \
  CURRENT_ENGINE_V1.tsv \
  CURRENT_SHA256SUMS \
  CURRENT_FILES \
  TRACKED_DELETIONS \
  SOURCE_STATUS \
  TRACKED_CHANGES.patch; do
  install -m 0644 \
    "${HZR_REPOSITORY_ROOT}/fork-core/${HZR_FORK_PROVENANCE}" \
    "${HZR_PROVENANCE_OUTPUT}/fork-core/${HZR_FORK_PROVENANCE}"
done
mkdir -p "${HZR_PROVENANCE_OUTPUT}/integrations/claude-code"
install -m 0644 "${HZR_REPOSITORY_ROOT}/HZR.md" \
  "${HZR_PROVENANCE_OUTPUT}/HZR.md"
install -m 0644 "${HZR_REPOSITORY_ROOT}/install.sh" \
  "${HZR_PROVENANCE_OUTPUT}/install.sh"
install -m 0644 "${HZR_REPOSITORY_ROOT}/.claude/skills/hzr-tdd/SKILL.md" \
  "${HZR_PROVENANCE_OUTPUT}/skills/hzr-tdd/SKILL.md"
install -m 0644 \
  "${HZR_REPOSITORY_ROOT}/.claude/skills/hzr-tdd/references/testing-patterns.md" \
  "${HZR_PROVENANCE_OUTPUT}/skills/hzr-tdd/references/testing-patterns.md"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/claude-code/hzr-awareness.md" \
  "${HZR_PROVENANCE_OUTPUT}/integrations/claude-code/hzr-awareness.md"
install -m 0644 "${HZR_REPOSITORY_ROOT}/integrations/claude-code/hzr-awareness-codex.md" \
  "${HZR_PROVENANCE_OUTPUT}/integrations/claude-code/hzr-awareness-codex.md"
install -m 0644 \
  "${HZR_REPOSITORY_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch" \
  "${HZR_PROVENANCE_OUTPUT}/patches/grepai/0.35.0-disable-worktree-discovery.patch"
install -m 0644 \
  "${HZR_REPOSITORY_ROOT}/patches/icm/0.10.61-refresh-workspace-lock.patch" \
  "${HZR_PROVENANCE_OUTPUT}/patches/icm/0.10.61-refresh-workspace-lock.patch"

hzr_build_stage "Generating the manifest and smoke-testing the bundle"
"${HZR_REPOSITORY_ROOT}/scripts/generate-bundle-manifest.sh" "${HZR_OUTPUT_ROOT}"
"${HZR_REPOSITORY_ROOT}/scripts/smoke-bundle.sh" "${HZR_OUTPUT_ROOT}"

echo "HZR bundle built at ${HZR_OUTPUT_ROOT}"
