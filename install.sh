#!/bin/sh
set -eu

HZR_VERSION="${HZR_VERSION:-0.3.8}"
HZR_REPOSITORY="${HZR_REPOSITORY:-heAdz0r/hzr}"
HZR_INSTALL_ROOT="${HZR_INSTALL_ROOT:-${HOME}/.local/share/hzr}"
HZR_BIN_DIR="${HZR_BIN_DIR:-${HOME}/.local/bin}"
HZR_INSTALL_HOOKS="${HZR_INSTALL_HOOKS:-1}"
HZR_INSTALL_SERVICE="${HZR_INSTALL_SERVICE:-1}"
HZR_PROJECT_ONLY="${HZR_PROJECT_ONLY:-0}"
HZR_FORCE="${HZR_FORCE:-0}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) HZR_PLATFORM="darwin-arm64" ;;
  Darwin-x86_64) HZR_PLATFORM="darwin-x64" ;;
  Linux-aarch64 | Linux-arm64) HZR_PLATFORM="linux-arm64" ;;
  Linux-x86_64) HZR_PLATFORM="linux-x64" ;;
  *)
    echo "hzr: unsupported platform $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

HZR_ARTIFACT="hzr-v${HZR_VERSION}-${HZR_PLATFORM}.tar.gz"
HZR_RELEASE_URL="https://github.com/${HZR_REPOSITORY}/releases/download/v${HZR_VERSION}"
HZR_INSTALL_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-install.XXXXXX")"

# Progress output. A release bundle is a few hundred megabytes, so a silent
# installer looks indistinguishable from a hung one. Steps are numbered and the
# closing summary states what exists on disk and what to run next. Colour is
# emitted only for an interactive terminal so logs and CI stay plain.
if [ -t 1 ]; then
  HZR_BOLD="$(printf '\033[1m')"
  HZR_DIM="$(printf '\033[2m')"
  HZR_GREEN="$(printf '\033[32m')"
  HZR_RESET="$(printf '\033[0m')"
else
  HZR_BOLD=""
  HZR_DIM=""
  HZR_GREEN=""
  HZR_RESET=""
fi
HZR_STEP_TOTAL=5
HZR_STEP_NUMBER=0

hzr_step() {
  HZR_STEP_NUMBER=$((HZR_STEP_NUMBER + 1))
  printf '%s[%d/%d]%s %s\n' \
    "${HZR_DIM}" "${HZR_STEP_NUMBER}" "${HZR_STEP_TOTAL}" "${HZR_RESET}" "$1"
}

hzr_note() {
  printf '      %s%s%s\n' "${HZR_DIM}" "$1" "${HZR_RESET}"
}

cleanup_hzr_install() {
  if [ -n "${HZR_INSTALL_TEMP:-}" ] && [ -d "${HZR_INSTALL_TEMP}" ]; then
    rm -rf -- "${HZR_INSTALL_TEMP}"
  fi
  # The pending `current` symlink is created outside the temp directory, so a
  # failure between its creation and the atomic swap would otherwise leave a
  # dangling .current-<pid> entry in the install root.
  if [ -n "${HZR_CURRENT_TEMP:-}" ] && [ -L "${HZR_CURRENT_TEMP}" ]; then
    rm -f -- "${HZR_CURRENT_TEMP}"
  fi
}
trap cleanup_hzr_install EXIT HUP INT TERM

printf '\n%sInstalling HZR v%s%s %s(%s)%s\n\n' \
  "${HZR_BOLD}" "${HZR_VERSION}" "${HZR_RESET}" \
  "${HZR_DIM}" "${HZR_PLATFORM}" "${HZR_RESET}"

