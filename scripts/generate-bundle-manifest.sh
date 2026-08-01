#!/usr/bin/env bash
set -euo pipefail

HZR_BUNDLE_ROOT="${1:-}"
if [[ -z "${HZR_BUNDLE_ROOT}" || ! -d "${HZR_BUNDLE_ROOT}" ]]; then
  echo "usage: scripts/generate-bundle-manifest.sh /absolute/path/to/hzr-bundle" >&2
  exit 2
fi
HZR_BUNDLE_ROOT="$(cd -- "${HZR_BUNDLE_ROOT}" && pwd -P)"
HZR_MANIFEST="${HZR_BUNDLE_ROOT}/share/hzr/BUNDLE_MANIFEST.sha256"
mkdir -p "$(dirname -- "${HZR_MANIFEST}")"
HZR_MANIFEST_TEMP="$(mktemp "${TMPDIR:-/tmp}/hzr-bundle-manifest.XXXXXX")"

cleanup_hzr_manifest() {
  if [[ -n "${HZR_MANIFEST_TEMP:-}" && -f "${HZR_MANIFEST_TEMP}" ]]; then
    rm -f -- "${HZR_MANIFEST_TEMP}"
  fi
}
trap cleanup_hzr_manifest EXIT

(
  cd "${HZR_BUNDLE_ROOT}"
  find . -type f ! -path './share/hzr/BUNDLE_MANIFEST.sha256' \
    -exec shasum -a 256 {} + \
    | LC_ALL=C sort
) >"${HZR_MANIFEST_TEMP}"
mv -- "${HZR_MANIFEST_TEMP}" "${HZR_MANIFEST}"
HZR_MANIFEST_TEMP=""

(
  cd "${HZR_BUNDLE_ROOT}"
  shasum -a 256 -c share/hzr/BUNDLE_MANIFEST.sha256 >/dev/null
)
