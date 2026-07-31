#!/bin/bash
# RTK Memory Context Hook - PreToolUse:Task
# Injects cached project memory + pre-read files into ALL subagent prompts.
# Goal: subagent gets everything pre-indexed -> minimal native tool calls.
#
# Pipeline: plan(graph seeds) -> explicit paths -> rgai(semantic) -> pre-read -> inject
# Input:  JSON from stdin (Claude Code tool_use event)
# Output: JSON with updatedInput (modified task prompt) or nothing (pass-through)

# Guards: dependencies required
if ! command -v rtk &>/dev/null || ! command -v jq &>/dev/null; then
  exit 0
fi

set -euo pipefail

clamp_chars() {
  local text="$1"
  local max_chars="$2"
  printf '%s' "$text" | head -c "$max_chars"
}

INPUT=$(cat)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)

# Activate for all Task invocations regardless of subagent_type.
if [ "$TOOL_NAME" != "Task" ]; then
  exit 0
fi

# Detect project root (walk up from CWD to find .git)
PROJECT_ROOT="."
dir="$PWD"
while [ "$dir" != "/" ]; do
  if [ -d "$dir/.git" ]; then
    PROJECT_ROOT="$dir"
    break
  fi
  dir=$(dirname "$dir")
done

# Strip previous RTK memory preamble to prevent recursive self-noise.
extract_base_prompt() {
  local prompt="$1"
  if [[ "$prompt" == *"RTK Project Memory Context"* ]]; then
    local stripped
    stripped="$(printf '%s' "$prompt" | awk '
      /^---[[:space:]]*$/ { sep=1; next }
      sep { print }
    ')" || true
    if [ -n "${stripped//[$'\n\r\t ']}" ]; then
      printf '%s' "$stripped"
      return
    fi
  fi
  printf '%s' "$prompt"
}

CURRENT_PROMPT=$(echo "$INPUT" | jq -r '.tool_input.prompt // empty')
CURRENT_PROMPT=$(extract_base_prompt "$CURRENT_PROMPT")

# --- Config (overridable via env) ---
RTK_MEM_PLAN_BUDGET="${RTK_MEM_PLAN_BUDGET:-2400}"
RTK_MEM_PLAN_TOP="${RTK_MEM_PLAN_TOP:-25}"
RTK_MEM_SEMANTIC_MAX="${RTK_MEM_SEMANTIC_MAX:-12}"
RTK_MEM_PRE_READ_COUNT="${RTK_MEM_PRE_READ_COUNT:-20}"
RTK_MEM_PRE_READ_MAX_LINES="${RTK_MEM_PRE_READ_MAX_LINES:-250}"
RTK_MEM_PRE_READ_MAX_CHARS="${RTK_MEM_PRE_READ_MAX_CHARS:-80000}"
RTK_MEM_PRE_READ_FILE_MAX_CHARS="${RTK_MEM_PRE_READ_FILE_MAX_CHARS:-8000}"
RTK_MEM_LARGE_FILE_THRESHOLD="${RTK_MEM_LARGE_FILE_THRESHOLD:-15000}"
RTK_MEM_GRAPH_MAX_CHARS="${RTK_MEM_GRAPH_MAX_CHARS:-4000}"
RTK_MEM_SEMANTIC_MAX_CHARS="${RTK_MEM_SEMANTIC_MAX_CHARS:-8000}"
RTK_MEM_CONTEXT_MAX_CHARS="${RTK_MEM_CONTEXT_MAX_CHARS:-120000}"

MEM_CONTEXT=""
TASK_HINT=$(printf '%s' "$CURRENT_PROMPT" | tr '\n' ' ' | head -c 700)

