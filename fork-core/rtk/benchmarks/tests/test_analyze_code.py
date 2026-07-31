"""
Tests for analyze_code.py benchmark analyzer.

Covers:
  - median_val computation
  - is_valid_exit semantics
  - extract_filenames from various output formats
  - compute_gold_hit_rate accuracy
  - is_miss detection (critical rule)
  - format_te / format_pct output
  - compute_metrics end-to-end with mock data
"""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import sys

# Add parent dir to path so we can import analyze_code
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import analyze_code as ac


class TestMedianVal(unittest.TestCase):
    def test_odd_count(self):
        self.assertEqual(ac.median_val([3, 1, 2]), 2.0)

    def test_even_count(self):
        self.assertEqual(ac.median_val([1, 2, 3, 4]), 2.5)

    def test_single(self):
        self.assertEqual(ac.median_val([42]), 42.0)

    def test_empty(self):
        self.assertEqual(ac.median_val([]), 0.0)

    def test_already_sorted(self):
        self.assertEqual(ac.median_val([10, 20, 30, 40, 50]), 30.0)

    def test_duplicates(self):
        self.assertEqual(ac.median_val([5, 5, 5, 5, 5]), 5.0)


class TestIsValidExit(unittest.TestCase):
    def test_exit_0_valid(self):
        self.assertTrue(ac.is_valid_exit(0))

    def test_exit_1_valid(self):
        """exit=1 means 'no matches' for grep — still valid."""
        self.assertTrue(ac.is_valid_exit(1))

    def test_exit_2_invalid(self):
        """exit>=2 means execution error."""
        self.assertFalse(ac.is_valid_exit(2))

    def test_exit_127_invalid(self):
        self.assertFalse(ac.is_valid_exit(127))


class TestExtractFilenames(unittest.TestCase):
    def test_grep_output(self):
        """Standard grep -rn output format."""
        text = (
            "src/tracking.rs:42:pub struct TimedExecution {\n"
            "src/main.rs:100:    let timer = TimedExecution::new();\n"
            "src/discover/registry.rs:77:const RULES: &[RtkRule] = &[\n"
        )
        filenames = ac.extract_filenames(text)
        self.assertIn("tracking.rs", filenames)
        self.assertIn("main.rs", filenames)
        self.assertIn("discover/registry.rs", filenames)

    def test_rtk_file_headers(self):
        """RTK grouped output headers."""
        text = (
            "📄 /Users/andrew/Programming/rtk/src/tracking.rs (1):\n"
            "📄 /.../discover/registry.rs [9.1]\n"
        )
        filenames = ac.extract_filenames(text)
        self.assertIn("tracking.rs", filenames)
        self.assertIn("discover/registry.rs", filenames)

    def test_no_filenames(self):
        """Text with no .rs files."""
        text = "no rust files here\njust plain text"
        filenames = ac.extract_filenames(text)
        self.assertEqual(len(filenames), 0)

    def test_nested_path(self):
        """Nested directory paths."""
        text = "src/parser/mod.rs:1:pub mod types;"
        filenames = ac.extract_filenames(text)
        self.assertIn("parser/mod.rs", filenames)

    def test_absolute_path_normalization(self):
        text = (
            "/Users/andrew/Programming/rtk/src/tracking.rs:42:code\n"
            "/Users/andrew/Programming/rtk/src/discover/registry.rs:77:code\n"
        )
        filenames = ac.extract_filenames(text)
        self.assertIn("tracking.rs", filenames)
        self.assertIn("discover/registry.rs", filenames)

    def test_deduplication(self):
        """Same file appearing multiple times."""
        text = (
            "src/git.rs:10:code\n"
            "src/git.rs:20:more code\n"
            "src/git.rs:30:even more\n"
        )
        filenames = ac.extract_filenames(text)
        self.assertEqual(filenames.count("git.rs") if isinstance(filenames, list) else 1, 1)
        self.assertIn("git.rs", filenames)

    def test_does_not_parse_rs_inside_code_snippet(self):
        text = ' 565: classify_command("cat src/main.rs"),'
        filenames = ac.extract_filenames(text)
        self.assertEqual(len(filenames), 0)


