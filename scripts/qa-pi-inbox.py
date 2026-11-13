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
import re
import selectors
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unicodedata

parser = argparse.ArgumentParser()
parser.add_argument('output', type=Path)
parser.add_argument('--corral', type=Path, required=True)
parser.add_argument('--seed', type=Path, required=True)
args = parser.parse_args()
args.output = args.output.resolve()
args.output.mkdir(parents=True, exist_ok=True)
(args.output / 'result.json').unlink(missing_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='corral-inbox-pty-'))
master = slave = watch = None
proc = None
selector = selectors.DefaultSelector()
log = bytearray()
INCOMPLETE = re.compile(r'\x1b(?:\[[0-?]*[ -/]*|[\]_P^](?:[^\x07\x1b]|\x1b(?!\\))*\x1b?|[()])?\Z', re.S)
TOKEN = re.compile(r'\x1b\[([<=>?]?)([\d;:]*)[ -/]*([@-~])|\x1b[\]_P^].*?(?:\x07|\x1b\\)|\x1b[()].|\x1b.|[\x00-\x1f\x7f]|[^\x00-\x1f\x7f\x1b]+', re.S)
# The status bar: the role pill ` ASSISTANT `, then the binding dot and generation (`● g1` bound, `○` not), styling between.
BOUND = re.compile(rb' ASSISTANT (?:\x1b\[[\d;:]*m| )*\xe2\x97\x8f(?:\x1b\[[\d;:]*m)* g1\b')


