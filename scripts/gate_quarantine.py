#!/usr/bin/env python3
"""Run the selected nextest suite and every quarantined test, without masking errors."""
import subprocess
import sys
import tomllib
from pathlib import Path


def escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace(")", "\\)").replace(",", "\\,")


def main() -> int:
    entries = tomllib.loads(Path('.config/gate-quarantine.toml').read_text())['tests']
    identities = [(entry['binary'], entry['test']) for entry in entries]
    if len(set(identities)) != len(identities):
        raise ValueError('duplicate quarantined test identity')
    filters = [f"(binary_id(={escape(binary)}) and test(={escape(test)}))"
               for binary, test in identities]
    # The caller supplies the ordinary selection as one -E expression. Combining rather
    # than replacing it preserves module/slow/expected-failure selection.
    selection, *command = sys.argv[1:]
    ordinary = f"({selection})"
    if filters:
        ordinary += " and not (" + " or ".join(filters) + ")"
    empty_ok = ['--no-tests', 'pass'] if selection == 'none()' else []
    result = subprocess.run([*command, *empty_ok, '-E', ordinary], check=False).returncode
    failed = result != 0
    for entry, expression in zip(entries, filters, strict=True):
        code = subprocess.run([*command, '--ignore-default-filter', '-E', expression],
                              check=False).returncode
        identity = f"{entry['binary']} {entry['test']}"
        if code == 0:
            print(f"QUARANTINED PASS {identity} (fix: {entry['fix']})", flush=True)
        elif code == 100:  # nextest's test-run-failed exit, not setup/selection errors
            print(f"QUARANTINED FAIL {identity} (fix: {entry['fix']})", flush=True)
        else:
            print(f"QUARANTINED ERROR {identity} (exit: {code})", flush=True)
            failed = True
    return int(failed)


if __name__ == '__main__':
    sys.exit(main())
