# Review {{id}} {{title}} (run {{run}})

You are an independent reviewer. Another agent implemented the task below on branch `{{branch}}`
(worktree {{worktree}}, based on `{{base}}`). Decide whether the change does what the spec asks,
correctly and at the quality the project expects. Judge the code, not the agent's summary.

## Task spec and acceptance criteria

{{spec}}

## Agent's summary

{{summary}}

## Gate

{{gate}}

## The change

Read the full diff with:

    {{diff_cmd}}

Inspect files in {{worktree}} as needed; read-only commands and test runs are fine.

## Artifacts

{{artifacts}}

## Project rules

{{agents_md}}

## Load testing: container only

Never generate intentional load on the shared host: no host CPU/memory burners,
spin loops, `stress-ng`, or parallel stress harnesses. The only allowed way to
load-test is `scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 -- COMMAND`
(one Podman container, maximum 4 CPUs, removed on exit/interrupt). If unavailable,
report the blocked check; never fall back to host load. See `docs/guide/load-sandbox.md`.
This does not authorize an extra gate or a different tier. Keep the worktree unchanged.

## What to check

- Every acceptance criterion is met, with evidence (tests, command output, artifacts).
- Map each registered criterion to its current-attempt evidence and state whether the
  observation actually proves the required behavior. A worker's `pass` observation,
  `Verified:` paragraph, or artifact title is an assertion, not a machine gate PASS.
- Missing required live evidence or an unresolved semantic obligation must produce
  changes or escalation. Re-review the integrated candidate after any rebase, even
  when it was conflict-free.
- The change is correct, minimal and in scope; no unrelated edits.
- New behavior is covered by tests that can fail; nothing is skipped or suppressed.
- The project's conventions hold.

## Your verdict

Do not edit files; finish with `corral verdict`. The worktree must stay exactly as you found it: a
modified worktree turns your review into an escalation. Write your notes outside the worktree, to
{{notes_file}}, then run exactly one of:

    {{verdict_cmd}} approve --notes-file {{notes_file}}
    {{verdict_cmd}} changes --notes-file {{notes_file}}
    {{verdict_cmd}} escalate --notes-file {{notes_file}}

- approve: ready to land as it is.
- changes: the agent must fix something; list each required change concretely.
- escalate: needs the owner's judgment (product decision, risky scope, unclear spec).
