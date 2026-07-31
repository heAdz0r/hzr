#!/usr/bin/env bash
#
# Memory-layer benchmark runner (CLI/API latency + context savings).
#
# Usage:
#   bash benchmarks/memory/bench_memory.sh
#
# Output:
#   benchmarks/memory/results_raw.csv
#   benchmarks/memory/results_env.txt
#
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CSV_OUT="$SCRIPT_DIR/results_raw.csv"
ENV_OUT="$SCRIPT_DIR/results_env.txt"

COLD_RUNS="${COLD_RUNS:-5}"
HOT_RUNS="${HOT_RUNS:-80}"
API_RUNS="${API_RUNS:-80}"
MEM_PORT="${MEM_PORT:-17770}"
RTK_BIN="${RTK_BIN:-$PROJECT_DIR/target/release/rtk}"
NATIVE_EXPLORE_TOKENS="${NATIVE_EXPLORE_TOKENS:-52000}"

echo "Building rtk release binary..."
(cd "$PROJECT_DIR" && cargo build --release --bin rtk)

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
BENCH_PROJECT="$WORK_DIR/memory-bench-project"

python3 - "$BENCH_PROJECT" <<'PY'
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
(root / "src").mkdir(parents=True, exist_ok=True)

for i in range(80):
    prev = f"module_{i-1}" if i > 0 else None
    body = [
        f"pub struct Type{i} {{ pub id: u32 }}",
        f"pub fn run_{i}() -> u32 {{ {i} }}",
    ]
    if prev:
        body.insert(0, f"use crate::{prev}::Type{i-1};")
        body.append(f"pub fn link_{i}(x: Type{i-1}) -> u32 {{ x.id + {i} }}")
    (root / "src" / f"module_{i}.rs").write_text("\n".join(body) + "\n", encoding="utf-8")

mods = [f"pub mod module_{i};" for i in range(80)]
mods.append("pub fn aggregate() -> u32 { (0..80).sum() }")
(root / "src" / "lib.rs").write_text("\n".join(mods) + "\n", encoding="utf-8")

(root / "Cargo.toml").write_text(
    "\n".join(
        [
            "[package]",
            'name = "memory_bench_project"',
            'version = "0.1.0"',
            'edition = "2021"',
        ]
    )
    + "\n",
    encoding="utf-8",
)
PY

{
  echo "Date: $(date -u)"
  echo "Commit: $(cd "$PROJECT_DIR" && git rev-parse HEAD)"
  echo "cold_runs: $COLD_RUNS"
  echo "hot_runs: $HOT_RUNS"
  echo "api_runs: $API_RUNS"
  echo "mem_port: $MEM_PORT"
  echo "rtk_bin: $RTK_BIN"
  echo "native_explore_tokens: $NATIVE_EXPLORE_TOKENS"
  echo "OS: $(uname -a)"
} > "$ENV_OUT"

echo "=== Environment ==="
cat "$ENV_OUT"
echo ""

echo "scenario,run,latency_ms,savings_pct,cache_hit" > "$CSV_OUT"

python3 - "$RTK_BIN" "$BENCH_PROJECT" "$CSV_OUT" "$COLD_RUNS" "$HOT_RUNS" "$API_RUNS" "$MEM_PORT" <<'PY'
from __future__ import annotations

import csv
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

rtk_bin = sys.argv[1]
project = Path(sys.argv[2])
csv_path = Path(sys.argv[3])
cold_runs = int(sys.argv[4])
hot_runs = int(sys.argv[5])
api_runs = int(sys.argv[6])
port = int(sys.argv[7])


def run_cmd(args: list[str]) -> str:
    res = subprocess.run(args, capture_output=True, text=True, check=True)
    return res.stdout


