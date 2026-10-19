"""Expose the real cancel command's first inbox-related lock boundary."""

import json
import os
import selectors
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path


def main() -> None:
    """Signal either blocked delivery-lock entry or completed unsynchronized cancel."""
    tracer = shutil.which("strace")
    assert tracer is not None, "the Linux inbox cancellation regression requires strace"
    root = Path(sys.argv[2])
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(("CORRAL_", "HERDR_"))
    }
    for name in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ]:
        private = root / "cancel-environment" / name
        private.mkdir(parents=True)
        environment[name] = str(private)
    environment["HERDR_BIN_PATH"] = str(root / "no-herdr")
    process = subprocess.Popen(
        [
            tracer, "-yy", "-e", "trace=flock,fcntl", sys.argv[1],
            "--root", str(root), "inbox", "mark", sys.argv[3],
            "--cancel", "--reason", "withdrawn during recovery",
        ],
        cwd=root,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        assert process.stderr is not None
        seen = b""
        delivery = b"/deliver-pm.lock>, LOCK_EX"
        inbox = b"/inbox-pm.lock>, LOCK_EX"
        deadline = time.monotonic() + 10
        with selectors.DefaultSelector() as ready:
            ready.register(process.stderr, selectors.EVENT_READ)
            while delivery not in seen and inbox not in seen:
                remaining = deadline - time.monotonic()
                assert remaining > 0 and ready.select(remaining), seen.decode()
                chunk = os.read(process.stderr.fileno(), 4096)
                assert chunk, seen.decode()
                seen += chunk
        cancelled = delivery not in seen
        if cancelled:
            output, trace = process.communicate(timeout=10)
            assert process.returncode == 0, (output, seen, trace)
        print(json.dumps({"cancelled_before_send": cancelled}), flush=True)
        if not cancelled:
            output, trace = process.communicate(timeout=10)
            assert process.returncode == 0, (output, seen, trace)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=10)


if __name__ == "__main__":
    main()
