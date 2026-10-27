### Plan review (v3)

Question: can another agent execute this plan without inventing missing decisions?

- Read the human sections before implementation details: problem, goal, included changes and
  retained behavior, exclusions, choices and done scenarios. New generated plans opt in with
  `human_review`; historical plans without it remain valid and do not need a format-only rewrite.
  `goal`, `non_goals`, `decisions` and `acceptance` remain canonical. Check that the overview
  and body agree with them, rather than treating a separate brief as authority.
- Structural validation checks completeness, not semantic quality or approval. Assess whether
  the included changes solve the stated problem within the task's scope, whether retained
  behavior and exclusions are explicit, and whether choices and done scenarios support an
  informed decision. Do not invent extra product scope to fill an apparent gap.
- On a revision, compare against the preceding plan. Name scope additions, removals and changed
  promises separately from technical-only edits. Changed behavior or widened commitments are
  scope changes even in the same files; changed paths or decomposition alone need not be.
  Request changes when added scope is hidden in implementation detail or contradicts the task.
  Reviewer approval is not owner consent to a scope expansion.
- Inspect the repository contracts the plan relies on. Do not trust paths, APIs or command
  names just because they appear in a polished plan.
- Check feasibility, dependencies, interfaces, write scopes, migration, acceptance scenarios,
  verification commands and recorded owner decisions.
- Identify omissions that would force the implementer to choose a materially different
  product or architecture. Request concrete changes rather than a preferred rewrite.
- For each scope `gate` of an integration-target plan, check that the tier fits the scope's risk,
  that declared `tests` cover the scope's acceptance (they replace the touched-crate default),
  and that every expected failure names one exact `<binary-id> <test-name>`, gives a reason it
  fails, and names the child scope that restores it. Request changes for an expected failure
  without a credible reason or restoring child, or one that no later child restores.
- Member gates are fixed by the plan the owner approves. Once a plan is published, a member's
  gate changes only through an owner-only, revision-guarded member-gate amendment on the
  tracker; editing a child task, a worker's choice or a later plan step never changes it.
  Request changes for a plan that lets workers pick or relax their own gate, or that relies on
  child edits to change a published member's gate.
- Judge the plan as a proposed solution. Do not require execution evidence for code that is
  not meant to exist yet; plan approval is not implementation approval.
