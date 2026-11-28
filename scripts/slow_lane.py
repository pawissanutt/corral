#!/usr/bin/env python3
"""Host-only nightly slow lane.

Not a landing/review gate. Never installs a scheduler, retries a test, or creates load.
Logs stay in .evidence/slow; no board commands run.
"""
from __future__ import annotations

from datetime import datetime, timezone
import fcntl
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


def run() -> int:
    os.chdir(REPO)
    idle_priority()  # All subprocesses inherit nice 19 and idle I/O priority.
    load, cores = os.getloadavg()[0], available_cores()
    if not load_admitted(load, cores):
        print(f'DEFER slow lane: load/core {load / cores:.3f} > 1.0; no tests or log writes')
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
                 '{ CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace --profile slow --no-tests=pass; nextest=$?; '
                 f'{pi_slow_command()}; pi=$?; exit $((nextest || pi)); }}')
        with log.open('w') as output:
            output.write(f'candidate={sha} load1={load} cores={cores} nice=19 ionice=idle\n')
            output.flush()
            code = subprocess.run([sys.executable, 'scripts/gate_timer.py', 'slow', '-c', shell],
                                  stdin=subprocess.DEVNULL, stdout=output,
                                  stderr=subprocess.STDOUT, check=False).returncode
        names = failures(log.read_text()) if code else []
        if code and not names:
            names = ['slow-lane infrastructure']
        print(json.dumps({'candidate': sha, 'exit_code': code, 'log': str(log), 'failures': names}))
        return code


if __name__ == '__main__':
    # Reject retired board-reporting flags instead of silently accepting a stale cron job.
    if sys.argv[1:] == ['--help']:
        print(__doc__.strip())
        sys.exit(0)
    if sys.argv[1:]:
        sys.exit('Usage: scripts/slow_lane.py (no arguments; local reports only)')
    try:
        sys.exit(run())
    except (OSError, ValueError, StopIteration, subprocess.SubprocessError) as error:
        print(f'FAILED slow lane: {error}', file=sys.stderr)
        sys.exit(1)
