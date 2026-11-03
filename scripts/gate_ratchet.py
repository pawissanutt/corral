#!/usr/bin/env python3
"""Map changed registered Rust test bodies/attributes to candidate nextest identities."""
from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess

from gate_select import module_tree
from test_cost import git

RAW = re.compile(r'(?:b|c)?r(#+)?"')
CHAR = re.compile(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'")
DECL = re.compile(r'((?:#\[[^]]*\]\s*)+)(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(?:r#)?(\w+)')


def code_mask(text: str) -> str:
    """Preserve positions while hiding strings and nested comments from brace matching."""
    out = list(text)
    at = 0
    while at < len(text):
        end = at
        if text.startswith('//', at):
            end = text.find('\n', at)
            if end < 0:
                end = len(text)
        elif text.startswith('/*', at):
            end, depth = at + 2, 1
            while end < len(text) and depth:
                if text.startswith('/*', end):
                    depth += 1
                    end += 2
                elif text.startswith('*/', end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
        elif raw := RAW.match(text, at):
            marker = '"' + (raw[1] or '')
            found = text.find(marker, raw.end())
            end = len(text) if found < 0 else found + len(marker)
        elif text[at] == '"':
            end = at + 1
            while end < len(text):
                if text[end] == '\\':
                    end += 2
                elif text[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
        elif literal := CHAR.match(text, at):
            end = literal.end()
        if end > at:
            out[at:end] = ['\n' if char == '\n' else ' ' for char in text[at:end]]
            at = end
        else:
            at += 1
    return ''.join(out)


def close_brace(code: str, opening: int) -> int:
    depth = 0
    for at in range(opening, len(code)):
        depth += (code[at] == '{') - (code[at] == '}')
        if depth == 0:
            return at + 1
    return len(code)


def test_bodies(text: str) -> dict[str, list[str]]:
    code = code_mask(text)
    modules = [(match[1], match.end() - 1, close_brace(code, match.end() - 1))
               for match in re.finditer(r'\bmod\s+(\w+)\s*\{', code)]
    tests: dict[str, list[str]] = {}
    for match in DECL.finditer(code):
        if not re.search(r'\b(test|rstest|test_case)\b', match[1]):
            continue
        opening = code.find('{', match.end())
        if opening < 0 or ';' in code[match.end():opening]:
            continue
        parents = [name for name, start, end in modules if start < match.start() < end]
        name = '::'.join([*parents, match[2]])
        tests.setdefault(name, []).append(text[match.start():close_brace(code, opening)])
    return tests


def candidate_tests(cargo: dict, binaries: dict, suites: dict) -> tuple[list[list[str]], str]:
    hint = os.environ.get('CORRAL_INTEGRATION_BASE', 'HEAD^')
    base = git('merge-base', hint, 'HEAD').strip()
    changed = {}
    for value in git('diff', '--name-only', base, '--', '*.rs').splitlines():
        file = Path(value).resolve()
        if not file.is_file():
            continue
        try:
            old = test_bodies(git('show', f'{base}:{value}'))
        except subprocess.CalledProcessError:
            old = {}
        current = test_bodies(file.read_text())
        names = {name for name, body in current.items() if old.get(name) != body}
        if names:
            changed[file] = names
    selected = set()
    packages = {package['id']: package for package in cargo['packages']}
    for binary, info in binaries.get('rust-binaries', {}).items():
        package = packages[info['package-id']]
        target = next(target for target in package['targets']
                      if target['name'] == info['binary-name'] and info['kind'] in target['kind'])
        tree = module_tree(Path(target['src_path']))
        for file, names in changed.items():
            if file not in tree:
                continue
            for name in names:
                full = '::'.join(part for part in (tree[file], name) if part)
                for case in suites[binary]['testcases']:
                    if case == full or case.startswith(full + '::'):
                        selected.add((binary, case))
    return [list(value) for value in sorted(selected)], base
