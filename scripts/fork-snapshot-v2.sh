#!/usr/bin/env bash

# Shared canonicalization primitives for fork-core import and verification.
# This file is sourced by the entry-point scripts; it intentionally does not
# change shell options or execute work at load time.

hzr_snapshot_hex() {
  LC_ALL=C od -An -v -tx1 | tr -d ' \n'
}

hzr_snapshot_sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

hzr_snapshot_symlink_bytes() {
  perl -e '
    use strict;
    use warnings;
    binmode STDOUT;
    my $target = readlink($ARGV[0]);
    die "readlink failed: $ARGV[0]\n" unless defined $target;
    print $target;
  ' "$1"
}

hzr_snapshot_file_mode() {
  local HZR_ENTRY_PATH="$1"

  # Git repositories portably retain the regular-file executable bit, not
  # owner/group permission differences such as 0600 vs 0644. Canonicalize to
  # Git tree modes so a clean checkout reproduces the same snapshot identity.
  if [[ -x "${HZR_ENTRY_PATH}" ]]; then
    printf '100755'
  else
    printf '100644'
  fi
}

hzr_snapshot_file_size() {
  local HZR_ENTRY_PATH="$1"
  local HZR_STAT_SIZE

  if HZR_STAT_SIZE="$(stat -f '%z' "${HZR_ENTRY_PATH}" 2>/dev/null)" &&
    [[ "${HZR_STAT_SIZE}" =~ ^[0-9]+$ ]]; then
    printf '%s' "${HZR_STAT_SIZE}"
    return
  fi
  stat -c '%s' -- "${HZR_ENTRY_PATH}"
}

hzr_snapshot_validate_relative_path() {
  local HZR_RELATIVE_PATH="$1"

  case "${HZR_RELATIVE_PATH}" in
    "" | /* | . | .. | ./* | ../* | */./* | */../* | */. | */..)
      echo "unsafe path in fork-core manifest: ${HZR_RELATIVE_PATH}" >&2
      return 1
      ;;
  esac
}

hzr_snapshot_emit_entry() {
  local HZR_CORE_ROOT="$1"
  local HZR_RELATIVE_PATH="$2"
  local HZR_ENTRY_PATH
  local HZR_PATH_HEX
  local HZR_KIND
  local HZR_MODE
  local HZR_SIZE
  local HZR_SHA256

  hzr_snapshot_validate_relative_path "${HZR_RELATIVE_PATH}"
  HZR_ENTRY_PATH="${HZR_CORE_ROOT}/${HZR_RELATIVE_PATH}"
  HZR_PATH_HEX="$(printf '%s' "${HZR_RELATIVE_PATH}" | hzr_snapshot_hex)"

  if [[ -L "${HZR_ENTRY_PATH}" ]]; then
    HZR_KIND="symlink"
    HZR_MODE="120000"
    HZR_SIZE="$(hzr_snapshot_symlink_bytes "${HZR_ENTRY_PATH}" | wc -c | awk '{print $1}')"
    HZR_SHA256="$(hzr_snapshot_symlink_bytes "${HZR_ENTRY_PATH}" | shasum -a 256 | awk '{print $1}')"
  elif [[ -f "${HZR_ENTRY_PATH}" ]]; then
    HZR_KIND="file"
    HZR_MODE="$(hzr_snapshot_file_mode "${HZR_ENTRY_PATH}")"
    HZR_SIZE="$(hzr_snapshot_file_size "${HZR_ENTRY_PATH}")"
    HZR_SHA256="$(hzr_snapshot_sha256_file "${HZR_ENTRY_PATH}")"
  else
    echo "fork-core entry is missing or has unsupported type: ${HZR_RELATIVE_PATH}" >&2
    return 1
  fi

  printf 'entry\t%s\t%s\t%s\t%s\t%s\n' \
    "${HZR_PATH_HEX}" "${HZR_KIND}" "${HZR_MODE}" "${HZR_SIZE}" "${HZR_SHA256}"
}

