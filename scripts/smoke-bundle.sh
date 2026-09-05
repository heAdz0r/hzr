#!/usr/bin/env bash
set -euo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HZR_BUNDLE_ROOT="${1:-}"
if [[ -z "${HZR_BUNDLE_ROOT}" || ! -d "${HZR_BUNDLE_ROOT}" ]]; then
  echo "usage: scripts/smoke-bundle.sh /absolute/path/to/hzr-bundle" >&2
  exit 2
fi
HZR_BUNDLE_ROOT="$(cd -- "${HZR_BUNDLE_ROOT}" && pwd -P)"
HZR_BINARY_ROOT="${HZR_BUNDLE_ROOT}/bin"
HZR_ENGINE_ROOT="${HZR_BUNDLE_ROOT}/engines"
HZR_CAVEMAN_ROOT="${HZR_ENGINE_ROOT}/caveman-code"
HZR_NODE_ROOT="${HZR_BUNDLE_ROOT}/runtime/node"
HZR_NODE_BINARY="${HZR_NODE_ROOT}/bin/node"
HZR_PROVENANCE_ROOT="${HZR_BUNDLE_ROOT}/share/hzr"
HZR_VISUALIZER_ROOT="${HZR_PROVENANCE_ROOT}/visualizer"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required smoke command: $1" >&2
    exit 1
  fi
}

for HZR_REQUIRED_COMMAND in awk git grep kill readlink shasum sleep; do
  require_command "${HZR_REQUIRED_COMMAND}"
done

for HZR_EXECUTABLE in \
  "${HZR_BINARY_ROOT}/hzr" \
  "${HZR_BINARY_ROOT}/hzrd" \
  "${HZR_ENGINE_ROOT}/grepai" \
  "${HZR_ENGINE_ROOT}/icm" \
  "${HZR_NODE_BINARY}" \
  "${HZR_ENGINE_ROOT}/rtk"; do
  if [[ ! -x "${HZR_EXECUTABLE}" ]]; then
    echo "bundle executable is missing: ${HZR_EXECUTABLE}" >&2
    exit 1
  fi
done

if [[ ! -L "${HZR_BINARY_ROOT}/rtk" || \
  "$(readlink -- "${HZR_BINARY_ROOT}/rtk")" != "hzr" ]]; then
  echo "bin/rtk must be a relative compatibility alias to bin/hzr" >&2
  exit 1
fi
if [[ -L "${HZR_ENGINE_ROOT}/rtk" ]]; then
  echo "engines/rtk must remain the private exact fork-core binary" >&2
  exit 1
fi
if [[ ! -L "${HZR_ENGINE_ROOT}/node" || \
  "$(readlink -- "${HZR_ENGINE_ROOT}/node")" != "../runtime/node/bin/node" ]]; then
  echo "engines/node must point to the bundled Node.js runtime" >&2
  exit 1
fi

for HZR_RUNTIME_FILE in \
  bridge.mjs \
  agent-capabilities.json \
  package.json \
  package-lock.json \
  verify-safe-extract.mjs \
  vendor/SHA256SUMS \
  vendor/extract-zip/index.js \
  vendor/extract-zip/index.d.ts \
  vendor/extract-zip/package.json \
  vendor/extract-zip/LICENSE; do
  if [[ ! -f "${HZR_CAVEMAN_ROOT}/${HZR_RUNTIME_FILE}" ]]; then
    echo "managed Caveman runtime file is missing: ${HZR_RUNTIME_FILE}" >&2
    exit 1
  fi
done
if [[ ! -d "${HZR_CAVEMAN_ROOT}/node_modules" ]]; then
  echo "managed Caveman production node_modules is missing" >&2
  exit 1
fi

