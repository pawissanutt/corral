#!/usr/bin/env python3
"""Host-only nightly slow lane. PM installs the documented cron line after landing.

Not a landing/review gate. Never installs a scheduler, retries a test, or creates load.
Corral writes happen outside the disposable test HOME/XDG and only after the suite exits.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

from gate_lanes import PI_SLOW

REPO = Path(__file__).resolve().parent.parent
# Mirrored from SystemResourceProbe: load1 / available cores, threshold 1.0.
MAX_LOAD_PER_CORE = 1.0


def available_cores() -> int:
    cores = len(os.sched_getaffinity(0))
    # Respect cgroup-v2 quotas too. Rounding down is conservative for admission.
    group = next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines()
                 if line.startswith('0::'))
    mount = Path('/sys/fs/cgroup')
    current = (mount / group.lstrip('/')).resolve()
    if not current.is_relative_to(mount):
        raise ValueError('unresolvable cgroup path; refusing load admission')
    for directory in (current, *current.parents):
        if not directory.is_relative_to(mount):
            break
        quota = directory / 'cpu.max'
        if quota.is_file():
            limit, period = quota.read_text().split()
            if limit != 'max':
                cores = min(cores, max(1, int(limit) // int(period)))
    if cores < 1:
        raise ValueError('no available cores')
    return cores


def load_admitted(load: float, cores: int) -> bool:
    return cores > 0 and math.isfinite(load) and 0 <= load / cores <= MAX_LOAD_PER_CORE


def command(args: list[str]) -> str:
    return subprocess.run(args, stdin=subprocess.DEVNULL, capture_output=True,
                          text=True, check=True, timeout=120).stdout


def idle_priority() -> None:
    os.setpriority(os.PRIO_PROCESS, 0, 19)
    command(['ionice', '-c', '3', '-p', str(os.getpid())])


def failures(text: str) -> list[str]:
    text = re.sub(r'\x1b\[[0-9;]*m', '', text)
    # Nextest and Bun repeat failed rows in their final summaries; identify each test only once.
    rust = re.findall(
        r'^\s*(?:FAIL|TIMEOUT|ABORT|LEAK)\s+\[[^\]]+\]\s+(?:\(\s*\d+/\d+\)\s+)?(\S+ \S+)\s*$',
        text, re.M)
    bun = re.findall(r'^\(fail\) (.+?)(?: \[[\d.]+m?s\])?\s*$', text, re.M)
    return sorted({*rust, *(f'corral-pi {name}' for name in bun)})


def pi_slow_command() -> str:
    """Runs every PI_SLOW test (Bun) with CORRAL_PI_SLOW=1; a failing entry fails the whole command."""
    runs = ' '.join(f'CORRAL_PI_SLOW=1 bun test {shlex.quote(path)} -t {shlex.quote("^" + re.escape(name) + "$")} || rc=1;'
                    for path, name, _reason in PI_SLOW)
    return f'(cd packages/corral-pi && bun install --frozen-lockfile && {{ rc=0; {runs} exit $rc; }})'


class Board:
    def __init__(self, binary: str, root: str, project: str):
        self.prefix = [binary, '--root', root]
        self.project = project

    def call(self, *args: str) -> str:
        return command([*self.prefix, *args])

    def attach(self, task: str, log: Path) -> None:
        output = self.call('artifact', 'add', task, str(log), '--kind', 'log',
                           '--title', f'Nightly slow profile {log.stem}')
        # CLI returns '<run> <kind> <tier> <stored-path>'. Retain raw proof explicitly.
        stored = output.strip().split(maxsplit=3)[3]
        self.call('artifact', 'set-tier', stored, 'keep')

    def file_failures(self, names: list[str], log: Path, sha: str) -> list[str]:
        tasks = json.loads(self.call('list', '--project', self.project, '--json'))
        ids = []
        for name in names:
            marker = 'slow-' + hashlib.sha256(name.encode()).hexdigest()[:16]
            task = next((t['id'] for t in tasks if marker in t['title']), None)
            if task is None:
                body = (f'## Goal\nFix the named slow-lane failure without retries or weakening assertions.\n\n'
                        f'Failure key: {marker}\nTest: `{name}`\nCandidate: `{sha}`\n'
                        f'Raw retained proof: {log}\n\n## Acceptance\n'
                        '- Reproduce this failure in the private test-herdr environment.\n'
                        '- Fix the root cause with the smallest failing-first regression.\n'
                        '- The named test passes; no timeout bump, assertion relaxation or retry-to-green.\n\n'
                        '## Relevant files\n.config/nextest.toml; scripts/slow_lane.py; T-761.\n\n'
                        '## Classification\ntest, s, standard: diagnose one bounded named regression.\n')
                output = self.call('new', f'[{marker}] Slow test failure: {name}', '--origin', 'agent',
                                   '-p', self.project, '--kind', 'test', '--size', 's',
                                   '--difficulty', 'standard', '--body', body)
                match = re.search(r'\bT-\d+\b', output)
                if match is None:
                    raise ValueError(f'new task acknowledgement has no ID: {output!r}')
                task = match.group()
                tasks.append({'id': task, 'title': marker})
            self.attach(task, log)
            ids.append(task)
        return ids


def run(args: argparse.Namespace) -> int:
    os.chdir(REPO)
    idle_priority()  # All subprocesses inherit nice 19 and idle I/O priority.
    load, cores = os.getloadavg()[0], available_cores()
    if not load_admitted(load, cores):
        print(f'DEFER slow lane: load/core {load / cores:.3f} > 1.0; no tests or board writes')
        return 75
    scratch = REPO / '.evidence' / 'slow'
    scratch.mkdir(parents=True, exist_ok=True)
    with (scratch / 'lane.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print('DEFER slow lane: another nightly owns the lock')
            return 75
        if command(['git', 'status', '--porcelain']).strip():
            print('DEFER slow lane: dirty worktree')
            return 75
        sha = command(['git', 'rev-parse', 'HEAD']).strip()
        stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')
        log = scratch / f'{stamp}-{sha[:12]}.log'
        # The Bun slow catalog runs even when nextest fails, so each lane reports its own failures.
        shell = ('source scripts/test-herdr.sh && corral_test_herdr_start && '
                 '{ CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace --profile slow; nextest=$?; '
                 f'{pi_slow_command()}; pi=$?; exit $((nextest || pi)); }}')
        with log.open('w') as output:
            output.write(f'candidate={sha} load1={load} cores={cores} nice=19 ionice=idle\n')
            output.flush()
            code = subprocess.run([sys.executable, 'scripts/gate_timer.py', 'slow', '-c', shell],
                                  stdin=subprocess.DEVNULL, stdout=output,
                                  stderr=subprocess.STDOUT, check=False).returncode
        board = Board(args.corral, args.root, args.project)
        # Attachment failures propagate, leaving the local log for reconciliation. Never retry
        # an uncertain mutation automatically; the next scheduled run searches open tasks first.
        names = failures(log.read_text()) if code else []
        if code and not names:
            names = ['slow-lane infrastructure']
        ids = board.file_failures(names, log, sha) if names else []
        board.attach(args.report_task, log)
        print(json.dumps({'candidate': sha, 'exit_code': code, 'log': str(log), 'tasks': ids}))
        return code


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, help='Canonical Corral workspace, outside the private fixture')
    parser.add_argument('--report-task', required=True, help='Monitoring task that retains every nightly log')
    parser.add_argument('--project', default='corral')
    parser.add_argument('--corral', default='corral')
    try:
        sys.exit(run(parser.parse_args()))
    except (OSError, ValueError, StopIteration, subprocess.SubprocessError) as error:
        print(f'FAILED slow lane: {error}', file=sys.stderr)
        sys.exit(1)
