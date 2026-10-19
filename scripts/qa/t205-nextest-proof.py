#!/usr/bin/env python3
"""Capture five full isolated suites on one committed candidate and main base.

Run from any directory: python3 scripts/qa/t205-nextest-proof.py <new-evidence-dir>.
The output directory must be ignored by Git so recording proof cannot dirty the
candidate. Stop at the first test failure or identity change; never retry tests.
"""

from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from typing import Final


ROOT: Final = Path(__file__).resolve().parents[2]
COMMAND: Final = (
    "bash", "-c",
    "source scripts/test-herdr.sh && corral_test_herdr_start && "
    "CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace",
)


def git(*args: str) -> str:
    return subprocess.check_output(
        ["git", *args], cwd=ROOT, stdin=subprocess.DEVNULL, text=True,
    ).strip()


@dataclass(frozen=True, slots=True)
class Candidate:
    head: str
    base: str
    tree: str
    status: str


def candidate() -> Candidate:
    return Candidate(
        git("rev-parse", "HEAD"), git("rev-parse", "main"),
        git("rev-parse", "HEAD^{tree}"), git("status", "--porcelain"),
    )


def main() -> int:
    os.chdir(ROOT)
    directory = Path(sys.argv[1]).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    expected = candidate()
    if expected.status:
        print("INVALID: commit the candidate before capturing proof", flush=True)
        return 2
    for run in range(1, 6):
        before = candidate()
        if before != expected:
            print("INVALID: candidate or main base changed before suite", flush=True)
            return 2
        header = {
            "run": run, "candidate": asdict(before), "command": COMMAND,
            "started_at": datetime.now(timezone.utc).isoformat(),
            "stdin": "/dev/null",
        }
        log_path = directory / f"suite-{run}.log"
        print("START " + json.dumps(header), flush=True)
        with log_path.open("w") as log:
            log.write(json.dumps(header) + "\n")
            log.flush()
            started = time.monotonic()
            result = subprocess.run(
                COMMAND, stdin=subprocess.DEVNULL, stdout=log,
                stderr=subprocess.STDOUT, check=False,
            )
            wall_seconds = round(time.monotonic() - started, 3)
        after = candidate()
        summaries = [
            line.strip() for line in log_path.read_text().splitlines()
            if line.lstrip().startswith("Summary [")
        ]
        summary = summaries[0] if len(summaries) == 1 else ""
        counts = re.search(r"(\d+) tests run: (\d+) passed", summary)
        valid = (
            result.returncode == 0 and after == expected and counts is not None
            and int(counts[1]) > 0 and counts[1] == counts[2]
        )
        proof = {
            **header, "exit_code": result.returncode, "wall_seconds": wall_seconds,
            "candidate_after": asdict(after), "summary": summary, "valid": valid,
        }
        (directory / f"suite-{run}.json").write_text(json.dumps(proof, indent=2) + "\n")
        print("RESULT " + json.dumps(proof), flush=True)
        if not valid:
            return result.returncode or 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
