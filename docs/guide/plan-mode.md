# Plan mode

In plan mode the agent does not implement. It reads the code, asks you about the things
only you can decide, and writes a plan. You review the plan and choose how it runs, or send
it back with feedback.

Plan mode is available in Build sessions and in chat. Task workers use
[plan runs](plans.md) instead.

## Start planning

| Where | How |
|---|---|
| Terminal | `/plan`, or `/plan <request>` to enter and send the request in one step |
| Launch | `corral-pi build --plan` or `corral-pi chat --plan` |
| App | **Enter plan mode** in the session's actions, or type `/plan` |

The composer title shows `PLAN` while the agent is planning and `PLAN · review` while a
submitted plan is waiting for you. `/plan` again leaves plan mode during planning; `/plan off`
leaves from any phase. You cannot enter or leave while a turn is running: stop the turn first.

`/todo` still shows the session's task list. `/plan` no longer does.

## While the agent plans

- The host refuses `edit` and `write` everywhere except the plan file, and refuses to start
  children that write. `explore` and `review` children still run.
- Shell commands are not blocked by plan mode. The agent is told to run only read-only
  commands; your `bash_rules` apply as usual. Plan mode is not a sandbox.
- The agent asks its questions through the normal question dialog. Each one should offer a
  recommended option.
- The plan is one Markdown file in the session's own directory, outside the worktree. It is
  never part of your branch.

## Review the plan

When the agent submits, the review opens in the terminal once its turn ends, and the same
review appears in the app. The first answer wins. The review has no time limit and survives
a restart; reopen it in the terminal with `/plan`.

| Choice | Terminal key | What happens |
|---|---|---|
| Execute | `e` | Runs the plan with the planning conversation still in context |
| Execute with clean context | `c` | Clears the conversation; the agent starts from the plan alone |
| Execute with compacted context | `s` | Summarises the planning discussion, then runs the plan |
| Keep planning | `k` | Returns to planning with your feedback |
| Not now | `Esc` | Closes the review and leaves it pending |

To run the plan on a different model, change the model first (`m` in the terminal review, or
the model picker in the app) and then pick any of the execute choices. The change takes
effect when you pick the model: if you then choose **Keep planning**, planning continues on
the new model.

Sending an ordinary message while a plan is waiting counts as feedback. The review closes and
the agent goes back to planning.

What you review is a copy taken when the agent submitted, and that copy is what runs. A later
change to the working plan file does not alter a plan you are reviewing or have approved.

A clean context stays in the same session and transcript: rewind, export and the Build record
keep working. The cleared conversation is replaced by the approved plan itself, which is the
agent's whole brief.

## After approval

Normal tool access returns. The agent has the plan in its context and the path of the
submitted copy under `plans/` in the session directory, and opens a todo list with one step
per plan step when the todo list is enabled. If the conversation is compacted later, the
agent is pointed back to that copy.

If the plan could not be started (for example the submitted copy is missing), a note in the
transcript says why and the review stays open.

## Older daemons and apps

The dedicated plan view in the app needs a daemon and app that include it. With an older
daemon or app the review shows as a card the app cannot answer; answer it in the terminal.
