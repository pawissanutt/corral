#!/usr/bin/env python3
"""Drive the real production host/main.ts in an owned PTY. Offline fixture I/O only."""
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
work = Path(tempfile.mkdtemp(prefix='cpn-'))
master = slave = watch = None
proc = None
selector = selectors.DefaultSelector()
log = bytearray()
try:
    env = {key: value for key, value in os.environ.items() if not key.startswith(('HERDR_', 'CORRAL_', 'PI_'))}
    env.update(HOME=str(work / 'home'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'),
               XDG_STATE_HOME=str(work / 'state'), XDG_CACHE_HOME=str(work / 'cache'), TERM='xterm-kitty', KITTY_WINDOW_ID='owned-fixture')
    installed = json.loads(subprocess.check_output(['bun', '--no-env-file', str(package / 'test/support/prepare-workbench.ts'), str(work)], cwd=repo, env=env))
    for command in [['init', '-q'], ['config', 'user.name', 'Native TUI fixture'], ['config', 'user.email', 'fixture@example.invalid']]:
        subprocess.run(['git', '-C', str(work), *command], check=True, env=env)
    (work / '.gitignore').write_text('*\n!.gitignore\n!notification.ts\n!new-file.ts\n')
    os.mkfifo(work / 'monitor-gate'); os.mkfifo(work / 'exit-gate')
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


    import re
    until(lambda: text('Bound to Corral') and text('WORKER'), 'native editor ready')
    send(b'/fixture-workbench-image\r')
    send(b'Read the owned image.\r')
    until(lambda: any(call['payload'].get('kind')=='turn_ended' for call in calls()), 'actual image read completes')
    def placement():
        cursor=(1,1); found=None
        token=re.compile(rb'\x1b\[(\d+);(\d+)H|\x1b_G([^\x1b]*)\x1b\\')
        for m in token.finditer(bytes(log)):
            if m.group(1): cursor=(int(m.group(1)),int(m.group(2)))
            elif b'a=p' in m.group(3) or b'a=T' in m.group(3):
                found=cursor
        return found
    until(lambda: placement() is not None, 'native Kitty image placement')
    save_frame('image-inline')
    row,col=placement()
    send(f'\x1b[<0;{col+2};{row+1}M\x1b[<0;{col+2};{row+1}m'.encode())
    until(lambda: text('click / Esc close'), 'inline image mouse click expands image')
    save_frame('image-expanded')
    send(b'\x1b[<0;40;23M\x1b[<0;40;23m')
    def image_columns():
        matches=re.findall(rb'\x1b_Ga=[Tp],[^\x1b;]*?c=(\d+)',bytes(log))
        return int(matches[-1]) if matches else None
    until(lambda: image_columns()==60, 'second image click restores original inline image')
    save_frame('image-closed')
    send(b'\x15\x04')
    until(lambda: b'\x1b[?1049l' in log, 'terminal restored')
    until(lambda: proc.poll() is not None, 'host process exits after terminal restoration')
    if proc.returncode!=0:raise AssertionError('host exit '+str(proc.returncode))
    result=dict(status='PASS',entrypoint='src/host/main.ts',backend='offline provider; actual read tool and Kitty protocol fixture',
                checks=['real tool image placement','pointer opens expanded image','second pointer click closes','clean terminal exit'],
                limitation='Bitmap protocol and geometry verified; plain xterm evidence replay does not rasterize Kitty graphics.')
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
