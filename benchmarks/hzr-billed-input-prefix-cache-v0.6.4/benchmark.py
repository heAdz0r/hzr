#!/usr/bin/env python3
"""Legacy local-output comparison across filter-placement arms.

This runner launches CLI commands, not agent tasks. Aggregate stats have no per-case request
binding and cannot establish provider-billed usage. Every economic comparison therefore fails
closed as not_measured. Use ../hzr-task-economics-v0.8.0 for request-correlated task evidence.
Filtering newly appended output does not itself invalidate an existing cached prefix.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

TOKEN_METHOD_DELIVERED = "ceil(UTF-8 bytes / 4); approximate, not a provider tokenizer"
TOKEN_METHOD_BILLED = "not_measured: no per-case provider request receipt channel"

# The same matrix as hzr-vs-rtk-upstream-v0.44.1, so the two benchmarks stay comparable.
CASES: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("read README.md", ("read", "README.md")),
    ("read src/main.rs", ("read", "src/main.rs")),
    ("read src/core/filter.rs", ("read", "src/core/filter.rs")),
    ("read Cargo.toml", ("read", "Cargo.toml")),
    ("ls src", ("exec", "run", "ls src")),
    ('grep -rn "fn run" src', ("search", "fn run", "--mode", "exact")),
    ('find . -name "*.rs" -type f', ("exec", "run", 'find . -name "*.rs" -type f')),
    ("git status", ("exec", "run", "git status")),
    ("git log -30", ("exec", "run", "git log -30")),
    ("git diff HEAD~5", ("exec", "run", "git diff HEAD~5")),
    ("git show HEAD", ("exec", "run", "git show HEAD")),
    ("git branch -a", ("exec", "run", "git branch -a")),
    ("cargo check", ("exec", "run", "cargo check")),
    ("cargo test", ("exec", "run", "cargo test")),
)

ARMS = ("anywhere", "turn_boundary")


@dataclass
class CaseResult:
    label: str
    arm: str
    delivered_tokens_estimated: int | None = None
    billed_input_fresh: int | None = None
    billed_input_cache_read: int | None = None
    billed_input_cache_write: int | None = None
    unmeasured_reason: str | None = None

    @property
    def measured(self) -> bool:
        return self.billed_input_fresh is not None

    def as_json(self) -> dict:
        payload = {
            "label": self.label,
            "arm": self.arm,
            "delivered_tokens_estimated": self.delivered_tokens_estimated,
        }
        if self.measured:
            payload["billed_input"] = {
                "fresh": self.billed_input_fresh,
                "cache_read": self.billed_input_cache_read,
                "cache_write": self.billed_input_cache_write,
            }
        else:
            payload["billed_input"] = "not_measured"
            payload["unmeasured_reason"] = self.unmeasured_reason or "no paired provider receipt"
        return payload


@dataclass
class ArmResult:
    arm: str
    cases: list[CaseResult] = field(default_factory=list)
    placement_deferred_operations: int = 0

    @property
    def complete(self) -> bool:
        return len(self.cases) == len(CASES) and all(case.measured for case in self.cases)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--hzr-binary", type=Path, required=True)
    parser.add_argument("--work-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--repetitions",
        type=int,
        default=3,
        help="local command repetitions per case; delivered ledger deltas are totals, not medians",
    )
    return parser.parse_args()


def fixture_commit(repo_root: Path) -> str:
    return subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=repo_root, text=True
    ).strip()


def set_arm(hzr: Path, config: Path, arm: str) -> None:
    """Point the config at one placement arm.

    Written through the config file rather than an environment override so the run exercises the
    same code path an operator would.
    """
    text = config.read_text() if config.exists() else ""
    if "[policy]" not in text:
        text += "\n[policy]\n"
    lines = [line for line in text.splitlines() if not line.startswith("filter_placement")]
    out: list[str] = []
    for line in lines:
        out.append(line)
        if line.strip() == "[policy]":
            out.append(f'filter_placement = "{arm}"')
    config.write_text("\n".join(out) + "\n")


def stats_json(hzr: Path, config: Path, repo_root: Path) -> dict | str:
    """Read the ledger, or return why it could not be read.

    A failure here is reported as an unmeasured case rather than crashing the run: a benchmark
    that dies partway leaves no record of which cases it did cover, and the doctrine in the
    README is that a dropped case must be listed with its reason.
    """
    completed = subprocess.run(
        [str(hzr), "--config", str(config), "stats", "--json", "--workspace", str(repo_root)],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip().splitlines()
        return f"hzr stats failed: {detail[0] if detail else 'no output'}"
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        return f"hzr stats emitted unparseable JSON: {error}"


def run_case(hzr: Path, config: Path, repo_root: Path, argv: tuple[str, ...]) -> int:
    completed = subprocess.run(
        [str(hzr), "--config", str(config), *argv],
        cwd=repo_root,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return completed.returncode


def collect_receipt(stats: dict) -> tuple[int | None, int | None, int | None, str | None]:
    """Read billed input from provider receipts, or say why it is unavailable.

    `observed_model_usage.tasks == 0` means no provider-billed task was recorded. That is not a
    zero-cost run; it is an unmeasured one, and the distinction is the whole point.
    """
    del stats
    return None, None, None, (
        "aggregate stats cannot identify this case or repetition; "
        "use the v0.8.0 task harness with per-request provider receipts"
    )


def run_arm(args: argparse.Namespace, config: Path, arm: str) -> ArmResult:
    set_arm(args.hzr_binary, config, arm)
    result = ArmResult(arm=arm)
    for label, argv in CASES:
        before = stats_json(args.hzr_binary, config, args.repo_root)
        exit_codes = [run_case(args.hzr_binary, config, args.repo_root, argv)
                      for _ in range(args.repetitions)]
        after = stats_json(args.hzr_binary, config, args.repo_root)

        if isinstance(before, str) or isinstance(after, str):
            unreadable = before if isinstance(before, str) else after
            result.cases.append(
                CaseResult(label=label, arm=arm, unmeasured_reason=unreadable)
            )
            continue

        delivered_before = (before.get("direct_savings") or {}).get(
            "delivered_tokens_estimated", 0
        )
        delivered_after = (after.get("direct_savings") or {}).get("delivered_tokens_estimated", 0)
        fresh, cache_read, cache_write, reason = collect_receipt(after)
        if any(exit_codes):
            reason = f"command repetitions failed with exit codes {exit_codes}; no economic comparison"
        result.cases.append(
            CaseResult(
                label=label,
                arm=arm,
                delivered_tokens_estimated=max(0, delivered_after - delivered_before),
                billed_input_fresh=fresh,
                billed_input_cache_read=cache_read,
                billed_input_cache_write=cache_write,
                unmeasured_reason=reason,
            )
        )
    return result


def compare(arms: dict[str, ArmResult]) -> dict:
    """Emit a comparison only when both arms actually measured the same case list.

    A partial comparison is worse than none: it would be read as evidence about prefix-cache
    behaviour while resting on whichever cases happened to produce a receipt.
    """
    if set(arms) != set(ARMS) or not all(arm.complete for arm in arms.values()):
        missing = {
            name: [case.label for case in arm.cases if not case.measured]
            for name, arm in arms.items()
        }
        return {
            "status": "not_measured",
            "reason": "billed input requires a paired provider receipt for every case in both arms",
            "unmeasured_cases": missing,
        }
    totals = {
        name: {
            "billed_input_fresh": sum(c.billed_input_fresh or 0 for c in arm.cases),
            "billed_input_cache_read": sum(c.billed_input_cache_read or 0 for c in arm.cases),
            "delivered_tokens_estimated": sum(
                c.delivered_tokens_estimated or 0 for c in arm.cases
            ),
        }
        for name, arm in arms.items()
    }
    # Do not infer prefix causality from two scalar totals.
    return {
        "status": "measured",
        "totals": totals,
        "hypothesis_supported": None,
        "reading": "Descriptive arm totals cannot establish cache invalidation causality.",
    }


def main() -> int:
    args = parse_args()
    if args.repetitions < 1:
        raise ValueError("repetitions must be positive")
    args.work_root.mkdir(parents=True, exist_ok=True)
    config = args.work_root / "config.toml"
    if not config.exists():
        config.write_text(f'schema_version = 1\ndata_dir = "{args.work_root}"\n\n[policy]\n')

    arms = {arm: run_arm(args, config, arm) for arm in ARMS}
    payload = {
        "benchmark": "hzr-billed-input-prefix-cache-v0.6.4",
        "fixture_commit": fixture_commit(args.repo_root),
        "repetitions": args.repetitions,
        "token_method_delivered": TOKEN_METHOD_DELIVERED,
        "token_method_billed": TOKEN_METHOD_BILLED,
        "arms": {
            name: {
                "placement_deferred_operations": arm.placement_deferred_operations,
                "cases": [case.as_json() for case in arm.cases],
            }
            for name, arm in arms.items()
        },
        "comparison": compare(arms),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2) + "\n")
    print(json.dumps(payload["comparison"], indent=2))
    # An unmeasured comparison is a non-zero exit: a benchmark that "succeeds" without measuring
    # anything is how an unproven claim gets into a README.
    return 0 if payload["comparison"]["status"] == "measured" else 2


if __name__ == "__main__":
    sys.exit(main())
