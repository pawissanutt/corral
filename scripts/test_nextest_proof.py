#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
# Run: python3 scripts/test_nextest_proof.py
"""Proof identity regressions in disposable Git repositories; no real herdr."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from typing import Final, TypedDict, cast, override
import unittest


SCRIPTS: Final = Path(__file__).resolve().parent


class CandidateRecord(TypedDict):
    head: str
    base: str
    tree: str
    status: str


class ProofRecord(TypedDict):
    candidate: CandidateRecord
    candidate_after: CandidateRecord
    main_tip: str
    main_tip_after: str
    valid: bool


class ProofIdentityTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.repo: Path = Path()
        self.environment: dict[str, str] = {}
        self.base: str = ""

    @override
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory(prefix="corral-proof-")
        self.addCleanup(temp.cleanup)
        self.repo = Path(temp.name)
        (self.repo / "scripts/qa").mkdir(parents=True)
        (self.repo / "bin").mkdir()
        _ = shutil.copy2(
            SCRIPTS / "qa/t205-nextest-proof.py",
            self.repo / "scripts/qa/t205-nextest-proof.py",
        )
        _ = (self.repo / ".gitignore").write_text("evidence/\n")
        _ = (self.repo / "scripts/test-herdr.sh").write_text(
            "corral_test_herdr_start() { return 0; }\n"
        )
        cargo = self.repo / "bin/cargo"
        _ = cargo.write_text(f"""#!{sys.executable}
import os
import subprocess

def git(*args: str) -> str:
    return subprocess.check_output(
        ["git", *args], stdin=subprocess.DEVNULL, text=True,
    ).strip()

ref = os.environ["PROOF_CHANGED_REF"]
parent = git("rev-parse", ref)
tree = git("rev-parse", ref + "^{{tree}}")
tip = git("commit-tree", tree, "-p", parent, "-m", "Concurrent update")
git("update-ref", "refs/heads/" + ref, tip)
print("Summary [ 0.001s] 1 tests run: 1 passed")
""")
        cargo.chmod(0o755)
        self.environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("GIT_", "CORRAL_", "HERDR_"))
        }
        self.environment.update({
            "HOME": str(self.repo / "home"),
            "XDG_CONFIG_HOME": str(self.repo / "config"),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": "Proof fixture",
            "GIT_AUTHOR_EMAIL": "proof@example.invalid",
            "GIT_COMMITTER_NAME": "Proof fixture",
            "GIT_COMMITTER_EMAIL": "proof@example.invalid",
            "PATH": str(self.repo / "bin") + os.pathsep + os.environ["PATH"],
        })
        _ = self.git("init", "--quiet", "--initial-branch=main")
        _ = self.git("add", ".")
        _ = self.git("commit", "--quiet", "-m", "Initial fixture")
        self.base = self.git("rev-parse", "HEAD")
        _ = self.git("checkout", "--quiet", "-b", "candidate")

    def git(self, *args: str) -> str:
        return subprocess.check_output(
            ["git", *args], cwd=self.repo, env=self.environment,
            stdin=subprocess.DEVNULL, text=True,
        ).strip()

    def run_proof(self, ref: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(self.repo / "scripts/qa/t205-nextest-proof.py"),
             str(self.repo / "evidence")],
            cwd=self.repo, env={**self.environment, "PROOF_CHANGED_REF": ref},
            stdin=subprocess.DEVNULL, capture_output=True, text=True,
            check=False, timeout=30,
        )

    def proof(self, run: int) -> ProofRecord:
        return cast(
            ProofRecord,
            json.loads((self.repo / f"evidence/suite-{run}.json").read_text()),
        )

    def test_main_updates_do_not_change_the_tested_candidate(self) -> None:
        # Given a fixed candidate while another branch receives new commits.
        before = self.git("rev-parse", "HEAD")
        # When main advances during every synthetic suite.
        result = self.run_proof("main")
        # Then all five passes retain their actual source integration ancestor.
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotEqual(self.git("rev-parse", "main"), self.base)
        for run in range(1, 6):
            record = self.proof(run)
            self.assertTrue(record["valid"])
            self.assertEqual(record["candidate"], record["candidate_after"])
            self.assertEqual(record["candidate"]["head"], before)
            self.assertEqual(record["candidate"]["base"], self.base)
            self.assertNotEqual(record["main_tip"], record["main_tip_after"])

    def test_candidate_updates_still_stop_the_proof(self) -> None:
        # Given a candidate whose HEAD changes while its synthetic suite runs.
        result = self.run_proof("candidate")
        # Then the source identity guard rejects it instead of accepting five.
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        record = self.proof(1)
        self.assertFalse(record["valid"])
        self.assertNotEqual(
            record["candidate"]["head"], record["candidate_after"]["head"]
        )
        self.assertFalse((self.repo / "evidence/suite-2.json").exists())


if __name__ == "__main__":
    _ = unittest.main(verbosity=2)
