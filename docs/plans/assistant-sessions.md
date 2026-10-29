---
human_review:
  problem: "The owner has one bound PM. Every board question, research request and drafting job goes through it and competes with its board-operating context, so the owner waits or derails the PM. Running an agent from a plain shell is worse: corral attributes its mutations to the owner, because owner-only guards only check environment variables and run worktrees."
  changes:
    - "Add a new session kind, the assistant (short label asst): any number of interactive corral-pi sessions per workspace, each with its own durable session record (A-<n>), transcript, artifacts, cost and resume."
    - "Make `corral pi` in a workspace open an assistant; `corral pi --pm` opens the PM as today; outside a workspace it behaves as `--bare`."
    - "Give assistants read access to the whole board, research capabilities from the T-460 assistant capability column, filing (new tasks, ideas, comments), session artifacts and PM inbox items, with every CLI mutation attributed to `assistant:A-<n>`."
    - "Add a durable PM inbox (`.corral/pm-inbox/PMI-<n>.md`) that assistants post to, the PM consumes on its sweep through `pm next`, and that records each item's outcome."
    - "Let an assistant act with the owner's authority only for an action the owner explicitly asked for in that session, verified against a host-recorded owner input or a host-rendered confirmation, receipted and attributed as `owner via assistant:A-<n>`."
    - "Add scratch worktrees for spikes and co-work, with lifecycle, garbage collection (diff saved before removal) and disk accounting."
    - "Let owner-co-worked scratch changes finish into a task record that gates, optionally reviews (default none) and lands through the normal approve path on the owner's explicit instruction."
    - "Add task focus (`corral pi --task T-x`) with a bounded preloaded brief, a transcript reader for worker and reviewer runs, and board integration."
  retained:
    - "Exactly one PM. Its binding, approved project scope, tool loadout, wakes and `corral pi` adopted-pane semantics are unchanged; the PM only gains inbox read/resolve operations."
    - "Owner-only commands still refuse PM and agent entry points. The new owner path is one typed, receipted operation, not a relaxed guard."
    - "Landing keeps its gate, landing lock, integration, size and freshness checks; autopilot, operating grants and review policy for ordinary tasks are untouched."
    - "Unattended assistant spikes never land; promotion of a spike means filing a task."
    - "`corral pi --bare` is unchanged. Assistants take no coding slot and never type into another pane."
goal: "Add many short-lived assistant sessions for querying, research, analysis, filing and owner co-work, separate from the single PM, with a durable PM inbox for handoffs and a verified, receipted path for explicitly instructed owner actions."
non_goals:
  - "No second PM, no PM-to-assistant delegation, no assistant-driven autopilot or background board operation."
  - "No OS sandbox: owner binding is a misattribution and audit guard against an honest but fallible model, not a boundary against an adversarial process running as the owner's user (see Threat model)."
  - "No OmO or Claude assistant harness; assistants are Pi-native only. Other harnesses keep `corral pi --bare` or their own shells."
  - "No new capability resolver or tool gating system; the assistant column is one more role in T-460's catalog."
  - "No change to how ordinary tasks are reviewed or accepted, and no review bypass outside owner-bound co-work landings."
  - "No cross-machine sessions, no shared assistants between owners, no assistant concurrency hard limit beyond the soft cap and the existing machine-load check."
decisions:
  - "Owner (2026-10-29): name is assistant, short label asst; many assistants, one PM; durable PM inbox in .corral with recorded outcomes and nothing typed into the PM pane; scratch worktrees allowed and spikes never land; owner authority on explicit instruction with ambiguous or out-of-scope requests routed to the PM instead of refused; co-work in the same role without a prior task, gate on finish, review optional with default none, landing on explicit instruction."
  - "The assistant is a sixth Pi-native role (`AgentRole::Assistant`) with its own container record, generations and usage, not a fake worker run or PM alias. It consumes T-563's resolver with its own column."
  - "Workspace-scoped, not project-bound: reads span the workspace; every mutation names its project and is checked against that project at commit."
  - "Reads and filing use the existing CLI from the assistant's shell. The host exports `CORRAL_ASSISTANT=A-<n>`; one exhaustive command classification in the CLI entry point allows read and filing commands, attributes them to `assistant:A-<n>`, and refuses every other mutation with a pointer to the typed owner operation or the PM inbox."
  - "Owner-level actions use one typed operation, `owner_action`, over the existing authenticated receipt client. Rust verifies a binding to a host-journaled owner input from the current turn (literal quote naming the target) or a host-rendered confirmation of the exact action; destructive actions always need the confirmation."
  - "The PM inbox is a separate store from the corral-to-agent agent inbox. Items are revisioned Markdown records; the PM sees them as `pm next` cards and resolves them through audited commands or Pi PM tools."
  - "Co-work finishing materializes a task record (origin assistant, label cowork) at finish time so the existing gate, approve, history, stats and search keep one data model; no task exists before or during the work."
  - "Implementation children depend on T-563 (capability-framework) in addition to their local edges; this plan does not wait on later T-460 waves. Features that later waves ship appear in the assistant column only when their child lands."
