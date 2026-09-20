#!/usr/bin/env python3
"""Build-time verification of the immutable orchestration pattern source."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
UPSTREAM = ROOT / "upstream"


def verify_upstream():
    provenance = json.loads((ROOT / "PROVENANCE.json").read_text())
    manifest = UPSTREAM / "MANIFEST.sha256"
    if hashlib.sha256(manifest.read_bytes()).hexdigest() != provenance["manifest_sha256"]:
        raise ValueError("Pinned upstream manifest checksum mismatch")
    expected = set()
    for line in manifest.read_text().splitlines():
        digest, relative = line.split("  ", 1)
        if Path(relative).is_absolute() or ".." in Path(relative).parts:
            raise ValueError("Unsafe upstream inventory path")
        path = UPSTREAM / relative
        if any(p.is_symlink() for p in [path, *path.parents]):
            raise ValueError("Symlink in upstream source")
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError("Pinned upstream source checksum mismatch: " + relative)
        expected.add(relative)
    actual = {p.relative_to(UPSTREAM).as_posix() for p in UPSTREAM.rglob("*") if p.is_file()}
    if actual != expected | {"MANIFEST.sha256"}:
        raise ValueError("Upstream inventory differs from pinned source")
    print("Pinned upstream inventory verified.")


if __name__ == "__main__":
    verify_upstream()
