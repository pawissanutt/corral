#!/usr/bin/env python3
"""Regression for report-only timing output failures."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import gate_timer


class ReportTests(unittest.TestCase):
    def test_blocking_stdout_does_not_fail_report(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / 'state.json'
            state.write_text(json.dumps({'tier': 'quick', 'measured': 1, 'budgeted': 1,
                                         'compile': 0, 'limit': 180}))
            with patch('gate_timer.os.set_blocking'), patch('sys.stdout') as stdout, \
                 patch('sys.stderr') as stderr, patch('sys.argv', ['gate_timer.py', 'report', str(state)]):
                stdout.write.side_effect = BlockingIOError(11, 'would block')
                self.assertEqual(gate_timer.main(), 0)
                self.assertIn('would block', ''.join(call.args[0] for call in stderr.write.call_args_list))


if __name__ == '__main__':
    unittest.main()
