#!/usr/bin/env python3
"""Independent deterministic protocol evaluator for the three tiny fixtures."""
import hashlib
import json
import sys

request = json.load(sys.stdin)
run = request["run"]
expected = {"targeted-read": "session['revoked'] returns 403 in both validate and refresh",
            "whole-file-invariant": "validate rejects expires_at == now; refresh accepts it",
            "repeated-range-expansion": "refresh and validate disagree at expires_at == now"}[run["task_id"]]
evidence = {"expected": expected, "actual": request["result"].get("answer"), "task_sha256": run["task_sha256"]}
encoded = json.dumps(evidence, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
json.dump({"run_id": run["run_id"], "task_sha256": run["task_sha256"], "accepted": evidence["actual"] == expected,
           "evidence_sha256": hashlib.sha256(encoded).hexdigest(), "evidence": evidence}, sys.stdout)
