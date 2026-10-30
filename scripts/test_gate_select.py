#!/usr/bin/env python3
"""Quick-tier test selection (scripts/gate_select.py) in a temporary workspace."""

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
from typing import Final, override
import unittest

SELECT: Final = Path(__file__).resolve().parent / "gate_select.py"
FILES: Final = {
    "crates/alpha/Cargo.toml": '[package]\nname = "alpha"\n',
    "crates/alpha/src/lib.rs": "mod a;\npub mod b;\nmod c;\nmod helper;\nmod lone;\nmod widget;\n"
                               "pub use widget::Gizmo;\n#[cfg(test)]\nmod a_tests;\n"
                               "#[cfg(test)]\nmod e_tests;\n"
                               '#[cfg(test)]\n#[path = "../tests/shared/mod.rs"]\nmod shared;\n',
    "crates/alpha/src/a.rs": "use crate::helper;\n#[cfg(test)]\nmod tests {}\n",
    "crates/alpha/src/helper.rs": "",
    "crates/alpha/src/c.rs": ("mod d;\n#[cfg(test)]\nmod tests {\n    #[test]\n"
                              "    fn calls_a() { crate::a::run(); }\n}\n"),
    "crates/alpha/src/c/d.rs": "#[test]\nfn t() {}\n",
    "crates/alpha/src/e_tests.rs": ("#[test]\nfn uses_widget() { let _ = crate::Gizmo; }\n"
                                    "#[test]\nfn uses_shared() { crate::shared::f(); }\n"),
    "crates/alpha/src/a_tests.rs": "",
    "crates/alpha/src/b/mod.rs": "pub mod inner;\n",
    "crates/alpha/src/b/inner.rs": "",
    "crates/alpha/src/lone.rs": "",
    "crates/alpha/src/widget.rs": 'pub struct Gizmo;\nconst D: &str = include_str!("data.toml");\n',
    "crates/alpha/src/orphan.rs": "",
    "crates/alpha/src/data.toml": "",
    "crates/alpha/src/notes.md": "",
    "crates/alpha/tests/one.rs": 'use alpha::a::thing;\n#[path = "shared/mod.rs"]\nmod shared;\n',
    "crates/alpha/tests/shared/mod.rs": "",
    "crates/alpha/tests/two.rs": ("use alpha::{Gizmo, b::inner};\n"
                                   '#[path = "two/helper.rs"]\nmod helper;\n'),
    "crates/alpha/tests/two/helper.rs": "",
    "crates/alpha/tests/three.rs": "use alpha::*;\nfn f() { let _ = Gizmo; }\n",
    "crates/alpha/tests/four.rs": ('#[path = "four/inner_cases.rs"]\nmod inner_cases;\n'
                                   '#[path = "four/other.rs"]\nmod other;\n'),
    "crates/alpha/tests/four/inner_cases.rs": "use alpha::b::inner;\n",
    "crates/alpha/tests/four/other.rs": "",
    "crates/alpha/tests/fixtures/data.json": "{}",
    "crates/tool/Cargo.toml": '[package]\nname = "tool"\n',
    "crates/tool/src/main.rs": "mod cmd;\nfn main() {}\n",
    "crates/tool/src/cmd.rs": "",
    "crates/tool/tests/cli.rs": "",
}
SUITES: Final = {
    ("alpha", "lib"): {"a::tests::x": "matches", "a_tests::y": "matches",
                       "b::inner::t": "matches", "b::other": "matches", "skip": "ignored",
                       "c::tests::calls_a": "matches", "c::d::t": "matches",
                       "e_tests::uses_widget": "matches", "e_tests::uses_shared": "matches"},
    ("alpha::one", "test"): {"one_t": "matches", "slow_one": "default-filter"},
    ("alpha::two", "test"): {"two_t": "matches"},
    ("alpha::three", "test"): {"three_t": "matches"},
    ("alpha::four", "test"): {"inner_cases::t": "matches", "other::t": "matches"},
    ("tool::bin/tool", "bin"): {"cmd::t": "matches"},
    ("tool::cli", "test"): {"cli_t": "matches"},
}
UNIT_A: Final = "(binary_id(=alpha) and (test(/^a::/) or test(/^a_tests::/)))"
CALLS_A: Final = "(binary_id(=alpha) and test(/^c::/) and not test(/^c::d::/))"
WIDGET: Final = ("(binary_id(=alpha) and (test(/^widget::/) or test(/^widget_tests::/))) or "
                 "(binary_id(=alpha) and test(/^e_tests::/)) or "
                 "(binary_id(=alpha::three)) or (binary_id(=alpha::two))")


