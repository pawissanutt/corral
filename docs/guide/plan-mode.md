# Plan mode

In plan mode the agent does not implement. It reads the code, asks you about the things
only you can decide, and writes a plan. You review the plan and choose how it runs, or send
it back with feedback.

Plan mode is available in Build, Code and chat sessions.

## Start planning

| Where | How |
|---|---|
| Terminal | `/plan`, or `/plan <request>` to enter and send the request in one step |
| Launch | `corral-pi build --plan`, `corral-pi code --plan` or `corral-pi chat --plan` |
| App | **Enter plan mode** in the session's actions, or type `/plan` |

The composer title shows `PLAN` while the agent is planning and `PLAN · review` while a
submitted plan is waiting for you. `/plan` again leaves plan mode during planning; `/plan off`
leaves from any phase. You cannot enter or leave while a turn is running: stop the turn first.

Use `/todo` to show the session's task list.

## While the agent plans

- The host refuses `edit` and `write` everywhere except the plan file, which also becomes
  read-only while awaiting review. Plan mode refuses `corral_artifact` and limits new `task`
  children to the `explore` and `review` kinds.
- Shell commands are not blocked by plan mode. The agent is told to run only read-only
  commands; your `bash_rules` apply as usual. Plan mode is not a sandbox.
- The agent asks its questions through the normal question dialog. Each one should offer a
  recommended option.
- Write the draft to `plan.md` in the session's state directory, not to the branch. The
  `plan_submit` tool accepts a nonempty draft of up to 256 KiB.

## Review the plan

When the agent submits, the review opens in the terminal once its turn ends, and the same
review appears in the app. The first answer wins. The review has no time limit and survives
a restart; reopen it in the terminal with `/plan` or the waiting document row in F10 Plan.

| Choice | Terminal key | What happens |
|---|---|---|
| Execute | `1`, then Enter | Runs the plan with the planning conversation still in context |
| Execute with clean context | `2`, then Enter | Clears the conversation; the agent starts from the plan alone |
| Execute with compacted context | `3`, then Enter | Summarises the planning discussion, then runs the plan |
| Keep planning | `4`, then Enter | Closes the review for you to write your feedback in the composer; the plan stays under review until you send it |
| Not now | `Esc` | Closes the review and leaves it pending |

In the terminal the plan is drawn as Chat draws Markdown, with its sections listed beside it when
the terminal is wide enough. A digit only selects a choice and Enter carries it out, so no single
key approves a plan; a click on a choice only selects it as well. On a short or narrow terminal the
choices show as their digits with short labels (`Execute`, `Clean`, `Compacted`, `Keep planning`),
wrapped over as many rows as they need. A terminal too small to show the whole model name above
the choices reads `widen the terminal to execute`, and the execute choices do nothing until it does.

| Key | In the terminal review |
|---|---|
| ↑ / ↓, j / k, PgUp / PgDn, Space, Home / End, g / G, the wheel | Scroll the plan |
| `[` / `]` | The previous or next section; a click on a section in the outline goes to it |
| `c` | Mark or unmark the section you are reading for feedback |
| `d` | Show what changed since the submission before (from the second submission on), and back |
| `m` | Change the model that would execute |
| Esc, q | Close the review and leave it pending |

From the second submission on, the first row counts the sections that changed, are new or were
removed since the one before, and the outline marks changed and added sections `~` and `+`. `d` shows only those:
a changed section as its removed and added lines, a new section whole, a removed one by name.

**Keep planning** puts the headings you marked into the composer as quoted lines, after anything
you had already typed there. Write what should change under them and send it.

To run the plan on a different model, change the model first (`m` in the terminal review, or
the model picker in the app) and then pick any of the execute choices. The change takes
effect when you pick the model: if you then choose **Keep planning**, planning continues on
the new model.

Sending an ordinary message while a plan is waiting counts as feedback. The review closes and
the agent goes back to planning.

In the app the execute choices stay unavailable until the whole plan has been read from the
host; **Keep planning** and **Not now** are always available. A choice is refused while the
session is paused or compacting, because it could not start then.

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