for HZR_PROVENANCE_FILE in \
  BUNDLE_MANIFEST.sha256 \
  agent-capabilities.json \
  HZR.md \
  install.sh \
  skills/hzr-tdd/SKILL.md \
  skills/hzr-tdd/references/testing-patterns.md \
  integrations/claude-code/hzr-awareness.md \
  integrations/claude-code/hzr-awareness-codex.md \
  fork-core/CURRENT_ENGINE.toml \
  fork-core/CURRENT_ENGINE_V1.tsv \
  fork-core/CURRENT_FILES \
  fork-core/CURRENT_SHA256SUMS; do
  if [[ ! -f "${HZR_PROVENANCE_ROOT}/${HZR_PROVENANCE_FILE}" ]]; then
    echo "bundle provenance file is missing: ${HZR_PROVENANCE_FILE}" >&2
    exit 1
  fi
done

(
  cd "${HZR_BUNDLE_ROOT}"
  shasum -a 256 -c share/hzr/BUNDLE_MANIFEST.sha256 >/dev/null
)

for HZR_VISUALIZER_FILE in index.html assets/app.css assets/app.js hzr-hero.png; do
  if [[ ! -f "${HZR_VISUALIZER_ROOT}/${HZR_VISUALIZER_FILE}" ]]; then
    echo "visualizer asset is missing: ${HZR_VISUALIZER_FILE}" >&2
    exit 1
  fi
done

"${HZR_BINARY_ROOT}/hzr" tdd --json \
  | "${HZR_NODE_BINARY}" -e '
      const fs = require("node:fs");
      const contract = JSON.parse(fs.readFileSync(0, "utf8"));
      if (contract.name !== "hzr-tdd" ||
          contract.workflow !== "red_green_refactor" ||
          contract.strict !== true) {
        process.exit(1);
      }
    '

verify_sha256() {
  local HZR_EXPECTED_SHA256="$1"
  local HZR_ARTIFACT="$2"
  local HZR_ACTUAL_SHA256

  HZR_ACTUAL_SHA256="$(shasum -a 256 "${HZR_ARTIFACT}" | awk '{print $1}')"
  if [[ "${HZR_ACTUAL_SHA256}" != "${HZR_EXPECTED_SHA256}" ]]; then
    echo "bundle digest mismatch for ${HZR_ARTIFACT}" >&2
    echo "expected: ${HZR_EXPECTED_SHA256}" >&2
    echo "actual:   ${HZR_ACTUAL_SHA256}" >&2
    exit 1
  fi
}

# A file HZR ships out of its own tree needs no transcribed digest. The property worth
# checking is "the bundle carries exactly the reviewed source", and comparing the two
# directly states it without a constant that goes stale the moment the file is edited. A
# transcribed digest for an in-repo file only relocates a code review into a build failure.
#
# Keep `verify_sha256` for artifacts that come from outside this repository: there the pin is
# the only thing standing between an upstream change and a silent bundle change.
verify_matches_repository() {
  local HZR_ARTIFACT="$1"
  local HZR_SOURCE="${HZR_REPOSITORY_ROOT}/$2"

  if [[ ! -f "${HZR_SOURCE}" ]]; then
    echo "bundle provenance source is missing from the repository: ${HZR_SOURCE}" >&2
    exit 1
  fi
  verify_sha256 "$(shasum -a 256 "${HZR_SOURCE}" | awk '{print $1}')" "${HZR_ARTIFACT}"
}

verify_matches_repository \
  "${HZR_CAVEMAN_ROOT}/bridge.mjs" \
  "integrations/caveman-code/bridge.mjs"
verify_matches_repository \
  "${HZR_CAVEMAN_ROOT}/agent-capabilities.json" \
  "contracts/agent-capabilities.json"
verify_matches_repository \
  "${HZR_CAVEMAN_ROOT}/package.json" \
  "integrations/caveman-code/package.json"
verify_matches_repository \
  "${HZR_CAVEMAN_ROOT}/package-lock.json" \
  "integrations/caveman-code/package-lock.json"
