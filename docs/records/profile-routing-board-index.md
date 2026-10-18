# Profile routing implementation plan: board index

Filed: 2026-10-18, following the owner's approval and request for fast-path filing.
Parent tracking task: **T-149**.
Project: corral.

## Plan and filing mode

Design source: `docs/records/pm-led-profile-routing-design.md`.
Immutable board snapshot:
`T-149:R0:4f6067a4-pm-led-profile-routing-design.md`.

The parent holds the approved design as a Plan artifact. All ten implementation
tasks carry their full acceptance criteria, relevant paths, plan reference and
machine-readable dependencies. The `plan-T-149` label groups them.

This is a fast-path tracking plan, not a native `plan submit`/review/acceptance
record. No reviewer verdict was invented and no planning or implementation agent
was launched. The parent and tasks are in Backlog with `dispatch_hold = true`.
The parent is not an unfinished dependency of its implementation tasks.

## Implementation tasks

| Task | Design label | Deliverable | Dependencies |
|---|---|---|---|
| T-150 | PRF-01 | Stable profiles and setup evidence contracts | None |
| T-151 | PRF-02 | Transactional setup and proposal CLI services | T-150 |
| T-152 | PRF-03 | PM-led setup and current-model selection procedure | T-151 |
| T-153 | PRF-04 | Explained, reasoning-based profile selection | T-150, T-151 |
| T-154 | PRF-08 | Dedicated reviewer profiles and task-specific review contracts | T-152, T-153 |
| T-155 | PRF-09 | Bounded availability fallback within profiles | T-151, T-153, T-154 |
| T-156 | PRF-10 | Repeated-failure escalation and bounded PM diagnosis | T-153, T-154, T-155 |
| T-157 | PRF-05 | First-run, profile and review/escalation TUI flows | T-151, T-152, T-153, T-154, T-155, T-156 |
| T-158 | PRF-06 | Mapping refresh and explicit legacy migration | T-152, T-153, T-154, T-155, T-156 |
| T-159 | PRF-07 | Integrated journeys, verification and user guidance | T-157, T-158 |

## Dependency order

1. T-150.
2. T-151.
3. T-152 and T-153 may proceed alongside each other with distinct owned paths.
4. T-154.
5. T-155.
6. T-156.
7. T-157 and T-158 may proceed alongside each other with distinct owned paths.
8. T-159.

Parallel eligibility does not override resource limits or shared-file coordination.
Each task includes its own targeted verification; T-159 does not defer all tests
until the end.

## Integration context

- T-142 and T-143 through T-148 own operating modes, scoped authority and related PM/UI
  work. Reuse their effective authority rather than creating a second permission system.
- T-137 through T-141 own recovery contracts. T-155 must integrate with the shared
  checkpoint/recovery services, not invent a second recovery subsystem.
- These are coordination references, not silently changed dependencies of existing work.

## Starting work later

Filing did not authorize dispatch. Once the owner requests implementation, release
the relevant tasks through the normal queue/admission path in dependency order.
Keep T-149 as a tracking item; do not dispatch it as another implementation or planner.
Existing gate, reviewer and owner-acceptance policy still applies to each task.
The approved design does not itself change live profile mappings or provider access.
