#!/usr/bin/env python3
"""Owned production-host PTY: disposable REAL corral store and the real worktree corral
binary for the catalog (`run inspect --artifacts`) and viewers (`artifact show|serve`); only host
binding calls go to the fixture corral. Real keyboard/resize input.
No provider calls, owner panes, scheduling operations, or live board state.
"""
import argparse
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
parser.add_argument('--graphics', choices=('kitty', 'none'), default='none')
parser.add_argument('--fail-after', metavar='SCENE', help='raise after this scene to exercise failure-path cleanup')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]
package = repo / 'packages/corral-pi'
work = Path(tempfile.mkdtemp(prefix='artifacts-real-pty-'))
proc = None
master = slave = None
log = bytearray()
scenes = []
resizes = []
size = dict(columns=140, rows=46)
# The status bar's role badge, then the binding dot (● bound, ○ not bound), possibly with styling between.
BOUND = re.compile(rb'\xe2\x96\x8cWORKER(?:\x1b\[[\d;:]*m| )*\xe2\x97\x8f')
try:
    env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'CORRAL_', 'PI_')) and k not in ('TERM_PROGRAM', 'KITTY_WINDOW_ID')}
    env['PI_IMAGE_PROTOCOL'] = args.graphics
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-256color')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    import hashlib, shutil as _shutil
    cargo_env = dict(os.environ)
    subprocess.run(['cargo', 'build', '-p', 'corral'], cwd=repo, env=cargo_env, check=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
    real = repo / 'target/debug/corral'
    worktree = work / 'worktree'; worktree.mkdir()
    git = ['git', '-c', 'user.name=QA', '-c', 'user.email=qa@example.invalid', '-C', str(worktree)]
    subprocess.run(git + ['init', '-q'], check=True); (worktree / 'README').write_text('qa\n')
    subprocess.run(git + ['add', 'README'], check=True); subprocess.run(git + ['commit', '-qm', 'qa'], check=True)
    head = subprocess.check_output(git + ['rev-parse', 'HEAD'], text=True).strip()
    inputs = work / 'inputs'; inputs.mkdir()
    (inputs / 'my report.md').write_text('# Real registered report\nREAL_REPORT_BODY\n')
    _shutil.copyfile(work / 'zoom.png', inputs / 'résumé.png')
    (inputs / 'page.html').write_text('<!doctype html><title>page</title><p>REAL_HTML</p>\n')
    (inputs / 'gone.log').write_text('removed blob\n'); (inputs / 'other.md').write_text('# Other run\n')
    built = subprocess.check_output(['cargo', 'test', '-p', 'corral-ops', '--lib', 'artifact_gallery::tests::qa_real_store', '--', '--ignored', '--exact', '--nocapture'],
                                    cwd=repo, env={**cargo_env, 'CORRAL_GALLERY_QA_ROOT': str(work), 'CORRAL_GALLERY_QA_WORKTREE': str(worktree), 'CORRAL_GALLERY_QA_HEAD': head},
                                    stdin=subprocess.DEVNULL, stderr=subprocess.STDOUT, text=True)
    built = json.loads(next(line for line in built.splitlines() if line.startswith('CORRAL_GALLERY_QA ')).split(' ', 1)[1])
    task = built['task']
    (work / 'wrapper').mkdir()
    wrapper = work / 'wrapper/corral'
    wrapper.write_text(f"""#!/usr/bin/env python3
import json, os, sys
argv = sys.argv[1:]
rest = argv[2:] if argv[:1] == ['--root'] else argv
real = rest[:2] == ['run', 'inspect'] or rest[:2] in (['artifact', 'show'], ['artifact', 'serve'])
with open({str(work / 'routed.jsonl')!r}, 'a') as log: log.write(json.dumps(dict(real=real, argv=argv)) + '\\n')
target = {str(real)!r} if real else {installed['corral']!r}
os.execv(target, [target, *argv])
""")
    wrapper.chmod(0o700)
    opener = work / 'opener'
    opener.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$*\" >> {work / 'opener.log'}\n"); opener.chmod(0o700)
    env['CORRAL_OPENER'] = str(opener)
    launch = dict(version=1, store='st-0123456789abcdef', project='demo', role='worker', task=task, run='R1', generation=1,
                  pane='private-pty', root=str(work), cwd=str(work), corral=str(wrapper), assets=installed['assets'],
                  runtime_dir=str(work / 'run'), session_dir=str(work / 'sessions'), model='gpt-6.1-sol', ui='rich',
                  secrets=dict(host_cap='h'*64, ctl_cap='c'*64, bind_nonce='n'*64))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    read_fd, write_fd = os.pipe()
    def setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(installed['assets']) / 'src/host/main.ts'), '--launch-fd', str(read_fd)], cwd=work, env=env,
                            stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=setup)
    os.close(slave); slave = None; os.close(read_fd)
    os.write(write_fd, json.dumps(launch).encode()); os.close(write_fd)
    selector = selectors.DefaultSelector(); selector.register(master, selectors.EVENT_READ)
    def seen(text, mark):
        return text.search(log, mark) is not None if isinstance(text, re.Pattern) else text.encode() in log[mark:]
    def label(text): return text.pattern.decode(errors='replace') if isinstance(text, re.Pattern) else text
    def until(text, mark=0):
        deadline = time.monotonic() + 20
        while not seen(text, mark):
            events = selector.select(max(0, deadline - time.monotonic()))
            if not events: raise AssertionError('Timeout waiting for ' + label(text))
            try: chunk = os.read(master, 65536)
            except OSError: chunk = b''
            if not chunk: raise AssertionError(f'Host exited (code {proc.poll()}) before {label(text)!r}; last output: {bytes(log[-600:])!r}')
            log.extend(chunk)
    def send(data): os.write(master, data)
    def scene(name, text, data=None, resize=None):
        mark = len(log)
        if resize:
            columns, rows = resize
            resizes.append(dict(offset=mark, columns=columns, rows=rows))
            size.update(columns=columns, rows=rows)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
            os.kill(proc.pid, signal.SIGWINCH)
        if data: send(data)
        until(text, mark)
        while not log.endswith(b'\x1b[?2026l'):
            if not selector.select(10): raise AssertionError('Incomplete terminal frame: ' + name)
            log.extend(os.read(master, 65536))
        (args.output / (name + '.ansi')).write_bytes(log)
        scenes.append(dict(name=name, offset=mark, bytes=len(log), columns=size['columns'], rows=size['rows'], input_hex=data.hex() if data else None, resize=resize, asserted=text))
        if name == args.fail_after: raise AssertionError('injected failure after ' + name)
    until(BOUND)
    def routed(): return [json.loads(line) for line in (work / 'routed.jsonl').read_text().splitlines()]
    def real_calls(kind): return [c['argv'] for c in routed() if c['real'] and kind in c['argv']]
    scene('real-evidence', 'Proof', b'\x1b[21~')
    scene('real-artifacts', 'Evidence: unknown · run R2 evidence not read', b'a')
    assert ('Bound worktree HEAD ' + head).encode() in log, 'real bound worktree HEAD shown'
    assert real_calls('inspect') and not real_calls('artifact'), routed()
    scene('real-absent', 'ABSENT: no snapshot file or live blob', b'j')
    scene('real-absent-open', 'Unavailable · not opened', b'\r')
    scene('real-html-selected', 'page.html · report · run R1', b'j')
    assert not real_calls('artifact'), 'selection must not start a viewer'
    scene('real-html-browser', 'Browser requested', b'o')
    scene('real-report-selected', 'differs from bound HEAD at read', b'j')
    scene('real-report-opened', 'REAL_REPORT_BODY', b'\r')
    scene('real-image-selected', 'same SHA as bound HEAD at read', b'j')
    if args.graphics == 'none':
        scene('real-image-fallback', 'Graphics unavailable in this terminal', b'\r')
    else:
        # The expanded image's footer offers F9 back to chat; the artifacts footer does not.
        scene('real-image-viewer', 'chat', b'\r')
        scene('real-image-closed', 'Bytes at open match the registered SHA256', b'\x1b')
    assert b'Bytes at open match the registered SHA256' in log
    scene('real-mark-a', 'Selected for comparison: A', b'm')
    send(b'k')
    scene('real-mark-b', ' · B ' + task + ':R1:', b'm')
    scene('real-compare', 'Registered bytes differ (different SHA256)', b'c')
    scene('real-compact-compare', 'Title B: my report.md', resize=(52, 30))
    viewers = [c['argv'][2:] for c in routed() if c['real'] and 'artifact' in c['argv']]
    assert [(v[1], v[2].rsplit(':', 1)[0]) for v in viewers] == [('serve', task + ':R1'), ('show', task + ':R1'), ('show', task + ':R1')], viewers
    assert viewers[1][2].endswith('-my report.md') and viewers[2][2].endswith('-résumé.png'), viewers
    opened = (work / 'opener.log').read_text().split()
    assert len(opened) == 1 and opened[0].startswith('http://127.0.0.1:'), opened
    assert not [c for c in routed() if not c['real'] and 'artifact' in c['argv']]
    calls = [json.loads(json.loads(line)['stdin']) for line in (work / 'calls.jsonl').read_text().splitlines() if json.loads(line)['stdin']]
    assert not [c for c in calls if c['op'] not in ('hello', 'event')], calls
    assert [c['op'] for c in calls].count('hello') == 1, 'exactly one host launched and bound'
    scene('return-conversation', 'Chat', b'\x1b[20~')
    send(b'\x04'); until('\x1b[?1049l'); proc.wait(timeout=10)
    assert proc.returncode == 0
    real_sha = hashlib.sha256(real.read_bytes()).hexdigest()
    (args.output / 'result.json').write_text(json.dumps(dict(status='PASS', candidate=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        initial_size=dict(columns=140, rows=46), resizes=resizes, graphics=args.graphics,
        source='disposable real corral store built by corral-ops artifact_gallery::tests::qa_real_store through add_artifact_to_run; catalog and viewers served by the candidate corral binary; production host/main.ts and NativeWorkbench',
        real_binary=dict(path=str(real), sha256=real_sha), store=built['store'], task=task, worktree_head=head,
        registered=dict(image=built['image'], report=built['report']), routed_calls=routed(), opener=opened,
        checks=['real registered artifacts listed through the real --artifacts catalog', 'real report opened only on Enter through real corral artifact show',
                'blob-backed real image opened from the decoded copy only on Enter with hash verified (viewer or text fallback)',
                'HTML opened only on o through real corral artifact serve and corral opener route', 'absent blob shown unavailable',
                'unknown other-run evidence, historical and same-SHA evidence candidates labeled', 'explicit A/B comparison of real artifacts',
                'Unicode and space artifact names opened', 'real PTY keyboard input at 140 and 52 columns'], scenes=scenes), indent=2))
    print('PASS: owned production-host PTY over a disposable real store; real catalog and viewers; viewers only on explicit keys')
