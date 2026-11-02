# Pi attention and decision inbox

Rich Pi sessions have a read-only **Inbox** main view. Press **F11**, click Inbox,
or enter `/panels inbox`. **F9** restores the existing conversation; the real Pi
composer and its unsent draft stay in place.

- Up/Down or j/k selects a record; a plain click selects the literal displayed item.
- Enter opens its read-only detail sheet. Esc closes it.
- `h` includes answered, expired, withdrawn, cancelled and other retained history.
- `/` filters by task, container, key, scope, state or reason; `r` refreshes.
- `a` refreshes the canonical source and, if still eligible, opens **instructions**
  for the established answer flow. It never executes the displayed command, sends
  an answer, edits the composer, journals owner input or claims a presentation.
- Tab returns keyboard focus to the composer without closing Inbox.

The reader uses `corral attention inbox --json` at the launch-bound workspace root,
not the Changes view's repository selector. The same CLI read can be narrowed with
`--project`. It includes native task/planning question history, portable run decision
records, retained PM coordination decisions and current attention conditions. It
is not a cross-workspace aggregator or a replacement for PM's computed next-action
recommendations. Inspection does not reconcile or terminalize source records.

Question rows carry the exact task/container/key and numeric question revision.
Other rows explicitly label a digest of their source as `source_digest`, not an
invented question revision. Effective native container closure is applied even
before a sync sweep has persisted cancellation. Details retain reasons, triage,
answers, source scope and the canonical inspection route. Historical portable
records keep their recorded receipt/observation and run identity.

## Answer authority

This view adds **no answer operation**. Worker/reviewer/planning roles cannot open
an answer flow from the panel. PM flow instructions require a native pending
question in that PM's project; the actual authenticated question-answer operation
still validates its generation, scope, basis references and scope note. Assistant
sessions can inspect instructions for a separate trusted owner terminal, but
cannot answer questions through their owner-action tool: that operation is not in
its closed kind set. A question kind or a displayed coordination answer never
creates a permission grant.

Stale revisions, missing/closed records, failed reads and ineligible flow requests
are refused. A removed selection is not silently replaced with another task.
Resolved/history and attention-only rows are inspection-only here; existing owner
correction and other control-plane operations remain outside this panel.
Unavailable, malformed or over-limit reads are shown as unavailable/stale, never
as a successful empty inbox. The last successful snapshot may remain visible with
its observation timestamp. Navigation and details do not dispatch, accept,
restart, resume, attend, present or authorize anything.

## Isolated real-input check

Build the candidate binary and the disposable `pi_inbox_fixture` example, then run
`scripts/qa-pi-inbox.py` with its output directory, `--corral` candidate path and
`--seed` example path. It owns a fresh PTY and private HOME/XDG, uses the production
host entrypoint and candidate Rust CLI, and deletes its fixture on exit. It makes
no model requests and never attaches to a live herdr pane. Its result, ANSI frames
and before/after canonical-file hashes prove keyboard/mouse inspection, history,
answer-flow navigation and draft continuity without answer or owner-input effects.
