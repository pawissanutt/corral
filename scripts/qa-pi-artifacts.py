#!/usr/bin/env python3
"""Owned production-host PTY: offline artifact catalog fixture, real keyboard/resize input.
No provider calls, owner panes, scheduling operations, or live board state.
"""
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
parser.add_argument('output', type=Path)
parser.add_argument('--graphics', choices=('kitty', 'none'), default='none')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='artifacts-pty-'))
proc = None
master = slave = None
log = bytearray()
scenes = []
resizes = []
size = dict(columns=140, rows=46)
try:
    env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'CORRAL_', 'PI_')) and k not in ('TERM_PROGRAM', 'KITTY_WINDOW_ID')}
    env['PI_IMAGE_PROTOCOL'] = args.graphics
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    import hashlib
    store = work / 'store'; (store / 'artifacts/T-001/R1').mkdir(parents=True)
    png = (work / 'zoom.png').read_bytes(); (store / 'artifacts/T-001/R1/1111-shot.png').write_bytes(png)
    head, old = 'b' * 40, 'a' * 40
    def row(name, **extra):
        value = dict(reference='T-001:R1:' + name, run='R1', kind='report', title=name, path='artifacts/T-001/R1/' + name,
                     sha256=hashlib.sha256(name.encode()).hexdigest(), bytes=10, added='2026-01-01T00:00:00Z', compressed=False,
                     purged=False, available=True, raw=str(store / 'artifacts/T-001/R1' / name), evidence=[], evidence_omitted=0)
        value.update(extra); return value
    fixture = dict(version=1, task='T-001', run='R1', store=str(store), head=head, more=True, artifacts=[
        row('1111-shot.png', kind='screenshot', sha256=hashlib.sha256(png).hexdigest(), bytes=len(png),
            evidence=[dict(criterion='C1', key='c1-obs', status='pass', captured='2026-01-02T00:00:00Z', head=old, hash_match=True)]),
        row('2222-report.md', evidence=[dict(criterion='C2', key='c2-obs', status='pass', captured='2026-01-02T00:00:00Z', head=head, hash_match=False)]),
        row('3333-gone.log', kind='log', available=False, raw=None),
        row('4444-page.html'),
        dict(row('5555-other.md'), reference='T-001:R2:5555-other.md', run='R2', path='artifacts/T-001/R2/5555-other.md')])
    (work / 'inspection.json').write_text(json.dumps(fixture))
    (Path(installed['corral']).parent / 'artifact-show.txt').write_text('# Fixture report\nFIXTURE_REPORT_BODY\n')
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
    def until(text, mark=0):
        deadline = time.monotonic() + 20
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
        until(text, mark)
        while not log.endswith(b'\x1b[?2026l'):
            if not selector.select(10): raise AssertionError('Incomplete terminal frame: ' + name)
            log.extend(os.read(master, 65536))
        (args.output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, offset=mark, bytes=len(log), columns=size['columns'], rows=size['rows'], input_hex=data.hex() if data else None, resize=resize, asserted=text))
    until('Bound to Corral')
    viewer_log = Path(installed['corral']).parent / 'viewer-calls.jsonl'
    def viewer_calls(): return [json.loads(line) for line in viewer_log.read_text().splitlines()] if viewer_log.exists() else []
    scene('wide-evidence', 'Evidence', b'\x1b[21~')
    scene('wide-artifacts', 'differs from bound HEAD at read', b'a')
    assert viewer_calls() == [], 'focus/render must not start a viewer'
    if args.graphics == 'none':
        scene('wide-image-fallback', 'Graphics unavailable in this terminal', b'\r')
    else:
        scene('wide-image-viewer', 'click / Esc close', b'\r')
        scene('wide-image-closed', 'Bytes at open match the registered SHA256', b'\x1b')
    scene('wide-report-selected', 'recorded hash MISMATCH', b'j')
    assert viewer_calls() == [], 'selection must not start a viewer'
    scene('wide-report-opened', 'FIXTURE_REPORT_BODY', b'\r')
    scene('wide-absent', 'ABSENT: no snapshot file or live blob', b'j')
    scene('wide-absent-open', 'Unavailable · not opened', b'\r')
    scene('wide-html-selected', '4444-page.html', b'j')
    scene('wide-html-open', 'Browser document', b'\r')
    assert [c[3] for c in viewer_calls()] == ['show'], viewer_calls()
    scene('wide-html-browser', 'Browser requested', b'o')
    assert [(c[3], c[4]) for c in viewer_calls()] == [('show', 'T-001:R1:2222-report.md'), ('serve', 'T-001:R1:4444-page.html')], viewer_calls()
    scene('wide-other-run', 'Evidence: unknown · run R2 evidence not read', b'j')
    scene('wide-mark-a', 'Selected for comparison: A T-001:R2:5555-other.md', b'm')
    send(b'k'); send(b'k'); send(b'k')
    scene('wide-mark-b', 'B T-001:R1:2222-report.md', b'm')
    scene('wide-compare', 'no before/after, candidate order or freshness implied', b'c')
    scene('compact-compare', 'Title B: 2222-report.md', resize=(52, 30))
    scene('compact-export-refused', 'Session export refused', b'x')
    (work / 'inspection.json').unlink()
    scene('stale-disconnected-retained', 'STALE / disconnected', b'r')
    calls = [json.loads(json.loads(line)['stdin']) for line in (work / 'calls.jsonl').read_text().splitlines()]
    assert not [c for c in calls if c['op'] not in ('hello', 'event')], calls
    assert not [c for c in calls if c['op'] == 'event' and c['payload'].get('kind') == 'turn_started'], calls
    assert all(c['identity']['generation'] == 1 for c in calls)
    assert len(viewer_calls()) == 2, viewer_calls()
    scene('return-conversation', 'Chat', b'\x1b[20~')
    send(b'\x04'); until('\x1b[?1049l'); proc.wait(timeout=10)
    assert proc.returncode == 0
    (args.output / 'result.json').write_text(json.dumps(dict(status='PASS', candidate=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(), initial_size=dict(columns=140, rows=46), resizes=resizes, source='offline artifact catalog fixture; production host/main.ts and NativeWorkbench',
        graphics=args.graphics, checks=['real PTY keyboard input at 140 and 52 columns', 'image opens in the native viewer (kitty) or as text/path fallback (none)', 'report opens only on Enter via corral artifact show',
                'absent snapshot shown unavailable', 'HTML opens in browser only on o via corral artifact serve', 'stale/unknown candidate and other-run evidence labeled',
                'explicit A/B comparison without before/after', 'export refusal visible', 'unavailable source retains explicit stale rows',
                'generation unchanged; no model turn or scheduling mutation'], viewer_calls=viewer_calls(), scenes=scenes), indent=2))
    print('PASS: owned production-host PTY artifact browser; viewers only on explicit keys; no model turns or scheduling mutations')
finally:
    (args.output / 'pty-session.ansi').write_bytes(log)
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=5)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    for fd in (master, slave):
        if fd is not None: os.close(fd)
    shutil.rmtree(work, ignore_errors=True)
