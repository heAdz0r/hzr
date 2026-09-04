#!/usr/bin/env python3
"""Synthetic transport fixture: no model calls, secrets, or economic evidence."""
import hashlib
import json
import sys
import time
from datetime import datetime, timezone

request = json.load(sys.stdin)
run = request["run"]
answer = {"targeted-read": "session['revoked'] returns 403 in both validate and refresh",
          "whole-file-invariant": "validate rejects expires_at == now; refresh accepts it",
          "repeated-range-expansion": "refresh and validate disagree at expires_at == now"}[run["task_id"]]
usage = {"input_tokens": 100, "output_tokens": 20}
if run["provider"] == "openai":
    usage.update(input_tokens_details={"cached_tokens": 40}, total_tokens=120)
else:
    usage.update(cache_read_input_tokens=40, cache_creation_input_tokens=10)
request_id = "offline-" + run["run_id"]
receipt = {"run_id": run["run_id"], "request_id": request_id,
           "observed_at": datetime.now(timezone.utc).isoformat(),
           "raw_response": {"id": request_id, "model": run["model"], "created_at": int(time.time()), "usage": usage}}
result = {key: run[key] for key in ("run_id", "task_sha256", "repo_commit", "repo_tree_sha256", "arm", "provider", "model", "toolchain_sha256")}
ranges = [(1, 16)]
if run["task_id"] == "targeted-read" and run["arm"] != "native":
    ranges = [(2, 5)]
if run["task_id"] == "repeated-range-expansion" and run["arm"] != "native":
    ranges = [(1, 10), (8, 16)]
events = [{"event_id": f"read-{index}", "kind": "read", "status": "completed", "source_sha256": hashlib.sha256(b"offline-source").hexdigest(), "from_line": first, "to_line": last, "total_lines": 16, "delivered_bytes": (last - first + 1) * 25} for index, (first, last) in enumerate(ranges)]
result.update(evidence_origin="offline_fixture", request_ids=[request_id], receipts=[receipt], answer=answer, events=events)
json.dump(result, sys.stdout)
