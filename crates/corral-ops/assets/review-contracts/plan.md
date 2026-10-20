### Plan review (v1)

Question: can another agent execute this plan without inventing missing decisions?

- Inspect the repository contracts the plan relies on. Do not trust paths, APIs or command
  names just because they appear in a polished plan.
- Check feasibility, dependencies, interfaces, write scopes, migration, acceptance scenarios,
  verification commands and recorded owner decisions.
- Identify omissions that would force the implementer to choose a materially different
  product or architecture. Request concrete changes rather than a preferred rewrite.
- Judge the plan as a proposed solution. Do not require execution evidence for code that is
  not meant to exist yet; plan approval is not implementation approval.
