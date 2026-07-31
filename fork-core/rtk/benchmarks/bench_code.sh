#!/usr/bin/env bash
#
# Benchmark runner: grep vs rtk grep vs rtk rgai on rtk codebase
#
# Usage:
#   bash benchmarks/bench_code.sh
#
# Output:
#   benchmarks/results_raw.csv    — raw measurements (30 queries × 4 tools × 5 runs)  # CHANGED: 4 tools
#   benchmarks/results_env.txt    — environment snapshot
#   benchmarks/quality_samples/   — first-run full output samples (no truncation)
#   benchmarks/gold_auto.json     — auto-generated gold files from grep output
#
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SRC_DIR="$PROJECT_DIR/src"
GOLD_PATH="$SCRIPT_DIR/gold_standards.json"
CSV_OUT="$SCRIPT_DIR/results_raw.csv"
ENV_OUT="$SCRIPT_DIR/results_env.txt"
QUALITY_DIR="$SCRIPT_DIR/quality_samples"
GOLD_AUTO="$SCRIPT_DIR/gold_auto.json"  # ADDED: auto-generated gold

RUNS=5
HEAD_N_LINES="${HEAD_N_LINES:-100}"  # ADDED: negative control truncation threshold
TOKENIZER_ENCODING="${TOKENIZER_ENCODING:-cl100k_base}"
RTK_BIN="${RTK_BIN:-$PROJECT_DIR/target/release/rtk}"
ALLOW_DIRTY="${ALLOW_DIRTY:-0}"
RTK_GREP_MAX="${RTK_GREP_MAX:-200}"
RGAI_MAX="${RGAI_MAX:-8}"

# ── Pre-flight checks ──────────────────────────────────────────────────── #

if [ ! -d "$SRC_DIR" ]; then
    echo "ERROR: src/ directory not found at $SRC_DIR" >&2
    exit 1
fi

if [ ! -f "$GOLD_PATH" ]; then
    echo "ERROR: gold_standards.json not found at $GOLD_PATH" >&2
    exit 1
fi

if [ ! -x "$RTK_BIN" ]; then
    echo "ERROR: rtk binary not found or not executable at $RTK_BIN" >&2
    echo "Build local binary first: cargo build --release" >&2
    exit 1
fi

if ! python3 - "$TOKENIZER_ENCODING" <<'PY'
import sys
import tiktoken

enc = sys.argv[1]
tiktoken.get_encoding(enc)
PY
then
    echo "ERROR: Python package 'tiktoken' is required for token-based TE." >&2
    echo "Install it with: python3 -m pip install tiktoken" >&2
    exit 1
fi

# ── Pin commit for reproducibility ──────────────────────────────────────── #

EXPECTED_COMMIT="$(python3 -c "import json;print(json.load(open('$GOLD_PATH', encoding='utf-8'))['metadata']['pinned_commit'])")"
PINNED_COMMIT="$(cd "$PROJECT_DIR" && git rev-parse HEAD)"
if [ "$PINNED_COMMIT" != "$EXPECTED_COMMIT" ]; then
    echo "ERROR: Current HEAD ($PINNED_COMMIT) does not match pinned commit in gold_standards.json ($EXPECTED_COMMIT)." >&2
    echo "Checkout pinned commit first for reproducible results." >&2
    exit 2
fi

echo "Pinned commit: $PINNED_COMMIT"

if [ "$ALLOW_DIRTY" != "1" ]; then
    if ! (cd "$PROJECT_DIR" && git diff --quiet -- src Cargo.toml Cargo.lock && git diff --cached --quiet -- src Cargo.toml Cargo.lock); then
        echo "ERROR: Working tree has local changes in benchmarked sources (src/, Cargo.toml, Cargo.lock)." >&2
        echo "Commit/stash changes for auditable reproducibility, or set ALLOW_DIRTY=1 to override." >&2
        exit 3
    fi
    if [ -n "$(cd "$PROJECT_DIR" && git ls-files --others --exclude-standard -- src)" ]; then
        echo "ERROR: Untracked files exist under src/; benchmark dataset is not clean." >&2
        echo "Commit/remove untracked source files, or set ALLOW_DIRTY=1 to override." >&2
        exit 3
    fi
fi

# ── Environment snapshot ────────────────────────────────────────────────── #

