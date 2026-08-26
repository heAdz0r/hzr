#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_ARCHIVE="${1:-}"
HZR_CHECKSUMS="${2:-}"
if [[ ! -f "${HZR_ARCHIVE}" || ! -f "${HZR_CHECKSUMS}" ]]; then
  echo "usage: scripts/smoke-install.sh /path/to/hzr.tar.gz /path/to/SHA256SUMS" >&2
  exit 2
fi

HZR_SMOKE_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-install-smoke.XXXXXX")"
# macOS exposes /var through /private/var. Match the installer's canonical paths so the
# integration assertions test stable HZR locations instead of a lexical path alias.
HZR_SMOKE_TEMP="$(cd -- "${HZR_SMOKE_TEMP}" && pwd -P)"
cleanup_hzr_install_smoke() {
  if [[ -n "${HZR_SMOKE_DAEMON_PID:-}" ]]; then
    kill "${HZR_SMOKE_DAEMON_PID}" 2>/dev/null || true
    wait "${HZR_SMOKE_DAEMON_PID}" 2>/dev/null || true
  fi
  if [[ "${HZR_KEEP_SMOKE:-0}" == 1 ]]; then
    echo "retained smoke workspace at ${HZR_SMOKE_TEMP}" >&2
  elif [[ -n "${HZR_SMOKE_TEMP:-}" && -d "${HZR_SMOKE_TEMP}" ]]; then
    rm -rf -- "${HZR_SMOKE_TEMP}"
  fi
}
trap cleanup_hzr_install_smoke EXIT

mkdir -p "${HZR_SMOKE_TEMP}/home" "${HZR_SMOKE_TEMP}/workspace" "${HZR_SMOKE_TEMP}/tools"
export HOME="${HZR_SMOKE_TEMP}/home"
# ProjectDirs follows XDG overrides on Linux. Pin every user directory to the fixture so
# a hosted runner cannot redirect config or data back into its real HOME.
export XDG_CONFIG_HOME="${HOME}/.config"
export XDG_DATA_HOME="${HOME}/.local/share"
export XDG_CACHE_HOME="${HOME}/.cache"
export XDG_STATE_HOME="${HOME}/.local/state"
HZR_SERVICE_LOG="${HZR_SMOKE_TEMP}/service-manager.log"
HZR_INSTALL_LOG="${HZR_SMOKE_TEMP}/install.log"
HZR_SERVICE_STUB="${HZR_SMOKE_TEMP}/tools/service-manager"
printf '%s\n' '#!/bin/sh' 'printf '\''%s\n'\'' "$*" >> "${HZR_SERVICE_LOG}"' 'exit 0' \
  >"${HZR_SERVICE_STUB}"
chmod 0755 "${HZR_SERVICE_STUB}"
# The host may legitimately be running unrelated user engines. Keep the clean-HOME
# acceptance fixture deterministic without stopping or hiding them in production.
HZR_PS_STUB="${HZR_SMOKE_TEMP}/tools/ps"
printf '%s\n' '#!/bin/sh' 'exit 0' >"${HZR_PS_STUB}"
chmod 0755 "${HZR_PS_STUB}"
git -C "${HZR_SMOKE_TEMP}/workspace" init --quiet
printf 'pub const SMOKE_MARKER: &str = "smoke_marker";\n' \
  >"${HZR_SMOKE_TEMP}/workspace/main.rs"
(
  cd "${HZR_SMOKE_TEMP}/workspace"
  HOME="${HZR_SMOKE_TEMP}/home" \
  HZR_ARCHIVE_PATH="${HZR_ARCHIVE}" \
  HZR_CHECKSUMS_PATH="${HZR_CHECKSUMS}" \
  HZR_INSTALL_HOOKS=1 \
  HZR_SYSTEMCTL="${HZR_SERVICE_STUB}" \
  HZR_LAUNCHCTL="${HZR_SERVICE_STUB}" \
  HZR_SERVICE_LOG="${HZR_SERVICE_LOG}" \
  PATH="/usr/bin:/bin" \
    /bin/sh "${HZR_REPOSITORY_ROOT}/install.sh"
) | tee "${HZR_INSTALL_LOG}"

