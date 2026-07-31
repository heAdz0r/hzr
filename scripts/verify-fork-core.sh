#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_SNAPSHOT_ROOT="${HZR_REPOSITORY_ROOT}/fork-core"
HZR_CORE_ROOT="${HZR_SNAPSHOT_ROOT}/rtk"
HZR_RUN_TESTS="${1:-}"
HZR_EXPECTED_SNAPSHOT_SHA256="f4296ec404f461d6fc03c966c0dc79caee6c3118a73d1ed1a078ded5529f0a16"
HZR_EXPECTED_CONTENT_MANIFEST_SHA256="072a62adc754b728ec99a507d2c1a223d83077d067a9249a26d357eec890b4cc"
HZR_EXPECTED_CORE_VERSION="0.44.1-fork.1"

# shellcheck source=scripts/fork-snapshot-v2.sh
source "${HZR_REPOSITORY_ROOT}/scripts/fork-snapshot-v2.sh"

hzr_snapshot_toml_string() {
  local HZR_FIELD="$1"
  sed -n "s/^${HZR_FIELD} = \"\([^\"]*\)\"$/\\1/p" \
    "${HZR_SNAPSHOT_ROOT}/SNAPSHOT.toml"
}

hzr_snapshot_toml_integer() {
  local HZR_FIELD="$1"
  sed -n "s/^${HZR_FIELD} = \([0-9][0-9]*\)$/\\1/p" \
    "${HZR_SNAPSHOT_ROOT}/SNAPSHOT.toml"
}

hzr_current_toml_string() {
  local HZR_FIELD="$1"
  sed -n "s/^${HZR_FIELD} = \"\([^\"]*\)\"$/\\1/p" \
    "${HZR_SNAPSHOT_ROOT}/CURRENT_ENGINE.toml"
}

hzr_current_toml_integer() {
  local HZR_FIELD="$1"
  sed -n "s/^${HZR_FIELD} = \([0-9][0-9]*\)$/\\1/p" \
    "${HZR_SNAPSHOT_ROOT}/CURRENT_ENGINE.toml"
}

for HZR_REQUIRED_FILE in \
  SNAPSHOT.toml SNAPSHOT_V2.tsv FORK_FILES SHA256SUMS TRACKED_DELETIONS \
  TRACKED_CHANGES.patch SOURCE_STATUS CURRENT_ENGINE.toml \
  CURRENT_ENGINE_V1.tsv CURRENT_FILES CURRENT_SHA256SUMS; do
  if [[ ! -f "${HZR_SNAPSHOT_ROOT}/${HZR_REQUIRED_FILE}" ]]; then
    echo "fork-core manifest file is missing: ${HZR_REQUIRED_FILE}" >&2
    exit 1
  fi
done
HZR_NESTED_GIT_PATH="$(
  find "${HZR_CORE_ROOT}" \
    \( -type d \( -path "${HZR_CORE_ROOT}/target" -o -name .grepai \) -prune \) -o \
    -name .git -print -quit
)"
if [[ -n "${HZR_NESTED_GIT_PATH}" ]]; then
  echo "fork-core must not contain nested Git history: ${HZR_NESTED_GIT_PATH}" >&2
  exit 1
fi

HZR_SCHEMA_VERSION="$(hzr_snapshot_toml_integer schema_version)"
HZR_DECLARED_SNAPSHOT_SHA256="$(hzr_snapshot_toml_string snapshot_sha256)"
HZR_DECLARED_CONTENT_MANIFEST_SHA256="$(hzr_snapshot_toml_string content_manifest_sha256)"
HZR_ACTUAL_SNAPSHOT_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_SNAPSHOT_ROOT}/SNAPSHOT_V2.tsv"
)"
HZR_ACTUAL_CONTENT_MANIFEST_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_SNAPSHOT_ROOT}/SHA256SUMS"
)"
if [[ "${HZR_SCHEMA_VERSION}" != "2" ]]; then
  echo "unsupported fork-core snapshot schema: ${HZR_SCHEMA_VERSION:-missing}" >&2
  exit 1
