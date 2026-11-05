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


EXPECTED_COUNT = 134  # 1.1.0 (US-014)
# Recomputed for 0.11.2: the ls/read/search/go_cmd accounting fixes moved existing
# diagnostics' line numbers. A (code, message, file) multiset comparison against 0.11.1
# shows no warning added or removed; the new code is clean.
# 1.1.0 (US-014): the permission gate now uses the lexer's `split_for_permissions` and
# `contains_unattestable_construct`, so their three dead_code warnings (and
# `contains_substitution`'s) are gone: 137 -> 134. No warning was added; other entries only
# moved lines in the files the upstream v1.1.0 sync edited.
EXPECTED_SHA256 = "a204e29a5e8912f5927da57800b06490f136c9034160c1596749eb43f0d48422"  # 1.1.0 (US-014)


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
