# Plan mode for corral-pi

Status: design, awaiting owner approval. Nothing here is implemented yet.

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
| Read-only enforcement | Deferred to this document; see the next section |

Task workers and reviewers are excluded: workers keep task-level Plan runs.

## Read-only enforcement: what other harnesses do

Checked in local sources, not from memory:

- **Claude Code** enforces it in the harness. The Agent SDK's `plan` permission mode is
  documented as "Planning mode, no execution of tools" (`sdk.d.ts`).
- **Pi's bundled plan-mode example** disables `edit` and `write` and filters `bash` through a
  read-only allowlist (`examples/extensions/plan-mode`).
- **omp** hard-blocks working-tree writes, renames and deletes (`tools/plan-mode-guard.ts`)
  and lets the agent write only its plan file. Shell is prompt-only.

None of them rely on the prompt alone for file edits; they differ on shell.

**Decision: omp's level.** The host refuses `edit` and `write` outside the plan file, and
refuses writer children. Shell stays prompt-only, with today's fixed refusals and
`bash_rules` unchanged. Reasons: the file guard is a few lines at a hook that already exists
and stops the common failure (the agent starts implementing); a shell allowlist is a parser
that either blocks legitimate read-only commands or gives false assurance, and corral
already says shell rules are not a sandbox.

## How it works

### Phases

`off → planning → review → off`. From `review`, "Keep planning" returns to `planning`.

The phase is saved as a transcript custom entry `corral_plan_mode`
`{version: 1, phase, title?, revision?, digest?, approved?}` at every transition and restored
from the last such entry on the current branch at host start and after a rewind. Rewinding to
a turn before plan mode was entered therefore also leaves plan mode.

### Files

`<state dir>` is the directory that already holds `todo.json`
(`options.build?.dir ?? options.runtimeDir ?? options.agentDir`).

- `<state dir>/plan.md`: the one plan document of the session. The agent creates and revises
  it with the ordinary `write` and `edit` tools.
- `<state dir>/plans/<revision>-<slug>.md`: a copy made at each approval.

The plan is session state, not part of the candidate: it is never written into the worktree.

### Entering and leaving

| Input | Phase | Effect |
|---|---|---|
| `/plan` | off | Enter planning |
| `/plan <request>` | off | Enter planning and send the request |
| `/plan` | planning | Leave plan mode; the draft stays on disk |
| `/plan` | review | Open the review |
| `/plan off` | any | Leave plan mode and withdraw a pending review |
| `corral-pi build --plan`, `corral-pi chat --plan` | new session, or `off` on reopen | Start in planning |
| App action "Plan mode" | off / planning | Same as `/plan` |

Entering or leaving is refused while a turn is open or compacting: stop the turn first.
`/plan` stops being an alias of `/todo` in the app's command list; `/todo` is unchanged.

### What the agent is told

The planning contract is a new asset, `assets/plan-mode.md`, delivered as a conversation
message (custom type `corral_plan_mode`) from the existing `before_agent_start` hook when a
planning turn starts and the contract is not yet in context. It is re-sent after a compaction
while planning. Leaving without approval sends a one-line "plan mode ended" message the same
way. The system prompt and the tool list never change with the mode, so entering and leaving
do not invalidate the prompt cache.

The contract, adapted from omp's:

- Do not change the working tree or run state-changing commands. Only `<plan path>` is
  writable.
- Resolve facts by reading code; ask the owner only about intent, UX, scope and tradeoffs,
  through `ask_user`, with a recommended option. Every question must change the plan.
- Write findings into the plan file as they are learned.
- The plan is an execution spec: another agent with no memory of this conversation must be
  able to execute it with no design decisions left. Sections: Context, Approach (ordered
  steps with exact targets, signatures and literals), Critical files, Verification,
  Assumptions and contingencies. No Non-Goals, Alternatives or Risks sections.
- Finish only by calling `plan_submit`, then end the turn. Never ask for approval in prose or
  with `ask_user`.

### Host enforcement while planning or in review

In `installToolControls` (`src/host/tool-controls.ts`), before the write-scope branch:

- `edit` or `write` whose canonical destination is `plan.md`: allowed in `planning`, with no
  write-scope classification and no approval prompt. Refused in `review` with
  `A plan is awaiting the owner's review; nothing was written.`
- Any other `edit` or `write`: refused with
  `Plan mode: the working tree is read-only. Write the plan to <path>; nothing was written.`
- `task` with any agent whose effective kind is not `explore` or `review`: refused with
  `Plan mode: only explore and review children can start; nothing was started.`

Nested Code Mode calls pass through the same `tool_call` hook and get the same answers.