verify_matches_repository \
  "${HZR_CAVEMAN_ROOT}/verify-safe-extract.mjs" \
  "integrations/caveman-code/verify-safe-extract.mjs"
# Vendored third-party code: the pin is the supply-chain control, not a restated diff.
verify_sha256 \
  "dc8d9f6d6b26bee37d6e0ccf563789e4325cfffae3f9910feef8333c52968e46" \
  "${HZR_CAVEMAN_ROOT}/vendor/SHA256SUMS"
verify_sha256 \
  "caa31d7dfbd9292ed71ecf2a4955b7b228c0faadb7e5bdf66e558a28c42d69b0" \
  "${HZR_CAVEMAN_ROOT}/vendor/extract-zip/index.js"
verify_matches_repository \
  "${HZR_PROVENANCE_ROOT}/engines.lock.toml" \
  "engines.lock.toml"
verify_matches_repository \
  "${HZR_PROVENANCE_ROOT}/fork-core/SNAPSHOT_V2.tsv" \
  "fork-core/SNAPSHOT_V2.tsv"
verify_matches_repository \
  "${HZR_PROVENANCE_ROOT}/fork-core/SHA256SUMS" \
  "fork-core/SHA256SUMS"

HZR_CURRENT_MANIFEST_SHA256="$(
  sed -n 's/^manifest_sha256 = "\([0-9a-f][0-9a-f]*\)"$/\1/p' \
    "${HZR_PROVENANCE_ROOT}/fork-core/CURRENT_ENGINE.toml"
)"
HZR_CURRENT_CONTENT_SHA256="$(
  sed -n 's/^content_manifest_sha256 = "\([0-9a-f][0-9a-f]*\)"$/\1/p' \
    "${HZR_PROVENANCE_ROOT}/fork-core/CURRENT_ENGINE.toml"
)"
verify_sha256 "${HZR_CURRENT_MANIFEST_SHA256}" \
  "${HZR_PROVENANCE_ROOT}/fork-core/CURRENT_ENGINE_V1.tsv"
verify_sha256 "${HZR_CURRENT_CONTENT_SHA256}" \
  "${HZR_PROVENANCE_ROOT}/fork-core/CURRENT_SHA256SUMS"
verify_matches_repository \
  "${HZR_PROVENANCE_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch" \
  "patches/grepai/0.35.0-disable-worktree-discovery.patch"
verify_matches_repository \
  "${HZR_PROVENANCE_ROOT}/patches/icm/0.10.61-refresh-workspace-lock.patch" \
  "patches/icm/0.10.61-refresh-workspace-lock.patch"
verify_matches_repository \
  "${HZR_PROVENANCE_ROOT}/patches/icm/0.10.61-exit-with-parent.patch" \
  "patches/icm/0.10.61-exit-with-parent.patch"
verify_matches_repository \
  "${HZR_BUNDLE_ROOT}/licenses/HZR-Apache-2.0.txt" \
  "LICENSE"
verify_matches_repository \
  "${HZR_BUNDLE_ROOT}/licenses/rtk-fork-core-MIT.txt" \
  "fork-core/rtk/LICENSE"
# grepai and ICM licences arrive from their upstream clones, so their digests are pinned:
# an upstream relicence must fail the build rather than be repackaged silently.
verify_sha256 \
  "49966552514373129de9faea43a890bf6a8b04f158b2966876a57fdf915980e5" \
  "${HZR_BUNDLE_ROOT}/licenses/grepai-MIT.txt"
verify_sha256 \
  "db0693db32ddac486c96656ec8b827467c1d5d7dc7468eaa0051298425edf2cc" \
  "${HZR_BUNDLE_ROOT}/licenses/ICM-Apache-2.0.txt"
verify_matches_repository \
  "${HZR_BUNDLE_ROOT}/licenses/caveman-code-MIT.txt" \
  "licenses/caveman-code-MIT.txt"