for HZR_INSTALL_OUTPUT in \
  '[1/5] Using the local release archive' \
  '[2/5] Verifying the download' \
  '[3/5] Unpacking and checking the bundle contents' \
  '[4/5] Placing the files and command-line entry points' \
  '[5/5] Registering this project and starting the background service' \
  'HZR v0.6.3 is installed.' \
  'What went where' \
  'Next steps' \
  'hzr doctor --workspace .'; do
  if ! grep -F "${HZR_INSTALL_OUTPUT}" "${HZR_INSTALL_LOG}" >/dev/null; then
    echo "installer summary is missing: ${HZR_INSTALL_OUTPUT}" >&2
    exit 1
  fi
done

HZR_INSTALLED_ROOT="${HZR_SMOKE_TEMP}/home/.local/share/hzr/current"
HZR_INSTALLED_BIN="${HZR_SMOKE_TEMP}/home/.local/bin"
case "$(uname -s)" in
  Darwin) HZR_SMOKE_CONFIG="${HOME}/Library/Application Support/dev.headz0r.hzr/config.toml" ;;
  Linux) HZR_SMOKE_CONFIG="${HOME}/.config/hzr/config.toml" ;;
  *) echo "unsupported config platform" >&2; exit 1 ;;
esac
HZR_SMOKE_PORT="$("${HZR_INSTALLED_ROOT}/engines/node" -e '
  const net = require("node:net");
  const server = net.createServer();
  server.on("error", (error) => {
    console.error(error);
    process.exit(1);
  });
  server.listen(0, "127.0.0.1", () => {
    console.log(server.address().port);
    server.close();
  });
')"
if [[ ! -f "${HZR_SMOKE_CONFIG}" ]]; then
  echo "installer did not create ${HZR_SMOKE_CONFIG}" >&2
  exit 1
fi
HZR_SMOKE_CONFIG_NEXT="${HZR_SMOKE_CONFIG}.smoke-next"
while IFS= read -r HZR_CONFIG_LINE || [[ -n "${HZR_CONFIG_LINE}" ]]; do
  if [[ "${HZR_CONFIG_LINE}" == bind\ =* ]]; then
    printf 'bind = "127.0.0.1:%s"\n' "${HZR_SMOKE_PORT}"
  else
    printf '%s\n' "${HZR_CONFIG_LINE}"
  fi
done <"${HZR_SMOKE_CONFIG}" >"${HZR_SMOKE_CONFIG_NEXT}"
mv -- "${HZR_SMOKE_CONFIG_NEXT}" "${HZR_SMOKE_CONFIG}"
run_hzr() {
  HOME="${HZR_SMOKE_TEMP}/home" \
  HZR_SYSTEMCTL="${HZR_SERVICE_STUB}" \
  HZR_LAUNCHCTL="${HZR_SERVICE_STUB}" \
  HZR_SERVICE_LOG="${HZR_SERVICE_LOG}" \
  PATH="${HZR_INSTALLED_BIN}:${HZR_SMOKE_TEMP}/tools:/usr/bin:/bin" \
    "${HZR_INSTALLED_BIN}/hzr" "$@"
}
PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" hzr --version | grep -Fx "hzr 0.6.3" >/dev/null
PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" rtk --version \
  | grep -Fx "rtk 0.44.1-fork.1" >/dev/null
"${HZR_INSTALLED_ROOT}/engines/grepai" version | grep -F "0.35.0" >/dev/null
"${HZR_INSTALLED_ROOT}/engines/icm" --version | grep -F "0.10.61" >/dev/null
"${HZR_INSTALLED_ROOT}/engines/node" --version | grep -Fx "v22.17.1" >/dev/null
for HZR_VISUALIZER_FILE in index.html assets/app.css assets/app.js hzr-hero.png; do
  if [[ ! -f "${HZR_INSTALLED_ROOT}/share/hzr/visualizer/${HZR_VISUALIZER_FILE}" ]]; then
    echo "installed visualizer asset is missing: ${HZR_VISUALIZER_FILE}" >&2
    exit 1
  fi
done
if [[ ! -L "${HZR_SMOKE_TEMP}/workspace/.grepai" ]]; then
  echo "clean installer did not initialize the workspace" >&2
  exit 1
fi
if [[ ! -f "${HZR_SMOKE_TEMP}/home/.claude/settings.json" ]] || \
  ! grep -F "hooks dispatch" "${HZR_SMOKE_TEMP}/home/.claude/settings.json" >/dev/null || \
  ! grep -F "managed agent contract" "${HZR_SMOKE_TEMP}/home/.claude/CLAUDE.md" >/dev/null || \
  ! grep -F "[mcp_servers.hzr]" "${HZR_SMOKE_TEMP}/home/.codex/config.toml" >/dev/null; then
  echo "clean installer did not wire hooks, instructions, and HZR MCP ownership" >&2
  exit 1