acceptance:
  - criterion: C1
    scenario: "In a herdr pane inside the workspace the owner runs `corral pi`, then `corral pi` in a second pane, then `corral pi --pm` in a third, then `corral pi` in /tmp."
    expected: "Two assistant sessions A-1 and A-2 open with role assistant and separate records; the third pane opens or takes over the PM exactly as T-490's bound mode did; /tmp starts bare Pi with a one-line notice. No coding slot is taken. `corral assistant list` shows A-1 and A-2 live with their panes."
  - criterion: C2
    scenario: "Assistant A-1 runs `corral show T-5 --json`, `corral stats --json`, `corral new \"Fix X\" -p corral --idea`, `corral comment T-5 ...` and `corral artifact add --session A-1 report.md --kind report`, then tries `corral approve T-5`, `corral move T-5 ready` and `corral config set ...` from its shell."
    expected: "Reads succeed; the idea, comment and artifact are recorded with actor assistant via `assistant:A-1`; each refused command exits non-zero with `assistant_refused` naming `corral_owner_action` or `corral pm inbox post`, and changes nothing."
  - criterion: C3
    scenario: "The owner types `approve T-5` in A-1's pane; the model calls owner_action approve T-5 quoting that text. Separately the model calls owner_action approve T-6 citing the same turn, and calls it again after the turn has ended."
    expected: "T-5 lands through the normal approve path with an `approved` event attributed owner via `assistant:A-1` carrying session, generation, turn ID, input digest and quote. The T-6 call refuses `instruction_unbound` (target not in the instruction) and the stale call refuses `turn_closed`; neither changes the board."
  - criterion: C4
    scenario: "A corral wake, a PM outcome message and a recovery notice arrive in A-1's session; the model cites each as an owner turn."
    expected: "Each refuses `not_owner_input`: only pane-typed input and host-rendered confirmations are journaled as owner input."
  - criterion: C5
    scenario: "The owner says `do it` after the assistant proposed `dispatch T-9`; then asks it to discard task T-3."
    expected: "The literal binding fails, so the host renders `Run: corral dispatch T-9? [y/N]`; y executes once with binding `confirmed`. The discard is destructive and requires its own confirmation even though the owner named T-3; N records a declined receipt and executes nothing."
  - criterion: C6
    scenario: "The owner asks A-1 to `land whatever is ready` and separately to `approve T-5` while autopilot holds T-5's approval lock."
    expected: "The ambiguous request becomes PM inbox item PMI-1 with the verbatim quote and the assistant's reading; the assistant tells the owner `routed to the PM as PMI-1; follow with corral pm inbox show PMI-1`. The T-5 approval refuses `conflict_in_flight` naming the autopilot lock and is reported, not retried or overridden."
  - criterion: C7
    scenario: "The PM runs `corral pm next` with PMI-1 open, claims it at revision 0, then resolves it done with refs; a second claim at revision 0 is attempted."
    expected: "The card lists PMI-1 with source session and kind; claim and resolve record PM attribution, revisions 1 and 2, and an outcome; the stale claim refuses `stale_revision`. Live A-1 receives one native inbox message `pmi:PMI-1:2`; nothing is typed into the PM pane."
  - criterion: C8
    scenario: "A-1 creates scratch `spike-cache` in project corral, commits, ends the session; `corral assistant scratch gc --dry-run` then runs after the idle period, then `corral assistant scratch gc --yes`."
    expected: "The scratch lives at `<repo>/.worktrees/A-1-spike-cache` on branch `corral/A-1/spike-cache`; `corral stats --storage` shows its bytes. Dry run lists it without changes; the real run saves a diff artifact on A-1, removes the worktree and branch, and skips any scratch with a live process cwd, pane or pending finish."
  - criterion: C9
    scenario: "The owner co-works in A-1's scratch, then says `finish this as a fix for the cache, no review` and later `approve T-60`."
    expected: "A confirmation shows the diffstat and review choice; on y, task T-60 is created (origin assistant, label cowork, session A-1, run R1 on the scratch branch), the gate runs, and T-60 reaches Review with reviewer none. `approve T-60` lands it through landing lock and integration checks; T-60 is Done with A-1's usage on its run and the session record lists the landed commits."
  - criterion: C10
    scenario: "An unattended spike (created without an owner-bound cowork confirmation) is offered for finish; a spike is promoted instead."
    expected: "Finish refuses `spike_not_cowork`. Promote files a Backlog task through `corral new` with the spike diff attached as an artifact and leaves the scratch unchanged."
  - criterion: C11
    scenario: "`corral pi --task T-42` opens; the model reads the brief and then `corral run transcript T-42 R1 --lines 1-200` and the review child transcript."
    expected: "The brief (at most 24 KiB) carries spec, plan summary, runs, review notes and artifact refs; the transcript reader returns bounded text for OmO, Claude and Pi transcripts or a named diagnostic; focus grants no task authority."
  - criterion: C12
    scenario: "Five assistants are live with `assistant.max_live = 4`; the machine-load probe reports critical."
    expected: "The fifth `corral pi` warns and needs `--over-cap`; at critical load the launch refuses `machine_load` with the probe values. `corral stats` shows assistant cost by session and in a role row."
affected_paths:
  - crates/corral/src/pi_cmd.rs
  - crates/corral/src/entry.rs
  - crates/corral/src/main.rs
  - crates/corral/src/assistant_cmd.rs
  - crates/corral/src/pm_cmd.rs
  - crates/corral/src/inbox_cmd.rs
  - crates/corral-core/src/agent/records.rs
  - crates/corral-core/src/attribution.rs
  - crates/corral-core/src/model.rs
  - crates/corral-core/src/assistant.rs
  - crates/corral-core/src/pm_inbox.rs
  - crates/corral-ops/src/agent/pi.rs
  - crates/corral-ops/src/agent/call.rs
  - crates/corral-ops/src/agent/ops/assistant.rs
  - crates/corral-ops/src/agent/ops/pm.rs
  - crates/corral-ops/src/automation_controls.rs
  - crates/corral-ops/src/assistant
  - crates/corral-ops/src/pm_next.rs
  - crates/corral-ops/src/review.rs
  - crates/corral-ops/src/cleanup_worktree.rs
  - crates/corral/src/doctor
  - crates/corral-ops/src/stats.rs
  - crates/corral-ops/src/usage.rs
  - crates/corral-ops/src/artifacts.rs
  - crates/corral-tui/src
  - packages/corral-pi/src/roles/assistant
  - packages/corral-pi/src/roles/types.ts
  - packages/corral-pi/src/roles/pm/tools.ts
  - packages/corral-pi/src/host/session.ts
  - docs/guide/pi-native.md
  - docs/guide/pm.md
  - docs/guide/assistants.md
