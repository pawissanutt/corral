#!/usr/bin/env python3
"""Report candidate-added Rust tests and their measured nextest seconds, not test volume.

Renames/parameter-case splits are deliberately counted as additions: the report must
justify their cost too. No baseline build or duplicate suite is needed. The receipt log
is unique per attempt; missing added-test timings are reported as unmeasured, never zero.
"""
from __future__ import annotations
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

TEST = re.compile(r"#\[(?:[\w:]+::)?test(?:\([^]]*\))?\](?:\s*#\[[^]]*\])*\s*(?:async\s+)?fn\s+(\w+)")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], stdin=subprocess.DEVNULL, check=True,
                          capture_output=True, text=True).stdout


def added_tests(base: str) -> set[str]:
    added = set()
    for path in git("diff", "--name-only", f"{base}...HEAD", "--", "*.rs").splitlines():
        current = Path(path).read_text() if Path(path).is_file() else ""
        try:
            old = git("show", f"{base}:{path}")
        except subprocess.CalledProcessError:
            old = ""
        added.update(set(TEST.findall(current)) - set(TEST.findall(old)))
    return added


def report(profile: str) -> int:
    try:
        base = git("merge-base", os.environ.get("CORRAL_INTEGRATION_BASE", "HEAD^"), "HEAD").strip()
        added = added_tests(base)
    except subprocess.CalledProcessError:
        print("Test cost: baseline unavailable (legacy/manual context)")
        return 0
    context = os.environ.get("CORRAL_GATE_CONTEXT", "")
    path = Path(context.removesuffix(".context.json") + ".log") if context else None
    durations = {}
    if path is not None and path.is_file():
        for seconds, name in re.findall(r"PASS\s+\[\s*([0-9.]+)s\]\s+.*? (\S+)\s*$",
                                        path.read_text(), re.M):
            name = name.split("::")[-1]
            if name in added:
                durations[name] = durations.get(name, 0.0) + float(seconds)
    seconds = sum(durations.values())
    unmeasured = added - durations.keys()
    print(f"Test cost: +{len(added)} Rust tests; added suite {seconds:.3f}s measured; "
          f"{len(unmeasured)} unmeasured (unselected/slow/ignored)")
    if unmeasured:
        print("Unmeasured additions: " + ", ".join(sorted(unmeasured)))
    if profile == "count":
        return 0
    config = tomllib.loads(Path(".config/gate.toml").read_text())
    threshold = config["test_cost"]["justification_threshold_seconds"]
    justification = re.search(r"^\+Test-cost justification:\s*\S.+",
                              git("diff", f"{base}...HEAD", "--", "*.md"), re.M)
    if seconds > threshold and not justification:
        print(f"FAILED: >{threshold}s added suite needs Test-cost justification:", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(report(sys.argv[1]))
