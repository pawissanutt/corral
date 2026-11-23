# Plan mode for corral-pi

Status: approved by the owner and implemented. The user guide is
[docs/guide/plan-mode.md](../guide/plan-mode.md); this document records the design.

## Context

The owner wants a plan mode like other harnesses have: the agent does not implement, it
explores and asks until it is designing what the owner actually wants, then shows a design
document. The owner then chooses to execute, execute with a clean context, execute with a
compacted context, or keep working on the plan, and can switch model with any of the
execute choices. Build sessions are the main target; it ships as a core feature for chat too.

This is a different thing from the three existing uses of "plan" and replaces none of them:
task-level Plan runs (`dispatch --plan`), the `corral_todo` request plan, and `board_plan`.

## Owner decisions

| Decision | Choice |
|---|---|
| Clean context | Reset in place: same session and transcript, context replaced by the approved plan, no model call |
| Surfaces | Build and assistant chat; terminal, plus a typed card and a dedicated plan view in the Corral app |
| Entry | `/plan` toggle with an app action, and a `--plan` launch flag on `build` and `chat` |
| Read-only enforcement | The host blocks `edit`/`write` outside the plan file and writer children; shell stays prompt-only |

Task workers and reviewers are excluded: workers keep task-level Plan runs.

## Read-only enforcement: what other harnesses do

Checked in local sources:

- **Claude Code** enforces it in the harness. The Agent SDK's `plan` permission mode is
  documented as "Planning mode, no execution of tools" (`sdk.d.ts`).
- **Pi's bundled plan-mode example** disables `edit` and `write` and filters `bash` through a
  read-only allowlist (`examples/extensions/plan-mode`).
- **omp** hard-blocks working-tree writes, renames and deletes (`tools/plan-mode-guard.ts`)
  and lets the agent write only its plan file. Shell is prompt-only.

None of them rely on the prompt alone for file edits; they differ on shell.

**Decision: omp's level.** The file guard is a few lines at a hook that already exists and
stops the common failure (the agent starts implementing). A shell allowlist is a parser that
either blocks legitimate read-only commands or gives false assurance, and corral already
says shell rules are not a sandbox.

## How it works

### Phases

`off → planning → review → off`. From `review`, "Keep planning" returns to `planning`.

Entering, submitting, returning to planning and leaving each append a transcript custom
entry `corral_plan_mode` `{version: 1, phase, revision, at, title?, id?, digest?, path?, left?}`.
An approval appends none: a review ends as approved exactly when the message the host
admitted as its kickoff is in the transcript. The host sets `corralPlanKickoff: <card id>` on
that one message as it is persisted, through the same queue tag that carries an input's
origin. The kickoff's first line, `[corral plan approved <card id>]`, is for the model and
proves nothing: the same words from any other input approve nothing. No approval can
therefore be recorded without its kickoff, lost with it, or outlive a rewind of it.

`PlanMode` (`src/host/plan-mode.ts`) keeps no phase of its own: it reads the current branch,
cached by leaf id. A restart restores the phase, and rewinding to a turn before plan mode was
entered leaves it.

### Files

`<state dir>` belongs to one conversation. A Build has a directory of its own
(`options.build.dir`, which also holds `todo.json`). A chat's runtime directory serves every
conversation held there, so its state dir is `<runtime dir>/plan-mode/<transcript id>`: a
resumed chat finds its draft, and `chat --new` starts without one.

- `<state dir>/plan.md`: the one plan document of the session. The agent creates and revises
  it with the ordinary `write` and `edit` tools.
- `<state dir>/plans/<revision>-<slug>-<id>.md`: the write-once copy made at each submission.
  The review and the execution read this copy, never the working file, so the owner approves
  exactly the text they were shown. `<id>` is random: a rewind rolls the revision back, and a
  resubmission must not replace an earlier copy or reuse its card.

The plan is session state, not part of the candidate: it is never written into the worktree.

### Entering and leaving

| Input | Phase | Effect |
|---|---|---|
| `/plan` | off | Enter planning |
| `/plan <request>` | off | Enter planning and send the request |
| `/plan` | planning | Leave plan mode; the draft stays on disk |
| `/plan` | review | Open the review |
| `/plan off` | any | Leave plan mode and withdraw a pending review |
| `corral-pi build --plan`, `corral-pi chat --plan` | off | Start in planning |
| App action "Plan mode" | off / planning / review | Enter or leave |

