"""Fake test runner for the gate_tiers_e2e fixtures.

Each file suite/<binary>/<test> in the gated checkout is one test whose content is its outcome
("pass" or anything else). The runner selects tests the way a tier command does: from the gate
context (schema v1) that corral exports as CORRAL_GATE_CONTEXT, the declared tests (binary IDs)
replace the default "every test" selection and each expected failure "<binary> <test>" is
excluded by exact name. The tier command, not corral, enforces the expected-failure subset.

Usage: suite.py <tier-label> <log>. One line per test is appended to <log>:
"<label> <attempt> <binary> <test> <pass|fail|excluded|unselected>".
"""

import json
import os
import sys

label, log = sys.argv[1], sys.argv[2]
tier = os.environ.get("CORRAL_GATE_TIER", label)
if tier != label:
    sys.exit(f"FAILED: {label} command ran as tier {tier}")
context = {}
if os.environ.get("CORRAL_GATE_CONTEXT"):
    with open(os.environ["CORRAL_GATE_CONTEXT"], encoding="utf-8") as file:
        context = json.load(file)
attempt = context.get("attempt", "-")
declared = context.get("tests") or []
expected = {failure["test"] for failure in context.get("expected_failures", [])}

lines, failed = [], []
for binary in sorted(os.listdir("suite")):
    for test in sorted(os.listdir(os.path.join("suite", binary))):
        name = f"{binary} {test}"
        if declared and binary not in declared:
            status = "unselected"
        elif name in expected:
            status = "excluded"
        else:
            with open(os.path.join("suite", binary, test), encoding="utf-8") as file:
                status = "pass" if file.read().strip() == "pass" else "fail"
            if status == "fail":
                failed.append(name)
        lines.append(f"{label} {attempt} {name} {status}")

with open(log, "a", encoding="utf-8") as file:
    file.write("".join(line + "\n" for line in lines))
print("\n".join(lines))
if failed:
    sys.exit("FAILED: " + ", ".join(failed))
