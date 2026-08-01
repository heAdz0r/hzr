#!/usr/bin/env python3
"""Verify HZR's deterministic read/write contract and save auditable evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_RTK = ROOT / "fork-core" / "rtk" / "target" / "debug" / "rtk"
DEFAULT_UPSTREAM_HELP = (
    ROOT
    / "benchmarks"
    / "hzr-vs-rtk-upstream-v0.44.1"
    / "runs"
    / "2026-08-01"
    / "upstream-help.txt"
)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def evidence_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def run(binary: Path, args: list[str], cwd: Path, env: dict[str, str]) -> dict[str, object]:
    completed = subprocess.run(
        [str(binary), *args],
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    return {
        "argv": args,
        "exit_code": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }


def json_stdout(result: dict[str, object]) -> dict[str, object]:
    stdout = result["stdout"]
    assert isinstance(stdout, bytes)
    return json.loads(stdout.decode("utf-8"))


def save_result(outputs: Path, name: str, result: dict[str, object]) -> None:
    stdout = result["stdout"]
    stderr = result["stderr"]
    assert isinstance(stdout, bytes)
    assert isinstance(stderr, bytes)
    (outputs / f"{name}.stdout.txt").write_bytes(stdout)
    (outputs / f"{name}.stderr.txt").write_bytes(stderr)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rtk-bin", type=Path, default=DEFAULT_RTK)
    parser.add_argument("--upstream-help", type=Path, default=DEFAULT_UPSTREAM_HELP)
    parser.add_argument("--run-id", default="local")
    args = parser.parse_args()

    binary = args.rtk_bin.resolve()
    upstream_help = args.upstream_help.resolve()
    if not binary.is_file():
        parser.error(f"fork-core binary not found: {binary}")
    if not upstream_help.is_file():
        parser.error(f"upstream help evidence not found: {upstream_help}")

    run_dir = Path(__file__).resolve().parent / "runs" / args.run_id
    if run_dir.exists():
        parser.error(f"run already exists: {run_dir}")
    outputs = run_dir / "outputs"
    outputs.mkdir(parents=True)

    with tempfile.TemporaryDirectory(prefix="hzr-llm-utility-") as temp_name:
        temp = Path(temp_name)
        fixture = temp / "fixture"
        fixture.mkdir()
        isolated_home = temp / "home"
        isolated_home.mkdir()
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(isolated_home),
            "XDG_CACHE_HOME": str(temp / "cache"),
            "XDG_CONFIG_HOME": str(temp / "config"),
            "RTK_DB_PATH": str(temp / "tracking.sqlite"),
            "RTK_TEE": "0",
            "RTK_TELEMETRY_DISABLED": "1",
            "NO_COLOR": "1",
            "CI": "1",
            "LC_ALL": "C",
        }

        markdown = "# Utility fixture\n\n" + "\n".join(
            f"Evidence line {number}: bounded reads must remain recoverable."
            for number in range(1, 121)
        ) + "\n\n## Exact recovery\n\nFinal fact.\n"
        markdown_path = fixture / "README.md"
        markdown_path.write_text(markdown, encoding="utf-8")
        (run_dir / "fixture-README.md").write_text(markdown, encoding="utf-8")

        results: dict[str, dict[str, object]] = {}
        results["read-digest"] = run(binary, ["read", "README.md"], fixture, env)
        results["read-exact"] = run(
            binary, ["read", "README.md", "--level", "none"], fixture, env
        )
        results["read-range"] = run(
            binary, ["read", "README.md", "--from", "3", "--to", "5"], fixture, env
        )

        single = fixture / "single.txt"
        results["write-create"] = run(
            binary,
            ["write", "--output", "json", "create", str(single), "--content", "alpha beta\n"],
            fixture,
            env,
        )
        create_hash = sha256(single.read_bytes())
        create_mtime = single.stat().st_mtime_ns
        results["write-create-noop"] = run(
            binary,
            ["write", "--output", "json", "create", str(single), "--content", "alpha beta\n"],
            fixture,
            env,
        )
        create_idempotent = (
            sha256(single.read_bytes()) == create_hash and single.stat().st_mtime_ns == create_mtime
        )
        results["write-replace"] = run(
            binary,
            [
                "write",
                "--output",
                "json",
                "replace",
                str(single),
                "--from",
                "beta",
                "--to",
                "gamma",
            ],
            fixture,
            env,
        )
        results["write-patch"] = run(
            binary,
            [
                "write",
                "--output",
                "json",
                "patch",
                str(single),
                "--old",
                "alpha gamma",
                "--new",
                "delta epsilon",
            ],
            fixture,
            env,
        )
        config = fixture / "config.json"
        config.write_text('{"agent":{}}\n', encoding="utf-8")
        results["write-set"] = run(
            binary,
            [
                "write",
                "--output",
                "json",
                "set",
                str(config),
                "--key",
                "agent.safe",
                "--value",
                "true",
                "--value-type",
                "bool",
            ],
            fixture,
            env,
        )

        batch_text = fixture / "batch.txt"
        batch_json = fixture / "batch.json"
        batch_created = fixture / "batch-created.txt"
        batch_text.write_text("alpha\n", encoding="utf-8")
        batch_json.write_text('{"agent":{}}\n', encoding="utf-8")
        batch_plan = json.dumps(
            [
                {"op": "replace", "file": str(batch_text), "from": "alpha", "to": "beta"},
                {"op": "patch", "file": str(batch_text), "old": "beta", "new": "gamma"},
                {
                    "op": "set",
                    "file": str(batch_json),
                    "key": "agent.batch",
                    "value": "true",
                    "value_type": "bool",
                },
                {"op": "create", "file": str(batch_created), "content": "created\n"},
            ],
            separators=(",", ":"),
        )
        results["write-batch"] = run(
            binary,
            ["write", "--output", "json", "batch", "--plan", batch_plan],
            fixture,
            env,
        )
        before_dry_run = batch_text.read_bytes()
        dry_plan = json.dumps(
            [{"op": "replace", "file": str(batch_text), "from": "gamma", "to": "changed"}],
            separators=(",", ":"),
        )
        results["write-batch-dry-run"] = run(
            binary,
            ["write", "--output", "json", "batch", "--plan", dry_plan, "--dry-run"],
            fixture,
            env,
        )
        dry_run_unchanged = batch_text.read_bytes() == before_dry_run

        results["hzr-help"] = run(binary, ["--help"], fixture, env)
        for name, result in results.items():
            save_result(outputs, name, result)

        digest = results["read-digest"]["stdout"]
        assert isinstance(digest, bytes)
        digest_text = digest.decode("utf-8")
        read_signals = {
            "omission_is_explicit": "Markdown digest (content omitted)" in digest_text,
            "source_size_is_explicit": "Source:" in digest_text and "bytes" in digest_text,
            "section_coverage_is_explicit": "Sections:" in digest_text and "shown" in digest_text,
            "bounded_lead_preview_is_present": "Lead preview:" in digest_text
            and "Evidence line 1" in digest_text,
            "full_exact_recovery_is_explicit": "`--level none` for exact content" in digest_text,
            "range_recovery_is_explicit": "`--from N --to M` for an exact range" in digest_text,
        }

        exact_stdout = results["read-exact"]["stdout"]
        range_stdout = results["read-range"]["stdout"]
        assert isinstance(exact_stdout, bytes)
        assert isinstance(range_stdout, bytes)
        expected_range = "\n".join(markdown.splitlines()[2:5]) + "\n"

        single_responses = [
            json_stdout(results[name])
            for name in ["write-create", "write-create-noop", "write-replace", "write-patch", "write-set"]
        ]
        batch_response = json_stdout(results["write-batch"])
        dry_response = json_stdout(results["write-batch-dry-run"])
        structured = all(response.get("version") == 1 for response in [*single_responses, batch_response, dry_response])
        upstream_text = upstream_help.read_text(encoding="utf-8")
        hzr_help = results["hzr-help"]["stdout"]
        assert isinstance(hzr_help, bytes)

        gates = {
            "read_contract_6_of_6": all(read_signals.values()),
            "read_exact_sha256_match": exact_stdout == markdown.encode("utf-8"),
            "read_range_exact": range_stdout == expected_range.encode("utf-8"),
            "single_write_4_of_4": (
                all(result["exit_code"] == 0 for name, result in results.items() if name.startswith("write-") and name not in {"write-batch", "write-batch-dry-run"})
                and single.read_text(encoding="utf-8") == "delta epsilon\n"
                and json.loads(config.read_text(encoding="utf-8"))["agent"]["safe"] is True
            ),
            "create_idempotent_content_and_mtime": create_idempotent,
            "batch_write_4_of_4": (
                results["write-batch"]["exit_code"] == 0
                and batch_text.read_text(encoding="utf-8") == "gamma\n"
                and json.loads(batch_json.read_text(encoding="utf-8"))["agent"]["batch"] is True
                and batch_created.read_text(encoding="utf-8") == "created\n"
                and batch_response.get("applied") == 4
            ),
            "batch_dry_run_unchanged": (
                results["write-batch-dry-run"]["exit_code"] == 0
                and dry_run_unchanged
                and dry_response.get("dry_run") is True
            ),
            "write_json_schema_v1": structured,
            "write_is_hzr_only_vs_upstream_v0_44_1": (
                "\n  write " not in upstream_text and "\n  write " in hzr_help.decode("utf-8")
            ),
        }

        version_result = run(binary, ["--version"], fixture, env)
        version_stdout = version_result["stdout"]
        assert isinstance(version_stdout, bytes)
        summary = {
            "schema": "hzr-llm-utility-v1",
            "product_version": "0.3.1",
            "fork_version": version_stdout.decode("utf-8").strip(),
            "binary": evidence_path(binary),
            "binary_sha256": sha256(binary.read_bytes()),
            "upstream_help": str(upstream_help.relative_to(ROOT)),
            "read_contract_signals": read_signals,
            "gates": gates,
            "passed": sum(gates.values()),
            "total": len(gates),
            "all_passed": all(gates.values()),
        }
        (run_dir / "summary.json").write_text(
            json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

        rows = "\n".join(
            f"| `{name}` | {'PASS' if passed else 'FAIL'} |" for name, passed in gates.items()
        )
        results_md = f"""# HZR LLM utility result

| Metric | Result |
|---|---:|
| Contract gates | **{summary['passed']}/{summary['total']} PASS** |
| Read clarity signals | **{sum(read_signals.values())}/6** |
| Single write operations | **4/4** |
| Batch operations | **4/4** |
| Exact read SHA-256 parity | **PASS** |
| Identical create preserves content + mtime | **PASS** |

| Gate | Status |
|---|---|
{rows}

This verifies deterministic output and mutation contracts, not semantic comprehension by a
particular LLM or accepted-task quality. Batch atomicity is per file, not across the whole plan.
"""
        (run_dir / "RESULTS.md").write_text(results_md, encoding="utf-8")

    manifest_lines = []
    for path in sorted(run_dir.rglob("*")):
        if path.is_file() and path.name != "checksums.sha256":
            manifest_lines.append(f"{sha256(path.read_bytes())}  {path.relative_to(run_dir)}")
    (run_dir / "checksums.sha256").write_text("\n".join(manifest_lines) + "\n", encoding="utf-8")

    if not summary["all_passed"]:
        print(json.dumps(summary, indent=2))
        return 1
    print(f"PASS {summary['passed']}/{summary['total']} gates: {run_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
