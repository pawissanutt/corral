#!/usr/bin/env python3
"""The quick gate tier's context logic (scripts/gate.sh --tier quick).

'plan <context>' prints Bash assignments of the touched packages, whether a workspace-wide
input changed, the declared selection and the expected failures.

'select <context> <nextest list JSON> <id>...' reads the listing of the test scope (taken
with the 'quick' nextest profile, so the slow group shows as a default-filter mismatch),
selects tests by touched module, subtracts the expected failures and prints Bash
assignments of the run filter and counts. Its report goes to stderr, which is the gate log.

Module selection, per changed path inside a package with a library target:
- src/<file>.rs in the library's module tree selects that module's unit tests (test names
  under its module path or a sibling `<module>_tests` module) and every integration test target whose sources name the module,
  directly (`crate::module`), through a crate-root re-export, or after a glob import. Within
  a test target only the test modules whose files name it are selected, or the whole target
  when the naming file is its root or a fixture module that holds no tests of its own.
  A data file the library reads with include_str!/include_bytes! counts as the including
  module. A module that selects no test that way is exercised through the library modules that
  use it: their unit tests and the test targets that name them are selected instead.
- tests/<file> that belongs to an integration test target's module tree (including files
  shared through #[path]) selects its test module in those targets, or the whole targets
  when it is their root or a fixture module.
A crate root made only of `mod` and `use` declarations adds nothing when other changed paths
of its crate select tests. Everything else falls back to the whole crate and says why: any
other crate root change, a file outside
any module tree, a non-Rust file, a binary-only crate (its tests drive the binary), or a
path whose selection matches no test.
"""

from __future__ import annotations

import json
import re
import shlex
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

Suite = dict[str, Any]

WORKSPACE_INPUTS = {"Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml",
                    "rustfmt.toml", ".rustfmt.toml", "clippy.toml", ".clippy.toml"}
MOD = re.compile(r"((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")
PATH_ATTR = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]')
PUB_USE = re.compile(r"\bpub\s+use\s+([^;]+);")
WORD = re.compile(r"\w+")
INCLUDE = re.compile(r'include_(?:str|bytes)!\(\s*"([^"]+)"')
DECLARATION = re.compile(r"//[^\n]*|/\*.*?\*/|#!?\[[^\]]*\]|(?:pub(?:\([^)]*\))?\s+)?"
                         r"(?:mod\s+\w+|use\s+[^;]+|extern\s+crate\s+[^;]+);", re.S)


def package(directory: Path) -> str | None:
    manifest = directory / "Cargo.toml"
    if not manifest.is_file():
        return None
    return tomllib.loads(manifest.read_text()).get("package", {}).get("name")


def owning_package(changed: str) -> tuple[str, Path] | None:
    for directory in Path(changed).parents:
        name = package(directory)
        if name is not None:
            return name, directory
    return None


def load_context(path: str) -> dict[str, Any]:
    context = json.loads(Path(path).read_text())
    if context.get("schema_version") != 1:
        sys.exit(f"unsupported gate context schema {context.get('schema_version')!r}")
    return context


def plan(path: str) -> None:
    context = load_context(path)
    packages: list[str] = []
    workspace = False
    for changed in context["changed_paths"]:
        if changed in WORKSPACE_INPUTS or changed.startswith(".cargo/"):
            workspace = True
            continue
        owner = owning_package(changed)
        if owner is not None and owner[0] not in packages:
            packages.append(owner[0])
    tests = context["tests"]
    selection = " or ".join(f"({test})" for test in tests) if tests else "all()"
    print(f"packages=({' '.join(map(shlex.quote, sorted(packages)))})")
    print(f"workspace={int(workspace)}")
    pi = any(changed.startswith("packages/corral-pi/") for changed in context["changed_paths"])
    print(f"pi_touched={int(pi)}")
    print(f"declared={len(tests)}")
    print(f"selection={shlex.quote(selection)}")
    ids = [failure["test"] for failure in context["expected_failures"]]
    print(f"expected=({' '.join(map(shlex.quote, ids))})")


