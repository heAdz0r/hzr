#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_SOURCE_ROOT="${1:-}"
HZR_DESTINATION_ROOT="${HZR_REPOSITORY_ROOT}/fork-core"
HZR_DESTINATION_TREE="${HZR_DESTINATION_ROOT}/rtk"
HZR_SELECTION="git tracked files present in the worktree plus untracked files accepted by repository .gitignore rules only"

# shellcheck source=scripts/fork-snapshot-v2.sh
source "${HZR_REPOSITORY_ROOT}/scripts/fork-snapshot-v2.sh"

if [[ -z "${HZR_SOURCE_ROOT}" ]]; then
  echo "usage: scripts/import-fork-core.sh /absolute/path/to/current-rtk-worktree" >&2
  exit 2
fi

HZR_SOURCE_ROOT="$(cd -- "${HZR_SOURCE_ROOT}" && pwd -P)"
if [[ "$(git -C "${HZR_SOURCE_ROOT}" rev-parse --is-inside-work-tree 2>/dev/null)" != "true" ]]; then
  echo "fork source is not a Git worktree: ${HZR_SOURCE_ROOT}" >&2
  exit 2
fi
if [[ -e "${HZR_DESTINATION_ROOT}" ]]; then
  echo "fork-core already exists; refusing to overwrite: ${HZR_DESTINATION_ROOT}" >&2
  exit 2
fi

HZR_SOURCE_HEAD="$(git -C "${HZR_SOURCE_ROOT}" rev-parse HEAD)"
HZR_SOURCE_BRANCH="$(git -C "${HZR_SOURCE_ROOT}" branch --show-current)"
HZR_SOURCE_ORIGIN="$(git -C "${HZR_SOURCE_ROOT}" remote get-url origin)"
HZR_SOURCE_REPOSITORY="${HZR_SOURCE_ORIGIN}"
HZR_SOURCE_DIFF_SHA256="$(git -C "${HZR_SOURCE_ROOT}" diff --binary HEAD | shasum -a 256 | awk '{print $1}')"
HZR_SOURCE_STATUS_SHA256="$(git -C "${HZR_SOURCE_ROOT}" status --porcelain=v1 -z --untracked-files=all | shasum -a 256 | awk '{print $1}')"
HZR_IMPORT_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-fork-import.XXXXXX")"

cleanup_hzr_import() {
  if [[ -n "${HZR_IMPORT_TEMP:-}" && -d "${HZR_IMPORT_TEMP}" ]]; then
    rm -rf -- "${HZR_IMPORT_TEMP}"
  fi
}
trap cleanup_hzr_import EXIT

mkdir -p "${HZR_IMPORT_TEMP}/fork-core/rtk"
: >"${HZR_IMPORT_TEMP}/fork-core/FORK_FILES"
: >"${HZR_IMPORT_TEMP}/fork-core/SHA256SUMS"
: >"${HZR_IMPORT_TEMP}/fork-core/TRACKED_DELETIONS"

HZR_FILE_COUNT=0
while IFS= read -r -d '' HZR_RELATIVE_PATH; do
  HZR_SOURCE_PATH="${HZR_SOURCE_ROOT}/${HZR_RELATIVE_PATH}"
  if [[ ! -f "${HZR_SOURCE_PATH}" && ! -L "${HZR_SOURCE_PATH}" ]]; then
    continue
  fi
  if [[ "${HZR_RELATIVE_PATH}" == *$'\n'* ]]; then
    echo "newline in source path is unsupported by the auditable manifest" >&2
    exit 1
  fi

  HZR_DESTINATION_PATH="${HZR_IMPORT_TEMP}/fork-core/rtk/${HZR_RELATIVE_PATH}"
  mkdir -p -- "$(dirname -- "${HZR_DESTINATION_PATH}")"
  cp -pP -- "${HZR_SOURCE_PATH}" "${HZR_DESTINATION_PATH}"

  if [[ -L "${HZR_DESTINATION_PATH}" ]]; then
    HZR_FILE_SHA256="$(
      hzr_snapshot_symlink_bytes "${HZR_DESTINATION_PATH}" | shasum -a 256 | awk '{print $1}'
    )"
  else
    HZR_FILE_SHA256="$(shasum -a 256 "${HZR_DESTINATION_PATH}" | awk '{print $1}')"
  fi
  printf '%s\n' "${HZR_RELATIVE_PATH}" >>"${HZR_IMPORT_TEMP}/fork-core/FORK_FILES"
  printf '%s  %s\n' "${HZR_FILE_SHA256}" "rtk/${HZR_RELATIVE_PATH}" \
    >>"${HZR_IMPORT_TEMP}/fork-core/SHA256SUMS"
  HZR_FILE_COUNT=$((HZR_FILE_COUNT + 1))
done < <(
  git -C "${HZR_SOURCE_ROOT}" \
    ls-files --cached --others --exclude-per-directory=.gitignore -z
)

HZR_DELETED_COUNT=0
while IFS= read -r -d '' HZR_DELETED_PATH; do
  if [[ "${HZR_DELETED_PATH}" == *$'\n'* ]]; then
    echo "newline in deleted source path is unsupported by the auditable manifest" >&2
    exit 1
  fi
  printf '%s\n' "${HZR_DELETED_PATH}" >>"${HZR_IMPORT_TEMP}/fork-core/TRACKED_DELETIONS"
  HZR_DELETED_COUNT=$((HZR_DELETED_COUNT + 1))