class TestComputeGoldHitRate(unittest.TestCase):
    def test_all_found(self):
        sample = "src/tracking.rs:1:code\nsrc/main.rs:2:code\n"
        gold = ["tracking.rs", "main.rs"]
        self.assertAlmostEqual(ac.compute_gold_hit_rate(sample, gold), 1.0)

    def test_partial_found(self):
        sample = "src/tracking.rs:1:code\nsrc/utils.rs:2:code\n"
        gold = ["tracking.rs", "main.rs"]
        self.assertAlmostEqual(ac.compute_gold_hit_rate(sample, gold), 0.5)

    def test_none_found(self):
        sample = "src/utils.rs:1:code\n"
        gold = ["tracking.rs", "main.rs"]
        self.assertAlmostEqual(ac.compute_gold_hit_rate(sample, gold), 0.0)

    def test_empty_gold_files(self):
        """No gold files => N/A for hit-rate."""
        sample = "src/anything.rs:1:code\n"
        self.assertIsNone(ac.compute_gold_hit_rate(sample, []))

    def test_empty_sample(self):
        gold = ["tracking.rs"]
        self.assertAlmostEqual(ac.compute_gold_hit_rate("", gold), 0.0)

    def test_nested_gold_files(self):
        sample = "src/discover/registry.rs:77:const RULES\n"
        gold = ["discover/registry.rs"]
        self.assertAlmostEqual(ac.compute_gold_hit_rate(sample, gold), 1.0)


class TestIsMiss(unittest.TestCase):
    def test_zero_results_expects_results(self):
        """0 results when gold expects results → MISS."""
        self.assertTrue(ac.is_miss(0, True))

    def test_zero_results_expects_nothing(self):
        """0 results when gold expects nothing → NOT miss."""
        self.assertFalse(ac.is_miss(0, False))

    def test_has_results_expects_results(self):
        """Has results when expected → NOT miss."""
        self.assertFalse(ac.is_miss(42, True))

    def test_has_results_expects_nothing(self):
        """Has results when nothing expected → NOT miss (unexpected but not MISS)."""
        self.assertFalse(ac.is_miss(5, False))


class TestFormatFunctions(unittest.TestCase):
    def test_format_te_normal(self):
        self.assertEqual(ac.format_te(0.123, False), "0.123")

    def test_format_te_miss(self):
        self.assertEqual(ac.format_te(0.5, True), "MISS")

    def test_format_te_none(self):
        self.assertEqual(ac.format_te(None, False), "N/A")

    def test_format_pct_savings(self):
        self.assertEqual(ac.format_pct(0.3, False), "70.0%")

    def test_format_pct_miss(self):
        self.assertEqual(ac.format_pct(0.3, True), "MISS")

    def test_format_pct_expansion(self):
        """TE > 1.0 means output is larger than grep baseline."""
        self.assertEqual(ac.format_pct(1.5, False), "-50.0%")

    def test_format_gold_full(self):
        self.assertEqual(ac.format_gold(1.0, 10, 5), "100% (10/5)")

    def test_format_gold_partial(self):
        self.assertEqual(ac.format_gold(0.6, 3, 8), "60% (3/8)")

    def test_format_gold_none(self):
        self.assertEqual(ac.format_gold(None, 0, 0), "N/A")

    def test_format_gold_none_with_min(self):
        self.assertEqual(ac.format_gold(None, 2, 10), "N/A (2/10)")

    def test_format_timing_microseconds(self):
        self.assertEqual(ac.format_timing(500), "500μs")

    def test_format_timing_milliseconds(self):
        self.assertEqual(ac.format_timing(5000), "5.0ms")

    def test_format_timing_seconds(self):
        self.assertEqual(ac.format_timing(2_500_000), "2.50s")


