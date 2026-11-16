#!/usr/bin/env python3
"""Real-service /handoff acceptance in a PTY. Run inside scripts/test-herdr.sh's private server:
the production host/main.ts as a resumed generation 2 of a real store, the built corral binary as
its only control plane, real keyboard bytes, and an offline provider (no model request is made).
Proves exact r3-after-r4 staleness, cancel without effect, 140/80/52/short rendering, one native
Save that commits exactly one held task, and taking it back from the same sheet."""
import argparse, fcntl, json, os, pty, re, selectors, shutil, signal, struct, subprocess, tempfile, termios, time
from pathlib import Path

parser = argparse.ArgumentParser(); parser.add_argument('output', type=Path); args = parser.parse_args()
out = args.output.resolve(); out.mkdir(parents=True, exist_ok=True)
repo = Path(__file__).resolve().parents[1]; package = repo / 'packages/corral-pi'
home = os.environ.get('HOME', '')
if os.environ.get('CORRAL_HERDR_SESSION') != 'corral-test' or Path(home).name != 'home':
    raise SystemExit('requires scripts/test-herdr.sh private server')
target = Path(os.environ.get('CARGO_TARGET_DIR', repo / 'target')).resolve()
corral = str(target / 'debug/corral')
work = Path(tempfile.mkdtemp(prefix='handoff-live-', dir=Path(home).parent))
ESC = re.compile(rb'\x1b\[[0-9;?<=>]*[ -/]*[@-~]|\x1b[\]_P^][\s\S]*?(?:\x07|\x1b\\)|\x1b[()][\s\S]|\x1b.')
master = None; proc = None; workspace = None; selector = selectors.DefaultSelector(); log = bytearray()

def ok(cmd, **kw):
    r = subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL, **kw)
    if r.returncode: raise AssertionError(f'{cmd}: {r.stdout[-2000:]}{r.stderr[-2000:]}')
    return r.stdout

