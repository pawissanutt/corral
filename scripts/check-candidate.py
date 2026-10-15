#!/usr/bin/env python3
"""Read-only, stdlib-only candidate gate. See docs/guide/candidate-guards.md."""

import argparse
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import re
import subprocess
from typing import Final, TypedDict


POLICY: Final = "corral-candidate-v3"
MARKER: Final = re.compile(r"^(?:<{7,}(?: .*)?|={7,}|>{7,}(?: .*)?|\|{7,}(?: .*)?)$")
RAW: Final = re.compile(r'(?:br|cr|r)(#{0,255})"')
CHAR: Final = re.compile(r"'(?:[^'\\\n]|\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|[^\n]))'")
OPERATIONS: Final = (
    "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge",
    "rebase-apply", "sequencer", "BISECT_LOG",
)


@dataclass(frozen=True)
class Violation:
    reason: str
    path: str | None = None
    line: int | None = None
    detail: str | None = None


class RustFile(TypedDict):
    path: str
    policy_paths: list[str]
    sloc: int
    limit: int


class Report(TypedDict):
    schema_version: int
    policy: str
    base: str | None
    head: str | None
    counts: dict[str, int]
    rust_files: list[RustFile]
    source_files: list[RustFile]
    violations: list[dict[str, str | int | None]]
    ok: bool


def git(*args: str) -> bytes:
    """Run only read-only Git argv, with no stdin or optional index writes."""
    return subprocess.run(
        ["git", "--no-optional-locks", "-c", "core.fsmonitor=false", *args],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        check=True, env={**os.environ, "GIT_NO_REPLACE_OBJECTS": "1"},
    ).stdout


def rust_sloc(text: str) -> int:
    """Count lines with nonblank code/string content, preserving lexical state."""
    lines: set[int] = set()
    line = 1
    pos = 0
    depth = 0
    quoted = False
    raw_end = ""
    while pos < len(text):
        char = text[pos]
        if char == "\n":
            line += 1
            pos += 1
            continue
        if depth:
            if text.startswith("/*", pos):
                depth += 1
                pos += 2
            elif text.startswith("*/", pos):
                depth -= 1
                pos += 2
            else:
                pos += 1
            continue
        if raw_end:
            if not char.isspace():
                lines.add(line)
            if text.startswith(raw_end, pos):
                pos += len(raw_end)
                raw_end = ""
            else:
                pos += 1
            continue
        if quoted:
            if not char.isspace():
                lines.add(line)
            if char == "\\" and pos + 1 < len(text):
                if text[pos + 1] == "\n":
                    line += 1
                pos += 2
            else:
                if char == '"':
                    quoted = False
                pos += 1
            continue
        if text.startswith("//", pos):
            end = text.find("\n", pos)
            pos = len(text) if end < 0 else end
            continue
        if text.startswith("/*", pos):
            depth = 1
            pos += 2
            continue
        if not char.isspace():
            lines.add(line)
        raw = RAW.match(text, pos)
        literal = CHAR.match(text, pos) if char == "'" else None
        if raw:
            raw_end = '"' + raw[1]
            pos = raw.end()
        elif literal:
            pos = literal.end()
        else:
            quoted = char == '"'
            pos += 1
    return len(lines)


def source_sloc(text: str, extension: str) -> int:
    """Keep Rust's lexical counter and the integration packet's other-source convention."""
    if extension == "rs":
        return rust_sloc(text)
    return sum(bool(line.strip()) and not line.lstrip().startswith("//")
               for line in text.splitlines())


