# Pi-native agent questions

A Pi-native worker or reviewer (plan T-252) never waits on screen text for an answer. It asks
through `corral agent call --op ask`, and corral stores the question on the asking run or review
child. The PM triages it by key and revision, and the owner decides anything the PM may not.
OmO runs keep their native question handling; this page covers Pi-native runs only.

## Keys and revisions

Every question has a canonical, globally unique key:

- `<task>/<run>/Q<n>` for a worker run, for example `T-a/R1/Q1`
- `<task>/<run>/<review-child>/Q<n>` for a review child, for example `T-c/R1/R1-review1/Q1`

A new review attempt starts again at `Q1` under its own child ID, so an answer can never reach
another attempt. Every transition (answer, escalate, cancel, withdraw, expire, closure) bumps the
question's revision. Every PM or owner action names the current revision with `--rev`, and corral
refuses a stale one (`stale_revision`).

## States

| State | Pending | Reached by |
|---|---|---|
| `open` | yes | `ask` |
| `escalated` | yes | `corral question escalate` |
| `answered` | no | a PM or owner answer, or an owner supersede |
| `expired` | no | an owner deadline that passed at an unchanged revision |
| `withdrawn` | no | the asking agent's `withdraw`, with its reason |
| `cancelled` | no | `corral question cancel`, or closure of the asking container |

A question is pending only while its container is open. A worker run closes on discard, retry,
drop or landing. Done in Review, a refused operation, a cancelled turn and a lost host leave it
open. A review child closes on a recorded verdict, `supersede_review`, a pane lost on the
existing path, a newer review attempt, or a closed parent run. Readers treat the questions of a
closed container as cancelled at once. The sync hook (`questions::terminalize`) then records
`cancelled` with the reason `run_ended`, `review_closed`, `review_superseded` or
`review_interrupted`. It also cancels the container's undelivered Pi-native messages
(`recipient_closed`). Any later action on those keys is refused `question_closed`.

## Asking (agent)

`ask` takes `kind` (`clarification`, `technical_choice`, `owner_only` or `permission`),
`blocking`, `text`, and optionally `options`, `recommendation`, `context_refs` and `needed_by`.
It returns the key and revision at once, whether or not the question blocks. A non-blocking
question lets the worker keep working, but `done` refuses while any question is pending, so the
worker waits or withdraws. `withdraw` takes the key and a reason.

Bounds, refused rather than truncated:

| Field | Bound |
|---|---|
| pending (open plus escalated) per run or review child | 8 |
| blocking per run or review child | 1 |
| `text`, `recommendation` | 4 KiB |
| `options` | 6, each 512 bytes |
| `context_refs`, basis refs | 16, each 256 bytes |
| answer text, deadline instruction | 8 KiB |
| reasons, notes, scope note | 1 KiB |

## PM triage

```bash
corral question list                       # pending questions, all tasks
corral question show T-a/R1/Q1 --json
corral question answer T-a/R1/Q1 --rev 1 --text "Use the default" \
  --basis spec:T-a@<revision> --scope-note "inside the approved write scope"
corral question escalate T-a/R1/Q2 --rev 1 --kind owner_only --note "needs a product call"
corral question cancel T-a/R1/Q3 --rev 1 --reason "superseded by the new spec"
```

The PM answers only `clarification` and `technical_choice` questions that are still open. It
must cite at least one basis ref, and every ref must resolve to a canonical object:

| Ref | Resolves to |
|---|---|
| `spec:<task>@<revision>` | the task's current spec revision |
| `plan:<task>@<plan-id>` | a submitted plan, for example `plan:T-252@P5` |
| `decision:<task>@<plan-id>#<n>` | the n-th decision of that plan |
| `decision:<task>/<run>/<decision-id>` | a registered run decision |
| `comment:<task>#<n>` | the n-th task comment |

The PM must also state a `--scope-note`. `owner_only` and `permission` questions always escalate.
The PM may reclassify a question upward when it escalates, never downward. A Pi-native PM uses
the `question_answer`, `question_escalate` and `question_cancel` ops. Each one commits in the
target task's transaction with a receipt in `Task.agent_pm_receipts`, after its liveness, fence
and project scope are rechecked there.

An answer, correction or expiry for a worker run is one task transaction. It takes the run's
inbox lock first and reserves the `question_answer` or `question_correction` message in the same
commit. A review child's `ReviewRun` inbox recipient lands with the integration-seams child.
Until then, a reviewer's answer is recorded on the question with delivery `pending_recipient`.

## Owner decisions

An escalated question is held indefinitely and nothing is applied automatically. On a worker run
the task moves to Needs me, and it returns to Running once no question of that run waits on the
owner. A reviewer's task stays in Review. Either way, `corral pm next` shows an
`owner_question` card with the key, revision and text.

```bash
corral question answer T-a/R1/Q2 --rev 2 --owner --text "Ship it behind the flag"
corral question deadline T-a/R1/Q2 --rev 2 --at 2026-10-24T09:00:00Z --instruction "Use option a"
```

`answer --owner` answers any pending question. On an answered question it supersedes the
earlier answer: the triage entry names the superseded revision, and a `question_correction`
message is queued. `deadline` is owner-only. When the deadline passes and the revision has not
changed, the question becomes `expired` with an `owner_deadline` answer carrying the
instruction, delivered like any answer. Any transition before then voids the deadline, and no
question ever expires without one.

## Unattended questions

Consider a blocking question, or one whose run's last turn ended `awaiting_answer`. If it gets no
PM triage for `[agent] question_unattended_secs` (default 600), `questions::unattended` marks it
once per revision and raises Needs me with the reason `question_unattended`. The sync call site
lands with integration-seams.

## Configuration and events

```toml
[agent]
question_unattended_secs = 600   # also gate_start_uncertain_secs = 300, drain_secs = 60
# assets_dir = "/path/to/fixture"  # explicit fixture or development installation only

[pm]
agent = "omo"
push_kinds = ["done", "needs_me", "question_opened", "question_escalated"]
```

`pm.push_kinds` accepts `question_opened`, `question_escalated` and
`turn_ended_without_report`. None of them is in the default list. Question events reference keys
and revisions only: `question_opened`, `question_answered` (with `supersedes_revision` for a
correction), `question_escalated`, `question_expired`, `question_cancelled`,
`question_withdrawn`, `question_unattended` and `question_deadline_set`.