# --- Stage 1: Graph-first plan (structural context) ---
PLAN_OUTPUT=""
PLAN_CANDIDATES=""
GRAPH_SEEDS=""
if [ -n "$TASK_HINT" ]; then
  PLAN_OUTPUT=$(rtk memory plan "$TASK_HINT" "$PROJECT_ROOT"     --format text --top "$RTK_MEM_PLAN_TOP"     --token-budget "$RTK_MEM_PLAN_BUDGET" 2>/dev/null) || true
  PLAN_CANDIDATES=$(printf '%s' "$PLAN_OUTPUT" | grep -E "^  \[" | sed "s/^  \[[^]]*\] //" | awk '!seen[$0]++' || true)
  GRAPH_SEEDS="$PLAN_OUTPUT"
fi

# --- Stage 1.5: Explicit path extraction from query ---
# If query mentions "src/memory_layer/", expand to all source files in that dir
EXPLICIT_FILES=""
DIR_PATTERNS=$(printf '%s' "$TASK_HINT" | grep -oE '\b(src|lib|hooks|scripts|tests|benchmarks)/[a-zA-Z0-9_./]*' | sort -u || true)
if [ -n "$DIR_PATTERNS" ]; then
  while IFS= read -r pattern; do
    [ -z "$pattern" ] && continue
    pattern="${pattern%/}"
    pattern="${pattern%.}"
    full_path="$PROJECT_ROOT/$pattern"
    if [ -d "$full_path" ]; then
      for f in "$full_path"/*.rs "$full_path"/*.ts "$full_path"/*.tsx "$full_path"/*.js "$full_path"/*.py "$full_path"/*.sh; do
        [ -f "$f" ] && EXPLICIT_FILES="${EXPLICIT_FILES}${f#$PROJECT_ROOT/}
"
      done
    elif [ -f "$full_path" ]; then
      EXPLICIT_FILES="${EXPLICIT_FILES}$pattern
"
    fi
  done <<< "$DIR_PATTERNS"
fi

# --- Build combined candidate list: explicit paths first, then planner ---
CANDIDATES=""
if [ -n "$EXPLICIT_FILES" ]; then
  CANDIDATES="$EXPLICIT_FILES"
fi
if [ -n "$PLAN_CANDIDATES" ]; then
  CANDIDATES=$(printf '%s\n%s' "$CANDIDATES" "$PLAN_CANDIDATES")
fi
CANDIDATES=$(printf '%s' "$CANDIDATES" | grep -v '^$' | awk '!seen[$0]++' || true)

# --- Stage 2: Semantic search ---
SEMANTIC_HITS=""
if [ -n "$CANDIDATES" ]; then
  FILES_CSV=$(printf '%s' "$CANDIDATES" | paste -sd "," -)
  SEMANTIC_HITS=$(rtk rgai "$TASK_HINT"     --path "$PROJECT_ROOT"     --files "$FILES_CSV"     --compact --max "$RTK_MEM_SEMANTIC_MAX" 2>/dev/null) || true
fi

# Fallback: unscoped search if scoped returned 0 hits
if [ -z "$SEMANTIC_HITS" ] || printf '%s' "$SEMANTIC_HITS" | grep -q '0 for '; then
  UNSCOPED_HITS=$(rtk rgai "$TASK_HINT"     --path "$PROJECT_ROOT"     --compact --max "$RTK_MEM_SEMANTIC_MAX" 2>/dev/null) || true
  if [ -n "$UNSCOPED_HITS" ]; then
    SEMANTIC_HITS="$UNSCOPED_HITS"
    # Extract file paths from unscoped results and add to candidates
    RGAI_FILES=$(printf '%s' "$UNSCOPED_HITS" | grep -E '^..' | sed -n 's/^..  \(\S*\) \[.*/\1/p' || true)
    if [ -n "$RGAI_FILES" ]; then
      CANDIDATES=$(printf '%s\n%s' "$CANDIDATES" "$RGAI_FILES" | grep -v '^$' | awk '!seen[$0]++')
    fi
  fi
fi

# --- Stage 3: Pre-read top files ---
PRE_READ_CONTENT=""
PRE_READ_COUNT=0
PRE_READ_TOTAL_BYTES=0

