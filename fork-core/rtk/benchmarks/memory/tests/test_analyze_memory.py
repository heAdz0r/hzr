from __future__ import annotations

import unittest
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import analyze_memory as am


class TestPercentile(unittest.TestCase):
    def test_percentile_basic(self):
        self.assertEqual(am.percentile([10, 20, 30], 0.5), 20.0)
        self.assertEqual(am.percentile([10, 20, 30], 0.0), 10.0)
        self.assertEqual(am.percentile([10, 20, 30], 1.0), 30.0)

    def test_percentile_interpolated(self):
        self.assertAlmostEqual(am.percentile([1, 2, 3, 4], 0.95), 3.85, places=6)


class TestThresholds(unittest.TestCase):
    def test_threshold_evaluation(self):
        rows = []
        for run, ms in enumerate([160.0, 170.0, 180.0], start=1):
            rows.append(
                {
                    "scenario": "cli_cold",
                    "run": str(run),
                    "latency_ms": f"{ms:.3f}",
                    "savings_pct": "",
                    "cache_hit": "0",
                }
            )
        for run, ms in enumerate([30.0, 40.0, 50.0], start=1):
            rows.append(
                {
                    "scenario": "cli_hot",
                    "run": str(run),
                    "latency_ms": f"{ms:.3f}",
                    "savings_pct": "",
                    "cache_hit": "1",
                }
            )
        for run, ms in enumerate([35.0, 45.0, 55.0], start=1):
            rows.append(
                {
                    "scenario": "api_hot",
                    "run": str(run),
                    "latency_ms": f"{ms:.3f}",
                    "savings_pct": "",
                    "cache_hit": "1",
                }
            )
        rows.append(
            {
                "scenario": "memory_gain",
                "run": "1",
                "latency_ms": "0.000",
                "savings_pct": "99.20",
                "cache_hit": "",
            }
        )

        stats, savings = am.compute_stats(rows)
        checks = am.evaluate_thresholds(stats, savings, 52_000)

        names = [c[0] for c in checks]
        self.assertIn("cli hot p95 < 200ms", names)
        self.assertIn("api hot p95 < 200ms", names)
        self.assertIn("memory gain savings >= 50%", names)
        self.assertIn(
            "estimated memory tokens <= 50% of native explore baseline", names
        )
        self.assertIn("5-step cumulative savings >= 1x native explore baseline", names)
        self.assertTrue(all(ok for _, ok, _ in checks))

    def test_native_baseline_loader_uses_default_when_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "env.txt"
            self.assertEqual(
                am.load_native_explore_tokens(path), am.DEFAULT_NATIVE_EXPLORE_TOKENS
            )

    def test_token_projection_contains_followup_steps(self):
        rows = am.build_token_projections(52_000, 90.0)
        self.assertEqual([r.steps for r in rows], list(am.FOLLOWUP_STEPS))
        self.assertTrue(all(r.saved_tokens > 0 for r in rows))

    def test_failed_checks_detection(self):
        checks = [
            ("gate-a", True, "ok"),
            ("gate-b", False, "ratio=1.9x"),
        ]
        self.assertTrue(am.has_failed_checks(checks))

    def test_parse_args_fail_on_thresholds(self):
        args = am.parse_args(["--fail-on-thresholds"])
        self.assertTrue(args.fail_on_thresholds)


if __name__ == "__main__":
    unittest.main()
