#!/usr/bin/env python3
"""Observe compilation, test time and load; all time thresholds are report-only (T-776)."""
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


def stop(child: subprocess.Popen, grace: float = 10) -> None:
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()


def execute(command: list[str], seconds: float | None, respect_load: bool = False,
            output: Path | None = None, grace: float = 10) -> dict:
    started = time.monotonic()
    before = load_snapshot() if respect_load else {'quiet': True}
    busy = not before['quiet']
    stream = output.open('w') if output else None
    interrupted = False
    def terminate(_signum, _frame):
        nonlocal interrupted
        interrupted = True
    previous = signal.signal(signal.SIGTERM, terminate)
    child = None
    code = 0
    try:
        child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stream, start_new_session=True)
        while True:
            if interrupted:
                raise KeyboardInterrupt
            try:
                code = child.wait(timeout=1)
                break
            except subprocess.TimeoutExpired:
                if respect_load:
                    busy |= not load_snapshot()['quiet']
    except KeyboardInterrupt:
        if child is not None:
            stop(child, grace)
        code = 130
    finally:
        signal.signal(signal.SIGTERM, previous)
        if stream:
            stream.close()
    elapsed = time.monotonic() - started
    after = load_snapshot() if respect_load else {'quiet': True}
    busy |= not after['quiet']
    return {'exit_code': code, 'budget_exceeded': seconds is not None and elapsed > seconds,
            'report_only': True, 'enforcement_load': None, 'seconds': elapsed,
            'load_before': before, 'load_after': after, 'busy_exempt': busy}


def run(command: list[str], seconds: float) -> int:
    """Observe a threshold without replacing the command's actual result."""
    result = execute(command, seconds)
    if result['budget_exceeded']:
        print(f'Time observation: {seconds:g}s threshold exceeded (report-only; T-776)', file=sys.stderr)
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
        print(f'Time observation: cumulative {state["tier"]} threshold {state["limit"]}s exceeded '
              '(report-only; T-776)',
              file=sys.stderr, flush=True)
    return result['exit_code']


def cost_audit(config: dict, rows: list[dict]) -> None:
    import re
    attempt = re.sub(r'[^A-Za-z0-9_-]', '_', os.environ.get('CORRAL_GATE_ATTEMPT', 'manual'))
    target = Path('.evidence') / f'{attempt}-existing-test-costs.md'
    target.parent.mkdir(exist_ok=True)
    lines = ['# Existing test cost audit — T-766 gardening input', '',
             f'Candidate: `{os.environ.get("CORRAL_CANDIDATE_HEAD", "manual")}`.',
             f'Integration base: `{config["ratchet_base"]}`.', '',
             'All time thresholds are report-only; enforcement is deferred to T-776.',
             'Candidate and existing overruns are observations; assertion failures still fail.',
             'Approved candidate exceptions retain their documented treatment. This list is',
             'partial if the gate fails/cancels; unrun tests are not timing proof.', '',
             '| Test | Seconds | Classification | Load/core before → after |',
             '|---|---:|---|---|']
    for row in sorted((row for row in rows if row['budget_exceeded']),
                      key=lambda value: value['seconds'], reverse=True):
        name = ' '.join(row['test']).replace('|', '\\|')
        lines.append(f'| `{name}` | {row["seconds"]:.3f} | {row["classification"]} | '
                     f'{row["load_before"].get("load_per_core")} → {row["load_after"].get("load_per_core")} |')
    if not any(row['budget_exceeded'] for row in rows):
        lines.append('| No overrun observed before the gate ended | | | |')
    target.write_text('\n'.join(lines) + '\n')


def report(path: Path) -> int:
    try:
        os.set_blocking(sys.stdout.fileno(), True)
        state = json.loads(path.read_text())
        metrics = path.with_name('case-metrics.jsonl')
        rows = []
        if metrics.is_file():
            for line in metrics.read_text().splitlines():
                row = json.loads(line)
                rows.append(row)
                print('Per-test budget observation: ' + line, flush=True)
        config = path.with_name('case.json')
        if config.is_file():
            cost_audit(json.loads(config.read_text()), rows)
        print(f'Test budget summary: {state["tier"]} measured={state["measured"]:.3f}s '
              f'budgeted-quiet={state["budgeted"]:.3f}s build-preparation={state["compile"]:.3f}s '
              f'reference={state["limit"]}s; ALL timing is report-only (T-776), including quiet load', flush=True)
    except OSError as error:
        # Discard any buffered stdout that Python would otherwise retry at shutdown.
        try:
            with open(os.devnull, 'w') as sink:
                os.dup2(sink.fileno(), sys.stdout.fileno())
        except OSError:
            pass
        try:
            print(f'Timing report unavailable: {error}', file=sys.stderr, flush=True)
        except OSError:
            pass
    return 0


def main() -> int:
    args = sys.argv[1:]
    # The host-only slow lane reports a wall reference too; no timing-based failure.
    # It is never used as another worker gate.
    if args and args[0] == 'slow':
        result = execute(['bash', *args[1:]], config()['seconds']['slow'], respect_load=True)
        print('Slow-lane observation: ' + json.dumps(result), flush=True)
        return result['exit_code']
    mode = args.pop(0)
    if mode == 'init':
        initialize(Path(args[0]), args[1])
        return 0
    if mode == 'report':
        return report(Path(args[0]))
    split = args.index('--')
    parser = argparse.ArgumentParser()
    parser.add_argument('state', type=Path)
    parser.add_argument('label')
    parser.add_argument('--output', type=Path)
    parsed = parser.parse_args(args[:split])
    return phase(parsed.state, parsed.label, args[split + 1:], mode == 'compile', parsed.output)


if __name__ == '__main__':
    sys.exit(main())
