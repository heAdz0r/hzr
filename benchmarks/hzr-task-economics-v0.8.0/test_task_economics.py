import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import task_economics as bench

ROOT = Path(__file__).parent


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name) / "repo"
        self.repo.mkdir()
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        (self.repo / "source.txt").write_text("fixture\n")
        subprocess.run(["git", "add", "source.txt"], cwd=self.repo, check=True)
        subprocess.run(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture"], cwd=self.repo, check=True)
        self.manifest = json.loads((ROOT / "fixtures/tasks.json").read_text())
        self.plan = bench.make_plan(self.manifest, self.repo, "openai", "fixture-model", {arm: {"version": "fixture"} for arm in bench.ARMS}, 1)

    def record(self, run, origin="provider_response", accepted=True):
        evidence = {"verified": accepted}
        raw = {"id": "request-" + run["run_id"], "model": run["model"], "created_at": 100,
               "usage": {"input_tokens": 100, "input_tokens_details": {"cached_tokens": 60}, "output_tokens": 20, "total_tokens": 120}}
        receipt = {"run_id": run["run_id"], "request_id": raw["id"], "observed_at": "1970-01-01T00:01:40Z", "raw_response": raw,
                   "billed_cost": {"basis": "provider_billed", "billing_reference": "synthetic-test", "currency": "USD", "amount": "0.01"}}
        result = {key: run[key] for key in bench.BINDINGS}
        result.update(evidence_origin=origin, request_ids=[raw["id"]], receipts=[receipt], events=[], answer="fixture")
        return {"run": run, "status": "completed", "started_at": "1970-01-01T00:01:39Z", "finished_at": "1970-01-01T00:01:41Z", "adapter_sha256": "fixture", "evaluator_sha256": "fixture",
                "result": result, "evaluation": {"run_id": run["run_id"], "task_sha256": run["task_sha256"], "accepted": accepted, "evidence_sha256": bench.digest(evidence), "evidence": evidence}}

    def test_plan_hash_schedule_and_checkout(self):
        bench.validate_plan(self.plan)
        self.assertEqual(len(self.plan["runs"]), 18)
        self.assertEqual(len({run["run_id"] for run in self.plan["runs"]}), 18)
        self.plan["runs"][0]["model"] = "changed"
        with self.assertRaises(ValueError):
            bench.validate_plan(self.plan)
        (self.repo / "source.txt").write_text("dirty")
        with self.assertRaises(ValueError):
            bench.make_plan(self.manifest, self.repo, "openai", "fixture", {arm: "fixture" for arm in bench.ARMS})

    def test_openai_cache_is_subset_and_reasoning_is_not_added_twice(self):
        run = self.plan["runs"][0]
        record = self.record(run)
        record["result"]["receipts"][0]["raw_response"]["usage"]["output_tokens_details"] = {"reasoning_tokens": 15}
        result = bench.validate_record(run, record, set())
        self.assertEqual(result["usage"]["input_tokens"], 100)
        self.assertEqual(result["usage"]["fresh_input_tokens"], 40)
        self.assertEqual(result["usage"]["output_tokens"], 20)

    def test_anthropic_cache_dimensions_are_disjoint(self):
        run = {**self.plan["runs"][0], "provider": "anthropic"}
        record = self.record(run)
        record["result"]["receipts"][0]["raw_response"]["usage"] = {"input_tokens": 100, "cache_read_input_tokens": 60, "cache_creation_input_tokens": 10, "output_tokens": 20}
        self.assertEqual(bench.validate_record(run, record, set())["usage"]["input_tokens"], 170)

    def test_prior_aggregate_missing_negative_or_reused_receipts_are_rejected(self):
        run = self.plan["runs"][0]
        for mutate in (
            lambda row: row["result"].update(receipts=[], observed_model_usage={"tasks": 400, "actual_input_tokens": 999999}),
            lambda row: row["result"]["receipts"][0].update(run_id="another-run"),
            lambda row: row["result"]["receipts"][0]["raw_response"].update(created_at=10),
            lambda row: row["result"]["receipts"][0]["raw_response"]["usage"].update(output_tokens=-1),
            lambda row: row["result"].update(request_ids=["missing"]),
            lambda row: row["evaluation"].update(task_sha256="foreign"),
        ):
            row = self.record(run)
            mutate(row)
            with self.assertRaises(ValueError):
                bench.validate_record(run, row, set())
        row = self.record(run)
        ids = set()
        bench.validate_record(run, row, ids)
        with self.assertRaises(ValueError):
            bench.validate_record(run, row, ids)

    def test_failed_tasks_stay_in_cost_denominator_and_no_sota_claim(self):
        records = {run["run_id"]: self.record(run, accepted=run["task_id"] != "targeted-read") for run in self.plan["runs"]}
        result = bench.report(self.plan, records)
        self.assertEqual(result["arms"]["native"]["billed_cost_per_accepted_task"]["amount"], "0.015")
        self.assertFalse(result["economic_claim_ready"])
        del records[next(iter(records))]
        self.assertEqual(bench.report(self.plan, records)["status"], "not_measured")

    def test_full_read_and_overlap_signals_do_not_invent_token_penalties(self):
        events = [{"event_id": str(index), "kind": "read", "status": "completed", "source_sha256": "source", "from_line": first, "to_line": last, "total_lines": 100} for index, (first, last) in enumerate([(1, 100), (1, 20), (10, 30)])]
        signals = bench.read_signals(events)
        self.assertEqual(signals["full_reads"], 1)
        self.assertEqual(signals["range_reads"], 2)
        self.assertEqual(signals["overlapping_lines"], 41)

    def test_offline_adapter_evaluator_round_trip_never_measures_economics(self):
        output = Path(self.temporary.name) / "runs"
        bench.execute(self.plan, [sys.executable, str(ROOT / "fixtures/offline_adapter.py")], [sys.executable, str(ROOT / "fixtures/offline_evaluator.py")], output, 10)
        records = {path.stem: json.loads(path.read_text()) for path in output.glob("*.json")}
        result = bench.report(self.plan, records)
        self.assertEqual(result["status"], "not_measured")
        self.assertTrue(all(row["status"] == "validated" for row in result["runs"]))
        self.assertIsNone(result["arms"]["native"]["billed_cost_per_accepted_task"])
        with self.assertRaises(ValueError):
            bench.execute(self.plan, [sys.executable], [sys.executable], output, 1)

    def test_bad_evidence_or_extra_runs_cannot_validate_a_complete_experiment(self):
        run = self.plan["runs"][0]
        record = self.record(run)
        record["evaluation"]["evidence"] = {"tampered": True}
        with self.assertRaises(ValueError):
            bench.validate_record(run, record, set())
        records = {run["run_id"]: self.record(run) for run in self.plan["runs"]}
        records["unexpected"] = self.record(run)
        self.assertEqual(bench.report(self.plan, records)["status"], "not_measured")
        records[run["run_id"]] = []
        self.assertEqual(bench.report(self.plan, records)["status"], "not_measured")

    def test_command_failure_is_recorded_without_stderr_contents(self):
        output = Path(self.temporary.name) / "failed"
        bench.execute(self.plan, [sys.executable, "-c", "import sys; sys.stderr.write('sensitive fixture'); sys.exit(7)"], [sys.executable], output, 10)
        records = [json.loads(path.read_text()) for path in output.glob("*.json")]
        self.assertTrue(all(record["status"] == "failed" for record in records))
        self.assertTrue(all("sensitive fixture" not in record["error"] for record in records))

    def test_legacy_aggregate_cannot_become_case_evidence(self):
        path = ROOT.parent / "hzr-billed-input-prefix-cache-v0.6.4/benchmark.py"
        spec = importlib.util.spec_from_file_location("legacy_benchmark", path)
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        self.assertIsNone(module.collect_receipt({"observed_model_usage": {"tasks": 100, "actual_input_tokens": 99999}})[0])
        self.assertEqual(module.compare({})["status"], "not_measured")
        with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], 9)):
            self.assertEqual(module.run_case(Path("hzr"), Path("config"), self.repo, ("read", "missing")), 9)


if __name__ == "__main__":
    unittest.main()
