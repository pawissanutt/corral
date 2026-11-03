#!/usr/bin/env python3
"""Candidate-bound nextest time observer; thresholds never change test results (T-776)."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tomllib


def exception_filter(config: dict) -> str:
    overrides = config.get('profile', {}).get('default', {}).get('overrides', [])
    filters = [item['filter'] for item in overrides if isinstance(item.get('slow-timeout'), str)]
    return ' or '.join(f'({value})' for value in filters) or 'not all()'


def prepare(path: Path, cargo: Path, binaries: Path) -> tuple[str, str]:
    """Let nextest interpret its own exception DSL; never duplicate its filter parser."""
    config = tomllib.loads(Path('.config/nextest.toml').read_text())
    result = subprocess.run([
        'cargo', 'nextest', 'list', '--cargo-metadata', str(cargo), '--binaries-metadata', str(binaries),
        '--profile', 'default', '--ignore-default-filter', '--run-ignored', 'all',
        '--message-format', 'json', '-E', exception_filter(config),
    ], stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True)
    suites = json.loads(result.stdout)['rust-suites']
    exemptions = [[binary, name] for binary, suite in suites.items()
                  for name, case in suite['testcases'].items()
                  if case['filter-match']['status'] == 'matches']
    metadata = json.loads(binaries.read_text())
    host = metadata.get('rust-build-meta', {}).get('platforms', {}).get('host', {}).get('platform', {}).get('triple')
    if not host:  # Legacy metadata and the disposable orchestration fixtures.
        version = subprocess.check_output(['rustc', '-vV'], stdin=subprocess.DEVNULL, text=True)
        host = next(line.split(': ', 1)[1] for line in version.splitlines() if line.startswith('host: '))
    key = 'CARGO_TARGET_' + host.upper().replace('-', '_') + '_RUNNER'
    if metadata.get('rust-binaries'):
        from gate_ratchet import candidate_tests
        enforce, base = candidate_tests(json.loads(cargo.read_text()), metadata, suites)
    else:
        enforce, base = [], 'no-test-binaries'
    path.write_text(json.dumps({
        'enforce': enforce,
        'ratchet_base': base,
        'seconds': tomllib.loads(Path('.config/gate.toml').read_text())['tests']['ordinary_seconds'],
        'exemptions': exemptions,
        'binaries': [item['binary-path'] for item in metadata.get('rust-binaries', {}).values()],
        'previous_runner': shlex.split(os.environ.get(key, '')),
        'metrics': str(path.with_name('case-metrics.jsonl').resolve()),
    }))
    return key, shlex.join([sys.executable, str(Path(__file__).resolve()), str(path.resolve())])


def guarded(command: list[str], seconds: float, identity: list[str], metrics: Path,
            classification: str = 'candidate-observation') -> int:
    import gate_timer
    os.environ['CORRAL_CASE_OBSERVED_LIMIT_SECONDS'] = str(seconds)
    result = gate_timer.execute(command, seconds, respect_load=True, grace=1)
    if result['seconds'] > seconds:
        entry = json.dumps({'test': identity, 'limit': seconds, 'classification': classification, **result}) + '\n'
        fd = os.open(metrics, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        try:
            os.write(fd, entry.encode())
        finally:
            os.close(fd)
    if result['budget_exceeded']:
        print(f'Time observation: per-test threshold {seconds:g}s: {identity}; '
              'report-only (T-776)', file=sys.stderr, flush=True)
    return result['exit_code']


def main() -> int:
    if sys.argv[1] == 'prepare':
        key, runner = prepare(*(Path(value) for value in sys.argv[2:]))
        print(f'export {key}={shlex.quote(runner)}')
        return 0
    config = json.loads(Path(sys.argv[1]).read_text())
    command = config['previous_runner'] + sys.argv[2:]
    identity = [os.environ.get('NEXTEST_BINARY_ID'), os.environ.get('NEXTEST_TEST_NAME')]
    # Listing and foreign nested Cargo commands are not test executions from this candidate.
    if not all(identity) or sys.argv[2] not in config['binaries']:
        os.execvp(command[0], command)
    approved = identity in config['exemptions']
    changed = identity in config['enforce']
    classification = ('candidate-exception' if approved else 'candidate-observation') if changed else 'existing-report'
    return guarded(command, config['seconds'], identity, Path(config['metrics']), classification)


if __name__ == '__main__':
    sys.exit(main())
