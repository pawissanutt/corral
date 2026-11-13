#!/usr/bin/env python3
"""T-696: real keyboard QA of packaged appearance and actions in an owned offline host PTY.
Reuses the isolated production-host fixture and terminal cell model from qa-pi-panel-shell.py.
No provider calls, live panes, ambient extensions or authority mutations.
"""
import argparse
import ctypes
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
import unicodedata

parser = argparse.ArgumentParser()
parser.add_argument('output', type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='cps-'))
owned: list[int] = []
gate: int | None = None
proc: subprocess.Popen[bytes] | None = None
selector = selectors.DefaultSelector()
log = bytearray()
DRAFT = 'Draft 界面 👩‍💻 stays'
INCOMPLETE = re.compile(r'\x1b(?:\[[0-?]*[ -/]*|[\]_P^](?:[^\x07\x1b]|\x1b(?!\\))*\x1b?|[()])?\Z', re.S)
TOKEN = re.compile(r'\x1b\[([<=>?]?)([\d;:]*)[ -/]*([@-~])|\x1b[\]_P^].*?(?:\x07|\x1b\\)|\x1b[()].|\x1b.|[\x00-\x1f\x7f]|[^\x00-\x1f\x7f\x1b]+', re.S)


class Screen:
    """Cell model of the alternate-screen frames pi-tui and PanelTerminal emit (absolute moves, erase, text)."""

    def __init__(self, columns, rows):
        self.columns, self.rows, self.row, self.col, self.saved = columns, rows, 0, 0, (0, 0)
        self.cells = [[' '] * columns for _ in range(rows)]
        self.overflow, self.overflowed, self.pending, self.cleared = 0, None, '', False

    def resize(self, columns, rows):
        self.cells = [[(self.cells[r][c] if r < len(self.cells) and c < len(self.cells[r]) else ' ') for c in range(columns)] for r in range(rows)]
        self.columns, self.rows, self.cleared = columns, rows, False
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
            # A full redraw starts here; bytes still in flight from the previous size are discarded with it.
            self.cells = [[' '] * self.columns for _ in range(self.rows)]
            self.overflow, self.overflowed, self.cleared = 0, None, True

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
            if self.col + width > self.columns:
                self.overflow += 1; self.overflowed = (self.row, ''.join(self.cells[self.row]), text); continue
            self.cells[self.row][self.col] = char
            if width == 2: self.cells[self.row][self.col + 1] = ''
            self.col += width

    def lines(self, end=None): return [''.join(row[:end]).rstrip() for row in self.cells]

    def find(self, needle):
        for row, cells in enumerate(self.cells):
            line = ''.join(cells)
            if needle in line:
                index = line.index(needle)
                return row, sum(2 if unicodedata.east_asian_width(c) in 'WF' else 0 if (c in '\u200d\ufe0f' or unicodedata.combining(c)) else 1 for c in line[:index])
        return None


