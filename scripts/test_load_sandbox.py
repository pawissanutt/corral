#!/usr/bin/env python3
"""Deterministic wrapper checks; --live additionally uses real Podman (no host burners)."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().with_name("load-sandbox.sh")
LIVE = "--live" in sys.argv
if LIVE:
    sys.argv.remove("--live")

FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, signal, sys
root = pathlib.Path(os.environ["FAKE_STATE"])
args = sys.argv[1:]
with (root / "calls").open("a") as log:
    log.write(json.dumps(args) + "\n")
state = root / "container"
if args[:2] == ["container", "exists"]:
    sys.exit(0 if state.exists() else 1)
if args[0] == "create":
    state.touch()
    if os.environ.get("FAKE_MODE") == "create-fail":
        sys.exit(42)
elif args[0] == "rm":
    state.unlink()
elif args[0] == "start":
    if os.environ.get("FAKE_MODE") == "interrupt":
        os.kill(os.getppid(), signal.SIGTERM)
    else:
        sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
'''


class SandboxTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        fake = self.root / "podman"
        fake.write_text(FAKE)
        fake.chmod(0o755)
        self.env = dict(os.environ, PATH=f"{self.root}:{os.environ['PATH']}",
                        FAKE_STATE=str(self.root))

    def run_sandbox(self, *args, **env):
        result = subprocess.run([str(SCRIPT), *args], env=dict(self.env, **env),
                                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                timeout=30)
        self.assertFalse((self.root / "container").exists(), result.stderr)
        return result

    def calls(self):
        path = self.root / "calls"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_refuses_unbounded_resources_before_podman(self):
        for option, values in (("--cpus", ["4.01", "5", "0", "-1", "nan", "inf", "1e9"]),
                               ("--memory", ["0", "-1"]),
                               ("--pids-limit", ["0", "-1"]),
                               ("--burners", ["-1", "100"])):
            for value in values:
                with self.subTest(option=option, value=value):
                    self.assertEqual(self.run_sandbox(option, value, "--", "true").returncode, 2)
        self.assertEqual(self.calls(), [])

    def test_defaults_argv_and_readonly_tools(self):
        result = self.run_sandbox("--burners", "2", "--", "printf", "%s", "a b; exit 99")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        create = calls[0]
        for flag, value in (("--cpus", "2"), ("--memory", "2g"),
                            ("--memory-swap", "2g"), ("--pids-limit", "256")):
            self.assertEqual(create[create.index(flag) + 1], value)
        self.assertEqual(create[-5:], ["load-sandbox", "2", "printf", "%s", "a b; exit 99"])
        mounts = [create[i + 1] for i, arg in enumerate(create) if arg == "--volume"]
        self.assertTrue(all(m.endswith(":ro") for m in mounts[1:]))
        self.assertEqual(calls[-1][0], "rm")

    def test_boundary_and_test_exit_status(self):
        result = self.run_sandbox("--cpus", "4", "--", "false", FAKE_EXIT="17")
        self.assertEqual(result.returncode, 17)

    def test_cleanup_after_partial_create_failure(self):
        result = self.run_sandbox("--", "true", FAKE_MODE="create-fail")
        self.assertEqual(result.returncode, 42)
        self.assertEqual(self.calls()[-1][0], "rm")

    def test_cleanup_on_interrupt(self):
        result = self.run_sandbox("--", "true", FAKE_MODE="interrupt")
        self.assertEqual(result.returncode, 143)
        self.assertEqual(self.calls()[-1][0], "rm")


@unittest.skipUnless(LIVE, "pass --live to exercise real Podman")
class LiveSandboxTests(unittest.TestCase):
    def containers(self):
        return subprocess.check_output(
            ["podman", "ps", "-aq", "--filter", "label=corral.load-sandbox=true"],
            stdin=subprocess.DEVNULL, text=True).splitlines()

    def test_live_limits_exit_and_interrupt_cleanup(self):
        before = self.containers()
        rejected = subprocess.run([str(SCRIPT), "--cpus", "5", "--", "true"],
                                  stdin=subprocess.DEVNULL)
        self.assertEqual(rejected.returncode, 2)
        self.assertEqual(self.containers(), before)
        image = os.environ.get("LOAD_SANDBOX_TEST_IMAGE", "docker.io/library/debian:trixie-slim")
        cmd = [str(SCRIPT), "--image", image, "--burners", "1", "--", "bash", "-c"]
        # Assertions inspect configured cgroup values, never elapsed time or load averages.
        result = subprocess.run(cmd + [
            'test "$(cat /sys/fs/cgroup/cpu.max)" = "200000 100000" && '
            'test "$(cat /sys/fs/cgroup/memory.max)" = 2147483648 && '
            'test "$(cat /sys/fs/cgroup/pids.max)" = 256 && '
            'cargo --version && rustc --version && bun --version && exit 17'],
            stdin=subprocess.DEVNULL)
        self.assertEqual(result.returncode, 17)
        self.assertEqual(self.containers(), before)
        success = subprocess.run(cmd + [
            'mkdir -p /tmp/probe/src; '
            'printf "[package]\\nname=\\\"sandbox-probe\\\"\\nversion=\\\"0.1.0\\\"\\n" > /tmp/probe/Cargo.toml; '
            'echo "pub fn probe() {}" > /tmp/probe/src/lib.rs; '
            'cargo check --offline --manifest-path /tmp/probe/Cargo.toml && '
            'bun -e "if (1 + 1 !== 2) process.exit(1)"'],
            stdin=subprocess.DEVNULL)
        self.assertEqual(success.returncode, 0)
        self.assertEqual(self.containers(), before)
        # The pipe is the readiness barrier. The test command blocks on its child;
        # send SIGTERM only after it has started, with no scheduling/timing assertion.
        process = subprocess.Popen(cmd + ['echo READY; sleep infinity'],
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(process.stdout.readline().strip(), "READY")
            process.send_signal(signal.SIGTERM)
            self.assertEqual(process.wait(timeout=30), 143)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=30)
            process.stdout.close()
        self.assertEqual(self.containers(), before)


if __name__ == "__main__":
    unittest.main(verbosity=2)
