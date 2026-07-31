#!/usr/bin/env python3
"""
Analyze memory benchmark results and generate RESULTS.md.
"""

from __future__ import annotations

import argparse
import csv
import re
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
CSV_PATH = SCRIPT_DIR / "results_raw.csv"
ENV_PATH = SCRIPT_DIR / "results_env.txt"
RESULTS_PATH = SCRIPT_DIR / "RESULTS.md"
EXPECTED_THRESHOLD_CHECKS = 7
DEFAULT_NATIVE_EXPLORE_TOKENS = 52_000
FOLLOWUP_STEPS = (1, 3, 5)


@dataclass
class ScenarioStats:
    scenario: str
    runs: int
    latency_p50_ms: float
    latency_p95_ms: float
    latency_p99_ms: float
    cache_hit_rate: float


@dataclass
class TokenProjection:
    steps: int
    native_tokens: int
    memory_tokens: int
    saved_tokens: int
    savings_pct: float


def percentile(values: list[float], p: float) -> float:
    if not values:
        return 0.0
    if len(values) == 1:
        return float(values[0])
    vals = sorted(values)
    idx = (len(vals) - 1) * p
    lo = int(idx)
    hi = min(lo + 1, len(vals) - 1)
    frac = idx - lo
    return float(vals[lo] * (1 - frac) + vals[hi] * frac)