fi

HZR_CLAUDE_SETTINGS="${HZR_SMOKE_TEMP}/home/.claude/settings.json"
HZR_CLAUDE_INSTRUCTIONS="${HZR_SMOKE_TEMP}/home/.claude/CLAUDE.md"
HZR_CODEX_INSTRUCTIONS="${HZR_SMOKE_TEMP}/home/.codex/AGENTS.md"
HZR_CODEX_CONFIG="${HZR_SMOKE_TEMP}/home/.codex/config.toml"
integration_contains_stable_path() {
  local integration_file="$1"
  local stable_path="$2"
  local macos_path_alias=""

  if grep -F "${stable_path}" "${integration_file}" >/dev/null; then
    return 0
  fi
  if [[ "$(uname -s)" != Darwin ]]; then
    return 1
  fi
  case "${stable_path}" in
    /var/*) macos_path_alias="/private${stable_path}" ;;
    /private/var/*) macos_path_alias="${stable_path#/private}" ;;
    *) return 1 ;;
  esac
  grep -F "${macos_path_alias}" "${integration_file}" >/dev/null
}
for HZR_INTEGRATION_FILE in \
  "${HZR_CLAUDE_SETTINGS}" \
  "${HZR_CLAUDE_INSTRUCTIONS}" \
  "${HZR_CODEX_INSTRUCTIONS}" \
  "${HZR_CODEX_CONFIG}"; do
  if grep -F "/versions/" "${HZR_INTEGRATION_FILE}" >/dev/null; then
    echo "integration is pinned to an immutable release: ${HZR_INTEGRATION_FILE}" >&2
    exit 1
  fi
done
if ! integration_contains_stable_path "${HZR_CLAUDE_SETTINGS}" "${HZR_INSTALLED_BIN}/hzr" || \
  ! integration_contains_stable_path "${HZR_CODEX_CONFIG}" "${HZR_INSTALLED_BIN}/hzr" || \
  ! integration_contains_stable_path "${HZR_CLAUDE_INSTRUCTIONS}" "${HZR_INSTALLED_ROOT}/share/hzr/HZR.md" || \
  ! integration_contains_stable_path "${HZR_CODEX_INSTRUCTIONS}" "${HZR_INSTALLED_ROOT}/share/hzr/HZR.md"; then
  echo "integrations do not use HZR's stable binary and contract paths" >&2
  exit 1
fi
HZR_CLAUDE_DESKTOP_CONFIG="${HZR_SMOKE_TEMP}/home/Library/Application Support/Claude/claude_desktop_config.json"
if [[ -f "${HZR_CLAUDE_DESKTOP_CONFIG}" ]] && \
  grep -F "/versions/" "${HZR_CLAUDE_DESKTOP_CONFIG}" >/dev/null; then
  echo "Claude Desktop MCP is pinned to an immutable release" >&2
  exit 1
fi

case "$(uname -s)" in
  Darwin) HZR_SERVICE_DEFINITION="${HZR_SMOKE_TEMP}/home/Library/LaunchAgents/dev.headz0r.hzr.hzrd.plist" ;;
  Linux) HZR_SERVICE_DEFINITION="${HZR_SMOKE_TEMP}/home/.config/systemd/user/dev.headz0r.hzr.hzrd.service" ;;
  *) echo "unsupported service smoke platform" >&2; exit 1 ;;
esac
if [[ ! -f "${HZR_SERVICE_DEFINITION}" ]] || \
  ! grep -F "current/bin/hzrd" "${HZR_SERVICE_DEFINITION}" >/dev/null || \
  grep -F "/versions/" "${HZR_SERVICE_DEFINITION}" >/dev/null; then
  echo "production service definition does not use the stable current/bin/hzrd" >&2
  exit 1
fi
if [[ ! -s "${HZR_SERVICE_LOG}" ]]; then
  echo "release installer did not invoke the platform service manager" >&2
  exit 1
fi

HOME="${HZR_SMOKE_TEMP}/home" \
PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" \
  "${HZR_INSTALLED_ROOT}/bin/hzrd" \
  >"${HZR_SMOKE_TEMP}/hzrd.log" 2>&1 &
HZR_SMOKE_DAEMON_PID="$!"
for _ in {1..100}; do
  if ! kill -0 "${HZR_SMOKE_DAEMON_PID}" 2>/dev/null; then
    echo "isolated daemon exited before becoming ready" >&2
    sed -n '1,120p' "${HZR_SMOKE_TEMP}/hzrd.log" >&2
    exit 1
  fi
  if run_hzr daemon status --json >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
run_hzr daemon status --json >/dev/null
"${HZR_INSTALLED_ROOT}/engines/node" -e '
  const fs = require("node:fs");
  const endpoint = process.argv[1];
  // The workspace path is still passed for parity with the CLI probes below; the dashboard
  // itself reports pseudonymous identity and is asserted on shape rather than on this path.
  void fs.realpathSync(process.argv[2]);
  Promise.all([
    fetch(`${endpoint}/`).then(async (response) => {
      const body = await response.text();
      if (response.status !== 200 || !body.includes("HZR · Local control plane")) {
        throw new Error(`installed visualizer index failed: ${response.status}`);
      }
    }),
    fetch(`${endpoint}/v1/dashboard`).then(async (response) => {
      const report = await response.json();
      // The dashboard reports project identity as a keyed pseudonym, not a filesystem path, so
      // matching the literal workspace here asserted the absence of a privacy guarantee the
      // control plane is supposed to provide. Assert the shape instead: a registered project
      // whose root is redacted, and the fork engine present.
      const pseudonymous = (project) =>
        typeof project.root === "string" && project.root.startsWith("hmac-sha256:");
      if (response.status !== 200 ||
          report.projects.length === 0 ||
          !report.projects.every(pseudonymous) ||
          !report.services.some((service) => service.id === "rtk")) {
        throw new Error(`installed dashboard contract failed: ${JSON.stringify(report)}`);
      }
    }),
  ]).catch((error) => {
    console.error(error);
    process.exit(1);
  });
' "http://127.0.0.1:${HZR_SMOKE_PORT}" "${HZR_SMOKE_TEMP}/workspace"
run_hzr daemon service restart --json >/dev/null
run_hzr daemon service status --json >/dev/null
HZR_MEMORY_READY=0
for _ in {1..1200}; do
  if run_hzr daemon status --json 2>/dev/null \
    | "${HZR_INSTALLED_ROOT}/engines/node" -e '
        const fs = require("node:fs");
        const health = JSON.parse(fs.readFileSync(0, "utf8"));
        const icm = health.engines.find((engine) => engine.name === "icm");
        process.exit(icm && String(icm.detail).includes("singleton is ready") ? 0 : 1);
      ' >/dev/null 2>&1; then
    HZR_MEMORY_READY=1
    break
  fi
  if ! kill -0 "${HZR_SMOKE_DAEMON_PID}" 2>/dev/null; then
    echo "isolated daemon exited while ICM was starting" >&2
    sed -n '1,120p' "${HZR_SMOKE_TEMP}/hzrd.log" >&2
    exit 1
  fi
  sleep 0.1
done
if [[ "${HZR_MEMORY_READY}" != 1 ]]; then
  echo "isolated ICM did not become ready" >&2
  run_hzr daemon status --json >&2 || true
  exit 1
fi
run_hzr search smoke_marker --mode exact \
  --workspace "${HZR_SMOKE_TEMP}/workspace" --json >/dev/null
run_hzr memory store smoke installed-bundle-memory \
  --workspace "${HZR_SMOKE_TEMP}/workspace" --json >/dev/null
run_hzr memory recall installed-bundle-memory \
  --workspace "${HZR_SMOKE_TEMP}/workspace" --json >/dev/null
run_hzr stats --json >/dev/null
run_hzr mcp --help >/dev/null
printf '%s\n' \
  '{"tool_name":"Bash","tool_input":{"command":"cat main.rs"}}' \
  | HOME="${HZR_SMOKE_TEMP}/home" \
    PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" \
    "${HZR_INSTALLED_BIN}/hzr" hooks dispatch \
    >"${HZR_SMOKE_TEMP}/hook.json"
grep -F "hookSpecificOutput" "${HZR_SMOKE_TEMP}/hook.json" >/dev/null
run_hzr doctor --workspace "${HZR_SMOKE_TEMP}/workspace" --json \
  >"${HZR_SMOKE_TEMP}/doctor.json"
grep -F '"bundle_node"' "${HZR_SMOKE_TEMP}/doctor.json" >/dev/null
grep -F '"daemon_service"' "${HZR_SMOKE_TEMP}/doctor.json" >/dev/null

# Project-only adoption must be a real activation boundary, not merely a project-scoped index.
mkdir -p "${HZR_SMOKE_TEMP}/baseline"
(
  cd "${HZR_SMOKE_TEMP}/workspace"
  run_hzr install --project-only --force --skip-service --json >/dev/null
)
if ! grep -F "init --if-enabled --quiet --session-start-hook" "${HZR_CLAUDE_SETTINGS}" >/dev/null || \
  grep -F "managed agent contract" "${HZR_CLAUDE_INSTRUCTIONS}" >/dev/null || \
  grep -F "[mcp_servers.hzr]" "${HZR_CODEX_CONFIG}" >/dev/null || \
  ! grep -F "managed agent contract" "${HZR_SMOKE_TEMP}/workspace/CLAUDE.md" >/dev/null || \
  ! grep -F "managed agent contract" "${HZR_SMOKE_TEMP}/workspace/AGENTS.md" >/dev/null; then
  echo "project-only adoption did not localize instructions and MCP ownership" >&2
  exit 1
fi
printf '%s\n' \
  '{"tool_name":"Bash","tool_input":{"command":"cat baseline.txt"}}' \
  | (
      cd "${HZR_SMOKE_TEMP}/baseline"
      HOME="${HZR_SMOKE_TEMP}/home" \
        PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" \
        "${HZR_INSTALLED_BIN}/hzr" hooks dispatch
    ) >"${HZR_SMOKE_TEMP}/baseline-hook.json"
if [[ -s "${HZR_SMOKE_TEMP}/baseline-hook.json" ]]; then
  echo "project-only hook emitted output in a baseline workspace" >&2
  exit 1
fi
run_hzr stats --workspace "${HZR_SMOKE_TEMP}/workspace" --json \
  | grep -F '"scope": "project ' >/dev/null

# Switching back to the default restores the user-global surfaces and keeps the release-smoke
# stages below on the installation mode they historically verify.
(
  cd "${HZR_SMOKE_TEMP}/workspace"
  run_hzr install --force --skip-service --json >/dev/null
)
if [[ ! -L "${HZR_INSTALLED_BIN}/hzr" ]] || \
  [[ "$(readlink "${HZR_INSTALLED_BIN}/hzr")" != *"/current/bin/hzr" ]]; then
  echo "hzr install --force replaced the public current-based symlink" >&2
  exit 1
fi
if ! grep -F "init --if-needed --quiet --session-start-hook" "${HZR_CLAUDE_SETTINGS}" >/dev/null || \
  ! grep -F "managed agent contract" "${HZR_CLAUDE_INSTRUCTIONS}" >/dev/null || \
  ! grep -F "[mcp_servers.hzr]" "${HZR_CODEX_CONFIG}" >/dev/null; then
  echo "all-project adoption was not restored after the project-only smoke" >&2
  exit 1
fi

echo "HZR clean-install smoke passed without external Node.js, RTK, grepai, or ICM"

# Same-version reuse is allowed only after re-attesting the existing root against the
# freshly verified archive manifest. A clean root is a byte-for-byte idempotent install.
HZR_CURRENT_BEFORE="$(readlink "${HZR_SMOKE_TEMP}/home/.local/share/hzr/current")"
(
  cd "${HZR_SMOKE_TEMP}/workspace"
  HOME="${HZR_SMOKE_TEMP}/home" \
  HZR_ARCHIVE_PATH="${HZR_ARCHIVE}" \
  HZR_CHECKSUMS_PATH="${HZR_CHECKSUMS}" \
  HZR_INSTALL_HOOKS=1 \
  HZR_SYSTEMCTL="${HZR_SERVICE_STUB}" \
  HZR_LAUNCHCTL="${HZR_SERVICE_STUB}" \
  HZR_SERVICE_LOG="${HZR_SERVICE_LOG}" \
  PATH="/usr/bin:/bin" \
    /bin/sh "${HZR_REPOSITORY_ROOT}/install.sh"
)
if [[ "$(readlink "${HZR_SMOKE_TEMP}/home/.local/share/hzr/current")" != "${HZR_CURRENT_BEFORE}" ]]; then
  echo "same-version re-install unexpectedly changed current" >&2
  exit 1
fi

assert_reinstall_rejects() {
  local HZR_CASE="$1"
  local HZR_CASE_HOME="${HZR_SMOKE_TEMP}/${HZR_CASE}-home"
  local HZR_CASE_WORKSPACE="${HZR_SMOKE_TEMP}/${HZR_CASE}-workspace"
  mkdir -p "${HZR_CASE_HOME}" "${HZR_CASE_WORKSPACE}"
  git -C "${HZR_CASE_WORKSPACE}" init --quiet
  (
    cd "${HZR_CASE_WORKSPACE}"
    HOME="${HZR_CASE_HOME}" \
    HZR_ARCHIVE_PATH="${HZR_ARCHIVE}" \
    HZR_CHECKSUMS_PATH="${HZR_CHECKSUMS}" \
    HZR_INSTALL_HOOKS=0 \
    HZR_INSTALL_SERVICE=0 \
    PATH="/usr/bin:/bin" \
      /bin/sh "${HZR_REPOSITORY_ROOT}/install.sh" >/dev/null
  )
  local HZR_CASE_CURRENT="${HZR_CASE_HOME}/.local/share/hzr/current"
  local HZR_CASE_ROOT
  HZR_CASE_ROOT="$(readlink "${HZR_CASE_CURRENT}")"
  rm -f -- "${HZR_CASE_CURRENT}"
  case "${HZR_CASE}" in
    tampered) printf 'tampered\n' >>"${HZR_CASE_ROOT}/share/hzr/HZR.md" ;;
    missing) rm -f -- "${HZR_CASE_ROOT}/engines/icm" ;;
    symlink) rm -f -- "${HZR_CASE_ROOT}/engines/icm"; ln -s /bin/true "${HZR_CASE_ROOT}/engines/icm" ;;
  esac
  if (
    cd "${HZR_CASE_WORKSPACE}"
    HOME="${HZR_CASE_HOME}" \
    HZR_ARCHIVE_PATH="${HZR_ARCHIVE}" \
    HZR_CHECKSUMS_PATH="${HZR_CHECKSUMS}" \
    HZR_INSTALL_HOOKS=0 \
    HZR_INSTALL_SERVICE=0 \
    PATH="/usr/bin:/bin" \
      /bin/sh "${HZR_REPOSITORY_ROOT}/install.sh" >/dev/null 2>&1
  ); then
    echo "same-version ${HZR_CASE} root was accepted" >&2
    exit 1
  fi
  if [[ -e "${HZR_CASE_CURRENT}" || -L "${HZR_CASE_CURRENT}" ]]; then
    echo "rejected ${HZR_CASE} root became current" >&2
    exit 1
  fi
}

assert_reinstall_rejects tampered
assert_reinstall_rejects missing
assert_reinstall_rejects symlink
echo "HZR same-version re-attestation rejects tampered, missing, and symlink-injected roots"

# ---------------------------------------------------------------------------
# Upgrade smoke.
#
# A clean install cannot catch the two upgrade regressions that shipped before:
#   1. `mv -f` followed the existing `current` symlink, so the temp link landed
#      *inside* the old release and `current` never moved.
#   2. The config persisted the canonicalized `versions/<release>/engines`, so a new
#      `hzr` kept launching the previous RTK/grepai/ICM/Node.
# Both are invisible to `hzr --version` alone, because the freshly installed binary is
# reached through ~/.local/bin. So this stage reinstalls over the first install and then
# asserts every engine resolves through `current`.
# ---------------------------------------------------------------------------

HZR_HOME="${HZR_SMOKE_TEMP}/home"
HZR_CURRENT_LINK="${HZR_HOME}/.local/share/hzr/current"
if [[ ! -L "${HZR_CURRENT_LINK}" ]]; then
  echo "install did not create a ${HZR_CURRENT_LINK} symlink" >&2
  exit 1
fi
HZR_FIRST_TARGET="$(readlink "${HZR_CURRENT_LINK}")"

# Install the verified archive again under a synthetic later version. This executes
# the shipped installer's real replacement helper end-to-end; extracting its shell
# function with `sed | source` is not portable across BSD and GNU userlands.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) HZR_SMOKE_PLATFORM="darwin-arm64" ;;
  Darwin-x86_64) HZR_SMOKE_PLATFORM="darwin-x64" ;;
  Linux-aarch64 | Linux-arm64) HZR_SMOKE_PLATFORM="linux-arm64" ;;
  Linux-x86_64) HZR_SMOKE_PLATFORM="linux-x64" ;;
  *) echo "unsupported upgrade-smoke platform" >&2; exit 1 ;;
esac
HZR_UPGRADE_VERSION="0.6.3-upgrade-smoke"
HZR_UPGRADE_ARTIFACT="hzr-v${HZR_UPGRADE_VERSION}-${HZR_SMOKE_PLATFORM}.tar.gz"
HZR_UPGRADE_CHECKSUMS="${HZR_SMOKE_TEMP}/SHA256SUMS.upgrade"
awk -v artifact="${HZR_UPGRADE_ARTIFACT}" \
  'NF >= 2 { print $1 "  " artifact; exit }' \
  "${HZR_CHECKSUMS}" >"${HZR_UPGRADE_CHECKSUMS}"
(
  cd "${HZR_SMOKE_TEMP}/workspace"
  HOME="${HZR_HOME}" \
  HZR_VERSION="${HZR_UPGRADE_VERSION}" \
  HZR_ARCHIVE_PATH="${HZR_ARCHIVE}" \
  HZR_CHECKSUMS_PATH="${HZR_UPGRADE_CHECKSUMS}" \
  HZR_INSTALL_HOOKS=0 \
  HZR_INSTALL_SERVICE=0 \
  PATH="/usr/bin:/bin" \
    /bin/sh "${HZR_REPOSITORY_ROOT}/install.sh" >/dev/null
)
HZR_SECOND_RELEASE="${HZR_HOME}/.local/share/hzr/versions/v${HZR_UPGRADE_VERSION}-${HZR_SMOKE_PLATFORM}"

HZR_SECOND_TARGET="$(readlink "${HZR_CURRENT_LINK}")"
if [[ "${HZR_SECOND_TARGET}" == "${HZR_FIRST_TARGET}" ]]; then
  echo "upgrade did not repoint current: still ${HZR_SECOND_TARGET}" >&2
  exit 1
fi
# The whole point: every engine must now come from the NEW release. Checking
# `hzr --version` alone would pass even when all four engines are stale.
HZR_RESOLVED_ENGINES="$(cd -- "${HZR_CURRENT_LINK}/engines" && pwd -P)"
HZR_EXPECTED_ENGINES="$(cd -- "${HZR_SECOND_RELEASE}/engines" && pwd -P)"
if [[ "${HZR_RESOLVED_ENGINES}" != "${HZR_EXPECTED_ENGINES}" ]]; then
  echo "engines still resolve to ${HZR_RESOLVED_ENGINES}, expected ${HZR_EXPECTED_ENGINES}" >&2
  exit 1
fi

PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" hzr --version | grep -Fx "hzr 0.6.3" >/dev/null
PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" rtk --version \
  | grep -Fx "rtk 0.44.1-fork.1" >/dev/null
"${HZR_CURRENT_LINK}/engines/rtk" --version | grep -Fx "rtk 0.44.1-fork.1" >/dev/null
"${HZR_CURRENT_LINK}/engines/grepai" version | grep -F "0.35.0" >/dev/null
"${HZR_CURRENT_LINK}/engines/icm" --version | grep -F "0.10.61" >/dev/null
"${HZR_CURRENT_LINK}/engines/node" --version | grep -Fx "v22.17.1" >/dev/null

# The engine directory HZR actually reports must be the stable current/ path, not a
# canonicalized versions/<release> path that would freeze engines at this release.
HZR_REPORTED_ENGINES="$(
  { HOME="${HZR_HOME}" PATH="${HZR_INSTALLED_BIN}:/usr/bin:/bin" \
      hzr doctor --json 2>/dev/null || true; } \
    | tr ',' '\n' \
    | sed -n 's/.*"detail": *"\([^"]*engines[^"]*\)".*/\1/p' \
    | head -1
)"
if [[ "${HZR_REPORTED_ENGINES}" == *"/versions/"* ]]; then
  echo "hzr reports a version-pinned engine path: ${HZR_REPORTED_ENGINES}" >&2
  exit 1
fi

echo "HZR upgrade smoke passed: current repointed and all four engines follow it"
