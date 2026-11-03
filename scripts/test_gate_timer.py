#!/usr/bin/env python3
"""Regression for report-only timing output failures."""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ReportTests(unittest.TestCase):
    def test_stdout_errors_do_not_fail_report_process(self):
        script = Path(__file__).with_name('gate_timer.py').resolve()
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / 'state.json'
            state.write_text(json.dumps({'tier': 'quick', 'measured': 1, 'budgeted': 1,
                                         'compile': 0, 'limit': 180}))
            with open('/dev/full', 'w') as stdout:
                result = subprocess.run([sys.executable, str(script), 'report', str(state)],
                                        stdout=stdout, stderr=subprocess.PIPE, text=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('Timing report unavailable:', result.stderr)

            # A full nonblocking pipe causes a real buffered stdout BlockingIOError.
            read_fd, write_fd = os.pipe2(os.O_NONBLOCK)
            try:
                os.write(write_fd, b'x' * fcntl.fcntl(write_fd, fcntl.F_GETPIPE_SZ))
                runner = ("import os, runpy, sys; os.set_blocking = lambda *args: None; "
                          "sys.argv = [sys.argv[1], 'report', sys.argv[2]]; "
                          "runpy.run_path(sys.argv[0], run_name='__main__')")
                result = subprocess.run([sys.executable, '-c', runner, str(script), str(state)],
                                        stdout=write_fd, stderr=subprocess.PIPE, text=True, check=False)
            finally:
                os.close(read_fd)
                os.close(write_fd)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('Timing report unavailable:', result.stderr)
            self.assertIn('[Errno 11]', result.stderr)


if __name__ == '__main__':
    unittest.main()