# Atomically repoint a symlink without following it.
#
# A plain `mv -f new current` is WRONG when `current` already exists as a symlink to a
# directory: mv resolves it and moves `new` *inside* the old version directory, leaving
# `current` still pointing at the previous release. Upgrades then silently keep running
# the old engines. `mv` needs an explicit no-follow/no-target-directory flag, and the
# flag differs by platform: BSD/Darwin uses `-h`, GNU coreutils uses `-T`. Probe both
# rather than branching on `uname`, so busybox and other userlands work too.
replace_hzr_symlink() {
  HZR_SYMLINK_SOURCE="$1"
  HZR_SYMLINK_TARGET="$2"
  if mv -fh -- "${HZR_SYMLINK_SOURCE}" "${HZR_SYMLINK_TARGET}" 2>/dev/null; then
    return 0
  fi
  if mv -fT -- "${HZR_SYMLINK_SOURCE}" "${HZR_SYMLINK_TARGET}" 2>/dev/null; then
    return 0
  fi
  # Neither flag is available. Removing the old link first is not atomic, so it is the
  # last resort: a concurrent reader during this window sees no `current` at all, which
  # fails loudly instead of silently resolving to a stale release.
  if [ -L "${HZR_SYMLINK_TARGET}" ] || [ ! -d "${HZR_SYMLINK_TARGET}" ]; then
    rm -f -- "${HZR_SYMLINK_TARGET}"
    mv -f -- "${HZR_SYMLINK_SOURCE}" "${HZR_SYMLINK_TARGET}"
    return 0
  fi
  echo "hzr: refusing to replace real directory ${HZR_SYMLINK_TARGET}" >&2
  exit 1
}

# A bundle download takes minutes on a slow link, so the large transfer shows a
# progress bar on an interactive terminal. `HZR_SHOW_PROGRESS=1` opts in; the
# small checksum manifest stays quiet.
download_hzr_file() {
  HZR_DOWNLOAD_URL="$1"
  HZR_DOWNLOAD_DESTINATION="$2"
  HZR_SHOW_PROGRESS="${3:-0}"
  if command -v curl >/dev/null 2>&1; then
    if [ "${HZR_SHOW_PROGRESS}" = "1" ]; then
      curl --fail --progress-bar --show-error --location --max-time 1800 --proto '=https' --tlsv1.2 \
        "${HZR_DOWNLOAD_URL}" --output "${HZR_DOWNLOAD_DESTINATION}"
    else
      curl --fail --silent --show-error --location --max-time 1800 --proto '=https' --tlsv1.2 \
        "${HZR_DOWNLOAD_URL}" --output "${HZR_DOWNLOAD_DESTINATION}"
    fi
  elif command -v wget >/dev/null 2>&1; then
    if [ "${HZR_SHOW_PROGRESS}" = "1" ]; then
      wget --https-only --quiet --show-progress \
        --output-document="${HZR_DOWNLOAD_DESTINATION}" "${HZR_DOWNLOAD_URL}"
    else
      wget --https-only --quiet --output-document="${HZR_DOWNLOAD_DESTINATION}" \
        "${HZR_DOWNLOAD_URL}"
    fi
  else
    echo "hzr: curl or wget is required to download the release bundle" >&2
    exit 1
  fi
}

verify_hzr_sha256() {
  HZR_EXPECTED_SHA256="$1"
  HZR_FILE="$2"
  if command -v shasum >/dev/null 2>&1; then
    HZR_ACTUAL_SHA256="$(shasum -a 256 "${HZR_FILE}" | awk '{print $1}')"
  elif command -v sha256sum >/dev/null 2>&1; then
    HZR_ACTUAL_SHA256="$(sha256sum "${HZR_FILE}" | awk '{print $1}')"
  else
    echo "hzr: shasum or sha256sum is required to verify the release bundle" >&2
    exit 1
  fi
  if [ "${HZR_ACTUAL_SHA256}" != "${HZR_EXPECTED_SHA256}" ]; then
    echo "hzr: checksum mismatch for ${HZR_ARTIFACT}" >&2
    exit 1
  fi
}