contracts:
  - "AgentRole::Assistant and Actor::Assistant (via `assistant:A-<n>`)"
  - "AssistantSession record v1 (.corral/assistants/A-<n>.md) and container (.corral/state/agent-assistant/A-<n>.json)"
  - "CommandClass table: Read | File | Refused, exhaustive over Cmd"
  - "OwnerInput journal entry v1 and OwnerAuthority {Shell, Tui, ViaAssistant(receipt)}"
  - "AgentOp::OwnerAction with OwnerActionKind (closed enum) and OwnerBinding {Literal, Confirmed}"
  - "PmInboxItem v1 (.corral/pm-inbox/PMI-<n>.md) and its state machine"
  - "Scratch v1 inside AssistantSession, branch corral/A-<n>/<slug>"
  - "Task.cowork {session, scratch, review: none|required}"
  - "T-460 CapabilitySet column `assistant`"
dependencies:
  - task: assistant-sessions
    write_scope: [crates/corral-core/src/agent/records.rs, crates/corral-core/src/attribution.rs, crates/corral-core/src/assistant.rs, crates/corral/src/assistant_cmd.rs, crates/corral/src/main.rs, crates/corral-ops/src/stats.rs, crates/corral-ops/src/usage.rs]
    depends_on: []
    acceptance:
      - scenario: "Create, list, show and end two sessions through the store API and CLI; read stats."
        expected: "IDs A-1 and A-2 are allocated atomically; records and containers round-trip; ended sessions keep transcript paths; `corral stats` has an assistant role row and per-session cost lines (unknown until the T-460 usage collector reports)."
  - task: assistant-launch
    write_scope: [crates/corral/src/pi_cmd.rs, crates/corral-ops/src/agent/pi.rs, crates/corral-ops/src/agent/call.rs, crates/corral-ops/src/agent/ops/assistant.rs, packages/corral-pi/src/roles/assistant, packages/corral-pi/src/roles/types.ts, docs/guide/pi-native.md, docs/guide/assistants.md]
    depends_on: [assistant-sessions]
    acceptance:
      - scenario: "C1 and C12 in an isolated herdr server; resume A-1 after killing its host."
        expected: "Invocation split, soft cap and load refusal behave as C1/C12; `corral pi --resume A-1` fences generation 1 and opens generation 2 in the current pane; doctor shows the resolved assistant column and digest."
  - task: assistant-entry-guard
    write_scope: [crates/corral/src/entry.rs, crates/corral/src/inbox_cmd.rs, crates/corral-ops/src/automation_controls.rs, crates/corral/tests]
    depends_on: [assistant-sessions]
    acceptance:
      - scenario: "C2 for every Cmd variant through a table-driven test with CORRAL_ASSISTANT set."
        expected: "Every variant has an explicit class; read and file commands carry assistant attribution; refused commands change nothing; owner guards refuse an assistant entry even without AI_AGENT or PI_CODING_AGENT."
  - task: assistant-artifacts
    write_scope: [crates/corral-core/src/model.rs, crates/corral-ops/src/artifacts.rs, crates/corral-ops/src/artifact_gc.rs, crates/corral/src/artifact_cmd.rs]
    depends_on: [assistant-sessions]
    acceptance:
      - scenario: "Add, list, show, pin, search and GC session artifacts `A-1:report.md`."
        expected: "Snapshots live under `.corral/artifacts/assistant/A-1/`; refs, tiers, search and GC treat them like run artifacts; an assistant cannot add to another session or to a task run."
  - task: pm-inbox
    write_scope: [crates/corral-core/src/pm_inbox.rs, crates/corral/src/pm_cmd.rs, crates/corral-ops/src/pm_next.rs, crates/corral-ops/src/agent/ops/pm.rs, packages/corral-pi/src/roles/pm/tools.ts, docs/guide/pm.md]
    depends_on: [assistant-entry-guard, assistant-launch]
    acceptance:
      - scenario: "C7, plus withdraw by the author, a post from a refused-class command and an OmO PM sweep."
        expected: "State machine and revisions hold; OmO sees the card through `corral pm next` without any pane push; outcomes reach a live assistant through the native agent inbox once, deduplicated."
  - task: owner-authority
    write_scope: [packages/corral-pi/src/host/session.ts, packages/corral-pi/src/roles/assistant, crates/corral-ops/src/agent/ops/assistant.rs, crates/corral-ops/src/assistant, crates/corral-core/src/assistant.rs, crates/corral-ops/src/automation_controls.rs]
    depends_on: [assistant-launch, assistant-entry-guard, pm-inbox]
    acceptance:
      - scenario: "C3, C4, C5 and C6, plus host crash between confirmation and execution, and a replayed call."
        expected: "Bindings verify as specified; events carry the receipt; a replay returns the first receipt without a second effect; a crash leaves the receipt recoverable as executed or not-executed, never guessed."
  - task: scratch-worktrees
    write_scope: [crates/corral-ops/src/assistant, crates/corral-ops/src/cleanup_worktree.rs, crates/corral-ops/src/stats.rs, crates/corral/src/assistant_cmd.rs, crates/corral/src/doctor, packages/corral-pi/src/roles/assistant]
    depends_on: [assistant-entry-guard, assistant-artifacts]
    acceptance:
      - scenario: "C8 and the promote half of C10; edit/write tools aimed at the main checkout or a task worktree."
        expected: "Lifecycle, accounting and GC behave as C8; promotion files a task with the diff; the host refuses edits outside the session's scratch and artifact roots."
  - task: cowork-landing
    write_scope: [crates/corral-ops/src/assistant, crates/corral-core/src/model.rs, crates/corral-ops/src/review.rs, crates/corral-ops/src/stats.rs, docs/guide/assistants.md]
    depends_on: [scratch-worktrees, owner-authority]
    acceptance:
      - scenario: "C9, the finish half of C10, `review required` at finish, and finish into an existing task without a live run."
        expected: "Materialization, gate, optional review and landing behave as C9; reviewer none applies only to cowork tasks; finish into an existing task adds a new run there, and refuses `task_busy` when it has a live run."
  - task: task-focus-transcripts
    write_scope: [crates/corral/src/pi_cmd.rs, crates/corral-ops/src/assistant, crates/corral/src/run_cmd.rs, crates/corral-ops/src/usage.rs]
    depends_on: [assistant-launch]
    acceptance:
      - scenario: "C11 for OmO, Claude and Pi-native runs and a reviewer child; a missing transcript."
        expected: "Bounded brief and transcript pages; the missing file yields `transcript_unavailable` with the searched location."
  - task: assistant-board
    write_scope: [crates/corral-tui/src]
    depends_on: [pm-inbox, cowork-landing, task-focus-transcripts]
    acceptance:
      - scenario: "TUI driver: assistants panel, open-assistant-from-card, PM inbox list and a Done cowork card."
        expected: "The panel lists live and recent sessions with focus, cost, scratch count and open inbox items; the card key opens `corral pi --task <id>` in a new pane of the isolated server; inbox items and cowork cards are labeled asst."
