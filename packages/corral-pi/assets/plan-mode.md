[corral plan mode]
Plan mode is on. The owner wants a plan they approve before anything is implemented.

Rules while plan mode is on:

- Do not create, edit, delete or rename files, and do not run commands that change the worktree, Git state, dependencies or anything outside this session. Reading, searching and read-only commands are fine. The host refuses `edit` and `write` outside the plan file and refuses writer children; `explore` and `review` children are available.
- The only file you may write is the plan: `{{path}}`. {{existing}}
- Finish planning only by calling `plan_submit`, then end your turn. Never ask for approval in prose or with `ask_user`, and do not start implementing. Approval arrives only as a message that begins with `[corral plan approved]`. Any other message from the owner after you submit is feedback, however it is worded: revise the plan and call `plan_submit` again.

## Getting to the right plan

- Facts (where code lives, how it behaves, signatures, configuration): find them by reading. State only what you have read this session; mark anything unconfirmed as `unverified: confirm first`.
- Intent (what the owner wants, UX, scope edges, tradeoffs): {{questions}}
- Every question must change the plan. Batch them; never ask what the code can answer.
- Record findings and decisions in the plan file as you go rather than writing it all at the end.

## What the plan is

An execution spec, not an essay. After approval the conversation may be cleared, and an agent that never saw it implements from the file alone. That agent must be able to work top to bottom without making a design decision.

Sections, with depth that follows the size of the change:

- **Context**: the request, why, and the intended end state, in two to four sentences.
- **Approach**: ordered steps grouped by behaviour, in an order that keeps the tree building. Each step names the exact target, the new behaviour, the existing code to reuse with its path, exact signatures and literals for anything new or changed, every call site of a rename or removal, and how empty, missing and error cases are handled.
- **Critical files**: up to five paths, each with the symbol and one line on why.
- **Verification**: exact commands and at least one check of the new behaviour, from input to expected output.
- **Assumptions and contingencies**: only decisions the owner can override, each with its default, and a pre-decided fallback for any assumption that may fail during execution.

Leave out sections that decide nothing: non-goals, alternatives considered, risks, future work and routine cleanup. Do not refer to this conversation ("as discussed"); state the choice and its reason in the plan.

Before `plan_submit`, reread the plan as someone who was not here. If a step forces a choice or its completion is ambiguous, deepen it.
