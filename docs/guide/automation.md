# Automation: review gate, reviewers and the autopilot

By default corral does nothing on your behalf after an agent finishes: the task lands in **Review** and you
decide. Automation is opt-in and layered, and each layer can be used on its own:

1. a **gate**: shell commands (tests, lint) run in the run's worktree;
2. a **reviewer**: a second agent that reads the diff and records a verdict;
3. the **autopilot**: advances the run through gate and review, sends failures back to the task agent, and
   either lands the change or hands it to you.

Check requirements live under `[review]`; permission to run them automatically
lives separately under workspace-only `[automation]` in `.corral/corral.toml`.
Every key is listed in [configuration](configuration.md#automation).

## Operating modes and owner confirmation

`assist` is the default. Sync still collects completed gate results, reconciles
already-running processes, and records submitted reviewer verdicts. New automatic
gates, independent reviews, repair requests, dispatches and landings require a
matching, finite owner-confirmed Autopilot grant. Explicit commands such as
`gate`, `review-run`, `verdict`, `changes`, and owner acceptance remain available.
Configuring `[review]` or enabling its legacy autopilot switch is not consent.

The owner previews a concrete selection and then confirms it from an interactive
terminal outside any agent run's worktree:

```bash
corral automation preview --task T-042 --dispatch
corral automation confirm --task T-042 --dispatch --digest <preview-id>
corral automation status
```

Confirmation displays the policy again and requires typing the full preview ID.
The JSON preview also shows `grant_digest`, the exact policy hash stored in the
approval receipt. Both hashes bind the proposed policy; they are not interchangeable.
If the selected task contracts, configuration or project identity change between
preview and confirmation, repeat the preview. PMs and agents cannot confirm a
grant or turn an attribution flag into owner authority.

`--task` and `--project` accept comma-separated values and may be repeated.
Project selection snapshots current eligible tasks, not tasks filed in the
future, unless you add `--include-new`. Then the grant covers every unfinished
task of those projects, including tasks filed later, and the whole grant ends
`--expires-after-days` days after confirmation (default 7, at most 365).
`--max-new-tasks N` caps how many tasks filed after confirmation it covers;
finished new tasks still count, so the cap is a budget of new work. A project
grant stops covering a project whose repository is moved or replaced. `--exclude` narrows the selection; `--plan-scope TASK:LABEL` selects exact
reviewed plan scopes. `--dispatch` delegates automatic launches, including gates,
reviews and bounded repair. Add `--accept` to delegate landing and
`--plan-accept` for plan acceptance. Delegation never overrides owner-only
exceptions, holds, unresolved decisions, independent review, candidate freshness,
capacity limits or gate requirements.

A landing-only grant (`--accept` without `--dispatch`) can land an already reviewed
candidate whose gate and review receipts are still fresh. It cannot refresh stale
verification: the task and status surfaces instead explain the missing Gate
authority until the owner delegates dispatch or explicitly reruns verification.

The board's `O` editor provides the same selection with a picker, a summary
preview and a one-key `y` confirmation; the owner and agent-worktree checks above
still apply, and a preview that changed since it was shown is rejected. Its
ordinary save action cannot grant authority. Successful confirmation retains the owner identity, timestamp,
revision and exact digest and emits an `operating_policy_confirmed` audit event.

Without authority, the board header and affected Review cards, `autopilot status`,
`show` (including `automation_blocker` in JSON), and `pm brief` / `pm next` explain
the actual refusal. Sync records `automation_blocked` once per changed reason and
`automation_resumed` when the task can advance again; it preserves the existing
gate/review stage and receipts while blocked. A switch being enabled must not be
read as effective Autopilot authority.

## Review policy

The effective policy for a task is built from four layers, later ones winning:

1. built-in defaults: no gate, no reviewer, `accept = "owner"`, `max_rounds = 2`, `merge = "rebase"`,
   `remove_worktree = true`;
2. `[review]` for the whole workspace;
3. `[projects.<key>.review]` for one project;
4. the task's own `review: owner` or `review: auto` frontmatter line (sets `accept` only).

With the defaults, or with no gate, no reviewer and `accept = "owner"`, the autopilot has nothing to do and the
flow stays manual.

```toml
[review]
gate = ["cargo fmt --all -- --check", "cargo clippy --all-targets -- -D warnings", "cargo test"]
reviewer = "deep"
accept = "auto"
max_rounds = 2
owner_paths = ["migrations/**", ".github/**"]

[profiles.deep]
harness = "claude"
model = "claude-opus-4-5"
effort = "high"
```

Here a finished run must pass the gate, get approved by an independent `deep` reviewer, stay under 800 changed
lines and avoid `migrations/` and `.github/`. Anything else comes back to you.

## The gate

`gate` is a list of shell commands run in order in the run's worktree, in a herdr pane, so you can watch it.
The result is recorded on the run and attached as a log artifact titled `Gate: pass` or `Gate: fail`.

```bash
corral gate T-042 --wait    # run it now and print the result
```

The autopilot runs the gate for you; the manual command is for trying a gate or re-running it by hand.

Every attempt has separate logs and command receipts plus a random ownership token in a `0600` file.
The gate shell keeps the token out of child command environments. An inherited
`CORRAL_GATE_ATTEMPT` alone cannot finish an attempt or start another gate in its owning store.
Legacy `<task>-<run>.json` results cannot complete a new attempt.
During tests or CI, workspace discovery stops at `.worktrees/<id>` when
`CORRAL_GATE_ATTEMPT`, `CORRAL_TEST`, `CI`, or `RUST_TEST_THREADS` is set.
Use a temporary fixture store or explicitly select the intended root.

## Reviewer runs and verdicts

`reviewer` is one of:

- `"none"`: no independent review;
- `"pm"`: the workspace PM agent reviews;
- a profile name: a fresh agent with that profile reviews in its own pane. Pick a different (often stronger)
  model than the one that wrote the code.

`require_cross_family` defaults to `false`: configured same-family reviewers, including
the same profile/model in a fresh review session, are allowed. Opt in with
`require_cross_family = true` globally or per project to reject same-family and unknown-vendor
reviewers in both plan and implementation review, including `--reviewer` overrides.
This changes only reviewer diversity; configured reviews, gates and owner acceptance still apply.
`corral route` reports the effective requirement and source. A rejected launch explains the
required policy and vendor mismatch.

With `"cross-vendor"`, historical runs resolve the implementer's vendor from their saved vendor, then
their model, then their current named profile (`profile`, or the legacy `agent` key). The profile's
explicit vendor wins over inference from its model. Known bare `claude-*`, `gpt-*` and `o`-series model
IDs work as well as provider-qualified IDs. The first resolved vendor is saved on the run so later
profile edits cannot rewrite it. If no vendor can be resolved, autopilot hands the task to you once,
in Needs me with the reason, instead of retrying every sync.

```bash
corral review-run T-042                    # use the configured reviewer
corral review-run T-042 --reviewer deep    # or name one
```

The reviewer examines the run and records a verdict on it:

```bash
corral verdict T-042 approve  --notes "Matches the spec; tests cover the overflow."
corral verdict T-042 changes  --notes-file review.md
corral verdict T-042 escalate --notes "Touches billing; needs the owner."
```

| Verdict | What the autopilot does |
|---|---|
| `approve` | continues to acceptance |
| `changes` | sends the notes back to the task agent as a new round |
| `escalate` | hands the task to you (Needs me) with the notes as the reason |

A verdict belongs to one commit. If the worktree moved on after the review started, or the reviewer closed
without recording a verdict, the task goes to you instead of being trusted.

An existing approve verdict can carry across corral's clean rebase when the patch is unchanged,
ignoring blob IDs and hunk headers but retaining whitespace, paths, modes and binary payloads.
The integrated gate must pass first, and the review's other inputs and completion evidence must
still match. A `verdict_carried` event records both SHAs and the patch comparison evidence.
Conflicts and changed patches need a new review. With `accept = "auto"` this lets consecutive
approved tasks land without another PM action. Manual `corral approve T-042` performs the same
rebase, gate, wait, carry and land sequence, or reports the next command.

A pending reviewer uses `[sync] review_idle_after_secs` (default 1800, or 30 minutes),
not the worker's 120-second idle warning. This longer bound accommodates quiet monitor
and background-check waits without closing the review pane. Its first idle observation
is persisted; repeated syncs and restarts do not extend the bound. Genuine activity
resets it, and a reviewer still idle at the bound goes to Needs me with its saved notes
path. A completed verdict stops the watchdog. A failed verdict submission escalates on the next idle sample;
an idle legacy review missing its candidate identity escalates immediately. This liveness check
continues with autopilot off, and activity resets the persisted idle deadline.

## Daily digests

`corral digest` collects the last day of recorded events and usage, snapshots a Report
artifact, and pins it. No model, transcript refresh or agent is involved.

```bash
corral digest
corral digest --since 2d --json
```

The report includes landed task IDs, commit SHAs and reviewer verdicts, escalated /
needs-owner events, delivered change requests, dropped tasks, and cost/time totals.
The window is `(since, snapshot_at]`; `--since` accepts positive whole `h`, `d`, or `w`
durations. Reported USD and API-equivalent estimates remain separate, and missing
usage is labelled unknown. Sent-back entries cover manual delivered requests and
autopilot change rounds, including queued retries. Accounting uses recorded cumulative usage of runs ending
in the window (or starting there when unfinished), including reviewer child runs
once, rather than inventing prorated daily costs.

Optional scheduling runs during sync:

```toml
[digest]
every = "1d"
```

The first enabled sync reports the preceding interval. Later scheduled reports start
at the previous scheduled snapshot, including any downtime, and manual reports do
not advance that cursor. There is no independent timer: run the board, plugin hooks,
or `corral sync --watch` for regular syncs. Each report has its own pin and protected
Report artifact. Like sprint reports, artifacts attach to the first existing task;
an empty workspace gets one Done report-anchor task, reused by later digests.

## Reverting a landing

```bash
corral revert T-042
```

This appends a revert commit on the task's base branch for its exact recorded landing.
It never rewrites history or pushes. Later unrelated commits are preserved. The
operation first tries the inverse in a private detached worktree; a conflict refuses
the operation without changing the base or task and reports conflicting paths plus
later commit SHAs touching those paths.

After success the task returns to Backlog with a dispatch hold, and a `reverted`
event links the original landed commits and new revert SHA. Use the normal explicit
Ready/queue action when it should run again. Existing run evidence remains intact.
The main checkout must be clean and on the recorded base; pending post-land
operations and active peer reviews must settle first. Reverts participate in the
same configured post-land operations as acceptance.

A task must be Done with a durable merged approval recording both the pre-landing
base and landed head. Kept branches, already reverted tasks, and legacy approvals
without that exact range are refused rather than guessing from the dispatch base.

## The autopilot

When `corral done` moves a run to Review, the review policy has work to do, and a
confirmed operating grant authorizes the next action, every sync advances that run
with no action from you. Syncs happen in the board, in the herdr plugin's event hook, and
headlessly with `corral sync --watch`.

Each run carries an autopilot state, shown on its card and review screen, and every change of state is logged to
`.corral/events.jsonl`:

| State | Meaning |
|---|---|
| `gating` | the gate is running on the run's latest commit |
| `reviewing` | the reviewer is judging the run |
| `landing` | rebasing onto the base, re-running the gate and fast-forwarding the base |
| `done` | landed; the task is Done |
| `reviewed` | gate and review passed and the run waits in Review for you (`accept = "owner"`); the card says `✓ reviewed` |
| `changes` | sent back to the task agent with the reason |
| `blocked` | landing waits for a precondition (the main checkout is dirty or not on the base branch); retried on later syncs and notified once |
| `escalated` | handed to you (Needs me) with the reason |

The path is `gating`, then `reviewing`, then either `reviewed` (you accept) or `landing` and `done`
(`accept = "auto"`).

### Failures loop back

Before starting a gate or reviewer, the autopilot simulates integration with `git merge-tree` on
the exact candidate and current base SHAs. It distinguishes a clean merge, a conflict, and a Git
infrastructure error by exit status and structured output. The simulation leaves branches, the index
and the worktree unchanged. Conflicts use the existing change-request rounds; infrastructure errors
escalate to Needs me instead of asking the worker to rebase.

An **Integration repair packet** JSON artifact records the dispatch base, merge base, candidate and
current base SHAs; every base commit since dispatch; commits still missing from the candidate;
recorded task IDs and titles joined by exact approved SHA; conflict paths; changed-path overlaps;
file SLOC counts; and effective gate commands. Commits without matching approval records stay
unattributed, even if their subjects mention task IDs. Deleted task branches are not needed.
SLOC counts use committed blobs, omit blank and whole-line `//` comments, and show the 800-line Rust
ceiling. These are factual size hints, not a replacement for the configured gate.

Messages contain counts and the full packet path, without truncating the artifact's landed-task
list. Integration requests are keyed by task, run, candidate SHA, base SHA and reason: repeated
syncs do not send the same request again, and base movement produces fresh context. Retry prompts
carry the packet path. Writes reject a superseded run. Semantic advice remains a separately
attributed reviewer note; overlaps and landed-task lists do not establish semantic compatibility.
A clean merge simulation does not prove a clean rebase: the later actual replay remains authoritative,
and a replay conflict is aborted and returned with its own conflict paths.

A failing gate sends the failing command and the log tail back to the task agent. A `changes` verdict sends
the review notes. If the agent's pane is gone, corral starts a retry run with the same profile and the request
in its prompt. Each send-back is a round; after `max_rounds` of them the task is escalated instead. Your own
request-changes or retry resets the count.

### Early main-movement notices

Sync also watches recorded landings for active Running/Needs me runs on the same project and base.
After two seconds without another observed landing, it compares the batch's changed paths with the
worker's committed changes, staged/unstaged/untracked paths, and optional `write_scope` globs.
Both rename endpoints count. Git status supplies uncommitted path names; corral does not read their
contents or follow their symlink targets. Disjoint paths send no message, but do not prove semantic
independence.

Declare intended writes in the task's YAML frontmatter through `corral edit`, for example:

```yaml
write_scope:
  - crates/corral-ops/src/**
  - docs/guide/automation.md
```

Globs use `*` within a path segment and `**` across directories, like artifact globs. An omitted scope
uses observed changes only. Declarations are advisory, not write restrictions or dispatch reservations.

One informational notice covers the batch and carries landed task/commit IDs, overlapping paths,
800-SLOC Rust headroom hints and the full integration context packet as a **Main movement context**
artifact. Counts use the packet's documented committed-blob method; uncommitted contents are not
counted, and these hints do not replace the candidate guard's lexical SLOC checks.

Delivery shares replies' `agent_prompt` transport, bounded terminal fallback and per-task delivery
lock, but uses a separate queue in `.corral/state/main-movement-<task>.json`. Missing panes, stuck
workers, pending replies and owner waits leave a visible deferred receipt and diagnostic; the queue
retries after they clear. The current task/run, recorded pane, workspace, worktree, branch and observed
shell are checked before submission. Finished or replaced runs cannot inherit an old notice.

`main_movement_notice` events record pending delivery outcomes, context/receipt paths and transport
evidence. A send interrupted before its receipt is saved becomes **uncertain**, not an automatic
resend: inspect the pane and receipt before manually forwarding its context. This avoids duplicating
a notice that might already have reached the worker. Ordinary failed submissions remain retryable.

Notices never rebase, accept, clear a pending answer, change task status, or consume a change round.
The worker chooses when to integrate; the reviewer checks semantic composition; the owner decides
conflicting requirements. Keep a normal sync driver running to construct and deliver notices.

### What always needs you

With `accept = "auto"`, these guards stop the autopilot and put the task in Needs me with the reason spelled
out and a notification:

- the change touches a path matching `owner_paths`;
- the task says `review: owner`;
- the worktree is missing or has uncommitted changes after `corral done`, or changed during review;
- the reviewer closed without a verdict, or its verdict was `escalate`;
- the gate pane closed before the gate finished;
- all `max_rounds` change rounds are used.

Diff size is not a guard: a large diff goes through the same gate, independent review and accept policy as a
small one. The former `max_diff_lines` key is deprecated; existing configs that still set it load, the value
is ignored, and corral warns once.

From Needs me you can approve, request changes, retry or discard like any other Review task.

### Landing

Landing needs the project's main checkout to be on the base branch and clean; otherwise the run is `blocked`
until that is true. A base that moved is rebased onto; a conflict is sent back to the agent. With
`merge = "ff"` the agent is asked to rebase itself. The gate re-runs on the rebased commit, then the base is
fast-forwarded and, with `remove_worktree = true`, the worktree is removed. Nothing is pushed to any remote.

Manual approval follows the same candidate boundary: a rebase that changes the candidate must get
fresh configured gate and semantic review receipts before it can land. Retrying approval, including
fast-forward, cannot reuse the old receipts.

### Updates and state compatibility

The board, watch loop and autopilot check the running executable on every sync before side effects.
Replacing or deleting that executable stops new gates, reviews and dispatches, records `version_skew`,
and reports "corral was updated; restart the board (q, then corral up)". Restart explicitly; automatic
re-execution is not enabled.

Saved task frontmatter uses a `schema_version: 2` envelope with the record under `task`.
Legacy flat records remain readable for recovery, but legacy receipts without candidate identity
are never trusted. Older flat readers cannot decode a newly saved task, so they cannot collect its
gate result. The state schema stamp is `.corral/state/schema-version`; unknown versions are refused
even by an already-open store.

### Acceptance record

Opt-in [post-land operations](post-land.md) run after base advancement under
the project landing lock. Their receipts distinguish landed commits from
operational readiness; a failed hook blocks dependent dispatch, not the
already-landed commit. See [cleanup](cleanup.md) for pane/process shutdown,
worktree removal, discard behavior and doctor warnings.

Every task the autopilot lands gets an **Acceptance record** artifact: the gate log, the verdict and notes, the
number of rounds, the landed commit and the policy that was in force. Find it in the artifact finder or with
`corral artifact list --task T-042`.

### Kill switch

```bash
corral autopilot status    # what it is doing and which tasks it is driving
corral autopilot off       # stop all automatic advancing
corral autopilot on        # enable the switch; does not create a grant
```

`corral autopilot off` (equivalently `autopilot = false` under `[review]`) is re-read on every sync, including
by a long-running board or `sync --watch`. With it off review does not advance automatically; all manual commands
(`corral gate`, `review-run`, `verdict`, `approve`, `changes`) keep working. The setting is workspace-wide and
is rejected under `[projects.<key>.review]`. In the board, `O` or a click on the header's autopilot
status flips the switch; the displayed status also reports effective authority.
Receipt collection and recording existing reviewer verdicts continue with the
switch off. Use `corral config set automation.paused true` to pause the operating
grant, or set `automation.mode` to `"assist"` to withdraw automatic authority.
Resuming a paused policy still requires its matching receipt; changing the mode
back to Autopilot by hand does not restore consent.

Sync releases its sampled shared session-ownership inventory before automatic
launches acquire an exclusive ownership claim. Another process retaining a real
inventory lease can still defer a launch; that contention must not be bypassed
or mistaken for a task's consent.

## Dispatch queue

Ready is the dispatch queue: Backlog is filed work; Ready is authorized work waiting for
capacity. Enqueue individual tasks without starting them, or restrict automatic admission
to named projects:

```bash
corral queue add T-042 --profile deep --model opus --effort high --base main
corral queue admit app
corral queue list --json
corral queue pause
corral queue resume
corral queue remove T-042
corral queue admit app --off
```

`queue add` and `dispatch --queue` both move the task to Ready and are enqueue-only, even
with free capacity. Move/drag to Ready has the same authorization meaning. All configured
projects are admitted by default. A nonempty `dispatch.ready_projects` list restricts
automatic admission, including already enqueued tasks. Empty or omitted means unrestricted;
`queue pause` stops all projects. `queue admit app` selects a restriction, while `--off`
removes that project; disabling the last allowed project requires `queue pause`.

Pausing and resuming dispatch preserves the original one-task owner consent,
including work enqueued while paused and a coding or planning launch already
prepared before the pause. Submission rechecks the live pause, so no prepared
callback runs while paused; resume reuses its original run and pane. This does
not renew consent after contract, review, authority or other dispatch-policy
changes. To replace obsolete queued intent, explicitly remove and enqueue the
task again.

`queue remove` and moving Ready work to Backlog return the task to Backlog, clear its pinned
queue intent, and prevent `auto_backlog` from immediately readmitting it. Move to Ready or
explicit enqueue clears that hold. Promoting an Idea to Backlog does not set a withdrawal
hold: it remains eligible for configured `auto_backlog` admission. Queue removal rejects
Running, Needs me, Review, Done, and Dropped tasks; it does not stop agents or finishing work.
Ready cards show the current dependency, capacity/headroom, machine-busy, paused, or
admission-restriction reason.

Migration: earlier versions used a hidden `queued` flag on Backlog cards. The next owning sync
moves those records to Ready, even while herdr is unavailable, without changing their pinned
route or launch journal. Each migrated task logs one `queue_migrated` event. Existing nonempty
`ready_projects` lists remain restrictions; an empty list now admits all configured projects
instead of disabling admission. Set `dispatch.paused = true` to retain a fully paused queue.

Sync uses the existing queue drain, ordered by P0, P1, P2, P3 (unknown priorities last), then creation
timestamp, then task ID. Dependencies must be **Done**: Review is not approval. Missing dependencies
and cycles are reported as errors with task IDs, not indefinite waiting. `queue list` explains pause,
authorization, dependency, capacity, model, launch-failure and finishing-work waits.

Routing resolves when explicitly enqueued (or at first admission after move/drag), using
explicit override -> task profile -> first matching owner rule -> current Auto receipt ->
project/workspace default precedence. Unreserved queued work reevaluates that precedence
before admission, including receipts saved after enqueue. Spec or catalog edits invalidate
Auto receipts and require reclassification. Explicit owner profile/model/effort overrides
remain fixed; other execution settings resolve the stable profile's current approved binding.
The queue also retains mode and explicit/project base. An unspecified base uses the
repository's current branch at claim. Model availability is checked again at claim; a
missing pinned model fails visibly, without substitution.

Reservation captures the run's exact execution snapshot: profile binding, model, effort,
harness command, arguments, instructions and implementation prompt template. Recovery,
active runs and historical runs retain that identity despite later configuration changes.
Legacy runs without a snapshot retain their original current-configuration behavior.

Each new run is recorded with a reservation **before** worktree/pane creation. All task/config writes
and both coding/review claims serialize on one workspace board lock; one kernel-owned per-task launch
lock excludes duplicate manual/sync launches. A durable phase journal and unique workspace label allow
sync to reuse the same run, branch, worktree and pane after an interrupted preparation. Failed preparation
releases capacity while retaining its intent and error for recovery. Claim/release/dispatch events carry
the run identity.

Shell submission has an unavoidable ambiguous boundary: the IPC may succeed immediately before the
launcher dies. Corral never sends that command a second time. It retains the one attached run/pane in
Needs me, releases the temporary reservation into that occupied task slot, and asks the owner to inspect
the pane before retrying. This is at-most-once submission, not a claim of exactly-once arbitrary shell
execution. An ordinary task retry or explicit reply is the owner's recovery action.

### Finishing work and capacity

Running/Needs me tasks, running gates and live reviewer records each use one global and project slot by
default. A gate started while its coding task is still Running therefore needs another slot.
`limits.review` is an additional gate/reviewer cap; `limits.count_reviews` and `limits.count_gates`
can explicitly exclude those process types from the shared pool (both default on).

The autopilot leaves a capacity-blocked task in Review with a **waiting for a slot** reason. Sync advances
review before starting new coding work, and atomic coding claims yield to eligible waiting finishing
work even across concurrent syncs. A saturated project's wait does not stall unrelated projects.
The board header and `corral autopilot status` display the component counts. Completed gates, verdicts,
and vanished reviewer panes release review capacity; an interrupted review launch is reconciled before
its reservation is reused.

`queue pause` controls coding admission, while `autopilot off` controls review advancement. Neither kills
running work. Keep a board, hook or `corral sync --watch` driver alive to execute either queue.

### Automatic Backlog dispatch and Idea promotion

`[dispatch] auto_backlog = true` is the default. After reconciling gates, reviews, landings,
escalations, resource holds, and the explicit queue, each sync fills automatic coding capacity
from dependency-ready Backlog tasks, moving each through Ready before Running. Routing chooses
the profile. P0 through P3 priority comes
first, then oldest creation timestamp, then task ID. Only Done satisfies a dependency.
Dequeue holds, project limits, plan approval, operational blockers, and the machine resource
guard still apply. A malformed dependency graph is reported without blocking unrelated work.

Automatic claims leave `limits.review` headroom in the shared running pool. Live counted gates,
reviewers, and their reservations use that headroom instead of reserving it twice. Consequently,
`limits.running <= limits.review` leaves no automatic coding capacity. Direct manual dispatch
retains its existing capacity rules; all sync-driven Ready admission reserves headroom. Claims recheck headroom under
the board lock, including recovery of an interrupted automatic launch.

`promote_ideas` defaults to `"suggest"`:

- `"never"` neither suggests nor promotes Ideas.
- `"suggest"` makes `corral pm next` present a deterministic ranked `promote` card with
  eligible IDs, reasons, and predicted conflicts. The owner answers one question; readback
  does not change a task.
- `"auto"` moves eligible Ideas to Backlog and dispatches them without asking, even when
  `auto_backlog` is false. Existing eligible Backlog work is considered first.

An eligible Idea has a nonempty Acceptance or Acceptance criteria section, size at most M
(or unset), no effective plan requirement, no `owner-only` label, and completed dependencies.
Set labels at creation with `corral new "Owner scope" --idea --label owner-only`.
Declared write scopes and T-064's historical likely-file inference predict overlap with active
tasks in the same project. Conflicting Ideas are excluded. When paths cannot be inferred,
the shortlist explicitly says the overlap check was skipped. Ranking uses priority, age,
low conflict risk, and task ID for a stable tie break; eligibility is recalculated after each
new run.

`max_auto_promotions_per_day` defaults to 3. Promotion events consume a durable workspace-wide
budget on the UTC date of the policy pass; a project's effective limit is compared against
that shared count. Dispatching existing Backlog work does not consume this budget.
`[projects.<name>.dispatch]` may override each of the three selection settings independently;
unspecified fields inherit the workspace values.

Every automatic promotion and dispatch writes a `dispatch_policy` event containing the reason,
rank, eligibility inputs, free slots, and selected route where applicable. `corral autopilot status`
shows the effective settings, daily count, and recent decision inputs even with review autopilot
off. The board, plugin hooks, and `corral sync --watch` provide periodic evaluation; no PM model
is needed to keep the policy running. `queue pause` stops automatic admission too.

T-099 owns this dispatch/promotion policy. T-019's remaining scope is daily digests and reversible
revert commits; it must not implement a second scheduler.

## The watchdog: stuck commands

An agent that waits forever on a foreground command (a hung test run, a pager) looks idle to herdr but is not
finished. Independently of the idle badge, every sync tracks how long herdr has reported the pane idle. After
`[sync] stuck_after_secs` (default 300), corral inspects the pane's foreground process; a foreign process older
than that threshold marks the run **stuck**: the card and the detail screen show `⚠ stuck: <command>`, and one
notification is sent. The card does not change column. The agent's own harness, its known helpers and the pane
shell are never treated as stuck, and the badge clears when the pane becomes active again or the process exits.

```bash
corral unstick T-042    # confirm and terminate the stuck command, then retry the pending reply
```

`corral unstick` (or `u` on the detail screen, which asks first) shows the process, re-checks that it is still
the same one, sends SIGTERM and, if it has not exited after three seconds, SIGKILL. It then resubmits the last
reply you sent, if one was pending. While a run is flagged, replies queue instead of being typed into the stuck
terminal. Recovery relies on Linux process identity.

## Explicit waits and owner decisions

Workers and adapters can declare why a run is unattended through the same CLI:

```bash
corral run wait T-042 --file wait.json
corral run decision T-042 --file decision.json
corral run resume T-042 --wait ci --file receipt.json
corral run resume T-042 --decision migration --file answer.json
corral show T-042 --json
```

These are version 1 records stored on the current run, not a second scheduler. Every registration
names the exact current run ID (`R1`, `R2`, ...); old retry-run receipts are rejected as
`mismatched_run`. Registration time is assigned by corral. Repeating identical registrations or
receipts is a no-op; conflicting stable IDs fail without replacing data.

A channel wait declaration looks like this:

```json
{
  "version": 1,
  "id": "ci",
  "run": "R1",
  "condition": "CI result is available",
  "owner": "fixture-worker",
  "source": "worker",
  "deadline": "2026-10-16T12:00:00Z",
  "handle": {
    "kind": "channel",
    "channel_id": "worker:ci-42",
    "capability": "wait_v1"
  }
}
```

`source` is `corral`, `adapter` or `worker`. A channel ID is a declared handle, not a command.
`wait_v1` declares that the producer understands this receipt protocol; it does not prove the
channel is still alive or guarantee that a harness wakes. Channel waits need a nonempty ID and
a bounded deadline to be pending. Missing/unknown capability is `unsupported`; a missing ID or
deadline is `unverifiable`. Producers report lost handles using a `lost` receipt. Sync also
classifies expired deadlines, missing gate panes, changed gate invocations and unreadable results.

For an actual corral gate, use `"handle": {"kind": "gate", "sha": "<gate SHA>",
"pane": "<gate pane>"}` from the run's gate record. Corral derives the result-file path itself
and only its existing gate collector can record success or failure; a worker cannot assert a gate
PASS through resume. Finished gates produce one receipt per wait, even across sync restarts.
Their delivery is `unknown`: collecting a result does not prove the harness received a wake.

A decision uses `version`, `id`, `run`, `owner`, `source`, optional `deadline`, `question` and
`requested_action`. It moves the task to Needs me with that exact question and action, even if
herdr says idle or working. A valid external wait stays Running, distinct from an unanswered
owner question. Cards show the condition and diagnostic state; the detail Runs tab includes the
request, handle, observation and receipt. An expired decision still requires an owner answer.

A resume receipt supplies `version`, a stable receipt `id`, `run`, `outcome`
(`ready`, `failed`, `cancelled`, `lost`, or `answered` for decisions), `delivery`
(`acknowledged` or `unknown`), `observed_at`, optional `answer`, and optional JSON `data`.
The observation must be between registration and receipt submission. Decisions require a
nonempty answer; answers remain in the run for future resume. A successfully delivered normal
`corral reply` also answers the first pending decision. Failed replies retain the question and
the existing pending reply, so the normal resend path remains available.

Sync reconciles these records under the task lock and notifies once per recorded state
transition. Receipt delivery uncertainty survives terminal activity and restart; corral never
resends a command or launches another worker because a wait completed. Receipts are immutable.
Unknown delivery requires inspection and, if necessary, an explicit owner reply rather than an
automatic resend. All declarations leave the T-033 watchdog and its confirmed, identity-checked
recovery path intact: a wait is never permission to suppress a stuck diagnostic or kill a process.

## Manual review

Without the autopilot, or after it hands a task to you:

```bash
corral review T-042 --diff               # summary, files, gate and artifacts
corral approve T-042 --merge rebase      # land it (ff | rebase | keep)
corral changes T-042 "Please add a test for the overflow case"
corral retry T-042 --profile deep        # a fresh run, optionally with another profile
corral discard T-042                     # drop the run and the task
```

`corral approve` removes the worktree and then deletes the merged task branch with a safe `git branch -d`.
`--keep-worktree`, `remove_worktree = false` and `--keep-branch` retain them. A refused branch deletion is a
warning, not a failed approval. `--merge keep` and `discard` leave unmerged branches alone.

Approval (including auto-land) and discard close corral's agent, gate and non-PM reviewer panes
across all runs of the task, even when the worktree is kept. Task workspaces created by corral
close once empty; user tabs and the PM workspace are preserved. Requesting changes or escalating
to the owner closes that round's gate/reviewer panes but retains the agent and workspace.
`corral sync` also reaps leftover panes and empty owned workspaces of Done/Dropped tasks.
Cleanup failures are recorded as `cleanup_failed` events and retried by later syncs.

## PM board awareness

The workspace PM can subscribe to actionable board events with `corral wait`, or opt in to live-PM push
under `[pm]`. This wakes the coordinator, not dispatched coding agents, and does not authorize dispatch,
approval or process recovery. See [board events](events.md) for filters, cursor re-arming, timeout behavior
and the OmO/Claude Code monitor versus omp background-command workflow.
