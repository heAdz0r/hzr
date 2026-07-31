#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_BUNDLE_ROOT="${1:-}"
HZR_OUTPUT_ROOT="${2:-${HZR_REPOSITORY_ROOT}/dist-release}"
if [[ -z "${HZR_BUNDLE_ROOT}" || ! -d "${HZR_BUNDLE_ROOT}" ]]; then
  echo "usage: scripts/package-release.sh /absolute/path/to/hzr-bundle [output-directory]" >&2
  exit 2
fi
HZR_BUNDLE_ROOT="$(cd -- "${HZR_BUNDLE_ROOT}" && pwd -P)"
mkdir -p "${HZR_OUTPUT_ROOT}"
HZR_OUTPUT_ROOT="$(cd -- "${HZR_OUTPUT_ROOT}" && pwd -P)"

HZR_VERSION="$(${HZR_BUNDLE_ROOT}/bin/hzr --version | awk '{print $2}')"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) HZR_PLATFORM="darwin-arm64" ;;
  Darwin-x86_64) HZR_PLATFORM="darwin-x64" ;;
  Linux-aarch64 | Linux-arm64) HZR_PLATFORM="linux-arm64" ;;
  Linux-x86_64) HZR_PLATFORM="linux-x64" ;;
  *)
    echo "unsupported release platform: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

HZR_PACKAGE_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-package.XXXXXX")"
cleanup_hzr_package() {
  if [[ -n "${HZR_PACKAGE_TEMP:-}" && -d "${HZR_PACKAGE_TEMP}" ]]; then
    rm -rf -- "${HZR_PACKAGE_TEMP}"
  fi
}
trap cleanup_hzr_package EXIT

cp -R "${HZR_BUNDLE_ROOT}" "${HZR_PACKAGE_TEMP}/hzr"
HZR_MANIFEST="${HZR_PACKAGE_TEMP}/hzr/share/hzr/BUNDLE_MANIFEST.sha256"
(
  cd "${HZR_PACKAGE_TEMP}/hzr"
  find . -type f ! -path './share/hzr/BUNDLE_MANIFEST.sha256' -print \
    | LC_ALL=C sort \
    | while IFS= read -r HZR_FILE; do
        shasum -a 256 "${HZR_FILE}"
      done
) >"${HZR_MANIFEST}"

HZR_ARTIFACT="hzr-v${HZR_VERSION}-${HZR_PLATFORM}.tar.gz"
tar -czf "${HZR_OUTPUT_ROOT}/${HZR_ARTIFACT}" -C "${HZR_PACKAGE_TEMP}" hzr
HZR_ARTIFACT_SHA256="$(shasum -a 256 "${HZR_OUTPUT_ROOT}/${HZR_ARTIFACT}" | awk '{print $1}')"
printf '%s  %s\n' "${HZR_ARTIFACT_SHA256}" "${HZR_ARTIFACT}" \
  >"${HZR_OUTPUT_ROOT}/SHA256SUMS"

echo "HZR release artifact: ${HZR_OUTPUT_ROOT}/${HZR_ARTIFACT}"
