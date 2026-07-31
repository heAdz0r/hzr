#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import sys
from dataclasses import dataclass
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
CSV_PATH = SCRIPT_DIR / "results_raw.csv"
ENV_PATH = SCRIPT_DIR / "results_env.txt"
RESULTS_PATH = SCRIPT_DIR / "RESULTS.md"


@dataclass
class Totals:
    rows: int
    exit_match_rate: float
    side_effect_match_rate: float
    stderr_signal_match_rate: float


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Analyze git parity benchmark results")
    p.add_argument(
        "--fail-on-thresholds",
        action="store_true",
        help="Exit non-zero if threshold gates fail.",
    )
    return p.parse_args(argv)


def load_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def compute_totals(rows: list[dict[str, str]]) -> Totals:
    if not rows:
        return Totals(0, 0.0, 0.0, 0.0)

    def avg(col: str) -> float:
        return sum(float(r[col]) for r in rows) / len(rows) * 100.0

    return Totals(
        rows=len(rows),
        exit_match_rate=avg("exit_match"),
        side_effect_match_rate=avg("side_effect_match"),
        stderr_signal_match_rate=avg("stderr_signal_match"),
    )


def evaluate_thresholds(t: Totals) -> list[tuple[str, bool, str]]:
    return [
        ("exit_code_match_rate = 100%", t.exit_match_rate == 100.0, f"{t.exit_match_rate:.1f}%"),
        (
            "side_effect_match_rate = 100%",
            t.side_effect_match_rate == 100.0,
            f"{t.side_effect_match_rate:.1f}%",
        ),
        (
            "stderr_key_signal_match_rate >= 99%",
            t.stderr_signal_match_rate >= 99.0,
            f"{t.stderr_signal_match_rate:.1f}%",
        ),
    ]


def render(rows: list[dict[str, str]], totals: Totals, checks: list[tuple[str, bool, str]]) -> str:
    lines: list[str] = []
    lines.append("# Git Mutating Parity Benchmarks")
    lines.append("")
    lines.append("## Repro")
    lines.append("```bash")
    lines.append("bash benchmarks/parity_git/bench_parity_git.sh")
    lines.append("python3 benchmarks/parity_git/analyze_parity_git.py")
    lines.append("python3 -m unittest discover -s benchmarks/parity_git/tests -p 'test_*.py'")
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

    lines.append("## Aggregate")
    lines.append(f"- rows: {totals.rows}")
    lines.append(f"- exit_match_rate: {totals.exit_match_rate:.1f}%")
    lines.append(f"- side_effect_match_rate: {totals.side_effect_match_rate:.1f}%")
    lines.append(f"- stderr_signal_match_rate: {totals.stderr_signal_match_rate:.1f}%")
    lines.append("")

    lines.append("## Scenario Rows")
    lines.append("| scenario | kind | native_exit | rtk_exit | exit_match | side_effect_match | stderr_signal_match |")
    lines.append("|---|---|---:|---:|---:|---:|---:|")
    for r in rows:
        lines.append(
            "| {scenario} | {kind} | {native_exit} | {rtk_exit} | {exit_match} | {side_effect_match} | {stderr_signal_match} |".format(
                **r
            )
        )
    lines.append("")

    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)

    if not CSV_PATH.exists():
        print(f"ERROR: {CSV_PATH} not found. Run bench_parity_git.sh first.", file=sys.stderr)
        return 1

    rows = load_rows(CSV_PATH)
    totals = compute_totals(rows)
    checks = evaluate_thresholds(totals)

    RESULTS_PATH.write_text(render(rows, totals, checks), encoding="utf-8")
    print(f"Wrote {RESULTS_PATH}")
    print(f"Rows: {totals.rows}")
    print(f"Checks: {len(checks)}")

    if args.fail_on_thresholds and any(not ok for _, ok, _ in checks):
        print("Threshold check failure detected.", file=sys.stderr)
        return 2

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
