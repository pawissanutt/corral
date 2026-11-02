#!/usr/bin/env bash
# Offline T-253 cutover proof. No installed binary, live board or pane is changed.
set -euo pipefail
exec </dev/null
exec python3 -I - "$@" <<'PY'
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import statistics
import subprocess
import tarfile
import time
import tempfile
import yaml

PREPARED = '0a8d23ffb0a40f366f633c39d59fcd14aa87d7894072f61ea6e48623ef2837f0'
RAW = 'f3109055997851a2b70882f769ce3062b1e0972b3724fa17f8a53e8ef79a3f72'
OLD = 'aba46287e1c97797341abf3e8ed6add6643a7b1ea54366e842edcfa53e572fff'
IDENTITY = '33edcd5506fcf4475242b75af2554fd383b226a14aeb2e65efff947a7a92eaff'
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--fixtures', type=Path, default=Path.home()/'.local/share/corral-bench/t253-2026-10-22')
p.add_argument('--before', type=Path, help='defaults to fixtures/corral-0.3.0')
p.add_argument('--after', type=Path, required=True, help='candidate binary, never installed by this script')
p.add_argument('--out', type=Path, required=True, help='NEW directory for copies and proof; retained on failure')
p.add_argument('--projects', type=Path, help='optional repository container, linked read-only by convention for git probes')
p.add_argument('--barrier-binary', type=Path, help='same-source debug build, enables fourth-copy B6 rehearsal')
p.add_argument('--rehearse', action='store_true', help='also third/fourth-copy rollback and raw refusal checks')
p.add_argument('--rehearsal-only', choices=['published', 'prepublish', 'raw'], help='run one independent rehearsal on fresh copies, without the benchmark')
p.add_argument('--after-live', type=Path, metavar='ROOT', help='OWNER ONLY: explicit root; read-only measurements, no migration')
a = p.parse_args()
a.after = a.after.resolve(strict=True)
a.before = (a.before or a.fixtures/'corral-0.3.0').resolve()
a.out = a.out.resolve()
a.out.mkdir(parents=True, exist_ok=False)
log = a.out/'commands.jsonl'
failures = []
# Roots inside a project's .worktrees are intentionally forbidden by v2. Keep
# disposable HOME/cache/scratch outside all repository containers, even if --out
# is .evidence/. Retain the path for inspection and explicit owner cleanup.
homes = Path(tempfile.mkdtemp(prefix='corral-storage-bench-'))
(a.out/'runtime-root.txt').write_text(str(homes)+'\n')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1024*1024), b''):
            h.update(chunk)
    return h.hexdigest()


def verify(path, expected):
    actual = digest(path)
    assert actual == expected, (path, actual, expected)


def env(root):
    home = homes/root.name
    home.mkdir(parents=True, exist_ok=True)
    return dict(PATH='/usr/bin:/bin', HOME=str(home),
                XDG_CONFIG_HOME=str(home/'config'), XDG_DATA_HOME=str(home/'data'),
                XDG_STATE_HOME=str(home/'state'), XDG_CACHE_HOME=str(home/'cache'))


def run(binary, root, args, label, expect=0, trace=False):
    cmd = [str(binary), '--root', str(root), *args]
    base = a.out/label
    if trace:
        cmd = ['strace', '-f', '-c', '-o', str(base)+'.strace', *cmd]
    load = Path('/proc/loadavg').read_text().strip()
    start = time.monotonic()
    with open(str(base)+'.stdout', 'wb') as stdout, open(str(base)+'.stderr', 'wb') as stderr:
        result = subprocess.run(cmd, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                env=env(root), cwd=root, timeout=1800)
    elapsed = time.monotonic()-start
    row = dict(label=label, argv=cmd, exit=result.returncode, seconds=elapsed, load=load)
    with log.open('a') as f:
        f.write(json.dumps(row)+'\n')
    print(json.dumps(row), flush=True)
    assert (result.returncode == expect if expect is not None else result.returncode != 0), row
    return Path(str(base)+'.stdout').read_text()