"${HZR_NODE_BINARY}" --check "${HZR_CAVEMAN_ROOT}/bridge.mjs"
(
  cd "${HZR_CAVEMAN_ROOT}"
  shasum -a 256 -c vendor/SHA256SUMS >/dev/null
)
"${HZR_NODE_BINARY}" "${HZR_CAVEMAN_ROOT}/verify-safe-extract.mjs"
"${HZR_NODE_BINARY}" -e '
  const manifest = require(process.argv[1]);
  if (manifest.version !== "0.65.2") process.exit(1);
' "${HZR_CAVEMAN_ROOT}/node_modules/@juliusbrussee/caveman-code/package.json"
(
  cd "${HZR_CAVEMAN_ROOT}"
  "${HZR_NODE_BINARY}" --input-type=module -e '
    import {
      createAgentSession,
      DefaultResourceLoader,
      SessionManager,
      SettingsManager,
    } from "@juliusbrussee/caveman-code";
    for (const [name, value] of Object.entries({
      createAgentSession,
      DefaultResourceLoader,
      SessionManager,
      SettingsManager,
    })) {
      if (typeof value !== "function") {
        throw new Error(`Caveman runtime export is unavailable: ${name}`);
      }
    }
  '
)

"${HZR_BINARY_ROOT}/hzr" --version | grep -Fx "hzr 0.8.1" >/dev/null
"${HZR_ENGINE_ROOT}/grepai" version | grep -F "0.35.0" >/dev/null
"${HZR_ENGINE_ROOT}/icm" --version | grep -F "0.10.61" >/dev/null
"${HZR_NODE_BINARY}" --version | grep -Fx "v22.17.1" >/dev/null
"${HZR_ENGINE_ROOT}/rtk" --version | grep -Fx "rtk 0.44.1-fork.1" >/dev/null

HZR_SMOKE_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/hzr-bundle-smoke.XXXXXX")"
HZR_DAEMON_PID=""
HZR_SECOND_DAEMON_PID=""
stop_hzr_daemon() {
  if [[ -z "${HZR_DAEMON_PID}" ]] || ! kill -0 "${HZR_DAEMON_PID}" 2>/dev/null; then
    HZR_DAEMON_PID=""
    return
  fi
  kill -INT "${HZR_DAEMON_PID}" 2>/dev/null || true
  for _ in {1..100}; do
    if ! kill -0 "${HZR_DAEMON_PID}" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if kill -0 "${HZR_DAEMON_PID}" 2>/dev/null; then
    kill -TERM "${HZR_DAEMON_PID}" 2>/dev/null || true
  fi
  wait "${HZR_DAEMON_PID}" 2>/dev/null || true
  HZR_DAEMON_PID=""
}
stop_hzr_second_daemon() {
  if [[ -z "${HZR_SECOND_DAEMON_PID}" ]] || \
    ! kill -0 "${HZR_SECOND_DAEMON_PID}" 2>/dev/null; then
    HZR_SECOND_DAEMON_PID=""
    return
  fi
  kill -INT "${HZR_SECOND_DAEMON_PID}" 2>/dev/null || true
  for _ in {1..50}; do
    if ! kill -0 "${HZR_SECOND_DAEMON_PID}" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if kill -0 "${HZR_SECOND_DAEMON_PID}" 2>/dev/null; then
    kill -TERM "${HZR_SECOND_DAEMON_PID}" 2>/dev/null || true
  fi
  wait "${HZR_SECOND_DAEMON_PID}" 2>/dev/null || true
  HZR_SECOND_DAEMON_PID=""
}
cleanup_hzr_smoke() {
  stop_hzr_second_daemon
  stop_hzr_daemon
  if [[ -n "${HZR_SMOKE_TEMP:-}" && -d "${HZR_SMOKE_TEMP}" ]]; then
    rm -rf -- "${HZR_SMOKE_TEMP}"
  fi
}
trap cleanup_hzr_smoke EXIT