Entering or leaving is refused while a turn is open or compacting. `/plan` is no longer
listed as an alias of `/todo`; `/todo` is unchanged.

### What the agent is told

The mode reaches the model only as conversation messages (custom type `corral_plan_note`,
not displayed), returned from the `before_agent_start` hook when a turn starts. The system
prompt and the tool list never change with the mode, so entering and leaving do not
invalidate the prompt cache. The transcript records what each message covered, and the scan
of the branch decides what the next turn still needs:

| Kind | When | Text |
|---|---|---|
| `contract` | Planning or in review, and the contract is not in context (first turn, or after a compaction) | `assets/plan-mode.md` |
| `left` | The owner left without approving, after the contract had been given | Plan mode is off; the draft is not approved |
| `pointer` | A compaction happened after an approval | Where the approved plan's copy is |

Pi can also compact inside a run and continue without `before_agent_start`. The host answers
`session_compact` the same way, appending whatever is due to the transcript at once; Pi
builds its next request from the transcript after that event, so the contract is back before
the run goes on. Nothing is sent while an approval is being carried out, whose own compaction
must not be followed by the planning contract.

There is no notice for the owner's feedback on a submitted plan. The contract and the
`plan_submit` result both say that approval arrives only as a host message whose first line
is `[corral plan approved <id>]` and that any other owner message is feedback, so the agent
needs no host message tied to one particular input.

The contract, adapted from omp's: change nothing and run only read-only commands; only the
plan file is writable; resolve facts by reading and ask the owner only about intent, with a
recommended option; record findings in the plan as they are learned; the plan is an
execution spec another agent can follow with no design decisions left (Context, Approach,
Critical files, Verification, Assumptions and contingencies); finish only with `plan_submit`.

### Host enforcement while planning or in review

In `installToolControls` (`src/host/tool-controls.ts`), before the write-scope branch:

- `edit` or `write` whose destination is `plan.md`: allowed in `planning`, with no
  write-scope classification and no approval prompt. Refused in `review` with
  `A plan is awaiting the owner's review; nothing was written.` A symbolic link at the plan's
  own name never counts as the plan.
- Any other `edit` or `write`: refused with
  `Plan mode: the working tree is read-only. Write the plan to <path>; nothing was written.`
- `task` with any agent whose kind is not `explore` or `review` (an omitted kind included):
  refused with `Plan mode: only children with kind explore or review can start; nothing was
  started.`

Nested Code Mode calls pass through the same `tool_call` hook and get the same answers.

### `plan_submit`

A host tool in the Build and assistant loadouts in every phase. It is registered beside the
role's local tools rather than among them, so children never inherit it.

- Parameters: `{ title?: string }`. The title defaults to the first `# ` heading, then `Plan`.
- Outside `planning` it fails with `Plan mode is not active; nothing was submitted.`
- It fails if `plan.md` is missing, empty or over 256 KiB, naming the path.
- On success it writes the submission's copy, moves to `review`, increments `revision`,
  records the submission's id, copy path and SHA-256 digest, and returns `Submitted for the owner's review. End your turn now and do not implement; the
  owner's choice arrives as a later message.`

### Review

One durable card of kind `plan_review`, with no expiry. Its id is
`plan-<revision>-<submission id>`, so an answer to an older submission is `missing`. The
terminal and the app answer the same submission; the first answer wins.

| Id | Label | Effect |
|---|---|---|
| `execute` | Execute | Keep the conversation as it is |
| `execute_clean` | Execute with clean context | Replace the context with the plan |
| `execute_compact` | Execute with compacted context | Summarise the planning discussion, then execute |
| `revise` | Keep planning | Back to `planning`; the answer's text is the feedback |

Any answer may carry text. With an execute option it is passed to the executor as the
owner's note. Text with no option is `revise`. Feedback given with `revise` is sent as the
owner's message and withdraws the review the way any owner message does, when it reaches the
model. It starts a turn or is refused, never queued, so `delivered` means taken; if the
session does not take it, the answer is refused and the review stays open, so the owner's
text is not lost (the terminal puts it in the composer). `revise` without text returns to
planning at once. One answer is carried out at a time: while an approval or feedback is on
its way into the transcript, any other answer to that review is refused, not acknowledged. Dismissing puts the card aside and stays in
`review`. An ordinary owner message during `review` is feedback: the phase returns to
`planning` and the message is delivered normally. The review is withdrawn at the moment that
message enters the transcript, and only if the host's queue tagged that very message as the
owner's (typed in the terminal or sent by the owner over the socket). A refused message never
gets there, and the same words from an extension or automated input are not the owner's, so
both leave the review open.