verification:
  - "Changed-file diagnostics and `cargo test -p <crate>` for each touched crate; table-driven Cmd classification test in crates/corral/tests."
  - "`scripts/pi-lane.sh` for packages/corral-pi role and host changes."
  - "Live pane scenarios (C1, C3 to C6, C9, C11, C12) only inside `source scripts/test-herdr.sh && corral_test_herdr_start`, never the owner's server."
  - "Crash and concurrency fixtures subscribe to exact barriers before acting; no sleeps; any load test only through scripts/load-sandbox.sh."
  - "Each child runs the gate its run contract names; landing runs the full gate."
owner_decisions:
  - "OD1 `corral pi` default: recommend the assistant in a workspace and `--pm` for the PM (changes T-490's default; the launch prints which one opened)."
  - "OD2 Threat model: recommend accepting owner binding as a misattribution and audit guard now, with a hard boundary only once T-460's deferred sandbox idea exists; reject delaying owner actions until then."
  - "OD3 Binding strength: recommend literal current-turn quote or host confirmation, with confirmation always for destructive actions; alternative is a confirmation for every owner action."
  - "OD4 PM wake for inbox items: recommend a native, deduplicated wake for a Pi-native PM and sweep-only for an OmO PM; alternative is sweep-only everywhere."
  - "OD5 Co-work record: recommend materializing a task at finish; alternative is a new non-task history entity read by every board, history and stats reader."
  - "OD6 Defaults: recommend assistant.max_live 4 (soft), assistant.scratch_max 8, scratch idle GC after 7 days with a diff artifact saved, owner-input binding limited to the current turn."
  - "OD7 Session ID prefix: recommend A-<n>."
---

# Plan: assistant sessions

Status: revision 1 (2026-10-29), design for T-544. Implementation waits for owner acceptance
and for T-563 (capability-framework) to land; see "Members and waves".

## Problem

The owner has one PM. It is bound to one approved project, runs the board loop's
judgment calls, and holds its context for that. Every other thing the owner wants from an
agent (a board question, a stats breakdown, research, a drafted spec, a quick spike, a
session of pair work) either goes to the PM, where it competes with board operation, or
to a plain shell, where corral cannot tell the agent from the owner.

That second path is worse than it looks. `Entry::resolve` (`crates/corral/src/entry.rs`)
attributes a CLI call to the owner unless it runs inside a task run's worktree or
`CORRAL_PM=1` is set. The owner-only guards (`require_owner_entry` in
`crates/corral-ops/src/automation_controls.rs`, `caller` in `crates/corral/src/inbox_cmd.rs`,
and the plan member commands) refuse only on `AI_AGENT`, `PI_CODING_AGENT`, `CORRAL_PM` or a
run worktree cwd. Most mutating commands, `corral approve` included, have no such check at
all. An agent started in the workspace root therefore acts as the owner without trace.

People affected: the owner, who waits on the PM or gives up attribution; the PM, whose
context is diluted; reviewers of history, who cannot tell owner actions from agent actions.

## Goal

Add many short-lived **assistant** sessions for querying, research, analysis, filing and
co-work, separate from the single PM. Assistants hand board-operating requests to the PM
through a durable inbox, and perform an owner-level action only when the owner explicitly
asked for it in that session, through a verified, receipted path.

## Included changes and retained behavior

Included, in delivery order:

1. **Assistant role and sessions.** A sixth Pi-native role with its own session records,
   transcript, artifacts, cost and resume; `corral pi` opens one, `corral pi --pm` the PM.
2. **Attributed reads and filing.** Assistants use the corral CLI from their shell. A single
   classification at the CLI entry point allows read and filing commands under
   `assistant:A-<n>` attribution and refuses everything else.
3. **PM inbox.** Durable request items the PM consumes on its sweep, each with an outcome.
4. **Owner authority on explicit instruction.** One typed operation that executes an
   owner-level action bound to the owner's own words or confirmation.
5. **Scratch worktrees** with lifecycle, GC and disk accounting.
6. **Co-work landing**: finish a co-worked scratch into a task, gate it, optionally review
   it, and land it on the owner's instruction.
7. **Task focus and transcripts.**
8. **Board integration.**