def inspect(base: str | None, size_limits: dict[str, int] | None = None) -> Report:
    """Inspect HEAD and aggregate violations without touching candidate files."""
    limits = {"rs": 800} if size_limits is None else size_limits
    violations: list[Violation] = []
    report: Report = {
        "schema_version": 1, "policy": POLICY, "base": None, "head": None,
        "counts": {
            "changed_entries": 0, "policy_paths": 0, "deleted_entries": 0,
            "text_files": 0, "binary_files": 0, "rust_files": 0,
            "dirty_paths": 0, "violations": 0, "source_files": 0,
        },
        "rust_files": [], "source_files": [], "violations": [], "ok": False,
    }
    counts = report["counts"]
    try:
        # Enter the top level even when invoked from a repository subdirectory.
        root = os.fsdecode(git("rev-parse", "--show-toplevel").rstrip(b"\n"))
        os.chdir(root)
        head = git("rev-parse", "--verify", "HEAD^{commit}").decode().strip()
        report["head"] = head
        status = git("status", "--porcelain=v1", "-z", "--untracked-files=all",
                     "--ignore-submodules=none").split(b"\0")
        index = 0
        dirty: set[str] = set()
        while index < len(status) and status[index]:
            record = status[index]
            dirty.add(os.fsdecode(record[3:]))
            index += 1
            if b"R" in record[:2] or b"C" in record[:2]:
                dirty.add(os.fsdecode(status[index]))
                index += 1
        counts["dirty_paths"] = len(dirty)
        violations.extend(Violation("DIRTY_CANDIDATE", path) for path in sorted(dirty))
        for operation in OPERATIONS:
            location = os.fsdecode(git("rev-parse", "--git-path", operation).rstrip(b"\n"))
            if os.path.lexists(location):
                violations.append(Violation("GIT_OPERATION_IN_PROGRESS", detail=operation))
        if not base:
            violations.append(Violation("BASE_REQUIRED"))
        else:
            try:
                base_sha = git("rev-parse", "--verify", "--end-of-options",
                               base + "^{commit}").decode().strip()
            except subprocess.CalledProcessError:
                violations.append(Violation("BASE_INVALID", detail=base))
            else:
                report["base"] = base_sha
                changes = git("diff", "--name-status", "-z", "--find-renames",
                              "--no-ext-diff", "--no-textconv", base_sha,
                              head, "--").split(b"\0")
                tree = {}
                for entry in git("ls-tree", "-rz", "--full-tree", head).split(b"\0"):
                    if entry:
                        metadata, name = entry.split(b"\t", 1)
                        mode, kind, oid = metadata.split()
                        tree[os.fsdecode(name)] = (mode, kind, oid)
                policy_paths: set[str] = set()
                index = 0
                while index < len(changes) and changes[index]:
                    status_code = changes[index]
                    path = os.fsdecode(changes[index + 1])
                    paths = [path]
                    index += 2
                    if status_code.startswith((b"R", b"C")):
                        path = os.fsdecode(changes[index])
                        paths.append(path)
                        index += 1
                    policy_paths.update(paths)
                    counts["changed_entries"] += 1
                    if status_code == b"D":
                        counts["deleted_entries"] += 1
                        continue
                    mode, kind, oid = tree[path]
                    if mode not in (b"100644", b"100755") or kind != b"blob":
                        violations.append(Violation("UNSUPPORTED_FILE_MODE", path,
                                                    detail=os.fsdecode(mode)))
                        continue
                    data = git("cat-file", "blob", oid.decode("ascii"))
                    is_rust = any(name.endswith(".rs") for name in paths)
                    extensions = {Path(name).suffix.removeprefix(".") for name in paths}
                    applicable = sorted(extensions & limits.keys())
                    if b"\0" in data:
                        counts["binary_files"] += 1
                        if is_rust:
                            violations.append(Violation("RUST_BINARY_CONTENT", path))
                        elif applicable:
                            violations.append(Violation("SOURCE_BINARY_CONTENT", path))
                        continue
                    counts["text_files"] += 1
                    text = data.decode("utf-8", errors="surrogateescape")
                    for number, content in enumerate(text.splitlines(), 1):
                        if MARKER.fullmatch(content):
                            violations.append(Violation("CONFLICT_MARKER", path, number))
                    if is_rust and "rs" in limits:
                        counts["rust_files"] += 1
                        sloc = rust_sloc(text)
                        report["rust_files"].append({"path": path, "policy_paths": paths,
                                                     "sloc": sloc, "limit": limits["rs"]})
                    for extension in applicable:
                        sloc = source_sloc(text, extension)
                        limit = limits[extension]
                        counts["source_files"] += 1
                        report["source_files"].append({"path": path, "policy_paths": paths,
                                                       "sloc": sloc, "limit": limit})
                        if sloc > limit:
                            reason = "RUST_SLOC_EXCEEDED" if extension == "rs" else "SOURCE_SLOC_EXCEEDED"
                            violations.append(Violation(reason, path, detail=f"{sloc} > {limit}"))
                counts["policy_paths"] = len(policy_paths)
    except (subprocess.CalledProcessError, OSError) as error:
        detail = str(error)
        if isinstance(error, subprocess.CalledProcessError):
            detail = error.stderr.decode("utf-8", errors="replace").strip()
        violations.append(Violation("GIT_READ_FAILED", detail=detail))
    counts["violations"] = len(violations)
    report["violations"] = [asdict(item) for item in violations]
    report["ok"] = not violations
    return report


