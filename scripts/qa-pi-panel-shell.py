#!/usr/bin/env python3
"""T-688: drive the production host/main.ts panel shell in an owned PTY with real keys and SGR mouse input.

Offline fixture I/O only (fake provider transport and control plane, private HOME/XDG, PTY and socket).
Checks stable reading through a held stream and 140/80/52 relocation, explicit jump-to-live by keyboard
and pointer without taking inspector focus, short-terminal compact paging by keyboard and pointer,
dialog isolation and clean terminal restoration. Writes cumulative ANSI and screen-text captures.
Never targets an owner, PM or worker pane.
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
        rail = screen.columns >= 105 and all(screen.cells[r][screen.columns - 34] in '┌│└' for r in range(screen.rows))
        return screen.columns - 35 if rail else screen.columns

    def visible(text): return text in '\n'.join(screen.lines(main_columns()))
    def on_screen(text): return text in '\n'.join(screen.lines())

    def anchor():
        lines = screen.lines(main_columns())
        top = 2
        while top < len(lines) and not lines[top].strip(): top += 1
        for row in range(top, len(lines)):
            marker = re.search(r'P(\d\d)(E| )', lines[row])
            if marker:
                number = int(marker.group(1))
                return number if marker.group(2) == 'E' or row == top else number - 1
        raise AssertionError('no paragraph visible\n' + '\n'.join(lines))

    captures = []

    def save(name):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete synchronized frame')
        (args.output / (name + '.ansi')).write_bytes(log)
        (args.output / (name + '.txt')).write_text('\n'.join(screen.lines()) + '\n')
        captures.append(dict(name=name, cols=screen.columns, rows=screen.rows))
        assert screen.overflow == 0, ('cell overflow', name, screen.overflowed)

    def send(value): os.write(master, value.encode() if isinstance(value, str) else value)

    def click(col, row):
        send(f'\x1b[<0;{col + 1};{row + 1}M'); send(f'\x1b[<0;{col + 1};{row + 1}m')

    def resize(columns, rows):
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        screen.resize(columns, rows)
        os.kill(host.pid, signal.SIGWINCH)

    def pager():
        for row, line in enumerate(screen.lines()):
            match = re.search(r'‹ (\d+)(?:–(\d+))?/(\d+) ›', line)
            found = screen.find(match.group(0)) if match else None
            if match and found:
                start = found[1]
                return dict(row=row, first=int(match.group(1)), last=int(match.group(2) or match.group(1)), total=int(match.group(3)),
                            prev=start, next=start + len(match.group(0)) - 1)
        return None

    checks = []
    until(lambda: on_screen('Bound to Corral'), 'production panel host ready')
    send('/fixture-panel-reading\r')
    send('Stream the reading fixture.\r')
    until(lambda: visible('P40E'), 'live end of the first streamed delta')
    save('live-140x46')
    send('\x1b[1;5H\x1b[6~\x1b[6~')  # Ctrl+Home, PageDown x2: read back during the held stream.
    until(lambda: visible('Jump to latest message'), 'jump-to-live control while reading')
    reading = anchor()
    send(DRAFT)
    until(lambda: on_screen(DRAFT), 'grapheme draft in the real editor')
    save('reading-paused-140x46')
    checks.append(f'keyboard read-back pauses follow during the held stream at P{reading:02d}; grapheme draft in the real editor')
    os.write(gate, b'go\n'); os.close(gate); gate = None
    until(lambda: ended() == 1, 'released stream ends the turn')
    resize(140, 46)  # Same size: settle one frame after the final delta.
    until(lambda: on_screen(DRAFT), 'settled reading frame')
    assert not visible('LIVE_TAIL') and anchor() == reading, ('stream moved the reader', anchor())
    save('reading-after-stream-140x46')
    checks.append('stream completion appends LIVE_TAIL below without moving the paused reader')
    for columns, rows in [(80, 34), (52, 24), (140, 46)]:
        resize(columns, rows)
        until(lambda: screen.cleared and on_screen(DRAFT[:8]) and visible('Jump to latest'), f'{columns}x{rows} reading frame')
        assert anchor() == reading, (columns, anchor(), reading)
        save(f'reading-{columns}x{rows}')
    checks.append('140→80→52→140 relocation keeps the same paragraph at the top, the draft and the jump control')
    send('\x02')
    until(lambda: main_columns() == 140 and visible('Jump to latest'), 'manual inline toggle at 140 columns')
    assert anchor() == reading, ('manual toggle moved the reader', anchor())
    save('reading-manual-inline-140x46')
    send('\x02')
    until(lambda: main_columns() == 105 and visible('Jump to latest'), 'rail restored')
    assert anchor() == reading
    checks.append('Ctrl+B manual inline/rail transitions keep the same paragraph')
    resize(80, 34)
    until(lambda: screen.cleared and on_screen('F6 inspect'), 'narrow frame')
    send('\x1b[17~')
    until(lambda: on_screen('↑↓ Enter · Esc'), 'focused inline inspector')
    send('\x1b[5~\x1b[1;5F')  # PageUp then Ctrl+End while the inspector has focus.
    until(lambda: visible('LIVE_TAIL') and not visible('Jump to latest'), 'keyboard jump to live')
    assert on_screen('↑↓ Enter · Esc'), 'inspector lost focus on keyboard jump'
    save('jump-keyboard-80x34')
    checks.append('narrow: PageUp/Ctrl+End scroll and jump to live while the inspector keeps focus')
    resize(140, 46)
    until(lambda: screen.cleared and visible('LIVE_TAIL') and on_screen('tabs · ↑/↓'), 'wide focused rail at live')
    for _ in range(8): send('\x1b[<64;30;12M')
    until(lambda: visible('Jump to latest message'), 'wheel pauses follow')
    target = screen.find('Jump to latest message')
    assert target, 'jump control not on screen'
    click(target[1] + 2, target[0])
    until(lambda: visible('LIVE_TAIL') and not visible('Jump to latest'), 'pointer jump to live')
    assert on_screen('tabs · ↑/↓') and on_screen(DRAFT), 'pointer jump changed focus or draft'
    save('jump-pointer-140x46')
    checks.append('wide: wheel pauses follow; clicking the jump control resumes it with rail focus and draft intact')
    resize(80, 14)
    until(lambda: screen.cleared and pager() is not None, 'short-terminal compact pager')
    start = pager()
    assert start and start['first'] == 1 and start['total'] > 1, start
    save('pager-80x14')
    send('\x1b[B')
    until(lambda: (pager() or {}).get('first') == 2, 'keyboard next item')
    click(start['prev'], start['row'])
    until(lambda: (pager() or {}).get('first') == 1, 'pointer previous item')
    click(start['next'], start['row'])
    until(lambda: (pager() or {}).get('first') == 2, 'pointer next item')
    save('pager-pointer-80x14')
    checks.append(f'80x14: compact pager ‹ n/{start["total"]} ›: Down and ‹/› clicks move the same item')
    send('\x15/fixture-native-confirm\r')
    until(lambda: on_screen('Native permission fixture'), 'native dialog')
    click(start['next'], start['row']); send('\x1b[17~\x02')
    save('dialog-80x14')
    send('\x1b')
    until(lambda: on_screen('NATIVE_CANCELLED'), 'dialog cancelled by Esc')
    checks.append('dialog owns input: pager click, F6 and Ctrl+B underneath it change nothing; Esc cancels')
    send('\x1b[17~')
    until(lambda: (pager() or {}).get('first') == 2, 'same inspector item after the dialog')
    assert (pager() or {}).get('last') == 2, ('Ctrl+B under the dialog expanded the inspector', pager())
    save('after-dialog-80x14')
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
    result = dict(status='PASS', production_entrypoint='src/host/main.ts', input='real PTY key sequences and SGR mouse reports',
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
