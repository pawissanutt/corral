#!/usr/bin/env python3
"""Measure compilation separately; cap cumulative test runtime only on an unbusy host."""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import tomllib


def load_snapshot() -> dict:
    try:
        from slow_lane import available_cores
        cores = available_cores()
    except (AttributeError, ImportError, OSError, ValueError, StopIteration) as error:
        return {'cores': None, 'load_per_core': None, 'quiet': False,
                'unavailable': str(error)}
    try:
        load = os.getloadavg()[0]
        ratio = load / cores
        return {'load1': load, 'cores': cores, 'load_per_core': ratio,
                'quiet': math.isfinite(ratio) and 0 <= ratio <= 1.0}
    except (AttributeError, OSError):
        return {'cores': cores, 'load_per_core': None, 'quiet': False}


def stop(child: subprocess.Popen) -> None:
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()


def execute(command: list[str], seconds: float | None, respect_load: bool = False,
            output: Path | None = None) -> dict:
    started = time.monotonic()
    before = load_snapshot() if respect_load else {'quiet': True}
    busy = not before['quiet']
    stream = output.open('w') if output else None
    child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stream, start_new_session=True)
    code = 0
    budget_exceeded = False
    enforcement_load = None
    observed = before
    try:
        while True:
            elapsed = time.monotonic() - started
            remaining = seconds - elapsed if seconds is not None and not busy else None
            try:
                code = child.wait(timeout=max(0.001, min(5, remaining)) if remaining is not None else 5)
                break
            except subprocess.TimeoutExpired:
                # Bounded process waits, never load generation. Once load is busy/unknown,
                # report this phase as exempt rather than making host contention a flake.
                if respect_load:
                    observed = load_snapshot()
                    busy |= not observed['quiet']
                if seconds is not None and not busy and time.monotonic() - started >= seconds:
                    enforcement_load = observed
                    stop(child)
                    budget_exceeded = True
                    code = 124
                    break
    except KeyboardInterrupt:
        stop(child)
        code = 130
    finally:
        if stream:
            stream.close()
    elapsed = time.monotonic() - started
    after = load_snapshot() if respect_load else {'quiet': True}
    # A load change after a quiet-host termination cannot make killed tests a pass.
    # Retain the load at enforcement rather than mislabel that failure busy-exempt.
    if not budget_exceeded:
        busy |= not after['quiet']
    if code == 0 and seconds is not None and not busy and elapsed > seconds:
        code = 124
        budget_exceeded = True
        enforcement_load = after
    return {'exit_code': code, 'budget_exceeded': budget_exceeded,
            'enforcement_load': enforcement_load, 'seconds': elapsed, 'load_before': before,
            'load_after': after, 'busy_exempt': busy}


def run(command: list[str], seconds: float) -> int:
    """Strict primitive used by the tiny timeout fixture, independent of host load."""
    result = execute(command, seconds)
    if result['budget_exceeded']:
        print(f'FAILED: test runtime budget {seconds:g}s exceeded', file=sys.stderr)
    return result['exit_code']


def config() -> dict:
    return tomllib.loads((Path(__file__).resolve().parent.parent / '.config/gate.toml').read_text())


def initialize(path: Path, tier: str) -> None:
    state = {'tier': tier, 'limit': config()['seconds'][tier], 'measured': 0.0,
             'budgeted': 0.0, 'compile': 0.0, 'phases': [], 'load': load_snapshot()}
    path.write_text(json.dumps(state))
    print('Gate load: ' + json.dumps(state['load']), flush=True)


def phase(path: Path, label: str, command: list[str], compile_only: bool = False,
          output: Path | None = None) -> int:
    state = json.loads(path.read_text())
    result = execute(command, None if compile_only else max(0, state['limit'] - state['budgeted']),
                     respect_load=not compile_only, output=output)
    seconds = result['seconds']
    if compile_only:
        state['compile'] += seconds
    else:
        state['measured'] += seconds
        if not result['busy_exempt']:
            state['budgeted'] += seconds
    state['phases'].append({'label': label, 'compile': compile_only, **result})
    path.write_text(json.dumps(state))
    kind = 'Build/preparation' if compile_only else 'Test runtime'
    print(f'{kind} {label}: {seconds:.3f}s; load/core '
          f'{result["load_before"].get("load_per_core")} -> '
          f'{result["load_after"].get("load_per_core")}; busy-exempt={result["busy_exempt"]}', flush=True)
    if result['budget_exceeded']:
        print(f'FAILED: cumulative {state["tier"]} test runtime budget {state["limit"]}s exceeded; '
              f'enforced at load {result["enforcement_load"]}',
              file=sys.stderr, flush=True)
    return result['exit_code']


def report(path: Path) -> None:
    state = json.loads(path.read_text())
    print(f'Test budget summary: {state["tier"]} measured={state["measured"]:.3f}s '
          f'budgeted-quiet={state["budgeted"]:.3f}s build-preparation={state["compile"]:.3f}s '
          f'limit={state["limit"]}s; busy/unknown phases are reported, not budget failures', flush=True)


def main() -> int:
    args = sys.argv[1:]
    # The host-only slow lane retains a bounded wall guard, enforced only at quiet load.
    # It is not the quick/full gate's test budget and is never used by worker gates.
    if args and args[0] == 'slow':
        result = execute(['bash', *args[1:]], config()['seconds']['slow'], respect_load=True)
        print('Slow-lane guard: ' + json.dumps(result), flush=True)
        return result['exit_code']
    mode = args.pop(0)
    if mode == 'init':
        initialize(Path(args[0]), args[1])
        return 0
    if mode == 'report':
        report(Path(args[0]))
        return 0
    split = args.index('--')
    parser = argparse.ArgumentParser()
    parser.add_argument('state', type=Path)
    parser.add_argument('label')
    parser.add_argument('--output', type=Path)
    parsed = parser.parse_args(args[:split])
    return phase(parsed.state, parsed.label, args[split + 1:], mode == 'compile', parsed.output)


if __name__ == '__main__':
    sys.exit(main())
