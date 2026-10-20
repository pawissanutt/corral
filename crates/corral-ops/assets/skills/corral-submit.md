---
name: corral-submit
description: Procedure for filing work on the corral board - capture an idea, file a task with a spec, split work into dependent tasks or import a Markdown plan. Use when asked to file, capture or split corral tasks.
---
{{marker}}

# corral submit

You file work for the owner or PM; filing never dispatches it. Read `corral <verb> --help` for exact flags.

1. Check for an existing task first: `corral list --json` and `corral search --help`.
2. File one task with a spec (goal, scope, acceptance): `corral new --help`. Pass your own `--origin`.
3. Split dependent work with `--depends`, or import a Markdown plan: `corral import --help`.
4. Add context to an existing task with `corral comment --help`; re-plan only with `corral move --help`.