def module_tree(root: Path) -> dict[Path, str]:
    """Every file reachable from a crate root through `mod x;`, with its module path."""
    tree: dict[Path, str] = {}
    pending = [(root.resolve(), "", True)]
    while pending:
        file, module, owns_dir = pending.pop()
        if file in tree or not file.is_file():
            continue
        tree[file] = module
        for attrs, name in MOD.findall(file.read_text(errors="replace")):
            child = f"{module}::{name}" if module else name
            explicit = PATH_ATTR.search(attrs)
            if explicit:
                # rustc resolves #[path] against the declaring file's directory and lets the
                # loaded file own its directory, like mod.rs.
                pending.append(((file.parent / explicit.group(1)).resolve(), child, True))
                continue
            base = file.parent if owns_dir else file.parent / file.stem
            for candidate, owns in ((base / f"{name}.rs", False), (base / name / "mod.rs", True)):
                pending.append((candidate.resolve(), child, owns))
    return tree


def reexports(lib_root: Path, modules: set[str]) -> dict[str, set[str]]:
    """Names the crate root re-exports, mapped to the top-level modules they come from."""
    names: dict[str, set[str]] = {}
    for tree in PUB_USE.findall(lib_root.read_text(errors="replace")):
        words = WORD.findall(tree)
        if words and words[0] in modules:
            for word in words[1:]:
                if word not in {"as", "self", "super", "crate"}:
                    names.setdefault(word, set()).add(words[0])
    return names


def first_names(group: str) -> tuple[list[str], bool]:
    names: list[str] = []
    glob, depth, item = False, 0, ""
    for char in group:
        depth += (char == "{") - (char == "}")
        if depth == 0 or (depth == 1 and char == ","):
            stripped = item.strip()
            glob |= stripped.startswith("*")
            if word := WORD.match(stripped):
                names.append(word.group(0))
            item = ""
            if depth == 0:
                break
        elif depth > 1 or char != "{":
            item += char
    return names, glob


def named_modules(text: str, ident: str, modules: set[str],
                  exported: dict[str, set[str]]) -> set[str]:
    words: list[str] = []
    glob = False
    for match in re.finditer(rf"\b{ident}\s*::\s*", text):
        rest = text[match.end():]
        if rest.startswith("{"):
            names, group_glob = first_names(rest)
            words += names
            glob |= group_glob
        elif rest.startswith("*"):
            glob = True
        elif word := WORD.match(rest):
            words.append(word.group(0))
    if glob:
        words += WORD.findall(text)
    found = {word for word in words if word in modules}
    for word in words:
        found |= exported.get(word, set())
    return found


@dataclass
class Selection:
    terms: list[tuple[str, ...]] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    crate_wide: list[str] = field(default_factory=list)


def escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace(")", "\\)").replace(",", "\\,")


def term_filter(term: tuple[str, ...]) -> str:
    match term:
        case ("package", name):
            return f"package(={escape(name)})"
        case ("binary", binary):
            return f"binary_id(={escape(binary)})"
        case ("module", binary, module):
            return (f"binary_id(={escape(binary)}) and "
                    f"(test(/^{module}::/) or test(/^{module}_tests::/))")
        case _:
            raise ValueError(term)


def term_matches(term: tuple[str, ...], suite: Suite, name: str) -> bool:
    match term:
        case ("package", package_name):
            return suite["package-name"] == package_name
        case ("binary", binary):
            return suite["binary-id"] == binary
        case ("module", binary, module):
            return suite["binary-id"] == binary and name.startswith(
                (f"{module}::", f"{module}_tests::"))
        case _:
            raise ValueError(term)


@dataclass
class Package:
    lib: Suite | None
    lib_tree: dict[Path, str]
    declarations_only_root: bool
    suites: list[Suite]
    dependents: dict[str, set[str]]
    targets: dict[str, dict[Path, tuple[str, set[str]]]]

    def matches(self, terms: list[tuple[str, ...]]) -> bool:
        return any(term_matches(term, suite, test) for term in terms
                   for suite in self.suites for test in suite["testcases"])

    def target_terms(self, binary: str, modules: list[str]) -> list[tuple[str, ...]]:
        """A target's test modules among the given module paths; the whole target when one
        is its root or a fixture module that holds no tests of its own."""
        suite = next(suite for suite in self.suites if suite["binary-id"] == binary)
        if any(not module or not any(test.startswith(module + "::")
                                     for test in suite["testcases"]) for module in modules):
            return [("binary", binary)]
        return [("module", binary, module) for module in sorted(set(modules))]

    def module_terms(self, modules: list[str]) -> tuple[list[tuple[str, ...]], int]:
        assert self.lib is not None
        tops = {module.split("::")[0] for module in modules}
        terms: list[tuple[str, ...]] = [("module", self.lib["binary-id"], module)
                                        for module in modules]
        users = 0
        for binary, files in sorted(self.targets.items()):
            naming = [module for module, used in files.values() if tops & used]
            if naming:
                users += 1
                terms += self.target_terms(binary, naming)
        return terms, users