def check_worktree_sloc(size_limits: dict[str, int]) -> int:
    """Apply configured ceilings to every nonignored working-tree source file."""
    root = os.fsdecode(git("rev-parse", "--show-toplevel").rstrip(b"\n"))
    os.chdir(root)
    paths = git("ls-files", "-z", "--cached", "--others", "--exclude-standard",
                "--deduplicate").split(b"\0")
    failed = False
    counted = 0
    maximum = 0
    for name in filter(None, paths):
        path = Path(os.fsdecode(name))
        extension = path.suffix.removeprefix(".")
        if extension not in size_limits:
            continue
        limit = size_limits[extension]
        # Deleted tracked paths have no current source to count.
        if not path.exists() and not path.is_symlink():
            continue
        if path.is_symlink() or not path.is_file():
            print(f"SLOC ceiling: {path} is not a regular source file")
            failed = True
            continue
        sloc = source_sloc(path.read_text(encoding="utf-8", errors="surrogateescape"), extension)
        counted += 1
        maximum = max(maximum, sloc)
        if sloc > limit:
            print(f"SLOC ceiling: {path} has {sloc} code lines (max {limit})")
            failed = True
    limit_note = f" (limit {next(iter(size_limits.values()))})" if len(size_limits) == 1 else ""
    print(f"SLOC: checked {counted} source files; maximum {maximum}{limit_note}; limits {size_limits}")
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", help="integration commit/ref; overrides CORRAL_INTEGRATION_BASE")
    parser.add_argument("--size-limits", default=os.environ.get("CORRAL_SIZE_LIMITS", '{"rs":800}'),
                        help="JSON extension-to-ceiling map; overrides CORRAL_SIZE_LIMITS")
    parser.add_argument("--worktree-sloc", action="store_true",
                        help="check all nonignored configured source files, without candidate admission")
    args = parser.parse_args()
    try:
        size_limits = json.loads(args.size_limits)
        if not isinstance(size_limits, dict) or any(
            not isinstance(extension, str) or not re.fullmatch(r"[a-zA-Z0-9]+", extension)
            or type(limit) is not int or limit <= 0
            for extension, limit in size_limits.items()
        ):
            parser.error("size limits must map extensions without dots to positive integers")
    except json.JSONDecodeError as error:
        parser.error(f"invalid size limits JSON: {error}")
    if args.worktree_sloc:
        return check_worktree_sloc(size_limits)
    report = inspect(args.base if args.base is not None else os.environ.get("CORRAL_INTEGRATION_BASE"),
                     size_limits)
    print(json.dumps(report, sort_keys=True, ensure_ascii=True))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