hzr_snapshot_emit_deletion() {
  local HZR_RELATIVE_PATH="$1"
  local HZR_PATH_HEX

  hzr_snapshot_validate_relative_path "${HZR_RELATIVE_PATH}"
  HZR_PATH_HEX="$(printf '%s' "${HZR_RELATIVE_PATH}" | hzr_snapshot_hex)"
  printf 'deletion\t%s\n' "${HZR_PATH_HEX}"
}

hzr_snapshot_emit_ignore_file() {
  local HZR_IGNORE_PATH="$1"
  local HZR_IGNORE_SCOPE="$2"
  local HZR_SCOPE_HEX
  local HZR_REASON="source ignore rule"
  local HZR_RULE
  local HZR_RULE_HEX
  local HZR_REASON_HEX

  HZR_SCOPE_HEX="$(printf '%s' "${HZR_IGNORE_SCOPE}" | hzr_snapshot_hex)"
  while IFS= read -r HZR_RULE || [[ -n "${HZR_RULE}" ]]; do
    case "${HZR_RULE}" in
      "")
        continue
        ;;
      \#*)
        HZR_REASON="${HZR_RULE#\#}"
        HZR_REASON="${HZR_REASON#"${HZR_REASON%%[![:space:]]*}"}"
        [[ -n "${HZR_REASON}" ]] || HZR_REASON="source ignore rule"
        continue
        ;;
    esac
    HZR_RULE_HEX="$(printf '%s' "${HZR_RULE}" | hzr_snapshot_hex)"
    HZR_REASON_HEX="$(printf '%s' "${HZR_REASON}" | hzr_snapshot_hex)"
    printf 'exclusion\t%s\t%s\t%s\n' "${HZR_SCOPE_HEX}" "${HZR_RULE_HEX}" "${HZR_REASON_HEX}"
  done <"${HZR_IGNORE_PATH}"
}

hzr_snapshot_emit_exclusions() {
  local HZR_CORE_ROOT="$1"
  local HZR_FILE_LIST="$2"
  local HZR_RELATIVE_PATH
  local HZR_SCOPE
  local HZR_BUILTIN_SCOPE_HEX
  local HZR_BUILTIN_RULE_HEX
  local HZR_BUILTIN_REASON_HEX

  {
    HZR_BUILTIN_SCOPE_HEX="$(printf '.' | hzr_snapshot_hex)"
    HZR_BUILTIN_RULE_HEX="$(printf '.git/**' | hzr_snapshot_hex)"
    HZR_BUILTIN_REASON_HEX="$(printf 'nested source-control metadata is forbidden' | hzr_snapshot_hex)"
    printf 'exclusion\t%s\t%s\t%s\n' \
      "${HZR_BUILTIN_SCOPE_HEX}" "${HZR_BUILTIN_RULE_HEX}" "${HZR_BUILTIN_REASON_HEX}"

    while IFS= read -r HZR_RELATIVE_PATH; do
      if [[ "${HZR_RELATIVE_PATH}" != ".gitignore" && \
        "${HZR_RELATIVE_PATH}" != */.gitignore ]]; then
        continue
      fi
      HZR_SCOPE="$(dirname -- "${HZR_RELATIVE_PATH}")"
      hzr_snapshot_emit_ignore_file \
        "${HZR_CORE_ROOT}/${HZR_RELATIVE_PATH}" "${HZR_SCOPE}"
    done <"${HZR_FILE_LIST}"
  } | LC_ALL=C sort
}

