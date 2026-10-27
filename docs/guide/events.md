# Board events and PM awareness

Adding a [task comment](comments.md) appends a `commented` event with the structured
comment (`author`, `at`, `text`) and whether delivery was requested (`send`). Comments
are durable owner context, not a task-spec revision or an automatic dispatch instruction.

`corral wait` waits for one task or configuration event, then exits. It starts at the current
database event tail, so old events do not wake a new subscription.

```bash
corral wait --for done,needs_me,stuck,blocked,escalated --timeout 30m
corral wait --task T-041 --json --print-cursor
corral wait --project corral --since 2026-10-15T00:00:00Z --json --print-cursor
corral wait --after 12345 --json --print-cursor
corral wait --for config_changed,config_invalid --json --print-cursor
```

`--for` defaults to every actionable kind: `done`, `needs_me`, `stuck`, `blocked`, `escalated`,
`config_changed`, `config_invalid`, `lessons_due`, `run_resumed`, `wait_transition` and
`pm_decision_resolved`; `--task` and `--project` narrow the match. Configuration events
are workspace-wide and do not match task/project filters. Unlike PM push, an explicit
config subscription includes UI-only changes as well as policy changes.
`--since` includes events at or after an RFC3339 timestamp and scans retained history.
`--after SEQ` resumes strictly after a stored event sequence rather than starting
at the tail; `--after 0` replays all history. When combined, `--since` still filters timestamps.
The removed byte-offset cursor flag is rejected with a message: byte offsets cannot be used
as event sequences.
Without `--timeout`, the wait is
unlimited. Durations need a suffix: `ms`, `s`, `m`, `h`, or `d`. A match exits 0; timeout exits 124.

JSON output is the stored event (`seq`, `ts`, `kind`, `task`, `detail`), not a normalized replacement.
`--print-cursor` adds `cursor`, an object containing only `seq`. Human output appends
`cursor={"seq":12345}`. Re-arm after every wake with `--after <seq>` so events between waits
are considered. Database output contains no byte offsets or file identity.
Timeout prints nothing: retain the previous successful `seq` (equivalently `cursor.seq`).
Before the first success, retain an original `--since` timestamp for catch-up after timeout;
starting a fresh default wait at the tail could skip events between subscriptions.

Sequences are durable, increasing database positions. Readers resume from the stored sequence;
they do not depend on a JSONL file's size, rotation or byte boundaries.

Export retained history as stored Event JSONL, ordered by sequence:

```bash
corral export events --since 0 --json
corral export events --since 12345 --json
```

Export's `--since` is an exclusive sequence checkpoint, unlike wait's timestamp `--since`.
It defaults to 0 (all events); `--json` is required. Each line includes the stored `seq`,
timestamp, kind, optional task and detail. An export with no later events emits nothing.
`corral export tasks DIR` still exports Markdown task records.

```bash
corral export events --since 12345 --with-conditions --json
```

`--with-conditions` appends one `{"condition_run": {...}}` line, in opened order, for every
[condition run](#condition-runs) that is still open or whose `opened_seq` or `closed_seq` is
greater than `--since`. A consumer that re-exports from its last seen sequence therefore
receives every run summary that could have changed since then.

### Condition runs

Five noisy kinds are recorded as condition runs keyed by task and kind: `dispatch_failed`,
`queued`, `landing_stale`, `autopilot_error` and `main_movement_notice`. The first observation
appends an event and opens a run. An observation whose detail is identical (same canonical
JSON) to the open run's only increments the run's `count` and its `last_at`, `last_actor` and
`last_via`; it appends no event. A different detail appends a new event, which closes the old
run as `changed` and opens a new one. When the condition resolves, a `<kind>_cleared` event
(`run`, `count`, `first_at`, `last_at`) closes the run as `cleared`; with no open run nothing is
written. Each run is a durable row, so later changes, clears and restarts never overwrite an
earlier run's count, first and last time or attribution. Only the individual timestamps of
identical repeats inside one run are not kept.

| Kind | Cleared by |
|---|---|
| `queued`, `dispatch_failed` | a write that moves the task out of Ready (a successful dispatch, a dequeue or any move) |
| `landing_stale` | a landing that records `approved` |
| `autopilot_error` | an autopilot pass over the task that ends without an error |
| `main_movement_notice` | the notice reaching a terminal state (`delivered`, `uncertain` or `cancelled`) |

A kind whose resolution is never observed keeps its run open. Events imported from a legacy
log keep their original lines and have no runs.

`corral log <id> --json` orders entries by `seq`, so events with equal timestamps keep their
commit order. An entry whose event opened a run carries `repeats` (`count`, `first_at`,
`last_at`, `last_actor`, `last_via`, `closed_seq`, `close_reason`), read when the log runs: an
open run's count can still grow. `corral wait` never wakes on a suppressed repeat, because a
repeat appends no event.

Verdict notes of 16 KiB or more are stored once as a blob, and the `verdict` event's `notes`
holds its `blob:sha256:<hex>` reference. `log`, `review-sprint` and the other reports read the
text back from the blob; `export events` emits the reference as stored.

Workspace PMs on OmO or Claude Code should subscribe through their harness's `monitor`, not
foreground polling. Monitor the completion of `corral wait --json --print-cursor`.
On omp or other harnesses, run `wait --json --print-cursor` in the background and use the
harness's completion notification, then re-arm with the returned sequence. A dispatched coding
agent still follows its Run contract and finishes with `corral done`.

### Optional push to the PM

```toml
[pm]
agent = "omo"
push_events = false
push_kinds = ["done", "needs_me", "stuck", "blocked", "escalated", "config_changed",
              "lessons_due", "run_resumed", "wait_transition", "pm_decision_resolved"]
```

Push is off by default, including for existing configs. Enable it in `.corral/corral.toml` or
Settings > General > PM event push. PM push kinds is a comma-separated list of those ten
actionable names (all ten by default); clearing it selects none. Invalid names block saving. `config_changed`
wakes the PM only when the event's `pm_relevant` flag is true. `config_invalid` is available
to `wait` and config history but is not a PM push kind.

Sync sends selected events as `[corral]` messages only to the remembered, live PM pane, using
the same `agent_prompt` and terminal fallback as replies. It does not start a PM or send to
coding-agent panes. Position persists in `.corral/state/pm-events.json`. Disabled push,
unavailable herdr, and an absent PM checkpoint the event tail during sync: skipped events are
not replayed later. A first-ever enabled sync with a live PM and no saved cursor considers the
retained history from sequence zero. To skip old history, run one sync with push disabled first.
Event delivery is awareness, not authorization to dispatch, approve, or recover a process.

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
