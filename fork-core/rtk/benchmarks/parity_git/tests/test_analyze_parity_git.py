from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import analyze_parity_git as apg


class TestParityAnalyzer(unittest.TestCase):
    def test_compute_totals(self):
        rows = [
            {
                "scenario": "a",
                "kind": "failure",
                "native_exit": "1",
                "rtk_exit": "1",
                "exit_match": "1",
                "side_effect_match": "1",
                "stderr_signal_match": "1",
            },
            {
                "scenario": "b",
                "kind": "failure",
                "native_exit": "1",
                "rtk_exit": "2",
                "exit_match": "0",
                "side_effect_match": "1",
                "stderr_signal_match": "0",
            },
        ]

        totals = apg.compute_totals(rows)
        self.assertEqual(totals.rows, 2)
        self.assertAlmostEqual(totals.exit_match_rate, 50.0)
        self.assertAlmostEqual(totals.side_effect_match_rate, 100.0)
        self.assertAlmostEqual(totals.stderr_signal_match_rate, 50.0)

    def test_evaluate_thresholds(self):
        totals = apg.Totals(rows=3, exit_match_rate=100.0, side_effect_match_rate=100.0, stderr_signal_match_rate=99.0)
        checks = apg.evaluate_thresholds(totals)
        self.assertTrue(all(ok for _, ok, _ in checks))

    def test_parse_args(self):
        args = apg.parse_args(["--fail-on-thresholds"])
        self.assertTrue(args.fail_on_thresholds)


if __name__ == "__main__":
    unittest.main()
