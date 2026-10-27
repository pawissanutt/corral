# T-457: agent-first routing design record

## Outcome

T-457 delivers a design and implementation plan, not production changes. The
design is `docs/records/T-457-agent-first-routing-design.md`. It is a
byte-identical copy of the approved plan snapshot `T-457@P2`
(`artifacts/T-457/D1/2d74d0fa-plan.md`, SHA-256
`2d74d0fa423fe8f6a94a1c08e4a819e4b297191da20f1fec4482066e90376256`).

Lifecycle, as recorded by `corral show T-457 --json`:

| Step | Record |
|---|---|
| P1 submitted | `2e395bd8-plan.md`; independent review P1/PR1 returned `changes` (PR1-F1: baseline evidence not embedded in the immutable plan). |
| P2 submitted | Appendix A embeds the baseline verbatim; no other design change. |
| Independent review | P2/PR1 approved for that review attempt only. |
| Owner acceptance | P2 approval by `owner` at 2026-10-27T03:24:29Z. |

## Acceptance coverage

Section numbers refer to the design record.

| Task acceptance criterion | Where the design answers it |
|---|---|
| Task, agent-selection and settings wireframes with keyboard and mouse paths; named specialist identity; Auto is a mode | Section 3 (task detail, agent picker, settings, compact layout, first use); section 4 starter labels |
| When to use each agent, specialty and instructions; task-fit reason separate from model rationale | Section 3 picker and task detail; section 6 `fit_reason` versus execution rationale |
| Actual model and effort shown; model change preserves identity and references; shared models; no parallel category taxonomy | Section 3 execution details; section 4 selection/execution/policy revisions and snapshots |
| Manual choice, reset to Auto, next-run versus task-wide, defaults, review, retry, mixed specialty, empty/unavailable/single-provider; no authority granted | Section 5 scope table and exceptional states; section 1 authority statement |
| One resolver and explanation contract for TUI, CLI and PM; role obligations; availability separate from escalation; history, pins and sessions preserved | Section 6 precedence, explanation v2 fields and revalidation; section 5 availability and escalation |
| Effectiveness and scalability goals; bounded work; revision-aware caching; nothing on paint/input; load scale and baseline-derived budgets | Section 7; Appendix A baseline (31 samples, p95 6.590 ms, 18 MiB RSS) |
| Migration preview, rollout and rollback; conflicting precedence; five acceptance journeys | Section 8; section 11 journeys |
| Reconcile T-156 to T-159, reuse T-150 to T-155, dependency-ordered breakdown | Section 10 reuse/amend/supersede table and dependency manifest |
| Independent plan review and owner acceptance before implementation | Lifecycle table above |

## What this record does not do

- It does not edit T-156 through T-159, create child tasks, lift dispatch holds or
  change dependencies. Section 10 states that the workspace PM proposes those
  in-place amendments, including the new T-158 -> T-157 edge, as a separate
  board change.
- It does not change `.corral/corral.toml`, approve concrete legacy-reference or
  model mappings, or verify provider access. Starter labels are UX direction.
- The performance budgets in section 7 are engineering targets. No route, UI or
  10,000-task measurement was run; T-159 owns that verification.

## Verification

- `sha256sum` of the design record matches the approved P2 snapshot digest.
- The repository quick gate ran through `corral gate T-457 --wait` on the
  rebased tree; its log is attached to the task.
