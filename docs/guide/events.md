# Board events and PM awareness

`corral wait` waits for one task or configuration event, then exits. It starts at the current end of
`.corral/events.jsonl`, so old events do not wake a new subscription.

```bash
corral wait --for done,needs_me,stuck,blocked,escalated --timeout 30m
corral wait --task T-041 --json --print-cursor
corral wait --project corral --since 2026-10-15T00:00:00Z --json --print-cursor
corral wait --after-offset 12345 --json --print-cursor
corral wait --for config_changed,config_invalid --json --print-cursor
```

`--for` defaults to `done`, `needs_me`, `stuck`, `blocked`, `escalated`, `config_changed`
and `config_invalid`; `--task` and `--project` narrow the match. Configuration events
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
push_kinds = ["done", "needs_me", "stuck", "blocked", "escalated", "config_changed"]
```

Push is off by default, including for existing configs. Enable it in `.corral/corral.toml` or
Settings > General > PM event push. PM push kinds is a comma-separated list of those six
actionable names; clearing it selects none. Invalid names block saving. `config_changed`
wakes the PM only when the event's `pm_relevant` flag is true. `config_invalid` is available
to `wait` and config history but is not a PM push kind.

Sync sends selected events as `[corral]` messages only to the remembered, live PM pane, using
the same `agent_prompt` and terminal fallback as replies. It does not start a PM or send to
coding-agent panes. Position persists in `.corral/state/pm-events.json`. Disabled push,
unavailable herdr, and an absent PM checkpoint the log end during sync: skipped events are
not replayed later. A first-ever enabled sync with a live PM and no saved cursor considers the
retained log from offset zero. To skip old history, run one sync with push disabled first.
Event delivery is awareness, not authorization to dispatch, approve, or recover a process.

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
