#!/usr/bin/env python3
"""Report candidate-added Rust tests and their measured nextest seconds, not test volume.

Renames/parameter-case splits are deliberately counted as additions: the report must
justify their cost too. No baseline build or duplicate suite is needed. The receipt log
is unique per attempt; missing added-test timings are reported as unmeasured, never zero.
"""
from __future__ import annotations
import ast
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import time

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


def other_tests(base: str) -> set[tuple[str, str, str]]:
    added = set()
    for path in git("diff", "--name-only", f"{base}...HEAD", "--", "*.py", "*.ts", "*.tsx").splitlines():
        def names(text):
            if path.endswith(".py"):
                return {node.name for node in ast.walk(ast.parse(text))
                        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
                        and node.name.startswith("test_")}
            return set(re.findall(r"\b(?:test|it)(?:\.(?:skip|only|todo))?\s*\(\s*[\"\'`](.*?)[\"\'`]", text))
        current = Path(path).read_text() if Path(path).is_file() else ""
        try:
            old = git("show", f"{base}:{path}")
        except subprocess.CalledProcessError:
            old = ""
        language = "Python" if path.endswith(".py") else "Bun"
        added.update((language, path, name) for name in names(current) - names(old))
    return added


def python_tests(paths: list[str]) -> int:
    for path in paths:
        started = time.monotonic()
        code = subprocess.run([sys.executable, path], stdin=subprocess.DEVNULL, check=False).returncode
        print(f"Python suite {path} {time.monotonic() - started:.3f}s", flush=True)
        if code:
            return code
    return 0


def report(profile: str) -> int:
    try:
        base = git("merge-base", os.environ.get("CORRAL_INTEGRATION_BASE", "HEAD^"), "HEAD").strip()
        added = added_tests(base)
        other = other_tests(base)
    except subprocess.CalledProcessError:
        print("Test cost: baseline unavailable (legacy/manual context)")
        return 0
    context = os.environ.get("CORRAL_GATE_CONTEXT", "")
    path = Path(context.removesuffix(".context.json") + ".log") if context else None
    durations = {}
    text = re.sub(r"\x1b\[[0-9;]*m", "", path.read_text()) if path is not None and path.is_file() else ""
    if text:
        for seconds, name in re.findall(r"PASS\s+\[\s*([0-9.]+)s\]\s+.*? (\S+)\s*$",
                                        text, re.M):
            name = name.split("::")[-1]
            if name in added:
                durations[name] = durations.get(name, 0.0) + float(seconds)
    seconds = sum(durations.values())
    unmeasured = {"Rust " + name for name in added - durations.keys()}
    bun = {name: float(ms) / 1000 for name, ms in re.findall(r"^\(pass\) (.*?) \[([0-9.]+)ms\]$", text, re.M)}
    python = {file: float(s) for file, s in re.findall(r"^Python suite (\S+) ([0-9.]+)s$", text, re.M)}
    measured_python = set()
    for language, file, name in other:
        if language == "Bun" and name in bun:
            seconds += bun[name]
        elif language == "Python" and file in python:
            measured_python.add(file)
        else:
            unmeasured.add(f"{language} {file}::{name}")
    # Count the whole containing Python script once: a conservative suite cost, not a
    # fabricated per-test timing when unittest does not expose one.
    seconds += sum(python[file] for file in measured_python)
    print(f"Test cost: +{len(added) + len(other)} test registrations "
          f"(Rust {len(added)}, Bun {sum(t[0] == 'Bun' for t in other)}, "
          f"Python {sum(t[0] == 'Python' for t in other)}); added suite {seconds:.3f}s measured "
          f"(Python containing-suite upper bound); {len(unmeasured)} unmeasured (unselected/slow/ignored)")
    if unmeasured:
        print("Unmeasured additions: " + ", ".join(sorted(unmeasured)))
    if profile == "count":
        return 0
    config = tomllib.loads(Path(".config/gate.toml").read_text())
    threshold = config["test_cost"]["justification_threshold_seconds"]
    justification = re.search(r"^\+Test-cost justification:\s*\S.+",
                              git("diff", f"{base}...HEAD", "--", "*.md"), re.M)
    if seconds > threshold and not justification:
        print(f"Test-cost observation: >{threshold}s added suite has no Test-cost justification: "
              "(report-only; enforcement T-776)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(python_tests(sys.argv[2:]) if sys.argv[1] == "python-tests" else report(sys.argv[1]))
