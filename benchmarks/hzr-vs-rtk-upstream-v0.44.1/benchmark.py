#!/usr/bin/env python3
"""Reproducible RAW vs upstream RTK vs HZR command-output benchmark."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import shutil
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


UPSTREAM_COMMIT = "36591fb00d650bf987b57483c0b3a395a35a8dc1"
TOKEN_METHOD = "ceil(UTF-8 bytes / 4); approximate, not a provider tokenizer"


@dataclass(frozen=True)
class Case:
    label: str
    raw: tuple[str, ...]
    filtered: tuple[str, ...]

    @property
    def slug(self) -> str:
        safe = "".join(char if char.isalnum() else "-" for char in self.label.lower())
        return "-".join(part for part in safe.split("-") if part)


CASES = (
    Case("read README.md", ("cat", "README.md"), ("read", "README.md")),
    Case("read src/main.rs", ("cat", "src/main.rs"), ("read", "src/main.rs")),
    Case(
        "read src/core/filter.rs",
        ("cat", "src/core/filter.rs"),
        ("read", "src/core/filter.rs"),
    ),
    Case("read Cargo.toml", ("cat", "Cargo.toml"), ("read", "Cargo.toml")),
    Case("ls src", ("ls", "src"), ("ls", "src")),
    Case(
        'grep -rn "fn run" src',
        ("grep", "-rn", "fn run", "src"),
        ("grep", "-rn", "fn run", "src"),
    ),
    Case(
        'find . -name "*.rs" -type f',
        ("find", ".", "-name", "*.rs", "-type", "f"),
        ("find", ".", "-name", "*.rs", "-type", "f"),
    ),
    Case("git status", ("git", "status"), ("git", "status")),
    Case("git log -30", ("git", "log", "-30"), ("git", "log", "-30")),
    Case(
        "git diff HEAD~5",
        ("git", "diff", "HEAD~5"),
        ("git", "diff", "HEAD~5"),
    ),
    Case("git show HEAD", ("git", "show", "HEAD"), ("git", "show", "HEAD")),
    Case("git branch -a", ("git", "branch", "-a"), ("git", "branch", "-a")),
    Case("cargo check", ("cargo", "check"), ("cargo", "check")),
    Case("cargo test", ("cargo", "test"), ("cargo", "test")),
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--upstream-binary", type=Path, required=True)
    parser.add_argument("--hzr-binary", type=Path, required=True)
    parser.add_argument("--hzr-engine", type=Path, required=True)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work-root", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    return parser.parse_args()


def checked_output(command: list[str], cwd: Path | None = None) -> str:
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def prepare_layout(args: argparse.Namespace) -> tuple[Path, Path]:
    args.output.mkdir(parents=True, exist_ok=True)
    args.work_root.mkdir(parents=True, exist_ok=True)
    engine_dir = args.work_root / "engines"
    engine_dir.mkdir(parents=True, exist_ok=True)
    engine_link = engine_dir / "rtk"
    if engine_link.exists() or engine_link.is_symlink():
        engine_link.unlink()
    engine_link.symlink_to(args.hzr_engine.resolve())

    data_dir = args.work_root / "hzr-data"
    config_path = args.work_root / "hzr-config.toml"
    config_path.write_text(
        "\n".join(
            (
                "schema_version = 1",
                f'data_dir = "{data_dir}"',
                "",
                "[daemon]",
                'bind = "127.0.0.1:47393"',
                "request_limit_bytes = 1048576",
                "request_timeout_ms = 30000",
                "",
                "[engines]",
                f'directory = "{engine_dir}"',
                "strict_versions = true",
                "auto_start_icm = false",
                "icm_embeddings = false",
                "auto_index = false",
                "",
                "[policy]",
                'codec_profile = "adaptive"',
                "context_token_limit = 16000",
                "output_reserve = 2000",
                "safety_margin = 1000",
                "",
                "[privacy]",
                "telemetry = false",
                "raw_retention_seconds = 0",
                "redact_secrets = true",
                "",
            )
        ),
        encoding="utf-8",
    )
    return config_path, data_dir


def participant_env(args: argparse.Namespace, kind: str) -> dict[str, str]:
    home = args.work_root / "homes" / kind
    xdg = home / "xdg"
    xdg.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(xdg),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "CARGO_TARGET_DIR": str(args.work_root / "fixture-target"),
            "RTK_DB_PATH": str(args.work_root / f"{kind}-tracking.sqlite"),
            "RTK_TEE": "0",
            "RTK_TELEMETRY_DISABLED": "1",
            "NO_COLOR": "1",
            "CI": "1",
            "LC_ALL": "C",
            "LANG": "C",
            "COLUMNS": "120",
        }
    )
    return env


def command_for(
    case: Case, kind: str, args: argparse.Namespace, config_path: Path
) -> list[str]:
    if kind == "raw":
        return list(case.raw)
    if kind == "upstream":
        return [str(args.upstream_binary), *case.filtered]
    return [
        str(args.hzr_binary),
        "--config",
        str(config_path),
        "rtk",
        "--",
        *case.filtered,
    ]


def run_once(
    case: Case,
    kind: str,
    args: argparse.Namespace,
    config_path: Path,
) -> dict[str, object]:
    started = time.perf_counter()
    process = subprocess.run(
        command_for(case, kind, args, config_path),
        cwd=args.fixture,
        env=participant_env(args, kind),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    output = process.stdout + process.stderr
    return {
        "elapsed_ms": round(elapsed_ms, 3),
        "exit_code": process.returncode,
        "bytes": len(output),
        "tokens_est": math.ceil(len(output) / 4),
        "sha256": sha256_bytes(output),
        "output": output,
    }


def median_int(values: list[int]) -> int:
    return int(statistics.median(values))


def summarize_samples(samples: list[dict[str, object]]) -> dict[str, object]:
    canonical = min(
        samples,
        key=lambda sample: (
            abs(int(sample["tokens_est"]) - median_int([int(s["tokens_est"]) for s in samples])),
            float(sample["elapsed_ms"]),
        ),
    )
    return {
        "bytes_p50": median_int([int(sample["bytes"]) for sample in samples]),
        "tokens_est_p50": median_int([int(sample["tokens_est"]) for sample in samples]),
        "latency_p50_ms": round(
            statistics.median(float(sample["elapsed_ms"]) for sample in samples), 1
        ),
        "latency_min_ms": round(min(float(sample["elapsed_ms"]) for sample in samples), 1),
        "exit_codes": [int(sample["exit_code"]) for sample in samples],
        "samples": [
            {
                key: sample[key]
                for key in ("elapsed_ms", "exit_code", "bytes", "tokens_est", "sha256")
            }
            for sample in samples
        ],
        "canonical_output": bytes(canonical["output"]),
    }


def percent_reduction(candidate: int, baseline: int) -> float:
    if baseline == 0:
        return 0.0
    return round((1 - candidate / baseline) * 100, 1)


def capture_help(args: argparse.Namespace, config_path: Path) -> None:
    commands = {
        "upstream-help.txt": [str(args.upstream_binary), "--help"],
        "hzr-fork-help.txt": [
            str(args.hzr_binary),
            "--config",
            str(config_path),
            "rtk",
            "--",
            "--help",
        ],
    }
    for filename, command in commands.items():
        process = subprocess.run(
            command,
            cwd=args.fixture,
            env=participant_env(args, filename),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
        )
        (args.output / filename).write_bytes(process.stdout)


def capture_environment(args: argparse.Namespace) -> None:
    fixture_commit = checked_output(["git", "rev-parse", "HEAD"], args.fixture)
    if fixture_commit != UPSTREAM_COMMIT:
        raise SystemExit(
            f"fixture commit mismatch: expected {UPSTREAM_COMMIT}, got {fixture_commit}"
        )
    status = checked_output(["git", "status", "--short"], args.repo_root)
    diff = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=args.repo_root)
    environment = {
        "fixture_repository": "https://github.com/rtk-ai/rtk.git",
        "fixture_commit": fixture_commit,
        "upstream_version": checked_output([str(args.upstream_binary), "--version"]),
        "hzr_version": checked_output([str(args.hzr_binary), "--version"]),
        "hzr_engine_version": checked_output([str(args.hzr_engine), "--version"]),
        "hzr_repository_head": checked_output(["git", "rev-parse", "HEAD"], args.repo_root),
        "hzr_worktree_status": status.splitlines(),
        "hzr_worktree_diff_sha256": sha256_bytes(diff),
        "upstream_binary_sha256": sha256_file(args.upstream_binary),
        "hzr_binary_sha256": sha256_file(args.hzr_binary),
        "hzr_engine_binary_sha256": sha256_file(args.hzr_engine),
        "rustc": checked_output(["rustc", "--version"]),
        "cargo": checked_output(["cargo", "--version"]),
        "python": sys.version.splitlines()[0],
        "platform": platform.platform(),
        "machine": platform.machine(),
        "token_method": TOKEN_METHOD,
        "environment_policy": "separate empty HOME/XDG roots; shared fixture and Cargo target; telemetry and tee disabled",
    }
    write_json(args.output / "environment.json", environment)


def write_checksums(output: Path) -> None:
    checksum_path = output / "checksums.sha256"
    files = sorted(path for path in output.rglob("*") if path.is_file() and path != checksum_path)
    checksum_path.write_text(
        "".join(f"{sha256_file(path)}  {path.relative_to(output)}\n" for path in files),
        encoding="utf-8",
    )


def write_markdown(output: Path, rows: list[dict[str, object]]) -> None:
    totals = {
        kind: sum(int(row[kind]["tokens_est_p50"]) for row in rows)
        for kind in ("raw", "upstream", "hzr")
    }
    hzr_vs_raw = percent_reduction(totals["hzr"], totals["raw"])
    hzr_vs_upstream = percent_reduction(totals["hzr"], totals["upstream"])
    lines = [
        "# Recorded result: 2026-08-01",
        "",
        "Five repetitions per case, rotating order. Tokens are estimated as ",
        "`ceil(UTF-8 bytes / 4)` and are not provider-billed tokens.",
        "",
        "| Aggregate | RAW | Upstream RTK | HZR | HZR vs RAW | HZR vs upstream |",
        "|---|---:|---:|---:|---:|---:|",
        (
            f"| 14 cases | {totals['raw']:,} | {totals['upstream']:,} | "
            f"**{totals['hzr']:,}** | **−{hzr_vs_raw:.1f}%** | "
            f"**−{hzr_vs_upstream:.1f}%** |"
        ),
        "",
        "| Case | RAW | Upstream | HZR | HZR vs upstream |",
        "|---|---:|---:|---:|---:|",
    ]
    for row in rows:
        delta = float(row["hzr_reduction_vs_upstream_pct"])
        comparison = "parity" if delta == 0 else f"−{delta:.1f}%"
        lines.append(
            f"| `{row['case']}` | {row['raw']['tokens_est_p50']:,} | "
            f"{row['upstream']['tokens_est_p50']:,} | "
            f"{row['hzr']['tokens_est_p50']:,} | {comparison} |"
        )
    lines.extend(
        (
            "",
            "Inspect [`summary.json`](summary.json) for every repetition and "
            "[`outputs/`](outputs) for canonical full output. Verify all artifacts with:",
            "",
            "```bash",
            "shasum -a 256 -c checksums.sha256",
            "```",
            "",
        )
    )
    (output / "RESULTS.md").write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    args = parse_args()
    if args.repetitions < 1:
        raise SystemExit("--repetitions must be positive")
    for path in (args.fixture, args.upstream_binary, args.hzr_binary, args.hzr_engine):
        if not path.exists():
            raise SystemExit(f"required path does not exist: {path}")

    config_path, _ = prepare_layout(args)
    capture_environment(args)
    capture_help(args, config_path)
    output_root = args.output / "outputs"
    output_root.mkdir(exist_ok=True)

    rows = []
    kinds = ("raw", "upstream", "hzr")
    for case in CASES:
        samples = {kind: [] for kind in kinds}
        for repetition in range(args.repetitions):
            rotation = repetition % len(kinds)
            order = kinds[rotation:] + kinds[:rotation]
            for kind in order:
                samples[kind].append(run_once(case, kind, args, config_path))

        row: dict[str, object] = {"case": case.label, "slug": case.slug}
        case_output = output_root / case.slug
        case_output.mkdir(exist_ok=True)
        for kind in kinds:
            summary = summarize_samples(samples[kind])
            canonical_output = summary.pop("canonical_output")
            output_path = case_output / f"{kind}.txt"
            output_path.write_bytes(canonical_output)
            summary["canonical_output"] = str(output_path.relative_to(args.output))
            row[kind] = summary

        raw_tokens = int(row["raw"]["tokens_est_p50"])
        upstream_tokens = int(row["upstream"]["tokens_est_p50"])
        hzr_tokens = int(row["hzr"]["tokens_est_p50"])
        row["upstream_reduction_vs_raw_pct"] = percent_reduction(upstream_tokens, raw_tokens)
        row["hzr_reduction_vs_raw_pct"] = percent_reduction(hzr_tokens, raw_tokens)
        row["hzr_reduction_vs_upstream_pct"] = percent_reduction(hzr_tokens, upstream_tokens)
        rows.append(row)
        print(
            f"{case.label}: RAW {raw_tokens}, upstream {upstream_tokens}, HZR {hzr_tokens}",
            flush=True,
        )

    result = {
        "schema_version": 1,
        "fixture_commit": UPSTREAM_COMMIT,
        "repetitions": args.repetitions,
        "token_method": TOKEN_METHOD,
        "rows": rows,
    }
    write_json(args.output / "summary.json", result)

    with (args.output / "summary.csv").open("w", newline="", encoding="utf-8") as handle:
        fieldnames = [
            "case",
            "raw_tokens_est_p50",
            "upstream_tokens_est_p50",
            "hzr_tokens_est_p50",
            "upstream_reduction_vs_raw_pct",
            "hzr_reduction_vs_raw_pct",
            "hzr_reduction_vs_upstream_pct",
            "raw_latency_p50_ms",
            "upstream_latency_p50_ms",
            "hzr_latency_p50_ms",
            "exit_codes_equal",
        ]
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for row in rows:
            writer.writerow(
                {
                    "case": row["case"],
                    "raw_tokens_est_p50": row["raw"]["tokens_est_p50"],
                    "upstream_tokens_est_p50": row["upstream"]["tokens_est_p50"],
                    "hzr_tokens_est_p50": row["hzr"]["tokens_est_p50"],
                    "upstream_reduction_vs_raw_pct": row[
                        "upstream_reduction_vs_raw_pct"
                    ],
                    "hzr_reduction_vs_raw_pct": row["hzr_reduction_vs_raw_pct"],
                    "hzr_reduction_vs_upstream_pct": row[
                        "hzr_reduction_vs_upstream_pct"
                    ],
                    "raw_latency_p50_ms": row["raw"]["latency_p50_ms"],
                    "upstream_latency_p50_ms": row["upstream"]["latency_p50_ms"],
                    "hzr_latency_p50_ms": row["hzr"]["latency_p50_ms"],
                    "exit_codes_equal": row["raw"]["exit_codes"]
                    == row["upstream"]["exit_codes"]
                    == row["hzr"]["exit_codes"],
                }
            )

    write_markdown(args.output, rows)
    write_checksums(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
