#!/usr/bin/env bash
# 0.11.0 (FR-9): differential probe for the upstream RTK v0.50.0 sync.
#
# Reproduces the fidelity cases E1-E14 of docs/PRD_HZR_UPSTREAM_RTK_SYNC_0_50_0.md
# against the native tools on throwaway fixtures, and exits non-zero on the first
# regression class it finds. Every comparison is byte-exact on stdout plus the
# exit code unless the case documents an intentional rendering (a bounded window,
# a disclosure line); those assert the property instead.
#
# Usage: scripts/upstream-parity-probe.sh [path/to/rtk]
#        (default: fork-core/rtk/target/debug/rtk, built if missing)
set -uo pipefail

HZR_REPOSITORY_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
RTK="${1:-${HZR_REPOSITORY_ROOT}/fork-core/rtk/target/debug/rtk}"
if [[ ! -x "${RTK}" ]]; then
  cargo build --quiet --manifest-path "${HZR_REPOSITORY_ROOT}/fork-core/rtk/Cargo.toml" || exit 2
fi
RTK="$(cd -- "$(dirname -- "${RTK}")" && pwd -P)/$(basename -- "${RTK}")"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/hzr-parity-probe.XXXXXX")"
trap 'rm -rf "${WORK}"' EXIT
export RTK_TRACKING_DISABLED=1 RTK_TELEMETRY_DISABLED=1 RTK_TEE=0
export RTK_DB_PATH="${WORK}/history.sqlite"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); printf 'ok    %s\n' "$1"; }
fail() { FAIL=$((FAIL + 1)); printf 'FAIL  %s\n' "$1"; [[ -n "${2:-}" ]] && printf '      %s\n' "$2"; }

# Byte-exact stdout and exit code: same_as_native <label> <native argv…> -- <rtk argv…>
same_as_native() {
  local label="$1"; shift
  local native=() rtk_args=()
  while [[ "$1" != "--" ]]; do native+=("$1"); shift; done
  shift
  rtk_args=("$@")
  local n_out r_out n_code r_code
  n_out="$(cd "${WORK}/fx" && "${native[@]}" 2>/dev/null | od -An -c)"; n_code=${PIPESTATUS[0]}
  n_code=$(cd "${WORK}/fx" && "${native[@]}" >/dev/null 2>&1; echo $?)
  r_out="$(cd "${WORK}/fx" && "${RTK}" "${rtk_args[@]}" 2>/dev/null | od -An -c)"
  r_code=$(cd "${WORK}/fx" && "${RTK}" "${rtk_args[@]}" >/dev/null 2>&1; echo $?)
  if [[ "${n_out}" == "${r_out}" && "${n_code}" == "${r_code}" ]]; then
    pass "${label}"
  else
    fail "${label}" "exit native=${n_code} rtk=${r_code}; stdout equal: $([[ "${n_out}" == "${r_out}" ]] && echo yes || echo no)"
  fi
}

# --- fixtures -------------------------------------------------------------
mkdir -p "${WORK}/fx/src" "${WORK}/fx/.hid" "${WORK}/bin"
cd "${WORK}/fx" || exit 2
printf 'alpha one\nbeta two\ngamma three\n' > a.txt
printf 'alpha one\nbeta TWO\ngamma three\n' > b.txt
printf 'x\nalpha one\nbeta two\ngamma three\n' > c.txt
printf 'foo 1\nfoo 2\nfoo 3\n' > src/a.rs
printf 'foo\n' > src/b.rs
printf 'x\n' > src/.dot.rs
printf 'x\n' > .hid/x.rs
printf 'caf\xe9 one\nl2\nl3\nl4\n' > latin.txt
printf 'a\r\nb\r\nc' > crlf.txt

# E1-E3: diff answers like diff(1)
same_as_native "E1 diff, modified line"      diff a.txt b.txt -- diff a.txt b.txt
same_as_native "E2 diff, inserted line"      diff a.txt c.txt -- diff a.txt c.txt
same_as_native "E3 diff -u"                  diff -u a.txt b.txt -- diff -u a.txt b.txt
same_as_native "E3 diff, identical"          diff a.txt a.txt -- diff a.txt a.txt

# E4-E5: grep's own short flags
same_as_native "E4 grep -m 1"                grep -m 1 foo src/a.rs -- grep -m 1 foo src/a.rs
same_as_native "E5 grep -l"                  grep -l foo src/a.rs src/b.rs -- grep -l foo src/a.rs src/b.rs
same_as_native "E5 grep -v (not rtk verbose)" grep -v 1 src/a.rs -- grep -v 1 src/a.rs