def listing(statuses: dict[tuple[str, str], dict[str, str]]) -> dict[str, object]:
    return {"rust-suites": {binary: {
        "package-name": binary.split("::")[0], "binary-id": binary, "kind": kind,
        "binary-name": binary.split("::")[-1].split("/")[-1],
        "testcases": {name: {"ignored": status == "ignored", "filter-match":
                             {"status": "matches"} if status == "matches"
                             else {"status": "mismatch", "reason": status}}
                      for name, status in cases.items()}}
        for (binary, kind), cases in statuses.items()}}


class SelectTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.repo: Path = Path()

    @override
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory(prefix="corral-select-")
        self.addCleanup(temp.cleanup)
        self.repo = Path(temp.name)
        for name, content in FILES.items():
            (self.repo / name).parent.mkdir(parents=True, exist_ok=True)
            _ = (self.repo / name).write_text(content)

    def select(self, changed: list[str], tests: list[str] | None = None,
               expected: list[str] | None = None,
               statuses: dict[tuple[str, str], dict[str, str]] = SUITES,
               ) -> tuple[dict[str, str], list[str]]:
        context = {"schema_version": 1, "tests": tests or [], "changed_paths": changed,
                   "expected_failures": [{"test": t, "reason": "r"} for t in expected or []]}
        _ = (self.repo / "context.json").write_text(json.dumps(context))
        _ = (self.repo / "list.json").write_text(json.dumps(listing(statuses)))
        result = subprocess.run(
            [sys.executable, str(SELECT), "select", "context.json", "list.json",
             *(expected or [])], cwd=self.repo, capture_output=True, text=True, check=True,
            stdin=subprocess.DEVNULL, env={**os.environ})
        values = dict(line.split("=", 1) for line in result.stdout.splitlines())
        return {key: shlex.split(value)[0] for key, value in values.items()}, \
            result.stderr.splitlines()

    def test_a_changed_module_selects_its_unit_tests_and_the_targets_that_name_it(self) -> None:
        # (changed path, run filter, note, selected, remaining)
        cases = [
            # Its unit tests, a library test module elsewhere that calls it (not that file's
            # child module), and a target naming `alpha::a`; the slow test does not run.
            ("crates/alpha/src/a.rs", f"{UNIT_A} or {CALLS_A} or (binary_id(=alpha::one))",
             "module a (its unit tests, 1 calling library test modules and 1 test targets)", 5, 4),
            # A crate-root re-export, named in a use group and after a glob import.
            ("crates/alpha/src/widget.rs", WIDGET,
             "module widget (its unit tests, 1 calling library test modules and 2 test targets)", 4, 4),
            # A module no test names is exercised through the modules that use it.
            ("crates/alpha/src/helper.rs",
             "(binary_id(=alpha) and (test(/^helper::/) or test(/^helper_tests::/))) or "
             f"{UNIT_A} or {CALLS_A} or (binary_id(=alpha::one))",
             "module helper through its 1 dependent modules (their unit tests, 1 calling "
             "library test modules and 1 test targets)", 5, 4),
            # A data file the library includes counts as the including module.
            ("crates/alpha/src/data.toml", WIDGET,
             "included by widget (unit tests, 1 calling library test modules and 2 test targets)", 4, 4),
            # A nested module selects only its own unit tests, and `b::inner` names `b`: the
            # whole target whose root names it, only the test module that names it in another.
            ("crates/alpha/src/b/inner.rs",
             "(binary_id(=alpha) and (test(/^b::inner::/) or test(/^b::inner_tests::/))) or "
             "(binary_id(=alpha::four) and (test(/^inner_cases::/) or "
             "test(/^inner_cases_tests::/))) or (binary_id(=alpha::two))",
             "module b::inner (its unit tests, 0 calling library test modules and 2 test targets)", 3, 3),
            # Test files select the targets whose module trees include them, via #[path].
            # A test helper the library also includes through #[path] is a library module.
            ("crates/alpha/tests/shared/mod.rs",
             "(binary_id(=alpha::one)) or "
             "(binary_id(=alpha) and (test(/^shared::/) or test(/^shared_tests::/))) or "
             "(binary_id(=alpha) and test(/^e_tests::/))",
             "test targets alpha::one and library module shared", 4, 3),
            ("crates/alpha/tests/two/helper.rs", "(binary_id(=alpha::two))",
             "test targets alpha::two", 1, 1),
            # A test module file selects only its own tests within the target.
            ("crates/alpha/tests/four/other.rs",
             "(binary_id(=alpha::four) and (test(/^other::/) or test(/^other_tests::/)))",
             "test targets alpha::four", 1, 1),
        ]
        for changed, run_filter, note, selected, remaining in cases:
            with self.subTest(changed=changed):
                values, report = self.select([changed])
                self.assertEqual(values["run_filter"], run_filter)
                self.assertIn(f"  {changed}: {note}", report)
                self.assertIn("Test selection: module", report)
                self.assertEqual((int(values["selected"]), int(values["remaining"])),
                                 (selected, remaining))

    def test_paths_the_mapping_cannot_decide_fall_back_to_the_whole_crate(self) -> None:
        for changed, reason in (
            ("crates/alpha/tests/fixtures/data.json", "not part of any test target's module tree"),
            ("crates/alpha/src/lib.rs", "crate root"),
            ("crates/alpha/src/orphan.rs", "outside the library's module tree"),
            ("crates/alpha/src/notes.md", "not a Rust module or a file the library includes"),
            ("crates/alpha/src/lone.rs", "its selection matches no test"),
            ("crates/alpha/Cargo.toml", "not a library or test module"),
            ("crates/tool/src/cmd.rs", "binary crate: its integration tests drive the binary"),
        ):
            with self.subTest(changed=changed):
                values, report = self.select([changed])
                package = changed.split("/")[1]
                self.assertIn(f"  {changed}: crate-wide {package} ({reason})", report)
                self.assertIn("Test selection: crate-wide", report)
                self.assertEqual(values["run_filter"], "all()")
                # Every listed, non-ignored test of the scope is selected; the slow one skips.
                self.assertEqual((values["selected"], values["remaining"]), ("16", "15"))

    def test_a_partial_fallback_keeps_the_module_terms_and_adds_the_crate(self) -> None:
        values, report = self.select(["crates/alpha/src/a.rs", "crates/tool/src/cmd.rs"])
        self.assertEqual(values["run_filter"],
                         f"{UNIT_A} or {CALLS_A} or (binary_id(=alpha::one)) or (package(=tool))")
        self.assertIn("Test selection: module (crate-wide fallback: tool)", report)
        self.assertIn("Tests: 16 in scope, 7 selected, 9 skipped by selection, 1 slow skipped "
                      "(full tier only), 0 expected failures excluded; 6 to run", report)

    def test_a_declaration_only_crate_root_defers_to_the_other_changed_paths(self) -> None:
        values, report = self.select(["crates/alpha/src/lib.rs", "crates/alpha/src/a.rs"])
        self.assertEqual(values["run_filter"],
                         f"{UNIT_A} or {CALLS_A} or (binary_id(=alpha::one))")
        self.assertIn("  crates/alpha/src/lib.rs: crate root declarations only (covered by the "
                      "other changed paths)", report)
        _ = (self.repo / "crates/alpha/src/lib.rs").write_text(FILES["crates/alpha/src/lib.rs"]
                                                             + "pub fn run() {}\n")
        values, report = self.select(["crates/alpha/src/lib.rs", "crates/alpha/src/a.rs"])
        self.assertIn("  crates/alpha/src/lib.rs: crate-wide alpha (crate root)", report)
        self.assertIn("Test selection: crate-wide", report)

    def test_expected_failures_subtract_only_selected_tests_that_would_run(self) -> None:
        expected = ["alpha a::tests::x", "alpha::one slow_one", "alpha::two two_t"]
        values, report = self.select(["crates/alpha/src/a.rs"], expected=expected)
        self.assertEqual(values["run_filter"],
                         f"({UNIT_A} or {CALLS_A} or (binary_id(=alpha::one)))"
                         " - (binary_id(=alpha) and test(=a::tests::x))")
        self.assertEqual(values["remaining"], "3")
        for unmatched in expected[1:]:
            self.assertIn("Expected failure matches no selected test and excludes nothing: "
                          + unmatched, report)

    def test_declared_tests_replace_the_module_selection(self) -> None:
        statuses = {key: {name: "matches" if name == "one_t" else "expression"
                          for name in cases} for key, cases in SUITES.items()}
        values, report = self.select(["crates/alpha/src/a.rs"], tests=["test(=one_t)"],
                                     statuses=statuses)
        self.assertEqual(values["run_filter"], "(test(=one_t))")
        self.assertIn("Test selection: declared tests", report)
        self.assertEqual((values["selected"], values["remaining"]), ("1", "1"))


if __name__ == "__main__":
    unittest.main()
