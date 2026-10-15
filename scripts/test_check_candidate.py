#!/usr/bin/env python3
"""Isolated CLI tests; run directly with Python's standard library."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from typing import TypedDict


SCRIPT = Path(__file__).resolve().with_name("check-candidate.py")


class TestReport(TypedDict):
    base: str | None
    counts: dict[str, int]
    violations: list[dict[str, str | int | None]]
    rust_files: list[dict[str, str | int | list[str]]]


class CandidateTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.repo = Path()
        self.base = ""

    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.repo = Path(temp.name)
        self.git("init", "-q")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "Guard Test")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", str(self.repo / "no-hooks"))
        self.write("initial", "base\n")
        self.commit()
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args: str) -> str:
        return subprocess.run(
            ["git", *args], cwd=self.repo, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
        ).stdout.decode()

    def write(self, name: str, content: str) -> None:
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def commit(self) -> None:
        self.git("add", "-A")
        self.git("commit", "-qm", "fixture")

    def guard(self, *args: str, env_base: str | None = None) -> tuple[int, TestReport]:
        environment = dict(os.environ)
        environment.pop("CORRAL_INTEGRATION_BASE", None)
        if env_base is not None:
            environment["CORRAL_INTEGRATION_BASE"] = env_base
        result = subprocess.run(
            [sys.executable, str(SCRIPT), *args], cwd=self.repo, env=environment,
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.stderr, b"")
        return result.returncode, json.loads(result.stdout)

    def snapshot(self) -> dict[str, str]:
        """Hash repository and Git files to detect even index/cache writes."""
        return {
            str(path.relative_to(self.repo)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in self.repo.rglob("*") if path.is_file() and not path.is_symlink()
        }

    def test_missing_base_fails(self) -> None:
        # Given a clean repository, when no base is supplied:
        code, report = self.guard()
        # Then missing integration context fails closed.
        self.assertEqual(code, 1)
        self.assertEqual([v["reason"] for v in report["violations"]], ["BASE_REQUIRED"])

    def test_invalid_base_fails(self) -> None:
        # Given a clean repository, when an unknown ref is supplied:
        code, report = self.guard("--base=--not-a-ref")
        # Then the ref cannot be interpreted as a Git option.
        self.assertEqual(code, 1)
        self.assertEqual(report["violations"][0]["reason"], "BASE_INVALID")

    def test_environment_base_passes_without_mutation(self) -> None:
        # Given a clean changed candidate and its complete before snapshot:
        self.write("small.rs", "fn main() {}\n")
        self.commit()
        before = self.snapshot()
        # When the gate uses the environment base:
        code, report = self.guard(env_base=self.base)
        # Then it passes and changes no files, including the Git index.
        self.assertEqual(code, 0)
        self.assertEqual(report["counts"]["rust_files"], 1)
        self.assertEqual(self.snapshot(), before)

    def test_explicit_base_overrides_environment(self) -> None:
        # Given a valid explicit base and invalid environment base:
        code, report = self.guard("--base", self.base, env_base="missing-ref")
        # Then only the explicit base is selected.
        self.assertEqual(code, 0)
        self.assertEqual(report["base"], self.base)

    def test_all_offenders_and_exact_boundary(self) -> None:
        # Given two oversized files plus an exact-boundary test-bearing file:
        for name, size in (("a.rs", 801), ("b.rs", 802), ("tests.rs", 800)):
            self.write(name, "#[test]\n" + "fn f() {}\n" * (size - 1))
        self.commit()
        # When all changes are checked:
        code, report = self.guard("--base", self.base)
        # Then every offender is reported and 800 passes.
        self.assertEqual(code, 1)
        self.assertEqual({f["path"]: f["sloc"] for f in report["rust_files"]},
                         {"a.rs": 801, "b.rs": 802, "tests.rs": 800})
        self.assertEqual({v["path"] for v in report["violations"]}, {"a.rs", "b.rs"})
        self.assertEqual(report["counts"]["policy_paths"], 3)
        print("boundary evidence:", json.dumps(report, sort_keys=True))

    def test_lexical_comments_strings_chars_and_lifetimes(self) -> None:
        # Given comments, comment-like string content, hashes and lifetimes:
        self.write("lex.rs", r'''// ignored
/* outer
 /* nested */ still outer
*/
fn f<'a>(x: &'a str) { // code
let c = '/'; let q = '\\''; let u = '\\u{1F600}';
let s = "start
// string content
/* string content */

end"; // ignored
let raw = br##"start
"# // still raw
/* still raw */

end"##;
/* ignore */ let c = '\\x2f'; /* ignore */
}
''')
        self.commit()
        # When lexical SLOC is calculated:
        code, report = self.guard("--base", self.base)
        # Then comment-only/blank lines vanish but string lines remain.
        self.assertEqual(code, 0)
        self.assertEqual(report["rust_files"][0]["sloc"], 12)

    def test_escape_continuation_and_raw_closers(self) -> None:
        # Given escaped normal quotes/newlines and raw C strings:
        self.write("strings.rs", 'let s = "a\\"/*\n// content\\\n  \nend";\n'
                   'let r = cr#"x\n// content\n"#;\n// ignored\n')
        self.commit()
        # When the guard counts the file:
        code, report = self.guard("--base", self.base)
        # Then string/comment boundaries persist across physical lines.
        self.assertEqual(code, 0)
        self.assertEqual(report["rust_files"][0]["sloc"], 6)

    def test_rename_and_deletion_policy_paths(self) -> None:
        # Given an oversized Rust source renamed to text and a deleted source:
        self.write("old.rs", "fn f() {}\n" * 801)
        self.write("gone.rs", "fn g() {}\n" * 900)
        self.commit()
        base = self.git("rev-parse", "HEAD").strip()
        self.git("mv", "old.rs", "new.txt")
        (self.repo / "gone.rs").unlink()
        self.commit()
        # When the renamed and deleted paths are checked:
        code, report = self.guard("--base", base)
        # Then the renamed blob retains Rust policy; deletion has no content.
        self.assertEqual(code, 1)
        self.assertEqual(report["counts"]["policy_paths"], 3)
        self.assertEqual(report["counts"]["deleted_entries"], 1)
        self.assertEqual(report["rust_files"][0]["policy_paths"], ["old.rs", "new.txt"])
        self.assertEqual(report["rust_files"][0]["sloc"], 801)

    def test_markers_in_every_text_path_and_safe_examples(self) -> None:
        # Given markers in extensionless, Markdown, invalid UTF-8 and Rust files:
        self.write("README.md", "<<<<<<< ours\n=======\n>>>>>>> theirs\n")
        self.write(".hidden", "||||||| base\n")
        self.write("code.rs", 'let example = "<<<<<<< ours";\n  =======\n// >>>>>>>\n')
        (self.repo / "odd").write_bytes(b"\xff\n>>>>>>>> theirs\r\n")
        self.write("safe.txt", 'prefix <<<<<<< ours\n"======="\n =======\n')
        self.commit()
        # When all regular text blobs are scanned:
        code, report = self.guard("--base", self.base)
        # Then standalone forms fail, while quoted/indented examples pass.
        self.assertEqual(code, 1)
        self.assertEqual(report["counts"]["text_files"], 5)
        self.assertEqual(len(report["violations"]), 5)
        self.assertEqual({v["path"] for v in report["violations"]},
                         {"README.md", ".hidden", "odd"})

    def test_dirty_staged_untracked_and_unstaged_aggregate(self) -> None:
        # Given all three kinds of dirt:
        self.write("initial", "modified\n")
        self.write("staged", "staged\n")
        self.git("add", "staged")
        self.write("untracked", "new\n")
        before = self.snapshot()
        # When the guard runs:
        code, report = self.guard("--base", self.base)
        # Then every dirty path is reported without modifying the index.
        self.assertEqual(code, 1)
        self.assertEqual({v["path"] for v in report["violations"]},
                         {"initial", "staged", "untracked"})
        self.assertEqual(self.snapshot(), before)

    def test_operations_aggregate_with_content_violations(self) -> None:
        # Given unresolved operation state plus a committed marker:
        self.write("bad", "=======\n")
        self.commit()
        operations = ("MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge",
                      "rebase-apply", "sequencer", "BISECT_LOG", "AUTO_MERGE")
        for name in operations:
            path = Path(self.git("rev-parse", "--git-path", name).strip())
            path = self.repo / path
            if name in ("rebase-merge", "rebase-apply", "sequencer"):
                path.mkdir()
            else:
                path.write_text(self.base + "\n")
        # When the guard runs:
        code, report = self.guard("--base", self.base)
        # Then it reports every operation and still scans content.
        self.assertEqual(code, 1)
        self.assertEqual({v["detail"] for v in report["violations"]
                          if v["reason"] == "GIT_OPERATION_IN_PROGRESS"}, set(operations))
        self.assertIn("CONFLICT_MARKER", [v["reason"] for v in report["violations"]])

    def test_symlink_and_binary_never_follow_filesystem(self) -> None:
        # Given a changed symlink outside the repository and binary blobs:
        (self.repo / "escape.rs").symlink_to("/dev/zero")
        (self.repo / "binary.rs").write_bytes(b"\0fn f() {}")
        (self.repo / "asset").write_bytes(b"\0<<<<<<< ours\n")
        self.commit()
        # When the guard runs:
        code, report = self.guard("--base", self.base)
        # Then it fails closed on Rust binary/symlink without following the link.
        self.assertEqual(code, 1)
        self.assertEqual({v["reason"] for v in report["violations"]},
                         {"UNSUPPORTED_FILE_MODE", "RUST_BINARY_CONTENT"})
        self.assertEqual(report["counts"]["binary_files"], 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
