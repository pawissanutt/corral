#!/usr/bin/env python3
"""No host load, real test suite, live board mutation, or owner herdr in these fixtures."""
import argparse
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import slow_lane


class FakeBoard(slow_lane.Board):
    def __init__(self):
        self.project = 'corral'
        self.tasks = []
        self.calls = []

    def call(self, *args):
        import json
        self.calls.append(args)
        if args[0] == 'list':
            return json.dumps(self.tasks)
        if args[0] == 'new':
            task = f'T-{len(self.tasks) + 1:03}'
            self.tasks.append({'id': task, 'title': args[1]})
            return task
        if args[:2] == ('artifact', 'add'):
            return 'R0 log transient artifacts/T-001/R0/hash-proof.log\n'
        return ''


class SlowLaneTests(unittest.TestCase):
    def test_busy_machine_refuses_before_starting_or_writing_anything(self):
        self.assertTrue(slow_lane.load_admitted(32, 32))
        for load, cores in [(32.001, 32), (1, 0), (float('nan'), 32), (-1, 32)]:
            self.assertFalse(slow_lane.load_admitted(load, cores))
        with tempfile.TemporaryDirectory() as directory:
            # REPO is disposable; even admission must not consult a live board/server.
            with patch.object(slow_lane, 'REPO', Path(directory)), \
                 patch.object(slow_lane.os, 'chdir'), \
                 patch.object(slow_lane, 'idle_priority') as priority, \
                 patch.object(slow_lane, 'available_cores', return_value=32), \
                 patch.object(slow_lane.os, 'getloadavg', return_value=(33, 33, 33)), \
                 patch.object(slow_lane, 'command') as command, \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(slow_lane.run(argparse.Namespace()), 75)
                priority.assert_called_once()
                command.assert_not_called()
                self.assertEqual(list(Path(directory).iterdir()), [])

    def test_idle_priority_applies_only_to_our_process(self):
        with patch.object(slow_lane.os, 'setpriority') as nice, \
             patch.object(slow_lane, 'command') as command:
            slow_lane.idle_priority()
            nice.assert_called_once_with(slow_lane.os.PRIO_PROCESS, 0, 19)
            command.assert_called_once_with(['ionice', '-c', '3', '-p', str(slow_lane.os.getpid())])

    def test_repeated_failure_rows_and_nightly_runs_file_once_per_test_and_keep_raw_proof(self):
        names = slow_lane.failures('''
        FAIL [ 1.0s] ( 1/2) pkg::binary module::one
     TIMEOUT [ 2.0s] ( 2/2) pkg::binary module::two
        FAIL [ 1.0s] pkg::binary module::one
     TIMEOUT [ 2.0s] pkg::binary module::two
''')
        self.assertEqual(names, ['pkg::binary module::one', 'pkg::binary module::two'])
        board = FakeBoard()
        first = board.file_failures(names, Path('/disposable/proof.log'), 'candidate')
        second = board.file_failures(names, Path('/disposable/proof.log'), 'candidate')
        self.assertEqual(first, ['T-001', 'T-002'])
        self.assertEqual(first, second)
        self.assertEqual(sum(call[0] == 'new' for call in board.calls), 2)
        self.assertEqual(sum(call[:2] == ('artifact', 'set-tier') and call[-1] == 'keep'
                             for call in board.calls), 4)


if __name__ == '__main__':
    unittest.main()
