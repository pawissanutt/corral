#!/usr/bin/env python3
"""Run the full gate with hostile inherited identities and an immutable sentinel store."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from typing import TypedDict


Snapshot = dict[str, tuple[int, int, str | None]]


class CommandResult(TypedDict):
    argv: list[str]
    exit_code: int


class Report(TypedDict):
    commands: list[CommandResult]
    sentinel_unchanged: bool
    external_store_writes: list[str]
    environment: dict[str, str]
    cwd: str
    before: Snapshot
    after: Snapshot
    ok: bool


FILE_OPERATIONS = (
    "open,openat,openat2,creat,rename,renameat,renameat2,unlink,unlinkat,"
    "mkdir,mkdirat,rmdir,link,linkat,symlink,symlinkat,chmod,chown,truncate,utimensat"
)


def snapshot(root: Path) -> Snapshot:
    return {
        str(path.relative_to(root)): (
            path.stat().st_mtime_ns, path.stat().st_size,
            hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None,
        )
        for path in sorted(root.rglob("*"))
    }


def external_store_writes(trace: Path) -> list[str]:
    """All test-owned stores must be under the system temp directory, never the checkout."""
    writes = []
    mutator = re.compile(
        r"(?:open(?:at|at2)?\(.*O_(?:WRONLY|RDWR|CREAT|TRUNC)|"
        r"(?:creat|rename(?:at|at2)?|unlink(?:at)?|mkdir(?:at)?|rmdir|"
        r"link(?:at)?|symlink(?:at)?|chmod|chown|truncate|utimensat)\()"
    )
    temp = Path(tempfile.gettempdir()).resolve()
    with trace.open() as stream:
        for line in stream:
            if not mutator.search(line):
                continue
            paths = re.finditer(r'"([^"\n]*\.corral(?:/[^"\n]*)?)"|<([^>\n]*\.corral(?:/[^>\n]*)?)>', line)
            for match in paths:
                raw = match[1] or match[2]
                path = Path(raw)
                if not path.is_absolute():
                    descriptors = re.findall(r"<(/[^>]+)>", line[:match.start()])
                    if descriptors:
                        path = Path(descriptors[-1]) / path
                if not path.is_absolute() or not path.resolve().is_relative_to(temp):
                    writes.append(line.rstrip())
                    break
    return writes


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--trace-dir", type=Path, help="Use strace to audit store writes outside temp dirs")
    args = parser.parse_args()
    if args.trace_dir and not shutil.which("strace"):
        parser.error("--trace-dir requires strace")
    if args.trace_dir:
        args.trace_dir.mkdir(parents=True, exist_ok=True)
    commands = [["bash", "scripts/gate.sh"]]
    report: Report = {
        "commands": [], "sentinel_unchanged": True, "external_store_writes": [],
        "environment": {}, "cwd": str(Path.cwd()), "before": {}, "after": {}, "ok": False,
    }
    with tempfile.TemporaryDirectory(prefix="corral-isolation-sentinel-") as directory:
        root = Path(directory)
        store = root / ".corral"
        gates = store / "state/gates"
        gates.mkdir(parents=True)
        (store / "corral.toml").write_text("[review]\nautopilot = false\n")
        (store / "events.jsonl").write_text(
            '{"ts":"2026-10-15T00:00:00Z","task":null,"kind":"sentinel","detail":{}}\n'
        )
        (gates / "sentinel.log").write_text("sentinel log: untouched\n")
        (gates / "sentinel.json").write_text('{"result":"running","attempt":"sentinel"}\n')
        before = snapshot(store)
        environment = {
            **os.environ,
            "CORRAL_ROOT": str(root),
            "CORRAL_GATE_ATTEMPT": "sentinel",
            "CORRAL_CANDIDATE_HEAD": "deadbeef",
            "CORRAL_INTEGRATION_BASE": "deadbeef",
            "CORRAL_HERDR_SESSION": "corral-test",
            "HERDR_PANE_ID": "sentinel:p1",
            "HERDR_WORKSPACE_ID": "sentinel",
            "HERDR_TAB_ID": "sentinel:t1",
        }
        report["environment"] = {name: environment[name] for name in (
            "CORRAL_ROOT", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD",
            "CORRAL_INTEGRATION_BASE", "CORRAL_HERDR_SESSION",
            "HERDR_PANE_ID", "HERDR_WORKSPACE_ID", "HERDR_TAB_ID",
        )}
        report["cwd"] = str(Path.cwd())
        report["before"] = before
        for index, command in enumerate(commands):
            invocation = command
            trace = args.trace_dir / f"gate-{index}.trace" if args.trace_dir else None
            if trace:
                invocation = ["strace", "-f", "-qq", "-y", "-s", "4096",
                              "-e", f"trace={FILE_OPERATIONS}",
                              "-o", str(trace), *command]
            print("$ " + " ".join(command), flush=True)
            result = subprocess.run(invocation, env=environment, stdin=subprocess.DEVNULL, check=False)
            report["commands"].append({"argv": command, "exit_code": result.returncode})
            if trace:
                report["external_store_writes"].extend(external_store_writes(trace))
            report["after"] = snapshot(store)
            report["sentinel_unchanged"] &= report["after"] == before
            if result.returncode or not report["sentinel_unchanged"] or report["external_store_writes"]:
                break
    report["ok"] = (
        len(report["commands"]) == len(commands)
        and all(command["exit_code"] == 0 for command in report["commands"])
        and report["sentinel_unchanged"]
        and not report["external_store_writes"]
    )
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2), flush=True)
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