class Screen:
    """Cell model of the alternate-screen frames pi-tui and PanelTerminal emit, for reading pointer targets."""

    def __init__(self, columns, rows):
        self.columns, self.rows, self.row, self.col, self.saved = columns, rows, 0, 0, (0, 0)
        self.cells = [[' '] * columns for _ in range(rows)]
        self.pending = ''

    def resize(self, columns, rows):
        self.cells = [[(self.cells[r][c] if r < len(self.cells) and c < len(self.cells[r]) else ' ') for c in range(columns)] for r in range(rows)]
        self.columns, self.rows = columns, rows
        # The cursor stays inside the new grid until the next frame positions it.
        self.row, self.col = min(self.row, rows - 1), min(self.col, columns)

    def feed(self, data):
        data = self.pending + data
        tail = INCOMPLETE.search(data)  # A PTY read can end inside an escape sequence; finish it next read.
        self.pending, data = (data[tail.start():], data[:tail.start()]) if tail else ('', data)
        for match in TOKEN.finditer(data):
            token = match.group(0)
            if match.group(3) is not None:
                self.csi(match.group(1), match.group(2), match.group(3))
            elif token == '\x1b7': self.saved = (self.row, self.col)
            elif token == '\x1b8': self.row, self.col = self.saved
            elif token == '\r': self.col = 0
            elif token == '\n': self.row = min(self.rows - 1, self.row + 1)
            elif not token.startswith('\x1b') and not (len(token) == 1 and (ord(token) < 32 or token == '\x7f')): self.print(token)

    def csi(self, prefix, params, final):
        if prefix: return
        values = [int(value.split(':')[0] or 0) for value in params.split(';')]
        first = values[0] if values else 0
        if final in 'Hf':
            self.row = max(0, min(self.rows - 1, (first or 1) - 1))
            self.col = max(0, min(self.columns - 1, ((values[1] if len(values) > 1 else 0) or 1) - 1))
        elif final == 'K':
            span = range(self.columns) if first == 2 else range(self.col + 1) if first == 1 else range(self.col, self.columns)
            for col in span: self.cells[self.row][col] = ' '
        elif final == 'J' and first in (2, 3):
            self.cells = [[' '] * self.columns for _ in range(self.rows)]

    def print(self, text):
        joined = False
        for char in text:
            lead = self.col - 1
            if lead > 0 and self.cells[self.row][lead] == '': lead -= 1  # Zero-width marks join a wide lead cell.
            if char in '\u200d\ufe0f' or unicodedata.combining(char):
                joined = joined or char == '\u200d'
                if lead >= 0: self.cells[self.row][lead] += char
                continue
            width = 2 if unicodedata.east_asian_width(char) in 'WF' else 1
            if joined:  # The joined emoji shares its sequence's existing cell pair.
                joined = False
                if lead >= 0: self.cells[self.row][lead] += char
                continue
            if self.col + width > self.columns: continue
            self.cells[self.row][self.col] = char
            if width == 2: self.cells[self.row][self.col + 1] = ''
            self.col += width

    def lines(self, end=None): return [''.join(row[:end]).rstrip() for row in self.cells]

    def column(self, row, needle):
        """The cell column where `needle` starts on `row`, or None."""
        line = ''.join(self.cells[row])
        index = line.find(needle)
        if index < 0: return None
        return sum(2 if unicodedata.east_asian_width(c) in 'WF' else 0 if (c in '\u200d\ufe0f' or unicodedata.combining(c)) else 1 for c in line[:index])


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
if (JSON.stringify(args) === JSON.stringify(["--root", %s, "attention", "inbox", "--json"])) {
  appendFileSync(%s, JSON.stringify(args) + "\\n");
  const result = spawnSync(%s, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  process.stdout.write(result.stdout ?? ""); process.stderr.write(result.stderr ?? ""); process.exit(result.status ?? 1);
}
const result = spawnSync(%s, args, { input: await Bun.stdin.text(), encoding: "utf8" });
process.stdout.write(result.stdout ?? ""); process.stderr.write(result.stderr ?? ""); process.exit(result.status ?? 1);
''' % tuple(json.dumps(str(value)) for value in [work, work / 'inbox-reads.jsonl', args.corral.resolve(), installed['corral']]))
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
    screen = Screen(140, 46)
    decoder = __import__('codecs').getincrementaldecoder('utf-8')('replace')
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
                    log.extend(chunk); screen.feed(decoder.decode(chunk))
                    if not chunk and not predicate(): raise AssertionError('Host exited: ' + reason)
    def text(value, after=0): return value.encode() in log[after:]
    def send(value): os.write(master, value)
    def frame(name, columns=140, rows=46):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete synchronized frame')
        (args.output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, columns=columns, rows=rows))
    def resize(columns, rows):
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        screen.resize(columns, rows)
        os.kill(proc.pid, signal.SIGWINCH)
    def click(col, row): send(f'\x1b[<0;{col + 1};{row + 1}M\x1b[<0;{col + 1};{row + 1}m'.encode())
    def main_columns():
        # From 105 columns the sidebar is a rail inside the main view: a `│` column on its left, its first row
        # (under the tab rule) the tabs `Context Files Plan` ending with the `Ctrl+B ▸` toggle.
        edge = screen.columns - 35
        rail = screen.columns >= 105 and any(row[edge] == '│' and ''.join(row[edge:]).rstrip().endswith('▸') for row in screen.cells)
        return edge if rail else screen.columns
    def main_lines():
        # Rows the rail occupies lose its cells; header, tabs, dock, composer, status bar and footer span the full width.
        edge = main_columns()
        return [''.join(row[:edge] if edge < screen.columns and row[edge] == '│' else row).rstrip() for row in screen.cells]
    def tab(label, key):
        """Row and cell of a main view tab's label, read from the rendered tab row."""
        for row, line in enumerate(main_lines()):
            if key in line and label in line: return row, screen.column(row, label)
        return None
    def item(key):
        """Row and cell of the Inbox list row whose task is `key`'s task: left of the list/detail rule."""
        task = key.split('/')[0]
        for row, line in enumerate(main_lines()):
            left = line.split('│')[0]
            if '│' in line and task in left and 'F11' not in line: return row, screen.column(row, task)
        return None
    def prompt_ends():
        path = work / 'ui-events.jsonl'
        return len(path.read_text().splitlines()) if path.exists() else 0
    scenes = []
    until(lambda: BOUND.search(log) is not None, 'production host bound')
    send(b'Keep unsent native draft')
    until(lambda: text('Keep unsent native draft'), 'actual composer draft')
    mark = len(log); send(b'\x1b[23~')
    until(lambda: text('read only', mark) and text('T-001/R1/Q1', mark), 'F11 reads canonical store')
    frame('inbox-keyboard')
    mark = len(log); send(b'h')
    until(lambda: text('all states', mark) and text('answered', mark), 'history navigation')
    frame('inbox-history')
    # Click the Inbox tab, at the cell the rendered tab row shows it, after returning to Chat.
    mark = len(log); send(b'\x1b[20~')
    until(lambda: text('Chat', mark) and log.endswith(b'\x1b[?2026l'), 'Chat rendered before mouse click')
    target = tab('Inbox', 'F11')
    assert target, 'Inbox tab not on screen'
    mark = len(log); click(target[1], target[0])
    until(lambda: text('read only', mark), 'mouse opens Inbox')
    closed = prompt_ends(); send(b'/')
    until(lambda: text('Filter canonical inbox'), 'native filter dialog')
    mark = len(log); send(b'T-001/R1/Q1\r')
    until(lambda: prompt_ends() > closed and text('Keep unsent native draft', mark), 'filter closed without submitting draft')
    mark = len(log); send(b'\r')
    until(lambda: text('Decision inspection', mark) and text('Revision: 1', mark), 'Enter inspects exact identity')
    frame('inbox-inspect')
    mark = len(log); send(b'\x1b')
    until(lambda: text('filter T-001/R1/Q1', mark) and log.endswith(b'\x1b[?2026l'), 'inspection closed')
    mark = len(log); send(b'a')
    until(lambda: text('Established answer flow', mark) and text('NOT SENT', mark), 'answer route instructions, no mutation')
    frame('inbox-answer-flow')
    mark = len(log); send(b'\x1b')
    until(lambda: text('filter T-001/R1/Q1', mark) and log.endswith(b'\x1b[?2026l'), 'answer flow closed')
    mark = len(log); resize(52, 24)
    until(lambda: text('read only', mark) and text('Keep unsent native draft', mark), 'narrow inbox and preserved draft')
    frame('inbox-small', 52, 24)
    mark = len(log); resize(140, 46)
    until(lambda: text('Keep unsent native draft', mark), 'wide draft continuity')
    # Mouse focuses the rendered Q1 list row; Enter must still inspect that same Q1.
    until(lambda: item('T-001/R1/Q1') is not None, 'wide Inbox list row')
    row = item('T-001/R1/Q1')
    mark = len(log); click(row[1], row[0]); send(b'\r')
    until(lambda: text('Decision inspection', mark) and text('T-001/R1/Q1', mark), 'mouse selection then keyboard inspection')
    frame('inbox-mouse')
    mark = len(log); send(b'\x1b')
    until(lambda: text('filter T-001/R1/Q1', mark) and log.endswith(b'\x1b[?2026l'), 'mouse inspection closed')
    mark = len(log); send(b'\x1b[20~')
    until(lambda: text('Chat', mark) and log.endswith(b'\x1b[?2026l'), 'return to composer')
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
    if proc.returncode != 0: raise AssertionError('host exit ' + str(proc.returncode))
    after = fingerprint()
    assert before == after, 'inspection changed canonical workspace bytes'
    calls = [json.loads(json.loads(line)['stdin']) for line in (work/'calls.jsonl').read_text().splitlines()]
    # The assistant host refreshes its planning state read-only after hello; any other operation fails.
    assert all(call['op'] in ['hello', 'event'] or (call['op'] == 'assistant_plan' and call['payload'].get('action') == 'read') for call in calls), 'unexpected mutation operation'
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
