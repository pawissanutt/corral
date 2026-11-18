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

Use the existing planning task as the parent. When asked to file a new plan without a task,
create the parent with `"$CORRAL" new "<plan title>" -p <project>`. For each independently
verifiable slice, file `"$CORRAL" new "<child title>" -p <project> --parent <parent-id>`;
add `--depends-on <id>,<id>` only for real prerequisites. Set the parent and child Markdown
bodies with `"$CORRAL" edit <id> --body-file <file>`. State the goal, scope, acceptance and
checks of each child so it can run without this conversation. Report the document and task
IDs. A worker finishes the planning task with its normal `corral_done`.

## Review a plan

Read the requested document, parent task and child tasks. Judge the plan, not an
implementation: verify assumptions in the repository, look for missing or overlapping
slices, check dependencies and make sure the proposed checks can prove acceptance. Do not
edit the plan or code, create tasks, dispatch, approve or merge while reviewing. In a Corral
review round, record findings with the ordinary `corral_verdict`; outside a review round,
report findings in the conversation.
