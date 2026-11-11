#!/usr/bin/env python3
"""Run the selected nextest suite, then every quarantined test in one run, without masking errors."""
import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path
import xml.etree.ElementTree as ET

import gate_case


def escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace(")", "\\)").replace(",", "\\,")


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == 'scope-list':
        path = Path(sys.argv[2])
        listing = json.loads(path.read_text())
        packages = set(sys.argv[3:])
        listing['rust-suites'] = {key: suite for key, suite in listing['rust-suites'].items()
                                  if suite['package-name'] in packages}
        path.write_text(json.dumps(listing))
        return 0
    entries = tomllib.loads(Path('.config/gate-quarantine.toml').read_text())['tests']
    identities = [(entry['binary'], entry['test']) for entry in entries]
    if len(set(identities)) != len(identities):
        raise ValueError('duplicate quarantined test identity')
    filters = [f"(binary_id(={escape(binary)}) and test(={escape(test)}))"
               for binary, test in identities]
    # The caller supplies the ordinary selection as one -E expression. Combining rather
    # than replacing it preserves module/slow/expected-failure selection.
    selection, *command = sys.argv[1:]
    profile = command[command.index('--profile') + 1] if '--profile' in command else 'default'
    ordinary = f"({selection})"
    if filters:
        ordinary += " and not (" + " or ".join(filters) + ")"
    empty_ok = ['--no-tests', 'pass'] if (selection == 'none()' or
                os.environ.get('CORRAL_ALLOW_EMPTY_ORDINARY') == '1') else []
    result, _ = gate_case.observed(profile, [*command, *empty_ok, '-E', ordinary])
    failed = result != 0
    # The quarantine run replaces nextest's JUnit file. Report ordinary failures
    # first so repair hints describe the failure that actually made the gate fail.
    if failed and (repair := os.environ.get('CORRAL_GATE_REPAIR_PROFILE')):
        subprocess.run([sys.executable, 'scripts/gate_repair.py', 'report', repair], check=False)
    if not filters:
        return int(failed)
    # One run at the profile's parallelism: a failure here never stops the other quarantined
    # tests, and each verdict comes from this run's own report. A declared quick selection
    # already ignores the default filter, and nextest rejects a repeated flag.
    flags = [flag for flag in ('--ignore-default-filter', '--no-fail-fast') if flag not in command]
    code, report = gate_case.observed(profile, [*command, *flags, '-E', " or ".join(filters)])
    passed: dict[tuple[str, str], bool] = {}
    if code in (0, 100):  # 100 is nextest's test-run-failed exit, not setup/selection errors
        try:
            passed = {tuple(case['test']): case['passed'] for case in gate_case.cases(report)}
        except (OSError, ValueError, ET.ParseError) as error:
            print(f"Quarantine report unavailable: {error}", file=sys.stderr, flush=True)
    for entry, identity in zip(entries, identities, strict=True):
        label = f"{entry['binary']} {entry['test']}"
        outcome = passed.get(identity)
        if outcome is None:  # the run did not complete, or this identity never ran
            print(f"QUARANTINED ERROR {label} (exit: {code})", flush=True)
            failed = True
        elif outcome:
            print(f"QUARANTINED PASS {label} (fix: {entry['fix']})", flush=True)
        else:
            print(f"QUARANTINED FAIL {label} (fix: {entry['fix']})", flush=True)
    return int(failed)


if __name__ == '__main__':
    sys.exit(main())
