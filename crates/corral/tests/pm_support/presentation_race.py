# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
# How to run: python3 presentation_race.py /absolute/path/to/corral
"""Fence an actual Linux x86-64 presenter before its board lock."""

import ctypes
import json
import os
import signal
import subprocess
import sys
import tempfile
from collections.abc import Mapping
from pathlib import Path
from typing import Final


BIN: Final = sys.argv[1]
LIBC: Final = ctypes.CDLL(None, use_errno=True)
LIBC.ptrace.restype = ctypes.c_long
LIBC.ptrace.argtypes = [
    ctypes.c_uint, ctypes.c_uint, ctypes.c_void_p, ctypes.c_void_p,
]


def trace(request: int, pid: int, data: int = 0) -> None:
    """Issue one checked ptrace operation on our own disposable child."""
    if LIBC.ptrace(request, pid, None, data) == -1:
        raise OSError(ctypes.get_errno(), "ptrace")


def cli(root: Path, environment: Mapping[str, str], *args: str) -> str:
    """Run the actual candidate against the private store."""
    result = subprocess.run(
        [BIN, "--root", str(root), *args],
        env=environment, stdin=subprocess.DEVNULL, capture_output=True,
        text=True, check=True, timeout=30,
    )
    return result.stdout


def main() -> None:
    """Resolve at the syscall fence and assert durable answer preservation."""
    signal.alarm(90)
    with tempfile.TemporaryDirectory(prefix="corral-presentation-race-") as directory:
        root = Path(directory)
        (root / "app/.git").mkdir(parents=True)
        environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("CORRAL_", "HERDR_", "XDG_"))
        }
        for key in [
            "HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
            "XDG_STATE_HOME", "XDG_CACHE_HOME",
        ]:
            environment[key] = str(root / key)
        environment["CORRAL_ROOT"] = directory
        environment["CORRAL_HERDR_SESSION"] = "never-contacted"
        cli(root, environment, "init")
        cli(root, environment, "new", "Held", "-p", "app", "--ready", "--origin", "pm")
        cli(root, environment, "queue", "remove", "T-001")
        state = json.loads(cli(root, environment, "pm", "next", "--json"))
        decision = state["workset"]["decisions"][0]["id"]
        args = [
            BIN, "--root", directory, "pm", "present", decision,
            "--delivery", "concurrent-presentation", "--confirmed",
        ]
        child = os.fork()
        if child == 0:
            trace(0, 0)  # PTRACE_TRACEME.
            os.kill(os.getpid(), signal.SIGSTOP)
            os.execve(BIN, args, environment)
        reaped = False
        try:
            _, status = os.waitpid(child, 0)
            assert os.WIFSTOPPED(status)
            # TRACESYSGOOD identifies syscall stops; EXITKILL prevents orphaning.
            trace(0x4200, child, 1 | 0x00100000)
            fenced = False
            read_before_lock = False
            trace(24, child)  # PTRACE_SYSCALL.
            while True:
                _, status = os.waitpid(child, 0)
                if os.WIFEXITED(status) or os.WIFSIGNALED(status):
                    reaped = True
                    assert os.WIFEXITED(status), status
                    # A stale presentation may be refused or leave resolution intact.
                    assert os.WEXITSTATUS(status) in (0, 1), status
                    break
                assert os.WIFSTOPPED(status), status
                if os.WSTOPSIG(status) == (signal.SIGTRAP | 0x80):
                    registers = (ctypes.c_ulonglong * 27)()
                    trace(12, child, ctypes.addressof(registers))  # GETREGS.
                    # x86-64 syscall entry: orig_rax=openat, rax=-ENOSYS.
                    if registers[15] == 257 and registers[10] == (2**64 - 38):
                        with open(f"/proc/{child}/mem", "rb", buffering=0) as memory:
                            memory.seek(registers[13])  # rsi: pathname.
                            path = memory.read(4096).split(b"\0", 1)[0].decode()
                        if path.endswith("/state/pm-decisions.json"):
                            read_before_lock = True
                        if not fenced and path.endswith("/lock/dispatch.lock"):
                            print(
                                f"FENCE: board unlocked; pending_read={read_before_lock}",
                                flush=True,
                            )
                            cli(
                                root, environment, "pm", "resolve", decision,
                                "--answer", "Keep this task held",
                            )
                            records = json.loads(
                                (root / ".corral/state/pm-decisions.json").read_text()
                            )
                            assert records["records"][decision]["answer"] == "Keep this task held"
                            fenced = True
                trace(24, child)
            assert fenced, "presenter never reached the subscribed board-lock fence"
        finally:
            if not reaped:
                os.kill(child, signal.SIGKILL)
                os.waitpid(child, 0)
        records = json.loads((root / ".corral/state/pm-decisions.json").read_text())
        answer = records["records"][decision]["answer"]
        reopened = json.loads(cli(root, environment, "pm", "next", "--json"))
        pending = any(item["id"] == decision for item in reopened["workset"]["decisions"])
        print(f"AFTER PRESENT: answer={answer!r}, decision_pending={pending}", flush=True)
        assert answer == "Keep this task held", "presentation erased the owner answer"
        assert not pending, "presentation resurrected an answered decision"
        task = json.loads(cli(root, environment, "show", "T-001", "--json"))
        assert task["dispatch_hold"] and not task.get("runs")
    signal.alarm(0)


if __name__ == "__main__":
    main()