def load_rows(path: Path) -> list[dict]:
    with path.open(newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def compute_stats(rows: list[dict]) -> tuple[dict[str, ScenarioStats], float]:
    by_scenario: dict[str, list[dict]] = {}
    savings_pct = 0.0
    for row in rows:
        scenario = row["scenario"]
        if scenario == "memory_gain":
            if row.get("savings_pct"):
                savings_pct = float(row["savings_pct"])
            continue
        by_scenario.setdefault(scenario, []).append(row)

    out: dict[str, ScenarioStats] = {}
    for scenario, s_rows in by_scenario.items():
        lat = [float(r["latency_ms"]) for r in s_rows]
        hits = [
            float(r["cache_hit"])
            for r in s_rows
            if r.get("cache_hit") not in (None, "", " ")
        ]
        hit_rate = statistics.mean(hits) if hits else 0.0
        out[scenario] = ScenarioStats(
            scenario=scenario,
            runs=len(s_rows),
            latency_p50_ms=percentile(lat, 0.50),
            latency_p95_ms=percentile(lat, 0.95),
            latency_p99_ms=percentile(lat, 0.99),
            cache_hit_rate=hit_rate,
        )
    return out, savings_pct


def load_native_explore_tokens(path: Path) -> int:
    if not path.exists():
        return DEFAULT_NATIVE_EXPLORE_TOKENS
    text = path.read_text(encoding="utf-8")
    match = re.search(r"^native_explore_tokens:\s*(\d+)\s*$", text, flags=re.MULTILINE)
    if not match:
        return DEFAULT_NATIVE_EXPLORE_TOKENS
    value = int(match.group(1))
    return value if value > 0 else DEFAULT_NATIVE_EXPLORE_TOKENS


def build_token_projections(native_tokens: int, savings_pct: float) -> list[TokenProjection]:
    memory_per_explore = max(0.0, native_tokens * (1.0 - savings_pct / 100.0))
    rows: list[TokenProjection] = []
    for steps in FOLLOWUP_STEPS:
        native_total = native_tokens * steps
        memory_total = int(round(memory_per_explore * steps))
        saved_total = native_total - memory_total
        pct = (saved_total / native_total * 100.0) if native_total > 0 else 0.0
        rows.append(
            TokenProjection(
                steps=steps,
                native_tokens=native_total,
                memory_tokens=memory_total,
                saved_tokens=saved_total,
                savings_pct=pct,
            )
        )
    return rows


def evaluate_thresholds(
    stats: dict[str, ScenarioStats], savings_pct: float, native_tokens: int
) -> list[tuple[str, bool, str]]:
    checks: list[tuple[str, bool, str]] = []

    if "cli_hot" in stats:
        cli_hot = stats["cli_hot"]
        checks.append(
            (
                "cli hot p95 < 200ms",
                cli_hot.latency_p95_ms < 200.0,
                f"p95={cli_hot.latency_p95_ms:.2f}ms",
            )
        )
        checks.append(
            (
                "cli hot cache-hit rate >= 0.95",
                cli_hot.cache_hit_rate >= 0.95,
                f"rate={cli_hot.cache_hit_rate:.2f}",
            )
        )

    if "api_hot" in stats:
        api_hot = stats["api_hot"]
        checks.append(
            (
                "api hot p95 < 200ms",
                api_hot.latency_p95_ms < 200.0,
                f"p95={api_hot.latency_p95_ms:.2f}ms",
            )
        )

    if "cli_hot" in stats and "cli_cold" in stats:
        checks.append(
            (
                "cli hot p50 < cli cold p50",
                stats["cli_hot"].latency_p50_ms < stats["cli_cold"].latency_p50_ms,
                f"hot={stats['cli_hot'].latency_p50_ms:.2f}ms cold={stats['cli_cold'].latency_p50_ms:.2f}ms",
            )
        )

    checks.append(
        (
            "memory gain savings >= 50%",
            savings_pct >= 50.0,
            f"savings={savings_pct:.2f}%",
        )
    )
    estimated_memory_tokens = native_tokens * (1.0 - savings_pct / 100.0)
    checks.append(
        (
            "estimated memory tokens <= 50% of native explore baseline",
            estimated_memory_tokens <= native_tokens * 0.5,
            f"native={native_tokens} est={estimated_memory_tokens:.0f}",
        )
    )
    five_step_saved = (native_tokens - estimated_memory_tokens) * 5.0
    checks.append(
        (
            "5-step cumulative savings >= 1x native explore baseline",
            five_step_saved >= native_tokens,
            f"saved_5={five_step_saved:.0f} native={native_tokens}",
        )
    )
    return checks


def render_results(
    stats: dict[str, ScenarioStats],
    savings_pct: float,
    native_tokens: int,
    checks: list[tuple[str, bool, str]],
) -> str:
    projections = build_token_projections(native_tokens, savings_pct)
    estimated_memory_tokens = max(0.0, native_tokens * (1.0 - savings_pct / 100.0))
    saved_per_explore = native_tokens - estimated_memory_tokens

    lines: list[str] = []
    lines.append("# Memory Layer Benchmarks")
    lines.append("")
    lines.append("## Repro")
    lines.append("```bash")
    lines.append("bash benchmarks/memory/bench_memory.sh")
    lines.append("python3 benchmarks/memory/analyze_memory.py")
    lines.append("python3 -m unittest discover -s benchmarks/memory/tests -p 'test_*.py'")
    lines.append("```")
    lines.append("")

    if ENV_PATH.exists():
        lines.append("## Environment")
        lines.append("```text")
        lines.append(ENV_PATH.read_text(encoding="utf-8").strip())
        lines.append("```")
        lines.append("")

    lines.append("## Threshold Gates")
    for name, ok, detail in checks:
        mark = "PASS" if ok else "FAIL"
        lines.append(f"- [{mark}] {name} ({detail})")
    lines.append("")

    lines.append("## Metrics")
    lines.append("| scenario | runs | p50 ms | p95 ms | p99 ms | cache_hit_rate |")
    lines.append("|---|---:|---:|---:|---:|---:|")
    for key in sorted(stats.keys()):
        s = stats[key]
        lines.append(
            f"| {s.scenario} | {s.runs} | {s.latency_p50_ms:.3f} | {s.latency_p95_ms:.3f} | {s.latency_p99_ms:.3f} | {s.cache_hit_rate:.2f} |"
        )
    lines.append("")
    lines.append(f"memory_gain savings: **{savings_pct:.2f}%**")
    lines.append("")
    lines.append("## Native Explore Baseline Projection")
    lines.append(
        f"Assumed native Task/Explore baseline: **{native_tokens} tokens** per run (override via `NATIVE_EXPLORE_TOKENS`)."
    )
    lines.append(
        f"Estimated memory-context cost per run: **{estimated_memory_tokens:.0f} tokens** (saved: **{saved_per_explore:.0f}**)."
    )
    lines.append("")
    lines.append("| explore-driven steps | native tokens | memory tokens (est) | saved tokens | savings % |")
    lines.append("|---:|---:|---:|---:|---:|")
    for row in projections:
        lines.append(
            f"| {row.steps} | {row.native_tokens} | {row.memory_tokens} | {row.saved_tokens} | {row.savings_pct:.2f}% |"
        )
    lines.append("")
    return "\n".join(lines)


def has_failed_checks(checks: list[tuple[str, bool, str]]) -> bool:
    return any(not ok for _, ok, _ in checks)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Analyze memory benchmark CSV and enforce threshold gates."
    )
    parser.add_argument(
        "--fail-on-thresholds",
        action="store_true",
        help="Exit with non-zero status if any threshold gate fails.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if not CSV_PATH.exists():
        print(f"ERROR: {CSV_PATH} not found. Run bench_memory.sh first.", file=sys.stderr)
        return 1

    rows = load_rows(CSV_PATH)
    stats, savings_pct = compute_stats(rows)
    native_tokens = load_native_explore_tokens(ENV_PATH)
    checks = evaluate_thresholds(stats, savings_pct, native_tokens)

    RESULTS_PATH.write_text(
        render_results(stats, savings_pct, native_tokens, checks), encoding="utf-8"
    )
    print(f"Wrote {RESULTS_PATH}")
    print(f"Scenarios: {len(stats)}")
    print(f"Checks: {len(checks)}")

    if args.fail_on_thresholds and len(checks) < EXPECTED_THRESHOLD_CHECKS:
        print(
            f"Incomplete threshold dataset: expected >= {EXPECTED_THRESHOLD_CHECKS} checks, got {len(checks)}.",
            file=sys.stderr,
        )
        return 3
    if args.fail_on_thresholds and has_failed_checks(checks):
        print("Threshold check failure detected.", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