{
    echo "Date: $(date -u)"
    echo "Commit: $PINNED_COMMIT"
    echo "rtk_bin: $RTK_BIN"
    echo "rtk: $($RTK_BIN --version 2>&1 || echo 'N/A')"
    echo "grep: $(grep --version 2>&1 | head -1 || echo 'N/A')"
    echo "tiktoken_encoding: $TOKENIZER_ENCODING"
    echo "rtk_grep_max: $RTK_GREP_MAX"
    echo "rgai_max: $RGAI_MAX"
    echo "head_n_lines: $HEAD_N_LINES"  # ADDED: negative control param
    echo "OS: $(uname -a)"
    if [[ "$(uname)" == "Darwin" ]]; then
        echo "CPU: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo 'N/A')"
    else
        echo "CPU: $(lscpu 2>/dev/null | grep 'Model name' | sed 's/.*: *//' || echo 'N/A')"
    fi
    echo "Rust files: $(find "$SRC_DIR" -name '*.rs' | wc -l | tr -d ' ')"
    echo "Total LOC: $(find "$SRC_DIR" -name '*.rs' -exec cat {} + | wc -l | tr -d ' ')"
} > "$ENV_OUT"

echo "=== Environment ==="
cat "$ENV_OUT"
echo ""

# ── Warmup (populate OS page cache) ────────────────────────────────────── #

echo "Warming up filesystem cache..."
find "$SRC_DIR" -name '*.rs' -exec cat {} + > /dev/null 2>&1
echo ""

# ── Command runner helper ───────────────────────────────────────────────── #
# Prints: time_us,output_bytes,output_tokens,result_count,exit_code

count_tokens_tiktoken() {
    local out_file="$1"
    python3 - "$out_file" "$TOKENIZER_ENCODING" <<'PY'
import sys
from pathlib import Path
import tiktoken

path = Path(sys.argv[1])
encoding_name = sys.argv[2]
enc = tiktoken.get_encoding(encoding_name)
text = path.read_text(encoding="utf-8", errors="replace")
print(len(enc.encode(text)))
PY
}

