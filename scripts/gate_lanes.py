#!/usr/bin/env python3
"""Select full-gate optional lanes. Missing context is conservative (plan/legacy/manual).

The slow profile is the explicit torture catalog; full landings exclude it except when
an owning module changes. Plan verification always runs every lane and every test.
"""
from __future__ import annotations

import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tomllib

PI_CONTRACTS = (
    "packages/corral-pi/", "scripts/pi-lane.sh", "scripts/build-pi-host.sh",
    "crates/corral-ops/src/agent/", "crates/corral-core/src/agent/",
    "crates/corral-ops/src/agent.rs", "crates/corral-core/src/agent.rs",
    "crates/corral-ops/assets/", "crates/corral/src/agent_cmd.rs",
    "crates/corral-core/src/config/agent_capabilities", "crates/corral-core/src/inbox/",
    "crates/corral-core/src/inbox.rs", "crates/corral-ops/src/inbox",
)
# Each catalog entry names a behavior, not merely a duration. Changes to these owners
# restore the whole relevant torture suite; budget fixtures alone do not opt them in.
SLOW_OWNERS = {
    "one_winner_per_thousand": ("crates/corral-ops/src/operating",),
    "submission_paths_tests::crashes::": (
        "crates/corral-ops/src/operating", "crates/corral-ops/src/submission",
        "crates/corral-ops/tests/support/submission_path", "crates/corral-core/src/store/",
        "crates/corral-ops/src/dispatch", "crates/corral-ops/src/launch",
        "crates/corral-ops/src/plan", "crates/corral-ops/src/review",
    ),
    "ledger_cleanup_closes": ("crates/corral-ops/src/cleanup", "crates/corral-ops/tests/support/cleanup"),
    "delivery_history::s21_": (
        "crates/corral-ops/src/delivery", "crates/corral-ops/src/inbox",
        "crates/corral-core/src/inbox", "crates/corral-ops/tests/attention_eval",
    ),
    "self_projection_restart": ("crates/corral-ops/src/plan", "crates/corral-ops/tests/plan_ops"),
    "inputs_changed_between_approval": (
        "crates/corral-ops/src/autopilot", "crates/corral-ops/src/gate",
        "crates/corral-ops/src/approve", "crates/corral-ops/tests/autopilot_ops",
    ),
    "interrupted_coding_stages": ("crates/corral-ops/src/operating", "crates/corral-ops/tests/operating_launch"),
    "concrete_planning_paths": ("crates/corral-ops/src/operating", "crates/corral-ops/src/plan", "crates/corral-ops/tests/operating_launch"),
    "concrete_gate_and_profile_reviewer": ("crates/corral-ops/src/operating", "crates/corral-ops/src/gate", "crates/corral-ops/src/review", "crates/corral-ops/tests/operating_launch"),
    "binary_id(=corral-ops::gate_tiers_e2e)": (
        "crates/corral-ops/src/gate", "crates/corral-ops/src/plan",
        "crates/corral-ops/src/autopilot", "crates/corral-ops/tests/gate_tiers_e2e",
    ),
    "retained_branch_cleanup_refuses": (
        "crates/corral-ops/src/cleanup", "crates/corral-ops/src/approve",
        "crates/corral-ops/tests/cleanup_ops",
    ),
}


def pi_touched(paths: list[str]) -> bool:
    return any(path.startswith(PI_CONTRACTS) for path in paths)


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


def landing_filter(paths: list[str]) -> str:
    if any(workspace_input(path) or path.startswith((".config/nextest.toml", "scripts/gate_lanes.py", "crates/corral-core/", "crates/corral-git/", "crates/corral-herdr/", "crates/corral-ops/src/sync", "crates/corral-ops/tests/support/fixture"))
           for path in paths):
        return "all()"
    config = tomllib.loads(Path(".config/nextest.toml").read_text())
    catalog = config["profile"]["slow"]["default-filter"]
    # One OR term per line; fail closed if a new catalog entry has no owner mapping.
    terms = [line.strip().removeprefix("| ") for line in catalog.splitlines() if line.strip()]
    selected = []
    for term in terms:
        owners = [prefix for key, prefixes in SLOW_OWNERS.items() if key in term for prefix in prefixes]
        if not owners or any(path.startswith(tuple(owners)) for path in paths):
            selected.append(term)
    return f"not ({catalog})" + (f" or ({' | '.join(selected)})" if selected else "")


def plan(path: str | None) -> None:
    context = json.loads(Path(path).read_text()) if path else None
    all_lanes = context is None or context.get("plan_verify", True)
    if context is not None and context.get("schema_version") != 1:
        raise ValueError("unsupported gate context schema")
    paths = context["changed_paths"] if context else []
    values = {
        "pi_touched": int(all_lanes or pi_touched(paths)),
        "darwin_touched": int(all_lanes or darwin_touched(context)),
        "landing_filter": "all()" if all_lanes else landing_filter(paths),
    }
    for key, value in values.items():
        print(f"{key}={shlex.quote(str(value))}")


if __name__ == "__main__":
    plan(sys.argv[1] if len(sys.argv) > 1 else None)