if [[ "${HZR_ENGINE_ROOT}" == *$'\n'* || "${HZR_ENGINE_ROOT}" == *'"'* || \
  "${HZR_SMOKE_TEMP}" == *$'\n'* || "${HZR_SMOKE_TEMP}" == *'"'* ]]; then
  echo "bundle smoke paths cannot be represented safely in TOML" >&2
  exit 1
fi
HZR_SMOKE_CONFIG="${HZR_SMOKE_TEMP}/config.toml"
HZR_SMOKE_PORT="$("${HZR_NODE_BINARY}" -e '
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
{
  printf 'schema_version = 1\n'
  printf 'data_dir = "%s/data"\n' "${HZR_SMOKE_TEMP}"
  printf '[daemon]\n'
  printf 'bind = "127.0.0.1:%s"\n' "${HZR_SMOKE_PORT}"
  printf '[engines]\n'
  printf 'directory = "%s"\n' "${HZR_ENGINE_ROOT}"
  printf 'strict_versions = true\n'
  printf 'auto_start_icm = false\n'
  printf 'auto_index = false\n'
} >"${HZR_SMOKE_CONFIG}"

mkdir -p "${HZR_SMOKE_TEMP}/home" "${HZR_SMOKE_TEMP}/data" "${HZR_SMOKE_TEMP}/workspace"
export HOME="${HZR_SMOKE_TEMP}/home"
git -C "${HZR_SMOKE_TEMP}/workspace" init --quiet
HZR_DOCTOR_JSON="$(
  "${HZR_BINARY_ROOT}/hzr" \
    --config "${HZR_SMOKE_CONFIG}" \
    --json doctor --workspace "${HZR_SMOKE_TEMP}/workspace" || true
)"
"${HZR_NODE_BINARY}" -e '
  const report = JSON.parse(process.argv[1]);
  for (const name of ["engine_grepai", "engine_icm", "engine_rtk", "caveman_code"]) {
    const check = report.checks.find((candidate) => candidate.name === name);
    if (!check || check.status !== "pass") {
      console.error(`assembled doctor check failed: ${name}`, check);
      process.exit(1);
    }
  }
' "${HZR_DOCTOR_JSON}"

HZR_FORK_VERSION="$(
  "${HZR_BINARY_ROOT}/hzr" --config "${HZR_SMOKE_CONFIG}" rtk -- --version
)"
if [[ "${HZR_FORK_VERSION}" != "rtk 0.44.1-fork.1" ]]; then
  echo "assembled HZR-to-fork passthrough failed: ${HZR_FORK_VERSION}" >&2
  exit 1
fi

HZR_COMPAT_FORK_VERSION="$(
  "${HZR_BINARY_ROOT}/rtk" --version
)"
if [[ "${HZR_COMPAT_FORK_VERSION}" != "rtk 0.44.1-fork.1" ]]; then
  echo "bin/rtk argv0 compatibility routing failed: ${HZR_COMPAT_FORK_VERSION}" >&2
  exit 1
fi

HZR_DAEMON_LOG="${HZR_SMOKE_TEMP}/hzrd.log"
"${HZR_BINARY_ROOT}/hzr" --config "${HZR_SMOKE_CONFIG}" daemon serve \
  >"${HZR_DAEMON_LOG}" 2>&1 &
HZR_DAEMON_PID=$!
HZR_HEALTH_JSON=""
for _ in {1..100}; do
  if HZR_HEALTH_JSON="$(
    "${HZR_BINARY_ROOT}/hzr" --config "${HZR_SMOKE_CONFIG}" --json daemon status 2>/dev/null
  )"; then
    break
  fi
  if ! kill -0 "${HZR_DAEMON_PID}" 2>/dev/null; then
    echo "assembled daemon exited before becoming healthy" >&2
    sed -n '1,160p' "${HZR_DAEMON_LOG}" >&2
    exit 1
  fi
  sleep 0.1
