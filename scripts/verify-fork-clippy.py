#!/usr/bin/env python3
"""Reject any drift in the inherited fork-core warning set.

The imported engine still contains upstream warning debt. Suppressing those lints would
hide new defects, while making the release wait on unrelated cleanup would create broad
parity churn. This ratchet hashes the exact warning code, message, source path and line for
all targets. Any addition, removal or movement is therefore an explicit reviewed change.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path


EXPECTED_COUNT = 141
# Recomputed after the 0.8.0 Git diff status fix moved the same needless_return diagnostic
# from git.rs:1226 to :1229 in two targets. Reverse line mapping reproduces the prior hash;
# no warning code/message/source identity was added or removed (see the verification report).
EXPECTED_SHA256 = "5b013a3c862ea687ad6e8c12a9ce9be2370113b7620a60018c81a90fcd19bcd8"


def main() -> int:
    repository = Path(__file__).resolve().parent.parent
    command = [
        "cargo",
        "clippy",
        "--manifest-path",
        str(repository / "fork-core/rtk/Cargo.toml"),
        "--all-targets",
        "--all-features",
        "--message-format=json",
    ]
    process = subprocess.Popen(
        command,
        cwd=repository,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert process.stdout is not None
    rows: list[str] = []
    for line in process.stdout:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("reason") != "compiler-message":
            continue
        message = event.get("message", {})
        if message.get("level") != "warning":
            continue
        spans = [span for span in message.get("spans", []) if span.get("is_primary")]
        span = spans[0] if spans else {}
        code = (message.get("code") or {}).get("code", "")
        rows.append(
            "|".join(
                [
                    code,
                    message.get("message", ""),
                    span.get("file_name", ""),
                    str(span.get("line_start", 0)),
                ]
            )
        )
    stderr = process.stderr.read() if process.stderr is not None else ""
    return_code = process.wait()
    if return_code != 0:
        sys.stderr.write(stderr)
        print(f"fork-core Clippy failed with exit code {return_code}", file=sys.stderr)
        return 1

    rows.sort()
    digest = hashlib.sha256("\n".join(rows).encode()).hexdigest()
    if len(rows) != EXPECTED_COUNT or digest != EXPECTED_SHA256:
        print("fork-core Clippy warning baseline changed", file=sys.stderr)
        print(
            f"expected count={EXPECTED_COUNT} sha256={EXPECTED_SHA256}",
            file=sys.stderr,
        )
        print(f"actual   count={len(rows)} sha256={digest}", file=sys.stderr)
        return 1
    print(f"fork-core Clippy baseline {digest} verified ({len(rows)} warnings)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