# E6: gh pr checks with a failing check (gh exits 1)
cat > "${WORK}/bin/gh" <<'EOF'
#!/bin/sh
printf 'build\tpass\t1m\thttps://x/1\t\ntest\tfail\t2m\thttps://x/2\t\ndeploy\tcancel\t0\thttps://x/3\t\n'
exit 1
EOF
chmod +x "${WORK}/bin/gh"
out="$(PATH="${WORK}/bin:${PATH}" "${RTK}" gh pr checks 7 2>&1)"; code=$?
if [[ ${code} -eq 1 && "${out}" == *"test"* && "${out}" == *"fail"* ]]; then pass "E6 gh pr checks, failing"; else fail "E6 gh pr checks, failing" "exit=${code} out=${out}"; fi

# E7-E8: find
same_as_native "E7 find, missing root"      find nope -name '*.rs' -- find nope -name '*.rs'
out="$("${RTK}" find . -name '*.rs' 2>&1)"
if [[ "${out}" == *"hidden/gitignored, not searched"* ]]; then pass "E8 find discloses hidden matches"; else fail "E8 find discloses hidden matches" "${out}"; fi

# E9-E10: head/tail windows are byte-exact on stdout
same_as_native "E9 head, latin-1"           head -n 2 latin.txt -- read latin.txt --head-lines 2
same_as_native "E10 tail, CRLF no EOL"      tail -n 2 crlf.txt -- read crlf.txt --tail-lines 2

# E11: ls without -a lists no dot entries
out="$("${RTK}" ls src 2>&1)"
if [[ "${out}" != *".dot.rs"* && "${out}" == *"a.rs"* ]]; then pass "E11 ls hides dot entries"; else fail "E11 ls hides dot entries" "${out}"; fi

# E12: coloured git configuration does not empty the compacted diff
git init -q -b main repo && cd repo || exit 2
git config user.email probe@example.com && git config user.name probe && git config color.ui always
printf 'one\n' > f.txt && git add f.txt && git commit -qm one && printf 'two\n' > f.txt
out="$("${RTK}" git diff 2>&1)"
if [[ "${out}" != *$'\e'* && "${out}" == *"+two"* ]]; then pass "E12 git diff under color.ui=always"; else fail "E12 git diff under color.ui=always" "${out}"; fi
cd "${WORK}/fx" || exit 2

# E13: the runner the caller named is the runner that runs
cat > "${WORK}/bin/bunx" <<'EOF'
#!/bin/sh
echo "bunx-ran: $*"
EOF
chmod +x "${WORK}/bin/bunx"
out="$(PATH="${WORK}/bin:/usr/bin:/bin" "${RTK}" --js-runner bunx tsc --version 2>&1)"
if [[ "${out}" == *"bunx-ran: tsc --version"* ]]; then pass "E13 bunx tsc runs through bunx"; else fail "E13 bunx tsc runs through bunx" "${out}"; fi

# E14: routes that were raw proxies
while IFS='|' read -r command expected; do
  got="$("${RTK}" rewrite "${command}" 2>/dev/null | head -1)"
  if [[ "${got}" == "${expected}" ]]; then pass "E14 route: ${command}"; else fail "E14 route: ${command}" "got '${got}'"; fi
done <<'EOF'
deno test|rtk deno test
pnpm --filter web test|rtk pnpm --filter web test
timeout 60 cargo test|timeout 60 rtk cargo test
bunx tsc --noEmit|rtk --js-runner bunx tsc --noEmit
head -1 --help|
EOF

# US-008: a stderr-only failure is never rendered as success
printf '#!/bin/sh\necho "FATAL-MARKER" >&2\nexit 2\n' > "${WORK}/bin/go"
chmod +x "${WORK}/bin/go"
out="$(PATH="${WORK}/bin:/usr/bin:/bin" "${RTK}" go build ./... 2>&1)"; code=$?
if [[ ${code} -eq 2 && "${out}" == *"FATAL-MARKER"* && "${out}" != *"Success"* ]]; then pass "US-008 go build stderr failure"; else fail "US-008 go build stderr failure" "exit=${code} ${out}"; fi

# US-007: SIGTERM keeps captured output and dies by the signal
printf '#!/bin/sh\necho "error: before signal"\nsleep 30\n' > "${WORK}/slow.sh"
chmod +x "${WORK}/slow.sh"
"${RTK}" err "${WORK}/slow.sh" > "${WORK}/sig.out" 2>&1 < /dev/null &
pid=$!
sleep 1
kill -TERM "${pid}"
wait "${pid}"; code=$?
if [[ ${code} -eq 143 ]] && grep -q "before signal" "${WORK}/sig.out"; then pass "US-007 SIGTERM flush + signal death"; else fail "US-007 SIGTERM flush + signal death" "exit=${code} $(cat "${WORK}/sig.out")"; fi

printf '\n%d passed, %d failed\n' "${PASS}" "${FAIL}"
[[ ${FAIL} -eq 0 ]]
