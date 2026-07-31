#!/usr/bin/env python3
"""
Analyze write benchmark results and generate RESULTS.md.
"""

from __future__ import annotations

import argparse
import csv
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
CSV_PATH = SCRIPT_DIR / "results_raw.csv"
ENV_PATH = SCRIPT_DIR / "results_env.txt"
RESULTS_PATH = SCRIPT_DIR / "RESULTS.md"
EXPECTED_THRESHOLD_CHECKS = 8


@dataclass
class GroupStats:
    scenario: str
    size_label: str
    file_size: int
    tool: str
    mode: str
    runs: int
    latency_p50_ms: float
    latency_p95_ms: float
    throughput_mib_s: float
    write_amplification: float
    avg_fsync_count: float
    avg_rename_count: float
    skip_rate: float


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


def compute_groups(rows: list[dict]) -> dict[tuple[str, str, str, str], GroupStats]:
    groups: dict[tuple[str, str, str, str], list[dict]] = {}
    for row in rows:
        key = (row["scenario"], row["size_label"], row["tool"], row["mode"])
        groups.setdefault(key, []).append(row)

    out: dict[tuple[str, str, str, str], GroupStats] = {}
    for key, g_rows in groups.items():
        lat_us = [float(r["latency_us"]) for r in g_rows]
        file_size = int(g_rows[0]["file_size"])
        bytes_written = [float(r["bytes_written"]) for r in g_rows]
        fsync_count = [float(r["fsync_count"]) for r in g_rows]
        rename_count = [float(r["rename_count"]) for r in g_rows]
        skipped = [float(r["skipped_unchanged"]) for r in g_rows]

        p50_us = percentile(lat_us, 0.50)
        p95_us = percentile(lat_us, 0.95)
        throughput_mib_s = 0.0
        if p50_us > 0 and statistics.mean(bytes_written) > 0:
            throughput_mib_s = (file_size / (1024 * 1024)) / (p50_us / 1_000_000)

        write_amp = 0.0
        if file_size > 0:
            write_amp = statistics.mean(bytes_written) / file_size

        out[key] = GroupStats(
            scenario=key[0],
            size_label=key[1],
            file_size=file_size,
            tool=key[2],
            mode=key[3],
            runs=len(g_rows),
            latency_p50_ms=p50_us / 1000.0,
            latency_p95_ms=p95_us / 1000.0,
            throughput_mib_s=throughput_mib_s,
            write_amplification=write_amp,
            avg_fsync_count=statistics.mean(fsync_count),
            avg_rename_count=statistics.mean(rename_count),
            skip_rate=statistics.mean(skipped),
        )

    return out


def evaluate_thresholds(groups: dict[tuple[str, str, str, str], GroupStats]) -> list[tuple[str, bool, str]]:
    checks: list[tuple[str, bool, str]] = []

    # unchanged path target: < 2ms p50 for small/medium
    for size in ("small", "medium"):
        for mode in ("durable", "fast"):
            k = ("unchanged", size, "write_core", mode)
            if k in groups:
                p50 = groups[k].latency_p50_ms
                checks.append(
                    (
                        f"unchanged p50 < 2ms ({size}, {mode})",
                        p50 < 2.0,
                        f"p50={p50:.3f}ms",
                    )
                )

    # changed durable path <= 1.25x native_safe baseline
    for size in ("small", "medium", "large"):
        kd = ("changed", size, "write_core", "durable")
        kn = ("changed", size, "native_safe", "durable")
        if kd in groups and kn in groups and groups[kn].latency_p50_ms > 0:
            ratio = groups[kd].latency_p50_ms / groups[kn].latency_p50_ms
            checks.append(
                (
                    f"durable <= 1.25x native ({size})",
                    ratio <= 1.25,
                    f"ratio={ratio:.3f}x",
                )
            )

    # fast mode should be faster than durable on small changed writes
    kf = ("changed", "small", "write_core", "fast")
    kd = ("changed", "small", "write_core", "durable")
    if kf in groups and kd in groups:
        checks.append(
            (
                "fast < durable (changed, small)",
                groups[kf].latency_p50_ms < groups[kd].latency_p50_ms,
                f"fast={groups[kf].latency_p50_ms:.3f}ms durable={groups[kd].latency_p50_ms:.3f}ms",
            )
        )

    return checks


def render_results(groups: dict[tuple[str, str, str, str], GroupStats], checks: list[tuple[str, bool, str]]) -> str:
    lines: list[str] = []
    lines.append("# Write Benchmarks")
    lines.append("")
    lines.append("## Repro")
    lines.append("```bash")
    lines.append("bash benchmarks/write/bench_write.sh")
    lines.append("python3 benchmarks/write/analyze_write.py")
    lines.append("python3 -m unittest discover -s benchmarks/write/tests -p 'test_*.py'")
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
    lines.append("| scenario | size | tool | mode | p50 ms | p95 ms | MiB/s | write_amp | fsync | rename | skip_rate |")
    lines.append("|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|")

    for key in sorted(groups.keys()):
        g = groups[key]
        lines.append(
            f"| {g.scenario} | {g.size_label} | {g.tool} | {g.mode} | "
            f"{g.latency_p50_ms:.3f} | {g.latency_p95_ms:.3f} | {g.throughput_mib_s:.2f} | "
            f"{g.write_amplification:.2f} | {g.avg_fsync_count:.2f} | {g.avg_rename_count:.2f} | {g.skip_rate:.2f} |"
        )

    lines.append("")
    return "\n".join(lines)


def has_failed_checks(checks: list[tuple[str, bool, str]]) -> bool:
    return any(not ok for _, ok, _ in checks)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Analyze write benchmark CSV and enforce threshold gates.")
    parser.add_argument(
        "--fail-on-thresholds",
        action="store_true",
        help="Exit with non-zero status if any threshold gate fails.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if not CSV_PATH.exists():
        print(f"ERROR: {CSV_PATH} not found. Run bench_write.sh first.", file=sys.stderr)
        return 1

    rows = load_rows(CSV_PATH)
    groups = compute_groups(rows)
    checks = evaluate_thresholds(groups)

    RESULTS_PATH.write_text(render_results(groups, checks), encoding="utf-8")
    print(f"Wrote {RESULTS_PATH}")
    print(f"Groups: {len(groups)}")
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
