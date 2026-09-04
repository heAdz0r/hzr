#!/usr/bin/env python3
"""Correlated whole-task evidence; never infer billed savings from aggregate counters."""
from __future__ import annotations
import argparse
import hashlib
import json
import random
import statistics
import subprocess
import sys
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
from pathlib import Path

ARMS = ("native", "rtk_only", "hzr_exec", "hzr_retrieval", "hzr_memory", "hzr_full")
BINDINGS = ("run_id", "task_sha256", "repo_commit", "repo_tree_sha256", "arm", "provider", "model", "toolchain_sha256")


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(value):
    return hashlib.sha256(encoded(value)).hexdigest()


def integer(value, name):
    if type(value) is not int or value < 0:
        raise ValueError(f"{name} must be a nonnegative integer")
    return value


def timestamp(value):
    if not isinstance(value, str):
        raise ValueError("timestamp must be an ISO-8601 string")
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("timestamps require a timezone")
    return parsed.timestamp()


def utc_now():
    return datetime.now(timezone.utc).isoformat()


def write_new(path, payload):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False, sort_keys=True, indent=2)
        stream.write("\n")


def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()


def make_plan(manifest, repo, provider, model, toolchain, repetitions=3, seed=1):
    if provider not in ("openai", "anthropic") or not model or repetitions < 1:
        raise ValueError("supported provider, exact model and positive repetitions required")
    if git(repo, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("use a clean dedicated evaluation checkout")
    tasks = manifest["tasks"]
    if not tasks or len({task["id"] for task in tasks}) != len(tasks):
        raise ValueError("tasks must be nonempty with unique IDs")
    for task in tasks:
        if not task.get("prompt") or not task.get("acceptance"):
            raise ValueError("each task requires a prompt and an acceptance specification")
    if set(toolchain) != set(ARMS) or any(not value for value in toolchain.values()):
        raise ValueError("toolchain must identify the resolved configuration of every arm")
    base = {"schema_version": 1, "repo": str(repo.resolve()), "repo_commit": git(repo, "rev-parse", "HEAD"),
            "repo_tree_sha256": hashlib.sha256(git(repo, "ls-tree", "-r", "HEAD").encode()).hexdigest(),
            "provider": provider, "model": model, "seed": seed, "repetitions": repetitions,
            "tasks": tasks, "arms": list(ARMS), "toolchain": toolchain}
    experiment = digest(base)
    rng = random.Random(seed)
    runs = []
    for task in tasks:
        for trial in range(repetitions):
            order = list(ARMS)
            rng.shuffle(order)
            for arm in order:
                run = {"experiment_id": experiment, "task_id": task["id"], "task_sha256": digest(task),
                       "trial": trial, "arm": arm, "repo_commit": base["repo_commit"],
                       "repo_tree_sha256": base["repo_tree_sha256"], "provider": provider, "model": model,
                       "toolchain_sha256": digest(toolchain[arm])}
                run["run_id"] = digest(run)
                runs.append(run)
    base.update(experiment_id=experiment, runs=runs)
    base["plan_sha256"] = digest(base)
    return base


def validate_plan(plan):
    content = {key: value for key, value in plan.items() if key != "plan_sha256"}
    if plan.get("plan_sha256") != digest(content):
        raise ValueError("plan digest mismatch")
    tasks = {task["id"]: task for task in plan["tasks"]}
    expected = {(task, trial, arm) for task in tasks for trial in range(plan["repetitions"]) for arm in ARMS}
    seen = set()
    run_ids = set()
    for run in plan["runs"]:
        key = (run["task_id"], run["trial"], run["arm"])
        if key not in expected or key in seen or run["run_id"] in run_ids:
            raise ValueError("duplicate or foreign run in plan")
        seen.add(key)
        run_ids.add(run["run_id"])
        for binding in ("experiment_id", "repo_commit", "repo_tree_sha256", "provider", "model"):
            if run[binding] != plan[binding]:
                raise ValueError(f"plan/run {binding} mismatch")
        if run["toolchain_sha256"] != digest(plan["toolchain"][run["arm"]]):
            raise ValueError("resolved arm configuration mismatch")
        if run["task_sha256"] != digest(tasks[run["task_id"]]):
            raise ValueError("task hash mismatch")
        if run["run_id"] != digest({key: value for key, value in run.items() if key != "run_id"}):
            raise ValueError("run ID mismatch")
    if seen != expected:
        raise ValueError("incomplete paired plan")


def usage(receipt, run, start, end):
    if not isinstance(receipt, dict):
        raise ValueError("receipt must be an object")
    if receipt.get("run_id") != run["run_id"]:
        raise ValueError("receipt belongs to another run")
    if not start <= timestamp(receipt["observed_at"]) <= end:
        raise ValueError("receipt lies outside the captured run interval")
    raw = receipt["raw_response"]
    if not isinstance(raw, dict) or not isinstance(raw.get("usage"), dict):
        raise ValueError("raw provider response and usage must be objects")
    if raw.get("id") != receipt.get("request_id") or not raw.get("id"):
        raise ValueError("raw provider request ID mismatch")
    if raw.get("model") != run["model"]:
        raise ValueError("resolved model differs from the plan")
    counts = raw["usage"]
    output = integer(counts["output_tokens"], "output_tokens")
    if run["provider"] == "openai":
        created = integer(raw["created_at"], "provider created_at")
        if not start - 1 <= created <= end:
            raise ValueError("provider response predates this run")
        total = integer(counts["input_tokens"], "input_tokens")
        cached = integer(counts["input_tokens_details"]["cached_tokens"], "cached_tokens")
        if cached > total:
            raise ValueError("cached input exceeds total input")
        if integer(counts["total_tokens"], "total_tokens") != total + output:
            raise ValueError("provider token totals are inconsistent")
        fresh, writes = total - cached, 0
    else:
        fresh = integer(counts["input_tokens"], "input_tokens")
        cached = integer(counts["cache_read_input_tokens"], "cache_read_input_tokens")
        writes = integer(counts["cache_creation_input_tokens"], "cache_creation_input_tokens")
        total = fresh + cached + writes
    cost = receipt.get("billed_cost")
    if cost is not None:
        if cost.get("basis") != "provider_billed" or not cost.get("billing_reference") or not cost.get("currency"):
            raise ValueError("cost requires provider billing provenance")
        try:
            amount = Decimal(str(cost["amount"]))
        except (InvalidOperation, KeyError) as error:
            raise ValueError("invalid billed cost") from error
        if not amount.is_finite() or amount < 0:
            raise ValueError("invalid billed cost")
    return {"input_tokens": total, "fresh_input_tokens": fresh, "cache_read_tokens": cached,
            "cache_write_tokens": writes, "output_tokens": output}


def read_signals(events):
    ids, coverage = set(), {}
    result = {"tool_calls": len(events), "full_reads": 0, "range_reads": 0, "overlapping_lines": 0, "delivered_bytes": 0}
    for event in events:
        event_id = event.get("event_id")
        if not event_id or event_id in ids or event.get("status") not in ("completed", "failed", "cancelled"):
            raise ValueError("tool events require unique IDs and explicit status")
        ids.add(event_id)
        result["delivered_bytes"] += integer(event.get("delivered_bytes", 0), "delivered_bytes")
        if event.get("kind") != "read" or event["status"] != "completed":
            continue
        first = integer(event["from_line"], "from_line")
        last = integer(event["to_line"], "to_line")
        total = integer(event["total_lines"], "total_lines")
        if not 1 <= first <= last <= total or not event.get("source_sha256"):
            raise ValueError("invalid source read range")
        result["full_reads" if first == 1 and last == total else "range_reads"] += 1
        spans = coverage.setdefault(event["source_sha256"], [])
        result["overlapping_lines"] += sum(max(0, min(last, right) - max(first, left) + 1) for left, right in spans)
        spans.append((first, last))
        merged = []
        for left, right in sorted(spans):
            if merged and left <= merged[-1][1] + 1:
                merged[-1] = (merged[-1][0], max(merged[-1][1], right))
            else:
                merged.append((left, right))
        coverage[event["source_sha256"]] = merged
    return result


def validate_record(run, record, used_request_ids):
    if not isinstance(record, dict):
        raise ValueError("run record must be an object")
    if record.get("status") != "completed":
        raise ValueError(record.get("error", "adapter/evaluator did not complete"))
    if not record.get("adapter_sha256") or not record.get("evaluator_sha256"):
        raise ValueError("adapter/evaluator identity missing")
    if record.get("run") != run:
        raise ValueError("captured run binding differs from the plan")
    result = record["result"]
    if not isinstance(result, dict) or not isinstance(record.get("evaluation"), dict):
        raise ValueError("adapter result and evaluation must be objects")
    for key in BINDINGS:
        if result.get(key) != run[key]:
            raise ValueError(f"adapter {key} mismatch")
    start, end = timestamp(record["started_at"]), timestamp(record["finished_at"])
    if end < start:
        raise ValueError("negative run interval")
    receipts = result["receipts"]
    receipt_ids = [receipt["request_id"] for receipt in receipts]
    if not receipt_ids or result.get("request_ids") != receipt_ids or len(set(receipt_ids)) != len(receipt_ids):
        raise ValueError("request receipt coverage is incomplete or duplicated")
    if used_request_ids.intersection(receipt_ids):
        raise ValueError("request receipt reused across runs")
    counts = [usage(receipt, run, start, end) for receipt in receipts]
    evaluator = record["evaluation"]
    if evaluator.get("run_id") != run["run_id"] or evaluator.get("task_sha256") != run["task_sha256"] or type(evaluator.get("accepted")) is not bool:
        raise ValueError("independent evaluation is not bound to this task run")
    evidence = evaluator.get("evidence_sha256", "")
    if len(evidence) != 64 or any(char not in "0123456789abcdef" for char in evidence):
        raise ValueError("evaluation evidence hash missing")
    if "evidence" not in evaluator or digest(evaluator["evidence"]) != evidence:
        raise ValueError("evaluation evidence body does not match its hash")
    origin = result.get("evidence_origin")
    if origin not in ("provider_response", "offline_fixture"):
        raise ValueError("unknown receipt provenance")
    costs = [receipt.get("billed_cost") for receipt in receipts]
    cost = None
    if all(costs):
        currencies = {value["currency"] for value in costs}
        if len(currencies) != 1:
            raise ValueError("mixed billing currencies")
        cost = {"amount": str(sum(Decimal(str(value["amount"])) for value in costs)), "currency": currencies.pop()}
    used_request_ids.update(receipt_ids)
    return {**{key: run[key] for key in ("run_id", "task_id", "trial", "arm")},
            "status": "validated", "evidence_origin": origin, "accepted": evaluator["accepted"],
            "latency_seconds": end - start, "request_count": len(receipts),
            "usage": {key: sum(count[key] for count in counts) for key in counts[0]},
            "cost": cost, "reads": read_signals(result.get("events", []))}


def report(plan, records):
    validate_plan(plan)
    seen, rows = set(), []
    for run in plan["runs"]:
        record = records.get(run["run_id"])
        try:
            if record is None:
                raise ValueError("missing run")
            rows.append(validate_record(run, record, seen))
        except (ValueError, KeyError, TypeError) as error:
            rows.append({"run_id": run["run_id"], "task_id": run["task_id"], "trial": run["trial"], "arm": run["arm"], "status": "invalid", "reason": str(error)})
    summaries = {}
    for arm in ARMS:
        all_rows = [row for row in rows if row["arm"] == arm]
        valid = [row for row in all_rows if row["status"] == "validated"]
        accepted = sum(row["accepted"] for row in valid)
        summary = {"expected_runs": len(all_rows), "validated_runs": len(valid), "accepted_runs": accepted,
                   "acceptance_rate": accepted / len(all_rows), "billed_cost_per_accepted_task": None}
        if len(valid) == len(all_rows) and all(row["cost"] and row["evidence_origin"] == "provider_response" for row in valid):
            currencies = {row["cost"]["currency"] for row in valid}
            if len(currencies) == 1 and accepted:
                summary["billed_cost_per_accepted_task"] = {"amount": str(sum(Decimal(row["cost"]["amount"]) for row in valid) / accepted), "currency": currencies.pop()}
        summary["provider_usage_totals"] = None
        if len(valid) == len(all_rows) and all(row["evidence_origin"] == "provider_response" for row in valid):
            summary["provider_usage_totals"] = {key: sum(row["usage"][key] for row in valid) for key in valid[0]["usage"]}
            summary["request_count"] = sum(row["request_count"] for row in valid)
            summary["median_latency_seconds"] = statistics.median(row["latency_seconds"] for row in valid)
        summaries[arm] = summary
    paired = {}
    for arm in ARMS[1:]:
        differences = []
        for task in plan["tasks"]:
            for trial in range(plan["repetitions"]):
                pair = [row for row in rows if row["task_id"] == task["id"] and row["trial"] == trial and row["arm"] in ("native", arm)]
                if len(pair) == 2 and all(row["status"] == "validated" and row["accepted"] and row["evidence_origin"] == "provider_response" for row in pair):
                    costs = {row["arm"]: row["usage"]["input_tokens"] + row["usage"]["output_tokens"] for row in pair}
                    differences.append(costs["native"] - costs[arm])
        paired[arm] = {"accepted_pairs": len(differences), "median_total_token_difference": statistics.median(differences) if differences else None,
                       "selection": "both accepted; not a quality-adjusted economic claim"}
    complete = all(row["status"] == "validated" for row in rows)
    extras = sorted(set(records) - {run["run_id"] for run in plan["runs"]})
    provider = complete and not extras and all(row["evidence_origin"] == "provider_response" for row in rows)
    return {"schema_version": 1, "plan_sha256": plan["plan_sha256"], "status": "provider_evidence_validated" if provider else "not_measured",
            "economic_claim_ready": False, "reason": "Structural evidence validation is not a representative task-quality or causal economic evaluation.",
            "extra_run_ids": extras, "arms": summaries, "paired": paired, "runs": rows}


def command(value):
    result = json.loads(value)
    if not isinstance(result, list) or not result or any(not isinstance(arg, str) or not arg for arg in result):
        raise ValueError("adapter/evaluator command must be a nonempty JSON argv array")
    return result


def invoke(argv, payload, timeout):
    try:
        result = subprocess.run(argv, input=encoded(payload), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        raise ValueError(f"command timed out after {timeout} seconds") from error
    if result.returncode:
        raise ValueError(f"command exited {result.returncode}; stderr_sha256={hashlib.sha256(result.stderr).hexdigest()}")
    if len(result.stdout) > 16 * 1024 * 1024:
        raise ValueError("adapter output exceeds 16 MiB")
    return json.loads(result.stdout)


def execute(plan, adapter, evaluator, output_dir, timeout):
    validate_plan(plan)
    tasks = {task["id"]: task for task in plan["tasks"]}
    for run in plan["runs"]:
        path = output_dir / f'{run["run_id"]}.json'
        if path.exists():
            raise ValueError("run already exists; refusing silent replay")
        record = {"run": run, "started_at": utc_now(), "adapter_sha256": digest(adapter), "evaluator_sha256": digest(evaluator)}
        try:
            repo = Path(plan["repo"])
            if git(repo, "status", "--porcelain", "--untracked-files=all") or git(repo, "rev-parse", "HEAD") != plan["repo_commit"]:
                raise ValueError("baseline checkout changed; each task must use an isolated adapter worktree")
            request = {"run": run, "task": tasks[run["task_id"]], "repo": plan["repo"], "toolchain": plan["toolchain"][run["arm"]]}
            result = invoke(adapter, request, timeout)
            record["result"] = result
            record.update(evaluation=invoke(evaluator, {**request, "result": result}, timeout), status="completed")
        except (ValueError, OSError, subprocess.SubprocessError) as error:
            record.update(status="failed", error=str(error))
        record["finished_at"] = utc_now()
        write_new(path, record)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    make = commands.add_parser("plan")
    for name in ("manifest", "repo", "toolchain-json", "output"):
        make.add_argument(f"--{name}", required=True, type=Path)
    make.add_argument("--provider", required=True, choices=("openai", "anthropic"))
    make.add_argument("--model", required=True)
    make.add_argument("--repetitions", type=int, default=3)
    make.add_argument("--seed", type=int, default=1)
    run = commands.add_parser("run")
    run.add_argument("--plan", required=True, type=Path)
    run.add_argument("--adapter-json", required=True)
    run.add_argument("--evaluator-json", required=True)
    run.add_argument("--output-dir", required=True, type=Path)
    run.add_argument("--timeout", type=int, default=1800)
    summarize = commands.add_parser("report")
    for name in ("plan", "runs", "output"):
        summarize.add_argument(f"--{name}", required=True, type=Path)
    args = parser.parse_args()
    if args.action == "plan":
        plan = make_plan(json.loads(args.manifest.read_text()), args.repo, args.provider, args.model, json.loads(args.toolchain_json.read_text()), args.repetitions, args.seed)
        write_new(args.output, plan)
    else:
        plan = json.loads(args.plan.read_text())
        if args.action == "run":
            execute(plan, command(args.adapter_json), command(args.evaluator_json), args.output_dir, args.timeout)
        else:
            records = {path.stem: json.loads(path.read_text()) for path in args.runs.glob("*.json")}
            result = report(plan, records)
            write_new(args.output, result)
            return 0 if result["status"] == "provider_evidence_validated" else 2
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        print(f"task evidence error: {error}", file=sys.stderr)
        sys.exit(2)
