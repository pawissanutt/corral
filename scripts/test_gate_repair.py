#!/usr/bin/env python3
"""gate_repair.py turns a fresh nextest JUnit report into narrow rerun commands."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from typing import Final, override
import unittest


SCRIPT: Final = Path(__file__).resolve().parent / "gate_repair.py"
START: Final = "Repair commands (diagnostic only, not a gate result):"
END: Final = "End of repair commands."


def case(name: str, failed: bool) -> str:
    body = '<failure message="boom" type="test failure"/>' if failed else ""
    return f'<testcase name="{name}" classname="x">{body}</testcase>'


class GateRepairTests(unittest.TestCase):
    root: Path = Path()

    @override
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory(prefix="corral-gate-repair-")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.metadata = self.root / "binaries.json"
        _ = self.metadata.write_text(json.dumps({
            "rust-build-meta": {"target-directory": str(self.root / "target")},
            "rust-binaries": {
                "corral-ops": {"binary-id": "corral-ops", "binary-name": "corral-ops",
                               "kind": "lib"},
                "corral-ops::cleanup_ops": {"binary-id": "corral-ops::cleanup_ops",
                                            "binary-name": "cleanup_ops", "kind": "test"},
            },
        }))
        self.junit = self.root / "target/nextest/quick/junit.xml"
        self.junit.parent.mkdir(parents=True)

    def write_junit(self, suites: dict[str, str], age: float = 10) -> None:
        body = "".join(f'<testsuite name="{name}">{cases}</testsuite>'
                       for name, cases in suites.items())
        _ = self.junit.write_text(f'<?xml version="1.0"?><testsuites>{body}</testsuites>')
        stamp = self.metadata.stat().st_mtime + age
        os.utime(self.junit, (stamp, stamp))

    def report(self, metadata: Path | None = None) -> subprocess.CompletedProcess[str]:
        env = {k: v for k, v in os.environ.items() if k != "CORRAL_GATE_BINARIES_METADATA"}
        if metadata is not None:
            env["CORRAL_GATE_BINARIES_METADATA"] = str(metadata)
        return subprocess.run([sys.executable, str(SCRIPT), "report", "quick"], env=env,
                              capture_output=True, text=True, timeout=30, check=False)

    def test_fresh_report_names_one_narrow_command_per_failing_binary(self) -> None:
        self.write_junit({
            "corral-ops": case("inner::ok", False) + case("inner::weird,name)", True),
            "corral-ops::cleanup_ops": case("worktree::verdict_reaps", True),
        })
        result = self.report(self.metadata)
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = result.stderr.splitlines()
        self.assertEqual((lines[0], lines[-1]), (START, END))
        self.assertIn("cargo nextest run -p corral-ops --lib --no-fail-fast -E "
                      r"'binary_id(=corral-ops) and (test(=inner::weird\,name\)))'", lines[1])
        self.assertIn("cargo nextest run -p corral-ops --test cleanup_ops --no-fail-fast -E "
                      "'binary_id(=corral-ops::cleanup_ops) and "
                      "(test(=worktree::verdict_reaps))'", lines[2])
        self.assertTrue(lines[1].startswith(
            "(source scripts/test-herdr.sh && corral_test_herdr_start && "))
        self.assertNotIn("inner::ok", result.stderr)

    def test_stale_missing_or_passing_reports_print_nothing(self) -> None:
        cases = [
            ({"corral-ops": case("a", True)}, -10, self.metadata),
            ({"corral-ops": case("a", True)}, 10, None),
            ({"corral-ops": case("a", False)}, 10, self.metadata),
            ({"corral-ops": case("a", True)}, 10, self.root / "absent.json"),
        ]
        for suites, age, metadata in cases:
            with self.subTest(age=age, metadata=metadata, suites=suites):
                self.write_junit(suites, age)
                result = self.report(metadata)
                self.assertEqual((result.returncode, result.stderr), (0, ""))

    def test_unreadable_report_prints_one_note_and_never_fails(self) -> None:
        _ = self.junit.write_text("<testsuites><testsuite")
        stamp = self.metadata.stat().st_mtime + 10
        os.utime(self.junit, (stamp, stamp))
        result = self.report(self.metadata)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(result.stderr.splitlines()), 1)
        self.assertIn("Repair commands unavailable:", result.stderr)

    def test_many_failures_are_capped_and_counted(self) -> None:
        self.write_junit({"corral-ops": "".join(case(f"t{i}", True) for i in range(25))})
        result = self.report(self.metadata)
        self.assertIn("test(=t19)", result.stderr)
        self.assertNotIn("test(=t20)", result.stderr)
        self.assertIn("5 more failing tests not shown", result.stderr)


if __name__ == "__main__":
    _ = unittest.main()
