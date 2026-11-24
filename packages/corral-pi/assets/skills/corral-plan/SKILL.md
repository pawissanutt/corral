---
name: corral-plan
description: Write or review a Markdown implementation plan for a Corral parent task with ordinary child tasks. Use when the task asks for a plan, not implementation.
---

# corral plan

A plan is a Markdown document, an ordinary parent task and its child tasks. There is no
separate planner, plan review workflow, approval command or plan state.

## Write a plan

Read the parent task, repository guidance and relevant code. Ask the owner only about
questions the available evidence cannot answer. Write one Markdown plan at the project's
normal plans location (or the path named by the task), and link it from the parent's task
body. Describe the goal and non-goals, the approach and why, concrete steps, dependencies,
verification and risks. Stop at the plan; do not implement or dispatch it.

Use the existing planning task as the parent. Before filing a parent or children, check
`"$CORRAL" list --all --json` and `show <id> --json` for likely duplicates, including completed
work. Use existing work instead of filing it again. Keep unrelated problems out of the plan.

When asked to file a plan without a parent task, use
`"$CORRAL" new "<plan title>" -p <project> --body-file <parent.md>`. For each independently
verifiable child use `"$CORRAL" new "<child title>" -p <project> --parent <parent-id> --body-file <child.md>`;
add `--depends-on <id>,<id>` only for real prerequisites. Write every new body's Problem/Goal,
Observed vs Expected, Repro steps (bugs only), Acceptance criteria, Relevant files/starting
points, and Open questions or unknowns. Make each child runnable without this conversation;
mark guesses and do not invent facts.

Pass `--size <xs|s|m|l|xl>` for work volume and independently assess
`--difficulty <routine|standard|hard>` for technical complexity or uncertainty. Repeat `--tag='<label>'`
for relevant built-ins (`accessibility`, `bug`, `dependencies`, `documentation`, `performance`,
`testing`) or exact existing custom labels. Honor explicit owner values. Omit a field only if
explicitly requested or it cannot be assessed; explain why in the body. Verify each new task's
body and fields with `show <id> --json`. Do not rewrite metadata on existing tasks unless asked.

Report the document and task IDs. A worker finishes the planning task with its normal
`corral_done`.

## Review a plan

Read the requested document, parent task and child tasks. Judge the plan, not an
implementation: verify assumptions in the repository, look for missing or overlapping
slices, check dependencies and make sure the proposed checks can prove acceptance. Do not
edit the plan or code, create tasks, dispatch, approve or merge while reviewing. In a Corral
review round, record findings with the ordinary `corral_verdict`; outside a review round,
report findings in the conversation.
