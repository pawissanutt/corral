"""Private-fixture controlling terminal driver; never connects to herdr."""
import errno
import os
import pty
import select
import signal
import sys
import time

response, task, digest, binary, *args = sys.argv[1:]
pid, terminal = pty.fork()
if pid == 0:
    os.execv(binary, [binary, *args])

capture = bytearray()
sent = False
reaped = False
deadline = time.monotonic() + 10
try:
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("fixture recovery terminal did not finish")
        ready, _, _ = select.select([terminal], [], [], remaining)
        if not ready:
            raise TimeoutError("fixture recovery terminal did not respond")
        try:
            chunk = os.read(terminal, 65536)
        except OSError as error:
            if error.errno != errno.EIO:
                raise
            break
        if not chunk:
            break
        capture.extend(chunk)
        if not sent and b"anything else cancels):" in capture:
            challenge = f"confirm {task} {digest}" if response == "confirm" else "cancel"
            os.write(terminal, (challenge + "\n").encode())
            sent = True
    _, status = os.waitpid(pid, 0)
    reaped = True
    sys.stdout.buffer.write(capture)
    sys.exit(os.waitstatus_to_exitcode(status))
finally:
    os.close(terminal)
    # The driver owns only this disposable child, including on a bounded deadline.
    if not reaped:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
