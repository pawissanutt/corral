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


def receipt_valid(log: bytes, start: int, end: int, exit_code: int, result: str) -> bool:
    """command_log_valid (crates/corral-ops/src/gate.rs) for a one-command gate."""
    marker = f"corral gate: {result}".encode()
    if start != 0 or not log[start:end].startswith(f"\n$ {RUNNER_COMMAND}\n".encode()):
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
"""
        self.write("bin/cargo", recorder)
        self.write("scripts/check-docs.sh", recorder)
        self.write("bin/rustup", f"""#!{sys.executable}
import os
print(os.environ.get("INSTALLED_TARGETS", ""))
""")
        for path in (bin_dir / "cargo", bin_dir / "rustup",
                     self.repo / "scripts/check-docs.sh"):
            path.chmod(0o755)
        return str(bin_dir) + os.pathsep + os.environ["PATH"]

    def gate_as_runner(self, **extra_env: str) -> tuple[bytes, int, int, int]:
        """Run the gate the way corral's runner logs one command: header, output, footer."""
        log_path = self.repo / "gate.log"
        with log_path.open("wb") as log:
            _ = log.write(f"\n$ {RUNNER_COMMAND}\n".encode())
            log.flush()
            code = subprocess.run(
                ["bash", "-o", "pipefail", "-c", RUNNER_COMMAND], cwd=self.repo,
                env={**os.environ, **CONTEXT, **extra_env}, stdin=subprocess.DEVNULL,
                stdout=log, stderr=subprocess.STDOUT, check=False, timeout=60,
            ).returncode
            end = log.tell()
            _ = log.write(f"\ncorral gate: {'pass' if code == 0 else 'fail'}\n".encode())
        content = log_path.read_bytes()
        print(content.decode())
        return content, 0, end, code

    def assert_step_timing(self, output: str, steps: list[str], failed: str | None) -> None:
        lines = output.splitlines()
        self.assertEqual(lines[0], "==> tier full")
        self.assertEqual([line for line in lines if line.startswith("==> tier")],
                         ["==> tier full"])
        headers = [i for i, line in enumerate(lines) if line.startswith("==> ")][1:]
        self.assertEqual([lines[i][4:] for i in headers], steps)
        finished = 0
        for n, start in enumerate(headers):
            stop = headers[n + 1] if n + 1 < len(headers) else len(lines)
            ends = [line for line in lines[start + 1:stop] if line.startswith("<== ")]
            if steps[n] == failed:
                self.assertEqual(ends, [], steps[n])
                continue
            self.assertEqual(len(ends), 1, steps[n])
            match = STEP_END.fullmatch(ends[0])
            self.assertIsNotNone(match, ends[0])
            self.assertEqual(match and match.group(1), steps[n])
            finished += 1
        self.assertEqual(sum(line.startswith("<== ") for line in lines), finished)

    def test_step_timing_keeps_runner_receipts_valid_for_pass_and_fail(self) -> None:
        # Given recorded commands, both Darwin targets, and each step failing in turn:
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
        cases: list[tuple[str | None, str, str]] = [
            (None, "", ""), (steps[0], "cargo:fmt --all -- --check", ""),
            (steps[1], "", "const _: () = ();\n" * 801),
            (steps[3], "cargo:check --workspace --all-targets --target " + targets[1], ""),
            (steps[4], "cargo:clippy --workspace --all-targets -- -D warnings", ""),
            (steps[5], "cargo:nextest run --workspace", ""),
            (steps[6], "cargo:test --workspace --doc", ""), (steps[7], work_budget, ""),
            (steps[8], "cargo:build -p corral", ""),
            (steps[8], "check-docs.sh:target/debug/corral", ""),
        ]
        for failed, command, source in cases:
            with self.subTest(failed=failed, command=command):
                self.write("src/lib.rs", source or "pub fn fixture() {}\n")
                # When the gate runs under the runner's log framing:
                log, start, end, code = self.gate_as_runner(
                    PATH=path, INSTALLED_TARGETS="\n".join(targets), FAIL_COMMAND=command)
                result = "pass" if failed is None else "fail"
                self.assertEqual(code == 0, failed is None, log.decode())
                # Then the tier line opens the output, each finished step is timed
                # once, and the receipt validates with exactly one result marker.
                output = log[len(f"\n$ {RUNNER_COMMAND}\n"):end].decode()
                announced = steps if failed is None else steps[:steps.index(failed) + 1]
                self.assert_step_timing(output, announced, failed)
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
