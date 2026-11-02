---
name: simplify-and-cleanup
description: Simplify code, remove redundant comments, and tighten documentation while preserving behavior and meaning. Use when asked to clean up, simplify, reduce boilerplate, remove unnecessary abstractions, improve readability, trim verbose docs, or remove AI slop from specified files or recent changes. Not for feature work, architectural redesign, or correctness-only review.
allowed-tools: read grep find ls edit write bash lsp_diagnostics lsp_definition lsp_references
---
{{marker}}

# Simplify and cleanup

Perform a bounded maintainability cleanup, not an authorship judgment. Find concrete redundancy,
needless complexity, stale statements and empty prose; do not change something merely because it
looks AI-generated. A clean result may need no edits.

## Scope and intent

- Resolve the requested files or diff/base, including staged, unstaged and untracked inclusion.
  Ask if scope is ambiguous; never silently expand to the repository.
- Follow user intent: review/audit requests are read-only; cleanup/simplify requests authorize
  edits only within the role's existing permissions. Ask if intent is unclear. Loading this skill
  alone does not authorize edits, commits, pushes, landing, or bypassing review.
- Read applicable repository instructions, the scoped diff and surrounding code/prose, relevant
  callers/tests and nearby maintained examples. Identify and preserve pre-existing edits.
- Leave generated/vendor files and historical records alone unless expressly included. Honor
  repository policy and the run contract; this skill grants no new tools or board authority.

## Find justified improvements

### Code

Prefer existing repository helpers or standard-library functionality where semantics match.
Remove proven dead private code, redundant work, needless pass-through indirection and avoidable
nesting. Keep useful test seams, framework hooks, compatibility paths, intentional duplication
and abstractions with a real contract. Account for reflection and dynamic/string references
before declaring code unused. Do not add dependencies, speculative abstractions or optimization
work. Propose architectural changes and bug fixes separately rather than hiding them in cleanup.

### Comments

Remove narration of obvious syntax, empty boilerplate and obsolete implementation notes.
Preserve rationale, invariants, units, ownership, concurrency constraints, security warnings,
workarounds, ticket references, licenses and tool directives. Public API documentation can be
useful even when concise. Correct a stale comment only from verified behavior; otherwise flag it.

### Documentation

Remove filler, repetition, chat residue and inflated language; use direct, concrete wording.
Preserve factual claims, preconditions, warnings, uncertainty, examples, citations, public heading
anchors and required-versus-optional distinctions. Never invent facts. Keep commands, code,
paths, links, versions and machine-consumed content unchanged unless their correction is
explicitly in scope and verified. Do not impose punctuation bans or phrase blacklists.

## Safety threshold

For each edit identify the concrete defect and why its replacement preserves behavior or meaning.
Inspect contracts and callers before deleting guards, catches, fallbacks, compatibility code or
apparently unused exports. Types alone and passing tests do not prove runtime input validity or
redundancy. Preserve trust-boundary validation and resource cleanup unless equivalence is established.
Preserve outputs, error behavior, side effects, ordering, cancellation and public contracts.
Skip uncertain changes and report the missing evidence; no deletion quota or arbitrary size limit.

In audit mode report file/line findings, proposed changes, justification and risk without editing.
In apply mode make the smallest coherent edits within scope. Revert only your own faulty hunks,
never whole files containing someone else's edits.

## Proportional verification

- Prefer existing tests and repository checks. Establish relevant baseline results where feasible
  so pre-existing failures are not confused with cleanup regressions.
- Do not add tests merely because a file was edited or lacks coverage. Comment/prose-only edits
  normally need diff review and existing documentation checks, not new tests.
- Add a test only for a concrete behavioral risk in the proposed transformation that existing
  coverage does not adequately observe. It must fail if that behavior regresses.
- Avoid tests that pin wording, private implementation details or mock interactions without
  meaningful outcomes. Preserve genuine machine-consumed text contracts; do not weaken existing
  coverage or delete tests merely because this pass would not add them.
- If cleanup needs substantial new testing to establish safety, skip it and explain why rather
  than expanding the task. A failed or unavailable check is not permission to guess.
- Follow required repository gates and the run contract; do not invent extra gates or tooling.
  A checklist or green suite is evidence, not proof of behavioral equivalence.
- For prose compare claims before/after and use existing docs/link/example validation where
  relevant. Inspect the final diff for scope, semantic drift and damage to prior edits.

Report changed files, significant improvements, exact checks/results, skipped items and remaining
uncertainty. Say when nothing justified a change. Never claim a check ran or passed without evidence.
