#!/usr/bin/env python3
"""Narrow rerun commands for a failed gate test run.

`report <profile>` reads the nextest JUnit report the failed run just wrote (its path is set in
.config/nextest.toml) and prints, between fixed markers, one command per failing test binary.
Each command builds only that binary's target and runs only its failed tests, under the same
private test herdr as the gate. The commands are diagnostics for the worker's repair loop, never
a gate result: a report older than this gate's test binaries, or none at all, prints nothing,
and any error prints one note. Autopilot carries the block into the send-back.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import sys
from typing import Final
import xml.etree.ElementTree as ET

from gate_select import escape

START: Final = "Repair commands (diagnostic only, not a gate result):"
END: Final = "End of repair commands."
MAX_TESTS: Final = 20
# nextest binary kinds and the Cargo target flag that builds only that binary.
TARGET_FLAGS: Final = {"lib": "--lib", "proc-macro": "--lib", "test": "--test",
                       "bin": "--bin", "example": "--example", "bench": "--bench"}


def failures(junit: Path) -> dict[str, list[str]]:
    """Failed or errored test names per binary ID, in report order."""
    failed: dict[str, list[str]] = {}
    for suite in ET.parse(junit).getroot().iter("testsuite"):
        for case in suite.iter("testcase"):
            if case.find("failure") is not None or case.find("error") is not None:
                failed.setdefault(suite.get("name", ""), []).append(case.get("name", ""))
    return failed


def command(binary: dict[str, str], tests: list[str]) -> str:
    binary_id = binary["binary-id"]
    args = ["cargo", "nextest", "run", "-p", binary_id.split("::", 1)[0]]
    flag = TARGET_FLAGS.get(binary.get("kind", ""))
    if flag == "--lib":
        args.append(flag)
    elif flag and binary.get("binary-name"):
        args += [flag, binary["binary-name"]]
    selection = " or ".join(f"test(={escape(test)})" for test in tests)
    args += ["--no-fail-fast", "-E", f"binary_id(={escape(binary_id)}) and ({selection})"]
    run = " ".join(shlex.quote(arg) for arg in args)
    return f"(source scripts/test-herdr.sh && corral_test_herdr_start && {run})"


def report(profile: str) -> None:
    metadata = Path(os.environ.get("CORRAL_GATE_BINARIES_METADATA", "/absent"))
    if not metadata.is_file():
        return
    meta = json.loads(metadata.read_text())
    target = Path(meta["rust-build-meta"]["target-directory"])
    junit = target / "nextest" / profile / "junit.xml"
    # Metadata is written before the run, the report after it: an older report is stale.
    if not junit.is_file() or junit.stat().st_mtime < metadata.stat().st_mtime:
        return
    failed = failures(junit)
    if not failed:
        return
    binaries = {binary["binary-id"]: binary for binary in meta["rust-binaries"].values()}
    lines = [START]
    shown = hidden = 0
    for binary_id, tests in failed.items():
        room = max(0, MAX_TESTS - shown)
        hidden += len(tests[room:])
        tests = tests[:room]
        if tests:
            shown += len(tests)
            lines.append(command(binaries.get(binary_id, {"binary-id": binary_id}), tests))
    if hidden:
        lines.append(f"{hidden} more failing tests not shown; see {junit}")
    lines.append("Fail-fast may have stopped the run before later tests; the next gate checks them.")
    lines.append(END)
    print("\n".join(lines), file=sys.stderr, flush=True)


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[1] != "report":
        print("usage: gate_repair.py report <nextest profile>", file=sys.stderr)
        return 2
    try:
        report(sys.argv[2])
    except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
        print(f"Repair commands unavailable: {error}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