fi
if [[ "${HZR_DECLARED_SNAPSHOT_SHA256}" != "${HZR_EXPECTED_SNAPSHOT_SHA256}" || \
  "${HZR_ACTUAL_SNAPSHOT_SHA256}" != "${HZR_EXPECTED_SNAPSHOT_SHA256}" ]]; then
  echo "fork-core canonical snapshot identity mismatch" >&2
  echo "expected: ${HZR_EXPECTED_SNAPSHOT_SHA256}" >&2
  echo "declared: ${HZR_DECLARED_SNAPSHOT_SHA256:-missing}" >&2
  echo "actual:   ${HZR_ACTUAL_SNAPSHOT_SHA256:-missing}" >&2
  exit 1
fi
if [[ "${HZR_DECLARED_CONTENT_MANIFEST_SHA256}" != \
    "${HZR_EXPECTED_CONTENT_MANIFEST_SHA256}" || \
  "${HZR_ACTUAL_CONTENT_MANIFEST_SHA256}" != \
    "${HZR_EXPECTED_CONTENT_MANIFEST_SHA256}" ]]; then
  echo "fork-core legacy content-manifest identity mismatch" >&2
  echo "expected: ${HZR_EXPECTED_CONTENT_MANIFEST_SHA256}" >&2
  echo "declared: ${HZR_DECLARED_CONTENT_MANIFEST_SHA256:-missing}" >&2
  echo "actual:   ${HZR_ACTUAL_CONTENT_MANIFEST_SHA256:-missing}" >&2
  exit 1
fi
if ! grep -Fqx -- "version = \"${HZR_EXPECTED_CORE_VERSION}\"" \
  "${HZR_CORE_ROOT}/Cargo.toml"; then
  echo "fork-core version mismatch: expected ${HZR_EXPECTED_CORE_VERSION}" >&2
  exit 1
fi
if [[ "$(hzr_snapshot_toml_string core_version)" != "${HZR_EXPECTED_CORE_VERSION}" ]]; then
  echo "fork-core declared version mismatch: expected ${HZR_EXPECTED_CORE_VERSION}" >&2
  exit 1
fi

HZR_CHECKSUM_OUTPUT="$(
  cd "${HZR_SNAPSHOT_ROOT}"
  shasum -a 256 -c CURRENT_SHA256SUMS 2>&1
)" || {
  echo "current fork-core file checksum verification failed:" >&2
  printf '%s\n' "${HZR_CHECKSUM_OUTPUT}" | grep -v ': OK$' >&2 || true
  exit 1
}

HZR_VERIFY_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-fork-verify.XXXXXX")"
cleanup_hzr_verify() {
  if [[ -n "${HZR_VERIFY_TEMP:-}" && -d "${HZR_VERIFY_TEMP}" ]]; then
    rm -rf -- "${HZR_VERIFY_TEMP}"
  fi
}
trap cleanup_hzr_verify EXIT

# In a Git checkout, prove that the immutable release tag still contains the
# byte-for-byte imported baseline. Standalone source bundles retain the signed
# baseline manifests but may not include repository history.
if git -C "${HZR_REPOSITORY_ROOT}" rev-parse --verify --quiet \
  "refs/tags/v0.1.0^{commit}" >/dev/null; then
  HZR_BASELINE_TAG_ROOT="${HZR_VERIFY_TEMP}/baseline-tag"
  mkdir -p "${HZR_BASELINE_TAG_ROOT}"
  git -C "${HZR_REPOSITORY_ROOT}" archive --format=tar v0.1.0 fork-core/rtk |
    tar -xf - -C "${HZR_BASELINE_TAG_ROOT}"
  HZR_BASELINE_CHECKSUM_OUTPUT="$(
    cd "${HZR_BASELINE_TAG_ROOT}/fork-core"
    shasum -a 256 -c "${HZR_SNAPSHOT_ROOT}/SHA256SUMS" 2>&1
  )" || {
    echo "v0.1.0 fork-core baseline checksum verification failed:" >&2
    printf '%s\n' "${HZR_BASELINE_CHECKSUM_OUTPUT}" | grep -v ': OK$' >&2 || true
    exit 1
  }
