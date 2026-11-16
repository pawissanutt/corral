"""Inert herdr fixture harness: read a generated prompt and await an explicit callback barrier.

This is not model-quality evidence. It executes the candidate's exact generated verdict argv
from the real launched reviewer session, with its saved pane/socket environment intact.
"""
import json
import os
from pathlib import Path
import re
import select
import shlex
import sys
import time

root, prompt = map(Path, sys.argv[1:3])
text = prompt.read_text()
control = root / ".corral" / "qa-control"
control.mkdir(exist_ok=True)
match = re.search(r"^# Plan review: (T-\d+) .*?\n.*?Target: plan (P\d+), attempt (PR\d+)\.", text, re.S)
if not match:
    # A real saved planning launch is used only to establish the author identity.
    # No planner result is fabricated; QA submits the inert source through `plan submit`.
    print("Inert fixture planner: no provider, API, or implementation command.", flush=True)
    select.select([], [], [], 120)
    sys.exit(0)
task, plan, attempt = match.groups()
key = f"{task}-{attempt}"
line = next(line.strip() for line in text.splitlines() if " plan verdict " in line and "approve|changes|escalate" in line)
argv = shlex.split(line)
argv[argv.index("approve|changes|escalate")] = "approve"
assert Path(argv[0]).resolve() == Path(os.environ["CORRAL"]).resolve(), argv
assert argv[argv.index("--root") + 1] == str(root), argv
assert argv[argv.index("--attempt") + 1] == attempt, argv
notes = Path(argv[argv.index("--notes-file") + 1])
notes.write_text("Genuine inert fixture callback: reviewed the exact generated plan snapshot; approve.\n")
fifo = control / f"{key}.fifo"
os.mkfifo(fifo, 0o600)
fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
record = {"argv": argv, "task": task, "plan": plan, "attempt": attempt, "prompt": str(prompt),
          "notes": str(notes), "pid": os.getpid(), "inert": True,
          "environment": {key: os.environ.get(key) for key in ["HERDR_PANE_ID", "HERDR_SOCKET_PATH", "CORRAL_HERDR_SESSION", "CORRAL_AGENT_ROLE"]}}
(control / f"{key}.ready.json").write_text(json.dumps(record, indent=2))
print(f"Inert reviewer {key} ready at exact callback barrier", flush=True)
if not select.select([fd], [], [], 60)[0] or os.read(fd, 128) != b"approve\n":
    raise RuntimeError("fixture callback barrier missing or invalid")
os.close(fd)
(control / f"{key}.executing.json").write_text(json.dumps(record, indent=2))
# No shell replay or locally manufactured verdict: this is the prompt's exact callback.
os.execv(argv[0], [*argv, "--json"])
