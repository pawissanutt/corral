#!/usr/bin/env python3
"""Gate regressions in temporary repositories; no live herdr session is used."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Final, TypedDict, override
import unittest


SCRIPTS: Final = Path(__file__).resolve().parent
CONTEXT: Final = {
    "CORRAL_GATE_ATTEMPT": "fixture-attempt",
    "CORRAL_CANDIDATE_HEAD": "fixture-head",
    "CORRAL_INTEGRATION_BASE": "fixture-base",
}
# How corral's runner (crates/corral-ops/src/gate_script.rs) frames one configured command.
RUNNER_COMMAND: Final = "bash scripts/gate.sh"
QUICK: Final = "bash scripts/gate.sh --tier quick"
STEP_END: Final = re.compile(r"<== (.+) ([0-9]+\.[0-9]{3})s")
STARTERS: Final = (
    "e2e.sh", "screenshots.sh", "tui-drive.sh", "tui-drive-autopilot.sh",
    "tui-drive-cards.sh", "tui-drive-dispatch.sh", "tui-drive-pins.sh",
    "tui-drive-profiles.sh",
)


class StartupEntry(TypedDict):
    args: list[str]
    home: str
    config: str
    context: dict[str, str | None]
    overrides: dict[str, str | None]
    pid: int


class CommandEntry(TypedDict):
    command: str
    args: list[str]
    context: dict[str, str | None]
    session: str | None
    stdin: str


def receipt_valid(log: bytes, start: int, end: int, exit_code: int, result: str,
                  command: str = RUNNER_COMMAND) -> bool:
    """command_log_valid (crates/corral-ops/src/gate.rs) for a one-command gate."""
    marker = f"corral gate: {result}".encode()
    if start != 0 or not log[start:end].startswith(f"\n$ {command}\n".encode()):
        return False
    if (exit_code != 0) != (result == "fail"):
        return False
    results = [line.strip() for line in log.split(b"\n")
               if line.strip().startswith(b"corral gate:")]
    return log[end:].strip() == marker and results == [marker]


class GateTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.repo: Path = Path()

    @override
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory(prefix="corral-gate-")
        self.addCleanup(temp.cleanup)
        self.repo = Path(temp.name)
        (self.repo / "scripts").mkdir()
        (self.repo / "src").mkdir()
        for name in ("gate.sh", "check-candidate.py", "work-budget.sh"):
            _ = shutil.copy2(SCRIPTS / name, self.repo / "scripts" / name)
        self.write("Cargo.toml", '[package]\nname = "gate-fixture"\nversion = "0.0.0"\n'
                   + 'edition = "2024"\n[workspace]\n')
        self.write("src/lib.rs", "pub fn fixture() {}\n")
        self.command("git", "init", "-q")

    def write(self, name: str, content: str) -> None:
        _ = (self.repo / name).write_text(content)

    def command(self, *args: str) -> None:
        _ = subprocess.run(args, cwd=self.repo, stdin=subprocess.DEVNULL,
                           capture_output=True, check=True)

    def gate(self, **extra_env: str) -> subprocess.CompletedProcess[str]:
        # Supply pipe input deliberately: the gate must replace it with null stdin.
        result = subprocess.run(
            ["bash", str(self.repo / "scripts/gate.sh")],
            cwd=self.repo / "src", env={**os.environ, **CONTEXT, **extra_env},
            input="must not reach a gate command\n", capture_output=True,
            text=True, check=False, timeout=60,
        )
        print(result.stdout + result.stderr)
        return result

    def test_fmt_violation_stops_before_size_check(self) -> None:
        # Given malformed formatting in a temporary Cargo project:
        self.write("src/lib.rs", "pub fn fixture( ){let x=1;}\n")
        # When the actual gate runs real cargo fmt:
        result = self.gate()
        # Then formatting fails by name and no later step starts.
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAILED: cargo fmt --all -- --check", result.stderr)
        self.assertNotIn("==> 800-SLOC", result.stdout)

    def test_oversized_tracked_and_untracked_sources_fail(self) -> None:
        # Given a formatted 801-SLOC source, both before and after staging:
        self.write("src/lib.rs", "const _: () = ();\n" * 801)
        for tracked in (False, True):
            with self.subTest(tracked=tracked):
                if tracked:
                    self.command("git", "add", "src/lib.rs")
                # When the actual fmt and lexical counter run:
                result = self.gate()
                # Then the gate rejects size and never reaches Clippy.
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("src/lib.rs has 801 code lines (max 800)", result.stdout)
                self.assertIn("FAILED: 800-SLOC Rust ceiling", result.stderr)
                self.assertNotIn("==> cargo clippy", result.stdout)

    def install_command_recorder(self) -> str:
        """Replace expensive commands only for orchestration contract checks."""
        bin_dir = self.repo / "bin"
        bin_dir.mkdir()
        recorder = f"""#!{sys.executable}
