#!/usr/bin/env python3
"""
Analyze code-search benchmark results and generate RESULTS.md.

Rules:
  - No composite score.
  - Per-category analysis only.
  - Report distributions (min/median/max).
  - If gold expects results and result_count == 0 => MISS.
  - Regex category: rgai is EXPECTED_UNSUPPORTED (not failure).
"""

from __future__ import annotations

import csv
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
CSV_PATH = SCRIPT_DIR / "results_raw.csv"
ENV_PATH = SCRIPT_DIR / "results_env.txt"
GOLD_PATH = SCRIPT_DIR / "gold_standards.json"
GOLD_AUTO_PATH = SCRIPT_DIR / "gold_auto.json"  # ADDED: auto-generated gold
QUALITY_DIR = SCRIPT_DIR / "quality_samples"
RESULTS_PATH = SCRIPT_DIR / "RESULTS.md"

TOOLS = ("grep", "rtk_grep", "rtk_rgai", "head_n")  # CHANGED: added head_n
RECOMMENDABLE_TOOLS = ("grep", "rtk_grep", "rtk_rgai")
CATEGORY_ORDER = [
    "exact_identifier",
    "regex_pattern",
    "semantic_intent",
    "cross_file",
    "edge_case",
]
CATEGORY_TITLES = {
    "exact_identifier": "Category A: Exact Identifier Search",
    "regex_pattern": "Category B: Regex Pattern Search",
    "semantic_intent": "Category C: Semantic Intent Search",
    "cross_file": "Category D: Cross-File Pattern Discovery",
    "edge_case": "Category E: Edge Cases",
}