def run_explore_json() -> tuple[float, bool]:
    t0 = time.perf_counter()
    out = run_cmd(
        [
            rtk_bin,
            "memory",
            "explore",
            str(project),
            "--format",
            "json",
            "--detail",
            "compact",
            "--query-type",
            "general",
        ]
    )
    elapsed = (time.perf_counter() - t0) * 1000.0
    payload = json.loads(out)
    cache_hit = bool(payload.get("cache_hit", False))
    return elapsed, cache_hit


def clear_cache() -> None:
    run_cmd([rtk_bin, "memory", "clear", str(project)])


rows: list[dict[str, str]] = []

# CLI cold path
for run in range(1, cold_runs + 1):
    clear_cache()
    latency, cache_hit = run_explore_json()
    rows.append(
        {
            "scenario": "cli_cold",
            "run": str(run),
            "latency_ms": f"{latency:.3f}",
            "savings_pct": "",
            "cache_hit": "1" if cache_hit else "0",
        }
    )

# CLI hot path
run_explore_json()  # warm-up after final cold clear
for run in range(1, hot_runs + 1):
    latency, cache_hit = run_explore_json()
    rows.append(
        {
            "scenario": "cli_hot",
            "run": str(run),
            "latency_ms": f"{latency:.3f}",
            "savings_pct": "",
            "cache_hit": "1" if cache_hit else "0",
        }
    )

# Gain metric
gain_out = run_cmd([rtk_bin, "memory", "gain", str(project)])
match = re.search(r"savings:\s+([0-9]+(?:\.[0-9]+)?)%", gain_out)
savings_pct = float(match.group(1)) if match else 0.0
rows.append(
    {
        "scenario": "memory_gain",
        "run": "1",
        "latency_ms": "0.000",
        "savings_pct": f"{savings_pct:.2f}",
        "cache_hit": "",
    }
)

# API hot path
server = subprocess.Popen(
    [rtk_bin, "memory", "serve", "--port", str(port), "--idle-secs", "300"],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)

base_url = f"http://127.0.0.1:{port}"
health_url = f"{base_url}/v1/health"
explore_url = f"{base_url}/v1/explore"

try:
    ready = False
    for _ in range(120):
        try:
            with urllib.request.urlopen(health_url, timeout=1.0) as resp:
                if resp.status == 200:
                    ready = True
                    break
        except Exception:
            time.sleep(0.05)
    if not ready:
        raise RuntimeError("memory serve did not become healthy")

    body = json.dumps(
        {
            "project_root": str(project),
            "query_type": "general",
            "detail": "compact",
            "format": "json",
        }
    ).encode("utf-8")
    req = urllib.request.Request(
        explore_url,
        method="POST",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    # API warm-up
    with urllib.request.urlopen(req, timeout=5.0) as resp:
        if resp.status != 200:
            raise RuntimeError(f"warm-up status={resp.status}")

    for run in range(1, api_runs + 1):
        t0 = time.perf_counter()
        with urllib.request.urlopen(req, timeout=5.0) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
            if resp.status != 200:
                raise RuntimeError(f"api status={resp.status}")
        elapsed = (time.perf_counter() - t0) * 1000.0
        rows.append(
            {
                "scenario": "api_hot",
                "run": str(run),
                "latency_ms": f"{elapsed:.3f}",
                "savings_pct": "",
                "cache_hit": "1" if payload.get("cache_hit", False) else "0",
            }
        )
finally:
    server.terminate()
    try:
        server.wait(timeout=3.0)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait(timeout=3.0)

with csv_path.open("a", newline="", encoding="utf-8") as f:
    writer = csv.DictWriter(
        f,
        fieldnames=["scenario", "run", "latency_ms", "savings_pct", "cache_hit"],
    )
    writer.writerows(rows)

print(f"wrote rows={len(rows)} to {csv_path}")
PY

echo ""
echo "Benchmark complete."
echo "Raw CSV: $CSV_OUT"
echo "Env:     $ENV_OUT"
echo "Next:    python3 benchmarks/memory/analyze_memory.py"
