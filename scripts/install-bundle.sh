#!/usr/bin/env bash
# Place an already-built bundle into a version-scoped root and switch `current`.
#
# This is the second half of `install.sh`, factored out so `hzr build` performs the exact
# same placement and symlink switch as a release install. Two implementations of this step
# would drift, and the step that drifted before was precisely the `current` switch.
set -euo pipefail

HZR_BUNDLE="${1:-}"
HZR_INSTALL_ROOT="${2:-${HOME}/.local/share/hzr}"
HZR_BIN_DIR="${HZR_BIN_DIR:-${HOME}/.local/bin}"

if [[ ! -d "${HZR_BUNDLE}" ]]; then
  echo "usage: scripts/install-bundle.sh <bundle-dir> [install-root]" >&2
  exit 2
fi
for required in \
  bin/hzr \
  bin/hzrd \
  engines/rtk \
  engines/grepai \
  engines/icm \
  engines/node \
  share/hzr/BUNDLE_MANIFEST.sha256 \
  share/hzr/visualizer/index.html \
  share/hzr/visualizer/assets/app.css \
  share/hzr/visualizer/assets/app.js \
  share/hzr/visualizer/hzr-hero.png; do
  if [[ ! -e "${HZR_BUNDLE}/${required}" ]]; then
    echo "bundle is incomplete: ${required} is missing" >&2
    exit 1
  fi
done

(
  cd "${HZR_BUNDLE}"
  shasum -a 256 -c share/hzr/BUNDLE_MANIFEST.sha256 >/dev/null
)

HZR_VERSION="$("${HZR_BUNDLE}/bin/hzr" --version | awk '{print $2}')"
if [[ -z "${HZR_VERSION}" ]]; then
  echo "cannot determine the bundle version" >&2
  exit 1
fi
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) HZR_PLATFORM="darwin-arm64" ;;
  Linux-aarch64 | Linux-arm64) HZR_PLATFORM="linux-arm64" ;;
  Linux-x86_64) HZR_PLATFORM="linux-x64" ;;
  *)
    echo "unsupported platform $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

# Reuse the shipped no-follow symlink replacement rather than re-deriving it. A plain
# `mv -f` resolves an existing `current` and moves the new link *inside* the old release,
# leaving `current` pointing at the previous version.
# shellcheck source=/dev/null
source <(sed -n \
  -e '/^replace_hzr_symlink()/,/^}/p' \
  -e '/^prune_hzr_versions()/,/^}/p' \
  "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)/install.sh")

HZR_VERSION_ROOT="${HZR_INSTALL_ROOT}/versions/v${HZR_VERSION}-${HZR_PLATFORM}"
mkdir -p "${HZR_INSTALL_ROOT}/versions" "${HZR_BIN_DIR}"

# Same-version rebuild is the common development case, so replace the root rather than
# refusing. The staging directory means `current` never points at a half-written tree.
HZR_STAGE="${HZR_INSTALL_ROOT}/versions/.staging-v${HZR_VERSION}-${HZR_PLATFORM}-$$"
rm -rf -- "${HZR_STAGE}"
cp -R -- "${HZR_BUNDLE}" "${HZR_STAGE}"
HZR_RETIRED=""
if [[ -e "${HZR_VERSION_ROOT}" ]]; then
  HZR_RETIRED="${HZR_VERSION_ROOT}.retired-$$"
  mv -- "${HZR_VERSION_ROOT}" "${HZR_RETIRED}"
fi
mv -- "${HZR_STAGE}" "${HZR_VERSION_ROOT}"

HZR_CURRENT_TEMP="${HZR_INSTALL_ROOT}/.current-$$"
rm -f -- "${HZR_CURRENT_TEMP}"
ln -s "${HZR_VERSION_ROOT}" "${HZR_CURRENT_TEMP}"
replace_hzr_symlink "${HZR_CURRENT_TEMP}" "${HZR_INSTALL_ROOT}/current"

# Public entry points resolve through `current`, never through a version root, so the
# next upgrade needs no changes here.
for name in hzr hzrd; do
  HZR_LINK_TEMP="${HZR_BIN_DIR}/.${name}-$$"
  rm -f -- "${HZR_LINK_TEMP}"
  ln -s "${HZR_INSTALL_ROOT}/current/bin/${name}" "${HZR_LINK_TEMP}"
  replace_hzr_symlink "${HZR_LINK_TEMP}" "${HZR_BIN_DIR}/${name}"
done

# Retire the previous root only after `current` and the public links resolve, so a failure
# above leaves the working installation intact.
if [[ -n "${HZR_RETIRED}" ]]; then
  rm -rf -- "${HZR_RETIRED}"
fi
prune_hzr_versions "${HZR_INSTALL_ROOT}/versions" "${HZR_VERSION_ROOT}"

echo "installed v${HZR_VERSION}-${HZR_PLATFORM}; current -> $(readlink "${HZR_INSTALL_ROOT}/current"); pruned ${HZR_PRUNED_VERSION_COUNT} inactive release(s)"
