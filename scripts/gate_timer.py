#!/usr/bin/env python3
"""Run one gate recipe within its configured wall budget; terminate its process group."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import tomllib


def run(command: list[str], seconds: float) -> int:
    started = time.monotonic()
    child = subprocess.Popen(command, stdin=subprocess.DEVNULL, start_new_session=True,
                             env={**os.environ, "CORRAL_GATE_BUDGET_CHILD": "1"})
    try:
        code = child.wait(timeout=seconds)
    except (subprocess.TimeoutExpired, KeyboardInterrupt):
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        # The shell may have exited while a descendant ignored TERM. Reap the whole group.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
        print(f"FAILED: gate wall budget {seconds:g}s exceeded", file=sys.stderr, flush=True)
        code = 124
    print(f"Gate wall time: {time.monotonic() - started:.3f}s (budget {seconds:g}s)", flush=True)
    return code


if __name__ == "__main__":
    config = tomllib.loads((Path(__file__).resolve().parent.parent / ".config/gate.toml").read_text())
    sys.exit(run(["bash", *sys.argv[2:]], config["seconds"][sys.argv[1]]))