Retained: one PM with its current binding, scope, tools and adopted-pane behavior; every
owner-only guard (assistants are refused there as agents are); landing checks; review and
acceptance policy for ordinary tasks; `corral pi --bare`; no slot use and no typing into
other panes.

## Exclusions

- No second PM, no assistant-run autopilot, no background board operation by assistants.
- No OS-level security boundary (see Threat model); no OmO or Claude assistant harness.
- No new capability resolver: the assistant is a column in T-460's catalog.
- No review bypass for ordinary tasks; only owner-bound co-work tasks may choose none.
- No cross-machine or multi-owner sessions; no hard concurrency limit.

## Choices

- **A real role, not a disguised worker or PM.** A worker run implies a task, slot, gate
  contract and completion; a PM alias implies a single bound project and board authority.
  The assistant has neither, so it gets `AgentRole::Assistant` and its own container, which
  is what T-460 P3's handoff asks for.
- **CLI for reads and filing, a typed tool only for owner authority.** The owner asked
  that assistant writes go through existing audited CLI paths with revision checks. The CLI
  already has them; duplicating forty commands as typed tools would add surface without
  safety. Owner-level actions are different: they need a binding to the owner's words that
  only the host can supply, so they get one typed, receipted operation.
- **Classification at one choke point.** `Entry::new` already sees the parsed `Cmd` for every
  invocation. A `match` with no wildcard arm makes each new command choose its class at
  compile time, so an assistant cannot reach a new mutating command by default.
- **Bind to the owner's input, not the model's claim.** The Pi host already distinguishes
  pane-typed input (`event.source === "interactive"`, `packages/corral-pi/src/host/session.ts`)
  from programmatic input. Journaling that input before the model sees it gives Rust an
  independent record to check citations against.
- **Route instead of refuse.** Anything the assistant cannot bind or may not do becomes a
  PM inbox item with the owner's verbatim words, and the assistant tells the owner how to
  follow it.
- **Materialize a task at finish.** Gate, approve, history, stats, search and the board all
  key on tasks. Creating the record when the owner says "finish" reuses all of them, and
  still satisfies "no task before or during the work".

Trade-offs and rejected alternatives are listed under "Alternatives" below.

## Done scenarios

The YAML acceptance list (C1 to C12) is canonical. In short: sessions open and resume
without slots (C1, C12); reads and filing work and other mutations are refused with a
route (C2); explicit owner instructions execute once with a receipt, and anything else is
refused (C3 to C5); ambiguity and conflicts go to the PM or are reported (C6); the PM
consumes and resolves inbox items without pane typing (C7); scratch worktrees are
accounted and collected without losing work (C8); co-work lands through the normal path
and spikes never do (C9, C10); focus and transcripts are bounded (C11).

## Implementation

### Evidence and existing contracts

| Fact | Where |
|---|---|
| `corral pi` has `--bare`, `--model`, `--provider`, `--effort`; bound mode calls `pi::bound` | `crates/corral/src/pi_cmd.rs`, `crates/corral-ops/src/agent/pi.rs` |
| PM adoption record and pane rules | `.corral/state/agent-pm.json`, `docs/session-ownership.md` |
| Roles: worker, reviewer, planner, plan_reviewer, pm | `AgentRole`, `crates/corral-core/src/agent/records.rs` |
| Actors: owner, pm, agent, autopilot, reviewer, system | `Actor`, `crates/corral-core/src/attribution.rs` |
| CLI attribution defaults to owner outside run worktrees | `Entry::resolve`, `crates/corral/src/entry.rs` |
| Owner guards check env and run worktrees only | `require_owner_entry`, `inbox_cmd::caller`, `plan/member_*.rs` |
| Agent ops and role tables | `AgentOp` and role lists, `crates/corral-ops/src/agent/call.rs` |
| Pi role modules and op names | `packages/corral-pi/src/roles/types.ts`, `roles/pm/tools.ts` |
| Host tags pane input `interactive` | `packages/corral-pi/src/host/session.ts` (input handler) |
| Approve requires a fresh review unless the resolved reviewer is none | `crates/corral-ops/src/review.rs` |
| Landing lock, integration, size checks | `approve_landed`, `docs/guide/acceptance.md` |
| Machine-load probe | `ResourceProbe`, `crates/corral-ops/src/resources.rs` |
| Artifacts are keyed by task and run | `Artifact`, `crates/corral-core/src/model.rs` |
| T-460 P3 handoff: assistant column from the W standard template, after capability-framework | T-460 plan "Existing T-544 handoff" |

### Session model and invocation

**Identity.** Sessions are `A-<n>` from a workspace counter allocated under the board
writer lock. Two records per session:

- Container `.corral/state/agent-assistant/A-<n>.json`: an `AgentRunState` with role
  `assistant`, binding (pane, herdr socket, workspace), generations, receipts, turns,
  wakes, the frozen CapabilitySet digest per generation, and the owner-input journal.
- Session record `.corral/assistants/A-<n>.md`: YAML frontmatter (`id`, `created`, `ended`,
  `focus` = workspace or task ID, `profile`, `scratch[]`, `artifacts[]`, `inbox_items[]`,
  `landed[]` task links, `summary`) and a free Markdown body the assistant may write via
  `corral assistant summary A-<n> --file`. Durable and searchable like tasks.

**Invocation** (`crates/corral/src/pi_cmd.rs`):

| Command | Result |
|---|---|
| `corral pi` in a herdr pane inside a workspace | new assistant in this pane |
| `corral pi --pm` | today's bound PM behavior, unchanged refusal codes |
| `corral pi --resume A-<n>` | fence the old generation, open the next one in this pane |
| `corral pi --task T-x` | new assistant with task focus |
| `corral pi` outside a workspace, or `--bare` | bare Pi (outside a workspace a one-line notice) |

