#!/usr/bin/env python3
"""T-688: drive the production host/main.ts panel shell in an owned PTY with real keys and SGR mouse input.

Offline fixture I/O only (fake provider transport and control plane, private HOME/XDG, PTY and socket,
and an owned Git repository in the private launch directory). Checks stable reading through a held
stream and 140/80/52 relocation, explicit jump-to-live by keyboard and pointer while the sidebar keeps
the focus, the 80x14 sidebar drawer paging by keyboard and pointer, dialog isolation and clean terminal
restoration. Writes cumulative ANSI and screen-text captures. Never targets an owner, PM or worker pane.
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
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    # An owned Git repository in the private launch directory: six committed files changed in the working
    # tree give the sidebar's Files tab enough items for the 80x14 drawer to page.
    NOTES = [f'qa-{index}.md' for index in range(1, 7)]
    git = ['git', '-c', 'user.name=QA', '-c', 'user.email=qa@example.invalid', '-c', 'core.hooksPath=/dev/null', '-C', str(work)]
    subprocess.run(git + ['init', '-q'], check=True, env=env)
    (work / '.gitignore').write_text('*\n!.gitignore\n!qa-*.md\n')
    for name in NOTES: (work / name).write_text(name + ' base\n')
    subprocess.run(git + ['add', '.gitignore', *NOTES], check=True, env=env)
    subprocess.run(git + ['commit', '-qm', 'fixture base'], check=True, env=env)
    for name in NOTES: (work / name).write_text(name + ' changed\n')
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
        # From 105 columns the sidebar is a rail inside the main view: a `│` column on its left, its first row
        # (under the tab rule) the tabs `Context Files Plan` ending with the `Ctrl+B ▸` toggle.
        edge = screen.columns - 35
        rail = screen.columns >= 105 and any(row[edge] == '│' and ''.join(row[edge:]).rstrip().endswith('▸') for row in screen.cells)
        return edge if rail else screen.columns

    def main_lines():
        # Rows the rail occupies lose its cells; header, tabs, dock, composer, status bar and footer span the full width.
        edge = main_columns()
        return [''.join(row[:edge] if edge < screen.columns and row[edge] == '│' else row).rstrip() for row in screen.cells]

    def visible(text): return text in '\n'.join(main_lines())
    def on_screen(text): return text in '\n'.join(screen.lines())

    def bound():
        # The status bar: the role pill ` WORKER `, then the binding dot and generation: `● g1` bound, `○` not.
        return any(re.match(r'  WORKER  ● g1\b', line) for line in screen.lines())

    def footer():
        """The footer hint row: the last non-empty row, full width under the rule above it."""
        rows = [line for line in main_lines() if line.strip()]
        return rows[-1] if rows else ''

    def sidebar_focused():
        words = footer().split()
        return '←→' in words and 'tab' in words

    def sidebar_row():
        """The collapsed sidebar row (▴) or the open drawer's tab row (▾) above the composer."""
        for row, line in enumerate(main_lines()):
            if all(word in line for word in ('Context', 'Files', 'Plan')) and ('▴' in line or '▾' in line):
                return row, line
        return None

    def drawer_open(): return '▾' in (sidebar_row() or (0, ''))[1]

    COMPARISONS = ('candidate', 'since session', 'working tree', 'unstaged', 'staged')

    def comparison():
        """The comparison the Changes comparison row names (`candidate ▾  <base> → <head> …`), or None outside Changes."""
        for line in main_lines():
            for name in COMPARISONS:
                if name + ' ▾' in line: return name
        return None

    def changes_open():
        """Changes has the focus: its footer offers `c compare`."""
        return 'compare' in footer().split()

    def anchor():
        lines = main_lines()
        tabs = next((row for row, line in enumerate(lines) if 'F9' in line), None)
        if tabs is None: raise AssertionError('no main view tab row\n' + '\n'.join(lines))
        top = tabs + 1  # The transcript starts below the header rows, the tab row and the rule under it.
        while top < len(lines) and (not lines[top].strip() or set(lines[top]) == {'─'}): top += 1
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
        """The drawer pager `‹ a–b/n ›`, with the cells of its ‹ and › read from the rendered row."""
        for row, line in enumerate(screen.lines()):
            match = re.search(r'‹ (\d+)(?:–(\d+))?/(\d+) ›', line)
            start = screen.column(row, match.group(0)) if match else None
            if match and start is not None:
                return dict(row=row, first=int(match.group(1)), last=int(match.group(2) or match.group(1)), total=int(match.group(3)),
                            prev=start, next=start + len(match.group(0)) - 1)
        return None

    def drawer_body():
        """Which of the three rows below the drawer's tab row list a changed file."""
        found = sidebar_row()
        lines = main_lines()
        if not found: return []
        return [any(name in line for name in NOTES) for line in lines[found[0] + 1:found[0] + 4]]

    def drawer_selected():
        """The changed file on the drawer row that carries the selection bar `▌`, or None."""
        found = sidebar_row()
        if not found: return None
        for line in main_lines()[found[0] + 1:found[0] + 3]:
            if line.startswith('▌'): return next((name for name in NOTES if name in line), None)
        return None

    checks = []
    until(bound, 'production panel host bound')
    # F7 opens Changes on the committed candidate (this fixture supplies no inspection), whose comparison row
    # names it; `c` steps to the session comparison, whose row names that.
    send('\x1b[18~')
    until(lambda: changes_open() and comparison() == 'candidate', 'Changes on the candidate: its comparison row names it')
    for _ in COMPARISONS:
        if comparison() == 'since session': break
        before = comparison(); send('c')
        until(lambda: comparison() not in (None, before), 'next comparison after c')
    until(lambda: comparison() == 'since session' and all(visible(name) for name in NOTES), 'session comparison lists the owned changes')
    send('\x1b[20~')
    until(lambda: comparison() is None and not changes_open(), 'F9 returns to Chat')
    checks.append(f'F7 and c select the session comparison: {len(NOTES)} owned working-tree changes')
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
    until(lambda: main_columns() == 140 and visible('Jump to latest') and sidebar_row() is not None, 'Ctrl+B hides the rail at 140 columns')
    assert anchor() == reading, ('manual toggle moved the reader', anchor())
    save('reading-manual-inline-140x46')
    send('\x02')
    until(lambda: main_columns() == 105 and visible('Jump to latest'), 'rail restored')
    assert anchor() == reading
    checks.append('Ctrl+B hides the rail into the collapsed sidebar row and restores it; the same paragraph stays at the top')
    resize(80, 34)
    until(lambda: screen.cleared and sidebar_row() is not None and visible('Jump to latest'), 'narrow frame with the collapsed sidebar row')
    send('\x1b[17~')
    until(lambda: sidebar_focused() and drawer_open(), 'F6 focuses the sidebar and opens its drawer')
    send('\x1b[1;5F')  # Ctrl+End while the sidebar has the focus.
    until(lambda: visible('LIVE_TAIL') and not visible('Jump to latest'), 'keyboard jump to live')
    assert sidebar_focused(), 'sidebar lost focus on keyboard jump'
    save('jump-keyboard-80x34')
    checks.append('narrow: F6 opens the drawer with sidebar focus; Ctrl+End jumps to live while the sidebar keeps it')
    resize(140, 46)
    until(lambda: screen.cleared and visible('LIVE_TAIL') and main_columns() == 105 and sidebar_focused(), 'wide rail with sidebar focus at live')
    for _ in range(8): send('\x1b[<64;30;12M')
    until(lambda: visible('Jump to latest message'), 'wheel pauses follow')
    target = screen.find('Jump to latest message')
    assert target, 'jump control not on screen'
    click(target[1] + 2, target[0])
    until(lambda: visible('LIVE_TAIL') and not visible('Jump to latest'), 'pointer jump to live')
    assert sidebar_focused() and on_screen(DRAFT), 'pointer jump changed focus or draft'
    save('jump-pointer-140x46')
    checks.append('wide: wheel pauses follow; clicking the jump control resumes it with sidebar focus and draft intact')
    resize(80, 14)
    until(lambda: screen.cleared and sidebar_row() is not None, 'short terminal with the sidebar drawer')
    send('\x1b')  # Esc gives the focus back and closes the drawer F6 opened.
    until(lambda: sidebar_row() is not None and not drawer_open() and not sidebar_focused(), 'collapsed sidebar row at 80x14')
    row, _ = sidebar_row()
    click(screen.column(row, 'Files'), row)
    until(lambda: drawer_open() and pager() is not None, 'Files tab click opens the drawer with its pager')
    start = pager()
    assert start['first'] == 1 and start['total'] == len(NOTES), start
    send('\x1b[17~')
    until(sidebar_focused, 'F6 focuses the open drawer')
    save('pager-80x14')
    # The drawer shows two files: ↓ moves the selection inside the window first, and the window scrolls by one
    # only when the selection would leave it.
    send('\x1b[B')
    until(lambda: drawer_selected() == NOTES[1], 'keyboard next item inside the window')
    assert (pager() or {}).get('first') == 1 and drawer_body() == [True, True, False], ('the window moved before the selection left it', pager(), drawer_body())
    send('\x1b[B')
    until(lambda: (pager() or {}).get('first') == 2, 'keyboard next item past the window scrolls it by one')
    assert (pager() or {}).get('last') == 3 and drawer_selected() == NOTES[2] and drawer_body() == [True, True, False], ('drawer body is not two file rows', pager(), drawer_body())
    # ‹ selects the item before the window and ends the page there; › selects the first item past it and starts there.
    now = pager(); click(now['prev'], now['row'])
    until(lambda: (pager() or {}).get('first') == 1, 'pointer previous page')
    now = pager(); click(now['next'], now['row'])
    until(lambda: (pager() or {}).get('first') == 3, 'pointer next page')
    save('pager-pointer-80x14')
    checks.append(f'80x14: a Files tab click opens the two-row drawer with its pager ‹ n/{start["total"]} ›; Down moves the selection, then scrolls the window by one; ‹/› clicks turn whole pages')
    before = pager()
    send('\x15/fixture-native-confirm\r')
    until(lambda: on_screen('Native permission fixture'), 'native dialog')
    click(before['next'], before['row']); send('\x1b[17~\x02')
    save('dialog-80x14')
    send('\x1b')
    until(lambda: on_screen('NATIVE_CANCELLED'), 'dialog cancelled by Esc')
    after = pager()
    assert drawer_open() and after and (after['first'], after['total']) == (before['first'], before['total']), ('input under the dialog changed the drawer', before, after)
    checks.append('dialog owns input: a pager click, F6 and Ctrl+B underneath it change nothing; Esc cancels')
    send('\x1b[17~')
    until(sidebar_focused, 'F6 focuses the drawer after the dialog')
    assert (pager() or {}).get('first') == 3 and drawer_body() == [True, True, False], ('F6 after the dialog moved the drawer', pager())
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
                  provider='offline fixture transport, fake control plane and an owned Git repository', checks=checks)
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
