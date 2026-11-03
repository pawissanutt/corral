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
import gate_case
import gate_ratchet
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

    def test_time_budget_reports_and_preserves_actual_failure(self):
        # Waiting is not load generation. Crossing a time threshold cannot change status.
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(gate_timer.run([sys.executable, "-c", "import time; time.sleep(.1)"], 0.05), 0)
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

    def test_nextest_candidate_existing_and_exception_times_are_report_only(self):
        config = tomllib.loads(Path(".config/nextest.toml").read_text())
        self.assertEqual(config["profile"]["default"]["slow-timeout"], "10s")
        self.write("Cargo.toml", '[package]\nname="cap-fixture"\nversion="0.0.0"\nedition="2024"\n[workspace]\n[lib]\npath="src.rs"\n')
        existing = "#[test] fn existing() { std::thread::sleep(std::time::Duration::from_millis(200)); }\n"
        self.write("src.rs", existing)
        self.commit()
        baseline = self.git("rev-parse", "HEAD").strip()
        self.write("src.rs", existing + "#[test] fn ordinary() { std::thread::sleep(std::time::Duration::from_millis(200)); }\n"
                   "#[test] fn exception() { std::thread::sleep(std::time::Duration::from_millis(200)); }\n")
        self.write(".config/nextest.toml", '[profile.default]\nslow-timeout="50ms"\n'
                   '[[profile.default.overrides]]\nfilter="test(=exception)"\nslow-timeout="30s"\n')
        self.write(".config/gate.toml", "[tests]\nordinary_seconds=0.05\n")
        env = {**os.environ, "RUSTC_WRAPPER": ""}
        def cargo(*args):
            return subprocess.run(["cargo", *args], stdin=subprocess.DEVNULL, capture_output=True,
                                  text=True, env=env, check=False, timeout=60)
        for file, args in (("cargo.json", ("metadata", "--format-version", "1")),
                           ("binaries.json", ("nextest", "list", "--lib", "--list-type", "binaries-only", "--message-format", "json"))):
            result = cargo(*args)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.write(file, result.stdout)
        with patch.dict(os.environ, {"CORRAL_INTEGRATION_BASE": baseline}):
            key, _ = gate_case.prepare(Path("guard.json"), Path("cargo.json"), Path("binaries.json"))
        for quiet in (True, False):
            # Mock load in a disposable entrypoint, never generate host load or add a
            # production test hook. Both cases use the actual runner and actual nextest.
            self.write("runner.py", f"import sys\nsys.path.insert(0, {str(ROOT / 'scripts')!r})\n"
                       "import gate_case, gate_timer\n"
                       f"gate_timer.load_snapshot=lambda: {{'quiet': {quiet!r}, 'load_per_core': {0.5 if quiet else 2.0}}}\n"
                       "raise SystemExit(gate_case.main())\n")
            env[key] = f"{sys.executable} {Path('runner.py').resolve()} {Path('guard.json').resolve()}"
            result = cargo("nextest", "run", "--cargo-metadata", "cargo.json", "--binaries-metadata", "binaries.json")
            output = result.stdout + result.stderr
            self.assertEqual(result.returncode, 0, output)
            self.assertRegex(output, r"PASS\s+\[[^]]+\].*exception")
            self.assertRegex(output, r"PASS\s+\[[^]]+\].*existing")
            self.assertRegex(output, r"PASS\s+\[[^]]+\].*ordinary")
            self.assertIn("3 passed", output)
        config = json.loads(Path("guard.json").read_text())
        rows = [json.loads(line) for line in Path(config["metrics"]).read_text().splitlines()]
        existing_rows = [row for row in rows if row["test"][1] == "existing"]
        self.assertEqual(len(existing_rows), 2)
        self.assertTrue(all(row["classification"] == "existing-report" and row["exit_code"] == 0
                            and row["budget_exceeded"] and row["report_only"] for row in existing_rows))
        candidate_rows = [row for row in rows if row["test"][1] == "ordinary"]
        self.assertEqual(len(candidate_rows), 2)
        self.assertTrue(all(row["exit_code"] == 0 and row["budget_exceeded"] and row["report_only"]
                            for row in candidate_rows))
        with patch.dict(os.environ, {"CORRAL_GATE_ATTEMPT": "ratchet-fixture"}), contextlib.redirect_stdout(io.StringIO()):
            gate_timer.cost_audit(config, rows)
        self.assertIn("cap-fixture existing", Path(".evidence/ratchet-fixture-existing-test-costs.md").read_text())

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
            self.assertEqual(gate_timer.phase(state, "quiet-tests",
                             [sys.executable, "-c", "import time; time.sleep(.1)"]), 0)
            ended = json.loads(state.read_text())["phases"][-1]
            self.assertFalse(ended["busy_exempt"])
            self.assertTrue(ended["budget_exceeded"])
            self.assertTrue(ended["report_only"])
            self.assertIsNone(ended["enforcement_load"])
        with patch.object(gate_timer, "load_snapshot", return_value=busy), contextlib.redirect_stdout(io.StringIO()):
            before = json.loads(state.read_text())["budgeted"]
            self.assertEqual(gate_timer.phase(state, "busy-tests",
                             [sys.executable, "-c", "import time; time.sleep(.1)"]), 0)
            data = json.loads(state.read_text())
            self.assertEqual(data["budgeted"], before)
            self.assertTrue(data["phases"][-1]["busy_exempt"])
            self.assertEqual(gate_case.guarded([sys.executable, "-c", "raise SystemExit(7)"],
                             .05, ["fixture", "assertion-failure"], Path("case-metrics.jsonl")), 7)
        with patch.object(gate_timer.os, "getloadavg", side_effect=OSError("unavailable")):
            self.assertFalse(gate_timer.load_snapshot()["quiet"])

    def test_ratchet_distinguishes_modified_and_added_tests_from_existing_and_other_targets(self):
        before = '''mod tests {
#[test] fn same() { let _ = r#"} #[test] fn phantom() {}"#; let _ = '{'; }
#[test] fn changed() { assert_eq!(1, 1); }
}
fn helper() {}
'''
        self.write("src.rs", before)
        self.write("other.rs", before)
        self.commit()
        base = self.git("rev-parse", "HEAD").strip()
        self.write("src.rs", before.replace("assert_eq!(1, 1)", "assert_eq!(2, 1)")
                   + "#[test] fn added() {}\n")
        cargo = {"packages": [{"id": "pkg", "targets": [
            {"name": name, "kind": [kind], "src_path": str(Path(file).resolve())}
            for name, kind, file in (("unit", "lib", "src.rs"), ("other", "test", "other.rs"))]}]}
        binaries = {"rust-binaries": {binary: {"package-id": "pkg", "kind": kind, "binary-name": name}
                    for binary, kind, name in (("pkg", "lib", "unit"), ("pkg::other", "test", "other"))}}
        suites = {binary: {"testcases": {name: {} for name in ("tests::same", "tests::changed", "added")}}
                  for binary in binaries["rust-binaries"]}
        with patch.dict(os.environ, {"CORRAL_INTEGRATION_BASE": base}):
            enforce, observed = gate_ratchet.candidate_tests(cargo, binaries, suites)
        self.assertEqual(observed, base)
        self.assertEqual(enforce, [["pkg", "added"], ["pkg", "tests::changed"]])

    def test_added_suite_cost_reports_missing_justification_without_failing(self):
        self.write("src.rs", "#[test]\nfn catches_named_defect() {}\n")
        self.commit()
        self.assertEqual(test_cost.added_tests(self.base), {"catches_named_defect"})
        self.write("attempt.log", "        PASS [ 35.000s] (1/1) pkg module::catches_named_defect\n")
        env = {"CORRAL_INTEGRATION_BASE": self.base, "CORRAL_GATE_CONTEXT": str(Path("attempt.context.json").resolve())}
        with patch.dict(os.environ, env), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(test_cost.report("default"), 0)
            self.write("report.md", "Test-cost justification: named defect requires a process boundary.\n")
            self.commit()
            self.assertEqual(test_cost.report("default"), 0)


if __name__ == "__main__":
    unittest.main()