The assistant pane is the owner's pane, adopted exactly like the PM's (`adopted_pane`
rules: never closed by corral, never typed into). Launch reuses the PM path's asset,
credential and provider checks but needs **no** approved operating scope, because the
assistant is not bound to a project and has no board authority of its own.

**Admission.** No coding slot. `assistant.max_live` (OD6, default 4) is soft: above it,
`corral pi` refuses `assistant_cap` unless `--over-cap` is given. The `ResourceProbe`
sample refuses `machine_load` at the critical state and prints the values. Subagents and
background jobs stay under T-460's per-feature caps.

**End and resume.** `/exit` or `corral assistant end A-<n>` closes the container, removes
the runtime directory, keeps the Pi session file as the transcript, and cancels its
undelivered inbox messages, as for ended runs. Scratch worktrees stay until GC or finish.

**Records in stats.** `corral stats` gains an `assistant` role row and per-session lines;
cost comes from the T-460 usage collector (T-570) and reads `unknown` until it lands.
`corral assistant list|show` gives status, focus, pane, cost, scratch and open items.

### Capability column (T-460 resolver)

The assistant column uses T-563's resolver, digest, doctor and resume rules; it is not a
separate gating system. Cells are **strict / standard / full**, notation as in T-460 P3.

| Key | A | Note |
|---|---|---|
| `context_files` | 0/1/1 | |
| `skills.enabled` | 0/1/1 | home `~/.agents/skills` as for W; the packaged `corral-assistant` contract always loads |
| `prompt_templates` | 0/1/1 | |
| `compaction` | D/D/D | |
| `lessons` | - | read on demand with `corral lessons` |
| `session_export` | 0/1/1 | the transcript itself is always kept |
| `todo` | 0/1/1 | |
| `background_shell` | 0/1/1 | jobs owned by the session, killed at end |
| `codemode` | 0/1/1 | |
| `mcp.enabled`, `tool_search` | 0/1/1 | workspace-configured servers only |
| `web.enabled` | 0/1/1 | |
| `lsp` | 0/1/1 | |
| `subagents` | 0/1/1 | explore-only, as T-460 OD5 |
| `hooks` | 0/0/1 | scratch roots only |
| `bash_rules` | 0/0/1 | plus fixed assistant rules below, not toggleable |
| `oracle` | 0/0/1 | |
| `checkpoints` | 0/0/1 | scratch worktrees only |
| `browser` | 0/0/1 | research use; same policy as W |
| `budget` | U/U/U | |
| `headless` | - | assistants are interactive by definition |
| `llama_cpp` | 0/0/0 | |

Core tools in every preset: read, grep, find, ls, bash, the typed assistant operations, and
edit/write limited by a fixed host path guard to the session's scratch worktrees,
`.corral/artifacts/assistant/A-<n>/` and the system temp directory. Fixed bash rules deny
`herdr pane` input commands and direct writes under `.corral/` (they stop accidents by an
honest model; see Threat model). No preset grants owner authority.

### Entry authority: command classification