**Different model.** The review shows the session's current model with a change control that
opens the existing model picker. The session is idle during review, so the existing
`set_model` path applies unchanged, and whichever execute option is chosen next runs on the
selected model. No new wire is needed for it.

**Terminal.** `src/ui/plan-review.ts` shows the plan in the existing pager with action keys
`e` execute, `c` clean, `s` compacted, `m` model, `k` keep planning (then a feedback input),
and Esc for not now. It opens once per submission when the agent's turn ends and again from
`/plan`. The composer title shows `PLAN`, or `PLAN · review`. A host without panels prints
the submitted copy and then offers the four choices as a plain selection from `/plan`; if
the copy cannot be shown, it offers Keep planning alone.

**App.** A dedicated plan view renders the whole document as Markdown with the four actions,
the model control and a feedback composer. The document is read in 40 KiB pages under the
card's id, so the app never shows one submission's text beside another's choices; the card
body carries the first 8 KiB for older apps.

### Executing an approved plan

On an execute answer the host, in order:

1. Checks the phase is `review`, cancels the turn if one is still running, and waits until
   that turn has settled for everyone. Pi is idle before it has run its settle hooks, and a
   prompt made in that window is deferred behind whatever else was deferred there; the
   kickoff must not join that list.
2. Reads the submission's copy from disk again; it must still match its digest.
3. Applies the context option:
   - `execute`: nothing.
   - `execute_compact`: `session.compact(<instructions>)`, where the instructions keep
     rationale, rejected alternatives, owner preferences and discovered files, drop tool
     noise and superseded drafts, and do not restate the plan. The SDK's "Nothing to compact"
     and "Already compacted" are not failures; execution continues with the context as it is.
   - `execute_clean`: `session.compact()` with a pending reset. The single
     `session_before_compact` handler returns a host-written summary with a null first-kept
     entry, the session manager's own "nothing kept". The summary is the submitted plan with
     a short header, so the cleared context holds the plan whatever happens afterwards. The
     header claims no approval, because it is written before the kickoff is admitted: it
     says to act on the plan only once the kickoff follows.
     Where the SDK refuses to compact (a session smaller than `keepRecentTokens`), the host
     appends the same compaction entry directly and calls `session.refreshContext()`, as
     rewind does.
4. Submits the kickoff message as automated input: the line `[corral plan approved <card id>]`,
   then `assets/plan-approved.md` with the copy's path, the owner's note, instructions to
   open a `corral_todo` plan from the plan's steps and verify each step, and the plan inline
   unless the context was just reset to it. It is admitted through `RemoteOwner.start`:
   ordered against pause like every admission, refused rather than parked in a paused
   session's held queue, and holding that order until its turn is open, so a pause that
   follows cancels the turn. It starts a run or is refused: it is never queued into another
   run, which it could outlive to be drained later after the owner had decided otherwise.
   A rewind is ordered the same way, so the review is checked again after the turn is
   stopped, after the context is prepared and at admission: a kickoff whose review a rewind
   removed is refused. Feedback on a review is admitted through the same ordered start. The
   patched Pi lets one prompt own a run from its start through its settle, so a kickoff
   whose preflight overlapped another prompt's, or the settle of another run, is refused
   before it starts instead of sharing that run's turn; it also refuses a prompt whose
   preflight ends during a manual compaction, such as the approval's own. Pi's preflight is
   asynchronous, and another run can withdraw a review and submit a newer one while an
   answer waits in it, so the patched Pi asks the host once more in the tick the run starts
   (`admit` on the queue observer): an answer to a review that is no longer the open one is
   refused there, before anything is emitted or recorded. So is an answer given in a
   conversation that has since gone on: the host notes the last entry of the model's context
   when the answer is given, and refuses it if another message, summary or context edit
   follows by then. A clean context is therefore the plan alone or the kickoff does not
   start, and the review stays open to be answered again.