class TestComputeMetrics(unittest.TestCase):
    """End-to-end test of compute_metrics with synthetic data."""

    def _make_rows(self, tid, tool, runs=5, time=1000, output_bytes=500,
                   output_tokens=100, result_count=10, exit_code=0):
        """Helper to generate mock CSV rows."""
        return [
            {
                "test_id": tid,
                "category": "exact_identifier",
                "query": f'"{tid} query"',
                "tool": tool,
                "run": i + 1,
                "time_us": time + i * 10,
                "output_bytes": output_bytes,
                "output_tokens": output_tokens,
                "result_count": result_count,
                "exit_code": exit_code,
            }
            for i in range(runs)
        ]

    def test_basic_metrics(self):
        """Verify TE computation for a simple case."""
        gold = {
            "T1": {
                "query": "test",
                "category": "exact_identifier",
                "gold_files": [],
                "gold_min_files": 0,
                "expect_results": True,
            }
        }
        rows = (
            self._make_rows("T1", "grep", output_bytes=1000, output_tokens=1000, result_count=50)
            + self._make_rows("T1", "rtk_grep", output_bytes=300, output_tokens=300, result_count=20)
            + self._make_rows("T1", "rtk_rgai", output_bytes=200, output_tokens=200, result_count=10)
        )

        with patch.object(ac, "load_quality_sample", return_value=""):
            metrics = ac.compute_metrics(rows, gold)

        self.assertEqual(len(metrics), 1)
        m = metrics[0]
        # TE = rtk_grep_tokens / grep_tokens = 300/1000 = 0.3
        self.assertAlmostEqual(m["rtk_grep_te"], 0.3)
        # TE = rtk_rgai_tokens / grep_tokens = 200/1000 = 0.2
        self.assertAlmostEqual(m["rtk_rgai_te"], 0.2)

    def test_miss_detection(self):
        """0 result count with expect_results=True → MISS."""
        gold = {
            "T2": {
                "query": "test",
                "category": "semantic_intent",
                "gold_files": ["tracking.rs"],
                "gold_min_files": 1,
                "expect_results": True,
            }
        }
        rows = (
            self._make_rows("T2", "grep", output_bytes=0, result_count=0)
            + self._make_rows("T2", "rtk_grep", output_bytes=0, result_count=0)
            + self._make_rows("T2", "rtk_rgai", output_bytes=500, result_count=5)
        )

        with patch.object(ac, "load_quality_sample", return_value=""):
            metrics = ac.compute_metrics(rows, gold)

        m = metrics[0]
        self.assertTrue(m["grep_miss"])
        self.assertTrue(m["rtk_grep_miss"])
        self.assertFalse(m["rtk_rgai_miss"])

    def test_miss_detection_with_rtk_zero_marker(self):
        """rtk '0 for' marker should force effective result_count=0."""
        gold = {
            "T2B": {
                "query": "semantic query",
                "category": "semantic_intent",
                "gold_files": ["tracking.rs"],
                "gold_min_files": 1,
                "expect_results": True,
            }
        }
        rows = (
            self._make_rows("T2B", "grep", output_bytes=0, result_count=0)
            + self._make_rows("T2B", "rtk_grep", output_bytes=42, result_count=1)
            + self._make_rows("T2B", "rtk_rgai", output_bytes=2400, result_count=80)
        )

        def fake_sample(tid, tool):
            if tid == "T2B" and tool == "rtk_grep":
                return "🔍 0 for 'semantic query'\n"
            return ""

        with patch.object(ac, "load_quality_sample", side_effect=fake_sample):
            metrics = ac.compute_metrics(rows, gold)

        m = metrics[0]
        self.assertEqual(m["rtk_grep_count"], 0)
        self.assertTrue(m["rtk_grep_miss"])

    def test_no_miss_when_not_expected(self):
        """0 results with expect_results=False → NOT miss."""
        gold = {
            "T3": {
                "query": "nonexistent",
                "category": "edge_case",
                "gold_files": [],
                "gold_min_files": 0,
                "expect_results": False,
            }
        }
        rows = (
            self._make_rows("T3", "grep", output_bytes=0, result_count=0)
            + self._make_rows("T3", "rtk_grep", output_bytes=0, result_count=0)
            + self._make_rows("T3", "rtk_rgai", output_bytes=0, result_count=0)
        )

        with patch.object(ac, "load_quality_sample", return_value=""):
            metrics = ac.compute_metrics(rows, gold)

        m = metrics[0]
        self.assertFalse(m["grep_miss"])
        self.assertFalse(m["rtk_grep_miss"])
        self.assertFalse(m["rtk_rgai_miss"])

    def test_grep_baseline_zero_te_none(self):
        """When grep baseline is 0 bytes, TE should be None."""
        gold = {
            "T4": {
                "query": "rare",
                "category": "exact_identifier",
                "gold_files": [],
                "gold_min_files": 0,
                "expect_results": False,
            }
        }
        rows = (
            self._make_rows("T4", "grep", output_bytes=0, output_tokens=0, result_count=0)
            + self._make_rows("T4", "rtk_grep", output_bytes=100, result_count=2)
            + self._make_rows("T4", "rtk_rgai", output_bytes=50, result_count=1)
        )

        with patch.object(ac, "load_quality_sample", return_value=""):
            metrics = ac.compute_metrics(rows, gold)

        m = metrics[0]
        self.assertIsNone(m["rtk_grep_te"])
        self.assertIsNone(m["rtk_rgai_te"])

    def test_zero_gold_hit_marks_low_coverage(self):
        gold = {
            "T5": {
                "query": "semantic",
                "category": "semantic_intent",
                "gold_files": ["tracking.rs"],
                "gold_min_files": 1,
                "expect_results": True,
            }
        }
        rows = (
            self._make_rows("T5", "grep", output_tokens=100, result_count=10)
            + self._make_rows("T5", "rtk_rgai", output_tokens=20, result_count=5)
        )

        def fake_sample(tid, tool):
            if tool == "grep":
                return "src/tracking.rs:1:code\n"
            if tool == "rtk_rgai":
                return "📄 src/utils.rs [10.0]\n"
            return ""

        with patch.object(ac, "load_quality_sample", side_effect=fake_sample):
            metrics = ac.compute_metrics(rows, gold)

        m = metrics[0]
        self.assertTrue(m["rtk_rgai_low_coverage"])


