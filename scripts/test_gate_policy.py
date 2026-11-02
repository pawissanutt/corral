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