### `plan_submit`

A host-local tool in the Build and assistant loadouts in every phase.

- Parameters: `{ title?: string }`. The title defaults to the first `# ` heading, then `Plan`.
- Outside `planning` it fails with `Plan mode is not active; nothing was submitted.`
- It fails if `plan.md` is missing, empty or over 256 KiB, naming the path.
- On success it moves to `review`, increments `revision`, records the SHA-256 digest, and
  returns `Submitted for the owner's review. End your turn now and do not implement.`

### Review

One durable card of the new kind `plan_review`, with no expiry. It survives a host restart.
Its id is `plan-<revision>-<digest prefix>`, so an answer to an older submission is
`missing`. The terminal and the app answer the same card; the first answer wins.

Options:

| Id | Label | Effect |
|---|---|---|
| `execute` | Execute | Keep the conversation as it is |
| `execute_clean` | Execute with clean context | Replace the context with the plan |
| `execute_compact` | Execute with compacted context | Summarise the planning discussion, then execute |
| `revise` | Keep planning | Back to `planning`; the answer's text is the feedback |

Any answer may carry text. With an execute option it is passed to the executor as the
owner's note. Dismissing puts the card aside and stays in `review`. An ordinary owner message
sent during `review` is treated as feedback: the phase returns to `planning` and the message
is delivered normally.

**Different model.** The review shows the session's current model with a change control that
opens the existing model picker (Ctrl+L in the terminal, the model picker in the app). The
session is idle during review, so the existing `set_model` path applies unchanged. Whichever
execute option is chosen next runs on the selected model, which gives "different model" in
combination with all three. No new wire is needed for it.

**Terminal.** The existing pager (`ui/pager.ts`) shows the plan with action keys `e` execute,
`c` clean, `s` compacted, `m` model, `k` keep planning (then a feedback input), and Esc for
not now. `m` closes the pager, runs the native model selector, and reopens the review when
the selection settles. The header shows `PLAN` while planning and `PLAN · review` in review.

**App.** A dedicated plan view renders the whole document as Markdown with the four actions,
the model control and a feedback composer. The document is read in 40 KiB pages; the card
body carries the first 8 KiB for older apps. A `PLAN` indicator sits in the session status.

### Executing an approved plan

On an execute answer the host, in order:

1. Checks the card is current, the phase is `review` and `plan.md` still matches the digest.
2. Cancels the turn if one is still open.
3. Copies the plan to `plans/<revision>-<slug>.md`.
4. Applies the context option:
   - `execute`: nothing.
   - `execute_compact`: `session.compact(<instructions>)`, where the instructions keep
     rationale, rejected alternatives, owner preferences and discovered files, drop tool
     noise and superseded drafts, and do not restate the plan. "Nothing to compact" and
     "Already compacted" are not failures; execution continues with the context as it is.
   - `execute_clean`: `session.compact()` with a pending reset. The single
     `session_before_compact` handler checks the reset before the quick-summary logic and
     returns a host-written summary that keeps no earlier entry: "The earlier conversation
     was a planning discussion, cleared at the owner's request. The approved plan follows."
5. Records `phase: off` with `approved: {title, revision, digest, context, model}`.
6. Submits the kickoff message (asset `assets/plan-approved.md`) as host input: the plan
   inline, its durable path, the owner's note, and instructions to initialise `corral_todo`
   from the plan's steps and verify each step. The Build record notes
   `Approved plan: <title> (revision N, <context option>)` rather than the plan text.

Any failure in steps 1 to 4 leaves the phase at `review` with the card still open, and posts
the reason as a transcript note.

After an approval, the first turn following any later compaction gets a short pointer message
naming the approved plan's path, so automatic compaction cannot lose the plan.

## Wire contract

Host registry methods (`packages/corral-pi/src/host/remote/plan.ts`, new):

| Method | Arguments | Result | Refusals |
|---|---|---|---|
| `plan_mode` | `{enabled: boolean}` | `{text}` | `plan_mode_unavailable` while a turn is open, compacting or not ready |
| `plan_doc` | `{after?: string}` | `{title, revision, body, bytes, offset, next?}` | `plan_doc_missing`; `stale_page` when the document changed between pages |

`plan_doc` serves `plan.md` in any phase, with the same cursor helpers as `board_plan_task`.

State: `SessionState.plan?: {phase: "planning" | "review", title?: string, revision?: number}`,
absent when off. Capability ids: `plan_mode` (unavailable while a turn is open) and `plan_doc`.

Rust (`crates/corral-protocol`, schema regenerated):

