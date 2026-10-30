"""Exercise the real strace barrier with varying unrelated close counts."""

import fcntl
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest

from filing_race import publication_trace, stopped_trace


class PublicationTraceTest(unittest.TestCase):
    def test_only_first_dispatch_close_counts(self) -> None:
        tracer = shutil.which("strace")
        self.assertIsNotNone(tracer, "this regression requires strace")
        with tempfile.TemporaryDirectory(prefix="corral-filing-trace-") as directory:
            lock = Path(directory) / "lock/dispatch.lock"
            lock.parent.mkdir()
            lock.touch()
            for unrelated in [0, 1, 32]:
                with self.subTest(unrelated=unrelated):
                    # Both startup and locked-section closes must be invisible
                    # to the counter. A second publication must not be reached.
                    child = """
import fcntl, os, sys
lock, count, marker = sys.argv[1:]
for _ in range(int(count)):
    os.close(os.open('/dev/null', os.O_RDONLY))
fd = os.open(lock, os.O_RDWR)
fcntl.flock(fd, fcntl.LOCK_EX)
for _ in range(int(count)):
    os.close(os.open('/dev/null', os.O_RDONLY))
fcntl.flock(fd, fcntl.LOCK_UN)
os.close(fd)
with open(marker, 'w') as stream:
    stream.write('second publication')
"""
                    marker = Path(directory) / f"later-{unrelated}"
                    with subprocess.Popen(
                        [*publication_trace(tracer, lock), sys.executable, "-c", child,
                         str(lock), str(unrelated), str(marker)],
                        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE, start_new_session=True,
                    ) as process:
                        try:
                            trace = stopped_trace(process)
                            closes = [line for line in trace.splitlines()
                                      if line.startswith(b"close(")]
                            self.assertEqual(len(closes), 1, trace.decode())
                            self.assertIn(os.fsencode(lock) + b">", closes[0])
                            self.assertIn(b"LOCK_UN) = 0", trace)
                            self.assertFalse(marker.exists(), trace.decode())
                            # Sync must be able to acquire the board lock now.
                            with lock.open() as stream:
                                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                            os.killpg(process.pid, signal.SIGCONT)
                            _, stderr = process.communicate(timeout=10)
                            self.assertEqual(process.returncode, 0, stderr.decode())
                            self.assertTrue(marker.exists())
                        finally:
                            if process.poll() is None:
                                os.killpg(process.pid, signal.SIGKILL)
                                process.communicate(timeout=10)


if __name__ == "__main__":
    unittest.main()
