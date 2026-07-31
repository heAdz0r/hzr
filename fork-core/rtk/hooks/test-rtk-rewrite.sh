#!/usr/bin/env bash
set -u

HOOK="${HOOK:-$HOME/.claude/hooks/rtk-rewrite.sh}"
PASS=0
FAIL=0

check_rewrite() {
  local description="$1"
  local command="$2"
  local expected
  expected=$(rtk rewrite "$command" 2>/dev/null) || expected=""
  [ "$expected" = "$command" ] && expected=""

  local input output actual decision metadata
  input=$(jq -n --arg cmd "$command" '{"tool_name":"Bash","tool_input":{"command":$cmd,"description":"keep-me"}}')
  output=$(bash "$HOOK" <<<"$input" 2>/dev/null) || true
  actual=$(jq -r '.hookSpecificOutput.updatedInput.command // empty' <<<"${output:-{}}" 2>/dev/null)
  decision=$(jq -r '.hookSpecificOutput.permissionDecision // empty' <<<"${output:-{}}" 2>/dev/null)
  metadata=$(jq -r '.hookSpecificOutput.updatedInput.description // empty' <<<"${output:-{}}" 2>/dev/null)

  if [ "$actual" = "$expected" ] && [ -z "$decision" ] && { [ -z "$expected" ] || [ "$metadata" = "keep-me" ]; }; then
    printf 'PASS %s\n' "$description"
    PASS=$((PASS + 1))
  else
    printf 'FAIL %s\n  expected: %s\n  actual:   %s\n  decision: %s\n' \
      "$description" "${expected:-(no rewrite)}" "${actual:-(no rewrite)}" "${decision:-(deferred)}"
    FAIL=$((FAIL + 1))
  fi
}

check_rewrite "git status" "git status"
check_rewrite "mutating command keeps host policy" "git commit -m 'message'"
check_rewrite "ripgrep keeps rg engine" "rg pattern src"
check_rewrite "grep keeps grep engine" "grep -rn pattern src"
check_rewrite "semantic search" "grepai search auth flow"
check_rewrite "uv pytest" "uv run pytest tests/"
check_rewrite "unsupported command" "echo hello"
check_rewrite "already rewritten" "rtk git status"
check_rewrite "heredoc" $'cat <<EOF\nhello\nEOF'
check_rewrite "independent multiline commands" $'git status\ncargo test'
check_rewrite "safe pipeline producer" "git log --oneline | head -10"
check_rewrite "transforming pipeline stays raw" "git log --oneline | awk '{print \$1}'"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
test "$FAIL" -eq 0