fi

HZR_CURRENT_DECLARED_MANIFEST_SHA256="$(hzr_current_toml_string manifest_sha256)"
HZR_CURRENT_ACTUAL_MANIFEST_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_SNAPSHOT_ROOT}/CURRENT_ENGINE_V1.tsv"
)"
HZR_CURRENT_DECLARED_CONTENT_SHA256="$(hzr_current_toml_string content_manifest_sha256)"
HZR_CURRENT_ACTUAL_CONTENT_SHA256="$(
  hzr_snapshot_sha256_file "${HZR_SNAPSHOT_ROOT}/CURRENT_SHA256SUMS"
)"
if [[ "$(hzr_current_toml_integer schema_version)" != "1" || \
  "$(hzr_current_toml_string parent_snapshot_sha256)" != \
    "${HZR_EXPECTED_SNAPSHOT_SHA256}" || \
  "$(hzr_current_toml_string engine_version)" != "${HZR_EXPECTED_CORE_VERSION}" || \
  "$(hzr_current_toml_string manifest)" != "CURRENT_ENGINE_V1.tsv" || \
  "$(hzr_current_toml_string content_manifest)" != "CURRENT_SHA256SUMS" || \
  "$(hzr_current_toml_string files)" != "CURRENT_FILES" ]]; then
  echo "current fork-core metadata is invalid" >&2
  exit 1
fi
if [[ "${HZR_CURRENT_DECLARED_MANIFEST_SHA256}" != \
    "${HZR_CURRENT_ACTUAL_MANIFEST_SHA256}" || \
  "${HZR_CURRENT_DECLARED_CONTENT_SHA256}" != \
    "${HZR_CURRENT_ACTUAL_CONTENT_SHA256}" ]]; then
  echo "current fork-core manifest identity mismatch" >&2
  exit 1
fi
if [[ "$(hzr_current_toml_integer included_file_count)" != \
  "$(wc -l <"${HZR_SNAPSHOT_ROOT}/CURRENT_FILES" | awk '{print $1}')" ]]; then
  echo "current fork-core file count mismatch" >&2
  exit 1
fi

HZR_SOURCE_REPOSITORY="$(hzr_snapshot_toml_string source_repository)"
HZR_SOURCE_BRANCH="$(hzr_snapshot_toml_string source_branch)"
HZR_SOURCE_HEAD="$(hzr_snapshot_toml_string source_head)"
HZR_SOURCE_DIFF_SHA256="$(hzr_snapshot_toml_string source_diff_sha256)"
HZR_SOURCE_STATUS_SHA256="$(hzr_snapshot_toml_string source_status_sha256)"
HZR_CONTENT_MANIFEST_SHA256="${HZR_CURRENT_ACTUAL_CONTENT_SHA256}"
HZR_CORE_VERSION="${HZR_EXPECTED_CORE_VERSION}"
HZR_SELECTION="$(hzr_current_toml_string selection)"

if [[ "$(hzr_snapshot_toml_string canonical_manifest)" != "SNAPSHOT_V2.tsv" || \
  "$(hzr_snapshot_toml_string canonical_format)" != "hzr-fork-snapshot-v2-tsv" || \
  "$(hzr_snapshot_toml_string path_encoding)" != "hex-lowercase" ]]; then
  echo "fork-core canonical manifest metadata is invalid" >&2
  exit 1
fi
if [[ "$(hzr_snapshot_toml_string source_status_record_sha256)" != \
    "$(hzr_snapshot_sha256_file "${HZR_SNAPSHOT_ROOT}/SOURCE_STATUS")" || \
  "$(hzr_snapshot_toml_string fork_files_sha256)" != \
    "$(hzr_snapshot_sha256_file "${HZR_SNAPSHOT_ROOT}/FORK_FILES")" || \
  "$(hzr_snapshot_toml_string tracked_deletions_sha256)" != \
    "$(hzr_snapshot_sha256_file "${HZR_SNAPSHOT_ROOT}/TRACKED_DELETIONS")" ]]; then
  echo "fork-core auxiliary manifest digest mismatch" >&2
  exit 1
