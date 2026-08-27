#!/usr/bin/env python3
"""Verify that CI and release publishing share the complete fail-closed gate."""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parent.parent


class GateError(RuntimeError):
    pass


def require(text: str, needle: str, context: str) -> None:
    if needle not in text:
        raise GateError(f"{context} must contain `{needle}`")


def forbid(text: str, needle: str, context: str) -> None:
    if needle in text:
        raise GateError(f"{context} must not contain `{needle}`")


def workflow_job(workflow: str, name: str) -> str:
    marker = f"  {name}:\n"
    start = workflow.find(marker)
    if start < 0:
        raise GateError(f"workflow is missing `{name}` job")
    start += len(marker)
    end = len(workflow)
    for line_start in range(start, len(workflow)):
        if line_start != start and workflow[line_start - 1] != "\n":
            continue
        line_end = workflow.find("\n", line_start)
        if line_end < 0:
            line_end = len(workflow)
        line = workflow[line_start:line_end]
        if line.startswith("  ") and not line.startswith("    ") and line.endswith(":"):
            end = line_start
            break
    return workflow[start:end]


def verify_script(script: str, build_script: str, fork_verifier: str) -> None:
    for command in (
        "python3 scripts/verify-release-gates.py --self-test",
        "bash -n scripts/*.sh",
        "cargo fmt --all --check",
        "cargo clippy --locked --workspace --all-targets --all-features -- -D warnings",
        "cargo test --locked --workspace --all-targets --all-features",
        "scripts/verify-fork-core.sh --test",
        '"${HZR_REPOSITORY_ROOT}/scripts/build-bundle.sh" "$2"',
    ):
        require(script, command, "scripts/complete-gate.sh")
    require(build_script, "scripts/smoke-bundle.sh", "scripts/build-bundle.sh")
    require(
        fork_verifier,
        'scripts/verify-fork-clippy.py"',
        "scripts/verify-fork-core.sh --test branch",
    )


def verify_ci(workflow: str) -> None:
    complete = workflow_job(workflow, "complete-gate")
    assembled = workflow_job(workflow, "assembled-bundle")
    require(complete, "scripts/complete-gate.sh", "CI complete-gate job")
    require(assembled, "needs: complete-gate", "CI assembled-bundle job")
    require(
        assembled,
        'scripts/complete-gate.sh --bundle "$RUNNER_TEMP/hzr-dist"',
        "CI assembled-bundle job",
    )
    forbid(assembled, "run: scripts/build-bundle.sh", "CI assembled-bundle job")


def verify_release(workflow: str) -> None:
    preflight = workflow_job(workflow, "preflight")
    native = workflow_job(workflow, "native-bundles")
    publish = workflow_job(workflow, "publish")
    require(preflight, "scripts/complete-gate.sh", "release preflight job")
    forbid(preflight, "acceptance_gate_", "release preflight job")
    require(native, "needs: preflight", "release native-bundles job")
    require(
        native,
        'scripts/complete-gate.sh --bundle "$RUNNER_TEMP/hzr-dist"',
        "release native-bundles job",
    )
    forbid(native, "run: scripts/build-bundle.sh", "release native-bundles job")
    require(publish, "needs: [preflight, native-bundles]", "release publish job")
    if workflow.count("platform: linux-x64") != 1:
        raise GateError("release matrix must contain linux-x64 exactly once")
    if workflow.count("platform: linux-arm64") != 1:
        raise GateError("release matrix must contain linux-arm64 exactly once")
    if workflow.count("platform: darwin-arm64") != 1:
        raise GateError("release matrix must contain darwin-arm64 exactly once")
    forbid(workflow, "darwin-x64", "release workflow")


def verify_repository(repository: Path) -> None:
    script_path = repository / "scripts/complete-gate.sh"
    if not script_path.stat().st_mode & stat.S_IXUSR:
        raise GateError("scripts/complete-gate.sh must be executable")
    verify_script(
        script_path.read_text(),
        (repository / "scripts/build-bundle.sh").read_text(),
        (repository / "scripts/verify-fork-core.sh").read_text(),
    )
    verify_ci((repository / ".github/workflows/ci.yml").read_text())
    verify_release((repository / ".github/workflows/release.yml").read_text())


class ReleaseGateRegressionTests(unittest.TestCase):
    def test_ordinary_unit_failure_stops_before_fork_gate(self) -> None:
        self.assert_source_failure_stops("test")

    def test_clippy_failure_stops_before_fork_gate(self) -> None:
        self.assert_source_failure_stops("clippy")

    def assert_source_failure_stops(self, failing_subcommand: str) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            scripts = repository / "scripts"
            binaries = repository / "bin"
            scripts.mkdir()
            binaries.mkdir()
            shutil.copy(REPOSITORY / "scripts/complete-gate.sh", scripts)
            (scripts / "verify-release-gates.py").write_text("#!/usr/bin/env python3\n")
            marker = repository / "fork-ran"
            (scripts / "verify-fork-core.sh").write_text(
                f"#!/bin/sh\ntouch {marker}\n"
            )
            (scripts / "build-bundle.sh").write_text("#!/bin/sh\nexit 0\n")
            for path in scripts.iterdir():
                path.chmod(0o755)
            log = repository / "cargo.log"
            (binaries / "cargo").write_text(
                "#!/bin/sh\n"
                'printf "%s\\n" "$*" >>"$HZR_TEST_CARGO_LOG"\n'
                'if [ "$1" = "$HZR_TEST_FAIL" ]; then exit 19; fi\n'
            )
            (binaries / "python3").write_text("#!/bin/sh\nexit 0\n")
            for path in binaries.iterdir():
                path.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "HZR_TEST_CARGO_LOG": str(log),
                    "HZR_TEST_FAIL": failing_subcommand,
                    "PATH": f"{binaries}:{environment['PATH']}",
                }
            )
            result = subprocess.run(
                ["/bin/bash", str(scripts / "complete-gate.sh")],
                cwd=repository,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 19, result.stderr)
            self.assertFalse(marker.exists())
            calls = log.read_text()
            self.assertIn(failing_subcommand, calls)

    def test_static_gate_rejects_missing_native_preflight_dependency(self) -> None:
        release = (REPOSITORY / ".github/workflows/release.yml").read_text()
        mutated = release.replace("    needs: preflight\n", "", 1)
        with self.assertRaises(GateError):
            verify_release(mutated)

def main() -> int:
    try:
        verify_repository(REPOSITORY)
    except (GateError, OSError) as error:
        print(f"release gate verification failed: {error}", file=sys.stderr)
        return 1
    if "--self-test" in sys.argv[1:]:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(ReleaseGateRegressionTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        if not result.wasSuccessful():
            return 1
    print("release workflows are transitively gated by the complete source and bundle gates")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
