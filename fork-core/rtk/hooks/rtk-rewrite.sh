#!/usr/bin/env bash
# rtk-hook-version: 3
# RTK Claude Code hook. Rewrite rules live in `rtk rewrite`.

if ! command -v jq &>/dev/null || ! command -v rtk &>/dev/null; then
  exit 0
fi

INPUT=$(cat)
CMD=$(jq -r '.tool_input.command // empty' <<<"$INPUT")

if [ -z "$CMD" ]; then
  exit 0
fi

REWRITTEN=$(rtk rewrite "$CMD" 2>/dev/null) || exit 0

if [ "$CMD" = "$REWRITTEN" ]; then
  exit 0
fi

# The fork's `rtk rewrite` does not return upstream permission verdicts yet.
# Omit permissionDecision so Claude Code applies its normal policy to the
# rewritten command, including mutating commands.
jq -c --arg cmd "$REWRITTEN" \
  '.tool_input.command = $cmd | {
    "hookSpecificOutput": {
      "hookEventName": "PreToolUse",
      "updatedInput": .tool_input
    }
  }' <<<"$INPUT"