fi
if [[ "$(hzr_snapshot_toml_integer included_file_count)" != \
    "$(wc -l <"${HZR_SNAPSHOT_ROOT}/FORK_FILES" | awk '{print $1}')" || \
  "$(hzr_snapshot_toml_integer tracked_deleted_count)" != \
    "$(wc -l <"${HZR_SNAPSHOT_ROOT}/TRACKED_DELETIONS" | awk '{print $1}')" ]]; then
  echo "fork-core declared entry/deletion counts do not match their records" >&2
  exit 1
fi
if [[ "$(hzr_snapshot_toml_integer exclusion_record_count)" != \
  "$(awk -F '\t' '$1 == "exclusion" { count += 1 } END { print count + 0 }' \
    "${HZR_SNAPSHOT_ROOT}/SNAPSHOT_V2.tsv")" ]]; then
  echo "fork-core declared exclusion count does not match canonical records" >&2
  exit 1
fi

hzr_snapshot_emit_v2 \
  "${HZR_CORE_ROOT}" \
  "${HZR_SNAPSHOT_ROOT}/CURRENT_FILES" \
  "${HZR_SNAPSHOT_ROOT}/TRACKED_DELETIONS" \
  "${HZR_SNAPSHOT_ROOT}/TRACKED_CHANGES.patch" \
  "${HZR_SNAPSHOT_ROOT}/SOURCE_STATUS" \
  >"${HZR_VERIFY_TEMP}/CURRENT_ENGINE_V1.actual.tsv"
if ! cmp -s \
  "${HZR_SNAPSHOT_ROOT}/CURRENT_ENGINE_V1.tsv" \
  "${HZR_VERIFY_TEMP}/CURRENT_ENGINE_V1.actual.tsv"; then
  echo "current fork-core entry/type/mode/size/content metadata drift detected:" >&2
  diff -u \
    "${HZR_SNAPSHOT_ROOT}/CURRENT_ENGINE_V1.tsv" \
    "${HZR_VERIFY_TEMP}/CURRENT_ENGINE_V1.actual.tsv" | sed -n '1,80p' >&2 || true
  exit 1
fi

HZR_IGNORE_ROOT="${HZR_VERIFY_TEMP}/ignore-root"
mkdir -p "${HZR_IGNORE_ROOT}"
git -C "${HZR_IGNORE_ROOT}" init --quiet
while IFS= read -r HZR_RELATIVE_PATH; do
  if [[ "${HZR_RELATIVE_PATH}" != ".gitignore" && \
    "${HZR_RELATIVE_PATH}" != */.gitignore ]]; then
    continue
  fi
  HZR_IGNORE_PATH="${HZR_IGNORE_ROOT}/${HZR_RELATIVE_PATH}"
  mkdir -p -- "$(dirname -- "${HZR_IGNORE_PATH}")"
  cp -p -- "${HZR_CORE_ROOT}/${HZR_RELATIVE_PATH}" "${HZR_IGNORE_PATH}"
done <"${HZR_SNAPSHOT_ROOT}/CURRENT_FILES"

while IFS= read -r HZR_ACTUAL_PATH; do
  HZR_RELATIVE_PATH="${HZR_ACTUAL_PATH#${HZR_CORE_ROOT}/}"
  if grep -Fqx -- "${HZR_RELATIVE_PATH}" "${HZR_SNAPSHOT_ROOT}/CURRENT_FILES"; then
    continue
  fi
  if git -C "${HZR_IGNORE_ROOT}" check-ignore --no-index -q -- "${HZR_RELATIVE_PATH}"; then
    continue
  fi
  printf '%s\n' "${HZR_RELATIVE_PATH}" >>"${HZR_VERIFY_TEMP}/unexpected"