run_command_capture() {
    local out_file="$1"
    shift

    local time_file elapsed_s time_us bytes tokens lines exit_code
    time_file="$(mktemp)"

    TIMEFORMAT='%R'
    set +e
    { time "$@" > "$out_file" 2>/dev/null; } 2> "$time_file"
    exit_code=$?
    set -e

    elapsed_s="$(tr -d ' \t\r\n' < "$time_file")"
    rm -f "$time_file"

    if [[ "$elapsed_s" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
        time_us="$(awk -v s="$elapsed_s" 'BEGIN { printf "%.0f", s * 1000000 }')"
    else
        time_us=0
    fi

    bytes=$(wc -c < "$out_file" | tr -d ' ')
    tokens=$(count_tokens_tiktoken "$out_file")
    lines=$(wc -l < "$out_file" | tr -d ' ')
    echo "${time_us},${bytes},${tokens},${lines},${exit_code}"
}

# ── Quality sample capture ──────────────────────────────────────────────── #

mkdir -p "$QUALITY_DIR"
rm -f "$QUALITY_DIR"/*.txt

# ── Test matrix ─────────────────────────────────────────────────────────── #
# Fields: test_id  category  query  grep_flags

declare -a TEST_IDS=()
declare -a TEST_CATEGORIES=()
declare -a TEST_QUERIES=()
declare -a TEST_GREP_FLAGS=()

add_test() {
    TEST_IDS+=("$1")
    TEST_CATEGORIES+=("$2")
    TEST_QUERIES+=("$3")
    TEST_GREP_FLAGS+=("$4")
}

# Category A: Exact Identifier
add_test "A1" "exact_identifier" "TimedExecution"       ""
add_test "A2" "exact_identifier" "FilterLevel"          ""
add_test "A3" "exact_identifier" "classify_command"     ""
add_test "A4" "exact_identifier" "package_manager_exec" ""
add_test "A5" "exact_identifier" "strip_ansi"           ""
add_test "A6" "exact_identifier" "HISTORY_DAYS"         ""

# Category B: Regex Pattern
add_test "B1" "regex_pattern" 'fn run\(.*verbose: u8'   "-E"
add_test "B2" "regex_pattern" 'timer\.track\('          "-E"
add_test "B3" "regex_pattern" '\.unwrap_or\(1\)'        "-E"
add_test "B4" "regex_pattern" '#\[cfg\(test\)\]'        "-E"
add_test "B5" "regex_pattern" 'HashMap<String, Vec<'    "-E"
add_test "B6" "regex_pattern" 'lazy_static!'            ""

# Category C: Semantic Intent
add_test "C1"  "semantic_intent" "token savings tracking database"    ""
add_test "C2"  "semantic_intent" "exit code preservation"             ""
add_test "C3"  "semantic_intent" "language aware code filtering"      ""
add_test "C4"  "semantic_intent" "output grouping by file"            ""
add_test "C5"  "semantic_intent" "three tier parser degradation"      ""
add_test "C6"  "semantic_intent" "ANSI color stripping cleanup"       ""
add_test "C7"  "semantic_intent" "hook installation settings json"    ""
add_test "C8"  "semantic_intent" "command classification discover"    ""
add_test "C9"  "semantic_intent" "pnpm yarn npm auto detection"       ""
add_test "C10" "semantic_intent" "SQLite retention cleanup policy"    ""

# Category D: Cross-File Pattern Discovery
add_test "D1" "cross_file" "verbose > 0"      ""
add_test "D2" "cross_file" "anyhow::Result"   ""
add_test "D3" "cross_file" "process::exit"    ""
add_test "D4" "cross_file" "Command::new"     ""
add_test "D5" "cross_file" "from_utf8_lossy"  ""

# Category E: Edge Cases
add_test "E1" "edge_case" "the"                          ""
add_test "E2" "edge_case" "fn"                           ""
add_test "E3" "edge_case" "error handling retry backoff" ""

# ── CSV header ──────────────────────────────────────────────────────────── #

echo "test_id,category,query,tool,run,time_us,output_bytes,output_tokens,result_count,exit_code" > "$CSV_OUT"

# ── Run matrix ──────────────────────────────────────────────────────────── #

NUM_TESTS=${#TEST_IDS[@]}
echo "Running $NUM_TESTS tests × 4 tools × $RUNS runs = $(( NUM_TESTS * 4 * RUNS )) measurements"  # CHANGED: 4 tools
echo ""

for idx in $(seq 0 $(( NUM_TESTS - 1 ))); do
    tid="${TEST_IDS[$idx]}"
    category="${TEST_CATEGORIES[$idx]}"
    query="${TEST_QUERIES[$idx]}"
    grep_flags="${TEST_GREP_FLAGS[$idx]}"

    echo "[$tid] query=\"$query\""

    for run in $(seq 1 $RUNS); do
        tmp_grep="$(mktemp)"
        tmp_rtk_grep="$(mktemp)"
        tmp_rtk_rgai="$(mktemp)"
        tmp_head_n="$(mktemp)"  # ADDED: negative control

        grep_flag_arr=()
        if [ -n "$grep_flags" ]; then
            read -r -a grep_flag_arr <<< "$grep_flags"
        fi

        # grep
        grep_cmd=(grep -rn "${grep_flag_arr[@]}" -- "$query" "$SRC_DIR")
        IFS=',' read -r grep_time grep_bytes grep_tokens grep_lines grep_exit < <(run_command_capture "$tmp_grep" "${grep_cmd[@]}")
        echo "$tid,$category,\"$query\",grep,$run,$grep_time,$grep_bytes,$grep_tokens,$grep_lines,$grep_exit" >> "$CSV_OUT"

        if [ "$run" -eq 1 ]; then
            cp "$tmp_grep" "$QUALITY_DIR/${tid}_grep.txt" 2>/dev/null || true
        fi

        # rtk grep
        rtk_grep_cmd=("$RTK_BIN" grep "$query" "$SRC_DIR" --max "$RTK_GREP_MAX")
        IFS=',' read -r rtk_grep_time rtk_grep_bytes rtk_grep_tokens rtk_grep_lines rtk_grep_exit < <(run_command_capture "$tmp_rtk_grep" "${rtk_grep_cmd[@]}")
        echo "$tid,$category,\"$query\",rtk_grep,$run,$rtk_grep_time,$rtk_grep_bytes,$rtk_grep_tokens,$rtk_grep_lines,$rtk_grep_exit" >> "$CSV_OUT"

        if [ "$run" -eq 1 ]; then
            cp "$tmp_rtk_grep" "$QUALITY_DIR/${tid}_rtk_grep.txt" 2>/dev/null || true
        fi

        # rtk rgai
        rtk_rgai_cmd=("$RTK_BIN" rgai --path "$SRC_DIR" --max "$RGAI_MAX" -- "$query")
        IFS=',' read -r rtk_rgai_time rtk_rgai_bytes rtk_rgai_tokens rtk_rgai_lines rtk_rgai_exit < <(run_command_capture "$tmp_rtk_rgai" "${rtk_rgai_cmd[@]}")
        echo "$tid,$category,\"$query\",rtk_rgai,$run,$rtk_rgai_time,$rtk_rgai_bytes,$rtk_rgai_tokens,$rtk_rgai_lines,$rtk_rgai_exit" >> "$CSV_OUT"

        if [ "$run" -eq 1 ]; then
            cp "$tmp_rtk_rgai" "$QUALITY_DIR/${tid}_rtk_rgai.txt" 2>/dev/null || true
        fi

        # head_n (NEGATIVE CONTROL) ──────────────────────────────────  # ADDED: entire section
        # Naive truncation baseline: just take first N lines of grep output
        head -n "$HEAD_N_LINES" "$tmp_grep" > "$tmp_head_n" 2>/dev/null || true
        head_n_tokens=$(count_tokens_tiktoken "$tmp_head_n")
        head_n_bytes=$(wc -c < "$tmp_head_n" | tr -d ' ')
        head_n_lines=$(wc -l < "$tmp_head_n" | tr -d ' ')
        # Timing is negligible for head, use 0
        echo "$tid,$category,\"$query\",head_n,$run,0,$head_n_bytes,$head_n_tokens,$head_n_lines,0" >> "$CSV_OUT"

        if [ "$run" -eq 1 ]; then
            cp "$tmp_head_n" "$QUALITY_DIR/${tid}_head_n.txt" 2>/dev/null || true
        fi

        rm -f "$tmp_grep" "$tmp_rtk_grep" "$tmp_rtk_rgai" "$tmp_head_n"  # CHANGED: added tmp_head_n
    done
    echo "  done ($RUNS runs)"
done

echo ""
echo "=== Generating Auto Gold Standards ==="  # ADDED: entire section

# Generate gold_auto.json from grep output (automatic verification)
python3 - "$QUALITY_DIR" "$GOLD_AUTO" "$PINNED_COMMIT" << 'PYEOF'
import json
import re
import sys
from pathlib import Path

quality_dir = Path(sys.argv[1])
output_path = Path(sys.argv[2])
pinned_commit = sys.argv[3]

def extract_rs_files(text: str) -> list[str]:
    """Extract unique .rs filenames from grep output."""
    files = set()
    for match in re.finditer(r"([A-Za-z0-9_./-]+\.rs)", text):
        path = match.group(1)
        # Normalize: strip src/ prefix, keep nested paths
        if "/src/" in path:
            path = path.split("/src/", 1)[1]
        elif path.startswith("src/"):
            path = path[4:]
        path = path.lstrip("./")
        if path.endswith(".rs"):
            files.add(path)
    return sorted(files)

gold_auto = {
    "metadata": {
        "description": "Auto-generated gold standards from grep output",
        "pinned_commit": pinned_commit,
        "generated": "auto",
        "notes": "Gold files extracted automatically from grep results - no manual curation"
    },
    "queries": {}
}

# Process each grep sample
for grep_file in sorted(quality_dir.glob("*_grep.txt")):
    tid = grep_file.stem.replace("_grep", "")
    text = grep_file.read_text(errors="replace")
    gold_files = extract_rs_files(text)

    gold_auto["queries"][tid] = {
        "gold_files_auto": gold_files,
        "gold_file_count": len(gold_files),
        "grep_lines": len(text.splitlines()),
        "grep_bytes": len(text.encode("utf-8"))
    }

output_path.write_text(json.dumps(gold_auto, indent=2), encoding="utf-8")
print(f"Generated {output_path} with {len(gold_auto['queries'])} queries")
PYEOF

echo ""
echo "=== Benchmark Complete ==="
echo "Raw results:      $CSV_OUT"
echo "Quality samples:  $QUALITY_DIR/"
echo "Auto gold:        $GOLD_AUTO"  # ADDED
echo "Environment:      $ENV_OUT"
echo ""
echo "Total measurements: $(( $(wc -l < "$CSV_OUT") - 1 ))"
echo ""
echo "Next step: python3 benchmarks/analyze_code.py"
