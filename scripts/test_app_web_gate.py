import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class AppWebGateTests(unittest.TestCase):
    def test_gate_runs_the_packager_from_repo_root_and_propagates_failure(self):
        with tempfile.TemporaryDirectory() as scratch:
            flock = Path(scratch) / "flock"
            # Exercise the wrapper without taking a host lock or running a real build.
            flock.write_text('#!/bin/sh\nprintf "%s\\n" "$1"\nshift\nexec "$@"\n')
            flock.chmod(0o755)
            just = Path(scratch) / "just"
            just.write_text('#!/bin/sh\nprintf "%s\\n" "$PWD" "$@"\nexit 47\n')
            just.chmod(0o755)
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/app-web-gate.sh")],
                cwd=scratch,
                env={**os.environ, "PATH": scratch + os.pathsep + os.environ["PATH"]},
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 47, result.stderr)
            self.assertEqual(
                result.stdout.splitlines(),
                ["/tmp/corral-app-build.lock", str(ROOT), "app-web"],
            )


if __name__ == "__main__":
    unittest.main()