def restore(name, archive, expected):
    verify(archive, expected)
    root = a.out/name
    root.mkdir()
    # Verified archives are still extracted with tar's data filter (no escaping links/paths).
    with subprocess.Popen(['zstd', '-dc', str(archive)], stdout=subprocess.PIPE) as z:
        with tarfile.open(fileobj=z.stdout, mode='r|') as tar:
            tar.extractall(root, filter='data')
        assert z.wait() == 0
    binding_path = root/'.corral/herdr-session.json'
    binding = json.loads(binding_path.read_text())
    if expected == RAW:
        # Only an extracted offline copy is changed; preserve the raw archive.
        binding['socket'] = '/nonexistent/t253-herdr.sock'
        binding_path.write_text(json.dumps(binding)+'\n')
    assert binding['socket'] == '/nonexistent/t253-herdr.sock'
    if a.projects:
        for project in sorted(a.projects.iterdir()):
            if project.is_dir() and (project/'.git').exists():
                (root/project.name).symlink_to(project.resolve(), target_is_directory=True)
    return root


COMMANDS = [('list', ['list', '--json'], .2),
            ('pm-next', ['pm', 'next', '--json'], .5),
            ('automation-status', ['automation', 'status'], .5)]


def measure(sides):
    medians = {}
    for name, args, bound in COMMANDS:
        for side, binary, root in sides:
            run(binary, root, args, f'{side}-{name}-warmup')
        times = {side: [] for side, _, _ in sides}
        for i in range(5):
            for side, binary, root in sides:
                run(binary, root, args, f'{side}-{name}-{i}')
                times[side].append(json.loads(log.read_text().splitlines()[-1])['seconds'])
        for side, binary, root in sides:
            run(binary, root, args, f'{side}-{name}-syscalls', trace=True)
            # A separate trace avoids charging strace overhead to wall-time samples.
            trace = a.out/f'{side}-{name}.execve'
            cmd = ['strace', '-f', '-qq', '-e', 'trace=execve', '-o', str(trace),
                   str(binary), '--root', str(root), *args]
            start = time.monotonic()
            result = subprocess.run(cmd, stdin=subprocess.DEVNULL, env=env(root), cwd=root,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=1800)
            (a.out/f'{side}-{name}-exec.stdout').write_bytes(result.stdout)
            (a.out/f'{side}-{name}-exec.stderr').write_bytes(result.stderr)
            with log.open('a') as f:
                f.write(json.dumps(dict(label=f'{side}-{name}-exec', argv=cmd, exit=result.returncode,
                                        seconds=time.monotonic()-start, load=Path('/proc/loadavg').read_text().strip()))+'\n')
            assert result.returncode == 0
            lines = trace.read_text().splitlines()
            git = [line for line in lines if re.search(r'execve\("[^\"]*/git",', line) and '= 0' in line]
            medians[f'{side}-{name}'] = dict(seconds=statistics.median(times[side]),
                                            bound=bound, git=len(git),
                                            git_diff=sum('"diff"' in line for line in git))
    (a.out/'medians.json').write_text(json.dumps(medians, indent=2)+'\n')
    (a.out/'MEASURED').write_text('Every measurement exit status is 0; thresholds evaluated separately.\n')
    if not all(v['seconds'] <= v['bound'] for k, v in medians.items() if k.startswith('B-')):
        failures.append('B median threshold exceeded; see medians.json')


def usage(path):
    if not path.exists():
        return 0
    if path.is_file():
        return path.stat().st_size
    return sum(p.stat().st_size for p in path.rglob('*') if p.is_file() and not p.is_symlink())


def exported_task(text):
    # A substring split truncates valid YAML at Markdown table separators or
    # horizontal rules in an indented run summary. Only frontmatter fence LINES
    # delimit YAML, exactly as the legacy parser does.
    match = re.match(r'\A---[ \t]*\r?\n(.*?)^---[ \t]*\r?(?:\n|\Z)', text, re.M | re.S)
    if not match:
        raise ValueError('missing or unterminated exported task frontmatter')
    doc = yaml.safe_load(match[1])
    return doc.get('task', doc)


def inventory(root):
    return {str(p.relative_to(root)): digest(p) for p in root.rglob('*') if p.is_file() and not p.is_symlink()}


def legacy_invariant(root, fence):
    c = root/'.corral'
    assert not any((c/n).exists() for n in ['db', 'db.migrating', 'db.discard', 'migration.json'])
    assert (c/'state/schema-version').read_bytes() == fence