- `CardKind::PlanReview`, serialised `plan_review`, before `Other`.
- `SessionState.plan: Option<PlanBrief>` with `#[serde(default, skip_serializing_if)]`;
  `PlanBrief {phase: PlanPhase, title: Option<String>, revision: Option<u32>}`;
  `PlanPhase {Planning, Review, #[serde(other)] Other}`.
- `SessionAction::PlanMode {enabled: bool}`, forwarded to `plan_mode`.
- `DetailRef::SessionPlanDoc {id}` and `DetailBody::SessionPlanDoc(PlanDocPage {title,
  revision, body, bytes, offset})`, read by the daemon like `board_plan_task`.

Version skew: an old daemon rewrites the kind to `other` and drops the new state field, so
the app's dedicated view needs the new daemon. The card stays answerable in the terminal.

## Implementation slices

| Slice | Paths | Depends on |
|---|---|---|
| A. Protocol and daemon | `crates/corral-protocol`, `crates/corral-daemon` | none |
| B. Host and terminal | `packages/corral-pi` | none (uses the contract above) |
| C. App | `apps/corral-app` | A |
| D. Guide | `docs/guide/plan-mode.md`, links from `corral-pi.md` and `build-sessions.md`, the card table in `docs/plans/app-agent-parity.md`, `apps/corral-app/AGENTS.md` | B, C |

Critical anchors:

- `packages/corral-pi/src/host/session.ts`: `extension()` (the `before_agent_start` and
  `session_before_compact` handlers, command registration), the local tool list, the
  `RemoteDialogs` existing-card source, `remoteState()`.
- `packages/corral-pi/src/host/tool-controls.ts`: the `edit`/`write` branch.
- `packages/corral-pi/src/host/remote/commands.ts`: `HOST_COMMANDS` and the `/plan` entry.
- `crates/corral-daemon/src/views/session_parts.rs` (`board_plan_task()` as the template) and
  `src/sessions/forward.rs` (`action()`).
- `apps/corral-app/src/model/session/dock.rs` and `dock/fold.rs` (card classification),
  `board.rs` and `plan_brief.rs` (the template for a paged Markdown view).

## Verification

- Host, `bun test` and `bun run typecheck` in `packages/corral-pi`:
  - guard: in planning, `write` to a worktree file is refused, `write` to `plan.md` succeeds,
    `task` with a `slice` child is refused;
  - restore: a transcript whose last `corral_plan_mode` entry is `review` reopens the card,
    and a branch cut before the entry is `off`;
  - clean reset against the real SDK session manager at a small size: after
    `execute_clean`, the model context is the reset summary followed by the kickoff message
    and no planning message;
  - a card answer naming an older revision returns `missing`.
- Protocol: `checked_in_schema_is_current` after regenerating with `CORRAL_UPDATE_SCHEMA=1`.
- Daemon: one read test for `SessionPlanDoc` beside `src/tests/pi_build.rs`.
- App: one model test for the `plan_review` classification and view model; `cargo test` in
  `apps/corral-app`.
- By hand in an isolated build: `/plan add a --version flag`, answer a question, see the
  review, choose "Execute with clean context", and confirm the next request's context holds
  only the plan.

## Assumptions and contingencies

These are the choices the owner can still override:

- **Shell is prompt-only while planning.** To tighten later, route non-read-only commands
  through the existing approval prompt.
- **One plan document per session**, stored outside the worktree. A plan is not committed.
- **No automatic nudge** when a planning turn ends in prose instead of `plan_submit`. If
  models stall in practice, add one reminder per owner input, as omp does.
- **No "start in plan mode" setting and no agent-proposed entry**; neither was selected.
- **The app's New build dialog gets no plan-mode checkbox** in this change; start the build,
  then use the Plan mode action before the first message.
- **Model changes apply when picked**, not atomically with the execute choice. Choosing
  "Keep planning" after changing the model continues planning on the new model.

Unverified until implementation, each with its fallback:

- Whether the SDK accepts an extension compaction that keeps no entry. The SDK's own
  `appendCompaction` defaults `firstKeptEntryId` to the compaction's own id, which keeps
  nothing; the reset uses that. If the SDK refuses, the reset keeps the SDK's default tail
  and the review says so.
- `session.compact()` refuses a session smaller than `keepRecentTokens`. For
  `execute_clean` the fallback is `sessionManager.appendCompaction(...)` followed by
  `session.refreshContext()`, the call rewind already uses. If that does not produce a clean
  context on the Claude SDK lane, execution keeps the context and posts a note.
- Whether the app needs a sidebar or tab entry for the plan view, or opens it from the dock.
