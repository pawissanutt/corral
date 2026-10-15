#!/usr/bin/env python3
"""Gate regressions in temporary repositories; no live herdr session is used."""

import json
import os
from pathlib import Path
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


class CommandEntry(TypedDict):
    command: str
    args: list[str]
    context: dict[str, str | None]
    session: str | None
    stdin: str


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
        for name in ("gate.sh", "check-candidate.py"):
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
        for path in (bin_dir / "cargo", self.repo / "scripts/check-docs.sh"):
            path.chmod(0o755)
        return str(bin_dir) + os.pathsep + os.environ["PATH"]

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
            ("cargo", ["test", "--workspace"]),
            ("cargo", ["build", "-p", "corral"]),
            ("check-docs.sh", ["target/debug/corral"]),
        ])
        for entry in entries:
            self.assertEqual(entry["context"], CONTEXT)
            self.assertEqual(entry["stdin"], "")
        self.assertEqual([e["session"] for e in entries],
                         ["not-the-test-session", "not-the-test-session", "corral-test",
                          "not-the-test-session", "not-the-test-session"])
        self.assertIn("maximum 800 (limit 800)", result.stdout)

    def test_first_command_failure_preserves_status_and_stops(self) -> None:
        # Given command recorders that fail at a selected later command:
        path = self.install_command_recorder()
        for command, expected_calls in (
            ("cargo:clippy --workspace --all-targets -- -D warnings", 2),
            ("cargo:test --workspace", 3),
            ("cargo:build -p corral", 4),
            ("check-docs.sh:target/debug/corral", 5),
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


if __name__ == "__main__":
    _ = unittest.main(verbosity=2)