def rehearse_published():
    C = restore('C', a.fixtures/'prepared.tar.zst', PREPARED)
    fence = (C/'.corral/state/schema-version').read_bytes()
    # The fixture is already paused/drained. Save both binaries privately, never install live.
    run(a.before, C, ['queue', 'pause'], 'C-pause')
    run(a.after, C, ['migrate', '--dry-run', '--json'], 'C-drain')
    shutil.copy2(a.before, a.out/'corral-v1')
    shutil.copy2(a.after, a.out/'corral-candidate')
    candidate = a.out/'corral-candidate'
    run(candidate, C, ['migrate', '--dry-run', '--json'], 'C-dry')
    run(candidate, C, ['migrate', '--apply', '--json'], 'C-apply')
    run(candidate, C, ['storage', 'check', '--json'], 'C-check')
    for name, args, _ in COMMANDS:
        run(candidate, C, args, 'C-measure-'+name)
    run(candidate, C, ['migrate', '--rollback', '--json'], 'C-rollback-zero')
    legacy_invariant(C, fence)
    run(candidate, C, ['migrate', '--apply', '--json'], 'C-reapply')
    run(candidate, C, ['comment', 'T-001', 'T-338 rollback refusal rehearsal'], 'C-write')
    run(candidate, C, ['migrate', '--rollback', '--json'], 'C-refusal', expect=None)
    status = json.loads(run(candidate, C, ['migrate', '--status', '--json'], 'C-status'))
    n = status['changes']
    assert n > 0, status
    run(candidate, C, ['migrate', '--rollback', '--discard-v2-changes', str(n), '--json'], 'C-discard')
    legacy_invariant(C, fence)
    run(a.out/'corral-v1', C, ['list', '--all', '--json'], 'C-v1-after')

def rehearse_prepublish():
    assert a.barrier_binary, '--rehearse requires --barrier-binary (same-source debug build)'
    D = restore('D', a.fixtures/'prepared.tar.zst', PREPARED)
    original = run(a.before, D, ['list', '--all', '--json'], 'D-v1-before')
    fence = (D/'.corral/state/schema-version').read_bytes()
    cmd = [str(a.barrier_binary.resolve()), '--root', str(D), 'migrate', '--apply']
    with (a.out/'D-kill.stdout').open('wb') as stdout:
        child = subprocess.Popen(cmd, env={**env(D), 'CORRAL_MIGRATE_BARRIER': 'B6'},
                                 stdin=subprocess.DEVNULL, stdout=stdout, stderr=subprocess.PIPE)
        try:
            sel = selectors.DefaultSelector()
            sel.register(child.stderr, selectors.EVENT_READ)
            captured = bytearray()
            deadline = time.monotonic()+1800
            while b'corral-migrate-barrier B6\n' not in captured:
                assert time.monotonic() < deadline, 'B6 deadline expired'
                if not sel.select(timeout=1):
                    continue
                chunk = os.read(child.stderr.fileno(), 65536)
                assert chunk, 'candidate exited before B6'
                captured.extend(chunk)
            child.kill()
            assert child.wait(timeout=10) == -9
            (a.out/'D-kill.stderr').write_bytes(captured)
            with log.open('a') as f:
                f.write(json.dumps(dict(label='D-kill-B6', argv=cmd, exit=-9, barrier='B6'))+'\n')
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
    status = json.loads(run(a.after, D, ['migrate', '--status', '--json'], 'D-status'))
    assert status['state'] == 'verified' and status['candidate'] == 'db', status
    run(a.after, D, ['migrate', '--rollback', '--json'], 'D-rollback')
    legacy_invariant(D, fence)
    assert json.loads(original) == json.loads(run(a.before, D, ['list', '--all', '--json'], 'D-v1-after'))
    text = 'T-338 B6 composed tail imported from legacy'
    run(a.before, D, ['comment', 'T-001', text], 'D-comment')
    run(a.after, D, ['migrate', '--apply', '--json'], 'D-reapply')
    run(a.after, D, ['storage', 'check', '--json'], 'D-check')
    assert text in run(a.after, D, ['show', 'T-001', '--json'], 'D-imported-comment')
    assert (D/'.corral/db').is_dir()
    assert not any((D/'.corral'/n).exists() for n in ['db.migrating', 'db.discard'])

def rehearse_raw():
    R = restore('raw', a.fixtures/'corpus.tar.zst', RAW)
    original = inventory(R/'.corral')
    report = json.loads(run(a.after, R, ['migrate', '--dry-run', '--json'], 'raw-dry', expect=None))
    assert {b['task'] for b in report['blockers']} == {'T-125', 'T-203', 'T-251', 'T-252', 'T-253'}, report['blockers']
    assert len(report['blockers']) == 5
    run(a.after, R, ['migrate', '--apply', '--json'], 'raw-apply', expect=None)
    after = inventory(R/'.corral')
    # Acquiring maintenance's advisory lock may create its empty lock file;
    # refusal must change no legacy bytes, fence, journal or database state.
    lock = 'lock/migrate.lock'
    if lock not in original and lock in after:
        assert (R/'.corral'/lock).stat().st_size == 0
        after.pop(lock)
    assert original == after
    legacy_invariant(R, (R/'.corral/state/schema-version').read_bytes())
    (a.out/'raw-invariant.json').write_text(json.dumps(dict(legacy_files_unchanged=True,
        journal_absent=True, database_paths_absent=True, allowed_empty_lock=lock), indent=2)+'\n')