hzr_snapshot_emit_v2() {
  local HZR_CORE_ROOT="$1"
  local HZR_FILE_LIST="$2"
  local HZR_DELETION_LIST="$3"
  local HZR_TRACKED_CHANGES="$4"
  local HZR_SOURCE_STATUS_RECORD="$5"
  local HZR_ENTRY_COUNT
  local HZR_DELETION_COUNT
  local HZR_RELATIVE_PATH
  local HZR_TRACKED_CHANGES_SHA256
  local HZR_SOURCE_STATUS_RECORD_SHA256
  local HZR_DELETION_RECORD_SHA256
  local HZR_FILE_LIST_SHA256

  : "${HZR_SOURCE_REPOSITORY:?missing source repository}"
  : "${HZR_SOURCE_BRANCH:?missing source branch}"
  : "${HZR_SOURCE_HEAD:?missing source HEAD}"
  : "${HZR_SOURCE_DIFF_SHA256:?missing source diff digest}"
  : "${HZR_SOURCE_STATUS_SHA256:?missing source status digest}"
  : "${HZR_CONTENT_MANIFEST_SHA256:?missing content manifest digest}"
  : "${HZR_CORE_VERSION:?missing core version}"
  : "${HZR_SELECTION:?missing selection rule}"

  HZR_ENTRY_COUNT="$(wc -l <"${HZR_FILE_LIST}" | awk '{print $1}')"
  HZR_DELETION_COUNT="$(wc -l <"${HZR_DELETION_LIST}" | awk '{print $1}')"
  HZR_TRACKED_CHANGES_SHA256="$(hzr_snapshot_sha256_file "${HZR_TRACKED_CHANGES}")"
  HZR_SOURCE_STATUS_RECORD_SHA256="$(hzr_snapshot_sha256_file "${HZR_SOURCE_STATUS_RECORD}")"
  HZR_DELETION_RECORD_SHA256="$(hzr_snapshot_sha256_file "${HZR_DELETION_LIST}")"
  HZR_FILE_LIST_SHA256="$(hzr_snapshot_sha256_file "${HZR_FILE_LIST}")"

  printf 'format\thzr-fork-snapshot\n'
  printf 'schema_version\t2\n'
  printf 'path_encoding\thex-lowercase\n'
  printf 'core_version\t%s\n' "${HZR_CORE_VERSION}"
  printf 'source_repository_hex\t%s\n' "$(printf '%s' "${HZR_SOURCE_REPOSITORY}" | hzr_snapshot_hex)"
  printf 'source_branch_hex\t%s\n' "$(printf '%s' "${HZR_SOURCE_BRANCH}" | hzr_snapshot_hex)"
  printf 'source_head\t%s\n' "${HZR_SOURCE_HEAD}"
  printf 'source_diff_sha256\t%s\n' "${HZR_SOURCE_DIFF_SHA256}"
  printf 'tracked_changes_record_sha256\t%s\n' "${HZR_TRACKED_CHANGES_SHA256}"
  printf 'source_status_sha256\t%s\n' "${HZR_SOURCE_STATUS_SHA256}"
  printf 'source_status_record_sha256\t%s\n' "${HZR_SOURCE_STATUS_RECORD_SHA256}"
  printf 'selection_hex\t%s\n' "$(printf '%s' "${HZR_SELECTION}" | hzr_snapshot_hex)"
  printf 'content_manifest_sha256\t%s\n' "${HZR_CONTENT_MANIFEST_SHA256}"
  printf 'fork_files_record_sha256\t%s\n' "${HZR_FILE_LIST_SHA256}"
  printf 'tracked_deletions_record_sha256\t%s\n' "${HZR_DELETION_RECORD_SHA256}"
  printf 'entry_count\t%s\n' "${HZR_ENTRY_COUNT}"
  printf 'tracked_deletion_count\t%s\n' "${HZR_DELETION_COUNT}"

  while IFS= read -r HZR_RELATIVE_PATH; do
    hzr_snapshot_emit_entry "${HZR_CORE_ROOT}" "${HZR_RELATIVE_PATH}"
  done <"${HZR_FILE_LIST}" | LC_ALL=C sort

  while IFS= read -r HZR_RELATIVE_PATH; do
    hzr_snapshot_emit_deletion "${HZR_RELATIVE_PATH}"
  done <"${HZR_DELETION_LIST}" | LC_ALL=C sort

  hzr_snapshot_emit_exclusions "${HZR_CORE_ROOT}" "${HZR_FILE_LIST}"
}
