"""Synchronize real move/dequeue processes at their blocking lock syscalls."""

import fcntl
import json
import os
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def lock_attempt(process: subprocess.Popen[bytes], marker: bytes) -> None:
    """Await an exact syscall entry with a bounded deadline, without polling."""
    assert process.stderr is not None
    seen = b""
    deadline = time.monotonic() + 10
    with selectors.DefaultSelector() as ready:
        ready.register(process.stderr, selectors.EVENT_READ)
        while marker not in seen:
            remaining = deadline - time.monotonic()
            assert remaining > 0 and ready.select(remaining), seen.decode()
            chunk = os.read(process.stderr.fileno(), 4096)
            assert chunk, seen.decode()
            seen += chunk


def main() -> None:
    """Keep the task lock until both CLI operations reach the required ordering."""
    tracer = shutil.which("strace")
    assert tracer is not None, "the Linux dequeue race regression requires strace"
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(("CORRAL_", "HERDR_"))
    }
    with tempfile.TemporaryDirectory(prefix="corral-dequeue-race-") as root:
        for name in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ]:
            environment[name] = str(Path(root) / name)
            Path(environment[name]).mkdir()
        command = [sys.argv[1], "--root", root]
        subprocess.run(
            [*command, "init"], env=environment, check=True, stdin=subprocess.DEVNULL
        )
        task = subprocess.check_output(
            [*command, "new", "Concurrent withdrawal", "--ready"],
            env=environment,
            stdin=subprocess.DEVNULL,
            text=True,
        ).strip()
        lock = Path(root) / ".corral" / "lock" / f"{task}.lock"
        with lock.open("a") as held:
            fcntl.flock(held.fileno(), fcntl.LOCK_EX)
            mover = subprocess.Popen(
                [tracer, "-yy", "-e", "trace=flock,fcntl", *command, "move", task, "dropped", "--owner-ok"],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
            remover = None
            try:
                # Move holds the board lock and is blocked on our task lock.
                lock_attempt(mover, f"/{task}.lock>, LOCK_EX".encode())
                remover = subprocess.Popen(
                    [tracer, "-yy", "-e", "trace=flock,fcntl", *command, "queue", "remove", task],
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    start_new_session=True,
                )
                # Dequeue has read Ready and now waits behind move's board lock.
                lock_attempt(remover, b"/dispatch.lock>, LOCK_EX")
                fcntl.flock(held.fileno(), fcntl.LOCK_UN)
                moved, move_trace = mover.communicate(timeout=10)
                removed, remove_trace = remover.communicate(timeout=10)
                assert mover.returncode == 0, (moved, move_trace)
                state = json.loads(
                    subprocess.check_output(
                        [*command, "show", task, "--json"],
                        env=environment,
                        stdin=subprocess.DEVNULL,
                    )
                )
                assert remover.returncode != 0, (removed, remove_trace, state)
                assert state["status"] == "dropped", state
                assert not state.get("queued") and not state.get("dispatch_hold"), state
                assert not state.get("runs"), state
                events = [
                    json.loads(line)
                    for line in (Path(root) / ".corral" / "events.jsonl").read_text().splitlines()
                ]
                assert not any(event["kind"] == "dequeued" for event in events), events
                assert sum(event["kind"] == "moved" for event in events) == 1, events
                print("Concurrent Dropped move preserved; stale dequeue rejected without an event.")
            finally:
                fcntl.flock(held.fileno(), fcntl.LOCK_UN)
                for process in [mover, remover]:
                    if process is not None and process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=10)


if __name__ == "__main__":
    main()