def main():
    (a.out/'identity.json').write_text(json.dumps(dict(prepared_archive=PREPARED, prepared_content=IDENTITY,
        raw_archive=RAW, before_sha256=digest(a.before) if not a.after_live else None,
        after_sha256=digest(a.after), barrier_sha256=digest(a.barrier_binary) if a.barrier_binary else None), indent=2)+'\n')
    if a.after_live:
        measure([('B', a.after, a.after_live.resolve(strict=True))])
        assert not failures, failures
        return
    verify(a.before, OLD)
    if a.rehearsal_only:
        {'published': rehearse_published, 'prepublish': rehearse_prepublish, 'raw': rehearse_raw}[a.rehearsal_only]()
        (a.out/'REHEARSED').write_text(a.rehearsal_only+' passed.\n')
        return
    A = restore('A', a.fixtures/'prepared.tar.zst', PREPARED)
    B = restore('B', a.fixtures/'prepared.tar.zst', PREPARED)
    run(a.after, B, ['migrate', '--dry-run', '--json'], 'B-dry-run')
    run(a.after, B, ['migrate', '--apply', '--json'], 'B-apply')
    run(a.after, B, ['storage', 'check', '--json'], 'B-check')
    # Exports carry run vectors; v2 list is deliberately summary-only.
    before = json.loads(run(a.before, A, ['list', '--all', '--json'], 'A-all'))
    after = json.loads(run(a.after, B, ['list', '--all', '--json'], 'B-all'))
    exported = B/'export'
    run(a.after, B, ['export', 'tasks', str(exported)], 'B-export')
    def rows(data):
        return {t['id']: (t['status'], len(t.get('runs', []))) for t in data}
    expected = rows(before)
    actual = {}
    for path in exported.glob('*.md'):
        task = exported_task(path.read_text())
        actual[task['id']] = (task['status'], len(task.get('runs', [])))
    mismatches = {k: dict(before=expected.get(k), after=actual.get(k))
                  for k in expected.keys() | actual.keys() if expected.get(k) != actual.get(k)}
    if mismatches:
        failures.append('Workload equivalence failed; see equivalence.json')
    assert {t['id']: t['status'] for t in after} == {k: v[0] for k, v in actual.items()}
    (a.out/'equivalence.json').write_text(json.dumps(dict(tasks=actual, mismatches=mismatches), indent=2)+'\n')
    measure([('A', a.before, A), ('B', a.after, B)])
    c = B/'.corral'
    pre_gc = usage(c)
    run(a.after, B, ['storage', 'gc', '--dry-run', '--json'], 'B-gc-dry')
    run(a.after, B, ['storage', 'gc', '--yes', '--json'], 'B-gc')
    stats = json.loads(run(a.after, B, ['stats', '--storage', '--json'], 'B-storage-stats'))
    footprint = dict(db=usage(c/'db'), blobs=usage(c/'blobs'), reclaimed=pre_gc-usage(c),
                     artifacts_original=stats['original_bytes'], artifacts_stored=stats['stored_bytes'],
                     artifacts_compression_savings=stats['original_bytes']-stats['stored_bytes'],
                     moved_outside_corral=usage(homes/'B'/'cache'),
                     legacy_awaiting_finalize=sum(usage(c/n) for n in ['tasks', 'state', 'artifacts', 'prompts', 'lessons', 'events.jsonl']),
                     rollback_archive=usage(c/'rollback'))
    (a.out/'footprint.json').write_text(json.dumps(footprint, indent=2)+'\n')
    if footprint['db']+footprint['blobs'] > 1_000_000_000:
        failures.append('v2 footprint exceeds 1.0 GB; see footprint.json')
    if a.rehearse:
        rehearse_published()
        rehearse_prepublish()
        rehearse_raw()
        (a.out/'REHEARSED').write_text('C, D/B6 and raw refusal invariants passed.\n')
    assert not failures, failures
    (a.out/'PASS').write_text('Thresholds, equivalence and footprint passed.\n')


try:
    main()
except BaseException as error:
    (a.out/'FAILED').write_text(repr(error)+'\n')
    raise
PY