done < <(git -C "${HZR_SOURCE_ROOT}" diff --name-only --diff-filter=D -z HEAD)
LC_ALL=C sort -u -o "${HZR_IMPORT_TEMP}/fork-core/TRACKED_DELETIONS" \
  "${HZR_IMPORT_TEMP}/fork-core/TRACKED_DELETIONS"

git -C "${HZR_SOURCE_ROOT}" diff --binary HEAD \
  >"${HZR_IMPORT_TEMP}/fork-core/TRACKED_CHANGES.patch"
git -C "${HZR_SOURCE_ROOT}" status --porcelain=v1 --untracked-files=all \
  >"${HZR_IMPORT_TEMP}/fork-core/SOURCE_STATUS"

HZR_CONTENT_MANIFEST_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_IMPORT_TEMP}/fork-core/SHA256SUMS"
)"
HZR_CORE_VERSION="$(
  awk '
    /^\[package\]$/ { in_package = 1; next }
    in_package && /^version = "/ {
      value = $0
      sub(/^version = "/, "", value)
      sub(/"$/, "", value)
      print value
      exit
    }
  ' "${HZR_IMPORT_TEMP}/fork-core/rtk/Cargo.toml"
)"
HZR_SOURCE_STATUS_RECORD_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_IMPORT_TEMP}/fork-core/SOURCE_STATUS"
)"
HZR_TRACKED_DELETIONS_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_IMPORT_TEMP}/fork-core/TRACKED_DELETIONS"
)"
HZR_FORK_FILES_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_IMPORT_TEMP}/fork-core/FORK_FILES"
)"

hzr_snapshot_emit_v2 \
  "${HZR_IMPORT_TEMP}/fork-core/rtk" \
  "${HZR_IMPORT_TEMP}/fork-core/FORK_FILES" \
  "${HZR_IMPORT_TEMP}/fork-core/TRACKED_DELETIONS" \
  "${HZR_IMPORT_TEMP}/fork-core/TRACKED_CHANGES.patch" \
  "${HZR_IMPORT_TEMP}/fork-core/SOURCE_STATUS" \
  >"${HZR_IMPORT_TEMP}/fork-core/SNAPSHOT_V2.tsv"
HZR_SNAPSHOT_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_IMPORT_TEMP}/fork-core/SNAPSHOT_V2.tsv"
)"
HZR_EXCLUSION_COUNT="$(
  awk -F '\t' '$1 == "exclusion" { count += 1 } END { print count + 0 }' \
    "${HZR_IMPORT_TEMP}/fork-core/SNAPSHOT_V2.tsv"
)"
HZR_CREATED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
{
  printf 'schema_version = 2\n'
  printf 'core_version = "%s"\n' "${HZR_CORE_VERSION}"
  printf 'source_repository = "%s"\n' "${HZR_SOURCE_ORIGIN}"
  printf 'source_branch = "%s"\n' "${HZR_SOURCE_BRANCH}"
  printf 'source_head = "%s"\n' "${HZR_SOURCE_HEAD}"
  printf 'source_diff_sha256 = "%s"\n' "${HZR_SOURCE_DIFF_SHA256}"
  printf 'source_status_sha256 = "%s"\n' "${HZR_SOURCE_STATUS_SHA256}"
  printf 'source_status_record_sha256 = "%s"\n' "${HZR_SOURCE_STATUS_RECORD_SHA256}"
  printf 'snapshot_sha256 = "%s"\n' "${HZR_SNAPSHOT_SHA256}"
  printf 'content_manifest_sha256 = "%s"\n' "${HZR_CONTENT_MANIFEST_SHA256}"
  printf 'fork_files_sha256 = "%s"\n' "${HZR_FORK_FILES_SHA256}"
  printf 'tracked_deletions_sha256 = "%s"\n' "${HZR_TRACKED_DELETIONS_SHA256}"
  printf 'canonical_manifest = "SNAPSHOT_V2.tsv"\n'
  printf 'canonical_format = "hzr-fork-snapshot-v2-tsv"\n'
  printf 'path_encoding = "hex-lowercase"\n'
  printf 'included_file_count = %s\n' "${HZR_FILE_COUNT}"
  printf 'tracked_deleted_count = %s\n' "${HZR_DELETED_COUNT}"
  printf 'exclusion_record_count = %s\n' "${HZR_EXCLUSION_COUNT}"
  printf 'created_at = "%s"\n' "${HZR_CREATED_AT}"
  printf 'selection = "%s"\n' "${HZR_SELECTION}"
  printf '%s\n' 'excluded = [".git/** (forbidden nested history)", "repository .gitignore rules with recorded category reasons"]'
} >"${HZR_IMPORT_TEMP}/fork-core/SNAPSHOT.toml"

(
  cd "${HZR_IMPORT_TEMP}/fork-core"
  shasum -a 256 -c SHA256SUMS >/dev/null
)

mv -- "${HZR_IMPORT_TEMP}/fork-core" "${HZR_DESTINATION_ROOT}"
echo "Imported ${HZR_FILE_COUNT} fork files (${HZR_DELETED_COUNT} tracked deletions recorded)."
echo "Snapshot v2 SHA-256: ${HZR_SNAPSHOT_SHA256}"
echo "Legacy content-manifest SHA-256: ${HZR_CONTENT_MANIFEST_SHA256}"
