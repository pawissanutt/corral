#!/usr/bin/env python3
"""Owned PTY proof of a build session (`corral pi --build`) through the production build entry.

Prepares a private fixture install, a private Git repository with one commit and private HOME and XDG
directories, then starts packages/corral-pi/src/host/build.ts with an inherited launch descriptor in a
private PTY. The launch asks for the default layout: the entry itself creates the private worktree
`<repo>/.worktrees/build-qa` on branch `build/qa` (here=false); no Rust binary is involved. Real key
bytes drive it. The offline fixture transport answers every model request, the parent's and the
children's, from steps the driver writes; nothing reaches a network or a real provider.

Usage: python3 scripts/qa-pi-build.py <output-dir>
"""
import argparse
import codecs
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
work = Path(tempfile.mkdtemp(prefix='cpb-'))
owned: list[int] = []
gate: int | None = None
proc: subprocess.Popen[bytes] | None = None
selector = selectors.DefaultSelector()
log = bytearray()
captures: list[dict] = []
checks: list[str] = []
resizes = [dict(offset=0, columns=140, rows=46)]
INCOMPLETE = re.compile(r'\x1b(?:\[[0-?]*[ -/]*|[\]_P^](?:[^\x07\x1b]|\x1b(?!\\))*\x1b?|[()])?\Z', re.S)
TOKEN = re.compile(r'\x1b\[([<=>?]?)([\d;:]*)[ -/]*([@-~])|\x1b[\]_P^].*?(?:\x07|\x1b\\)|\x1b[()].|\x1b.|[\x00-\x1f\x7f]|[^\x00-\x1f\x7f\x1b]+', re.S)
# The status bar: the role pill ` BUILD `, then the binding. A build session has no corral binding and reads
# `○ unbound` (the word from 105 columns), never a generation.
UNBOUND = re.compile(r'^  BUILD  ○ unbound(?: │|$)')
# The braille spinner a running agent shows in its state cell.
SPIN = '[⠋⠙⠹⠸⠼⠴⠦⠧]'
EXITS = ('Keep on the branch', 'Land after the gate', 'Ship through the board', 'Keep iterating')
ALLOWED = ('Keep on the branch', 'Keep iterating')
REFUSED = ('Land after the gate', 'Ship through the board')
KEY = dict(f7='\x1b[18~', f8='\x1b[19~', f9='\x1b[20~', f10='\x1b[21~', esc='\x1b', alt_t='\x1bt', enter='\r')


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


screen = Screen(140, 46)


def dump() -> str:
    return f'\n--- screen {screen.columns}x{screen.rows} ---\n' + '\n'.join(screen.lines()) + '\n--- end of screen ---'


def check(condition, label):
    """Records `label` as a passed check, or fails with the screen it was read from."""
    if not condition: raise AssertionError('Check failed: ' + label + dump())
    checks.append(label)


