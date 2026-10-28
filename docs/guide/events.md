# Board events and PM awareness

Adding a [task comment](comments.md) appends a `commented` event with the structured
comment (`author`, `at`, `text`) and whether delivery was requested (`send`). Comments
are durable owner context, not a task-spec revision or an automatic dispatch instruction.

`corral wait` waits for one task or configuration event, then exits. It starts at the current end of
`.corral/events.jsonl`, so old events do not wake a new subscription.

```bash
corral wait --for done,needs_me,stuck,blocked,escalated --timeout 30m
corral wait --task T-041 --json --print-cursor
corral wait --project corral --since 2026-10-15T00:00:00Z --json --print-cursor
corral wait --after-offset 12345 --json --print-cursor
corral wait --for config_changed,config_invalid --json --print-cursor
```

`--for` accepts, and defaults to, every PM-actionable kind (`corral_core::PM_ACTIONABLE_KINDS`,
the same list `pm.push_kinds` accepts) plus `config_invalid`: `done`, `needs_me`, `stuck`,
`blocked`, `escalated`, `config_changed`, `lessons_due`, `run_resumed`, `wait_transition`,
`pm_decision_resolved`, `delivery_uncertain`, `delivery_orphaned`, `message_unobserved`,
`question_opened`, `question_escalated`, `question_unattended`, `turn_ended_without_report`,
`attention_due` and `slot_reacquired`.
Other question events such as `question_answered` never wake a wait. `--task` and `--project`
narrow the match. Configuration events
are workspace-wide and do not match task/project filters. Unlike PM push, an explicit
config subscription includes UI-only changes as well as policy changes.
`--since` includes events at or after an RFC3339 timestamp and scans retained history.
`--after-offset` resumes in the current log at an event's end byte offset rather than starting
at EOF; when combined, `--since` still filters timestamps. Without `--timeout`, the wait is
unlimited. Durations need a suffix: `ms`, `s`, `m`, `h`, or `d`. A match exits 0; timeout exits 124.

JSON output is the original event (`ts`, `kind`, `task`, `detail`), not a normalized replacement.
`--print-cursor` adds `after_offset` and `cursor` (an object with `offset`, optional `identity`, and
optional byte-array `anchor`). Human output appends `cursor=<JSON>`. Re-arm after every wake
with `--after-offset <after_offset>` so events between waits are considered.
Timeout prints nothing: retain the previous successful `after_offset` (equivalently `cursor.offset`).
Before the first success, retain an original `--since` timestamp for catch-up after timeout;
starting a fresh default wait at EOF could skip events between subscriptions.

The reader follows replacement/truncation from byte zero, retains incomplete new lines until
their newline arrives, and skips malformed or oversized lines with bounded memory. It does not
drain an old rotated file. A bare offset belongs to the current file, not a permanent event ID:
rotation between separate waits can invalidate it. Use `--since` to rescan retained history when
needed; history removed by rotation cannot be recovered by `wait`.

Workspace PMs on OmO or Claude Code should subscribe through their harness's `monitor`, not
foreground polling. A raw stream alternative is
`tail -n0 -F .corral/events.jsonl | grep --line-buffered <pattern>` through `monitor`.
On omp or other harnesses, run `wait --json --print-cursor` in the background and use the
harness's completion notification, then re-arm with the returned offset. A dispatched coding
agent still follows its Run contract and finishes with `corral done`.

### Optional push to the PM

```toml
[pm]
agent = "omo"
push_events = false
push_kinds = ["done", "needs_me", "stuck", "blocked", "escalated", "config_changed",
              "lessons_due", "run_resumed", "wait_transition", "pm_decision_resolved", "attention_due"]
```