finally:
    if 'real' in dir() and 'env' in dir():
        daemons = []
        for pid in filter(str.isdigit, os.listdir('/proc')):
            try:
                cmdline = Path(f'/proc/{pid}/cmdline').read_bytes()
                if b'serve\0--daemon' in cmdline and str(work).encode() in cmdline: daemons.append(os.pidfd_open(int(pid)))
            except OSError:
                continue
        subprocess.run([str(real), '--root', str(work), 'artifact', 'serve', '--stop'], env=env, stdin=subprocess.DEVNULL, capture_output=True)
        # `serve --stop` returns before the daemon exits; wait for each exit event (hang guard only).
        with selectors.DefaultSelector() as exited:
            for fd in daemons: exited.register(fd, selectors.EVENT_READ)
            deadline = time.monotonic() + 30
            while exited.get_map() and exited.select(max(0, deadline - time.monotonic())):
                for key, _ in exited.select(0): exited.unregister(key.fileobj); os.close(key.fileobj)
            for fd in list(exited.get_map()): os.close(fd)
    (args.output / 'pty-session.ansi').write_bytes(log)
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=5)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    for fd in (master, slave):
        if fd is not None: os.close(fd)
    # Audit every process still holding the private directory (host, viewer server, helpers).
    leftovers = []
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try:
            cmdline = Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\0', b' ').decode('utf8', 'replace')
            cwd = os.readlink(f'/proc/{pid}/cwd')
        except OSError:
            continue
        if int(pid) != os.getpid() and (str(work) in cmdline or cwd.startswith(str(work))):
            leftovers.append(dict(pid=int(pid), cmdline=cmdline[:300]))
    (args.output / 'cleanup.json').write_text(json.dumps(dict(work=str(work), host_reaped=proc is None or proc.poll() is not None, leftovers=leftovers), indent=2))
    print('CLEANUP: ' + ('clean' if not leftovers and (proc is None or proc.poll() is not None) else f'LEAK {leftovers}'))
    shutil.rmtree(work, ignore_errors=True)
