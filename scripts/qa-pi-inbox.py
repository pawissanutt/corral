#!/usr/bin/env python3
"""Real keyboard/mouse input in a private production Pi PTY over a disposable canonical store.
No model requests, live herdr panes, owner events or answer mutations are required.
Build corral and the corral-ops pi_inbox_fixture example before running.
"""
import argparse
import ctypes
import fcntl
import hashlib
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
parser.add_argument('--corral', type=Path, required=True)
parser.add_argument('--seed', type=Path, required=True)
args = parser.parse_args()
args.output = args.output.resolve()
args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='corral-inbox-pty-'))
master = slave = watch = None
proc = None
selector = selectors.DefaultSelector()
log = bytearray()
try:
    env = {key: value for key, value in os.environ.items() if not key.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    subprocess.run([str(args.seed.resolve()), str(work)], check=True, env=env, stdin=subprocess.DEVNULL)
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env, stdin=subprocess.DEVNULL))
    # Only lifecycle binding is offline. Inbox bytes come from the candidate Rust CLI and
    # canonical Store, not a canned UI snapshot. The fixed proxy cannot run response commands.
    proxy = work / 'corral-inbox-reader'
    proxy.write_text('''#!/usr/bin/env bun
import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
const args = process.argv.slice(2);
if (JSON.stringify(args) === JSON.stringify(["attention", "inbox", "--json"])) {
  appendFileSync(%s, JSON.stringify(args) + "\\n");
  const result = spawnSync(%s, ["--root", %s, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  process.stdout.write(result.stdout ?? ""); process.stderr.write(result.stderr ?? ""); process.exit(result.status ?? 1);
}
const result = spawnSync(%s, args, { input: await Bun.stdin.text(), encoding: "utf8" });
process.stdout.write(result.stdout ?? ""); process.stderr.write(result.stderr ?? ""); process.exit(result.status ?? 1);
''' % tuple(json.dumps(str(value)) for value in [work / 'inbox-reads.jsonl', args.corral.resolve(), work, installed['corral']]))
    proxy.chmod(0o700)
    def fingerprint():
        return {str(path.relative_to(work)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in sorted((work / '.corral').rglob('*')) if path.is_file()}
    before = fingerprint()
    launch = dict(version=1, store='st-0123456789abcdef', project='', role='assistant', assistant='A-1',
                  generation=1, pane='private-pty', root=str(work), cwd=str(work), corral=str(proxy),
                  assets=installed['assets'], runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'),
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
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd)],
                            cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None
    os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    def until(predicate, reason, timeout=20):
        deadline = time.monotonic() + timeout
        while not predicate():
            remaining = deadline - time.monotonic()
            if remaining <= 0: raise AssertionError('Timed out: ' + reason)
            events = selector.select(remaining)
            if not events: raise AssertionError('Timed out: ' + reason)
            for key, _ in events:
                if key.data == 'fixture': os.read(watch, 65536)
                else:
                    try: chunk = os.read(master, 65536)
                    except OSError: chunk = b''
                    log.extend(chunk)
                    if not chunk and not predicate(): raise AssertionError('Host exited: ' + reason)
    def text(value, after=0): return value.encode() in log[after:]
    def send(value): os.write(master, value)
    def frame(name, columns=140, rows=46):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete synchronized frame')
        (args.output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, columns=columns, rows=rows))
    def resize(columns, rows):
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        os.kill(proc.pid, signal.SIGWINCH)
    def prompt_ends():
        path = work / 'ui-events.jsonl'
        return len(path.read_text().splitlines()) if path.exists() else 0
    scenes = []
    until(lambda: text('Bound to Corral') and text('ASSISTANT'), 'production host ready')
    send(b'Keep unsent native draft')
    until(lambda: text('Keep unsent native draft'), 'actual composer draft')
    mark = len(log); send(b'\x1b[23~')
    until(lambda: text('Canonical attention', mark) and text('T-001/R1/Q1', mark), 'F11 reads canonical store')
    frame('inbox-keyboard')
    mark = len(log); send(b'h')
    until(lambda: text('All states', mark) and text('answered', mark), 'history navigation')
    frame('inbox-history')
    # Click the native main-view bar after returning to Conversation.
    mark = len(log); send(b'\x1b[20~')
    until(lambda: text('Conversation F9', mark) and log.endswith(b'\x1b[?2026l'), 'Conversation rendered before mouse click')
    mark = len(log); send(b'\x1b[<0;60;1M\x1b[<0;60;1m')
    until(lambda: text('Canonical attention', mark), 'mouse opens Inbox')
    closed = prompt_ends(); send(b'/')
    until(lambda: text('Filter canonical inbox'), 'native filter dialog')
    mark = len(log); send(b'T-001/R1/Q1\r')
    until(lambda: prompt_ends() > closed and text('Keep unsent native draft', mark), 'filter closed without submitting draft')
    mark = len(log); send(b'j')
    until(lambda: text('> T-001', mark), 'explicit selection after filtering')
    mark = len(log); send(b'\r')
    until(lambda: text('Decision inspection', mark) and text('Revision: 1', mark), 'Enter inspects exact identity')
    frame('inbox-inspect')
    mark = len(log); send(b'\x1b')
    until(lambda: text('Filter: T-001/R1/Q1', mark) and log.endswith(b'\x1b[?2026l'), 'inspection closed')
    mark = len(log); send(b'a')
    until(lambda: text('Established answer flow', mark) and text('NOT SENT', mark), 'answer route instructions, no mutation')
    frame('inbox-answer-flow')
    mark = len(log); send(b'\x1b')
    until(lambda: text('Filter: T-001/R1/Q1', mark) and log.endswith(b'\x1b[?2026l'), 'answer flow closed')
    mark = len(log); resize(52, 24)
    until(lambda: text('READ ONLY', mark) and text('Keep unsent native draft', mark), 'narrow inbox and preserved draft')
    frame('inbox-small', 52, 24)
    mark = len(log); resize(140, 46)
    until(lambda: text('Keep unsent native draft', mark), 'wide draft continuity')
    # Mouse focuses a literal rendered item row; Enter must still inspect that same Q1.
    mark = len(log); send(b'\x1b[<0;10;11M\x1b[<0;10;11m\r')
    until(lambda: text('Decision inspection', mark) and text('T-001/R1/Q1', mark), 'mouse selection then keyboard inspection')
    frame('inbox-mouse')
    mark = len(log); send(b'\x1b')
    until(lambda: text('Filter: T-001/R1/Q1', mark) and log.endswith(b'\x1b[?2026l'), 'mouse inspection closed')
    mark = len(log); send(b'\x1b[20~')
    until(lambda: text('Conversation F9', mark) and log.endswith(b'\x1b[?2026l'), 'return to composer')
    send(b'\x15\x04')
    until(lambda: b'\x1b[?1049l' in log, 'terminal restored')
    proc.wait(timeout=10)
    if proc.returncode != 0: raise AssertionError('host exit ' + str(proc.returncode))
    after = fingerprint()
    assert before == after, 'inspection changed canonical workspace bytes'
    calls = [json.loads(json.loads(line)['stdin']) for line in (work/'calls.jsonl').read_text().splitlines()]
    assert all(call['op'] in ['hello', 'event'] for call in calls), 'unexpected mutation operation'
    assert not any(call['payload'].get('kind') == 'owner_input' for call in calls), 'inspection fabricated owner input'
    reads = (work/'inbox-reads.jsonl').read_text().splitlines()
    (args.output/'canonical-hashes.json').write_text(json.dumps(dict(before=before, after=after), indent=2))
    (args.output/'inbox-reads.jsonl').write_text('\n'.join(reads)+'\n')
    (args.output/'pty-session.ansi').write_bytes(log)
    result = dict(status='PASS', production_entrypoint='src/host/main.ts', provider='none: no model requests',
                  canonical_reader=str(args.corral.resolve()), scenes=scenes, canonical_files_unchanged=len(before),
                  checks=['F11 navigation', 'mouse navigation and selection', 'exact task/container/key/revision', 'answered/cancelled history', 'read-only inspection',
                          'revalidated external answer flow, NOT SENT', '140→52→140 unsent draft retention', 'no owner-input event or answer operation', 'all canonical files including unrelated task byte-identical', 'clean PTY exit'])
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
