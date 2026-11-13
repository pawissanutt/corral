#!/usr/bin/env python3
"""Owned production Pi PTY: real keyboard history reads over a copied recorded task.
No model calls or owner panes. Only the offline host hello/events use a fake adapter.
"""
import argparse
import ctypes
import fcntl
import hashlib
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
parser.add_argument('--corral', type=Path, required=True)
parser.add_argument('--seed', type=Path, required=True)
parser.add_argument('--recorded', type=Path, required=True)
args = parser.parse_args()
args.output = args.output.resolve()
args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
work = Path(tempfile.mkdtemp(prefix='corral-history-pty-'))
selector = selectors.DefaultSelector()
master = slave = watch = None
proc = None
log = bytearray()
# The status bar's role badge, then the binding dot (● bound, ○ not bound), possibly with styling between.
BOUND = re.compile(rb'\xe2\x96\x8cASSISTANT(?:\x1b\[[\d;:]*m| )*\xe2\x97\x8f')
try:
    env = {key: value for key, value in os.environ.items() if not key.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work/'home'), XDG_CONFIG_HOME=str(work/'config'), XDG_DATA_HOME=str(work/'data'),
               XDG_STATE_HOME=str(work/'state'), XDG_CACHE_HOME=str(work/'cache'), TERM='xterm-256color')
    subprocess.run([str(args.seed.resolve()), str(work), str(args.recorded.resolve())], check=True, env=env, stdin=subprocess.DEVNULL)
    task = json.loads(args.recorded.read_text())['id']
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(repo/'packages/corral-pi/test/support/prepare-workbench.ts'), str(work)], env=env, stdin=subprocess.DEVNULL))
    proxy = work/'history-reader'
    proxy.write_text('''#!/usr/bin/env bun
import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
const args = process.argv.slice(2);
const read = args[0] === "--root";
if (read) appendFileSync(%s, JSON.stringify(args) + "\\n");
const result = spawnSync(read ? %s : %s, args, { input: read ? "" : await Bun.stdin.text(), encoding: "utf8", maxBuffer: 4 * 1024 * 1024 });
if (read && result.error) process.stderr.write(String(result.error));
process.stdout.write(result.stdout ?? ""); process.stderr.write(result.stderr ?? ""); process.exitCode = result.status ?? 1;
''' % tuple(json.dumps(str(p)) for p in [work/'history-reads.jsonl', args.corral.resolve(), installed['corral']]))
    proxy.chmod(0o700)
    def fingerprint():
        return {str(p.relative_to(work)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((work/'.corral').rglob('*')) if p.is_file()}
    before = fingerprint()
    launch = dict(version=1, store='st-0123456789abcdef', project='', role='assistant', assistant='A-1',
                  generation=1, pane='private-pty', root=str(work), cwd=str(work), corral=str(proxy),
                  assets=installed['assets'], runtime_dir=str(work/'run'), session_dir=str(work/'sessions'),
                  model='gpt-6.1-sol', ui='rich', secrets=dict(host_cap='h'*64, ctl_cap='c'*64, bind_nonce='n'*64))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    read_fd, write_fd = os.pipe()
    libc = ctypes.CDLL(None, use_errno=True)
    watch = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    if watch < 0 or libc.inotify_add_watch(watch, str(work).encode(), 0x100 | 0x2 | 0x8) < 0:
        raise RuntimeError('owned fixture event watch unavailable')
    selector.register(master, selectors.EVENT_READ, 'terminal')
    selector.register(watch, selectors.EVENT_READ, 'fixture')
    def setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets'])/'src/host/main.ts'), '--launch-fd', str(read_fd)],
                            cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None
    os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    def until(predicate, reason):
        deadline = time.monotonic() + 30
        while not predicate():
            remaining = deadline - time.monotonic()
            if remaining <= 0: raise AssertionError('Timed out: '+reason)
            for key, _ in selector.select(remaining):
                if key.data == 'fixture': os.read(watch, 65536)
                else:
                    try: chunk = os.read(master, 65536)
                    except OSError: chunk = b''
                    log.extend(chunk)
                    if not chunk and not predicate(): raise AssertionError('Host exited: '+reason)
    def text(value, after=0): return value.encode() in log[after:]
    def send(value): os.write(master, value)
    def frame(name):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete frame')
        (args.output/(name+'.ansi')).write_bytes(log)
    def history_action(mark):
        # F2 opens the actions palette; its own filter field leaves the editor draft untouched.
        send(b'\x1bOQ')
        until(lambda: text('type to filter actions', mark), 'actions palette')
        send(b'history\r')
    until(lambda: BOUND.search(log) is not None, 'production host bound')
    send(b'History must preserve this draft')
    until(lambda: text('History must preserve this draft'), 'native draft')
    mark = len(log)
    history_action(mark)
    until(lambda: text('Tasks / runs', mark), 'Browse history action opens read-only history')
    send(b'\r')
    until(lambda: text('Search recorded history', mark), 'search input')
    send(task.encode()+b'\r')
    until(lambda: text('Select history', mark), 'canonical task search results')
    send(b'\r')
    until(lambda: text('Runs of '+task, mark), 'canonical run choices')
    send(b'\r')
    until(lambda: text('Last-known outcome', mark), 'recorded recap')
    frame('history-recap')
    mark = len(log); send(b't')
    until(lambda: text('records 1-200', mark), 'bounded transcript first page')
    frame('history-transcript-first')
    mark = len(log); send(b'n')
    until(lambda: text('bounded raw JSONL', mark) and text('previous page', mark), 'transcript next page')
    frame('history-transcript-next')
    mark = len(log); send(b'p')
    until(lambda: text('records 1-200', mark), 'transcript previous page')
    frame('history-transcript-previous')
    closed = len((work/'ui-events.jsonl').read_text().splitlines())
    mark = len(log); send(b'\x1b')
    until(lambda: len((work/'ui-events.jsonl').read_text().splitlines()) > closed, 'history overlay closed')
    mark = len(log); history_action(mark)
    until(lambda: text('Tasks / runs', mark), 'reopen history')
    send(b'\r')
    until(lambda: text('Search recorded history', mark), 'checkpoint history search')
    send(task.encode()+b'\r')
    until(lambda: text('Select history', mark), 'checkpoint task choices')
    send(b'\r')
    until(lambda: text('Runs of '+task, mark), 'checkpoint run choices')
    send(b'\r')
    until(lambda: text('Last-known outcome', mark), 'checkpoint recap')
    mark = len(log); send(b'c')
    until(lambda: text('Native checkpoint / rewind history', mark), 'native read-only checkpoint list')
    frame('history-checkpoints')
    closed = len((work/'ui-events.jsonl').read_text().splitlines())
    send(b'\x1b')
    until(lambda: len((work/'ui-events.jsonl').read_text().splitlines()) > closed, 'checkpoint overlay closed')
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 52, 0, 0))
    mark = len(log); os.kill(proc.pid, signal.SIGWINCH)
    until(lambda: text('History must preserve', mark), 'narrow composer continuity')
    frame('history-narrow-draft')
    send(b'\x15\x04')
    until(lambda: b'\x1b[?1049l' in log, 'terminal restored')
    def wait_exit(timeout=30):
        """Keep reading the PTY until the host exits: it still writes after restoring the terminal."""
        exited = os.pidfd_open(proc.pid)
        waiting = selectors.DefaultSelector()
        waiting.register(exited, selectors.EVENT_READ, 'exit')
        waiting.register(master, selectors.EVENT_READ, 'terminal')
        deadline = time.monotonic() + timeout
        try:
            while proc.poll() is None:
                remaining = deadline - time.monotonic()
                events = waiting.select(remaining) if remaining > 0 else []
                if not events: raise AssertionError('Timed out: host process exit after terminal restoration')
                for key, _ in events:
                    if key.data != 'terminal': continue
                    try: chunk = os.read(master, 65536)
                    except OSError: chunk = b''
                    if chunk: log.extend(chunk)
                    else: waiting.unregister(master)  # EOF/EIO: the host closed its PTY; its exit follows.
        finally:
            waiting.close(); os.close(exited)
    wait_exit()
    assert proc.returncode == 0
    after = fingerprint()
    assert before == after, 'history changed canonical store'
    calls = [json.loads(json.loads(line)['stdin']) for line in (work/'calls.jsonl').read_text().splitlines()]
    # The assistant host refreshes its planning state read-only after hello; any other operation fails.
    assert all(call['op'] in ['hello', 'event'] or (call['op'] == 'assistant_plan' and call['payload'].get('action') == 'read') for call in calls)
    assert not any(call['payload'].get('kind') == 'owner_input' for call in calls)
    reads = (work/'history-reads.jsonl').read_text()
    (args.output/'history-reads.jsonl').write_text(reads)
    (args.output/'canonical-hashes.json').write_text(json.dumps(dict(before=before, after=after), indent=2))
    result = dict(status='PASS', source_task=task, source='copied recorded real task, original eligible transcript locators',
                  provider='none; no model requests', checks=['F2 Browse history action', 'keyboard search/select task and run',
                  'recorded recap provenance', 'transcript next/previous pages', 'native checkpoint/rewind list', 'draft intact at 140 and 52 columns',
                  'canonical store byte-identical', 'no owner-input event or replacement', 'clean private PTY exit'])
    (args.output/'result.json').write_text(json.dumps(result, indent=2)); print(json.dumps(result, indent=2))
finally:
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (args.output/'pty-final.ansi').write_bytes(log)
    selector.close()
    for fd in [master, slave, watch]:
        if fd is not None: os.close(fd)
    shutil.rmtree(work)
