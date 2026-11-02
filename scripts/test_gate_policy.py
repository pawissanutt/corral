#!/usr/bin/env python3
"""Small behavioral regressions for gate cost policy; only disposable Git/process fixtures."""
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import gate_lanes
import gate_timer
import test_cost
import gate_pi
import tomllib

ROOT = Path(__file__).resolve().parent.parent


class PolicyTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="corral-gate-policy-")
        self.addCleanup(temp.cleanup)
        previous = Path.cwd()
        os.chdir(temp.name)
        self.addCleanup(os.chdir, previous)
        Path(".config").mkdir()
        for file in ("gate.toml", "nextest.toml"):
            shutil.copy2(ROOT / ".config" / file, Path(".config") / file)
        self.git("init", "-q")
        self.write("src.rs", "#[cfg(target_os = \"macos\")]\nfn platform() {}\n")
        self.commit()
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        return subprocess.run(["git", "-c", "user.name=Fixture", "-c",
                               "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false",
                               *args], stdin=subprocess.DEVNULL, capture_output=True,
                              text=True, check=True).stdout

    def write(self, path, text):
        Path(path).parent.mkdir(parents=True, exist_ok=True)
        Path(path).write_text(text)

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")

    def test_optional_lanes_and_plan_verify_are_conservative(self):
        self.assertTrue(gate_lanes.pi_touched(["packages/corral-pi/assets/worker-contract.md"]))
        self.assertTrue(gate_lanes.pi_touched(["crates/corral-ops/src/agent/wire.rs"]))
        self.assertFalse(gate_lanes.pi_touched(["docs/guide/a.md"]))
        self.write("src.rs", "fn platform() {}\n")  # Removed cfg must still trigger Darwin.
        self.commit()
        context = {"schema_version": 1, "plan_verify": False,
                   "integration_base": self.base, "changed_paths": ["src.rs"]}
        self.assertTrue(gate_lanes.darwin_touched(context))
        context["changed_paths"] = ["docs/a.md"]
        self.assertFalse(gate_lanes.darwin_touched(context))
        for value in (True, None):
            if value is None:
                context.pop("plan_verify")  # Older contexts must not silently skip lanes.
            else:
                context["plan_verify"] = value
            self.write("context.json", json.dumps(context))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                gate_lanes.plan("context.json")
            self.assertIn("pi_touched=1", out.getvalue())
            self.assertIn("darwin_touched=1", out.getvalue())
            self.assertIn("landing_filter='all()'", out.getvalue())

    def test_slow_catalog_is_restored_by_owners_and_unknown_entries_fail_closed(self):
        ordinary = gate_lanes.landing_filter(["docs/a.md"])
        cleanup = gate_lanes.landing_filter(["crates/corral-ops/src/cleanup.rs"])
        self.assertNotIn(" or (", ordinary)
        self.assertIn(" or (", cleanup)
        self.assertIn("ledger_cleanup_closes", cleanup.split(" or (")[1])
        self.assertEqual(gate_lanes.landing_filter(["Cargo.lock"]), "all()")
        config = Path(".config/nextest.toml")
        config.write_text(config.read_text().replace("test(retained_branch_cleanup_refuses)",
                                                     "test(new_unmapped_torture)"))
        self.assertIn("new_unmapped_torture", gate_lanes.landing_filter(["docs/a.md"]).split(" or (")[1])

    def test_wall_budget_terminates_and_preserves_normal_failure(self):
        # Waiting is not load generation. The timer must stop this child, not wait 10s.
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(gate_timer.run([sys.executable, "-c", "import time; time.sleep(10)"], 0.05), 124)
            self.assertEqual(gate_timer.run([sys.executable, "-c", "raise SystemExit(7)"], 1), 7)

    def test_pi_quick_selection_follows_transitive_consumers_and_contract_roles(self):
        self.write("packages/corral-pi/src/value.ts", "export const value = 1;\n")
        self.write("packages/corral-pi/src/use.ts", 'import {value} from "./value.ts";\n')
        self.write("packages/corral-pi/test/value.test.ts", 'import "../src/use.ts";\n')
        self.write("packages/corral-pi/src/roles/worker/worker.test.ts", "// contract consumer\n")
        self.assertEqual(gate_pi.selection(["packages/corral-pi/src/value.ts"]), ["./test/value.test.ts"])
        self.assertEqual(gate_pi.selection(["packages/corral-pi/assets/worker-contract.md"]),
                         ["./src/roles/worker/worker.test.ts"])
        self.assertEqual(gate_pi.selection(["packages/corral-pi/bun.lock"]), [])

    def test_nextest_cap_and_report_only_exception_do_not_inherit_termination(self):
        config = tomllib.loads(Path(".config/nextest.toml").read_text())
        self.assertEqual(config["profile"]["default"]["slow-timeout"],
                         {"period": "10s", "terminate-after": 1})
        exception = config["profile"]["default"]["overrides"][-1]["slow-timeout"]
        self.write("Cargo.toml", '[package]\nname="cap-fixture"\nversion="0.0.0"\nedition="2024"\n[workspace]\n[lib]\npath="src.rs"\n')
        self.write("src.rs", "#[test] fn ordinary() { std::thread::sleep(std::time::Duration::from_millis(200)); }\n"
                   "#[test] fn exception() { std::thread::sleep(std::time::Duration::from_millis(200)); }\n")
        self.write("cap.toml", '[profile.default]\nslow-timeout={period="50ms", terminate-after=1}\n'
                   '[[profile.default.overrides]]\nfilter="test(=exception)"\nslow-timeout=' + json.dumps(exception) + "\n")
        result = subprocess.run(["cargo", "nextest", "run", "--lib", "--config-file", "cap.toml"],
                                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                env={**os.environ, "RUSTC_WRAPPER": ""}, check=False, timeout=60)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 100, output)
        self.assertRegex(output, r"TIMEOUT\s+\[[^]]+\].*ordinary")
        self.assertRegex(output, r"PASS\s+\[[^]]+\].*exception")

    def test_prebuilt_nextest_runs_without_compiler_or_source(self):
        self.write("Cargo.toml", '[package]\nname="reuse-fixture"\nversion="0.0.0"\nedition="2024"\n[workspace]\n[lib]\npath="src.rs"\n')
        self.write("src.rs", "#[test] fn cached() {}\n")
        self.write(".config/nextest.toml", "[profile.default]\ntest-threads=1\n")
        env = {**os.environ, "RUSTC_WRAPPER": "", "CARGO_TARGET_DIR": str(Path("target").resolve())}
        def run(*args):
            result = subprocess.run(["cargo", *args], stdin=subprocess.DEVNULL, capture_output=True,
                                    text=True, env=env, check=False, timeout=60)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result
        self.write("cargo.json", run("metadata", "--format-version", "1").stdout)
        self.write("binaries.json", run("nextest", "list", "--lib", "--list-type", "binaries-only",
                                        "--message-format", "json").stdout)
        Path("src.rs").unlink()
        result = run("nextest", "run", "--cargo-metadata", "cargo.json",
                     "--binaries-metadata", "binaries.json")
        self.assertIn("1 passed", result.stderr)
        self.assertNotIn("Compiling", result.stderr)

    def test_budget_excludes_compilation_and_busy_host_runtime(self):
        state = Path("budget.json")
        quiet = {"quiet": True, "load_per_core": 0.5, "load1": 1.0, "cores": 2}
        busy = {"quiet": False, "load_per_core": 2.0, "load1": 4.0, "cores": 2}
        with patch.object(gate_timer, "load_snapshot", return_value=quiet), \
             contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            gate_timer.initialize(state, "quick")
            data = json.loads(state.read_text())
            data["limit"] = 0.05
            state.write_text(json.dumps(data))
            self.assertEqual(gate_timer.phase(state, "cold-compile",
                             [sys.executable, "-c", "import time; time.sleep(.1)"], compile_only=True), 0)
            self.assertEqual(json.loads(state.read_text())["budgeted"], 0)
            with patch.object(gate_timer, "load_snapshot", side_effect=[quiet, quiet, busy]):
                self.assertEqual(gate_timer.phase(state, "quiet-tests",
                                 [sys.executable, "-c", "import time; time.sleep(10)"]), 124)
            ended = json.loads(state.read_text())["phases"][-1]
            self.assertFalse(ended["busy_exempt"])
            self.assertEqual(ended["enforcement_load"], quiet)
        with patch.object(gate_timer, "load_snapshot", return_value=busy), contextlib.redirect_stdout(io.StringIO()):
            before = json.loads(state.read_text())["budgeted"]
            self.assertEqual(gate_timer.phase(state, "busy-tests",
                             [sys.executable, "-c", "import time; time.sleep(.1)"]), 0)
            data = json.loads(state.read_text())
            self.assertEqual(data["budgeted"], before)
            self.assertTrue(data["phases"][-1]["busy_exempt"])
        with patch.object(gate_timer.os, "getloadavg", side_effect=OSError("unavailable")):
            self.assertFalse(gate_timer.load_snapshot()["quiet"])

    def test_added_suite_cost_requires_candidate_justification(self):
        self.write("src.rs", "#[test]\nfn catches_named_defect() {}\n")
        self.commit()
        self.assertEqual(test_cost.added_tests(self.base), {"catches_named_defect"})
        self.write("attempt.log", "        PASS [ 35.000s] (1/1) pkg module::catches_named_defect\n")
        env = {"CORRAL_INTEGRATION_BASE": self.base, "CORRAL_GATE_CONTEXT": str(Path("attempt.context.json").resolve())}
        with patch.dict(os.environ, env), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(test_cost.report("default"), 1)
            self.write("report.md", "Test-cost justification: named defect requires a process boundary.\n")
            self.commit()
            self.assertEqual(test_cost.report("default"), 0)


if __name__ == "__main__":
    unittest.main()
