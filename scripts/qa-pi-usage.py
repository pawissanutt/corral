#!/usr/bin/env python3
"""Replay actual accounting in an owned production-host PTY; offline transport only."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import selectors
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time

parser = argparse.ArgumentParser()
parser.add_argument('source', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
source = args.source.read_bytes()
inspection = json.loads(source)
assert inspection.get('usage', {}).get('rows'), 'Actual collected usage required'
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='usage-pty-'))
proc = None
master = slave = None
log = bytearray()
scenes, resizes = [], []
size = dict(columns=180, rows=38)
try:
    env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    (work / 'inspection.json').write_bytes(source)
    (args.output / 'inspection-source.json').write_bytes(source)
    launch = dict(version=1, store='st-0123456789abcdef', project='demo', role='worker', task=inspection['task'], run=inspection['run'], generation=1,
                  pane='private-pty', root=str(work), cwd=str(work), corral=installed['corral'], assets=installed['assets'],
                  runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'), model='gpt-6.1-sol', ui='rich',
                  secrets=dict(host_cap='h'*64, ctl_cap='c'*64, bind_nonce='n'*64))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 38, 180, 0, 0))
    read_fd, write_fd = os.pipe()
    def setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd)], cwd=work, env=env,
                            stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None; os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    selector = selectors.DefaultSelector(); selector.register(master, selectors.EVENT_READ)
    def until(text, mark=0):
        deadline = time.monotonic() + 30
        while text.encode() not in log[mark:]:
            events = selector.select(max(0, deadline - time.monotonic()))
            if not events: raise AssertionError('Timeout waiting for ' + text)
            chunk = os.read(master, 65536)
            if not chunk: raise AssertionError('Host exited before ' + text)
            log.extend(chunk)
    def send(data): os.write(master, data)
    def scene(name, text, data=None, resize=None):
        mark = len(log)
        if resize:
            columns, rows = resize
            resizes.append(dict(offset=mark, columns=columns, rows=rows))
            size.update(columns=columns, rows=rows)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
            os.kill(proc.pid, signal.SIGWINCH)
        if data: send(data)
        if text: until(text, mark)
        while len(log) <= mark or not log.endswith(b'\x1b[?2026l'):
            if not selector.select(15): raise AssertionError('Incomplete terminal frame: ' + name)
            log.extend(os.read(master, 65536))
        (args.output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, offset=mark, bytes=len(log), columns=size['columns'], rows=size['rows'], input_hex=data.hex() if data else None, resize=resize, asserted=text))
    until('Bound to Corral')
    # Local fake transport, not a provider: establish a retained conversation and editor draft.
    scene('conversation-before', 'FAKE_TRANSPORT_OK', b'Usage capture conversation\r')
    scene('wide-source-table', 'Measured usage', b'/panels usage\r')
    scene('wide-chart-toggle', 'Token bars', b'c')
    scene('wide-source-restored', 'No model rows', b'c')
    # End reaches the genuine legacy/unavailable budget state, without supplying invented limits.
    scene('wide-budget', 'Budget accounting unavailable', b'\x1b[6~' * 8)
    scene('compact-chart', '', resize=(52, 38))
    scene('compact-chart-top', 'Token bars', b'\x1b[5~' * 12 + b'c')
    scene('compact-budget', 'Budget accounting unavailable', b'\x1b[6~' * 12)
    scene('conversation-return', 'Usage capture conversation', b'\x1b[20~')
    scene('draft-retained', 'RETAINED_USAGE_DRAFT', b'RETAINED_USAGE_DRAFT')
    # Reopen Usage by its actual native navigation hit target, without consuming the draft.
    x = sum(map(len, [' Chat ', ' Diff ', ' Jobs ', ' Proof ', ' Inbox '])) + 2
    scene('draft-in-usage', 'unavailable', f'\x1b[<0;{x};1M\x1b[<0;{x};1m'.encode())
    scene('draft-usage-top', 'Measured usage', b'\x1b[5~' * 12)
    scene('draft-return', 'Usage capture conversation', b'\x1b[20~')
    calls = [json.loads(json.loads(line)['stdin']) for line in (work / 'calls.jsonl').read_text().splitlines()]
    assert all(c['op'] in ('hello', 'event') for c in calls), calls
    turns = [c for c in calls if c['op'] == 'event' and c['payload'].get('kind') == 'turn_started']
    assert len(turns) == 1, turns
    assert all(c['identity']['generation'] == 1 for c in calls)
    (args.output / 'calls.json').write_text(json.dumps(calls, indent=2))
    send(b'\x15\x04'); until('\x1b[?1049l'); proc.wait(timeout=15)
    assert proc.returncode == 0
    (args.output / 'result.json').write_text(json.dumps(dict(status='PASS', candidate=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(), initial_size=dict(columns=180, rows=38), resizes=resizes,
        source='actual collector inspection JSON replay; owned production host PTY; fake offline auth/bridge, no live panes',
        checks=['open /panels usage', 'chart on/off retains source table', 'PageDown/PageUp reaches source and budget', 'resize wide to compact',
                'F9 returns to same conversation', 'pointer reopens Usage and F9 preserves editor draft', 'one offline fixture turn; no paid calls; generation unchanged'], scenes=scenes), indent=2))
    print('PASS: owned actual-input usage PTY interactions; no live panes or provider calls')
finally:
    (args.output / 'pty-session.ansi').write_bytes(log)
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=5)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    for fd in (master, slave):
        if fd is not None: os.close(fd)
    shutil.rmtree(work, ignore_errors=True)