done < <(
  find "${HZR_CORE_ROOT}" \
    \( -type d \( -path "${HZR_CORE_ROOT}/target" -o -name .grepai -o -name __pycache__ \) -prune \) -o \
    \( -type f -o -type l \) -print
)

if [[ -s "${HZR_VERIFY_TEMP}/unexpected" ]]; then
  echo "unexpected non-ignored files are absent from CURRENT_FILES:" >&2
  sort "${HZR_VERIFY_TEMP}/unexpected" >&2
  exit 1
fi

if [[ -n "${HZR_RUN_TESTS}" && "${HZR_RUN_TESTS}" != "--test" ]]; then
  echo "usage: scripts/verify-fork-core.sh [--test]" >&2
  exit 2
fi

if [[ "${HZR_RUN_TESTS}" == "--test" ]]; then
  HZR_TEST_ROOT="${HZR_VERIFY_TEMP}/rtk"
  mkdir -p "${HZR_TEST_ROOT}"
  while IFS= read -r HZR_RELATIVE_PATH; do
    hzr_snapshot_validate_relative_path "${HZR_RELATIVE_PATH}"
    HZR_TEST_PATH="${HZR_TEST_ROOT}/${HZR_RELATIVE_PATH}"
    mkdir -p -- "$(dirname -- "${HZR_TEST_PATH}")"
    cp -pP -- "${HZR_CORE_ROOT}/${HZR_RELATIVE_PATH}" "${HZR_TEST_PATH}"
  done <"${HZR_SNAPSHOT_ROOT}/CURRENT_FILES"

  git -C "${HZR_TEST_ROOT}" init --quiet
  git -C "${HZR_TEST_ROOT}" config user.name "HZR Snapshot Verifier"
  git -C "${HZR_TEST_ROOT}" config user.email "snapshot-verifier@hzr.invalid"
  git -C "${HZR_TEST_ROOT}" add -f --all
  git -C "${HZR_TEST_ROOT}" commit --quiet -m "Synthetic fork-core snapshot"

  # The fork's real-repository churn test requires one file to occur more than
  # once in history. Create that history only in the disposable clone, then
  # restore the exact snapshot bytes before compiling and running the suite.
  cp -p -- "${HZR_TEST_ROOT}/src/main.rs" "${HZR_VERIFY_TEMP}/main.rs.snapshot"
  printf '\n// synthetic churn fixture\n' >>"${HZR_TEST_ROOT}/src/main.rs"
  git -C "${HZR_TEST_ROOT}" add -f -- src/main.rs
  git -C "${HZR_TEST_ROOT}" commit --quiet -m "Synthetic churn fixture"
  cp -p -- "${HZR_VERIFY_TEMP}/main.rs.snapshot" "${HZR_TEST_ROOT}/src/main.rs"
  git -C "${HZR_TEST_ROOT}" add -f -- src/main.rs
  git -C "${HZR_TEST_ROOT}" commit --quiet -m "Restore exact fork-core snapshot"

  HZR_TEST_MAIN_SHA256="$(shasum -a 256 "${HZR_TEST_ROOT}/src/main.rs" | awk '{print $1}')"
  HZR_CORE_MAIN_SHA256="$(shasum -a 256 "${HZR_CORE_ROOT}/src/main.rs" | awk '{print $1}')"
  if [[ "${HZR_TEST_MAIN_SHA256}" != "${HZR_CORE_MAIN_SHA256}" ]]; then
    echo "synthetic test repository did not restore exact src/main.rs bytes" >&2
    exit 1
  fi

  CARGO_TARGET_DIR="${HZR_VERIFY_TEMP}/target" \
    cargo test --quiet --manifest-path "${HZR_TEST_ROOT}/Cargo.toml" --locked --all-targets
fi

echo "fork-core baseline ${HZR_EXPECTED_SNAPSHOT_SHA256} and current engine ${HZR_CURRENT_ACTUAL_MANIFEST_SHA256} verified"