try:
    # Private HOME and XDG roots; no ambient corral, herdr, Pi, provider, cloud or Git configuration survives.
    secret = re.compile(r'API_KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL', re.I)
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(('HERDR_', 'CORRAL_', 'PI_', 'AWS_', 'ANTHROPIC', 'OPENAI', 'GIT_')) and not secret.search(key)}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color',
               GIT_CONFIG_NOSYSTEM='1')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)],
                                                   cwd=repo, env=env, stdin=subprocess.DEVNULL))

    # A private repository with one commit; the build entry creates its worktree inside it.
    project = work / 'qa-repo'
    project.mkdir()
    worktree = project / '.worktrees' / 'build-qa'

    def git(*argv, cwd=project):
        return subprocess.run(['git', '-c', 'user.name=Build QA', '-c', 'user.email=build-qa@example.test', '-c', 'commit.gpgsign=false',
                               '-c', 'core.hooksPath=/dev/null', *argv], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                              capture_output=True, text=True, check=True).stdout.strip()
    git('init', '--quiet', '--initial-branch=main')
    (project / 'README.md').write_text('Build session QA fixture.\n')
    git('add', 'README.md')
    git('commit', '--quiet', '-m', 'base')

    # Provider steps: the parent delegates two slices, each child writes its own file and parks inside its
    # report stream until the driver closes the gate; then the parent replies. `match` keeps concurrent
    # children's requests on their own steps.
    held_log, gate_path, steps_path = work / 'held.log', work / 'child-gate', work / 'steps.json'
    os.mkfifo(gate_path)
    gate = os.open(gate_path, os.O_RDWR)  # Holds a writer open: the children's reads wait for our close.
    slices = [dict(id='alpha', title='Alpha slice', brief='QA_SLICE_ALPHA: create alpha.txt holding the line ALPHA_CONTENT.', owns=['alpha.txt']),
              dict(id='beta', title='Beta slice', brief='QA_SLICE_BETA: create beta.txt holding the line BETA_CONTENT.', owns=['beta.txt'])]
    steps = [dict(match='QA_PARENT_DELEGATE', toolCalls=[dict(callId='delegate', name='task', arguments=dict(agents=slices))]),
             dict(match='QA_PARENT_DELEGATE', text='BUILD_INTEGRATED')]
    for child in ('alpha', 'beta'):
        token = 'QA_SLICE_' + child.upper()
        steps += [dict(match=token, toolCalls=[dict(callId=child + '-write', name='write',
                                                    arguments=dict(path=child + '.txt', content=child.upper() + '_CONTENT\n'))]),
                  dict(match=token, chunks=[child.upper() + '_REPORT', ' wrote ' + child + '.txt'],
                       hold=dict(gate=str(gate_path), mark=str(held_log), label=child))]
    steps_path.write_text(json.dumps(steps))

    data_root = work / 'data' / 'corral' / 'corral-pi'
    launch = dict(version=1, assets=installed['assets'], data_root=str(data_root), corral=installed['corral'],
                  repo_root=str(project), here=False, name='qa', provider='openai-codex', model='gpt-6.1-sol',
                  settings=dict(exit_policy='review', max_parallel=2, profiles=[]))
    master, slave = pty.openpty()
    owned += [master, slave]
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
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
    entry = Path(installed['assets']) / 'src/host/build.ts'
    proc = host = subprocess.Popen(['bun', '--no-env-file', str(entry), '--launch-fd', str(read_fd)],
                                   cwd=project, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    owned.remove(slave); os.close(slave)
    os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    decoder = codecs.getincrementaldecoder('utf-8')('replace')
    candidate = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, stdin=subprocess.DEVNULL, text=True).strip()

    def until(predicate, reason, timeout=15):
        deadline = time.monotonic() + timeout
        while not predicate():
            remaining = deadline - time.monotonic()
            events = selector.select(remaining) if remaining > 0 else []
            if not events: raise AssertionError('Timed out waiting for: ' + reason + dump())
            for key, _ in events:
                if key.data == 'fixture': os.read(watch, 65536); continue
                if key.data == 'exit': continue
                try: chunk = os.read(master, 65536)
                except OSError: chunk = b''
                log.extend(chunk); screen.feed(decoder.decode(chunk))
                if not chunk:
                    selector.unregister(master)  # EOF/EIO: the host closed its PTY; keep waiting for its exit.
                    if not predicate() and host.poll() is not None:
                        raise AssertionError(f'Host exited {host.returncode} while waiting for: ' + reason + dump())

    def send(value): os.write(master, value.encode())

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
    def main_text(): return '\n'.join(main_lines())
    def row_with(*needles):
        return next((line for line in main_lines() if all(needle in line for needle in needles)), None)
    def matches(pattern): return re.search(pattern, main_text(), re.M) is not None
    def status_unbound(): return any(UNBOUND.search(line) for line in screen.lines())
    def card_row(state, name):
        # A row of the bordered task card: its `│`, one blank cell, the state glyph, two cells, the name.
        return matches(r'│ ' + state + r'  ' + re.escape(name) + r'\b')
    def agent_row(state, name):
        # A row of the Agents table: one blank cell, the selection bar or a blank, two cells, the state glyph, two cells, the name.
        return matches(r'^ [▌ ]  ' + state + r'  ' + re.escape(name) + r'\b')
    def finished(name):
        # The line a child posts under the task card when it comes back: `✓ <name> finished · <n> file(s) · <report>`.
        return matches(r'✓ ' + re.escape(name) + r' finished · 1 file · ' + name.upper() + r'_REPORT\b')
    def tab_row(): return row_with('F9', 'F8', 'F7', 'F10')
    def header(): return row_with('build · qa')
    def footer():
        rows = [line for line in main_lines() if line.strip()]
        return rows[-1] if rows else ''
    def composer_title():
        # The composer box's top border: `╭─ Message[ → receiver]` then dashes.
        return next((line for line in main_lines() if line.lstrip().startswith('╭─') and 'Message' in line), None)
    def receiver(name):
        title = composer_title()
        return title is not None and re.search(r'Message → ' + re.escape(name) + r'(?![\w-])', title) is not None
    def held(): return set(held_log.read_text().split()) if held_log.exists() else set()
    def dialog_open(): return row_with('How should this leave') is not None
    def option_row(label):
        lines = main_lines()
        index = next((i for i, line in enumerate(lines) if label in line), None)
        if index is None: return None, None
        following = lines[index + 1] if index + 1 < len(lines) else ''
        return lines[index], (None if any(other in following for other in EXITS) else following)

    def save(name, scene):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'a complete synchronized frame for ' + name)
        (args.output / (name + '.ansi')).write_bytes(log)
        (args.output / (name + '.txt')).write_text('\n'.join(screen.lines()) + '\n')
        captures.append(dict(name=name, scene=scene, columns=screen.columns, rows=screen.rows, bytes=len(log), resizes=list(resizes)))
        if screen.overflow: raise AssertionError(f'Cell overflow in {name}: {screen.overflowed}' + dump())

    def resize(columns, rows):
        resizes.append(dict(offset=len(log), columns=columns, rows=rows))
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        screen.resize(columns, rows)
        os.kill(host.pid, signal.SIGWINCH)

    def exit_dialog_checks(size):
        """The docked exit choice lists the four exits; outside a workspace only keep and iterate are available."""
        for label in ALLOWED:
            row, _ = option_row(label)
            check(row is not None and 'unavailable' not in row, f'{size}: exit option "{label}" is offered and available')
        for label in REFUSED:
            row, detail = option_row(label)
            refused = row is not None and ('unavailable' in row or (detail is not None and 'unavailable' in detail))
            check(refused, f'{size}: exit option "{label}" keeps its row and is marked unavailable')

    # Scene 1: ready.
    until(status_unbound, 'the status bar shows the BUILD pill with ○ unbound', timeout=60)
    until(lambda: tab_row() is not None and header() is not None, 'the header with "build · qa" and the tab row')
    save('01-ready-140x46', 'ready')
    check(header() is not None and 'corral / build · qa' in header(), 'header row 1 reads "corral / build · qa"')
    tabs = tab_row()
    for word in ('Chat', 'Agents', 'Changes', 'Board'):
        check(re.search(r'\b' + word + r'\b', tabs) is not None, f'the tab row offers {word}')
    for word in ('Inbox', 'Task', 'Proof'):
        check(re.search(r'\b' + word + r'\b', tabs) is None, f'the tab row has no {word} tab')
    check(re.search(r'F10\s*Board', tabs) is not None, 'Board sits on F10')
    check(row_with('qa-repo · .worktrees/build-qa', 'exit policy: review required') is not None,
          'header row 2 names the repository, the session worktree and the exit policy')
    check(status_unbound(), 'the status bar shows the BUILD pill and "○ unbound": a build session has no corral binding')
    check(worktree.is_dir() and git('rev-parse', '--abbrev-ref', 'HEAD', cwd=worktree) == 'build/qa',
          'the entry created the private worktree .worktrees/build-qa on branch build/qa')

    # Scene 2: delegation while the children run.
    send('/fixture-script ' + str(steps_path) + '\r')
    send('Delegate both slices. QA_PARENT_DELEGATE')
    until(lambda: row_with('QA_PARENT_DELEGATE') is not None, 'the delegation prompt drafted in the composer')
    send(KEY['enter'])
    until(lambda: held() == {'alpha', 'beta'} and row_with('task · 2 slices, one worktree') is not None
          and card_row(SPIN, 'alpha') and card_row(SPIN, 'beta'),
          'both children parked in their report stream and the task card with a running (spinner) row for each', timeout=45)
    save('02-delegation-running-140x46', 'delegation')
    check(row_with('alpha', 'Alpha slice') is not None and row_with('beta', 'Beta slice') is not None,
          'the task card has one row per child with its title')
    check((worktree / 'alpha.txt').read_text() == 'ALPHA_CONTENT\n' and (worktree / 'beta.txt').read_text() == 'BETA_CONTENT\n',
          'each child wrote its own file in the session worktree before reporting')
    send(KEY['f8'])
    until(lambda: agent_row(SPIN, 'alpha') and agent_row(SPIN, 'beta'), 'F8 Agents lists alpha and beta as running (spinner rows of its table)')
    save('02-agents-running-140x46', 'delegation')
    check(main_text().index('alpha') < main_text().index('beta'), 'Agents lists the children in fleet order')

    # Scene 3: one child's conversation and the message target.
    send(KEY['enter'])
    until(lambda: row_with('‹ Agents', 'alpha') is not None and row_with('brief from parent') is not None and receiver('alpha'),
          "Enter opens alpha's conversation and the composer title names alpha as the receiver")
    save('03-child-alpha-140x46', 'child conversation')
    check(row_with('Owns: alpha.txt', 'no board access', 'no nested agents') is not None, "alpha's conversation states what it owns")
    check(row_with('QA_SLICE_ALPHA') is not None, "alpha's conversation shows its brief")
    send(KEY['alt_t'])
    until(lambda: receiver('beta'), 'Alt+T moves the receiver to beta, the next running child')
    save('03-target-beta-140x46', 'child conversation')
    send(KEY['alt_t'])
    until(lambda: receiver('parent'), 'Alt+T cycles the receiver back to the parent')
    save('03-target-parent-140x46', 'child conversation')
    checks.append('Alt+T cycled the receiver alpha → beta → parent')

    # Scene 2 continued: the children finish, the card shows their results.
    send(KEY['f9'])
    until(lambda: row_with('task · 2 slices') is not None, 'F9 returns to Chat with the task card')
    os.close(gate); gate = None
    until(lambda: card_row('✓', 'alpha') and card_row('✓', 'beta') and finished('alpha') and finished('beta')
          and row_with('BUILD_INTEGRATED') is not None and 'Idle' in (header() or ''),
          'both children done in the card, each posted its finished line under it, the parent replied, header Idle', timeout=45)
    save('02-delegation-done-140x46', 'delegation')
    checks.append('the task card shows ✓ for alpha and beta; each posted "✓ <name> finished · 1 file · <report>" under it; the parent replied')

    # Scene 4: Changes lists the children's files with their authors.
    send(KEY['f7'])
    until(lambda: row_with('alpha.txt') is not None and row_with('beta.txt') is not None
          and matches(r'\ball 2\b.*\balpha 1\b.*\bbeta 1\b'),
          'F7 Changes lists alpha.txt and beta.txt with the author filter row "all 2 · alpha 1 · beta 1"')
    save('04-changes-140x46', 'changes')
    send('f')
    until(lambda: row_with('alpha.txt') is not None and row_with('beta.txt') is None, 'the "by agent" filter on alpha shows only alpha.txt')
    save('04-changes-by-alpha-140x46', 'changes')
    send('f')
    until(lambda: row_with('beta.txt') is not None and row_with('alpha.txt') is None, 'the "by agent" filter on beta shows only beta.txt')
    send('f')
    until(lambda: row_with('alpha.txt') is not None and row_with('beta.txt') is not None, 'the filter cycles back to all files')
    checks.append('Changes attributes alpha.txt to alpha and beta.txt to beta through the author filter')

    # Scene 5: Board outside a corral workspace.
    send(KEY['f10'])
    until(lambda: row_with('The board is not available in this session.') is not None and matches(r'outside a corral workspace'),
          'F10 Board states that the session is outside a corral workspace')
    save('05-board-140x46', 'board')
    body = main_text()
    for offer in ('Plan board tasks', 'details', 'refresh'):
        check(offer not in body, f'Board offers no "{offer}"')

    # Scene 6: /finish docks the exit choice.
    send(KEY['f9'])
    until(lambda: row_with('task · 2 slices') is not None, 'F9 returns to Chat')
    send('/finish\r')
    until(lambda: row_with('How should this leave the build session?') is not None and all(row_with(label) for label in EXITS)
          and row_with('Build finished · 2/2 slices') is not None,
          '/finish docks the exit choice with its four options and posts the result card')
    save('06-exit-choice-140x46', 'exit')
    exit_dialog_checks('140x46')
    check(row_with('unavailable: This session is outside a corral workspace.') is not None, 'the refusal states the session is outside a workspace')
    check('exit choice pending' not in (header() or ''), 'the header does not repeat the docked choice')
    send(KEY['esc'])
    until(lambda: not dialog_open(), 'Esc closes the exit choice without choosing')

    # Scene 7: Chat, Agents and the exit choice at 80x24 and 52x24.
    for columns, rows in ((80, 24), (52, 24)):
        size = f'{columns}x{rows}'
        resize(columns, rows)
        until(lambda: screen.cleared and tab_row() is not None and header() is not None and composer_title() is not None,
              f'{size} redraw with the header, the tab row and the composer')
        save(f'07-chat-{size}', 'small terminals')
        send(KEY['f8'])
        until(lambda: agent_row('✓', 'alpha') and agent_row('✓', 'beta'), f'{size} Agents lists alpha and beta as done')
        save(f'07-agents-{size}', 'small terminals')
        send(KEY['f9'])
        until(lambda: not agent_row('✓', 'alpha') and composer_title() is not None, f'{size} back in Chat')
        send('/finish\r')
        until(lambda: dialog_open() and all(row_with(label) for label in EXITS), f'{size} /finish docks the exit choice with all four options')
        save(f'07-exit-choice-{size}', 'small terminals')
        exit_dialog_checks(size)
        send(KEY['esc'])
        until(lambda: not dialog_open(), f'{size} Esc closes the exit choice')
        checks.append(f'{size}: Chat, Agents and the exit choice drawn without cell overflow')

    # Scene 6 continued: choosing keep.
    resize(140, 46)
    until(lambda: screen.cleared and tab_row() is not None and header() is not None, '140x46 redraw')
    send('/finish\r')
    until(dialog_open, '/finish docks the exit choice again')
    send('1')
    until(lambda: not dialog_open() and 'kept on branch' in (header() or ''), 'keep closes the dialog and the header shows "Idle · kept on branch"')
    save('06-exit-kept-140x46', 'exit')
    check('Idle · kept on branch' in header(), 'the header shows the chosen exit "Idle · kept on branch"')
    records = [json.loads(path.read_text()) for path in sorted((data_root / 'build').glob('B-*/session.json'))]
    check(len(records) == 1 and records[0].get('exit', {}).get('choice') == 'keep' and records[0].get('worktree') == str(worktree),
          'the session record holds the keep choice for the private worktree')

    # Scene 8: Ctrl+D restores the terminal and the host exits 0.
    mark = len(log)
    send('\x15\x04')
    stopping = time.monotonic()
    exited = os.pidfd_open(host.pid); owned.append(exited)
    selector.register(exited, selectors.EVENT_READ, 'exit')
    until(lambda: b'\x1b[?1049l' in log[mark:], 'Ctrl+D leaves the alternate screen', timeout=10)
    until(lambda: host.poll() is not None, 'the host process exits within ten seconds of Ctrl+D',
          timeout=max(0.1, 10 - (time.monotonic() - stopping)))
    elapsed = time.monotonic() - stopping
    if host.returncode != 0:
        raise AssertionError(f'host exit {host.returncode}: ' + log[log.rfind(b'\x1b[?1049l'):].decode(errors='replace'))
    checks.append(f'Ctrl+D left the alternate screen and the host exited 0 after {elapsed:.1f}s')
    corral_dirs = [str(Path(root) / name) for root, dirs, _ in os.walk(project) for name in dirs if name == '.corral']
    check(not corral_dirs, 'no .corral directory exists in the repository or its worktree')
    calls_log = work / 'calls.jsonl'
    corral_calls = [json.loads(line)['argv'] for line in calls_log.read_text().splitlines() if line.strip()] if calls_log.exists() else []

    (args.output / 'pty-session.ansi').write_bytes(log)
    (args.output / 'captures.json').write_text(json.dumps(captures, indent=2))
    result = dict(status='PASS', candidate=candidate, initial_size=dict(columns=140, rows=46), resizes=resizes,
                  scenes=[dict(name=c['name'], scene=c['scene'], columns=c['columns'], rows=c['rows'], bytes=c['bytes']) for c in captures],
                  production_entrypoint='src/host/build.ts --launch-fd', layout='private worktree <repo>/.worktrees/build-qa on build/qa (here=false)',
                  input='real PTY key bytes', provider='offline fixture transport (parent and children); local control plane',
                  recorded_corral_calls=corral_calls, checks=checks)
    (args.output / 'result.json').write_text(json.dumps(result, indent=2)); print(json.dumps(result, indent=2))
except BaseException as error:
    failure = dict(status='FAIL', error=str(error), scenes=[dict(name=c['name'], scene=c['scene'], columns=c['columns'], rows=c['rows'])
                                                          for c in captures], checks=checks)
    (args.output / 'result.json').write_text(json.dumps(failure, indent=2))
    raise
finally:
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (args.output / 'pty-final.ansi').write_bytes(log)
    (args.output / 'final.txt').write_text('\n'.join(screen.lines()) + '\n')
    selector.close()
    for fd in owned + ([gate] if gate is not None else []):
        os.close(fd)
    shutil.rmtree(work, ignore_errors=True)
