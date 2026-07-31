#!/bin/bash
# RTK hook for Claude Code PreToolUse:Task (Explore subagent policy)
# Default behavior: hard deny native Task/Explore and route to RTK memory layer.
# Set RTK_ALLOW_NATIVE_EXPLORE=1 (or legacy RTK_BLOCK_NATIVE_EXPLORE=0) to allow native Explore.
# Set RTK_NOTIFY_NATIVE_EXPLORE=0 to suppress allow-mode notifications.

if ! command -v jq >/dev/null 2>&1; then
  cat <<'EOF_JSON'
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "Task/Explore policy requires jq. Use Bash: rtk memory explore <path>."
  }
}
EOF_JSON
  exit 0
fi

set -euo pipefail

INPUT="$(cat)"
TOOL_NAME="$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)"
SUBAGENT_TYPE="$(echo "$INPUT" | jq -r '.tool_input.subagent_type // empty' 2>/dev/null)"

# Only govern Task + Explore. Other Task usages are untouched.
if [ "$TOOL_NAME" != "Task" ] || [ "$SUBAGENT_TYPE" != "Explore" ]; then
  exit 0
fi

if [ "${RTK_ALLOW_NATIVE_EXPLORE:-0}" = "1" ] || [ "${RTK_BLOCK_NATIVE_EXPLORE:-1}" = "0" ]; then
  if [ "${RTK_NOTIFY_NATIVE_EXPLORE:-1}" = "0" ]; then
    exit 0
  fi

  cat <<'EOF_JSON'
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "Native Task/Explore allowed by override. Preferred: Bash rtk memory explore <path>."
  }
}
EOF_JSON
  exit 0
fi

cat <<'EOF_JSON'
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "Native Task/Explore blocked by RTK policy. Use Bash: rtk memory explore <path> (or rtk memory serve API). Override: RTK_ALLOW_NATIVE_EXPLORE=1 (RTK_BLOCK_NATIVE_EXPLORE=0)."
  }
}
EOF_JSON
