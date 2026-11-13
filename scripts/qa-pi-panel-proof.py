#!/usr/bin/env python3
"""T-687: drive the production host/main.ts panel layout in an owned PTY. Offline fixture I/O only.

Proves, against the real entrypoint, bridge and InteractiveMode: a held streamed reply and real
tool output update in the panel layout; the bound session/generation survive the agents view,
the sidebar and resize (read back over the generation's own bridge); one composer submission
reaches the host once; role-restricted actions stay refused. Never targets an owner, PM or worker pane.
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
import socket
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
work = Path(tempfile.mkdtemp(prefix='cpp-'))
owned: list[int] = []
gate: int | None = None
proc: subprocess.Popen[bytes] | None = None
selector = selectors.DefaultSelector()
log = bytearray()
states = []
scenes = []
resizes = []
size = dict(columns=140, rows=46)
candidate = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
INCOMPLETE = re.compile(r'\x1b(?:\[[0-?]*[ -/]*|[\]_P^](?:[^\x07\x1b]|\x1b(?!\\))*\x1b?|[()])?\Z', re.S)
TOKEN = re.compile(r'\x1b\[([<=>?]?)([\d;:]*)[ -/]*([@-~])|\x1b[\]_P^].*?(?:\x07|\x1b\\)|\x1b[()].|\x1b.|[\x00-\x1f\x7f]|[^\x00-\x1f\x7f\x1b]+', re.S)
# The status bar's role badge, then the binding dot (● bound, ○ not bound), possibly with styling between.
BOUND = re.compile(rb'\xe2\x96\x8cWORKER(?:\x1b\[[\d;:]*m| )*\xe2\x97\x8f')


class Screen:
    """Cell model of the alternate-screen frames pi-tui and PanelTerminal emit (absolute moves, erase, text)."""

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
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    (work / 'tool-output.txt').write_text('TOOL_OUTPUT_SENTINEL\n')
    os.mkfifo(work / 'stream-gate')
    os.mkfifo(work / 'monitor-gate'); os.mkfifo(work / 'exit-gate')
    gate = os.open(work / 'stream-gate', os.O_RDWR)  # Holds a writer open: the host's read waits for our EOF.
    launch = dict(version=1, store='st-0123456789abcdef', project='demo', role='worker', task='T-001', run='R1',
                  generation=1, pane='private-pty', root=str(work), cwd=str(work), corral=installed['corral'],
                  assets=installed['assets'], runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'),
                  model='gpt-6.1-sol', ui='rich',
                  capabilities=dict(catalog=2, digest='b'*64, preset='standard', enabled=['background_shell'], project_text=False,
                                    settings=dict(background_shell=dict(max_jobs=2, max_output_bytes=8192, max_runtime_secs=60))),
                  secrets=dict(host_cap='h' * 64, ctl_cap='c' * 64, bind_nonce='n' * 64))
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
    screen = Screen(140, 46)
    decoder = __import__('codecs').getincrementaldecoder('utf-8')('replace')

    def setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = host = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd)],
                            cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    owned.remove(slave); os.close(slave)
    os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)

    def calls():
        path = work / 'calls.jsonl'
        if not path.exists(): return []
        return [json.loads(json.loads(line)['stdin']) for line in path.read_text().splitlines(keepends=True) if line.endswith('\n')]

    def kinds(kind): return [call for call in calls() if call['payload'].get('kind') == kind]

    def until(predicate, reason, timeout=12):
        deadline = time.monotonic() + timeout
        while not predicate():
            remaining = deadline - time.monotonic()
            events = selector.select(remaining) if remaining > 0 else []
            if not events: raise AssertionError('Timed out: ' + reason)
            for key, _ in events:
                if key.data == 'fixture': os.read(watch, 65536)
                elif key.data == 'terminal':
                    try: chunk = os.read(master, 65536)
                    except OSError: chunk = b''
                    log.extend(chunk); screen.feed(decoder.decode(chunk))
                    if not chunk and not predicate(): raise AssertionError('Host exited: ' + reason)
                else:
                    client, reply = key.data
                    chunk = client.recv(65536)
                    reply.extend(chunk)
                    if not chunk and not predicate(): raise AssertionError('Bridge closed: ' + reason)

    def bridge(command, label, generation=1):
        reply = bytearray()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.connect(str(work / 'run/host.sock'))
            client.sendall((json.dumps(dict(version=1, id='proof-' + label, ctl_cap='c' * 64, generation=generation,
                                            command=command, args={})) + '\n').encode())
            client.setblocking(False)
            selector.register(client, selectors.EVENT_READ, (client, reply))
            try: until(lambda: reply.endswith(b'\n'), 'bridge ' + command)  # Keeps draining the PTY meanwhile.
            finally: selector.unregister(client)
        return json.loads(reply)

    def binding(label, streaming, turn):
        reply = bridge('get_state', label)
        state = reply['result']
        observed = dict(label=label, generation=state['generation'], bound=state['bound'], mode=state['mode'],
                        session=state['session'], streaming=state['streaming'], turn=state['turn'])
        states.append(observed)
        assert reply['ok'] and reply['generation'] == 1, reply
        assert state['generation'] == 1 and state['bound'] and state['mode'] == 'interactive', observed
        assert state['session']['bound_id'] == state['session']['current_id'] == first['session']['bound_id'], observed
        assert state['session']['created'] == first['session']['created'], observed
        assert state['streaming'] == streaming and state['turn'] == turn, observed

    def save_frame(name, after=0):
        until(lambda: len(log) > after and log.endswith(b'\x1b[?2026l'), 'complete synchronized terminal frame')
        (args.output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, bytes=len(log), **size))

    def text(value, after=0): return value.encode() in log[after:]
    def send(value): os.write(master, value)

    def resize(columns, rows):
        resizes.append(dict(offset=len(log), columns=columns, rows=rows))
        size.update(columns=columns, rows=rows)
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        screen.resize(columns, rows)
        os.kill(host.pid, signal.SIGWINCH)

    def click(col, row): send(f'\x1b[<0;{col + 1};{row + 1}M\x1b[<0;{col + 1};{row + 1}m'.encode())

    def main_columns():
        # The rail paints its own tab row on the top line, ending with its ▸ toggle.
        return screen.columns - 35 if screen.columns >= 105 and screen.lines()[0].endswith('▸') else screen.columns

    def composer_bar():
        """The composer bar's row with the cells of its model ▾ and effort ↻, read from the rendered row."""
        for row, line in enumerate(screen.lines(main_columns())):
            if '▾' in line and '↻' in line: return dict(row=row, model=screen.column(row, '▾'), effort=screen.column(row, '↻'))
        return None

    def footer_keys():
        """Words of the footer hint row, the last non-empty row of the main column."""
        rows = [line for line in screen.lines(main_columns()) if line.strip()]
        return rows[-1].split() if rows else []

    until(lambda: BOUND.search(log) is not None, 'production panel host bound')
    first = bridge('get_state', 'ready')['result']
    states.append(dict(label='ready', generation=first['generation'], session=first['session'], turn=first['turn']))
    assert first['turn'] == {'current': 0, 'open': False} and first['session']['bound_id'], first
    # T-690: real SGR pointer controls use the native picker/cycle and keep the editor's draft.
    mark = len(log); send(b'Pointer draft')
    until(lambda: text('Pointer draft', mark), 'pointer-control draft')
    save_frame('controls-draft', mark)
    bar = composer_bar()
    assert bar, 'composer model/effort bar not on screen'
    mark = len(log); click(bar['model'], bar['row'])
    until(lambda: text('Only showing models from configured providers', mark), 'native model picker by pointer')
    save_frame('model-picker-pointer', mark)
    mark = len(log); send(b'\x1b')
    until(lambda: text('Pointer draft', mark), 'native model picker cancellation preserves draft')
    until(lambda: composer_bar() is not None, 'composer bar after the picker closes')
    bar = composer_bar()
    mark = len(log); click(bar['effort'], bar['row'])
    save_frame('effort-cycle-pointer', mark)
    mark = len(log); resize(80, 34)
    until(lambda: text('Pointer draft', mark), 'effort cycle and narrow relocation retain draft')
    save_frame('controls-narrow', mark)
    mark = len(log); resize(140, 46)
    until(lambda: text('Pointer draft', mark), 'control draft returns wide')
    send(b'\x15')
    stale = bridge('get_state', 'stale', generation=2)
    assert not stale['ok'] and stale['error']['code'] == 'stale_generation', stale
    send(b'/fixture-panel-stream\r')
    mark = len(log); send(b'Run the panel proof.')
    until(lambda: text('Run the panel proof.', mark), 'real composer draft')
    save_frame('composer-draft')
    assert kinds('turn_started') == []
    mark = len(log); send(b'\r')
    until(lambda: text('STREAM_PART_ONE', mark) and text('tool-output.txt', mark), 'partial stream after the read tool call')
    save_frame('stream-partial-wide', mark)
    assert not text('STREAM_PART_TWO') and len(kinds('turn_started')) == 1 and kinds('turn_ended') == []
    binding('streaming', True, {'current': 1, 'open': True})
    session_files = sorted((work / 'sessions').rglob('*.jsonl'))
    entries = [json.loads(line) for line in session_files[0].read_text().splitlines()]
    assert len([entry for entry in entries if entry.get('type') == 'thinking_level_change']) >= 2, entries
    mark = len(log); send(b'\x0f')  # Stock app.tools.expand through the panel terminal.
    until(lambda: text('TOOL_OUTPUT_SENTINEL', mark), 'expanded real tool output')
    save_frame('tool-output', mark)
    mark = len(log); send(b'Draft survives')
    until(lambda: text('Draft survives', mark), 'unsent draft during the held stream')
    # F8: the agents view during the held stream, then F6 hands the focus to the sidebar and Esc takes it back.
    mark = len(log); send(b'\x1b[19~')
    until(lambda: text('helper calls and jobs', mark), 'Agents view on the bound generation')
    save_frame('inspect-agents', mark)
    mark = len(log); send(b'\x1b[17~')
    until(lambda: text('←→', mark), 'sidebar focus: its keys in the footer')
    binding('inspecting', True, {'current': 1, 'open': True})
    mark = len(log); send(b'\x1b')
    until(lambda: text('steer', mark), 'Esc leaves the sidebar; the footer offers the composer keys while streaming')
    save_frame('inspect-closed', mark)
    for columns, rows in [(80, 34), (52, 24), (140, 46)]:
        mark = len(log); resize(columns, rows)
        until(lambda: text('Draft survives', mark) and log.endswith(b'\x1b[?2026l'), f'{columns}x{rows} frame with the draft')
        save_frame(f'stream-held-{columns}x{rows}', mark)
        binding(f'resized-{columns}x{rows}', True, {'current': 1, 'open': True})
    mark = len(log); send(b'\x1b[20~')
    until(lambda: text('STREAM_PART_ONE', mark), 'Conversation view restored with the held stream')
    assert not text('STREAM_PART_TWO') and kinds('turn_ended') == []
    send(b'\x15')
    mark = len(log)
    os.write(gate, b'go\n'); os.close(gate); gate = None
    until(lambda: text('STREAM_PART_TWO', mark) and len(kinds('turn_ended')) == 1, 'released stream completes the turn')
    save_frame('stream-complete-wide', mark)
    binding('completed', False, {'current': 1, 'open': False})
    assert [call['payload']['origin'] for call in kinds('turn_started')] == [{'kind': 'owner_terminal'}]
    sessions = sorted((work / 'sessions').rglob('*.jsonl'))
    users = [json.loads(line) for line in sessions[0].read_text().splitlines()] if len(sessions) == 1 else []
    users = [entry for entry in users if entry.get('type') == 'message' and entry['message'].get('role') == 'user']
    assert len(sessions) == 1 and len(users) == 1, (sessions, users)
    # T-693: owned real-input inspection, never resumes or rebinds the session.
    mark = len(log); send(b'\x1b[19~')
    until(lambda: text('helper calls and jobs', mark), 'agents view')
    save_frame('agents-wide', mark)
    binding('agents-wide', False, {'current': 1, 'open': False})
    mark = len(log); resize(52, 24)
    until(lambda: text('host observations', mark) and log.endswith(b'\x1b[?2026l'), 'narrow agents view')
    save_frame('agents-narrow', mark)
    binding('agents-narrow', False, {'current': 1, 'open': False})
    mark = len(log); send(b'\x1b[20~'); resize(140, 46)
    until(lambda: text('STREAM_PART_TWO', mark), 'original conversation after inspection')
    binding('inspector-return', False, {'current': 1, 'open': False})
    send(b'/fixture-workbench-jobs\r'); send(b'Start owned job inspection.\r')
    until(lambda: len(kinds('turn_ended')) == 2 and any(call['op'] == 'job_monitor' for call in calls()), 'owned job and line monitor started')
    mark = len(log); send(b'\x1b[19~')
    until(lambda: text('MONITOR_READY/ armed', mark) and text('BUILD_WAIT', mark), 'actual running job with monitor')
    save_frame('real-job-running', mark)
    mark = len(log)
    with open(work / 'monitor-gate', 'w') as job_gate: job_gate.write('ready\n')
    until(lambda: text('MONITOR_READY/ matched', mark), 'observed readiness line (not service health)')
    save_frame('real-job-line-match', mark)
    # Enter on the selected job row opens the bounded output pager; its footer has no F1 until Esc closes it.
    mark = len(log); send(b'\r')
    until(lambda: text('retained output', mark) and 'F1' not in footer_keys(), 'Enter opens the bounded output pager')
    save_frame('real-job-output', mark)
    send(b'\x1b')
    until(lambda: 'F1' in footer_keys(), 'Esc closes the output pager')
    mark = len(log); started = len(kinds('turn_started'))
    with open(work / 'exit-gate', 'w') as job_gate: job_gate.write('done\n')
    until(lambda: text('exit 7', mark) and text('exited', mark), 'actual nonzero job exit')
    save_frame('real-job-exited', mark)
    # The exit reaches the agent as a completion notice that starts its own turn. Wait until that notice
    # is shown and every started turn has ended, then read the binding at that stable point.
    until(lambda: text('j1-1:completion', mark) and len(kinds('turn_started')) > started
          and len(kinds('turn_started')) == len(kinds('turn_ended')), 'completion notice turn ends and the host is idle')
    settled = len(kinds('turn_started'))
    binding('job-exited', False, {'current': settled, 'open': False})
    # The notice turn already shows Chat; F9 keeps it there.
    send(b'\x1b[20~')
    until(lambda: any('BACKGROUND_STARTED' in line for line in screen.lines(main_columns())), 'return from jobs to original conversation')
    mark = len(log); send(b'/fixture-call fixture_forbidden\r'); send(b'Call the fixture sentinel.\r')
    until(lambda: text('Tool fixture_forbidden not found', mark) and len(kinds('turn_ended')) == settled + 1, 'role guard refusal')
    assert not text('forbidden tool ran')
    save_frame('forbidden-refused', mark)
    mark = len(log); send(b'/fixture-new-session\r')
    until(lambda: text('a new session is refused', mark), 'bound-session replacement refusal')
    save_frame('replacement-refused', mark)
    binding('refusals', False, {'current': settled + 1, 'open': False})
    # Three owner turns from this terminal; every other turn is a job notice, never an owner input.
    origins = [call['payload'].get('origin') for call in kinds('turn_started')]
    assert origins.count({'kind': 'owner_terminal'}) == 3 and len(origins) == settled + 1, origins
    assert all(call['identity']['generation'] == 1 for call in calls()) and len([c for c in calls() if c['op'] == 'hello']) == 1
    assert len(sorted((work / 'sessions').rglob('*.jsonl'))) == 1
    send(b'\x15\x04')
    until(lambda: b'\x1b[?1049l' in log, 'terminal restored')
    host.wait(timeout=10)
    if host.returncode != 0: raise AssertionError('host exit ' + str(host.returncode))
    (args.output / 'pty-session.ansi').write_bytes(log)
    (args.output / 'bridge-states.json').write_text(json.dumps(states, indent=2))
    result = dict(status='PASS', candidate=candidate, initial_size=dict(columns=140, rows=46), scenes=scenes, resizes=resizes,
                  production_entrypoint='src/host/main.ts', provider='offline fixture transport and fake control plane',
                  checks=['SGR pointer opens native model picker; Escape retains draft', 'SGR pointer cycles supported effort in the same session',
                          'read tool call and held first delta render in the 140-column panel layout', 'stock Ctrl+O expands real tool output',
                          'bridge get_state: same generation and bound/current session while streaming, inspecting and at 80/52/140 columns',
                          'stale generation refused by the bridge', 'F8 agents view, then F6 sidebar focus and Esc back, during the held stream',
                          'unsent draft retained through inspection and resize', 'one composer submission: one owner_terminal turn and one user entry',
                          'released stream completes the same turn', 'tool outside the loadout refused', 'bound-session replacement refused',
                          'agents view by real F8 input at 140/52 columns retains binding, generation and turn; no lifecycle controls',
                          'real owned job selected in F8: running, armed watch, observed match, exit 7 and the bounded output pager',
                          'one hello, one session file, generation 1 on every control-plane call', 'clean terminal exit'])
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