The host exports `CORRAL_ASSISTANT=A-<n>` (with the container's generation) into the
session's shell. `Entry::new` maps it to `Actor::Assistant` with via `assistant:A-<n>` and
then applies `fn class(cmd: &Cmd) -> CommandClass`:

- `Read`: show, list, search, stats, log, review readback, plan show/progress, queue and
  pm readbacks, artifact list/show, lessons, events, `run transcript`, doctor, etc.
- `File`: `new` (Idea or Backlog only; a Ready or dispatching target is refused), `idea` capture,
  `comment`, `artifact add --session <own>`, `pm inbox post|withdraw` (own items),
  `assistant scratch new|list|discard|promote` and `assistant summary` (own session).
- `Refused`: everything else, with exit code 2 and
  `assistant_refused: <cmd> needs the owner; use corral_owner_action or corral pm inbox post`.

The `match` has no wildcard arm. A table-driven test enumerates all `Cmd` variants. The
owner guards (`require_owner_entry`, `inbox_cmd::caller`, plan member commands) treat
`CORRAL_ASSISTANT` like `AI_AGENT`. Scratch worktrees sit under `.worktrees/` but are not
task runs, so `run_from_cwd` correctly does not label them as runs; the assistant
variable decides attribution there.

### Owner authority on explicit instruction

**Owner-input journal.** For each pane-typed input (Pi `source: "interactive"`, including
steers into an open turn) the host sends a host-only `event` op *before* the text reaches
the model: `owner_input {id: A-<n>/g<gen>/i<seq>, turn, sha256, text (up to 16 KiB), at}`.
Rust appends it to the container. Corral wakes, inbox deliveries, PM outcome messages,
recovery notices, extension `sendUserMessage` and RPC prompts are never journaled. The
`event` op is not in the model's tool list.

**Operation.** `corral_owner_action` maps to the new `AgentOp::OwnerAction`, allowed for
the assistant role only. Payload: `{kind, target, args, expected_revision?, input, quote}`.
`kind` is a closed `OwnerActionKind`:

| Class | Kinds |
|---|---|
| Ordinary owner actions | approve (land), verdict, review-run, dispatch (incl. plan), queue/unqueue, move, plan accept/verdict/review, plan member-gate/add-member/move-member, config set (one key), park/unpark, cowork, finish |
| Destructive (always confirmed) | discard (drop) a task, revert, force or keep-branch overrides, automation grant changes, discard of a scratch with unlanded commits, any tag or release push |

Anything not in the enum is refused `unsupported_action` and the assistant routes it.

**Binding.** Rust accepts one of:

1. *Literal*: `input` names a journaled owner input of the **current turn** (opened since
   the model last went idle), `quote` is an exact substring of its text, and the quote
   contains the target ID; for `config set` it also contains the key and the value.
2. *Confirmed*: the host renders a confirmation for the exact action (command line and,
   for finish, diffstat and review choice) through Pi's UI. The owner's answer is itself
   journaled as an owner input of kind `confirmation` bound to the action digest.

Destructive kinds need (2) even when (1) holds. Failures: `not_owner_input`,
`turn_closed`, `instruction_unbound` (the host then offers a confirmation instead),
`declined`. One action per binding: the receipt key is `(input id, action digest)`, so a
replay returns the first receipt and a second target needs its own words or confirmation.

**Execution.** Rust executes through the same ops function as the CLI command, with a new
`OwnerAuthority::ViaAssistant(receipt)` argument accepted by the owner guards in place of
the shell check. Events are attributed actor `owner`, via `assistant:A-<n>`, and carry
`owner_authority {session, generation, input, sha256, quote, binding}`. Each action
re-reads current state under the same locks (task lock, `autopilot-<id>`, landing lock)
and refuses:

- `stale_revision` when `expected_revision` no longer matches;
- `conflict_in_flight` when autopilot, a PM action, a review or a landing holds the target,
  naming the holder; the assistant reports it and does not retry or override;
- every refusal the CLI path already has (stale gate, missing review, policy, size).

**Recovery.** `owner_action` is receipt-classified: a host crash after the receipt commits
but before the effect is reconciled from the receipt and the store (executed / not executed);
an uncertain outcome is reported, never re-executed by guess.

**Threat model (OD2).** The assistant runs as the owner's user with a shell. A model
determined to forge authority could unset variables, read the control credential, or use
herdr to type into its own pane. Today's guards share that limit, and T-460 deferred the
OS sandbox. This design therefore promises: an honest but fallible model cannot act as the
owner by misreading, inferring or widening a request; every owner action names the input
it executes; and the verification seams (journal, credential, herdr socket) are the ones a
future sandbox must hide to make it a hard boundary. The plan does not claim that boundary.

**Clarity rule in the role contract.** The packaged `corral-assistant` contract tells the
model: one action per explicit instruction; never infer an unrequested action; when the
target, values or scope are unstated, ask the owner or post a PM inbox item; after routing,
tell the owner the item ID and `corral pm inbox show <id>`.

### PM inbox

**Store.** `.corral/pm-inbox/PMI-<n>.md`, separate from the corral-to-agent agent inbox in
`.corral/inbox/`. Frontmatter: `id`, `revision`, `from` (`assistant:A-<n>` or `owner`),
`created`, `kind` (`action_request`, `question`, `handoff`, `routing`), `subject`, `refs`
(task IDs, artifact refs), optional `requested_action` (an `OwnerActionKind` payload), optional
`owner_words {input, sha256, quote}`, `state`, `outcome {by, at, text, refs}`. Body: the
assistant's reading and context.

**States.** `open -> claimed -> resolved(done | declined | escalated_to_owner)` and
`open|claimed -> withdrawn` (author or owner). Every transition takes
`--expected-revision`; stale writes refuse `stale_revision`. Events `pm_inbox_posted`,
`pm_inbox_claimed`, `pm_inbox_resolved`, `pm_inbox_withdrawn`.

**CLI** (under `corral pm`): `inbox post --kind --subject --body-file [--ref] [--key]`
(the key deduplicates retries), `inbox list [--state] [--all]`, `inbox show <id>`,
`inbox claim|resolve|withdraw <id> --expected-revision <r> ...`.

**PM consumption.** `pm next` gains an inbox observation: open items become cards below
authorized executable cards and pending questions, oldest first. Owner words in an item
are evidence of intent, not authority: the PM acts inside its own policy and escalates
owner-only requests through its existing owner cards. The Pi PM gets `corral_inbox_read`
and `corral_inbox_resolve` (claim and resolve) tools; the OmO PM uses the CLI. Nothing is
typed into the PM pane. OD4: a Pi-native PM also receives a native wake deduplicated as
`pmi:<id>:<revision>`, like `qwake`.

**Outcome to the assistant.** On resolve, a live author session gets one native agent-inbox
message `pmi:<id>:<revision>` with the outcome; an ended session keeps it in the item and
its session record. The board lists open items (assistant-board).

### Scratch worktrees

**Create.** `corral assistant scratch new --project <p> [--slug <s>] [--base <ref>]` from the
assistant's shell creates `<project repo>/.worktrees/A-<n>-<slug>` on branch
`corral/A-<n>/<slug>` (inside the project repo, per the workspace rule), records it in the
session (`path`, `branch`, `base`, `created`, `kind: spike|cowork`, `bytes`, `measured_at`)
and refuses `scratch_cap` above `assistant.scratch_max` (OD6, default 8 per workspace) or
`disk_low` from the resource probe. The host path guard allows edits only there.

**Kinds.** New scratch is a `spike`. The owner-bound `cowork` action (confirmation shows the
diffstat) marks it co-work; only co-work scratch can finish.

**Accounting.** `corral stats --storage` adds per-scratch bytes measured by a bounded walk
that skips the shared Cargo target and records `node_modules`-like trees separately; the
measurement runs in cleanup and stats, not in every sync.

**GC** (`corral assistant scratch gc --dry-run`, or `--yes` to act, mirroring `corral artifact gc`;
an owner command, refused from assistants). A scratch is eligible when
its session has ended and it has been idle for `assistant.scratch_idle_days` (OD6,
default 7), and it is not attached to a finished task. Before removal corral saves the
committed range and uncommitted changes as a `diff` artifact on the session, then removes
the worktree and branch. It skips any scratch with a live process cwd, pane or landing in
progress (the pidfd-checked worktree cleanup in `cleanup_worktree.rs`, limited to recorded
paths under `.worktrees/`) and reports why. `corral doctor` lists scratch directories whose
session record is missing, as it does for Done tasks' leftover worktrees.

**Promote.** `corral assistant scratch promote <slug> --title ... [-p]` files a Backlog
task through the normal `corral new` path with the spike diff attached; the scratch stays.

### Co-work finish and landing

1. **Finish** (`OwnerActionKind::Finish`, always confirmed: diffstat, title, project,
   kind, size and review choice, default `none`) on a co-work scratch with at least one
   commit. Spikes refuse `spike_not_cowork`.
2. **Materialize.** Corral creates task `T-<m>` (origin `assistant`, label `cowork`,
   `cowork {session, scratch, review}`) with body = session summary plus the owner's
   finish words, and run `R1` whose worktree and branch are the scratch, agent = the
   assistant's profile, no pane ownership. With `--into T-x` (owner-named) it adds a new
   run to an existing task instead, refusing `task_busy` when that task has a live run.
3. **Gate.** Finish runs `corral gate <id> --wait` on the scratch branch, rebased onto the
   project base first; failure leaves the task in Review with the failed gate, as for
   workers.
4. **Review choice.** `review: none` resolves the task's reviewer to none; this override is
   accepted only on cowork tasks, so `approve` (which already skips the review check when
   the reviewer is none) needs no special case. `review: required` dispatches the normal
   reviewer.
5. **Land** only on a separate owner-bound `approve` action; the normal `approve` path runs
   with landing lock, integration, size and freshness checks, and the full gate on landing
   where the project is tiered. The approved event records `owner via assistant:A-<n>`.
6. **Records.** The task is Done in history and stats like any task; its run carries the
   session's usage between scratch creation and finish; the session lists the landed task
   and commits; the scratch's lifecycle moves to the task run (removed by approve).

### Task focus and transcripts

`corral pi --task T-x` preloads a brief of at most 24 KiB: spec, latest plan summary, runs
with results and costs, latest review notes, open questions and artifact refs, with
truncation marked. Focus grants no authority over the task. A new read command
`corral run transcript <task> <run> [--review <n>] [--lines a-b]` pages transcripts for
OmO, Claude and Pi-native runs and review children using the transcript locators that sync
and usage already use, refusing `transcript_unavailable` with the searched location.

### Board integration

An assistants panel (live and recent sessions: focus, pane, cost, scratch count, open
inbox items); a card key that opens `corral pi --task <id>` in a new pane (owner action,
TUI attribution); a PM inbox list and badge; `asst` labels on cowork cards and on inbox
items. Kind `ui`.

### Members and waves

Waves follow the owner's phasing; dependency edges govern admission. Every member also
depends on **T-563 (capability-framework)**, which the PM adds at publication because a
plan scope cannot name an existing task (the same handoff pattern T-460 used for T-544).

| Wave | Member | Kind | Size | Depends on |
|---|---|---|---|---|
| 1 | assistant-sessions | backend | m | T-563 |
| 1 | assistant-launch | backend | l | assistant-sessions |
| 1 | assistant-entry-guard | backend | m | assistant-sessions |
| 1 | assistant-artifacts | backend | m | assistant-sessions |
| 2 | pm-inbox | backend | l | assistant-entry-guard, assistant-launch |
| 3 | owner-authority | backend | l | assistant-launch, assistant-entry-guard, pm-inbox |
| 4 | scratch-worktrees | backend | l | assistant-entry-guard, assistant-artifacts |
| 4 | cowork-landing | backend | l | scratch-worktrees, owner-authority |
| 5 | task-focus-transcripts | backend | m | assistant-launch |
| 6 | assistant-board | ui | m | pm-inbox, cowork-landing, task-focus-transcripts |

scratch-worktrees and task-focus-transcripts can start as soon as wave 1 lands. Each
member documents its own surface in `docs/guide/assistants.md` (new) or the guide it
changes; T-490's `corral pi` section changes in assistant-launch.

### Verification

Rust children: changed-file diagnostics, targeted `cargo test -p`, and the table-driven
`Cmd` classification test. Pi children: `scripts/pi-lane.sh`. Pane scenarios only in a
fresh plugin-free herdr server through `scripts/test-herdr.sh`. Crash fixtures (host
death between confirmation and execution, between receipt and effect) synchronize on
barriers and explicit clocks. No host load; load tests only through
`scripts/load-sandbox.sh`.

### Alternatives

- **Relax owner guards for agents in the owner's pane.** Rejected: cannot distinguish an
  instruction from an inference, and leaves no record of the words executed.
- **Confirmation dialog for every owner action.** Safer against misreading, more friction
  than the owner asked for; offered as OD3's alternative.
- **Typed tools for every read and filing command.** Duplicates the CLI and its revision
  checks; rejected per the owner's "existing audited CLI paths" constraint.
- **Reuse the agent inbox for PM requests.** The agent inbox is corral-to-agent delivery
  with transport receipts; PM requests need assistant-to-PM state with outcomes and
  revisioned claims. The agent inbox is reused only to notify a live assistant.
- **Session-only co-work records.** Every board, history, stats and search reader would
  need a second entity; offered as OD5's alternative.
- **Assistants as worker runs on a synthetic task.** Violates "no task first", takes a slot,
  and inherits a completion contract that does not fit conversation.

### Risks

- The `Cmd` classification can drift if someone adds a wildcard arm; the exhaustive test
  catches new variants, and review guards the arm.
- Co-work tasks with review none are a new acceptance path; it is limited to owner-bound
  finish and recorded in the event, so it is auditable and countable in stats.
- Scratch worktrees cost disk; the cap, accounting and GC bound it, and GC never discards
  commits without a diff artifact.
- Without the usage collector (T-570), assistant cost reads unknown; records still exist.
- Changing `corral pi`'s default can surprise the owner (OD1); the launch line names the
  role, and `--pm` is explicit.
