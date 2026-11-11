#!/usr/bin/env python3
"""Candidate-bound nextest time observer; thresholds never change test results (T-776).

`prepare` classifies candidate-changed and approved-exception tests once per gate. After each
nextest run, `observed` reads the JUnit report that run wrote and records per-test
observations; no wrapper process runs per test.
"""
from __future__ import annotations

from datetime import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET


def exception_filter(config: dict) -> str:
    overrides = config.get('profile', {}).get('default', {}).get('overrides', [])
    filters = [item['filter'] for item in overrides if isinstance(item.get('slow-timeout'), str)]
    return ' or '.join(f'({value})' for value in filters) or 'not all()'


def prepare(path: Path, cargo: Path, binaries: Path) -> None:
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
    if metadata.get('rust-binaries'):
        from gate_ratchet import candidate_tests
        enforce, base = candidate_tests(json.loads(cargo.read_text()), metadata, suites)
    else:  # Legacy metadata and the disposable orchestration fixtures.
        enforce, base = [], 'no-test-binaries'
    path.write_text(json.dumps({
        'enforce': enforce,
        'ratchet_base': base,
        'seconds': tomllib.loads(Path('.config/gate.toml').read_text())['tests']['ordinary_seconds'],
        'exemptions': exemptions,
        'metrics': str(path.with_name('case-metrics.jsonl').resolve()),
        'samples': str(path.with_name('load-samples.jsonl').resolve()),
    }))


def junit_path(profile: str) -> Path:
    """Where nextest writes this profile's report (its name is set in .config/nextest.toml)."""
    metadata = Path(os.environ.get('CORRAL_GATE_BINARIES_METADATA', '/absent'))
    try:
        target = json.loads(metadata.read_text())['rust-build-meta']['target-directory']
    except (OSError, ValueError, KeyError, TypeError):
        target = os.environ.get('CARGO_TARGET_DIR') or 'target'
    return Path(target) / 'nextest' / profile / 'junit.xml'


def cases(junit: Path) -> list[dict]:
    """Every executed test in one report: identity, seconds, start time and outcome."""
    rows = []
    for suite in ET.parse(junit).getroot().iter('testsuite'):
        for case in suite.iter('testcase'):
            stamp = case.get('timestamp')
            rows.append({
                'test': [suite.get('name', ''), case.get('name', '')],
                'seconds': float(case.get('time', 0)),
                'started': datetime.fromisoformat(stamp).timestamp() if stamp else None,
                'passed': case.find('failure') is None and case.find('error') is None,
            })
    return rows


def load_window(samples: list[dict], started: float | None, seconds: float) -> tuple[dict, dict, bool]:
    """The load samples around one test; an unknown window is busy, never timing proof."""
    if not samples or started is None:
        return {}, {}, True
    first = max((at for at, sample in enumerate(samples) if sample['at'] <= started), default=0)
    last = next((at for at, sample in enumerate(samples) if sample['at'] >= started + seconds),
                len(samples) - 1)
    window = samples[first:max(first, last) + 1]
    return window[0], window[-1], any(not sample['quiet'] for sample in window)


def harvest(config_path: Path, junit: Path) -> None:
    """Append candidate tests and overruns from one run's report to the gate's metrics."""
    if not config_path.is_file() or not junit.is_file():
        return
    config = json.loads(config_path.read_text())
    limit = config['seconds']
    exempt = {tuple(identity) for identity in config['exemptions']}
    changed = {tuple(identity) for identity in config['enforce']}
    log = Path(config['samples'])
    samples = [json.loads(line) for line in log.read_text().splitlines()] if log.is_file() else []
    entries = []
    for case in cases(junit):
        identity = tuple(case['test'])
        if identity in changed:
            classification = 'candidate-exception' if identity in exempt else 'candidate-observation'
        else:
            classification = 'existing-report'
        exceeded = case['seconds'] > limit
        if not exceeded and classification == 'existing-report':
            continue
        before, after, busy = load_window(samples, case['started'], case['seconds'])
        entries.append(json.dumps({
            'test': case['test'], 'limit': limit, 'classification': classification,
            'status': 'passed' if case['passed'] else 'failed', 'seconds': case['seconds'],
            'budget_exceeded': exceeded, 'report_only': True,
            'load_before': before, 'load_after': after, 'busy_exempt': busy}) + '\n')
    if entries:
        fd = os.open(config['metrics'], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        try:
            os.write(fd, ''.join(entries).encode())
        finally:
            os.close(fd)


def observed(profile: str, command: list[str]) -> tuple[int, Path]:
    """Run one nextest command; its report is fresh or absent, and observations never fail it."""
    report = junit_path(profile)
    report.unlink(missing_ok=True)
    code = subprocess.run(command, check=False).returncode
    state = os.environ.get('CORRAL_TEST_BUDGET_STATE')
    if state:
        try:
            harvest(Path(state).with_name('case.json'), report)
        except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
            print(f'Per-test observations unavailable: {error}', file=sys.stderr, flush=True)
    return code, report


def main() -> int:
    if sys.argv[1] == 'prepare':
        prepare(*(Path(value) for value in sys.argv[2:]))
        return 0
    # run <profile> -- <nextest command>
    return observed(sys.argv[2], sys.argv[4:])[0]


if __name__ == '__main__':
    sys.exit(main())
