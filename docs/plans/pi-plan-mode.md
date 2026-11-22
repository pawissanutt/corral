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

The phase is a transcript custom entry `corral_plan_mode`
`{version: 1, phase, revision, at, title?, id?, digest?, path?, left?, replied?, approved?}` appended at
every transition. `PlanMode` (`src/host/plan-mode.ts`) keeps no phase of its own: it reads the
current branch, cached by leaf id. A restart therefore restores the phase, and rewinding to a
turn before plan mode was entered leaves it.

### Files

`<state dir>` is the directory that already holds `todo.json`
(`options.build?.dir ?? options.runtimeDir ?? options.agentDir`).

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
invalidate the prompt cache. The transcript records each message, and the scan of the branch
decides which one, if any, the next turn still needs:

| Kind | When | Text |
|---|---|---|
| `contract` | Planning, and the contract is not in context (first turn, or after a compaction) | `assets/plan-mode.md` |
| `left` | The owner left without approving, after the contract had been given | Plan mode is off; the draft is not approved |
| `replied` | The owner answered a review with an ordinary message | Treat it as feedback, revise, submit again |
| `pointer` | A compaction happened after an approval | Where the approved plan's copy is |

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
owner's note. Text with no option is `revise`. Dismissing puts the card aside and stays in
`review`. An ordinary owner message during `review` is feedback: the phase returns to
`planning` and the message is delivered normally. If the session refuses that message, the
same review is put back.

**Different model.** The review shows the session's current model with a change control that
opens the existing model picker. The session is idle during review, so the existing
`set_model` path applies unchanged, and whichever execute option is chosen next runs on the
selected model. No new wire is needed for it.

**Terminal.** `src/ui/plan-review.ts` shows the plan in the existing pager with action keys
`e` execute, `c` clean, `s` compacted, `m` model, `k` keep planning (then a feedback input),
and Esc for not now. It opens once per submission when the agent's turn ends and again from
`/plan`. The composer title shows `PLAN`, or `PLAN · review`. A host without panels offers
the four choices as a plain selection from `/plan`.

**App.** A dedicated plan view renders the whole document as Markdown with the four actions,
the model control and a feedback composer. The document is read in 40 KiB pages under the
card's id, so the app never shows one submission's text beside another's choices; the card
body carries the first 8 KiB for older apps.

### Executing an approved plan

On an execute answer the host, in order:

1. Checks the phase is `review` and reads the submission's copy, which must still match its
   digest.
2. Cancels the turn if one is still open.
3. Applies the context option:
   - `execute`: nothing.
   - `execute_compact`: `session.compact(<instructions>)`, where the instructions keep
     rationale, rejected alternatives, owner preferences and discovered files, drop tool
     noise and superseded drafts, and do not restate the plan. The SDK's "Nothing to compact"
     and "Already compacted" are not failures; execution continues with the context as it is.
   - `execute_clean`: `session.compact()` with a pending reset. The single
     `session_before_compact` handler returns a host-written summary with a null first-kept
     entry, the session manager's own "nothing kept". The summary is the approved plan with
     a short header, so the cleared context holds the plan whatever happens afterwards.
     Where the SDK refuses to compact (a session smaller than `keepRecentTokens`), the host
     appends the same compaction entry directly and calls `session.refreshContext()`, as
     rewind does.
4. Records `phase: off` with `approved: {title, revision, digest, path, context, model}`.
5. Submits the kickoff message (`assets/plan-approved.md`) as automated input: the copy's
   path, the owner's note, instructions to open a `corral_todo` plan from the plan's steps
   and verify each step, and the plan inline unless the context was just reset to it. The
   Build record notes `Approved plan: <title> (revision N, <context option>)` rather than
   the plan text.

Any failure in steps 1 to 3 leaves the phase at `review` with the card still open, and posts
the reason as a transcript note.

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