verify_hzr_bundle_root() {
  HZR_ROOT="$1"
  if [ -L "${HZR_ROOT}" ] || [ ! -d "${HZR_ROOT}" ]; then
    echo "hzr: bundle root must be a real directory: ${HZR_ROOT}" >&2
    exit 1
  fi
  for HZR_REGULAR_PATH in \
    "bin/hzr" \
    "bin/hzrd" \
    "engines/rtk" \
    "engines/grepai" \
    "engines/icm" \
    "runtime/node/bin/node" \
    "share/hzr/HZR.md" \
    "share/hzr/install.sh" \
    "share/hzr/visualizer/index.html" \
    "share/hzr/visualizer/assets/app.css" \
    "share/hzr/visualizer/assets/app.js" \
    "share/hzr/visualizer/hzr-hero.png" \
    "share/hzr/BUNDLE_MANIFEST.sha256"; do
    if [ ! -f "${HZR_ROOT}/${HZR_REGULAR_PATH}" ] || [ -L "${HZR_ROOT}/${HZR_REGULAR_PATH}" ]; then
      echo "hzr: release bundle has an invalid regular file: ${HZR_REGULAR_PATH}" >&2
      exit 1
    fi
  done
  for HZR_EXECUTABLE_PATH in \
    "bin/hzr" \
    "bin/hzrd" \
    "engines/rtk" \
    "engines/grepai" \
    "engines/icm" \
    "runtime/node/bin/node"; do
    if [ ! -x "${HZR_ROOT}/${HZR_EXECUTABLE_PATH}" ]; then
      echo "hzr: release bundle executable has the wrong mode: ${HZR_EXECUTABLE_PATH}" >&2
      exit 1
    fi
  done
  if [ ! -L "${HZR_ROOT}/bin/rtk" ] || [ "$(readlink "${HZR_ROOT}/bin/rtk")" != "hzr" ]; then
    echo "hzr: bin/rtk must be the compatibility link to hzr" >&2
    exit 1
  fi
  if [ ! -L "${HZR_ROOT}/engines/node" ] || \
    [ "$(readlink "${HZR_ROOT}/engines/node")" != "../runtime/node/bin/node" ]; then
    echo "hzr: engines/node must point to the bundled Node.js runtime" >&2
    exit 1
  fi
  if command -v shasum >/dev/null 2>&1; then
    (cd "${HZR_ROOT}" && shasum -a 256 -c share/hzr/BUNDLE_MANIFEST.sha256 >/dev/null)
  else
    (cd "${HZR_ROOT}" && sha256sum -c share/hzr/BUNDLE_MANIFEST.sha256 >/dev/null)
  fi
}

HZR_ARCHIVE="${HZR_INSTALL_TEMP}/${HZR_ARTIFACT}"
HZR_CHECKSUMS="${HZR_INSTALL_TEMP}/SHA256SUMS"
if [ -n "${HZR_ARCHIVE_PATH:-}" ]; then
  hzr_step "Using the local release archive"
  hzr_note "${HZR_ARCHIVE_PATH}"
  cp -- "${HZR_ARCHIVE_PATH}" "${HZR_ARCHIVE}"
  cp -- "${HZR_CHECKSUMS_PATH:?HZR_CHECKSUMS_PATH is required with HZR_ARCHIVE_PATH}" \
    "${HZR_CHECKSUMS}"
else
  hzr_step "Downloading ${HZR_ARTIFACT}"
  hzr_note "a few hundred megabytes - the whole runtime ships in one bundle"
  download_hzr_file "${HZR_RELEASE_URL}/${HZR_ARTIFACT}" "${HZR_ARCHIVE}" 1
  download_hzr_file "${HZR_RELEASE_URL}/SHA256SUMS" "${HZR_CHECKSUMS}"
fi

hzr_step "Verifying the download"
HZR_EXPECTED_SHA256="$(awk -v artifact="${HZR_ARTIFACT}" '$2 == artifact { print $1 }' "${HZR_CHECKSUMS}")"
if [ -z "${HZR_EXPECTED_SHA256}" ]; then
  echo "hzr: ${HZR_ARTIFACT} is absent from the release checksum manifest" >&2
  exit 1
fi
verify_hzr_sha256 "${HZR_EXPECTED_SHA256}" "${HZR_ARCHIVE}"
hzr_note "checksum matches the published SHA256SUMS"

hzr_step "Unpacking and checking the bundle contents"
mkdir -p "${HZR_INSTALL_TEMP}/extract"
tar -xzf "${HZR_ARCHIVE}" -C "${HZR_INSTALL_TEMP}/extract"
HZR_EXTRACTED="${HZR_INSTALL_TEMP}/extract/hzr"
verify_hzr_bundle_root "${HZR_EXTRACTED}"
hzr_note "every file matches the internal bundle manifest"