if [ -n "$CANDIDATES" ]; then
  while IFS= read -r file; do
    [ -z "$file" ] && continue
    [ "$PRE_READ_COUNT" -ge "$RTK_MEM_PRE_READ_COUNT" ] && break
    [ "$PRE_READ_TOTAL_BYTES" -ge "$RTK_MEM_PRE_READ_MAX_CHARS" ] && break

    filepath="$PROJECT_ROOT/$file"
    [ -f "$filepath" ] || continue

    # Size-adaptive: large files get header only, small files get aggressive filter
    fsize=$(wc -c < "$filepath" 2>/dev/null || echo 0)
    if [ "$fsize" -gt "$RTK_MEM_LARGE_FILE_THRESHOLD" ]; then
      content=$(rtk read "$filepath" --level none --from 1 --to "$RTK_MEM_PRE_READ_MAX_LINES" 2>/dev/null) || continue
    else
      content=$(rtk read "$filepath" --level aggressive 2>/dev/null) || continue
    fi
    [ -z "$content" ] && continue
    content=$(clamp_chars "$content" "$RTK_MEM_PRE_READ_FILE_MAX_CHARS")

    content_size=${#content}
    PRE_READ_TOTAL_BYTES=$((PRE_READ_TOTAL_BYTES + content_size))
    PRE_READ_CONTENT="${PRE_READ_CONTENT}
#### ${file}
${content}
"
    PRE_READ_COUNT=$((PRE_READ_COUNT + 1))
  done <<< "$CANDIDATES"
fi

GRAPH_SEEDS=$(clamp_chars "${GRAPH_SEEDS:-}" "$RTK_MEM_GRAPH_MAX_CHARS")
SEMANTIC_HITS=$(clamp_chars "${SEMANTIC_HITS:-}" "$RTK_MEM_SEMANTIC_MAX_CHARS")

# --- Assemble context ---
if [ -n "$CANDIDATES" ]; then
  MEM_CONTEXT="### Graph Seeds (structural context)
${GRAPH_SEEDS:-none}

### Semantic Hits
${SEMANTIC_HITS:-none}

### Pre-Read Files (${PRE_READ_COUNT} files, ${PRE_READ_TOTAL_BYTES} bytes)
${PRE_READ_CONTENT:-none}

### All Candidates
${CANDIDATES:-none}"
fi

# Fallback: structural explore when pipeline produces no candidates
if [ -z "$MEM_CONTEXT" ]; then
  MEM_CONTEXT=$(rtk memory explore "$PROJECT_ROOT" 2>/dev/null | head -c "$RTK_MEM_CONTEXT_MAX_CHARS") || true
fi

if [ -z "$MEM_CONTEXT" ]; then
  exit 0
fi

MEM_CONTEXT=$(clamp_chars "$MEM_CONTEXT" "$RTK_MEM_CONTEXT_MAX_CHARS")

# --- Inject mandatory RTK rules + pre-indexed context ---
NEW_PROMPT="## CRITICAL: All source code is provided below. DO NOT read files.

All files relevant to your task have been pre-read and included in this prompt.
You MUST analyze ONLY the code provided below. DO NOT call Read, Grep, or any file tools.
The ONLY tool calls you should make are Bash calls to write output files (rtk write).
If a file is not included below, it is not relevant to your task.

Any Read/Grep/Edit/Write tool calls are blocked by hooks and will waste tokens.
Use ONLY: Bash with rtk write to save your review documents.

## Pre-Read Source Code (analyze this, do not re-read)

${MEM_CONTEXT}

---

${CURRENT_PROMPT}"

ORIGINAL_INPUT=$(echo "$INPUT" | jq -c '.tool_input')
UPDATED_INPUT=$(echo "$ORIGINAL_INPUT" | jq --arg p "$NEW_PROMPT" '.prompt = $p')

jq -n   --argjson updated "$UPDATED_INPUT"   '{
    "hookSpecificOutput": {
      "hookEventName": "PreToolUse",
      "permissionDecision": "allow",
      "permissionDecisionReason": "RTK memory context injected (pre-read files included)",
      "updatedInput": $updated
    }
  }'