def analyze(name: str, directory: Path, suites: list[Suite]) -> Package:
    lib = next((suite for suite in suites if suite["kind"] == "lib"), None)
    lib_root = directory / "src/lib.rs"
    lib_tree = module_tree(lib_root) if lib is not None and lib_root.is_file() else {}
    modules = {module for module in lib_tree.values() if module and "::" not in module}
    exported = reexports(lib_root, modules) if lib_tree else {}
    declarations_only = bool(lib_tree) and not DECLARATION.sub(
        "", lib_root.read_text(errors="replace")).strip()
    dependents: dict[str, set[str]] = {}
    for file, module in lib_tree.items():
        if module:
            for used in named_modules(file.read_text(errors="replace"), "crate", modules,
                                      exported):
                dependents.setdefault(used, set()).add(module.split("::")[0])
    ident = name.replace("-", "_")
    targets: dict[str, dict[Path, tuple[str, set[str]]]] = {}
    for suite in suites:
        if suite["kind"] != "test":
            continue
        root = directory / "tests" / f"{suite['binary-name']}.rs"
        tree = module_tree(root) if root.is_file() else {}
        targets[suite["binary-id"]] = {
            file: (module, named_modules(file.read_text(errors="replace"), ident, modules,
                                         exported))
            for file, module in tree.items()}
    return Package(lib, lib_tree, declarations_only, suites, dependents, targets)


def path_terms(changed: str, directory: Path, package: Package) -> tuple[list[tuple[str, ...]], str]:
    """The terms one changed path selects and what they are; no terms means crate-wide,
    with the reason in place of the description."""
    relative = Path(changed).relative_to(directory)
    resolved = Path(changed).resolve()
    if relative.parts[0] == "src":
        if package.lib is None:
            return [], "binary crate: its integration tests drive the binary"
        if relative.suffix != ".rs":
            includers = [module for file, module in package.lib_tree.items()
                         if any((file.parent / found).resolve() == resolved
                                for found in INCLUDE.findall(file.read_text(errors="replace")))]
            if not includers:
                return [], "not a Rust module or a file the library includes"
            terms, users = package.module_terms(includers)
            return terms, (f"included by {', '.join(includers)} (unit tests and {users} "
                           "test targets)")
        module = package.lib_tree.get(resolved)
        if module is None:
            return [], "outside the library's module tree"
        if module == "":
            return [], "crate root"
        terms, users = package.module_terms([module])
        if package.matches(terms):
            return terms, f"module {module} (its unit tests and {users} test targets)"
        # A module nothing tests directly is exercised through the modules that use it.
        top = module.split("::")[0]
        dependents = sorted(package.dependents.get(top, set()) - {top})
        terms, users = package.module_terms([module, *dependents])
        return terms, (f"module {module} through its {len(dependents)} dependent modules "
                       f"(their unit tests and {users} test targets)")
    if relative.parts[0] == "tests":
        owners = sorted(binary for binary, files in package.targets.items() if resolved in files)
        if not owners:
            return [], "not part of any test target's module tree"
        terms = [term for binary in owners
                 for term in package.target_terms(binary, [package.targets[binary][resolved][0]])]
        return terms, f"test targets {', '.join(owners)}"
    return [], "not a library or test module"