def median_val(values: list[int | float]) -> float:
    vals = sorted(values)
    if not vals:
        return 0.0
    n = len(vals)
    if n % 2 == 1:
        return float(vals[n // 2])
    return (vals[n // 2 - 1] + vals[n // 2]) / 2.0


def min_val(values: list[int | float]) -> float:
    return float(min(values)) if values else 0.0


def max_val(values: list[int | float]) -> float:
    return float(max(values)) if values else 0.0


def is_valid_exit(exit_code: int) -> bool:
    # 0 = matches/success, 1 = no matches, >=2 = execution error
    return exit_code in (0, 1)


def normalize_rs_path(path: str) -> str:
    p = path.strip(" \t\r\n:;,.()[]{}<>\"'")
    p = p.replace("\\", "/")
    if "/.../" in p:
        p = p.split("/.../", 1)[1]
    if "/src/" in p:
        p = p.split("/src/", 1)[1]
    elif p.startswith("src/"):
        p = p[4:]

    p = p.lstrip("./")
    p = re.sub(r"/{2,}", "/", p)
    return p


def extract_filenames(text: str) -> set[str]:
    filenames: set[str] = set()
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue

        # RTK grouped output:
        #   📄 /path/to/file.rs (12):
        #   📄 parser/mod.rs [9.4]
        m = re.match(r"^📄\s+(.+?\.rs)\s*(?:\(|\[|$)", line)
        if m:
            candidate = normalize_rs_path(m.group(1))
            if candidate.endswith(".rs"):
                filenames.add(candidate)
            continue

        # grep -rn style:
        #   /abs/src/file.rs:42:...
        #   src/file.rs:42:...
        m = re.match(r"^(.+?\.rs):\d+(?::|$)", line)
        if m:
            candidate = normalize_rs_path(m.group(1))
            if candidate.endswith(".rs"):
                filenames.add(candidate)

    return filenames


def file_matches_gold(gold_file: str, found_files: set[str]) -> bool:
    if gold_file in found_files:
        return True

    if "/" not in gold_file:
        suffix = f"/{gold_file}"
        return any(f == gold_file or f.endswith(suffix) for f in found_files)

    return False


def compute_gold_hits(sample_text: str, gold_files: list[str]) -> int:
    if not gold_files:
        return 0
    found_files = extract_filenames(sample_text)
    return sum(1 for gf in gold_files if file_matches_gold(gf, found_files))


def infer_no_result_from_sample(sample_text: str, tool: str) -> bool:
    text = sample_text.strip()
    if not text:
        return False
    if tool in {"rtk_grep", "rtk_rgai"}:
        # rtk no-results marker examples:
        # "🔍 0 for 'query'" / "🧠 0 for 'query'"
        if re.search(r"(?:🔍|🧠)\s*0\s+for\b", text):
            return True
        # Fallback in case glyphs differ.
        if re.search(r"^\s*0\s+for\b", text):
            return True
    return False


def compute_gold_hit_rate(sample_text: str, gold_files: list[str]) -> float | None:
    if not gold_files:
        return None
    hits = compute_gold_hits(sample_text, gold_files)
    return hits / len(gold_files)


def compute_gold_found_count(sample_text: str) -> int:
    return len(extract_filenames(sample_text))


def is_miss(result_count: int, expect_results: bool) -> bool:
    return result_count == 0 and expect_results


def format_te(te: float | None, miss: bool) -> str:
    if miss:
        return "MISS"
    if te is None:
        return "N/A"
    return f"{te:.3f}"


def format_pct(te: float | None, miss: bool) -> str:
    if miss:
        return "MISS"
    if te is None:
        return "N/A"
    savings = (1 - te) * 100
    return f"{savings:.1f}%"


def format_gold(rate: float | None, found_count: int, min_required: int) -> str:
    if rate is None:
        if min_required > 0:
            return f"N/A ({found_count}/{min_required})"
        return "N/A"
    if min_required > 0:
        return f"{rate * 100:.0f}% ({found_count}/{min_required})"
    return f"{rate * 100:.0f}%"


def format_timing(us: float) -> str:
    if us >= 1_000_000:
        return f"{us / 1_000_000:.2f}s"
    if us >= 1_000:
        return f"{us / 1_000:.1f}ms"
    return f"{us:.0f}μs"


def format_timing_range(min_us: float, med_us: float, max_us: float) -> str:
    return f"{format_timing(min_us)} / {format_timing(med_us)} / {format_timing(max_us)}"


def load_gold_standards() -> tuple[dict, dict]:
    with open(GOLD_PATH, encoding="utf-8") as f:
        data = json.load(f)
    return data["queries"], data.get("metadata", {})


def load_gold_auto() -> dict:  # ADDED: load auto-generated gold
    """Load auto-generated gold standards from grep output."""
    if not GOLD_AUTO_PATH.exists():
        return {}
    with open(GOLD_AUTO_PATH, encoding="utf-8") as f:
        data = json.load(f)
    return data.get("queries", {})


def load_csv() -> list[dict]:
    rows = []
    with open(CSV_PATH, newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        for row in reader:
            if "output_tokens" not in row or row["output_tokens"] in (None, ""):
                raise ValueError(
                    "results_raw.csv is missing 'output_tokens'. "
                    "Re-run benchmarks/bench_code.sh after installing tiktoken."
                )
            row["time_us"] = int(row["time_us"])
            row["output_bytes"] = int(row["output_bytes"])
            row["output_tokens"] = int(row["output_tokens"])
            row["result_count"] = int(row["result_count"])
            row["exit_code"] = int(row["exit_code"])
            row["run"] = int(row["run"])
            rows.append(row)
    return rows


def load_quality_sample(test_id: str, tool: str) -> str:
    path = QUALITY_DIR / f"{test_id}_{tool}.txt"
    if path.exists():
        return path.read_text(errors="replace")
    return ""


def parse_commit_from_env(env_text: str) -> str | None:
    m = re.search(r"^Commit:\s*([0-9a-f]{7,40})\s*$", env_text, flags=re.MULTILINE)
    return m.group(1) if m else None


def compute_metrics(rows: list[dict], gold: dict, gold_auto: dict | None = None) -> list[dict]:  # CHANGED: added gold_auto
    grouped: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for row in rows:
        grouped[(row["test_id"], row["tool"])].append(row)

    aggregates: dict[tuple[str, str], dict] = {}
    for (tid, tool), runs in grouped.items():
        times = [r["time_us"] for r in runs]
        bytess = [r["output_bytes"] for r in runs]
        tokens = [r["output_tokens"] for r in runs]  # ADDED: token counts
        counts = [r["result_count"] for r in runs]
        exits = [r["exit_code"] for r in runs]
        aggregates[(tid, tool)] = {
            "test_id": tid,
            "tool": tool,
            "category": runs[0]["category"],
            "query": runs[0]["query"].strip('"'),
            "median_time_us": median_val(times),
            "min_time_us": min_val(times),
            "max_time_us": max_val(times),
            "median_bytes": median_val(bytess),
            "median_tokens": median_val(tokens),  # ADDED
            "median_count": median_val(counts),
            "valid": all(is_valid_exit(e) for e in exits),
        }

    test_ids = sorted(
        set(tid for tid, _ in aggregates.keys()),
        key=lambda x: (x[0], int(x[1:]) if x[1:].isdigit() else 0),
    )

    results = []
    for tid in test_ids:
        gold_entry = gold.get(tid, {})
        category = gold_entry.get("category", "unknown")
        expect = gold_entry.get("expect_results", True)
        gold_files = gold_entry.get("gold_files", [])
        gold_min_files = int(gold_entry.get("gold_min_files", 0) or 0)

        grep_agg = aggregates.get((tid, "grep"))
        grep_tokens = grep_agg["median_tokens"] if grep_agg else 0

        entry = {
            "test_id": tid,
            "category": category,
            "query": gold_entry.get("query", ""),
            "expect_results": expect,
        }

        for tool in TOOLS:
            agg = aggregates.get((tid, tool))
            if not agg:
                continue

            sample = load_quality_sample(tid, tool)
            no_result_marker = infer_no_result_from_sample(sample, tool)
            adjusted_count = 0 if no_result_marker else int(agg["median_count"])

            unsupported = category == "regex_pattern" and tool == "rtk_rgai"
            miss = is_miss(adjusted_count, expect) and not unsupported
            unexpected_hit = (not expect) and adjusted_count > 0

            ghr = compute_gold_hit_rate(sample, gold_files) if sample else None
            found_count = compute_gold_found_count(sample) if sample else 0
            gold_hits = compute_gold_hits(sample, gold_files) if sample else 0
            gold_min_ok = None
            if gold_min_files > 0:
                gold_min_ok = found_count >= gold_min_files
            low_coverage = False
            if not miss and not unsupported:
                if gold_min_files > 0 and gold_min_ok is False:
                    low_coverage = True
                if ghr is not None and ghr == 0.0:
                    low_coverage = True

            te = None
            if agg["valid"] and grep_tokens > 0:
                te = agg["median_tokens"] / grep_tokens

            entry[f"{tool}_bytes"] = agg["median_bytes"]
            entry[f"{tool}_tokens"] = agg["median_tokens"]
            entry[f"{tool}_count"] = adjusted_count
            entry[f"{tool}_time_us"] = agg["median_time_us"]
            entry[f"{tool}_min_time_us"] = agg["min_time_us"]
            entry[f"{tool}_max_time_us"] = agg["max_time_us"]
            entry[f"{tool}_te"] = te
            entry[f"{tool}_gold_hit"] = ghr
            entry[f"{tool}_gold_found"] = found_count
            entry[f"{tool}_gold_hits"] = gold_hits
            entry[f"{tool}_gold_min_required"] = gold_min_files
            entry[f"{tool}_gold_min_ok"] = gold_min_ok
            entry[f"{tool}_low_coverage"] = low_coverage
            entry[f"{tool}_valid"] = agg["valid"]
            entry[f"{tool}_miss"] = miss
            entry[f"{tool}_unsupported"] = unsupported
            entry[f"{tool}_unexpected_hit"] = unexpected_hit

        results.append(entry)

    return results


def category_tool_stats(cat_metrics: list[dict], tool: str) -> dict:
    entries = [m for m in cat_metrics if f"{tool}_bytes" in m and not m.get(f"{tool}_unsupported", False)]

    te_vals = [m[f"{tool}_te"] for m in entries if m.get(f"{tool}_te") is not None and not m.get(f"{tool}_miss", False)]
    gold_vals = [m[f"{tool}_gold_hit"] for m in entries if m.get(f"{tool}_gold_hit") is not None]
    time_vals = [m[f"{tool}_time_us"] for m in entries]
    min_time_vals = [m[f"{tool}_min_time_us"] for m in entries]
    max_time_vals = [m[f"{tool}_max_time_us"] for m in entries]

    miss_count = sum(1 for m in entries if m.get(f"{tool}_miss", False))
    unexpected_count = sum(1 for m in entries if m.get(f"{tool}_unexpected_hit", False))
    low_cov_count = sum(
        1
        for m in entries
        if m.get(f"{tool}_low_coverage", False)
        and not m.get(f"{tool}_miss", False)
    )

    return {
        "entries": entries,
        "te_vals": te_vals,
        "gold_vals": gold_vals,
        "time_vals": time_vals,
        "min_time_vals": min_time_vals,
        "max_time_vals": max_time_vals,
        "miss_count": miss_count,
        "unexpected_count": unexpected_count,
        "low_cov_count": low_cov_count,
        "unsupported_count": sum(1 for m in cat_metrics if m.get(f"{tool}_unsupported", False)),
    }


def pick_best_for_exact(cat_metrics: list[dict]) -> tuple[str, str]:
    candidates = []
    for tool in RECOMMENDABLE_TOOLS:
        st = category_tool_stats(cat_metrics, tool)
        if not st["entries"]:
            continue
        med_te = median_val(st["te_vals"]) if st["te_vals"] else 1e18
        med_gold = median_val(st["gold_vals"]) if st["gold_vals"] else -1.0
        med_time = median_val(st["time_vals"]) if st["time_vals"] else 1e18
        candidates.append(
            (tool, st["miss_count"], st["low_cov_count"], med_gold, med_te, med_time)
        )

    if not candidates:
        return "N/A", "Insufficient valid metrics"

    # For exact/cross-file tasks: correctness first, compression second.
    candidates.sort(key=lambda x: (x[1], x[2], -x[3], x[4], x[5]))
    tool, miss, low_cov, med_gold, med_te, _ = candidates[0]
    gold_str = "N/A" if med_gold < 0 else f"{med_gold * 100:.0f}%"
    te_str = "N/A" if med_te == 1e18 else f"{med_te:.3f}"
    return tool, f"median gold hit={gold_str}, MISS={miss}, LOW_COVERAGE={low_cov}, median TE={te_str}"


def pick_best_for_semantic(cat_metrics: list[dict]) -> tuple[str, str]:
    candidates = []
    for tool in RECOMMENDABLE_TOOLS:
        st = category_tool_stats(cat_metrics, tool)
        if not st["entries"]:
            continue
        med_gold = median_val(st["gold_vals"]) if st["gold_vals"] else -1.0
        med_te = median_val(st["te_vals"]) if st["te_vals"] else 1e18
        miss = st["miss_count"]
        low_cov = st["low_cov_count"]
        unexpected = st["unexpected_count"]
        candidates.append((tool, miss, low_cov, unexpected, -med_gold, med_te, med_gold))

    if not candidates:
        return "N/A", "Insufficient valid metrics"

    # For semantic tasks: misses/coverage first, then relevance, then compression.
    candidates.sort(key=lambda x: (x[1], x[2], x[3], x[4], x[5]))
    tool, miss, low_cov, unexpected, _, med_te, med_gold = candidates[0]
    gold_str = "N/A" if med_gold < 0 else f"{med_gold * 100:.0f}%"
    te_str = "N/A" if med_te == 1e18 else f"{med_te:.3f}"
    return tool, f"median gold hit={gold_str}, MISS={miss}, LOW_COVERAGE={low_cov}, UNEXPECTED_HIT={unexpected}, median TE={te_str}"


def generate_report(
    metrics: list[dict],
    env_text: str,
    gold_queries: dict,
    pinned_commit: str,
    env_commit: str | None,
) -> str:
    lines: list[str] = []
    w = lines.append

    w("# Code Search Benchmark: grep vs rtk grep vs rtk rgai vs head_n\n")  # CHANGED: added head_n
    w("## Environment & Reproduction\n")
    w("```")
    w(env_text.strip())
    w("```\n")

    w(f"## Dataset: rtk-ai/rtk @ `{pinned_commit}`\n")
    if env_commit and env_commit != pinned_commit:
        w(
            f"> **WARNING**: benchmark env commit `{env_commit}` differs from pinned "
            f"gold commit `{pinned_commit}`. Results are not strictly reproducible.\n"
        )

    w("**Reproduction**:")
    w("```bash")
    w("rtk --version")
    w("bash benchmarks/bench_code.sh")
    w("python3 benchmarks/analyze_code.py")
    w("python3 -m unittest discover -s benchmarks/tests -p 'test_*.py'")
    w("```\n")

    w("## Methodology\n")
    w("### Metrics (reported separately, NO composite score)\n")
    w("| Metric | Definition | Purpose |")
    w("|--------|-----------|---------|")
    w("| Output bytes | `wc -c` of stdout | Raw size footprint |")
    w("| Output tokens | `tiktoken` (`cl100k_base`) on full stdout | Model-aligned token cost |")
    w("| Token Efficiency (TE) | `output_tokens / grep_output_tokens` | Token compression vs baseline |")
    w("| Result count | Effective output lines / no-result aware count | Distinguish compactness vs empty results |")
    w("| Gold hit rate | `% gold_files found` (plus found/min files) | Relevance/correctness |")
    w("| Timing | Median of 5 runs, plus min/max in summaries | Performance distribution |")
    w("")
    w("**Critical rule**: if `expect_results=true` and `result_count==0`, mark as **MISS**.")
    w("For regex category, `rtk rgai` is marked `EXPECTED_UNSUPPORTED` by design.\n")

    w("### Categories\n")
    w("| Category | Queries |")
    w("|----------|---------|")
    w("| A: Exact Identifier | 6 |")
    w("| B: Regex Pattern | 6 |")
    w("| C: Semantic Intent | 10 |")
    w("| D: Cross-File Pattern Discovery | 5 |")
    w("| E: Edge Cases | 3 |")
    w("")

    for cat_key in CATEGORY_ORDER:
        cat_title = CATEGORY_TITLES[cat_key]
        cat_metrics = [m for m in metrics if m["category"] == cat_key]
        if not cat_metrics:
            continue

        w(f"## {cat_title}\n")

        if cat_key == "regex_pattern":
            w("> `rtk rgai` does not support regex; misses are EXPECTED_UNSUPPORTED.\n")
        if cat_key == "semantic_intent":
            w("> For multi-concept queries, grep exact-substring misses are expected and shown as MISS.\n")
        if cat_key == "edge_case":
            w("> Edge cases are discussed per-case; no category-level winner is inferred.\n")

        w("| ID | Query | Tool | Bytes | Tokens | TE | Result Count | Gold Hit | Timing (med) | Status |")
        w("|----|-------|------|-------|--------|----|-------------|----------|-------------|--------|")

        for m in cat_metrics:
            for tool in TOOLS:
                if f"{tool}_bytes" not in m:
                    continue

                miss = m.get(f"{tool}_miss", False)
                unsupported = m.get(f"{tool}_unsupported", False)
                unexpected_hit = m.get(f"{tool}_unexpected_hit", False)
                valid = m.get(f"{tool}_valid", False)
                min_required = m.get(f"{tool}_gold_min_required", 0)
                low_coverage = m.get(f"{tool}_low_coverage", False)

                if unsupported:
                    status = "EXPECTED_UNSUPPORTED"
                elif not valid:
                    status = "INVALID"
                elif miss:
                    status = "**MISS**"
                elif unexpected_hit:
                    status = "**UNEXPECTED_HIT**"
                elif low_coverage:
                    status = "LOW_COVERAGE"
                else:
                    status = "OK"

                w(
                    f"| {m['test_id']} | {m['query']} | {tool} | {m[f'{tool}_bytes']:.0f} | "
                    f"{m.get(f'{tool}_tokens', 0):.0f} | "
                    f"{format_te(m.get(f'{tool}_te'), miss)} | "
                    f"{m.get(f'{tool}_count', 0):.0f} | "
                    f"{format_gold(m.get(f'{tool}_gold_hit'), m.get(f'{tool}_gold_found', 0), min_required)} | "
                    f"{format_timing(m.get(f'{tool}_time_us', 0.0))} | {status} |"
                )
        w("")

        if cat_key != "edge_case":
            w(f"### {cat_title} — Summary\n")
            for tool in TOOLS:
                st = category_tool_stats(cat_metrics, tool)
                if st["unsupported_count"] == len(cat_metrics):
                    w(f"- **{tool}**: expected unsupported for this category.")
                    continue

                parts = [f"**{tool}**:"]
                if st["te_vals"]:
                    parts.append(
                        "TE min/med/max="
                        f"{min_val(st['te_vals']):.3f}/"
                        f"{median_val(st['te_vals']):.3f}/"
                        f"{max_val(st['te_vals']):.3f}"
                    )
                if st["gold_vals"]:
                    parts.append(
                        "gold hit min/med/max="
                        f"{min_val(st['gold_vals']) * 100:.0f}%/"
                        f"{median_val(st['gold_vals']) * 100:.0f}%/"
                        f"{max_val(st['gold_vals']) * 100:.0f}%"
                    )
                if st["time_vals"]:
                    parts.append(
                        "time min/med/max="
                        + format_timing_range(
                            min_val(st["min_time_vals"]),
                            median_val(st["time_vals"]),
                            max_val(st["max_time_vals"]),
                        )
                    )
                if st["miss_count"] > 0:
                    parts.append(f"MISS={st['miss_count']}")
                if st["unexpected_count"] > 0:
                    parts.append(f"UNEXPECTED_HIT={st['unexpected_count']}")
                if st["low_cov_count"] > 0:
                    parts.append(f"LOW_COVERAGE={st['low_cov_count']}")
                w("- " + " | ".join(parts))
            w("")

    # Tool recommendation rows without cross-category averaging.
    w("## Summary: When to Use Which Tool\n")
    w("| Situation | Recommended | Evidence |")
    w("|-----------|-------------|----------|")

    cat_a = [m for m in metrics if m["category"] == "exact_identifier"]
    cat_d = [m for m in metrics if m["category"] == "cross_file"]
    cat_c = [m for m in metrics if m["category"] == "semantic_intent"]
    cat_e = [m for m in metrics if m["category"] == "edge_case"]

    best_a, ev_a = pick_best_for_exact(cat_a)
    best_d, ev_d = pick_best_for_exact(cat_d)
    best_c, ev_c = pick_best_for_semantic(cat_c)

    w(f"| Exact identifier search (Category A) | {best_a} | {ev_a} |")
    w(f"| Cross-file pattern discovery (Category D) | {best_d} | {ev_d} |")
    w(f"| Semantic intent search (Category C) | {best_c} | {ev_c} |")
    w("| Regex patterns (Category B) | grep / rtk grep | `rtk rgai` expected unsupported for regex |")

    # Edge evidence: E3 should be zero results.
    e3 = next((m for m in cat_e if m["test_id"] == "E3"), None)
    if e3:
        bad_tools = [t for t in TOOLS if e3.get(f"{t}_unexpected_hit", False)]
        if bad_tools:
            w(
                "| Exact zero-result validation (E3) | grep / rtk grep | "
                f"Unexpected hits observed for: {', '.join(bad_tools)} |"
            )
        else:
            w("| Exact zero-result validation (E3) | all tools | All returned zero results as expected |")
    w("")

    w("## Failure Modes\n")
    w("### grep")
    w("- Floods output on broad/common queries.")
    w("- Misses semantic intent queries that do not appear as exact substrings.")
    w("- No built-in grouping/truncation.\n")
    w("### rtk grep")
    w("- Output truncation (`--max 200`) can reduce recall in high-frequency queries.")
    w("- Still exact-match based (no semantic expansion).\n")
    w("### rtk rgai")
    w("- Regex queries are unsupported by design.")
    w("- Can return semantically related content even when strict zero results are expected.")
    w("- Quality depends on ranking/model behavior and may vary by environment.\n")
    if "head_n" in TOOLS:
        w("### head_n (negative control)")
        w("- Naive truncation may look token-efficient but is relevance-blind.")
        w("- Useful as a floor comparator, not as a production recommendation.\n")

    w("## Limitations\n")
    w("- Single codebase benchmark (`src/` Rust files only).")
    w("- Gold standards are author-defined and include subjective intent mapping.")
    w("- Gold hit is computed from first-run samples; non-deterministic tools may vary across runs.")
    w("- Timing is hardware and background-load dependent.")
    w("")

    return "\n".join(lines)


def main():
    if not CSV_PATH.exists():
        print(f"ERROR: {CSV_PATH} not found. Run bench_code.sh first.", file=sys.stderr)
        sys.exit(1)
    if not GOLD_PATH.exists():
        print(f"ERROR: {GOLD_PATH} not found.", file=sys.stderr)
        sys.exit(1)

    gold_queries, gold_meta = load_gold_standards()
    gold_auto = load_gold_auto()  # ADDED: auto-generated gold from grep output
    rows = load_csv()
    env_text = ENV_PATH.read_text() if ENV_PATH.exists() else ""
    env_commit = parse_commit_from_env(env_text)
    pinned_commit = gold_meta.get("pinned_commit", "unknown")

    print(f"Loaded {len(rows)} measurements from {CSV_PATH}")
    print(f"Loaded {len(gold_queries)} gold standards from {GOLD_PATH}")
    if gold_auto:
        print(f"Loaded {len(gold_auto)} auto-generated gold entries from {GOLD_AUTO_PATH}")  # ADDED

    metrics = compute_metrics(rows, gold_queries, gold_auto)  # CHANGED: pass gold_auto
    report = generate_report(metrics, env_text, gold_queries, pinned_commit, env_commit)
    RESULTS_PATH.write_text(report, encoding="utf-8")

    miss_count = 0
    unexpected_count = 0
    for m in metrics:
        for tool in TOOLS:
            if m.get(f"{tool}_miss", False):
                miss_count += 1
            if m.get(f"{tool}_unexpected_hit", False):
                unexpected_count += 1

    print(f"\nReport written to {RESULTS_PATH}")
    print(f"  {len(metrics)} queries analyzed")
    print(f"  MISS entries: {miss_count}")
    print(f"  UNEXPECTED_HIT entries: {unexpected_count}")


if __name__ == "__main__":
    main()