done
if [[ -z "${HZR_HEALTH_JSON}" ]]; then
  echo "assembled daemon did not become healthy within ten seconds" >&2
  sed -n '1,160p' "${HZR_DAEMON_LOG}" >&2
  exit 1
fi

"${HZR_NODE_BINARY}" -e '
  const report = JSON.parse(process.argv[1]);
if (report.protocol_version !== 1 || report.hzr_version !== "0.8.1") {
    console.error("assembled daemon protocol/version mismatch", report);
    process.exit(1);
  }
  if (report.state.toLowerCase() !== "ready") {
    console.error("assembled daemon is not healthy", report);
    process.exit(1);
  }
  const rtk = report.engines.find((engine) => engine.name === "rtk");
  if (!rtk || rtk.version !== "0.44.1-fork.1" || rtk.state.toLowerCase() !== "ready") {
    console.error("assembled daemon did not load exact fork-core", rtk);
    process.exit(1);
  }
' "${HZR_HEALTH_JSON}"

"${HZR_NODE_BINARY}" -e '
  const endpoint = process.argv[1];
  fetch(endpoint).then((response) => {
    if (response.status !== 401) {
      console.error(`unauthenticated daemon request returned ${response.status}`);
      process.exit(1);
    }
  }).catch((error) => {
    console.error(error);
    process.exit(1);
  });
' "http://127.0.0.1:${HZR_SMOKE_PORT}/v1/health"

"${HZR_NODE_BINARY}" -e '
  const endpoint = process.argv[1];
  Promise.all([
    fetch(`${endpoint}/`).then(async (response) => {
      const body = await response.text();
      if (response.status !== 200 || !body.includes("HZR · Local control plane")) {
        throw new Error(`visualizer index failed: ${response.status}`);
      }
    }),
    fetch(`${endpoint}/v1/dashboard`).then(async (response) => {
      const report = await response.json();
      const ids = new Set(report.services.map((service) => service.id));
if (response.status !== 200 || report.hzr_version !== "0.8.1" ||
          !["hzrd", "rtk", "icm", "grepai"].every((id) => ids.has(id))) {
        throw new Error(`visualizer dashboard contract failed: ${JSON.stringify(report)}`);
      }
    }),
  ]).catch((error) => {
    console.error(error);
    process.exit(1);
  });
' "http://127.0.0.1:${HZR_SMOKE_PORT}"

HZR_SECOND_DAEMON_LOG="${HZR_SMOKE_TEMP}/hzrd-second.log"
"${HZR_BINARY_ROOT}/hzr" --config "${HZR_SMOKE_CONFIG}" daemon serve \
  >"${HZR_SECOND_DAEMON_LOG}" 2>&1 &
HZR_SECOND_DAEMON_PID=$!
HZR_SECOND_DAEMON_EXITED=false
for _ in {1..50}; do
  if ! kill -0 "${HZR_SECOND_DAEMON_PID}" 2>/dev/null; then
    HZR_SECOND_DAEMON_EXITED=true
    break
  fi
  sleep 0.1
done
if [[ "${HZR_SECOND_DAEMON_EXITED}" != true ]]; then
  stop_hzr_second_daemon
  echo "assembled daemon singleton lock allowed a second daemon" >&2
  exit 1
fi
if wait "${HZR_SECOND_DAEMON_PID}"; then
  HZR_SECOND_DAEMON_PID=""
  echo "second assembled daemon unexpectedly exited successfully" >&2
  exit 1
fi
HZR_SECOND_DAEMON_PID=""
if ! grep -Eiq 'lock|already' "${HZR_SECOND_DAEMON_LOG}"; then
  echo "second assembled daemon failed for a reason other than singleton ownership" >&2
  sed -n '1,160p' "${HZR_SECOND_DAEMON_LOG}" >&2
  exit 1
fi

stop_hzr_daemon

echo "HZR assembled bundle smoke passed"
