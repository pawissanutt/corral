#!/usr/bin/env python3
"""Production-host PTY proof: bound Chat startup, F3 filtering, Tab focus and persistent run-end rows.

Offline fixture I/O only; owned Git changes and background jobs still exercise the real workbench.
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

parser = argparse.ArgumentParser()
parser.add_argument('output', type=Path)
parser.add_argument('--activity-only', action='store_true', help='Exercise the agents view and jobs without candidate-diff fixtures')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='cpn-'))
master = slave = watch = None
proc = None
selector = selectors.DefaultSelector()
log = bytearray()
# Bound role, explicit binding word and full-width generation; ANSI styling may separate each cell.
BOUND = re.compile(rb' WORKER (?:\x1b\[[\d;:]*m| )*\xe2\x97\x8f(?:\x1b\[[\d;:]*m)* bound(?:\x1b\[[\d;:]*m)* \xc2\xb7 (?:\x1b\[[\d;:]*m)*gen 1\b')
# The Changes comparison row's label: the comparison's name, then its ▾. A task-bound session opens on the committed
# candidate, whose row always shows (its exact base and head stay on screen); other comparisons name themselves too.
CHIP = re.compile(rb'(candidate|since session|working tree|unstaged|staged) \xe2\x96\xbe')
# The Changes list's path-search row: a blank cell, `/` in muted, then the query in the text colour (styling between).
QUERY = re.compile(rb' (?:\x1b\[[\d;:]*m)*/(?:\x1b\[[\d;:]*m)*notification(?:\x1b\[[\d;:]*m)* ')
SGR = re.compile(rb'\x1b\[[\d;:]*m')
# The composer box's top border, `╭─ Message` up to its `╮` on the same row; the working indicator sits near its right corner.
TOP = re.compile(rb'\xe2\x95\xad\xe2\x94\x80 (?:\x1b\[[\d;:]*m)*Message(?:(?!\x1b\[\d+;\d+H)[^\r\n])*?\xe2\x95\xae')
# The indicator while a turn runs (from 105 columns): spinner frame, `Working`, what the agent does, the phase's elapsed time.
WORKING = re.compile(r'[⠋⠙⠹⠸⠼⠴⠦⠧●] Working · [^·]+ · \d+(?:s|m \d+s|h \d+m)\b')
# The job's completion notice, drawn as one row: `✗ job j1-1 exited 7 · <size> output` (a nonzero exit is a failure).
JOB_NOTICE = re.compile(rb'\xe2\x9c\x97 (?:\x1b\[[\d;:]*m)*job j1-1 exited 7\b')
# The backgrounded command's own row once its job exits: `✗ bash printf 'BUILD_WAIT (+2 lines) · exit 7`, the reason after `·`.
FAILED_ROW = re.compile(rb'\xe2\x9c\x97 (?:\x1b\[[\d;:]*m)*bash (?:\x1b\[[\d;:]*m)*printf \'BUILD_WAIT[^\x1b]*(?:\x1b\[[\d;:]*m)* \xc2\xb7 (?:\x1b\[[\d;:]*m)*exit 7\b')
try:
    env = {key: value for key, value in os.environ.items() if not key.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    for command in [['init', '-q'], ['config', 'user.name', 'Native TUI fixture'], ['config', 'user.email', 'fixture@example.invalid']]:
        subprocess.run(['git', '-C', str(work), *command], check=True, env=env)
    (work / '.gitignore').write_text('*\n!.gitignore\n!notification.ts\n!new-file.ts\n')
    os.mkfifo(work / 'monitor-gate'); os.mkfifo(work / 'exit-gate'); os.mkfifo(work / 'indicator-gate')
    candidate = work / 'notification.ts'
    candidate.write_text('export const quietHours = false;\n')
    subprocess.run(['git', '-C', str(work), 'add', 'notification.ts'], check=True, env=env)
    subprocess.run(['git', '-C', str(work), '-c', 'core.hooksPath=/dev/null', 'commit', '-qm', 'fixture base'], check=True, env=env)
    candidate.write_text('export const quietHours = true;\n')
    launch = dict(version=1, store='st-0123456789abcdef', project='demo', role='worker', task='T-001', run='R1',
                  generation=1, pane='private-pty', root=str(work), cwd=str(work), corral=installed['corral'],
                  assets=installed['assets'], runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'),
                  model='gpt-6.1-sol', ui='rich', capabilities=dict(catalog=2,digest='b'*64,preset='standard',enabled=['background_shell'],project_text=False,settings=dict(background_shell=dict(max_jobs=2,max_output_bytes=8192,max_runtime_secs=60))), secrets=dict(host_cap='h'*64, ctl_cap='c'*64, bind_nonce='n'*64))
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
                            cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave,
                            pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None
    os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    def calls():
        path = work / 'calls.jsonl'
        if not path.exists(): return []
        return [json.loads(json.loads(line)['stdin']) for line in path.read_text().splitlines(keepends=True) if line.endswith('\n')]
    def prompt_ends():
        path = work / 'ui-events.jsonl'
        return len(path.read_text().splitlines()) if path.exists() else 0
    def until(predicate, reason, timeout=12):
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
    def save_frame(name):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete synchronized terminal frame')
        (args.output / (name + '.ansi')).write_bytes(log)
    def text(value, after=0): return value.encode() in log[after:]
    def send(value): os.write(master, value)
    def resize(columns, rows):
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        os.kill(proc.pid, signal.SIGWINCH)
    def comparison(name, after=None):
        """`c` cycles the Changes comparison until the comparison row names `name`; returns the log mark of the last step.
        With `after`, the row is drawn after that mark (Changes just opened); without it, the row is already on screen
        and Pi's differential renderer does not write it again, so the last label drawn so far names the comparison."""
        if after is None: after = 0
        else: until(lambda: CHIP.search(log, after) is not None, 'Changes comparison row')
        shown = [match.group(1).decode() for match in CHIP.finditer(log, after)][-1]
        for _ in range(5):
            if shown == name: return after
            previous, after = shown, len(log)
            send(b'c')
            # A chip repainted before `c` lands still names the previous comparison; the first other name is the new one.
            until(lambda: any(match.group(1).decode() != previous for match in CHIP.finditer(log, after)), 'next comparison after c')
            shown = next(match.group(1).decode() for match in CHIP.finditer(log, after) if match.group(1).decode() != previous)
        raise AssertionError('comparison never reached ' + name)
    def composer_top(after):
        """The plain text of the last composer top border drawn after `after`, or None."""
        tops = TOP.findall(log, after)
        return SGR.sub(b'', tops[-1]).decode('utf-8', 'replace') if tops else None
    def turns_settled():
        kinds = [c['payload'].get('kind') for c in calls()]
        return kinds.count('turn_started') == kinds.count('turn_ended')

    until(lambda: BOUND.search(log) is not None, 'production workbench bound')
    send(b'Reply only NATIVE_PTY_OK.\r')
    until(lambda: any(call['payload'].get('kind') == 'turn_ended' for call in calls()), 'native response')
    # The working indicator: while a turn runs, the composer's top border animates (spinner frame, elapsed time); the
    # turn's end reads `✓ done · <time>` for the notice time, then the border is plain again. A reply held after its
    # first chunk keeps the turn running for as long as the check needs.
    held = work / 'indicator-held.log'
    (work / 'indicator-steps.json').write_text(json.dumps([dict(chunks=['INDICATOR_HELD', ' INDICATOR_RELEASED'],
                                                                hold=dict(gate=str(work / 'indicator-gate'), mark=str(held), label='indicator'))]))
    send(b'/fixture-script ' + str(work / 'indicator-steps.json').encode() + b'\r')
    mark=len(log); send(b'Hold the reply.\r')
    until(lambda: held.exists() and text('INDICATOR_HELD', mark), 'reply held after its first chunk')
    mark=len(log)
    until(lambda: WORKING.search(composer_top(mark) or '') is not None, 'the composer border shows the working indicator')
    first, since, mark = composer_top(mark), time.monotonic(), len(log)
    until(lambda: time.monotonic() - since >= 0.3 and composer_top(mark) is not None, 'the composer border is redrawn 300 ms later')
    second = composer_top(mark)
    assert WORKING.search(second) and second != first, ('the working indicator did not move in 300 ms', first, second)
    save_frame('indicator-working')
    mark=len(log)
    with open(work / 'indicator-gate', 'w') as gate: gate.write('go\n')
    until(lambda: text('INDICATOR_RELEASED', mark) and turns_settled(), 'released reply ends its turn')
    until(lambda: re.search(r'✓ done · \d', composer_top(mark) or '') is not None, 'turn end: the border reads ✓ done with the time')
    until(lambda: re.search(rb' \xe2\x9c\x93 done \xc2\xb7 [^\x1b]*\b\d{2}:\d{2}\b', SGR.sub(b'', log[mark:])) is not None,
          'the transcript keeps a quiet run-end row with its clock time, separate from the composer indicator')
    mark=len(log)
    until(lambda: re.fullmatch(r'╭─ Message ─+╮', composer_top(mark) or '') is not None, 'idle: the border has no indicator')
    save_frame('indicator-idle')
    send(b'Keep native draft')
    until(lambda: text('Keep native draft'), 'composer draft')
    save_frame('conversation')
    if not args.activity_only:
        # F7 opens Changes on the committed candidate: its row reads `candidate ▾  <base> → <head> …`.
        mark=len(log); send(b'\x1b[18~')
        mark=comparison('working tree', mark)
        until(lambda: text('quietHours',mark), 'persistent Changes with real diff')
        save_frame('changes-wide')
        closed=prompt_ends(); send(b'\x1bOR')  # F3 filters; slash always starts a command.
        until(lambda: text('Filter changed paths'), 'native path search')
        mark=len(log); send(b'notification\r')
        until(lambda: prompt_ends()>closed and QUERY.search(log,mark) is not None and text('notification.ts',mark) and text('Keep native draft',mark) and log.endswith(b'\x1b[?2026l'), 'filtered change list (the /query row, then the matching file) and restored editor')
        save_frame('search')
        # External edit should update data without replacing the currently inspected hunk.
        candidate.write_text('export const quietHours = "updated";\n')
        until(lambda: text('New diff available'), 'external-change indicator')
        mark=len(log); send(b'r')
        until(lambda: text('"updated"',mark), 'apply refreshed patch')
        send(b'a')
        until(lambda: text('Please review'), 'hunk reference appended, not submitted')
        save_frame('ask-hunk')
        send(b'\x03')  # Stock Pi clear, not a fixture-only editor.
        mark=len(log); send(b'Keep native draft')
        until(lambda: text('Keep native draft',mark), 'draft restored for resize proof')
        mark=len(log); resize(80,34)
        until(lambda: text('Keep native draft',mark) and log.endswith(b'\x1b[?2026l'), 'narrow Changes and real composer')
        save_frame('changes-narrow')
        mark=len(log); resize(52,24)
        until(lambda: text('Keep native draft',mark) and log.endswith(b'\x1b[?2026l'), 'small Changes')
        save_frame('changes-small')
        # Switch to unstaged at 52 columns: its diff is the same text as the working-tree one, so only the
        # full repaint of the 52→140 resize shows it again. The diff view titles a hunk by its count, not
        # by its raw @@ header, so the wait reads the changed line itself. Changes already shows the
        # working-tree row from the 52-column repaint; F7 keeps Changes, and each `c` redraws the row.
        send(b'\x1b[18~')
        comparison('unstaged')
        mark=len(log); resize(140,46)
        until(lambda: text('Keep native draft',mark) and text('unstaged ▾',mark) and text('"updated"',mark) and log.endswith(b'\x1b[?2026l'), 'wide draft continuity and the loaded unstaged comparison')
        save_frame('comparison')
    # Return to actual editor and submit one scripted offline provider sequence.
    send(b'\t\x15/fixture-workbench-jobs\r')
    send(b'Start the owned background scenario.\r')
    until(lambda: any(call.get('op')=='job_monitor' or call['payload'].get('on')=='line' for call in calls()), 'line monitor registration')
    mark=len(log); send(b'\x1b[19~')  # F8: the agents view; with no helper calls its first entry is the job row.
    until(lambda: text('Commands',mark) and text('MONITOR_READY/ armed',mark) and text('BUILD_WAIT',mark), 'command row selected: preview and armed watch')
    save_frame('activity-running')
    with open(work/'monitor-gate','w') as gate: gate.write('ready\n')
    until(lambda: text('MONITOR_READY/ matched',mark), 'watch match in the job detail')
    save_frame('activity-matched')
    mark=len(log); send(b'\r')
    until(lambda: text('‹ Agents',mark) and text('following tail',mark), 'Enter opens the live command page')
    save_frame('job-output')
    mark=len(log); send(b'\x1b')
    until(lambda: text('Commands',mark) and text('MONITOR_READY/ matched',mark), 'Esc returns to the selected command preview')
    mark=len(log); started=[c['payload'].get('kind') for c in calls()].count('turn_started')
    with open(work/'exit-gate','w') as gate: gate.write('done\n')
    until(lambda: text('exit 7',mark) and text('exited',mark), 'actual failed process outcome, not command text')
    save_frame('activity-exited')
    # The exit reaches the agent as a completion notice that starts its own turn; the transcript draws it as one row
    # (`✗ job j1-1 exited 7 · <size> output`), not as a `you` bubble. Let that turn end before typing.
    until(lambda: JOB_NOTICE.search(log, mark) is not None and [c['payload'].get('kind') for c in calls()].count('turn_started') > started
          and turns_settled(), 'completion notice row drawn and its turn ends')
    send(b'\x1b[20~')  # Always return to Conversation, independently of composer focus.
    # The backgrounded command's row absorbed the exit: a failed call reads ✗ with its reason after `·`.
    until(lambda: FAILED_ROW.search(log, mark) is not None, 'the job_start row reads ✗ bash … · exit 7')
    save_frame('failed-row')
    send(b'\x15/fixture-native-confirm\r')
    until(lambda: text('Native permission fixture'), 'permission dialog owns input')
    save_frame('permission')
    send(b'\x1b')
    until(lambda: text('NATIVE_CANCELLED'), 'permission cancellation')
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
    if proc.returncode != 0: raise AssertionError('host exit '+str(proc.returncode))
    (args.output/'pty-session.ansi').write_bytes(log)
    result=dict(status='PASS',production_entrypoint='src/host/main.ts',provider='offline fixtures; real owned Git and jobs',
                checks=['native real composer', 'quiet transcript run-end row with clock time after the held reply',
                        'working indicator in the composer border moves within 300 ms while a turn runs, reads ✓ done at its end, then is absent when idle',
                        'F8 Agents selects the command row; Enter opens its command page', 'real background job output', 'armed and matched watch in the job detail', 'exit code 7, not task/gate PASS',
                        'job completion notice drawn as one row (✗ job j1-1 exited 7)', 'failed job_start row reads ✗ with its reason (exit 7)', 'permission cancellation', 'clean terminal exit']
                        + ([] if args.activity_only else ['persistent Changes, not modal', 'native path search', 'external refresh preserves hunk until applied', 'hunk reference without autosend', '140→80→52→140 draft retention', 'explicit comparison']))
    (args.output/'result.json').write_text(json.dumps(result,indent=2)); print(json.dumps(result,indent=2))
finally:
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (args.output / 'pty-final.ansi').write_bytes(log)
    selector.close()
    for fd in [master, slave, watch]:
        if fd is not None: os.close(fd)
    shutil.rmtree(work)
