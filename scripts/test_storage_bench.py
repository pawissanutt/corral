#!/usr/bin/env python3
"""Regression for T-338's false run-loss report; includes real v1 -> v2 CLI migration.

Usage: python3 -I scripts/test_storage_bench.py [--before PINNED_V1] [--after CANDIDATE]
No live board, process bindings, credentials, panes, or installed binaries are changed.
"""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

import yaml

REPO = Path(__file__).resolve().parent.parent
OLD_HASH = 'aba46287e1c97797341abf3e8ed6add6643a7b1ea54366e842edcfa53e572fff'
# Execute the actual helper from the embedded Python, not a duplicate test parser.
source = (REPO/'scripts/storage-bench.sh').read_text().split("<<'PY'\n", 1)[1].rsplit('\nPY', 1)[0]
module = ast.parse(source)
helper = next(node for node in module.body if isinstance(node, ast.FunctionDef) and node.name == 'exported_task')
namespace = dict(re=re, yaml=yaml)
exec(compile(ast.Module(body=[helper], type_ignores=[]), str(REPO/'scripts/storage-bench.sh'), 'exec'), namespace)
exported_task = namespace['exported_task']

SUMMARY = '''Earlier run, superseded after changes.
| Run | Result |
| --- | --- |
A horizontal rule --- isn't a frontmatter fence.
---
The later runs must not disappear.
'''


def task_doc(task_id, count):
    runs = []
    for n in range(1, count+1):
        runs.append(dict(id=f'R{n}', agent='claude', worktree=f'/nonexistent/{task_id}/R{n}',
                         branch=f'corral/{task_id}', base='main', base_sha='0'*40,
                         started=f'2026-10-0{n}T00:00:00Z', ended=f'2026-10-0{n}T01:00:00Z',
                         outcome='superseded' if n < count else 'done', summary=SUMMARY,
                         herdr=dict(workspace_id=f'w{n}', pane_id=f'w{n}:p1')))
    return dict(id=task_id, title='Retried run export regression', status='done', priority='P2',
                created='2026-10-01T00:00:00Z', updated='2026-10-05T00:00:00Z', runs=runs)


class LiteralDumper(yaml.SafeDumper):
    pass


def represent_string(dumper, value):
    return dumper.represent_scalar('tag:yaml.org,2002:str', value,
                                   style='|' if '\n' in value else None)


LiteralDumper.add_representer(str, represent_string)


def markdown(task):
    return '---\n'+yaml.dump(dict(schema_version=2, task=task), Dumper=LiteralDumper,
                             sort_keys=False)+'---\nBody\n'


class Frontmatter(unittest.TestCase):
    def test_markdown_table_and_horizontal_rule_preserve_every_retry(self):
        for count in [1, 3, 4]:
            task = task_doc('T-205', count)
            text = markdown(task)
            # Demonstrate the original defect without keeping it as an oracle.
            self.assertNotEqual(task, yaml.safe_load(text.split('---', 2)[1])['task'])
            self.assertEqual(task, exported_task(text))

    def test_flat_envelope_crlf_and_body_fences(self):
        task = task_doc('T-093', 3)
        flat = '---\n'+yaml.safe_dump(task, sort_keys=False)+'---\nBody\n---\nMore\n'
        self.assertEqual(task, exported_task(flat.replace('\n', '\r\n')))

    def test_missing_fences_fail_instead_of_comparing_partial_data(self):
        for text in ['not yaml', '---\ntask: {}\n', 'title: x\n---\n']:
            with self.assertRaises(ValueError):
                exported_task(text)


class MigrationExport(unittest.TestCase):
    def test_migrated_retry_vectors_equal_pinned_v1_list(self):
        before = ARGS.before.resolve(strict=True)
        after = ARGS.after.resolve(strict=True)
        self.assertEqual(hashlib.sha256(before.read_bytes()).hexdigest(), OLD_HASH)
        with tempfile.TemporaryDirectory(prefix='corral-export-regression-') as tmp:
            root = Path(tmp)/'board'
            home = Path(tmp)/'home'
            root.mkdir()
            home.mkdir()
            env = dict(PATH='/usr/bin:/bin', HOME=str(home),
                       XDG_CONFIG_HOME=str(home/'config'), XDG_DATA_HOME=str(home/'data'),
                       XDG_STATE_HOME=str(home/'state'), XDG_CACHE_HOME=str(home/'cache'))

            def run(binary, *args):
                cmd = [str(binary), '--root', str(root), *args]
                result = subprocess.run(cmd, stdin=subprocess.DEVNULL, capture_output=True,
                                        text=True, env=env, cwd=root, timeout=120)
                print(json.dumps(dict(argv=cmd, exit=result.returncode)), flush=True)
                self.assertEqual(result.returncode, 0, result.stdout+'\n'+result.stderr)
                return result.stdout

            run(before, 'init')
            c = root/'.corral'
            # Terminal, multi-retry shapes from the prepared corpus; Markdown
            # separators occur before the final runs just as in T-205's summaries.
            tasks = [task_doc(task, n) for task, n in
                     [('T-205', 4), ('T-093', 3), ('T-178', 3), ('T-230', 3)]]
            for task in tasks:
                (c/'tasks'/f"{task['id']}-retries.md").write_text(markdown(task))
            expected = json.loads(run(before, 'list', '--all', '--json'))
            self.assertEqual([len(t['runs']) for t in expected], [3, 3, 4, 3])
            run(after, 'migrate', '--dry-run', '--json')
            run(after, 'migrate', '--apply', '--json')
            run(after, 'storage', 'check', '--json')
            dest = root/'export'
            run(after, 'export', 'tasks', str(dest))
            actual = [exported_task(p.read_text()) for p in sorted(dest.glob('*.md'))]
            # YAML's timestamp construction is a reader representation only.
            actual = json.loads(json.dumps(actual, default=lambda value: value.isoformat().replace('+00:00', 'Z')))
            runs = lambda docs: {t['id']: t['runs'] for t in docs}
            # Compare the entire ordered run list, retaining each run's legacy
            # fields. Newly introduced v2 default fields are not lost-run evidence.
            old = runs(expected)
            new = runs(actual)
            self.assertEqual({k: [r['id'] for r in v] for k, v in old.items()},
                             {k: [r['id'] for r in v] for k, v in new.items()})
            for task_id, legacy_runs in old.items():
                for legacy, current in zip(legacy_runs, new[task_id], strict=True):
                    for key, value in legacy.items():
                        self.assertEqual(value, current.get(key), f'{task_id} {legacy["id"]}.{key}')
            self.assertEqual({t['id']: t['status'] for t in expected},
                             {t['id']: t['status'] for t in actual})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path,
                        default=Path.home()/'.local/share/corral-bench/t253-2026-10-22/corral-0.3.0')
    parser.add_argument('--after', type=Path, default=REPO/'target/debug/corral')
    ARGS, remaining = parser.parse_known_args()
    unittest.main(argv=[__file__, *remaining])
