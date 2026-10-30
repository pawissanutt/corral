---
name: debugging
description: Investigate crashes, wrong results, hangs and flakes with a hypothesis-and-evidence loop. Use to reproduce a bug, distinguish causes and prove a minimal fix.
allowed-tools: read grep find ls edit write bash
---
{{marker}}

# Debugging

Runtime observations outrank plausible stories. Do not patch a guess.

1. Read the symptom and repository guidance. Identify runtime/version, exact inputs, environment,
   expected outcome and observed failure. Search existing findings with the run's read-only search
   command before repeating expensive investigation. Inspect code and callers with `read`, `grep`,
   `find` and `ls`; use `bash` for a bounded reproduction in an isolated fixture.
2. Record at least two plausible competing hypotheses (three for a complex failure). For each,
   name a distinguishing observation and the cheapest probe that could disprove it. Keep a short
   evidence ledger of command, observed value and conclusion, separating facts from guesses.
3. Change one variable per probe. Narrow inputs, inspect boundary values and trace the real call
   path. Use installed debugger/tracing tools through `bash` only on task-owned processes; check
   runtime documentation before attaching. Do not touch a user's live process or secrets.
4. For a flake, first check fixture leaks, shared ports/paths, races, ordering and event signals.
   Synchronize on the responsible event, not a longer sleep, retry budget or timeout. Never
   generate load on the shared host; use only the repository's authorized capped container path,
   or report load reproduction blocked.
5. Confirm the cause by showing that changing it toggles the failure while competing explanations
   do not. Preserve a failing regression scenario, make the smallest root-cause fix with `edit`,
   and rerun the same scenario plus the cheapest affected checks. Add files with `write` only when
   needed. A mocked happy path or typecheck alone is not proof of the fix.
6. After two inconclusive investigation rounds, summarize observations and rejected hypotheses;
   change the angle or ask the run's decision channel for missing authority or requirements. Do
   not invent subagents or continue random edits. If actual runtime access is unavailable, label
   partial evidence and what remains unproven instead of asserting a confirmed cause.
7. Track temporary instrumentation, fixtures and processes as you create them; remove only your
   own debug artifacts, retain requested evidence, and inspect the final diff. Do not ship debug
   logging, credentials or broad exception swallowing. Report the repro, root cause, fix, checks
   and remaining limitations. Commit only when the run or user authorizes it.
