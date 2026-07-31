from __future__ import annotations

import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import analyze_write as aw


class TestPercentile(unittest.TestCase):
    def test_percentile_basic(self):
        self.assertEqual(aw.percentile([10, 20, 30], 0.5), 20.0)
        self.assertEqual(aw.percentile([10, 20, 30], 0.0), 10.0)
        self.assertEqual(aw.percentile([10, 20, 30], 1.0), 30.0)

    def test_percentile_interpolated(self):
        self.assertAlmostEqual(aw.percentile([1, 2, 3, 4], 0.95), 3.85, places=6)


class TestThresholds(unittest.TestCase):
    def test_threshold_evaluation(self):
        rows = []
        # unchanged small durable/fast
        for mode, us in [("durable", 1500), ("fast", 1200)]:
            rows.append(
                {
                    "scenario": "unchanged",
                    "size_label": "small",
                    "file_size": "1024",
                    "tool": "write_core",
                    "mode": mode,
                    "run": "1",
                    "latency_us": str(us),
                    "bytes_written": "0",
                    "fsync_count": "0",
                    "rename_count": "0",
                    "skipped_unchanged": "1",
                }
            )

        # changed small (write_core durable/fast + native)
        rows.extend(
            [
                {
                    "scenario": "changed",
                    "size_label": "small",
                    "file_size": "1024",
                    "tool": "write_core",
                    "mode": "durable",
                    "run": "1",
                    "latency_us": "2000",
                    "bytes_written": "1024",
                    "fsync_count": "2",
                    "rename_count": "1",
                    "skipped_unchanged": "0",
                },
                {
                    "scenario": "changed",
                    "size_label": "small",
                    "file_size": "1024",
                    "tool": "write_core",
                    "mode": "fast",
                    "run": "1",
                    "latency_us": "1000",
                    "bytes_written": "1024",
                    "fsync_count": "0",
                    "rename_count": "1",
                    "skipped_unchanged": "0",
                },
                {
                    "scenario": "changed",
                    "size_label": "small",
                    "file_size": "1024",
                    "tool": "native_safe",
                    "mode": "durable",
                    "run": "1",
                    "latency_us": "1800",
                    "bytes_written": "1024",
                    "fsync_count": "2",
                    "rename_count": "1",
                    "skipped_unchanged": "0",
                },
            ]
        )

        groups = aw.compute_groups(rows)
        checks = aw.evaluate_thresholds(groups)

        names = [c[0] for c in checks]
        self.assertIn("unchanged p50 < 2ms (small, durable)", names)
        self.assertIn("fast < durable (changed, small)", names)

        # all sample checks should pass in this synthetic case
        self.assertTrue(all(ok for _, ok, _ in checks))
        self.assertLess(len(checks), aw.EXPECTED_THRESHOLD_CHECKS)

    def test_failed_checks_detection(self):
        checks = [
            ("gate-a", True, "ok"),
            ("gate-b", False, "ratio=1.9x"),
        ]
        self.assertTrue(aw.has_failed_checks(checks))

    def test_parse_args_fail_on_thresholds(self):
        args = aw.parse_args(["--fail-on-thresholds"])
        self.assertTrue(args.fail_on_thresholds)


if __name__ == "__main__":
    unittest.main()
