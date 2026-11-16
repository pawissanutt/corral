#!/usr/bin/env python3
"""Real keyboard input for /handoff in the production host/main.ts PTY. Offline fake corral only:
proves the sheet, cancel-without-effect, 140/80/52/short-height rendering and that a selection
journals a bound source=choice input before the host-only handoff_choice consumer is called.
Service-side consumption is proven by the corral-ops native_tests, not here."""
import argparse, fcntl, json, os, pty, selectors, shutil, signal, struct, subprocess, tempfile, termios, time
from pathlib import Path

parser = argparse.ArgumentParser(); parser.add_argument('output', type=Path); args = parser.parse_args()
args.output = args.output.resolve(); args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]; package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='corral-handoff-pty-'))
master = slave = None; proc = None; selector = selectors.DefaultSelector(); log = bytearray()
try:
    env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)],
                                                   cwd=repo, env=env, stdin=subprocess.DEVNULL))
    fake = Path(installed['corral']).parent
    (fake / 'handoff.json').write_text(json.dumps(dict(
        driver='board', label='Board is handling this', waiting='Queued for normal admission', plan_only=False,
        phase='board_owned', task='T-9', contract_revision='c1', transfer='x', revision=2, shown='v'*64)))
    launch = dict(version=1, store='st-0123456789abcdef', project='', role='assistant', assistant='A-1', generation=1,
                  pane='private-pty', root=str(work), cwd=str(work), corral=installed['corral'], assets=installed['assets'],
                  runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'), model='gpt-6.1-sol', ui='rich',
                  secrets=dict(host_cap='h'*64, ctl_cap='c'*64, bind_nonce='n'*64))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    read_fd, write_fd = os.pipe()
    selector.register(master, selectors.EVENT_READ)
    def setup(): os.setsid(); fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd)],
                            cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None; os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)

    def calls():
        path = fake / 'calls.jsonl'
        if not path.exists(): return []
        out = []
        for line in path.read_text().splitlines(keepends=True):
            if not line.endswith('\n'): continue
            stdin = json.loads(line)['stdin']
            if stdin.strip(): out.append(json.loads(stdin))
        return out
    def kinds(cs): return [c['payload'].get('kind') for c in cs]
    def until(pred, reason, timeout=20):
        deadline = time.monotonic() + timeout
        while not pred():
            left = deadline - time.monotonic()
            if left <= 0 or not selector.select(left): raise AssertionError('Timed out: ' + reason)
            try: chunk = os.read(master, 65536)
            except OSError: chunk = b''
            log.extend(chunk)
            if not chunk and not pred(): raise AssertionError('Host exited: ' + reason)
    def text(v, after=0): return v.encode() in log[after:]
    def frame(name):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete frame ' + name)
        (args.output / (name + '.ansi')).write_bytes(log)
    def send(v): os.write(master, v)
    def resize(c, r):
        mark = len(log)
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', r, c, 0, 0)); os.kill(proc.pid, signal.SIGWINCH)
        until(lambda: len(log) > mark and log.endswith(b'\x1b[?2026l'), f'redraw at {c}x{r}')

    until(lambda: text('ASSISTANT') and text('g1'), 'assistant workbench ready')
    mark = len(log); send(b'/handoff\r')
    until(lambda: text('Hand off', mark) and text('Work on this here', mark), 'handoff sheet')
    frame('sheet-140')
    for cols, rows, name in [(80, 34, 'sheet-80'), (52, 24, 'sheet-52'), (52, 14, 'sheet-52-short')]:
        resize(cols, rows); frame(name)
    resize(140, 46)
    before = len(calls()); mark = len(log); send(b'\x1b')
    until(lambda: text('nothing changed', mark), 'cancel reports no change')
    cancelled = calls()[before:]
    assert not any(k in ('owner_input', 'handoff_choice') for k in kinds(cancelled)), cancelled
    frame('cancelled')
    # Board work has no context rows: focus starts on Save; Down then Enter = Work on this here.
    mark = len(log); send(b'/handoff\r')
    until(lambda: text('Work on this here', mark), 'sheet reopened')
    before = len(calls()); send(b'\x1b[B'); send(b'\r')
    until(lambda: 'handoff_choice' in kinds(calls()[before:]), 'consumer called')
    after = calls()[before:]
    owner = [c for c in after if c['payload'].get('kind') == 'owner_input']
    choice = [c for c in after if c['payload'].get('kind') == 'handoff_choice']
    assert len(owner) == 1 and owner[0]['payload']['source'] == 'choice' \
        and owner[0]['payload']['text'] == 'work_on_this_here' and owner[0]['payload']['binding'] == 'd' * 64, owner
    assert len(choice) == 1 and choice[0]['payload']['native'] == dict(action='take_back', task='T-9', contract_revision='c1'), choice
    assert kinds(after).index('owner_input') < kinds(after).index('handoff_choice')
    frame('selected')
    send(b'\x15\x04'); until(lambda: b'\x1b[?1049l' in log, 'terminal restored'); proc.wait(timeout=10)
    result = dict(status='PASS', production_entrypoint='src/host/main.ts', assets=installed['assets'],
                  provider='offline fake corral; no model requests', checks=[
                      '/handoff opens the goal-aware sheet in a bound assistant host',
                      'renders at 140, 80, 52 columns and 52x14 short height',
                      'Escape journals nothing and calls no consumer',
                      'keyboard selection journals exactly one source=choice input bound to the binding digest',
                      'host-only handoff_choice cites that input with the exact take_back payload, after journaling'],
                  not_covered=['pointer path in PTY (unit-tested)', 'real corral service consumption (Rust native_tests)'])
    (args.output / 'result.json').write_text(json.dumps(result, indent=2)); print(json.dumps(result, indent=2))
finally:
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (args.output / 'pty-final.ansi').write_bytes(log)
    selector.close()
    for fd in [master, slave]:
        if fd is not None: os.close(fd)
    shutil.rmtree(work)
