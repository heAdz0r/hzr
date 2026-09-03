#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_ENGINE_METADATA_ROOT="${HZR_REPOSITORY_ROOT}/fork-core"
HZR_ENGINE_ROOT="${HZR_ENGINE_METADATA_ROOT}/rtk"

# shellcheck source=scripts/fork-snapshot-v2.sh
source "${HZR_REPOSITORY_ROOT}/scripts/fork-snapshot-v2.sh"

hzr_baseline_string() {
  local HZR_FIELD="$1"
  sed -n "s/^${HZR_FIELD} = \"\([^\"]*\)\"$/\\1/p" \
    "${HZR_ENGINE_METADATA_ROOT}/SNAPSHOT.toml"
}

if [[ ! -f "${HZR_ENGINE_METADATA_ROOT}/CURRENT_FILES" ]]; then
  cp -p -- \
    "${HZR_ENGINE_METADATA_ROOT}/FORK_FILES" \
    "${HZR_ENGINE_METADATA_ROOT}/CURRENT_FILES"
fi

HZR_TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/hzr-current-engine.XXXXXX")"
cleanup_hzr_current_engine() {
  rm -rf -- "${HZR_TEMP_ROOT}"
}
trap cleanup_hzr_current_engine EXIT

while IFS= read -r HZR_RELATIVE_PATH; do
  hzr_snapshot_validate_relative_path "${HZR_RELATIVE_PATH}"
  HZR_ENTRY_PATH="${HZR_ENGINE_ROOT}/${HZR_RELATIVE_PATH}"
  if [[ -L "${HZR_ENTRY_PATH}" ]]; then
    HZR_SHA256="$(hzr_snapshot_symlink_bytes "${HZR_ENTRY_PATH}" | shasum -a 256 | awk '{print $1}')"
  elif [[ -f "${HZR_ENTRY_PATH}" ]]; then
    HZR_SHA256="$(hzr_snapshot_sha256_file "${HZR_ENTRY_PATH}")"
  else
    echo "current engine entry is missing: ${HZR_RELATIVE_PATH}" >&2
    exit 1
  fi
  printf '%s  rtk/%s\n' "${HZR_SHA256}" "${HZR_RELATIVE_PATH}"
done <"${HZR_ENGINE_METADATA_ROOT}/CURRENT_FILES" \
  >"${HZR_TEMP_ROOT}/CURRENT_SHA256SUMS"

HZR_SOURCE_REPOSITORY="$(hzr_baseline_string source_repository)"
HZR_SOURCE_BRANCH="$(hzr_baseline_string source_branch)"
HZR_SOURCE_HEAD="$(hzr_baseline_string source_head)"
HZR_SOURCE_DIFF_SHA256="$(hzr_baseline_string source_diff_sha256)"
HZR_SOURCE_STATUS_SHA256="$(hzr_baseline_string source_status_sha256)"
HZR_CONTENT_MANIFEST_SHA256="$(hzr_snapshot_sha256_file "${HZR_TEMP_ROOT}/CURRENT_SHA256SUMS")"
HZR_CORE_VERSION="$(hzr_baseline_string core_version)"
HZR_SELECTION="HZR-owned current engine files, derived from immutable snapshot v2 and enumerated by CURRENT_FILES"

hzr_snapshot_emit_v2 \
  "${HZR_ENGINE_ROOT}" \
  "${HZR_ENGINE_METADATA_ROOT}/CURRENT_FILES" \
  "${HZR_ENGINE_METADATA_ROOT}/TRACKED_DELETIONS" \
  "${HZR_ENGINE_METADATA_ROOT}/TRACKED_CHANGES.patch" \
  "${HZR_ENGINE_METADATA_ROOT}/SOURCE_STATUS" \
  >"${HZR_TEMP_ROOT}/CURRENT_ENGINE_V1.tsv"

HZR_CURRENT_MANIFEST_SHA256="$(hzr_snapshot_sha256_file "${HZR_TEMP_ROOT}/CURRENT_ENGINE_V1.tsv")"
HZR_CURRENT_FILE_COUNT="$(wc -l <"${HZR_ENGINE_METADATA_ROOT}/CURRENT_FILES" | awk '{print $1}')"
HZR_PARENT_SNAPSHOT_SHA256="$(hzr_baseline_string snapshot_sha256)"

{
  printf 'schema_version = 1\n'
  printf 'engine_version = "%s"\n' "${HZR_CORE_VERSION}"
  printf 'hzr_release_line = "0.7.x"\n'
  printf 'parent_snapshot_sha256 = "%s"\n' "${HZR_PARENT_SNAPSHOT_SHA256}"
  printf 'manifest = "CURRENT_ENGINE_V1.tsv"\n'
  printf 'manifest_sha256 = "%s"\n' "${HZR_CURRENT_MANIFEST_SHA256}"
  printf 'content_manifest = "CURRENT_SHA256SUMS"\n'
  printf 'content_manifest_sha256 = "%s"\n' "${HZR_CONTENT_MANIFEST_SHA256}"
  printf 'files = "CURRENT_FILES"\n'
  printf 'included_file_count = %s\n' "${HZR_CURRENT_FILE_COUNT}"
  printf 'selection = "%s"\n' "${HZR_SELECTION}"
  printf 'ownership = "HZR-owned evolvable engine; changes live only in this repository"\n'
} >"${HZR_TEMP_ROOT}/CURRENT_ENGINE.toml"

for HZR_GENERATED_FILE in CURRENT_SHA256SUMS CURRENT_ENGINE_V1.tsv CURRENT_ENGINE.toml; do
  mv -- \
    "${HZR_TEMP_ROOT}/${HZR_GENERATED_FILE}" \
    "${HZR_ENGINE_METADATA_ROOT}/${HZR_GENERATED_FILE}"
done

echo "current engine manifest ${HZR_CURRENT_MANIFEST_SHA256} refreshed"
