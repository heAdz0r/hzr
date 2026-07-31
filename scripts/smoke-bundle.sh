#!/usr/bin/env bash
set -euo pipefail

HZR_BUNDLE_ROOT="${1:-}"
if [[ -z "${HZR_BUNDLE_ROOT}" || ! -d "${HZR_BUNDLE_ROOT}" ]]; then
  echo "usage: scripts/smoke-bundle.sh /absolute/path/to/hzr-bundle" >&2
  exit 2
fi
HZR_BUNDLE_ROOT="$(cd -- "${HZR_BUNDLE_ROOT}" && pwd -P)"
HZR_BINARY_ROOT="${HZR_BUNDLE_ROOT}/bin"
HZR_ENGINE_ROOT="${HZR_BUNDLE_ROOT}/engines"
HZR_CAVEMAN_ROOT="${HZR_ENGINE_ROOT}/caveman-code"
HZR_PROVENANCE_ROOT="${HZR_BUNDLE_ROOT}/share/hzr"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required smoke command: $1" >&2
    exit 1
  fi
}

for HZR_REQUIRED_COMMAND in awk git grep kill node npm readlink shasum sleep; do
  require_command "${HZR_REQUIRED_COMMAND}"
done

for HZR_EXECUTABLE in \
  "${HZR_BINARY_ROOT}/hzr" \
  "${HZR_BINARY_ROOT}/hzrd" \
  "${HZR_ENGINE_ROOT}/grepai" \
  "${HZR_ENGINE_ROOT}/icm" \
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

for HZR_RUNTIME_FILE in bridge.mjs package.json package-lock.json; do
  if [[ ! -f "${HZR_CAVEMAN_ROOT}/${HZR_RUNTIME_FILE}" ]]; then
    echo "managed Caveman runtime file is missing: ${HZR_RUNTIME_FILE}" >&2
    exit 1
  fi
done
if [[ ! -d "${HZR_CAVEMAN_ROOT}/node_modules" ]]; then
  echo "managed Caveman production node_modules is missing" >&2
  exit 1
fi

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

verify_sha256 \
  "ef96d21b0745b1885bab9c05f9af88ce6419debd63dbe9d5d70c211533817f74" \
  "${HZR_CAVEMAN_ROOT}/bridge.mjs"
verify_sha256 \
  "2ab0f4d06265fea26f17aa0eaedfe8f132feb7fc2bc705b80f622659aa99a07e" \
  "${HZR_CAVEMAN_ROOT}/package.json"
verify_sha256 \
  "2ae970d2630c81c15315f0ca863b1d34d1ac8479245b7ae1826f798ac41b3ae4" \
  "${HZR_CAVEMAN_ROOT}/package-lock.json"
verify_sha256 \
  "584c252cbad7ea013f2256a46038d605e76dbc39f95375692818de57b2aaaabc" \
  "${HZR_PROVENANCE_ROOT}/engines.lock.toml"
verify_sha256 \
  "f4296ec404f461d6fc03c966c0dc79caee6c3118a73d1ed1a078ded5529f0a16" \
  "${HZR_PROVENANCE_ROOT}/fork-core/SNAPSHOT_V2.tsv"
verify_sha256 \
  "072a62adc754b728ec99a507d2c1a223d83077d067a9249a26d357eec890b4cc" \
  "${HZR_PROVENANCE_ROOT}/fork-core/SHA256SUMS"
verify_sha256 \
  "55535352bc9f4837198c652b8c44ec54a0a7ef82fbd81e11b4ec11f4c4082991" \
  "${HZR_PROVENANCE_ROOT}/patches/grepai/0.35.0-disable-worktree-discovery.patch"
verify_sha256 \
  "cd38e20e32f352bfde93a4ce297799ef8b5f984f8af928409ef0f3e47102e586" \
  "${HZR_PROVENANCE_ROOT}/patches/icm/0.10.61-refresh-workspace-lock.patch"
verify_sha256 \
  "c8c0bdfa8f8a8a606fac9ef50f1be51f46ee28afe63a94aa2430ee75342a9e2c" \
  "${HZR_BUNDLE_ROOT}/licenses/HZR-Apache-2.0.txt"
verify_sha256 \
  "1231ac74c1d872c6d40a64bb61921c04b1482dcb0a4f01dcff04779d0c98947f" \
  "${HZR_BUNDLE_ROOT}/licenses/rtk-fork-core-MIT.txt"
verify_sha256 \
  "49966552514373129de9faea43a890bf6a8b04f158b2966876a57fdf915980e5" \
  "${HZR_BUNDLE_ROOT}/licenses/grepai-MIT.txt"
verify_sha256 \
  "db0693db32ddac486c96656ec8b827467c1d5d7dc7468eaa0051298425edf2cc" \
  "${HZR_BUNDLE_ROOT}/licenses/ICM-Apache-2.0.txt"
verify_sha256 \
  "3c295c1f04099384f08061343c1b321455c0cb581160b6012cb7a840401dbe7b" \
  "${HZR_BUNDLE_ROOT}/licenses/caveman-code-MIT.txt"

node --check "${HZR_CAVEMAN_ROOT}/bridge.mjs"
node -e '
  const manifest = require(process.argv[1]);
  if (manifest.version !== "0.65.2") process.exit(1);
' "${HZR_CAVEMAN_ROOT}/node_modules/@juliusbrussee/caveman-code/package.json"
npm ls --omit=dev --all --prefix "${HZR_CAVEMAN_ROOT}" >/dev/null

"${HZR_BINARY_ROOT}/hzr" --version | grep -Fx "hzr 0.1.0" >/dev/null
"${HZR_ENGINE_ROOT}/grepai" version | grep -F "0.35.0" >/dev/null
"${HZR_ENGINE_ROOT}/icm" --version | grep -F "0.10.61" >/dev/null
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
HZR_SMOKE_PORT="$(node -e '
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

mkdir -p "${HZR_SMOKE_TEMP}/data" "${HZR_SMOKE_TEMP}/workspace"
git -C "${HZR_SMOKE_TEMP}/workspace" init --quiet
HZR_DOCTOR_JSON="$(
  "${HZR_BINARY_ROOT}/hzr" \
    --config "${HZR_SMOKE_CONFIG}" \
    --json doctor --workspace "${HZR_SMOKE_TEMP}/workspace"
)"
node -e '
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

node -e '
  const report = JSON.parse(process.argv[1]);
  if (report.protocol_version !== 1 || report.hzr_version !== "0.1.0") {
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

node -e '
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
