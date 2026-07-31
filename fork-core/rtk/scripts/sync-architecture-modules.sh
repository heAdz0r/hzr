#!/usr/bin/env bash
set -euo pipefail

ARCH_FILE="${1:-ARCHITECTURE.md}"

if [[ ! -f "$ARCH_FILE" ]]; then
  echo "WARN: ${ARCH_FILE} not found, skipping architecture module sync."
  exit 0
fi

main_modules="$(grep -Ec '^mod [a-zA-Z0-9_]+;' src/main.rs || true)"
if [[ -z "${main_modules}" ]]; then
  echo "ERROR: failed to detect module count from src/main.rs"
  exit 1
fi

tmp_file="$(mktemp)"
awk -v modules="$main_modules" '
{
  if ($0 ~ /^### Complete Module Map \([0-9]+ Modules\)$/) {
    print "### Complete Module Map (" modules " Modules)";
    next;
  }
  if ($0 ~ /^\*\*Total: [0-9]+ modules\*\*/) {
    print "**Total: " modules " modules** (auto-synced from src/main.rs)";
    next;
  }
  print;
}
' "$ARCH_FILE" > "$tmp_file"

if cmp -s "$tmp_file" "$ARCH_FILE"; then
  rm -f "$tmp_file"
  echo "ARCHITECTURE.md module count already in sync (${main_modules})."
else
  mv "$tmp_file" "$ARCH_FILE"
  echo "Updated ${ARCH_FILE} module count to ${main_modules}."
fi