HZR_VERSION_ROOT="${HZR_INSTALL_ROOT}/versions/v${HZR_VERSION}-${HZR_PLATFORM}"
mkdir -p "${HZR_INSTALL_ROOT}/versions" "${HZR_BIN_DIR}"
if [ -e "${HZR_VERSION_ROOT}" ] || [ -L "${HZR_VERSION_ROOT}" ]; then
  if [ -L "${HZR_VERSION_ROOT}" ] || [ ! -d "${HZR_VERSION_ROOT}" ]; then
    echo "hzr: existing version root is not a real directory: ${HZR_VERSION_ROOT}" >&2
    exit 1
  fi
  if ! cmp -s \
    "${HZR_EXTRACTED}/share/hzr/BUNDLE_MANIFEST.sha256" \
    "${HZR_VERSION_ROOT}/share/hzr/BUNDLE_MANIFEST.sha256"; then
    echo "hzr: existing version root does not match the verified release manifest" >&2
    exit 1
  fi
  verify_hzr_bundle_root "${HZR_VERSION_ROOT}"
else
  mv -- "${HZR_EXTRACTED}" "${HZR_VERSION_ROOT}"
  verify_hzr_bundle_root "${HZR_VERSION_ROOT}"
fi

HZR_CURRENT_TEMP="${HZR_INSTALL_ROOT}/.current-${$}"
ln -s "${HZR_VERSION_ROOT}" "${HZR_CURRENT_TEMP}"
replace_hzr_symlink "${HZR_CURRENT_TEMP}" "${HZR_INSTALL_ROOT}/current"

install_hzr_link() {
  HZR_LINK_NAME="$1"
  HZR_LINK_TARGET="${HZR_INSTALL_ROOT}/current/bin/${HZR_LINK_NAME}"
  HZR_LINK_PATH="${HZR_BIN_DIR}/${HZR_LINK_NAME}"
  if [ -e "${HZR_LINK_PATH}" ] && [ ! -L "${HZR_LINK_PATH}" ] && [ "${HZR_FORCE}" != "1" ]; then
    echo "hzr: refusing to replace non-symlink ${HZR_LINK_PATH}; set HZR_FORCE=1 to replace it" >&2
    exit 1
  fi
  HZR_LINK_TEMP="${HZR_BIN_DIR}/.${HZR_LINK_NAME}-${$}"
  ln -s "${HZR_LINK_TARGET}" "${HZR_LINK_TEMP}"
  replace_hzr_symlink "${HZR_LINK_TEMP}" "${HZR_LINK_PATH}"
}

hzr_step "Placing the files and command-line entry points"
install_hzr_link hzr
install_hzr_link hzrd
install_hzr_link rtk
hzr_note "${HZR_VERSION_ROOT}"
hzr_note "hzr, hzrd, rtk -> ${HZR_BIN_DIR}"

hzr_step "Registering this project and starting the background service"
hzr_note "initializing the current workspace registry"
"${HZR_INSTALL_ROOT}/current/bin/hzr" init --if-needed --quiet --skip-service
if [ "${HZR_INSTALL_HOOKS}" = "1" ]; then
  hzr_note "installing agent hooks and instructions"
  if [ "${HZR_PROJECT_ONLY}" = "1" ]; then
    if [ "${HZR_INSTALL_SERVICE}" = "1" ]; then
      "${HZR_INSTALL_ROOT}/current/bin/hzr" install --force --project-only
    else
      "${HZR_INSTALL_ROOT}/current/bin/hzr" install --force --project-only --skip-service
    fi
  else
    if [ "${HZR_INSTALL_SERVICE}" = "1" ]; then
      "${HZR_INSTALL_ROOT}/current/bin/hzr" install --force
    else
      "${HZR_INSTALL_ROOT}/current/bin/hzr" install --force --skip-service
    fi
  fi
fi
if [ "${HZR_INSTALL_SERVICE}" = "1" ]; then
  # Reinstalling the definition restarts an already-running daemon after `current`
  # changes, while also starting it on a first install. This keeps the live UI and
  # API on the exact bundle that was just verified above.
  hzr_note "installing or restarting the background daemon service"
  "${HZR_INSTALL_ROOT}/current/bin/hzr" daemon service install
fi