def module_selection(changed_paths: list[str], suites: dict[str, Suite]) -> Selection:
    selection = Selection()
    by_package: dict[str, list[Suite]] = {}
    for suite in suites.values():
        by_package.setdefault(suite["package-name"], []).append(suite)
    packages: dict[str, Package] = {}
    roots: list[tuple[str, str]] = []

    def crate_wide(changed: str, name: str, reason: str) -> None:
        selection.notes.append(f"  {changed}: crate-wide {name} ({reason})")
        if name not in selection.crate_wide:
            selection.crate_wide.append(name)

    for changed in changed_paths:
        owner = owning_package(changed)
        if owner is None:
            continue
        name, directory = owner
        if name not in by_package:
            selection.notes.append(f"  {changed}: {name} has no tests")
            continue
        if name not in packages:
            packages[name] = analyze(name, directory, by_package[name])
        package = packages[name]
        terms, description = path_terms(changed, directory, package)
        if not terms and description == "crate root" and package.declarations_only_root:
            roots.append((changed, name))
            continue
        if terms and not package.matches(terms):
            terms, description = [], "its selection matches no test"
        if not terms:
            crate_wide(changed, name, description)
            continue
        selection.notes.append(f"  {changed}: {description}")
        selection.terms += [term for term in terms if term not in selection.terms]
    # A crate root of only `mod` and `use` declarations changes no behaviour by itself:
    # the paths it declares carry the selection, and Clippy compiles it.
    for changed, name in roots:
        if name in selection.crate_wide or any(
                suites[term[1]]["package-name"] == name for term in selection.terms
                if term[0] != "package"):
            selection.notes.append(f"  {changed}: crate root declarations only (covered by "
                                   "the other changed paths)")
        else:
            crate_wide(changed, name, "crate root")
    selection.terms = [term for term in selection.terms
                       if suites[term[1]]["package-name"] not in selection.crate_wide
                       and not (term[0] == "module" and any(
                           other[0] == "module" and other[1] == term[1]
                           and term[2].startswith(other[2] + "::")
                           for other in selection.terms))]
    selection.terms += [("package", name) for name in selection.crate_wide]
    return selection


def select(context_path: str, listing_path: str, expected_ids: list[str]) -> None:
    context = load_context(context_path)
    suites = json.loads(Path(listing_path).read_text())["rust-suites"]
    declared = bool(context["tests"])
    candidates: list[tuple[str, str]] = []
    matched: set[tuple[str, str]] = set()
    slow: set[tuple[str, str]] = set()
    for binary, suite in suites.items():
        for name, case in suite["testcases"].items():
            match = case["filter-match"]
            if match.get("reason") == "ignored":
                continue
            candidates.append((binary, name))
            if match["status"] == "matches":
                matched.add((binary, name))
            elif match.get("reason") == "default-filter":
                slow.add((binary, name))
    workspace = any(changed in WORKSPACE_INPUTS or changed.startswith(".cargo/")
                    for changed in context["changed_paths"])
    if declared:
        selection_filter = " or ".join(f"({test})" for test in context["tests"])
        kind, selected = "declared tests", matched
    elif workspace:
        kind, selection_filter, selected = ("workspace (a workspace-wide input changed)",
                                            None, set(candidates))
    else:
        chosen = module_selection(context["changed_paths"], suites)
        for note in chosen.notes:
            print(note, file=sys.stderr)
        if not chosen.terms or all(term[0] == "package" for term in chosen.terms):
            kind, selection_filter = "crate-wide", None
        else:
            kind = "module"
            if chosen.crate_wide:
                kind += f" (crate-wide fallback: {', '.join(chosen.crate_wide)})"
            selection_filter = " or ".join(f"({term_filter(term)})" for term in chosen.terms)
        selected = {(binary, name) for binary, name in candidates
                    if selection_filter is None
                    or any(term_matches(term, suites[binary], name) for term in chosen.terms)}
    excluded: set[tuple[str, str]] = set()
    terms: list[str] = []
    for test in expected_ids:
        binary, name = test.split(" ", 1)
        if (binary, name) in selected - slow:
            excluded.add((binary, name))
            terms.append(f" - (binary_id(={escape(binary)}) and test(={escape(name)}))")
        else:
            print(f"Expected failure matches no selected test and excludes nothing: {test}",
                  file=sys.stderr)
    remaining = len(selected - slow - excluded)
    slow_skipped = len(selected & slow)
    print(f"Test selection: {kind}", file=sys.stderr)
    print(f"Tests: {len(candidates)} in scope, {len(selected)} selected, "
          f"{len(candidates) - len(selected)} skipped by selection, {slow_skipped} slow "
          f"skipped (full tier only), {len(excluded)} expected failures excluded; "
          f"{remaining} to run", file=sys.stderr)
    base = selection_filter or "all()"
    run_filter = f"({base}){''.join(terms)}" if terms else base
    print(f"run_filter={shlex.quote(run_filter)}")
    print(f"selected={len(selected)}")
    print(f"remaining={remaining}")


if __name__ == "__main__":
    if sys.argv[1] == "plan":
        plan(sys.argv[2])
    else:
        select(sys.argv[2], sys.argv[3], sys.argv[4:])