try:
    app = work / 'app'; app.mkdir()
    ok(['git', '-C', str(app), 'init', '-q', '-b', 'main'])
    (app / 'README.md').write_text('fixture\n'); (app / '.gitignore').write_text('.worktrees/\n')
    ok(['git', '-C', str(app), 'add', '.']); ok(['git', '-C', str(app), 'commit', '-qm', 'fixture'])
    assets = json.loads(ok(['bun', '--no-env-file', str(package / 'test/support/prepare-handoff-live.ts'), str(work)], cwd=repo))['assets']
    created = json.loads(ok(['herdr', '--session', 'corral-test', 'workspace', 'create', '--cwd', str(app), '--no-focus']))
    workspace = created['result']['workspace']['workspace_id']; pane = created['result']['root_pane']['pane_id']
    setup = ok(['cargo', 'test', '-p', 'corral-ops', '--lib', 'agent::ops::assistant::handoff_live_tests::handoff_live_setup',
                '--', '--ignored', '--exact', '--nocapture'], cwd=repo,
               env={**os.environ, 'RUSTC_WRAPPER': '', 'CORRAL_HANDOFF_LIVE': json.dumps(dict(root=str(work), assets=assets, pane=pane, corral=corral))})
    fixture = json.loads(re.search(r'HANDOFF_LIVE (\{.*\})', setup).group(1)); session = fixture['session']
    owner_env = {k: v for k, v in os.environ.items() if not k.startswith(('CORRAL_ASSISTANT', 'PI_'))}
    def record(): return json.loads(ok([corral, 'assistant', 'show', session, '--json'], cwd=work, env=owner_env))
    def phase(): return (record().get('workflow') or {}).get('phase')
    def scratches(): return [s['slug'] for s in record()['session']['scratch']]
    def tasks(): return json.loads(ok([corral, 'list', '--json'], cwd=work, env=owner_env))
    def choices():
        shown = json.loads(ok([corral, 'assistant', 'show', session, '--json'], cwd=work, env=owner_env))
        ids = set()  # workflow owner_evidence copies a consumed choice; count each input once
        def walk(v):
            if isinstance(v, dict):
                if v.get('source') == 'choice' and 'binding' in v: ids.add(v.get('id'))
                for x in v.values(): walk(x)
            elif isinstance(v, list):
                for x in v: walk(x)
        walk(shown); return len(ids)

    env = {k: v for k, v in owner_env.items() if not k.startswith('HERDR_PANE')}
    env.update(TERM='xterm-256color')
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    read_fd, write_fd = os.pipe(); selector.register(master, selectors.EVENT_READ)
    def child(): os.setsid(); fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    proc = subprocess.Popen(['bun', '--no-env-file', str(Path(assets) / 'src/host/main.ts'), '--launch-fd', str(read_fd)],
                            cwd=app, env=env, stdin=slave, stdout=slave, stderr=slave, pass_fds=(read_fd,), preexec_fn=child)
    os.close(slave); os.close(read_fd)
    os.write(write_fd, Path(fixture['launch']).read_bytes()); os.close(write_fd)

    def plain(after=0): return re.sub(r'\s+', ' ', ESC.sub(b' ', bytes(log[after:])).decode('utf8', 'replace'))
    def pump(timeout):
        if selector.select(timeout):
            try: chunk = os.read(master, 65536)
            except OSError: chunk = b''
            if not chunk: raise AssertionError('host exited:\n' + plain()[-3000:])
            log.extend(chunk)
    def until(pred, reason, timeout=30):
        deadline = time.monotonic() + timeout
        while not pred():
            left = deadline - time.monotonic()
            if left <= 0: raise AssertionError('timed out: ' + reason + '\n' + plain()[-3000:])
            pump(min(left, 0.5))
    def has(*words, after=0): return all(w in plain(after) for w in words)
    def frame(name):
        until(lambda: log.endswith(b'\x1b[?2026l'), 'complete frame ' + name)
        (out / (name + '.ansi')).write_bytes(log)
    def resize(c, r):
        mark = len(log)
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', r, c, 0, 0)); os.kill(proc.pid, signal.SIGWINCH)
        until(lambda: len(log) > mark and log.endswith(b'\x1b[?2026l'), f'redraw at {c}x{r}')

    until(lambda: has('ASSISTANT', 'gen 2'), 'assistant workbench ready', 90)
    mark = len(log); os.write(master, b'/handoff\r')
    until(lambda: has('Hand off', 'Continue planning', 'Plan r4', 'Local review r3', 'stale', 'current r4', after=mark), 'plan sheet', 60)
    frame('sheet-140')
    for cols, rows, name in [(80, 34, 'sheet-80'), (52, 24, 'sheet-52'), (52, 14, 'sheet-52-short')]:
        resize(cols, rows); frame(name)
    geometry = len(log); resize(140, 46)
    before = (tasks(), choices()); mark = len(log); os.write(master, b'\x1b')
    until(lambda: has('nothing changed', after=mark), 'cancel reports no change')
    assert (tasks(), choices()) == before == ([], 0), (tasks(), choices())
    frame('cancelled')

    # Streaming and a competing overlay at 52x14. A reply streams its first part and holds; the
    # owner scrolls back (reading position) and types a draft, opens the sheet from F2 over it,
    # moves and types inside it while the reply finishes, then cancels. Every screen is replayed
    # at 52x14 from that resize, so it observes the real short layout.
    draft = 'keep this draft'
    short = len(log); resize(52, 14)
    def settle(seconds=1.0):
        end = time.monotonic() + seconds
        while time.monotonic() < end: pump(0.1)
    def screen(name):
        settle(); (out / (name + '.ansi')).write_bytes(bytes(log[short:]))
        return json.loads(ok(['bun', '--no-env-file', str(package / 'test/support/screen-find.ts'), str(out / (name + '.ansi')), '52', '14', '--lines'], cwd=repo))
    def composer(lines): return next((l for l in lines if '│ ›' in l), '')
    def reading(lines): return [l for l in lines[2:] if 'Jump to latest' in l or 'new row' in l or 'STREAM_HOLD_1' in l]
    mark = len(log); os.write(master, b'STREAM_HOLD_1 summarise the plan\r')
    until(lambda: has('Streaming first part', after=mark), 'reply streaming', 60)
    os.write(master, b'\x1b[5~')
    for ch in draft.encode(): os.write(master, bytes([ch])); time.sleep(0.02)
    before = screen('short-before')
    assert draft in composer(before) and any('Jump to latest' in l for l in before) and 'writing' in ''.join(before), before
    mark = len(log); os.write(master, b'\x1bOQ')
    until(lambda: has('filter actions', after=mark), 'F2 palette')
    for ch in b'hand off': os.write(master, bytes([ch])); time.sleep(0.05)
    until(lambda: has('this work', after=mark), 'palette filtered')
    time.sleep(0.3); os.write(master, b'\r')
    until(lambda: has('next steps', after=mark), 'sheet from the palette', 60)
    os.write(master, b'\x1b[B'); time.sleep(0.15); os.write(master, b'zz')
    sheet = screen('short-sheet'); assert any('Context for the next session' in l for l in sheet), sheet
    (work / 'release-stream-1').write_text('')
    until(lambda: 'idle' in screen('short-released')[0], 'reply finished under the sheet', 30)
    os.write(master, b'\x1b'); time.sleep(2)
    after = screen('short-after')
    assert not any('Context for the next session' in l for l in after), after
    assert composer(after) == composer(before), (composer(before), composer(after))
    assert reading(after) and 'Streaming done' not in ''.join(after), ('reading position moved', before, after)
    os.write(master, b'!'); typed = screen('short-typed')
    assert draft + '!' in composer(typed), typed  # keys return to the composer
    assert (tasks(), choices()) == ([], 0), (tasks(), choices())
    os.write(master, b'\x1b[1;5F'); settle(0.5)  # Ctrl+End: End edits the draft
    assert any('Streaming done' in l for l in screen('short-latest')), 'reply completed'
    os.write(master, b'\x15'); time.sleep(0.3)
    geometry = len(log); resize(140, 46)
    until(lambda: has('ASSISTANT', 'Message', after=geometry), 'full redraw at 140x46')
    preserved = dict(geometry='52x14', composer=composer(after).strip('│ ').strip(), before=before, sheet=sheet, after=after)

    # Plan sheet rows: three contexts, Preview context, then Save plan for later.
    mark = len(log); os.write(master, b'/handoff\r')
    until(lambda: has('Save plan for later', after=mark), 'sheet reopened')
    for key in [b'\x1b[B'] * 3 + [b'\r']: os.write(master, key); time.sleep(0.15)
    until(lambda: has('Archived', 'loaded into the first prompt', 'Native fork: no', after=mark), 'read-only preview', 60)
    frame('preview'); assert (tasks(), choices()) == ([], 0), 'preview changed state'
    mark = len(log); os.write(master, b'q')
    until(lambda: has('Save plan for later', after=mark), 'sheet back after preview')
    for key in [b'\x1b[B'] * 4 + [b'\r']: os.write(master, key); time.sleep(0.15)
    until(lambda: len(tasks()) == 1 and has('local', after=mark), 'native Save committed a task', 90)
    frame('saved')
    task = tasks()[0]; shown = json.loads(ok([corral, 'show', task['id'], '--json'], cwd=work, env=owner_env))
    assert shown.get('dispatch_hold') is True and not shown.get('runs'), shown
    assert choices() == 1, choices()

    # Board surface on the same store: select the held card by pointer, N (continue planning)
    # by keyboard, then Save for later through the card menu by pointer; read back each delta.
    def held(): return json.loads(ok([corral, 'show', task['id'], '--json'], cwd=work, env=owner_env)).get('dispatch_hold')
    bmaster, bslave = pty.openpty(); blog = bytearray()
    fcntl.ioctl(bslave, termios.TIOCSWINSZ, struct.pack('HHHH', 46, 140, 0, 0))
    board = subprocess.Popen([corral, 'board'], cwd=work, env=env, stdin=bslave, stdout=bslave, stderr=bslave, preexec_fn=lambda: (os.setsid(), fcntl.ioctl(0, termios.TIOCSCTTY, 0)))
    os.close(bslave); bsel = selectors.DefaultSelector(); bsel.register(bmaster, selectors.EVENT_READ)
    def btext(): return re.sub(r'\s+', ' ', ESC.sub(b' ', bytes(blog)).decode('utf8', 'replace'))
    def buntil(pred, reason, timeout=30):
        deadline = time.monotonic() + timeout
        while not pred():
            if time.monotonic() > deadline: raise AssertionError('board timed out: ' + reason + '\n' + btext()[-3000:])
            if bsel.select(0.3):
                try: blog.extend(os.read(bmaster, 65536))
                except OSError: raise AssertionError('board exited:\n' + btext()[-3000:])
    def bclick(label, button=0):
        bseg = out / 'board.ansi'; bseg.write_bytes(bytes(blog)); time.sleep(0.5)
        while bsel.select(0.3): blog.extend(os.read(bmaster, 65536))
        bseg.write_bytes(bytes(blog))
        at = json.loads(ok(['bun', '--no-env-file', str(package / 'test/support/screen-find.ts'), str(bseg), '140', '46', label], cwd=repo))
        assert at, label + ' not located on the board'
        x, y = at['col'] + 2, at['row'] + 1
        os.write(bmaster, f'\x1b[<{button};{x};{y}M'.encode()); os.write(bmaster, f'\x1b[<{button};{x};{y}m'.encode())
    try:
        buntil(lambda: 'Saved plan' in btext(), 'board shows the held card', 60)
        # The first-run tour is the topmost modal: N must not reach the card under it.
        buntil(lambda: 'skip tour' in btext()[-400:], 'first-run tour', 30)
        os.write(bmaster, b'N'); time.sleep(1); assert held() is True, 'key leaked through the tour modal'
        os.write(bmaster, b'\x1b'); buntil(lambda: 'skip tour' not in btext()[-400:], 'tour closed', 30)
        def plan_runs(): return len(json.loads(ok([corral, 'show', task['id'], '--json'], cwd=work, env=owner_env)).get('plan_runs') or [])
        assert '▌ '+task['id'] in btext() and plan_runs() == 0, 'the only card is selected; nothing started'
        os.write(bmaster, b'N')
        buntil(lambda: plan_runs() == 1, 'board N admitted the plan continuation', 60)
        buntil(lambda: 'continuing' in btext(), 'board label after N', 30)
        (out / 'board-continued.ansi').write_bytes(blog)
        assert len(tasks()) == 1 and choices() == 1, (tasks(), choices())
        before_scratch = [s for s in scratches()]  # identities before the take-back
        bclick(task['id'] + ' app', button=2); bclick('work on this here')
        buntil(lambda: phase() == 'planning' and held() is True, 'board menu Work on this here returned it', 60)
        (out / 'board-taken-back.ansi').write_bytes(blog)
        returned = json.loads(ok([corral, 'show', task['id'], '--json'], cwd=work, env=owner_env))
        assert len(tasks()) == 1 and choices() == 1 and scratches() == before_scratch, (tasks(), choices(), scratches())
    finally:
        (out / 'board-final.ansi').write_bytes(blog)
        os.write(bmaster, b'q'); time.sleep(0.5)
        if board.poll() is None: board.terminate(); board.wait(timeout=5)
        os.close(bmaster)

    # The assistant sheet reflects the board's take-back: planning again, no coding scratch.
    mark = len(log); os.write(master, b'/handoff\r')
    until(lambda: has('Implement here', after=mark), 'returned sheet', 60)
    frame('taken-back'); os.write(master, b'\x1b'); time.sleep(1)

    # Implement here by a real SGR pointer click on its rendered row (current revision r4).
    mark = len(log); os.write(master, b'/handoff\r')
    until(lambda: has('Implement here', 'plan r4 scope', after=mark), 'implement offered', 60)
    frame('implement-offered')
    seg = out / 'segment.ansi'; seg.write_bytes(bytes(log[geometry:]))
    at = json.loads(ok(['bun', '--no-env-file', str(package / 'test/support/screen-find.ts'), str(seg), '140', '46', '[Implement here]'], cwd=repo))
    assert at, 'Implement here not located on screen'
    x, y = at['col'] + 3, at['row'] + 1
    os.write(master, f'\x1b[<0;{x};{y}M'.encode()); os.write(master, f'\x1b[<0;{x};{y}m'.encode())
    until(lambda: phase() == 'executing' and choices() == 2, 'pointer Implement here consumed', 60)
    frame('implemented')
    opened = [s for s in scratches() if s not in before_scratch]
    assert len(opened) == 1 and len(tasks()) == 1, (opened, tasks())

    # /plan: explicit opt-in planning entry from genuine pane input; source stays unchanged.
    head = lambda: ok(['git', '-C', str(app), 'rev-parse', 'HEAD']).strip()
    app_head = head()
    mark = len(log); os.write(master, b'/plan revise eviction order\r')
    until(lambda: phase() == 'planning', '/plan entered planning', 60)
    assert head() == app_head and len(scratches()) == len(before_scratch) + 1 and len(tasks()) == 1
    frame('planning-entered')

    # implementation_ship through the native entry: Implement here again (current r4), change
    # the granted scratch, then Take over and finish, each by a real SGR pointer click.
    def click(label):
        seg.write_bytes(bytes(log[geometry:]))
        at = json.loads(ok(['bun', '--no-env-file', str(package / 'test/support/screen-find.ts'), str(seg), '140', '46', label], cwd=repo))
        assert at, label + ' not located on screen'
        x, y = at['col'] + 3, at['row'] + 1
        os.write(master, f'\x1b[<0;{x};{y}M'.encode()); os.write(master, f'\x1b[<0;{x};{y}m'.encode())
    mark = len(log); os.write(master, b'/handoff\r')
    until(lambda: has('Implement here', 'plan r4 scope', after=mark), 'implement offered again', 60)
    click('[Implement here]')
    until(lambda: phase() == 'executing' and choices() == 3, 'second Implement here consumed', 60)
    granted = [x for x in record()['session']['scratch'] if x['slug'] not in before_scratch and x['slug'] != opened[0]]
    assert len(granted) == 1, granted
    Path(granted[0]['path'], 'cache.md').write_text('evict least recently used\n')
    mark = len(log); os.write(master, b'/handoff\r')
    until(lambda: has('Take over and finish', after=mark), 'implementation sheet', 60)
    frame('ship-offered')
    click('[Take over and finish]')
    # Lineage: the granted work continues the returned task, never a rival task.
    until(lambda: choices() == 4 and has('Ship when checks pass', after=mark), 'Take over and finish committed', 120)
    assert len(tasks()) == 1, tasks()
    ship = json.loads(ok([corral, 'show', task['id'], '--json'], cwd=work, env=owner_env))
    assert not ship.get('dispatch_hold') and not ship.get('runs'), ship
    mark = len(log); os.write(master, b'/handoff\r')
    until(lambda: has('Ship when checks pass', after=mark), 'goal/stop label after ship handoff', 60)
    frame('shipped')
    os.write(master, b'\x1b'); time.sleep(0.3)
    os.write(master, b'\x15\x04'); until(lambda: b'\x1b[?1049l' in log, 'terminal restored'); proc.wait(timeout=15)
    result = dict(status='PASS', preserved=preserved, production_entrypoint='src/host/main.ts', generation=fixture['generation'],
                  control_plane=corral, provider='offline fixture auth; no paid model requests', task=task['id'],
                  saved=dict(status=shown.get('status'), dispatch_hold=shown.get('dispatch_hold')),
                  returned=dict(status=returned.get('status'), dispatch_hold=returned.get('dispatch_hold'), assistant_phase='planning'),
                  asset_sha256=fixture.get('asset_sha256'), candidate=ok(['git', '-C', str(repo), 'rev-parse', 'HEAD']).strip(),
                  implemented=dict(input='SGR pointer click', scratch=opened[0], phase='executing', ships=False),
                  shipped=dict(input='SGR pointer click', task=ship['id'], status=ship.get('status'),
                               dispatch_hold=ship.get('dispatch_hold'), granted_scratch=granted[0]['slug'],
                               label='Ship when checks pass'),
                  checks=['real store: plan r4 with completed local review of r3 reads stale (current r4), advisory only',
                          'renders at 140, 80, 52 columns and 52x14 short height',
                          'Escape: no task, no choice input',
                          '52x14, screens replayed at 52x14 from the resize: a reply streams and holds; owner scrolls back and types a draft; F2 opens the sheet over it; Down and typed keys inside; the reply completes under the sheet; Escape: sheet closed, draft identical, reading position unchanged above the new reply, next key reaches the composer, no task or choice; Ctrl+End shows the completed reply',
                          'Preview context: read-only package preview (archived vs loaded, native fork no); no task, no choice',
                          'keyboard Save plan for later: one bound choice, one held task with no run, via native Local',
                          'board PTY (same store): first-run tour modal swallows N (no effect); keyboard N on the held card admits exactly one plan continuation (label Board continuing plan, same task); right-click card menu Work on this here by pointer returns it: assistant planning, task held, no new scratch',
                          'pointer click on Implement here (current r4) consumes a third choice: executing, one scratch, no task, no ship',
                          '/plan from pane input re-enters planning; app HEAD and task count unchanged',
                          'take-back: scratch identities identical before and after',
                          'second pointer Implement here, change in the granted scratch, pointer Take over and finish: the same task continues unheld as implementation_ship (no rival task), no run; sheet reads Ship when checks pass'])
    (out / 'result.json').write_text(json.dumps(result, indent=2)); print(json.dumps(result, indent=2))
finally:
    if proc and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: os.killpg(proc.pid, signal.SIGKILL); proc.wait()
    (out / 'pty-final.ansi').write_bytes(log)
    if master is not None: os.close(master)
    if workspace: subprocess.run(['herdr', '--session', 'corral-test', 'workspace', 'close', workspace], capture_output=True)
    shutil.rmtree(work, ignore_errors=True)
