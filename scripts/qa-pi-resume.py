#!/usr/bin/env python3
"""Owned production Pi PTY: an assistant's /resume picker over a fixture session list (T-728).
No model calls, owner panes or herdr. The host's binding/events use the offline fake adapter and
`assistant list --json` is answered from a fixture. The supervisor half of the switch (exit 77 ->
claim, revalidate, resume) is covered by the Rust pi_switch_tests.
Usage: python3 scripts/qa-pi-resume.py <output dir>; then bun scripts/qa/pi-panel-screenshots.mjs <output dir> --resume
"""
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
import sys
import tempfile
import termios
import time

output = Path(sys.argv[1]).resolve()
output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='resume-pty-'))
proc = None
master = slave = None
log = bytearray()
scenes = []
resizes = []
size = dict(columns=140, rows=46)
# The status bar: the role pill ` ASSISTANT `, then the binding dot and generation (`● g1` bound, `○` not), styling between.
BOUND = re.compile(rb' ASSISTANT (?:\x1b\[[\d;:]*m| )*\xe2\x97\x8f(?:\x1b\[[\d;:]*m)* g1\b')
SESSIONS = [
    dict(session='A-1', ended=None, live=True, focus='workspace', summary='this conversation', last_activity='2026-11-06T10:00:00Z'),
    dict(session='A-2', ended=None, live=False, focus='workspace', summary='storage spike: compare sqlite page sizes', last_activity='2026-11-06T09:00:00Z'),
    dict(session='A-3', ended='2026-11-05T00:00:00Z', live=False, focus='T-12', summary='', last_activity='2026-11-05T00:00:00Z'),
    dict(session='A-4', ended=None, live=True, focus='workspace', summary='running in another pane', last_activity='2026-11-06T08:00:00Z'),
    dict(session='A-5', ended=None, live=False, focus='workspace', summary='x' * 120, last_activity='2026-11-06T07:00:00Z'),
    dict(session='A-6', ended=None, live=False, focus='workspace', summary='会' * 40, last_activity='2026-11-06T06:00:00Z'),
]
try:
    env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)],
                                                   cwd=repo, env=env, stdin=subprocess.DEVNULL))
    (work / 'assistants.json').write_text(json.dumps(SESSIONS))
    proxy = work / 'resume-corral'
    proxy.write_text('''#!/usr/bin/env bun
import { spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
const args = process.argv.slice(2);
if (args[0] === "--root") {
  appendFileSync(%s, JSON.stringify(args) + "\\n");
  if (args.slice(2).join(" ") !== "assistant list --json") { process.stderr.write("fixture reads only assistant list\\n"); process.exit(1); }
  process.stdout.write(readFileSync(%s, "utf8")); process.exit(0);
}
const result = spawnSync(%s, args, { input: await Bun.stdin.text(), encoding: "utf8", maxBuffer: 4 * 1024 * 1024 });
process.stdout.write(result.stdout ?? ""); process.stderr.write(result.stderr ?? ""); process.exitCode = result.status ?? 1;
''' % tuple(json.dumps(str(p)) for p in [work / 'list-reads.jsonl', work / 'assistants.json', installed['corral']]))
    proxy.chmod(0o700)
    launch = dict(version=1, store='st-0123456789abcdef', project='', role='assistant', assistant='A-1', generation=1,
                  pane='private-pty', root=str(work), cwd=str(work), corral=str(proxy), assets=installed['assets'],
                  runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'), model='gpt-6.1-sol', ui='rich',
                  secrets=dict(host_cap='h'*64, ctl_cap='c'*64, bind_nonce='n'*64))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    read_fd, write_fd = os.pipe()
    def setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    # --assistant-new: the managed supervisor's flag that enables /new, relink and /resume.
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd), '--assistant-new'],
                            cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None; os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    selector = selectors.DefaultSelector(); selector.register(master, selectors.EVENT_READ)
    def pump(timeout):
        if not selector.select(timeout): return False
        try: chunk = os.read(master, 65536)
        except OSError: chunk = b''
        if not chunk: raise EOFError
        log.extend(chunk); return True
    def seen(text, mark):
        return text.search(log, mark) is not None if isinstance(text, re.Pattern) else text.encode() in log[mark:]
    def until(text, mark=0, seconds=20):
        deadline = time.monotonic() + seconds
        while not seen(text, mark):
            remaining = deadline - time.monotonic()
            if remaining <= 0: raise AssertionError('Timeout waiting for ' + (text.pattern.decode(errors='replace') if isinstance(text, re.Pattern) else text))
            try: pump(remaining)
            except EOFError: raise AssertionError('Host exited before ' + (text.pattern.decode(errors='replace') if isinstance(text, re.Pattern) else text))
    def settle():
        # A complete synchronized frame, then a short quiet period so later repaints are included.
        while not log.endswith(b'\x1b[?2026l'):
            if not pump(10): raise AssertionError('Incomplete terminal frame')
        while pump(0.3): pass
    def send(data): os.write(master, data)
    def scene(name, text, data=None, resize=None, mark=None):
        mark = len(log) if mark is None else mark
        if resize:
            columns, rows = resize
            resizes.append(dict(offset=mark, columns=columns, rows=rows))
            size.update(columns=columns, rows=rows)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
            os.kill(proc.pid, signal.SIGWINCH)
        if data: send(data)
        until(text, mark)
        settle()
        (output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, offset=mark, bytes=len(log), columns=size['columns'], rows=size['rows'],
                           input_hex=data.hex() if data else None, resize=resize, asserted=text))
    def resume_command():
        # Typed like the owner: the slash command, then Enter (a second Enter if autocomplete took the first).
        mark = len(log); send(b'/resume'); until('/resume', mark); settle()
        mark = len(log); send(b'\r')
        try: until('Resume an assistant session', mark, 5)
        except AssertionError: send(b'\r')
    def calls():
        return [json.loads(json.loads(line)['stdin']) for line in (work / 'calls.jsonl').read_text().splitlines()]
    until(BOUND); settle()
    scene('conversation', 'FAKE_TRANSPORT_OK', b'Reply to start a returnable conversation\r')
    mark = len(log); resume_command()
    scene('picker-wide', 'stopped · resumable', mark=mark)
    scene('picker-search', 'Resume an assistant session (1/6)', b'spike')
    before = calls()
    mark = len(log); send(b'\x1b'); settle()
    assert proc.poll() is None, 'cancel stopped the host'
    scene('picker-cancelled', 'RESUME_DRAFT_AFTER_CANCEL', b'RESUME_DRAFT_AFTER_CANCEL', mark=mark)
    assert not (work / 'run/resume-request.json').exists(), 'cancel wrote a switch request'
    assert calls() == before, 'cancel made a bridge call'
    send(b'\x15'); settle()  # Clear the draft line before the next command.
    mark = len(log)
    resizes.append(dict(offset=mark, columns=80, rows=24)); size.update(columns=80, rows=24)
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 0, 0)); os.kill(proc.pid, signal.SIGWINCH)
    settle(); resume_command()
    scene('picker-compact-80x24', 'Resume an assistant session (6/6)', mark=mark)
    scene('refused-live', 'running in another pane', b'A-4\r')
    assert not (work / 'run/resume-request.json').exists(), 'refusal wrote a switch request'
    assert proc.poll() is None, 'refusal stopped the host'
    mark = len(log); resume_command()
    scene('picker-select-a2', 'Resume an assistant session (1/6)', b'A-2', mark=mark)
    send(b'\r')
    deadline = time.monotonic() + 20
    while proc.poll() is None and time.monotonic() < deadline:
        try: pump(0.2)
        except EOFError: break
    proc.wait(timeout=10)
    assert proc.returncode == 77, f'host exited {proc.returncode}, not the supervisor /resume code 77'
    request = json.loads((work / 'run/resume-request.json').read_text())
    assert request == dict(version=1, assistant='A-1', generation=1, target='A-2'), request
    after = calls()
    # The assistant host refreshes its planning state read-only after hello; any other operation fails.
    assert all(c['op'] in ('hello', 'event') or (c['op'] == 'assistant_plan' and c['payload'].get('action') == 'read') for c in after), after
    assert all(c['identity']['generation'] == 1 and c['identity'].get('assistant') == 'A-1' for c in after)
    assert not any(c['op'] == 'event' and c['payload'].get('kind') == 'owner_input' and 'resume' in json.dumps(c['payload']) for c in after)
    kinds = [c['payload'].get('kind') for c in after if c['op'] == 'event']
    reads = (work / 'list-reads.jsonl').read_text()
    (output / 'list-reads.jsonl').write_text(reads)
    (output / 'switch-request.json').write_text(json.dumps(request, indent=2))
    (output / 'host-events.json').write_text(json.dumps(kinds, indent=2))
    result = dict(status='PASS', candidate=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
                  initial_size=dict(columns=140, rows=46), resizes=resizes,
                  source='offline fixture assistant list; production host/main.ts with --assistant-new; no provider calls',
                  calls_before_cancel=len(before), exit_code=proc.returncode,
                  checks=['/resume reaches the Corral picker, not the native file picker', 'search filters to one row',
                          'Esc cancels: host keeps running, editor accepts the draft, no switch request, generation 1 unchanged',
                          '80x24 rows keep activity and availability within the width (ASCII and CJK summaries)',
                          'live target refused visibly with no request', 'selecting stopped A-2 writes generation-scoped request and exits 77',
                          'only hello/event and read-only assistant_plan calls, all generation 1'],
                  scenes=scenes)
    (output / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != 'scenes'}, indent=2))
finally:
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (output / 'pty-final.ansi').write_bytes(log)
    for fd in [master, slave]:
        if fd is not None: os.close(fd)
    if os.environ.get('QA_KEEP'): print('kept fixture', work, file=sys.stderr)
    else: shutil.rmtree(work, ignore_errors=True)
