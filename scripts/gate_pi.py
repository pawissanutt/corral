#!/usr/bin/env python3
"""Quick Pi test selection: reverse local imports, with conservative unknown-input fallback.

Full/plan gates still run all Bun tests. Instruction assets select their role contract
consumers; changing a package manifest or a shared Rust wire contract selects all tests.
"""
from pathlib import Path
import re

PACKAGE = Path('packages/corral-pi')
LOCAL = re.compile(r'''["'](\.{1,2}/[^"'\n]+)["']''')


def selection(paths: list[str]) -> list[str]:
    roots = set()
    roles = set()
    for path in paths:
        if path.startswith('packages/corral-pi/'):
            relative = Path(path).relative_to(PACKAGE)
            if relative.suffix == '.md':
                if 'worker' in path:
                    roles.add('worker')
                elif 'reviewer' in path:
                    roles.add('reviewer')
                else:
                    return []
            elif relative.suffix in {'.ts', '.tsx'}:
                roots.add(Path(path).resolve())
            else:
                return []  # Manifest, lock, config, unknown asset: full package.
        elif path.startswith('crates/corral-ops/assets/') or path.startswith('scripts/'):
            roles.update(('worker', 'reviewer'))
        elif path.startswith(('crates/corral-ops/src/agent', 'crates/corral-core/src/agent',
                              'crates/corral-core/src/inbox', 'crates/corral-ops/src/inbox',
                              'crates/corral/src/agent_cmd.rs', 'crates/corral-core/src/config/agent_capabilities')):
            return []  # Shared Rust wire contracts cannot be mapped by TS imports.
    files = {file.resolve() for directory in ('src', 'test')
             for file in (PACKAGE / directory).rglob('*.ts')}
    if not roots.issubset(files):
        return []
    consumers = {}
    for file in files:
        for reference in LOCAL.findall(file.read_text()):
            target = (file.parent / reference).resolve()
            candidates = [target, target.with_suffix('.ts'), target / 'index.ts']
            for candidate in candidates:
                if candidate in files:
                    consumers.setdefault(candidate, set()).add(file)
    pending = list(roots)
    reached = set(roots)
    while pending:
        for caller in consumers.get(pending.pop(), set()) - reached:
            reached.add(caller)
            pending.append(caller)
    reached.update((PACKAGE / f'src/roles/{role}/{role}.test.ts').resolve() for role in roles)
    tests = sorted('./' + str(file.relative_to(PACKAGE.resolve()))
                   for file in reached if file.name.endswith('.test.ts') and file.is_file())
    return tests  # Empty means conservative all-tests, not a silent skip.


def needs_rust(tests: list[str]) -> bool:
    if not tests:
        return True
    pending = [(PACKAGE / test).resolve() for test in tests]
    seen = set()
    while pending:
        file = pending.pop()
        if file in seen:
            continue
        seen.add(file)
        if not file.is_file():
            return True
        text = file.read_text()
        if re.search(r"\bcargo\b", text):
            return True
        for reference in LOCAL.findall(text):
            target = (file.parent / reference).resolve()
            for candidate in (target, target.with_suffix(".ts"), target / "index.ts"):
                if candidate.is_file() and candidate.suffix == ".ts":
                    pending.append(candidate)
    return False


if __name__ == "__main__":
    import sys
    sys.exit(0 if needs_rust(sys.argv[1:]) else 1)
