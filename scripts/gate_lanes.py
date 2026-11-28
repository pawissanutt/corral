#!/usr/bin/env python3
"""Select full-gate optional lanes. Missing context is conservative (plan/legacy/manual).

The slow profile is the explicit torture catalog; full landings exclude it except when
an owning module changes. PI_SLOW is the same for Bun tests. Plan verification always runs
every lane and every test.
"""
from __future__ import annotations

import json
from pathlib import Path
import re
import shlex
import subprocess
import sys

PI_CONTRACTS = (
    "packages/corral-pi/", "scripts/pi-lane.sh", "scripts/build-pi-host.sh",
    "crates/corral-daemon/", "crates/corral-protocol/", "crates/corral-types/",
)
# Bun slow catalog: (test file under packages/corral-pi, test name, reason). The Pi lane skips
# these unless CORRAL_PI_SLOW=1: quick gates never run them, full landings restore them when an
# owner path changes, and plan verify and the nightly slow lane (scripts/slow_lane.py) always do.
PI_SLOW = (
    ("test/browser.test.ts", "a cancelled call stops its browser and removes its profile",
     "T-768: load-sensitive live browser cancellation"),
    ("src/extensions/lsp/lsp.test.ts", "LSP close joins concurrent preparation and retains positive not started evidence",
     "T-932: preparation/close race with joined startup flights"),
)
# Owners include the test support files that hold its synchronization and cleanup checks.
PI_SLOW_OWNERS = (
    "packages/corral-pi/src/extensions/browser/", "packages/corral-pi/test/browser.test.ts",
    "packages/corral-pi/test/support/exits.ts", "packages/corral-pi/test/support/wait.ts",
    "packages/corral-pi/src/extensions/lsp/", "packages/corral-pi/src/host/session.ts",
    "packages/corral-pi/src/host/managed-effects.ts",
)


def pi_touched(paths: list[str]) -> bool:
    return any(path.startswith(PI_CONTRACTS) for path in paths)


def pi_slow(paths: list[str]) -> bool:
    return any(path.startswith(PI_SLOW_OWNERS) for path in paths)


def workspace_input(path: str) -> bool:
    return (Path(path).name in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain"}
            or path.startswith(".cargo/"))


def darwin_touched(context: dict) -> bool:
    changed = context["changed_paths"]
    if any(workspace_input(path) for path in changed):
        return True
    paths = [path for path in changed if Path(path).suffix == ".rs"]
    if any(re.search(r"(?:darwin|macos|platform|build\.rs)", path) for path in paths):
        return True
    if not paths:
        return False
    # Inspect additions AND removals, so deleting a cfg branch cannot evade the lane.
    # Compare against merge-base, as the context's changed_paths does, never HEAD^.
    result = subprocess.run(
        ["git", "diff", "--unified=0", f"{context['integration_base']}...HEAD", "--", *paths],
        stdin=subprocess.DEVNULL, capture_output=True, text=True, check=False)
    if result.returncode:
        return True
    if any(Path(path).is_file() and Path(path).suffix == ".rs"
           and re.search(r"target_(?:os|arch|family|env)|#\[cfg\((?:unix|windows)", Path(path).read_text())
           for path in paths):
        return True
    return any(re.search(r"target_(?:os|arch|family|env)|\b(?:unix|windows)\b|libc::|std::os::", line)
               for line in result.stdout.splitlines() if line.startswith(("+", "-")))


def landing_filter(_paths: list[str]) -> str:
    # The Classic Rust catalog has retired. Do not omit workspace tests: new
    # slow entries run on landings until explicit owner scoping is introduced.
    return "all()"


def plan(path: str | None) -> None:
    context = json.loads(Path(path).read_text()) if path else None
    all_lanes = context is None or context.get("plan_verify", True)
    if context is not None and context.get("schema_version") != 1:
        raise ValueError("unsupported gate context schema")
    paths = context["changed_paths"] if context else []
    values = {
        "pi_touched": int(all_lanes or pi_touched(paths)),
        "pi_slow": int(all_lanes or pi_slow(paths)),
        "darwin_touched": int(all_lanes or darwin_touched(context)),
        "landing_filter": "all()" if all_lanes else landing_filter(paths),
    }
    for key, value in values.items():
        print(f"{key}={shlex.quote(str(value))}")


if __name__ == "__main__":
    plan(sys.argv[1] if len(sys.argv) > 1 else None)