Push is off by default, including for existing configs. Enable it in `.corral/corral.toml` or
Settings > General > PM event push. PM push kinds is a comma-separated list of PM-actionable
names: the eleven above by default, plus the opt-in `delivery_uncertain`, `delivery_orphaned`,
`message_unobserved`, `question_opened`, `question_escalated`, `question_unattended`,
`turn_ended_without_report` and `slot_reacquired`. Clearing it selects none. Invalid names block saving. `config_changed`
wakes the PM only when the event's `pm_relevant` flag is true. `config_invalid` is available
to `wait` and config history but is not a PM push kind.

Sync sends selected events as `[corral]` messages only to the remembered, live PM pane, using
the same `agent_prompt` and terminal fallback as replies. It does not start a PM or send to
coding-agent panes. Position persists in `.corral/state/pm-events.json`. Disabled push,
unavailable herdr, and an absent PM checkpoint the log end during sync: skipped events are
not replayed later. A first-ever enabled sync with a live PM and no saved cursor considers the
retained log from offset zero. To skip old history, run one sync with push disabled first.
Event delivery is awareness, not authorization to dispatch, approve, or recover a process.

### Attention lifecycle and routing

`attention_opened`, `attention_overdue` (re-wake/owner levels),
`attention_handled`, and `attention_resolved` carry deterministic `fact_id`
values. Interrupted publication may repeat a fact with the same ID; consumers
deduplicate by ID. Resolution facts contain the kind, responsibility, since/source,
opened time, PM liveness at opening, first handling, fired levels and resolution
time, so later record pruning does not lose these values.

`attention_due` carries `project` (nullable), complete `tasks`, complete `count`,
at most 16 display `entries`, and nullable `pm_live`. It is emitted at most once
per project/pass with announcements or re-wakes. `event.task` is set for a
single-task digest; `wait --task` also matches membership in `detail.tasks`.
`wait --project` retains its existing detail/project fallback rule.

`notification_claimed`, `owner_notified`, `owner_notify_failed` and
`notification_not_invoked` preserve `batch_id`, `claim_ids`, source, producer and
the complete coverage/exemption relation. One physical batch can cover several
conditions. A missing outcome is unknown, not proof that transport was skipped.

### Operating policy events

The workspace [operating mode](automation.md#operating-modes-and-owner-confirmation) records
its own audit and accounting events. None of them grants authority; executors recheck the
current effective policy at every side effect.

| Kind | Detail | Wakes `wait`/push |
|---|---|---|
| `operating_policy_confirmed` | preview ID, grant digest, revision, confirmed task IDs and delegated authority | no |
| `automation_blocked` / `automation_resumed` | per task and run: why automation stopped (logged once per reason) and when it may continue | no |
| `pm_decision_resolved` | decision ID; the owner's answer is kept in `.corral/state/pm-decisions.json` | yes |

A resolved owner decision can unblock work, so it wakes subscriptions. Decision presentation
receipts live in `.corral/state/automation.json` and per-pane effective-policy delivery in
`pm brief` health (`policy_delivery`); neither is an event, and the `pm_decision_presented` and
`pm_policy_delivery` kinds are never actionable. Effective-policy replacement to a remembered
live PM is mandatory and independent of `push_events`/`push_kinds`; see
[PM policy boundaries](pm.md#policy-boundaries).

### Active-worker integration notices

Recorded `approved` landings include the project, base branch, previous base SHA and landed SHA.
Sync coalesces relevant landings into informational worker notices, independently of PM push.
`main_movement_notice` events include a stable notice ID, run/pane identity, landed tasks/commits,
overlapping paths, context artifact, receipt path and any transport submission evidence.
States are `pending`, `deferred`, `sending`, `delivered`, `failed`, `uncertain` and `cancelled`;
these events are accounting records, not actionable `wait --for` kinds.

Deferred notices preserve pending owner replies and resume when the worker is receptive.
Interrupted sends become uncertain rather than blindly replaying a possible delivery.
See [early main-movement notices](automation.md#early-main-movement-notices) for scope declarations,
coalescing, identity checks and integration boundaries.