import json
import os
from pathlib import Path
import sys

entry = {{
    "command": Path(sys.argv[0]).name,
    "args": sys.argv[1:],
    "context": {{key: os.environ.get(key) for key in {list(CONTEXT)!r}}},
    "session": os.environ.get("CORRAL_HERDR_SESSION"),
    "stdin": sys.stdin.read(),
}}
with open("commands.jsonl", "a") as log:
    log.write(json.dumps(entry) + "\\n")
if os.environ.get("FAIL_COMMAND") == entry["command"] + ":" + " ".join(entry["args"]):
    sys.exit(23)
if entry["args"][:2] == ["nextest", "list"] and os.environ.get("LIST_JSON"):
    print(Path(os.environ["LIST_JSON"]).read_text())
"""
        self.write("bin/cargo", recorder)
        self.write("scripts/check-docs.sh", recorder)
        self.write("bin/rustup", f"""#!{sys.executable}
import os, sys
print(os.environ.get("INSTALLED_TARGETS", ""))
sys.exit(int(os.environ.get("RUSTUP_STATUS", "0")))
""")
        for path in (bin_dir / "cargo", bin_dir / "rustup",
                     self.repo / "scripts/check-docs.sh"):
            path.chmod(0o755)
        return str(bin_dir) + os.pathsep + os.environ["PATH"]

    def gate_as_runner(self, command: str = RUNNER_COMMAND,
                       **extra_env: str) -> tuple[bytes, int, int, int]:
        """Run the gate the way corral's runner logs one command: header, output, footer."""
        log_path = self.repo / "gate.log"
        with log_path.open("wb") as log:
            _ = log.write(f"\n$ {command}\n".encode())
            log.flush()
            code = subprocess.run(
                ["bash", "-o", "pipefail", "-c", command], cwd=self.repo,
                env={**os.environ, **CONTEXT, **extra_env}, stdin=subprocess.DEVNULL,
                stdout=log, stderr=subprocess.STDOUT, check=False, timeout=60,
            ).returncode
            end = log.tell()
            _ = log.write(f"\ncorral gate: {'pass' if code == 0 else 'fail'}\n".encode())
        content = log_path.read_bytes()
        print(content.decode())
        return content, 0, end, code

    def assert_step_timing(self, output: str, steps: list[str], tier: str = "full") -> None:
        lines = output.splitlines()
        self.assertEqual(lines[0], f"==> tier {tier}")
        self.assertEqual([line for line in lines if line.startswith("==> tier")],
                         [f"==> tier {tier}"])
        headers = [i for i, line in enumerate(lines) if line.startswith("==> ")][1:]
        self.assertEqual([lines[i][4:] for i in headers], steps)
        for n, start in enumerate(headers):
            stop = headers[n + 1] if n + 1 < len(headers) else len(lines)
            ends = [line for line in lines[start + 1:stop] if line.startswith("<== ")]
            self.assertEqual(len(ends), 1, steps[n])
            match = STEP_END.fullmatch(ends[0])
            self.assertIsNotNone(match, ends[0])
            self.assertEqual(match and match.group(1), steps[n])
        self.assertEqual(sum(line.startswith("<== ") for line in lines), len(steps))

    def test_step_timing_keeps_runner_receipts_valid_for_pass_and_fail(self) -> None:
        # Given recorded commands, both Darwin targets, and each step (or the target
        # listing between steps) failing in turn:
        path = self.install_command_recorder()
        targets = ("aarch64-apple-darwin", "x86_64-apple-darwin")
        steps = [
            "cargo fmt --all -- --check", "800-SLOC Rust ceiling",
            *(f"cargo check --workspace --all-targets --target {t}" for t in targets),
            "cargo clippy --workspace --all-targets -- -D warnings",
            "CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace",
            "CORRAL_HERDR_SESSION=corral-test cargo test --workspace --doc",
            "scripts/work-budget.sh",
            "cargo build -p corral && scripts/check-docs.sh target/debug/corral",
        ]
        work_budget = ("cargo:test -p corral-tui --lib -- --ignored --exact "
                       "app::work_budget::board_work_stays_within_budget_and_independent_of_history")
        def fail(command: str) -> dict[str, str]:
            return {"FAIL_COMMAND": command}

        # (announced steps, failure diagnostic and status or None, environment, lib.rs)
        cases: list[tuple[int, tuple[str, int] | None, dict[str, str], str]] = [
            (9, None, {}, ""),
            (1, (steps[0], 23), fail("cargo:fmt --all -- --check"), ""),
            (2, (steps[1], 1), {}, "const _: () = ();\n" * 801),
            (2, ("listing installed Rust targets", 41), {"RUSTUP_STATUS": "41"}, ""),
            (4, (steps[3], 23),
             fail("cargo:check --workspace --all-targets --target " + targets[1]), ""),
            (5, (steps[4], 23), fail("cargo:clippy --workspace --all-targets -- -D warnings"), ""),
            (6, (steps[5], 23), fail("cargo:nextest run --workspace"), ""),
            (7, (steps[6], 23), fail("cargo:test --workspace --doc"), ""),
            (8, (steps[7], 23), fail(work_budget), ""),
            (9, (steps[8], 23), fail("cargo:build -p corral"), ""),
            (9, (steps[8], 23), fail("check-docs.sh:target/debug/corral"), ""),
        ]
        for announced, failure, env, source in cases:
            with self.subTest(failure=failure, env=env):
                self.write("src/lib.rs", source or "pub fn fixture() {}\n")
                # When the gate runs under the runner's log framing:
                log, start, end, code = self.gate_as_runner(
                    PATH=path, INSTALLED_TARGETS="\n".join(targets), **env)
                result = "pass" if failure is None else "fail"
                self.assertEqual(code, 0 if failure is None else failure[1], log.decode())
                # Then the tier line opens the output, every started step (passed or
                # failed) is timed once, a failure keeps its diagnostic as the last
                # line, and the receipt validates with exactly one result marker.
                output = log[len(f"\n$ {RUNNER_COMMAND}\n"):end].decode()
                self.assert_step_timing(output, steps[:announced])
                last = output.splitlines()[-1]
                self.assertEqual(last, "Gate passed." if failure is None
                                 else f"FAILED: {failure[0]} (exit {failure[1]})")
                self.assertTrue(receipt_valid(log, start, end, code, result))
                self.assertFalse(receipt_valid(log, start, end, code,
                                               "fail" if result == "pass" else "pass"))
                forged = log[:end] + b"corral gate: pass\n" + log[end:]
                self.assertFalse(receipt_valid(forged, start, end + 18, code, result))

    def test_order_null_stdin_context_and_exact_boundary(self) -> None:
        # Given an exactly 800-SLOC source plus comments, and recorded commands:
        self.write("src/lib.rs", "/* nested /* comment */\nignored\n*/\n"
                   + "const _: () = ();\n" * 800)
        path = self.install_command_recorder()
        # When the gate runs from a subdirectory:
        result = self.gate(PATH=path, CORRAL_HERDR_SESSION="not-the-test-session")
        # Then all commands run in order with null stdin and preserved identity.
        self.assertEqual(result.returncode, 0, result.stderr)
        entries: list[CommandEntry] = [
            json.loads(line) for line in
            (self.repo / "commands.jsonl").read_text().splitlines()
        ]
        self.assertEqual([(e["command"], e["args"]) for e in entries], [
            ("cargo", ["fmt", "--all", "--", "--check"]),
            ("cargo", ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]),
            ("cargo", ["nextest", "run", "--workspace"]),
            ("cargo", ["test", "--workspace", "--doc"]),
            ("cargo", ["test", "-p", "corral-tui", "--lib", "--", "--ignored", "--exact",
                       "app::work_budget::board_work_stays_within_budget_and_independent_of_history"]),
            ("cargo", ["build", "-p", "corral"]),
            ("check-docs.sh", ["target/debug/corral"]),
        ])
        for entry in entries:
            self.assertEqual(entry["context"], CONTEXT)
            self.assertEqual(entry["stdin"], "")
        self.assertEqual([e["session"] for e in entries],
                         ["not-the-test-session", "not-the-test-session", "corral-test", "corral-test",
                          "not-the-test-session", "not-the-test-session",
                          "not-the-test-session"])
        self.assertIn("maximum 800 (limit 800)", result.stdout)

    def install_bun_recorder(self) -> None:
        """A fake bun that logs its argv, cwd and what node resolves to and returns."""
        _ = shutil.copy2(SCRIPTS / "pi-lane.sh", self.repo / "scripts" / "pi-lane.sh")
        self.write("bin/bun", f"""#!{sys.executable}
import json, os, shutil, subprocess, sys
node = shutil.which("node")
status = subprocess.run(["node", "-e", "1"], capture_output=True).returncode
with open({str(self.repo / "bun.jsonl")!r}, "a") as log:
    log.write(json.dumps({{"args": sys.argv[1:], "cwd": os.getcwd(), "node": node,
                          "node_status": status}}) + "\\n")
""")
        (self.repo / "bin/bun").chmod(0o755)

    def bun_calls(self) -> list[dict[str, object]]:
        log = self.repo / "bun.jsonl"
        if not log.exists():
            return []
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        log.unlink()
        return calls

    def assert_pi_lane_ran(self, output: str) -> None:
        calls = self.bun_calls()
        self.assertEqual([c["args"] for c in calls],
                         [["install", "--frozen-lockfile"], ["test"], ["run", "--bun", "typecheck"]])
        package = (self.repo / "packages/corral-pi").resolve()
        for call in calls:
            self.assertEqual(Path(str(call["cwd"])).resolve(), package)
            # node, npm and npx resolve to shims that fail with 127.
            self.assertNotEqual(call["node"], shutil.which("node"))
            self.assertEqual(call["node_status"], 127)
        self.assertIn("\n==> scripts/pi-lane.sh\n", output)

    def test_pi_lane_runs_only_while_the_package_exists(self) -> None:
        # Given recorded cargo and bun, first without packages/corral-pi/:
        path = self.install_command_recorder()
        self.install_bun_recorder()
        skip = "Skipping scripts/pi-lane.sh (packages/corral-pi/ is absent)."
        # When the full tier runs, the lane is reported skipped and bun never runs:
        result = self.gate(PATH=path)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(skip, result.stdout)
        self.assertNotIn("==> scripts/pi-lane.sh", result.stdout)
        self.assertEqual(self.bun_calls(), [])
        # And the lane itself reports the skip:
        lane = subprocess.run(["bash", str(self.repo / "scripts/pi-lane.sh")], cwd=self.repo,
                              env={**os.environ, "PATH": path}, capture_output=True,
                              text=True, check=False, timeout=60)
        self.assertEqual(lane.returncode, 0, lane.stderr)
        self.assertIn("pi-lane: packages/corral-pi is absent; skipping", lane.stdout)
        self.assertEqual(self.bun_calls(), [])
        # Given the package exists: the full tier runs the lane once with node shimmed.
        (self.repo / "packages/corral-pi").mkdir(parents=True)
        result = self.gate(PATH=path)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(skip, result.stdout)
        self.assert_pi_lane_ran(result.stdout)
        # A failing bun step fails the gate by name.
        self.write("bin/bun", "#!/bin/sh\nexit 9\n")
        result = self.gate(PATH=path)
        self.assertEqual(result.returncode, 9)
        self.assertIn("FAILED: scripts/pi-lane.sh (exit 9)", result.stderr)

    def test_quick_tier_runs_the_pi_lane_only_when_the_package_is_touched(self) -> None:
        # Given the package exists and recorded cargo and bun:
        path = self.install_command_recorder()
        self.install_bun_recorder()
        (self.repo / "packages/corral-pi").mkdir(parents=True)
        for changed, runs in (("docs/guide.md", False), ("packages/corral-pi/src/a.ts", True)):
            with self.subTest(changed=changed):
                env = self.quick_workspace([changed], [], [])
                # When the quick tier runs:
                result = self.quick_gate({**env, "PATH": path})
                # Then the lane runs only for a touched package and a skip is reported otherwise.
                self.assertEqual(result.returncode, 0, result.stderr)
                if runs:
                    self.assert_pi_lane_ran(result.stdout)
                else:
                    self.assertIn("Skipping scripts/pi-lane.sh (packages/corral-pi/ untouched).",
                                  result.stdout)
                    self.assertEqual(self.bun_calls(), [])

    def test_darwin_checks_run_only_for_installed_targets(self) -> None:
        # Given neither, one, or both Darwin targets installed:
        path = self.install_command_recorder()
        targets = ("aarch64-apple-darwin", "x86_64-apple-darwin")
        for installed in ((), targets[:1], targets[1:], targets):
            with self.subTest(installed=installed):
                (self.repo / "commands.jsonl").unlink(missing_ok=True)
                # When the gate runs without installing any additional targets:
                result = self.gate(PATH=path, INSTALLED_TARGETS="\n".join(installed))
                # Then every installed Darwin target is checked, and no others.
                self.assertEqual(result.returncode, 0, result.stderr)
                entries: list[CommandEntry] = [
                    json.loads(line) for line in
                    (self.repo / "commands.jsonl").read_text().splitlines()
                ]
                checks = [e for e in entries if e["args"][0] == "check"]
                self.assertEqual([e["args"] for e in checks], [
                    ["check", "--workspace", "--all-targets", "--target", target]
                    for target in installed
                ])
                for entry in checks:
                    self.assertEqual(entry["context"], CONTEXT)
                    self.assertEqual(entry["stdin"], "")

    def test_darwin_check_failure_stops_gate(self) -> None:
        # Given both targets installed and either target's compilation fails:
        path = self.install_command_recorder()
        targets = ("aarch64-apple-darwin", "x86_64-apple-darwin")
        for target in targets:
            with self.subTest(target=target):
                (self.repo / "commands.jsonl").unlink(missing_ok=True)
                command = f"check --workspace --all-targets --target {target}"
                # When the gate reaches the failed cross-check:
                result = self.gate(PATH=path, INSTALLED_TARGETS="\n".join(targets),
                                   FAIL_COMMAND="cargo:" + command)
                # Then its status is preserved and no later command runs.
                self.assertEqual(result.returncode, 23)
                entries: list[CommandEntry] = [
                    json.loads(line) for line in
                    (self.repo / "commands.jsonl").read_text().splitlines()
                ]
                self.assertEqual(entries[-1]["args"], command.split())

    def test_first_command_failure_preserves_status_and_stops(self) -> None:
        # Given command recorders that fail at a selected later command:
        path = self.install_command_recorder()
        for command, expected_calls in (
            ("cargo:clippy --workspace --all-targets -- -D warnings", 2),
            ("cargo:nextest run --workspace", 3),
            ("cargo:test --workspace --doc", 4),
            ("cargo:build -p corral", 6),
            ("check-docs.sh:target/debug/corral", 7),
        ):
            with self.subTest(command=command):
                (self.repo / "commands.jsonl").unlink(missing_ok=True)
                # When a command fails:
                result = self.gate(PATH=path, FAIL_COMMAND=command)
                # Then the named step fails with its original status; nothing follows.
                self.assertEqual(result.returncode, 23)
                self.assertIn("FAILED:", result.stderr)
                step_command = ("cargo build -p corral" if command.startswith("check-docs")
                                else command.replace("cargo:", "cargo "))
                self.assertIn(step_command, result.stderr)
                calls = (self.repo / "commands.jsonl").read_text().splitlines()
                self.assertEqual(len(calls), expected_calls)

    def quick_workspace(self, changed: list[str], tests: list[str],
                        expected: list[str]) -> dict[str, str]:
        """Two member crates and the schema-v1 context file corral's runner exports."""
        self.write("Cargo.toml", '[workspace]\nresolver = "3"\nmembers = ["crates/*"]\n')
        (self.repo / "src/lib.rs").unlink(missing_ok=True)
        for name in () if (self.repo / "crates").exists() else ("alpha", "beta"):
            (self.repo / f"crates/{name}/src").mkdir(parents=True, exist_ok=True)
            self.write(f"crates/{name}/Cargo.toml",
                       f'[package]\nname = "{name}"\nversion = "0.0.0"\nedition = "2024"\n')
            self.write(f"crates/{name}/src/lib.rs", "pub fn fixture() {}\n")
        context = {
            "schema_version": 1, "task": "T-1", "run": "R1", "attempt": "fixture-attempt",
            "tier": "quick", "commands": ["scripts/gate.sh --tier quick"], "scope": [],
            "tests": tests,
            "expected_failures": [{"test": t, "reason": "restored by T-2"} for t in expected],
            "destination": {"kind": "plan_target"}, "integration_base": "fixture-base",
            "changed_paths": changed,
        }
        self.write("context.json", json.dumps(context))
        return {"CORRAL_GATE_CONTEXT": str(self.repo / "context.json"),
                "CORRAL_GATE_TIER": "quick"}

    def quick_gate(self, env: dict[str, str], timeout: int = 60) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            ["bash", str(self.repo / "scripts/gate.sh"), "--tier", "quick"],
            cwd=self.repo / "crates", env={**os.environ, **CONTEXT, **env},
            input="must not reach a gate command\n", capture_output=True,
            text=True, check=False, timeout=timeout,
        )
        print(result.stdout + result.stderr)
        return result

    def recorded(self) -> list[CommandEntry]:
        log = self.repo / "commands.jsonl"
        entries: list[CommandEntry] = [json.loads(line) for line in log.read_text().splitlines()]
        log.unlink()
        return entries

    def test_quick_tier_selects_touched_crates_and_exact_exclusions(self) -> None:
        # Given alpha touched, a nextest listing where only some expected failures match, a
        # same-named test in beta, and both Darwin targets installed:
        path = self.install_command_recorder()
        ids = [r"alpha::odd weird,name)", r"alpha::odd back\slash", "gamma::odd plain"]
        listing = {"rust-suites": {
            binary: {"testcases": {name: {"filter-match": {"status": status}}
                                   for name, status in cases}}
            for binary, cases in (
                ("alpha::odd", (("weird,name)", "matches"), ("back\\slash", "matches"))),
                ("beta::odd", (("weird,name)", "matches"),)),
                ("gamma::odd", (("plain", "mismatch"),)),
            )}}
        self.write("list.json", json.dumps(listing))
        excluded = (r" - (binary_id(=alpha::odd) and test(=weird\,name\)))"
                    r" - (binary_id(=alpha::odd) and test(=back\\slash))")
        session = "CORRAL_HERDR_SESSION=corral-test cargo nextest"
        # (changed paths, declared tests, expected failures, commands, announced steps)
        cases: list[tuple[list[str], list[str], list[str], list[list[str]], list[str]]] = [
            (["crates/alpha/src/lib.rs", "docs/guide.md"], [], ids, [
                ["fmt", "-p", "alpha", "--", "--check"],
                ["clippy", "-p", "alpha", "--all-targets", "--", "-D", "warnings"],
                ["nextest", "list", "-p", "alpha", "-E", "all()", "--message-format", "json"],
                ["nextest", "run", "-p", "alpha", "-E", "(all())" + excluded],
            ], ["cargo fmt -p alpha -- --check", "800-SLOC Rust ceiling",
                "cargo clippy -p alpha --all-targets -- -D warnings",
                f"{session} list -p alpha -E all()",
                f"{session} run -p alpha -E (all()){excluded}"]),
            (["crates/alpha/src/lib.rs"], ["package(alpha)", "test(=plain)"], [], [
                ["fmt", "-p", "alpha", "--", "--check"],
                ["clippy", "-p", "alpha", "--all-targets", "--", "-D", "warnings"],
                ["nextest", "run", "--workspace", "-E", "(package(alpha)) or (test(=plain))"],
            ], ["cargo fmt -p alpha -- --check", "800-SLOC Rust ceiling",
                "cargo clippy -p alpha --all-targets -- -D warnings",
                f"{session} run --workspace -E (package(alpha)) or (test(=plain))"]),
            (["Cargo.lock", "crates/beta/Cargo.toml"], [], [], [
                ["fmt", "--all", "--", "--check"],
                ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
                ["nextest", "run", "--workspace"],
            ], ["cargo fmt --all -- --check", "800-SLOC Rust ceiling",
                "cargo clippy --workspace --all-targets -- -D warnings",
                f"{session} run --workspace"]),
            (["docs/guide.md", "scripts/gate.sh"], [], [], [], ["800-SLOC Rust ceiling"]),
        ]
        for changed, tests, expected, commands, steps in cases:
            with self.subTest(changed=changed, tests=tests):
                env = self.quick_workspace(changed, tests, expected)
                # When the quick tier runs under the runner's log framing:
                log, start, end, code = self.gate_as_runner(
                    QUICK, PATH=path,
                    INSTALLED_TARGETS="aarch64-apple-darwin\nx86_64-apple-darwin",
                    LIST_JSON=str(self.repo / "list.json"), **env)
                # Then only the touched or declared selection runs, minus exactly the
                # matching expected failures; nothing of the full-only steps runs.
                self.assertEqual(code, 0, log.decode())
                self.assertTrue(receipt_valid(log, start, end, code, "pass", QUICK))
                output = log[len(f"\n$ {QUICK}\n"):end].decode()
                self.assert_step_timing(output, steps, tier="quick")
                entries = self.recorded() if commands else []
                self.assertEqual([e["args"] for e in entries], commands)
                for entry in entries:
                    self.assertEqual(entry["context"], CONTEXT)
                    self.assertEqual(entry["stdin"], "")
                self.assertIn("Skipping Darwin checks, doctests, scripts/work-budget.sh and the "
                              "docs build (tier quick).", output)
                self.assertEqual("gamma::odd plain" in expected,
                                 "Expected failure matches no selected test and excludes "
                                 "nothing: gamma::odd plain" in output)
                self.assertEqual(output.splitlines()[-1], "Gate passed.")

    def test_quick_tier_needs_context_and_known_tier(self) -> None:
        # Given no context file, or an unknown tier argument:
        path = self.install_command_recorder()
        for args, env, message in (
            (["--tier", "quick"], {"CORRAL_GATE_CONTEXT": ""}, "needs CORRAL_GATE_CONTEXT"),
            (["--tier", "lint"], {}, "usage: scripts/gate.sh"),
            (["--tier"], {}, "usage: scripts/gate.sh"),
        ):
            with self.subTest(args=args):
                result = subprocess.run(
                    ["bash", str(self.repo / "scripts/gate.sh"), *args], cwd=self.repo,
                    env={**os.environ, **CONTEXT, "PATH": path, **env},
                    capture_output=True, text=True, check=False, timeout=60)
                # Then the gate refuses with status 2 and runs no command.
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn(message, result.stderr)
                self.assertFalse((self.repo / "commands.jsonl").exists())

    @unittest.skipIf(subprocess.run(["cargo", "nextest", "--version"], capture_output=True,
                                    check=False).returncode != 0, "cargo-nextest not installed")
    def test_quick_tier_real_nextest_excludes_exactly_the_expected_failures(self) -> None:
        # Given harness-free test binaries whose names hold ',', ')' and '\', alpha's two
        # failing tests, and a passing same-named test in beta:
        _ = self.quick_workspace([], [], [])
        for name, binary_tests in (("alpha", '("weird,name)", true), ("back\\\\slash", true), '
                                             '("plain", false)'),
                                   ("beta", '("weird,name)", false)')):
            (self.repo / f"crates/{name}/tests").mkdir(exist_ok=True)
            with (self.repo / f"crates/{name}/Cargo.toml").open("a") as manifest:
                _ = manifest.write('\n[[test]]\nname = "odd"\nharness = false\n')
            self.write(f"crates/{name}/tests/odd.rs", f"""use std::io::Write;

const TESTS: &[(&str, bool)] = &[{binary_tests}];

fn main() {{
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {{
        if !args.iter().any(|a| a == "--ignored") {{
            for (name, _) in TESTS {{
                println!("{{name}}: test");
            }}
        }}
        return;
    }}
    let (name, fails) = TESTS
        .iter()
        .find(|(name, _)| args.iter().any(|a| a == name))
        .expect("a known test name");
    let mut ran = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::var("RAN_LOG").expect("RAN_LOG"))
        .expect("ran log");
    // One write per line: parallel tests append to the same log.
    let line = format!("{{}}::odd {{name}}\n", env!("CARGO_PKG_NAME"));
    ran.write_all(line.as_bytes()).expect("write");
    if *fails {{
        std::process::exit(1);
    }}
}}
""")
        self.command("cargo", "fmt", "--all")
        ran = self.repo / "ran.txt"
        cargo_env = {"RUSTC_WRAPPER": "", "CARGO_TARGET_DIR": str(self.repo / "target"),
                     "CARGO_NET_OFFLINE": "true", "RAN_LOG": str(ran)}
        both = [r"alpha::odd weird,name)", r"alpha::odd back\slash"]
        alpha, beta = ["crates/alpha/tests/odd.rs"], ["crates/beta/tests/odd.rs"]
        only_weird = r"binary_id(=alpha::odd) and test(=weird\,name\))"
        # (changed paths, declared tests, expected failures, exit status, tests that ran)
        cases: list[tuple[list[str], list[str], list[str], int, set[str]]] = [
            (alpha, ["package(alpha)", "package(beta)"], [*both, "gamma::odd plain"], 0,
             {"alpha::odd plain", r"beta::odd weird,name)"}),
            (alpha, [], both, 0, {"alpha::odd plain"}),
            (alpha, ["package(alpha)", "package(beta)"], both[:1], 100, set()),
            # Approved exclusions cover the whole declared or touched-crate selection.
            (alpha, [only_weird], both[:1], 0, set()),
            (beta, [], [r"beta::odd weird,name)"], 0, set()),
            # A declared selection that matches nothing still fails as an empty run.
            (alpha, ["test(=missing)"], both[:1], 4, set()),
        ]
        for changed, tests, expected, status, executed in cases:
            with self.subTest(changed=changed, tests=tests, expected=expected):
                ran.unlink(missing_ok=True)
                env = self.quick_workspace(changed, tests, expected)
                # When real fmt, Clippy and nextest run the quick tier:
                result = self.quick_gate({**env, **cargo_env}, timeout=600)
                # Then exactly the matching expected failures are skipped, beta's same-named
                # test still runs, and the unmatched ID is only logged.
                self.assertEqual(result.returncode, status, result.stderr)
                ran_tests = set(ran.read_text().splitlines()) if ran.exists() else set()
                if status == 0:
                    self.assertEqual(ran_tests, executed)
                elif status == 4:
                    self.assertEqual(ran_tests, set())
                    self.assertIn("error: no tests to run", result.stderr)
                else:
                    # Fail-fast may cancel the rest, so only the failure and the excluded
                    # test's absence are deterministic.
                    self.assertIn(r"alpha::odd back\slash", ran_tests)
                    self.assertNotIn(r"alpha::odd weird,name)", ran_tests)
                package = changed[0].split("/")[1]
                self.assertIn(f"==> cargo clippy -p {package} --all-targets -- -D warnings",
                              result.stdout)
                self.assertEqual("gamma::odd plain" in expected,
                                 "excludes nothing: gamma::odd plain" in result.stderr)
                self.assertEqual(not executed and status == 0,
                                 "Skipping cargo nextest run (every selected test is an "
                                 "expected failure)." in result.stdout)

    def startup_environment(self) -> dict[str, str]:
        """Fake engine records startup scope and signals readiness before waiting."""
        (self.repo / "bin").mkdir()
        for name in (*STARTERS, "test-herdr.sh"):
            _ = shutil.copy2(SCRIPTS / name, self.repo / "scripts" / name)
        self.write("bin/herdr", f"""#!{sys.executable}
import json, os, signal, sys
from pathlib import Path
home = Path(os.environ["HOME"])
config = Path(os.environ["XDG_CONFIG_HOME"])
ready = config / "ready"
if sys.argv[3:] == ["server"]:
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    entry = {{"args": sys.argv[1:], "home": str(home), "config": str(config),
             "context": {{k: os.environ.get(k) for k in {list(CONTEXT)!r}}},
             "overrides": {{k: os.environ.get(k) for k in
                 ("HERDR_CONFIG_PATH", "HERDR_SOCKET_PATH", "HERDR_SESSION", "HERDR_ENV")}},
             "pid": os.getpid()}}
    with open(os.environ["STARTUP_LOG"], "a") as log:
        log.write(json.dumps(entry) + "\\n")
    if os.environ.get("FAIL_STARTUP") == "1":
        sys.exit(29)
    ready.touch()
    while True:
        signal.pause()
sys.exit(0 if ready.exists() else 1)
""")
        self.write("bin/corral", "#!/bin/sh\nexit 91\n")
        self.write("bin/cargo", """#!/bin/sh
printf '%s\\n' '{"target_directory":"/unused"}'
""")
        for path in (self.repo / "bin").iterdir():
            path.chmod(0o755)
        owner = self.repo / "owner"
        (owner / ".config/herdr").mkdir(parents=True)
        self.write("owner/.config/herdr/config.toml", "owner plugin sentinel")
        return {
            **os.environ, **CONTEXT, "PATH": str(self.repo / "bin") + os.pathsep + os.environ["PATH"],
            "HOME": str(owner), "XDG_CONFIG_HOME": str(owner / ".config"),
            "HERDR_CONFIG_PATH": str(owner / ".config/herdr/config.toml"),
            "HERDR_SOCKET_PATH": str(owner / "live.sock"), "HERDR_SESSION": "owner",
            "HERDR_ENV": "owner",
            "CORRAL_HERDR_SESSION": "owner", "STARTUP_LOG": str(self.repo / "startups.jsonl"),
            "CORRAL": str(self.repo / "bin/corral"), "EV": str(self.repo / "evidence"),
        }

    def assert_startup_isolated(self) -> None:
        entries: list[StartupEntry] = [
            json.loads(line) for line in
            (self.repo / "startups.jsonl").read_text().splitlines()
        ]
        self.assertEqual(len(entries), 1)
        entry = entries[0]
        self.assertEqual(entry["args"], ["--session", "corral-test", "server"])
        self.assertEqual(entry["context"], CONTEXT)
        self.assertTrue(all(value is None for value in entry["overrides"].values()))
        self.assertNotEqual(entry["home"], str(self.repo / "owner"))
        self.assertNotEqual(entry["config"], str(self.repo / "owner/.config"))
        self.assertFalse(Path(entry["home"]).parent.exists())
        with self.assertRaises(ProcessLookupError):
            os.kill(entry["pid"], 0)
        self.assertEqual((self.repo / "owner/.config/herdr/config.toml").read_text(),
                         "owner plugin sentinel")

    def test_private_startup_cleanup_preserves_command_status_and_gate_identity(self) -> None:
        # Given owner config/socket overrides and a deterministic fake server:
        environment = self.startup_environment()
        for status in (0, 37):
            with self.subTest(status=status):
                (self.repo / "startups.jsonl").unlink(missing_ok=True)
                # When a client exits normally or fails after startup:
                result = subprocess.run(
                    ["bash", "-c", 'source scripts/test-herdr.sh; '
                     'corral_test_herdr_start || exit 1; '
                     'herdr --session corral-test workspace list || exit 92; '
                     'exit "$1"', "fixture", str(status)],
                    cwd=self.repo, env=environment, capture_output=True, text=True, timeout=15,
                )
                # Then only the private server was started, reaped, and removed.
                self.assertEqual(result.returncode, status, result.stderr)
                self.assert_startup_isolated()

    def test_every_owning_script_isolates_and_stops_on_server_failure(self) -> None:
        # Given every actual startup entrypoint and a server that exits before ready:
        environment = {**self.startup_environment(), "FAIL_STARTUP": "1"}
        for script in STARTERS:
            with self.subTest(script=script):
                (self.repo / "startups.jsonl").unlink(missing_ok=True)
                # When the owning script attempts startup (never the owner's engine):
                result = subprocess.run(
                    ["bash", str(self.repo / "scripts" / script)],
                    cwd=self.repo, env=environment, capture_output=True, text=True, timeout=15,
                )
                # Then startup failure stops the script and cleans its own lifecycle.
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("Isolated herdr startup failed", result.stderr)
                self.assert_startup_isolated()


if __name__ == "__main__":
    _ = unittest.main(verbosity=2)
