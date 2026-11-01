#!/usr/bin/env python3
"""T-687: drive the production host/main.ts panel layout in an owned PTY. Offline fixture I/O only.

Proves, against the real entrypoint, bridge and InteractiveMode: a held streamed reply and real
tool output update in the panel layout; the bound session/generation survive inspection and
resize (read back over the generation's own bridge); one composer submission reaches the host
once; role-restricted actions stay refused. Never targets an owner, PM or worker pane.
"""
import argparse
import ctypes
import fcntl
import json
import os
from pathlib import Path
import pty
import selectors
import shutil
import signal
import socket
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
work = Path(tempfile.mkdtemp(prefix='cpp-'))
owned: list[int] = []
gate: int | None = None
proc: subprocess.Popen[bytes] | None = None
selector = selectors.DefaultSelector()
log = bytearray()
states = []
try:
    env = {key: value for key, value in os.environ.items() if not key.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    (work / 'tool-output.txt').write_text('TOOL_OUTPUT_SENTINEL\n')
    os.mkfifo(work / 'stream-gate')
    gate = os.open(work / 'stream-gate', os.O_RDWR)  # Holds a writer open: the host's read waits for our EOF.
    launch = dict(version=1, store='st-0123456789abcdef', project='demo', role='worker', task='T-001', run='R1',
                  generation=1, pane='private-pty', root=str(work), cwd=str(work), corral=installed['corral'],
                  assets=installed['assets'], runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'),
                  model='gpt-6.1-sol', ui='rich', secrets=dict(host_cap='h' * 64, ctl_cap='c' * 64, bind_nonce='n' * 64))
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
                    log.extend(chunk)
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

    def text(value, after=0): return value.encode() in log[after:]
    def send(value): os.write(master, value)

    def resize(columns, rows):
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        os.kill(host.pid, signal.SIGWINCH)

    until(lambda: text('Bound to Corral') and text('WORKER'), 'production panel host ready')
    first = bridge('get_state', 'ready')['result']
    states.append(dict(label='ready', generation=first['generation'], session=first['session'], turn=first['turn']))
    assert first['turn'] == {'current': 0, 'open': False} and first['session']['bound_id'], first
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
    mark = len(log); send(b'\x0f')  # Stock app.tools.expand through the panel terminal.
    until(lambda: text('TOOL_OUTPUT_SENTINEL', mark), 'expanded real tool output')
    save_frame('tool-output', mark)
    mark = len(log); send(b'Draft survives')
    until(lambda: text('Draft survives', mark), 'unsent draft during the held stream')
    mark = len(log); send(b'\x1b[17~\x1b[D\x1b[D')
    until(lambda: text('g1 · Working', mark), 'Agents inspector on the bound generation')
    mark = len(log); send(b'\r')
    until(lambda: text('Agents · Esc close', mark), 'inspector detail overlay')
    save_frame('inspect-agents', mark)
    binding('inspecting', True, {'current': 1, 'open': True})
    mark = len(log); send(b'q')
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
    mark = len(log); send(b'/fixture-call fixture_forbidden\r'); send(b'Call the fixture sentinel.\r')
    until(lambda: text('Tool fixture_forbidden not found', mark) and len(kinds('turn_ended')) == 2, 'role guard refusal')
    assert not text('forbidden tool ran')
    save_frame('forbidden-refused', mark)
    mark = len(log); send(b'/fixture-new-session\r')
    until(lambda: text('a new session is refused', mark), 'bound-session replacement refusal')
    save_frame('replacement-refused', mark)
    binding('refusals', False, {'current': 2, 'open': False})
    assert [call['payload']['origin'] for call in kinds('turn_started')] == [{'kind': 'owner_terminal'}] * 2
    assert all(call['identity']['generation'] == 1 for call in calls()) and len([c for c in calls() if c['op'] == 'hello']) == 1
    assert len(sorted((work / 'sessions').rglob('*.jsonl'))) == 1
    send(b'\x15\x04')
    until(lambda: b'\x1b[?1049l' in log, 'terminal restored')
    host.wait(timeout=10)
    if host.returncode != 0: raise AssertionError('host exit ' + str(host.returncode))
    (args.output / 'pty-session.ansi').write_bytes(log)
    (args.output / 'bridge-states.json').write_text(json.dumps(states, indent=2))
    result = dict(status='PASS', production_entrypoint='src/host/main.ts', provider='offline fixture transport and fake control plane',
                  checks=['read tool call and held first delta render in the 140-column panel layout', 'stock Ctrl+O expands real tool output',
                          'bridge get_state: same generation and bound/current session while streaming, inspecting and at 80/52/140 columns',
                          'stale generation refused by the bridge', 'Agents inspector and detail overlay during the held stream',
                          'unsent draft retained through inspection and resize', 'one composer submission: one owner_terminal turn and one user entry',
                          'released stream completes the same turn', 'tool outside the loadout refused', 'bound-session replacement refused',
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
