#!/usr/bin/env bash
# Check README.md, docs/guide/*.md and the role skill bodies (crates/corral-ops/assets/skills/*.md) against the built binary: every `corral <subcommand>` and --flag
# written in code exists in `--help`, and every ```toml example passes `corral config validate`.
# Usage: scripts/check-docs.sh [path-to-corral-binary]
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${1:-${CORRAL:-$REPO/target/debug/corral}}
[ -x "$CORRAL" ] || { echo "corral binary not found: $CORRAL (cargo build -p corral)" >&2; exit 1; }
cd "$REPO"
exec python3 -I - "$CORRAL" README.md docs/guide/*.md crates/corral-ops/assets/skills/*.md <<'PY'
import os
import re
import shutil
import subprocess
import sys
import tempfile

corral, files = sys.argv[1], sys.argv[2:]
errors = []


def help_text(*words):
    r = subprocess.run([corral, *words, "--help"], capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return r.stdout if r.returncode == 0 else None


def subcommands(text):
    out, on = [], False
    for line in text.splitlines():
        if line.startswith("Commands:"):
            on = True
        elif on and line and not line.startswith(" "):
            on = False
        elif on and line.startswith("  ") and line[2:3].isalpha():
            name = line.split()[0]
            if name != "help":
                out.append(name)
    return out


top_help = help_text()
top = {c: help_text(c) for c in subcommands(top_help)}
nested = {c: set(subcommands(h)) for c, h in top.items()}
all_flags = set(re.findall(r"--[a-z][a-z0-9-]*", top_help))
flags_of = {}
for c, h in top.items():
    text = h
    for s in nested[c]:
        text += help_text(c, s) or ""
    flags_of[c] = set(re.findall(r"--[a-z][a-z0-9-]*", text)) | {"--root", "--help"}
    all_flags |= flags_of[c]
all_flags |= {"--version", "--help"}

CODE = re.compile(r"```[^\n]*\n(.*?)```|`([^`\n]+)`", re.S)
CMD = re.compile(r"(?<![\w./-])corral((?: +[A-Za-z][\w-]*)?)((?: +[^\s`|;&]+)*)")


def check_code(path, code):
    for line in code.splitlines():
        line = re.sub(r"\"[^\"]*\"|'[^']*'", "", line)
        for m in CMD.finditer(line):
            first = m.group(1).strip()
            rest = m.group(2).split()
            if not first:
                continue
            if first not in top:
                if first.islower() and "." not in first:
                    errors.append(f"{path}: unknown subcommand `corral {first}`")
                continue
            if nested[first] and rest and re.fullmatch(r"[a-z][a-z-]*", rest[0]) and rest[0] not in nested[first]:
                errors.append(f"{path}: unknown subcommand `corral {first} {rest[0]}`")
            for flag in re.findall(r"--[a-z][a-z0-9-]*", " ".join(rest)):
                if flag not in flags_of[first]:
                    errors.append(f"{path}: `corral {first}` has no flag {flag}")


def toml_blocks(text):
    return re.findall(r"```toml\n(.*?)```", text, re.S)


def merge_with_base(base, fragment):
    headers = [h.strip("[] ") for h in re.findall(r"^\[[^\n]+\]\s*$", fragment, re.M)]

    def replaced(header):
        return any(header == h or header.startswith(h + ".") for h in headers)

    kept, drop = [], False
    for chunk in re.split(r"(?m)^(?=\[)", base):
        m = re.match(r"\[([^\n]+)\]", chunk)
        drop = bool(m) and replaced(m.group(1).strip())
        if not drop:
            kept.append(chunk)
    return "".join(kept).rstrip() + "\n\n" + fragment


def validate_toml(path, index, fragment):
    root = tempfile.mkdtemp(prefix="corral-docs-")
    environment = {
        key: value for key, value in os.environ.items()
        if not key.startswith(("CORRAL_", "HERDR_"))
    }
    environment["CORRAL_ROOT"] = root
    environment["CORRAL_HERDR_SESSION"] = "corral-test"
    try:
        for d in ("app", "api", "web"):
            os.makedirs(os.path.join(root, d))
            subprocess.run(["git", "init", "-q", os.path.join(root, d)], env=environment, check=True, stdin=subprocess.DEVNULL)
        subprocess.run([corral, "--root", root, "init"], env=environment, check=True, capture_output=True, stdin=subprocess.DEVNULL)
        cfg = os.path.join(root, ".corral", "corral.toml")
        base = open(cfg).read()
        with open(cfg, "w") as f:
            f.write(merge_with_base(base, fragment))
        r = subprocess.run([corral, "--root", root, "config", "validate"], env=environment, capture_output=True, text=True, stdin=subprocess.DEVNULL)
        if r.returncode != 0:
            errors.append(f"{path}: toml block {index} is invalid:\n{(r.stdout + r.stderr).strip()}")
    finally:
        shutil.rmtree(root, ignore_errors=True)


OTHER_TOOLS = {"--thinking", "--session"}
checked_cmds = checked_toml = 0
for path in files:
    text = open(path).read()
    for m in CODE.finditer(text):
        code = m.group(1) if m.group(1) is not None else m.group(2)
        if m.group(1) is not None and m.group(0).startswith("```toml"):
            continue
        if os.path.basename(path) == "cli.md":
            continue
        check_code(path, code)
        checked_cmds += 1
        if m.group(1) is None and code.startswith("--"):
            for flag in re.findall(r"--[a-z][a-z0-9-]*", code):
                if flag not in all_flags and flag not in OTHER_TOOLS:
                    errors.append(f"{path}: flag {flag} is not in any `corral --help` output")
    for i, block in enumerate(toml_blocks(text), 1):
        validate_toml(path, i, block)
        checked_toml += 1

if errors:
    print("\n".join(dict.fromkeys(errors)), file=sys.stderr)
    sys.exit(1)
print(f"docs ok: {checked_cmds} code spans/blocks, {checked_toml} toml examples, {len(files)} files")
PY