class TestGoldStandardsIntegrity(unittest.TestCase):
    """Verify gold_standards.json is well-formed."""

    def setUp(self):
        gold_path = Path(__file__).resolve().parent.parent / "gold_standards.json"
        with open(gold_path, encoding="utf-8") as f:
            self.data = json.load(f)
        self.queries = self.data["queries"]

    def test_has_metadata(self):
        self.assertIn("metadata", self.data)
        self.assertIn("pinned_commit", self.data["metadata"])

    def test_query_count(self):
        """Should have exactly 30 queries."""
        self.assertEqual(len(self.queries), 30)

    def test_category_distribution(self):
        """A=6, B=6, C=10, D=5, E=3."""
        cats = [q["category"] for q in self.queries.values()]
        self.assertEqual(cats.count("exact_identifier"), 6)
        self.assertEqual(cats.count("regex_pattern"), 6)
        self.assertEqual(cats.count("semantic_intent"), 10)
        self.assertEqual(cats.count("cross_file"), 5)
        self.assertEqual(cats.count("edge_case"), 3)

    def test_required_fields(self):
        """Every query has required fields."""
        required = {"query", "category", "grep_flags", "gold_files",
                    "gold_min_files", "expect_results", "notes"}
        for tid, q in self.queries.items():
            for field in required:
                self.assertIn(
                    field, q,
                    f"Query {tid} missing field '{field}'"
                )

    def test_id_prefix_matches_category(self):
        """A* → exact_identifier, B* → regex_pattern, etc."""
        prefix_map = {
            "A": "exact_identifier",
            "B": "regex_pattern",
            "C": "semantic_intent",
            "D": "cross_file",
            "E": "edge_case",
        }
        for tid, q in self.queries.items():
            expected_cat = prefix_map.get(tid[0])
            self.assertEqual(
                q["category"], expected_cat,
                f"Query {tid} has category '{q['category']}' "
                f"but expected '{expected_cat}'"
            )

    def test_e3_expects_no_results(self):
        """E3 (nonexistent phrase) should expect no results."""
        self.assertFalse(self.queries["E3"]["expect_results"])

    def test_gold_files_are_lists(self):
        for tid, q in self.queries.items():
            self.assertIsInstance(
                q["gold_files"], list,
                f"Query {tid} gold_files is not a list"
            )


if __name__ == "__main__":
    unittest.main()
