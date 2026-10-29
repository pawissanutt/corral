#!/usr/bin/env python3
"""Measure the nextest slow group from gate logs.

usage: python3 scripts/slow-tests.py [--last N] [--list LISTING] GATE_LOG...
e.g.   cargo nextest list --workspace --message-format json >/tmp/tests.json
       python3 scripts/slow-tests.py --list /tmp/tests.json ../../.corral/state/gates/*.log

Prints each test's 75th-percentile duration over the given logs (the newest N, default
200) and whether it should enter (p75 >= 40 s) or leave (p75 < 30 s) the slow group in
.config/nextest.toml. Group members never seen in the logs are reported too. With
--list, only tests in that nextest listing (the current tree) are proposed to enter.
"""

import argparse
import json
import os
import re
import tomllib
from pathlib import Path

ENTER, LEAVE = 40.0, 30.0
RESULT = re.compile(r"^\s+(?:PASS|FAIL|LEAK|TIMEOUT)\s+\[\s*([\d.]+)s\]\s+(?:\(\s*\d+/\d+\)\s+)?"
                    r"(\S+)\s+(.+?)\s*$")
MEMBER = re.compile(r"binary_id\(=([^)]+)\)\s*&\s*test\(=([^)]+)\)")


def main() -> None:
    parser = argparse.ArgumentParser()
    _ = parser.add_argument("--last", type=int, default=200)
    _ = parser.add_argument("--list", type=Path)
    _ = parser.add_argument("logs", nargs="+", type=Path)
    args = parser.parse_args()
    current: set[tuple[str, str]] | None = None
    if args.list:
        suites = json.loads(args.list.read_text())["rust-suites"]
        current = {(binary, name) for binary, suite in suites.items()
                   for name in suite["testcases"]}
    logs: list[Path] = sorted(args.logs, key=os.path.getmtime)[-args.last:]
    durations: dict[tuple[str, str], list[float]] = {}
    for log in logs:
        for line in log.read_text(errors="replace").splitlines():
            if match := RESULT.match(line):
                key = (match.group(2), match.group(3))
                durations.setdefault(key, []).append(float(match.group(1)))
    config = tomllib.loads((Path(__file__).parent.parent / ".config/nextest.toml").read_text())
    members = set(MEMBER.findall(config["profile"]["quick"]["default-filter"]))
    print(f"{len(logs)} logs, {len(members)} slow-group members")
    for key in sorted(set(durations) | members):
        values = sorted(durations.get(key, []))
        p75 = values[int(len(values) * 0.75)] if values else None
        if key in members and (p75 is None or p75 < LEAVE):
            verdict = "leave"
        elif (key not in members and p75 is not None and p75 >= ENTER
              and (current is None or key in current)):
            verdict = "enter"
        elif key in members:
            verdict = "keep"
        else:
            continue
        shown = "unseen" if p75 is None else f"p75 {p75:6.1f}s n={len(values)}"
        print(f"{verdict:5} {shown}  {key[0]} {key[1]}")


if __name__ == "__main__":
    main()
