#!/usr/bin/env bash
# 0.8.3: bounded hygiene for the Cargo target directories.
#
# Cargo never removes superseded artefacts from `target/`: every dependency bump, feature set,
# RUSTFLAGS or toolchain change adds a new hashed variant next to the old one, and debug info
# multiplies the size of each. On 2026-09-05 `target/debug` alone reached 58 GB and filled the
# disk. This script removes artefacts whose fingerprint has not been used for --max-age-days
# and, if a debug directory still exceeds --cap-gb afterwards, resets that profile with
# `cargo clean --profile dev`. Release artefacts and `target/hzr-component-cache` are never
# touched: the bundle builder owns them and rebuilding pinned components costs minutes.
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_HYGIENE_MAX_AGE_DAYS="${HZR_HYGIENE_MAX_AGE_DAYS:-7}"
HZR_HYGIENE_CAP_GB="${HZR_HYGIENE_CAP_GB:-20}"
HZR_HYGIENE_DRY_RUN=0

usage() {
  echo "usage: scripts/target-hygiene.sh [--max-age-days N] [--cap-gb N] [--dry-run]" >&2
  exit 2
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --max-age-days)
      [[ "$#" -ge 2 ]] || usage
      HZR_HYGIENE_MAX_AGE_DAYS="$2"
      shift 2
      ;;
    --cap-gb)
      [[ "$#" -ge 2 ]] || usage
      HZR_HYGIENE_CAP_GB="$2"
      shift 2
      ;;
    --dry-run)
      HZR_HYGIENE_DRY_RUN=1
      shift
      ;;
    *)
      usage
      ;;
  esac
done
if [[ ! "${HZR_HYGIENE_MAX_AGE_DAYS}" =~ ^[0-9]+$ || ! "${HZR_HYGIENE_CAP_GB}" =~ ^[0-9]+$ ]]; then
  usage
fi

# Every Cargo workspace in the repository: `<manifest>:<target directory>`.
HZR_HYGIENE_WORKSPACES=(
  "${HZR_REPOSITORY_ROOT}/Cargo.toml:${HZR_REPOSITORY_ROOT}/target"
  "${HZR_REPOSITORY_ROOT}/fork-core/rtk/Cargo.toml:${HZR_REPOSITORY_ROOT}/fork-core/rtk/target"
)

# `du` exits non-zero when a concurrent cargo build removes a file under it; the size it
# printed is still usable, so the pipeline failure must not end the script under `pipefail`.
size_kb() {
  local kb
  kb="$(du -sk -- "$1" 2>/dev/null | awk '{print $1}' || true)"
  printf '%s\n' "${kb:-0}"
}

format_gb() {
  awk -v kb="$1" 'BEGIN { printf "%.2f GB", kb / 1024 / 1024 }'
}

remove_path() {
  if [[ "${HZR_HYGIENE_DRY_RUN}" == 1 ]]; then
    echo "  would remove $1"
  else
    rm -rf -- "$1"
  fi
}

# One fingerprint unit `<name>-<hash>` owns `deps/*-<hash>` (test binaries), `deps/*-<hash>.*`
# (rlib, rmeta, dylib, .d and split debug objects), `build/<name>-<hash>` and its fingerprint.
sweep_unit() {
  local debug_dir="$1" unit="$2"
  local hash="${unit##*-}"
  if [[ ! "${hash}" =~ ^[0-9a-f]{16}$ ]]; then
    return 0
  fi
  local artefact
  for artefact in \
    "${debug_dir}/deps/"*"-${hash}" \
    "${debug_dir}/deps/"*"-${hash}."* \
    "${debug_dir}/build/${unit}" \
    "${debug_dir}/.fingerprint/${unit}"; do
    if [[ -e "${artefact}" ]]; then
      remove_path "${artefact}"
    fi
  done
}

sweep_debug_dir() {
  local debug_dir="$1"
  if [[ ! -d "${debug_dir}" ]]; then
    return 0
  fi
  local before after stamp unit session
  before="$(size_kb "${debug_dir}")"
  echo "sweeping ${debug_dir}: $(format_gb "${before}"), removing artefacts idle for more than ${HZR_HYGIENE_MAX_AGE_DAYS} days"
  if [[ -d "${debug_dir}/.fingerprint" ]]; then
    # Cargo touches `invoked.timestamp` on every build that uses the unit, fresh or rebuilt.
    while IFS= read -r stamp; do
      unit="$(basename -- "$(dirname -- "${stamp}")")"
      sweep_unit "${debug_dir}" "${unit}"
    done < <(find "${debug_dir}/.fingerprint" -mindepth 2 -maxdepth 2 -name invoked.timestamp \
      -mtime "+${HZR_HYGIENE_MAX_AGE_DAYS}" 2>/dev/null)
  fi
  if [[ -d "${debug_dir}/incremental" ]]; then
    while IFS= read -r session; do
      remove_path "${session}"
    done < <(find "${debug_dir}/incremental" -mindepth 1 -maxdepth 1 -type d \
      -mtime "+${HZR_HYGIENE_MAX_AGE_DAYS}" 2>/dev/null)
  fi
  after="$(size_kb "${debug_dir}")"
  echo "  ${debug_dir}: $(format_gb "${before}") -> $(format_gb "${after}")"
}

enforce_cap() {
  local manifest="$1" debug_dir="$2"
  if [[ ! -d "${debug_dir}" ]]; then
    return 0
  fi
  local size cap_kb
  size="$(size_kb "${debug_dir}")"
  cap_kb=$((HZR_HYGIENE_CAP_GB * 1024 * 1024))
  if [[ "${size}" -gt "${cap_kb}" ]]; then
    echo "  ${debug_dir} still holds $(format_gb "${size}") (cap ${HZR_HYGIENE_CAP_GB} GB); resetting the dev profile"
    if [[ "${HZR_HYGIENE_DRY_RUN}" == 1 ]]; then
      echo "  would run: cargo clean --profile dev --manifest-path ${manifest}"
    else
      cargo clean --profile dev --manifest-path "${manifest}"
    fi
  fi
}

for entry in "${HZR_HYGIENE_WORKSPACES[@]}"; do
  manifest="${entry%%:*}"
  target_dir="${entry#*:}"
  if [[ ! -f "${manifest}" || ! -d "${target_dir}" ]]; then
    continue
  fi
  # The host profile directory plus every `target/<triple>/debug` cross-compilation directory.
  for debug_dir in "${target_dir}/debug" "${target_dir}"/*/debug; do
    if [[ -d "${debug_dir}" ]]; then
      sweep_debug_dir "${debug_dir}"
    fi
  done
  enforce_cap "${manifest}" "${target_dir}/debug"
done

df -k "${HZR_REPOSITORY_ROOT}" | awk 'NR == 2 { printf "free disk space: %.1f GB\n", $4 / 1024 / 1024 }'
