#!/usr/bin/env python3
"""Owned production-host PTY: offline canonical read fixture, real keyboard/resize input.
No provider calls, owner panes, scheduling operations, or live board state.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import selectors
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time

parser = argparse.ArgumentParser()
parser.add_argument('output', type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='lifecycle-pty-'))
proc = None
master = slave = None
log = bytearray()
scenes = []
resizes = []
size = dict(columns=140, rows=46)
# The status bar's role badge, then the binding dot (● bound, ○ not bound), possibly with styling between.
BOUND = re.compile(rb'\xe2\x96\x8cWORKER(?:\x1b\[[\d;:]*m| )*\xe2\x97\x8f')
try:
    env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    fixture = dict(version=1, task='T-001', run='R1', graph_gap=True, event_gap=True, end=500000,
                   nodes=[dict(id='T-001', title='Blocked root fixture', status='running', depends_on=['T-002', 'T-999'], parent='T-002', missing=False),
                          dict(id='T-002', title='Unfinished prerequisite', status='backlog', depends_on=[], parent=None, missing=False),
                          dict(id='T-999', title='Missing canonical task', status=None, depends_on=[], parent=None, missing=True)],
                   events=[dict(source='store:owned-fixture', sequence=100+i, event=dict(ts='2026-01-01T00:00:00Z', task='T-001', kind='blocked', detail=dict(run='R1', index=i))) for i in range(80)])
    (work / 'inspection.json').write_text(json.dumps(fixture))
    launch = dict(version=1, store='st-0123456789abcdef', project='demo', role='worker', task='T-001', run='R1', generation=1,
                  pane='private-pty', root=str(work), cwd=str(work), corral=installed['corral'], assets=installed['assets'],
                  runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'), model='gpt-6.1-sol', ui='rich',
                  secrets=dict(host_cap='h'*64, ctl_cap='c'*64, bind_nonce='n'*64))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    read_fd, write_fd = os.pipe()
    def setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd)], cwd=work, env=env,
                            stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None; os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    selector = selectors.DefaultSelector(); selector.register(master, selectors.EVENT_READ)
    def seen(text, mark):
        return text.search(log, mark) is not None if isinstance(text, re.Pattern) else text.encode() in log[mark:]
    def label(text): return text.pattern.decode(errors='replace') if isinstance(text, re.Pattern) else text
    def until(text, mark=0):
        deadline = time.monotonic() + 20
        while not seen(text, mark):
            events = selector.select(max(0, deadline - time.monotonic()))
            if not events: raise AssertionError('Timeout waiting for ' + label(text))
            chunk = os.read(master, 65536)
            if not chunk: raise AssertionError('Host exited before ' + label(text))
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
        until(text, mark)
        while not log.endswith(b'\x1b[?2026l'):
            if not selector.select(10): raise AssertionError('Incomplete terminal frame: ' + name)
            log.extend(os.read(master, 65536))
        (args.output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, offset=mark, bytes=len(log), columns=size['columns'], rows=size['rows'], input_hex=data.hex() if data else None, resize=resize, asserted=text))
    until(BOUND)
    # F5 focuses Task; its footer offers g (graph) and l (lifecycle events).
    scene('wide-task', 'scroll', b'\x1b[15~')
    scene('wide-graph', '─depends_on→', b'g')
    scene('wide-selected-prerequisite', 'Inspect canonical task T-002', b'j')
    scene('wide-selected-missing', 'Inspect canonical task T-999', b'j')
    scene('compact-list', 'Dependency list', resize=(52, 30))
    scene('compact-selected-prerequisite', 'Inspect canonical task T-002', b'k')
    scene('compact-task', 'scroll', b'\x1b[15~')
    scene('compact-timeline', 'store:owned-fixture@101', b'l')
    scene('compact-event-selection', 'store:owned-fixture@102', b'j')
    scene('wide-timeline', 'Lifecycle timeline', resize=(140, 46))
    scene('wide-session-timeline', 'Inspect session', b'h')
    scene('wide-session-event-selected', '/ #2', b'j')
    scene('canonical-source-return', 'canonical database events', b'c')
    # A source outage must preserve rows and say stale, not show an empty successful history.
    (work / 'inspection.json').unlink()
    scene('stale-disconnected-retained', 'STALE / disconnected', b'r')
    calls = [json.loads(json.loads(line)['stdin']) for line in (work / 'calls.jsonl').read_text().splitlines()]
    assert not [c for c in calls if c['op'] not in ('hello', 'event')], calls
    assert not [c for c in calls if c['op'] == 'event' and c['payload'].get('kind') == 'turn_started'], calls
    assert all(c['identity']['generation'] == 1 for c in calls)
    scene('return-conversation', 'Chat', b'\x1b[20~')
    send(b'\x04'); until('\x1b[?1049l'); proc.wait(timeout=10)
    assert proc.returncode == 0
    (args.output / 'result.json').write_text(json.dumps(dict(status='PASS', candidate=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(), initial_size=dict(columns=140, rows=46), resizes=resizes, source='offline canonical read fixture; production host/main.ts and NativeWorkbench',
        checks=['real PTY keyboard input at 140 and 52 columns', 'F5 Task then g opens the graph/list and selects canonical task details', 'Task then l opens the timeline and selects source-identity event details',
                'unavailable source retains explicit stale rows', 'generation unchanged; no model turn or scheduling mutation'], scenes=scenes), indent=2))
    print('PASS: owned production-host PTY graph/list/timeline; no model turns or scheduling mutations')
finally:
    (args.output / 'pty-session.ansi').write_bytes(log)
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=5)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    for fd in (master, slave):
        if fd is not None: os.close(fd)
    shutil.rmtree(work, ignore_errors=True)