5. The review ends when that kickoff message is in the transcript, and only then does the
   Build record note `Approved plan: <title> (revision N, <context option>)`. From the answer
   until then the approval is pending: any other answer to the review is refused, and entering or
   leaving plan mode is refused. Only the approval's own compaction and the kickoff's own run
   go without the planning contract; any other turn that gets in first is still a turn under
   review and is given it, and the kickoff is then refused for the changed conversation. An
   owner message that gets in first is feedback like any other: it withdraws the review, and
   the kickoff is refused for the review that is gone. If the turn ends without the kickoff having reached the
   transcript, the pending state is dropped and the review is answerable again.

Because nothing is recorded before the kickoff itself, any refusal, failure or restart on the
way leaves the same review open, and the reason is posted as a transcript note. A choice is
refused while the session could start nothing: during a compaction, or while the owner has
paused it. The pause is checked again after the context is prepared and decided for good
inside the admission order: a pause that wins keeps the review, and one that loses cancels
the plan's turn.

## Wire contract

Host registry methods (`packages/corral-pi/src/host/remote/plan.ts`):

| Method | Arguments | Result | Refusals |
|---|---|---|---|
| `plan_mode` | `{enabled: boolean}`, or `{command: string}` for a typed `/plan` | `{text}` | `plan_mode_unavailable` while a turn is open, compacting or not ready |
| `plan_mode_doc` | `{card: string, after?: string}` | `{title, revision, body, bytes, offset, next?}` | `plan_mode_doc_missing` when `card` is not the open review; `stale_page` when it stopped being so between pages |

`plan_mode_doc` serves the copy of the submission that `card` reviews, with the same cursor
format as `board_plan_task`.

State: `SessionState.plan_mode?: {phase: "planning" | "review", title?: string, revision?: number}`,
absent when off. `SessionState.plan` remains the todo ledger. Capability ids: `plan_mode`
(unavailable while a turn is open) and `plan_mode_doc`.

Rust (`crates/corral-protocol`, schema regenerated):

- `CardKind::PlanReview`, serialised `plan_review`, before `Other`.
- `SessionState.plan_mode: Option<PlanModeBrief>`;
  `PlanModeBrief {phase: PlanModePhase, title: Option<String>, revision: Option<u32>}`;
  `PlanModePhase {Planning, Review, #[serde(other)] Other}`.
- `SessionAction::PlanMode {enabled: bool}`, forwarded to `plan_mode`.
- `DetailRef::SessionPlanModeDoc {id, card}` and `DetailBody::SessionPlanModeDoc(PlanModeDocPage
  {title, revision, body, bytes, offset})`. The card id is part of the reference, so a
  client's cache of one submission can never answer for another. The daemon reads it like `board_plan_task`;
  `plan_mode_doc_missing` becomes `NotFound` and `stale_page` becomes `Conflict`.

Version skew: an old daemon rewrites the kind to `other` and drops the new state field, so
the app's dedicated view needs the new daemon. The card stays answerable in the terminal.

## Verification

- Host: `test/plan-mode-host.test.ts` runs the production `HostSession` with a scripted
  provider through both reset paths (a session the SDK refuses to compact, and one it
  compacts). It checks that a worktree write is refused while planning and only the plan
  file is written, that the review keeps showing the submitted text after the working file
  changes, and that after "Execute with clean context" the reset summary holds that text,
  none of the planning conversation survives, and writes work again. A second test keeps the
  review open when the session refuses the owner's reply. `src/host/plan-mode.test.ts`
  checks that a resubmission after a rewind gets its own card and copy.
- Protocol: `plan_mode_state_card_and_action_are_typed`, and `checked_in_schema_is_current`.
- Daemon: `a_plan_mode_document_of_two_pages_joins_through_next`.
- App: the model test for the `plan_review` view.

## Assumptions the owner can still override

- **Shell is prompt-only while planning.** To tighten later, route non-read-only commands
  through the existing approval prompt.
- **One plan document per session**, stored outside the worktree. A plan is not committed.
- **No automatic nudge** when a planning turn ends in prose instead of `plan_submit`. If
  models stall in practice, add one reminder per owner input, as omp does.
- **No "start in plan mode" setting and no agent-proposed entry**; neither was selected.
- **The app's New build dialog has no plan-mode checkbox**; start the build, then use the
  Plan mode action before the first message.
- **Model changes apply when picked**, not atomically with the execute choice. Choosing
  "Keep planning" after changing the model continues planning on the new model.