try:
    env = {key: value for key, value in os.environ.items() if not key.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    os.mkfifo(work / 'reading-gate')
    gate = os.open(work / 'reading-gate', os.O_RDWR)  # Holds a writer open: the host's read waits for our EOF.
    launch = dict(version=1, store='st-0123456789abcdef', project='demo', role='worker', task='T-001', run='R1',
                  generation=1, pane='private-pty', root=str(work), cwd=str(work), corral=installed['corral'],
                  assets=installed['assets'], runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'),
                  model='gpt-6.1-sol', ui='rich', secrets=dict(host_cap='h' * 64, ctl_cap='c' * 64, bind_nonce='n' * 64))
    master, slave = pty.openpty()
    owned += [master, slave]
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    screen = Screen(140, 46)
    read_fd, write_fd = os.pipe()
    libc = ctypes.CDLL(None, use_errno=True)
    watch = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    if watch >= 0: owned.append(watch)
    if watch < 0 or libc.inotify_add_watch(watch, str(work).encode(), 0x100 | 0x2 | 0x8) < 0:
        raise RuntimeError('owned fixture event watch unavailable')
    selector.register(master, selectors.EVENT_READ, 'terminal')
    selector.register(watch, selectors.EVENT_READ, 'fixture')

    def setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = host = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd)],
                                   cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    owned.remove(slave); os.close(slave)
    os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    decoder = __import__('codecs').getincrementaldecoder('utf-8')('replace')

    def calls():
        path = work / 'calls.jsonl'
        if not path.exists(): return []
        return [json.loads(json.loads(line)['stdin']) for line in path.read_text().splitlines(keepends=True) if line.endswith('\n')]

    def ended(): return len([call for call in calls() if call['payload'].get('kind') == 'turn_ended'])

    def until(predicate, reason, timeout=15):
        deadline = time.monotonic() + timeout
        while not predicate():
            remaining = deadline - time.monotonic()
            events = selector.select(remaining) if remaining > 0 else []
            if not events: raise AssertionError('Timed out: ' + reason + '\n' + '\n'.join(screen.lines()))
            for key, _ in events:
                if key.data == 'fixture': os.read(watch, 65536); continue
                if key.data == 'exit': continue
                try: chunk = os.read(master, 65536)
                except OSError: chunk = b''
                log.extend(chunk); screen.feed(decoder.decode(chunk))
                if not chunk:
                    selector.unregister(master)  # EOF/EIO: the host closed its PTY; keep waiting for its exit.
                    if not predicate() and host.poll() is not None: raise AssertionError('Host exited: ' + reason)

    def main_columns():
        # The rail paints its own tab row on the top line, ending with its ▸ toggle.
        rail = screen.columns >= 105 and screen.lines()[0].endswith('▸')
        return screen.columns - 35 if rail else screen.columns

    def visible(text): return text in '\n'.join(screen.lines(main_columns()))
    def on_screen(text): return text in '\n'.join(screen.lines())

    def bound():
        # The status bar's role badge carries the binding dot: ● bound, ○ not bound.
        return any(re.search(r'▌WORKER\s*●', line) for line in screen.lines())

    def palette_open():
        # The palette's filter row: its › prompt, with the session scope (role · project) on the right.
        return any(line.startswith('›') and 'worker · demo' in line for line in screen.lines(main_columns()))

    def sidebar_row():
        for line in screen.lines(main_columns()):
            if all(word in line for word in ('Context', 'Files', 'Plan')) and '▴' in line: return line
        return None

    captures = []
    # Replay the actual PTY output with its original geometry changes, preserving SGR colors.
    # A screenshot renderer must not feed old 140-column frames into a 52-column terminal.
    resizes = [dict(offset=0, columns=140, rows=46)]
    candidate = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, stdin=subprocess.DEVNULL, text=True).strip()
    checks = []
    def send(value): os.write(master, value.encode() if isinstance(value, str) else value)
    def save(name):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete frame')
        (args.output / (name + '.ansi')).write_bytes(log)
        (args.output / (name + '.txt')).write_text('\n'.join(screen.lines()) + '\n')
        captures.append(dict(name=name, cols=screen.columns, rows=screen.rows, candidate=candidate,
                             bytes=len(log), resizes=list(resizes)))
        assert screen.overflow == 0, ('cell overflow', name, screen.overflowed)
    def resize(columns, rows):
        resizes.append(dict(offset=len(log), columns=columns, rows=rows))
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        screen.resize(columns, rows)
        os.kill(host.pid, signal.SIGWINCH)
    def palette(query):
        send('\x1bOQ')
        until(palette_open, 'F2 opens the actions palette')
        if query: send(query)
        until(lambda: any('›' in line and query in line for line in screen.lines(main_columns())), 'keyboard search ' + query)
    until(bound, 'production host bound')
    save('dark-140x46')
    palette('answer')
    until(lambda: on_screen('Role worker cannot answer'), 'bound role-disabled answer route')
    save('role-disabled-140x46')
    send('\r')
    send('x')
    until(lambda: on_screen('no action matches'), 'disabled Enter did not close or dispatch')
    send('\x1b')
    until(lambda: not palette_open(), 'Esc cancels search')
    checks.append('F2 searches bound-role actions; disabled Enter does not dispatch; Esc cancels')
    for appearance in ['light', 'contrast', 'dark']:
        palette('appearance.' + appearance)
        send('\r')
        until(lambda: not palette_open(), appearance + ' selected')
        save(appearance + '-selected-140x46')
        checks.append('selected packaged ' + appearance + ' using real keyboard search/Enter')
    palette('density.compact')
    send('\r')
    until(lambda: not palette_open(), 'compact selected')
    save('compact-140x46')
    resize(52, 24)
    until(lambda: screen.cleared and sidebar_row() is not None, 'minimum-width layout with the collapsed sidebar row')
    palette('effort')
    save('effort-52x24')
    send('\x1b')
    until(lambda: not palette_open(), 'cancel minimum width palette')
    palette('app.model.select')
    send('\r')
    until(lambda: not palette_open() and on_screen('Only showing models'), 'existing native model selector')
    save('native-model-selector-52x24')
    send('\x1b')
    until(lambda: not on_screen('Only showing models'), 'native selector cancel')
    checks.append('52x24 clipped palette opens existing native model selector; Esc cancels without a provider call')
    assert not ended(), 'QA must never submit a model turn'
    send('\x15\x04')
    until(lambda: b'\x1b[?1049l' in log, 'terminal restored')
    exited = os.pidfd_open(host.pid); owned.append(exited)
    selector.register(exited, selectors.EVENT_READ, 'exit')
    stopping = time.monotonic()
    until(lambda: host.poll() is not None, 'host process exit after terminal restoration', timeout=30)
    if host.returncode != 0: raise AssertionError('host exit ' + str(host.returncode) + ': ' + log[log.rfind(b'\x1b[?1049l'):].decode(errors='replace'))
    checks.append(f'host process exited 0 {time.monotonic() - stopping:.1f}s after restoring the terminal')
    checks.append('Ctrl+D exits cleanly and leaves the alternate screen')
    (args.output / 'pty-session.ansi').write_bytes(log)
    (args.output / 'captures.json').write_text(json.dumps(captures, indent=2))
    result = dict(status='PASS', candidate=candidate, initial_size=dict(columns=140, rows=46), resizes=resizes,
                  scenes=[dict(name=c['name'], columns=c['cols'], rows=c['rows'], bytes=c['bytes']) for c in captures],
                  production_entrypoint='src/host/main.ts', input='real PTY keyboard input',
                  provider='offline fixture transport and fake control plane', checks=checks)
    (args.output / 'result.json').write_text(json.dumps(result, indent=2)); print(json.dumps(result, indent=2))
finally:
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (args.output / 'pty-final.ansi').write_bytes(log)
    selector.close()
    for fd in owned + ([gate] if gate is not None else []):
        os.close(fd)
    shutil.rmtree(work)