# ---------------------------------------------------------------------------
# Closing summary.
#
# Everything below is reporting only: what now exists on disk, whether the
# commands are reachable, and the next thing to run. Someone who has never seen
# HZR should be able to act on this without opening the README.
# ---------------------------------------------------------------------------

case ":${PATH}:" in
  *":${HZR_BIN_DIR}:"*) HZR_BIN_DIR_ON_PATH=1 ;;
  *) HZR_BIN_DIR_ON_PATH=0 ;;
esac

case "${SHELL:-}" in
  */zsh)
    HZR_SHELL_PROFILE="${HOME}/.zshrc"
    HZR_SHELL_RELOAD="exec zsh -l"
    ;;
  */bash)
    HZR_SHELL_PROFILE="${HOME}/.bashrc"
    HZR_SHELL_RELOAD="exec bash -l"
    ;;
  *)
    HZR_SHELL_PROFILE="${HOME}/.profile"
    HZR_SHELL_RELOAD="open a new terminal"
    ;;
esac

printf '\n%s%sHZR v%s is installed.%s\n\n' \
  "${HZR_GREEN}" "${HZR_BOLD}" "${HZR_VERSION}" "${HZR_RESET}"

printf '%sWhat went where%s\n' "${HZR_BOLD}" "${HZR_RESET}"
printf '  Program files    %s\n' "${HZR_VERSION_ROOT}"
printf '  Active version   %s -> the directory above\n' "${HZR_INSTALL_ROOT}/current"
printf '  Commands         %s/{hzr,hzrd,rtk}\n' "${HZR_BIN_DIR}"
printf '  Data and memory  %s\n' "${HZR_INSTALL_ROOT}"

if [ "${HZR_INSTALL_HOOKS}" = "1" ]; then
  printf '\n%sAgent integration%s\n' "${HZR_BOLD}" "${HZR_RESET}"
  if [ "${HZR_PROJECT_ONLY}" = "1" ]; then
    printf '  Project-only: hooks are no-ops outside %s.\n' "$(pwd -P)"
    printf '  HZR blocks are local to this project; global HZR MCP registrations were removed.\n'
  else
    printf '  Claude Code hooks and the global HZR blocks in CLAUDE.md / AGENTS.md are configured.\n'
  fi
  printf '  Existing files were backed up before being changed.\n'
fi

if [ "${HZR_INSTALL_SERVICE}" = "1" ]; then
  printf '\n%sBackground service%s\n' "${HZR_BOLD}" "${HZR_RESET}"
  printf '  hzrd runs as your own user service and listens only on this machine.\n'
  printf '  Dashboard        http://127.0.0.1:47391/\n'
  printf '  Stop or check    hzr daemon service stop | status\n'
fi

printf '\n%sNext steps%s\n' "${HZR_BOLD}" "${HZR_RESET}"
HZR_NEXT_STEP=0
if [ "${HZR_BIN_DIR_ON_PATH}" != "1" ]; then
  HZR_NEXT_STEP=$((HZR_NEXT_STEP + 1))
  printf '  %d. Make the commands reachable. %s is not on your PATH yet:\n' \
    "${HZR_NEXT_STEP}" "${HZR_BIN_DIR}"
  printf '\n       echo '\''export PATH="%s:$PATH"'\'' >> %s\n' \
    "${HZR_BIN_DIR}" "${HZR_SHELL_PROFILE}"
  printf '       %s\n\n' "${HZR_SHELL_RELOAD}"
fi
HZR_NEXT_STEP=$((HZR_NEXT_STEP + 1))
printf '  %d. Confirm the install is healthy:\n' "${HZR_NEXT_STEP}"
printf '\n       hzr doctor --workspace .\n\n'
HZR_NEXT_STEP=$((HZR_NEXT_STEP + 1))
printf '  %d. Open a project and use it. From inside any repository:\n' "${HZR_NEXT_STEP}"
printf '\n       hzr search "where is the request timeout set"\n'
printf '       hzr stats\n\n'

printf '%sHelp%s\n' "${HZR_BOLD}" "${HZR_RESET}"
printf '  Command list     hzr --help\n'
printf '  Documentation    https://github.com/%s\n' "${HZR_REPOSITORY}"
printf '  Remove HZR       hzr uninstall --force, then rm -rf %s and the commands above\n\n' \
  "${HZR_INSTALL_ROOT}"
