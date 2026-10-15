# Run progress and checkpoints

Execution progress belongs to a task's current run, independently of the agent
harness. Read the current identity before preparing a write:

```sh
corral run progress T-058 --json
corral run checkpoint T-058 --json
```

Both commands, without `--file`, return the same identity and state without
writing: `task`, `run`, `spec_revision`, `head`, `base`, `tree`, `progress`, and
`checkpoints`. `base` is the current integration base commit, not the dispatch
base. `checkpoint_status` is `missing`, `compatible`, or `stale`, with
machine-readable `checkpoint_reasons` for the current worktree and spec.
`tree` is the server-computed digest covering HEAD and tracked and
untracked worktree content. Treat these identity strings as opaque; copy them
from readback rather than deriving them yourself.

With `--file`, the command parses the corresponding typed JSON payload, writes
through the guarded operation, and then returns fresh identity and state:

```sh
corral run progress T-058 --file progress.json --json
corral run checkpoint T-058 --file checkpoint.json --json
corral show T-058
corral show T-058 --json
```

Human output includes phase and step IDs, labels, states, criterion links,
checkpoint revisions and artifacts, next actions, decisions, and outstanding
wait/resource ownership. `show --json` includes stored progress and checkpoint
records for each run; the run commands provide current writable identity.

## Progress payload

For a first progress write, use `expected_revision: 0`. Replace the example's
identity strings with the exact values from readback:

```json
{
  "task": "T-058",
  "run": "R1",
  "spec_revision": "<readback.spec_revision>",
  "expected_revision": 0,
  "phases": [
    {
      "id": "implementation",
      "label": "Implementation",
      "steps": [
        {
          "id": "wire-cli",
          "label": "Wire the CLI",
          "status": { "state": "in_progress" },
          "criteria": []
        },
        {
          "id": "write-docs",
          "label": "Document payloads",
          "status": { "state": "pending" },
          "criteria": []
        }
      ]
    }
  ]
}
```

`status` is a nested tagged object. Its `state` is `pending`, `in_progress`,
`done`, or `dropped`. A dropped step supplies a reason, for example
`"status": {"state": "dropped", "reason": "Replaced by the combined check"}`.
Unknown input fields are rejected by the typed schema.

Send the full phase/step ledger for each update, not a partial patch. On later
writes, use the returned `progress.revision` as `expected_revision`. Keep phase
and step IDs stable when changing labels or states; IDs identify work, not its
position or wording. `criteria` stores opaque acceptance criterion IDs from the
task contract; an empty list means no criterion link. Progress-only mode does
not register or satisfy criteria.

Phase IDs must be unique and nonempty; step IDs must be unique and nonempty
across all phases. Labels must be nonempty, and at most one step may be
`in_progress`. Retain existing phases and steps in later updates: drop unwanted
steps with a reason instead of removing them. Done and dropped states are
terminal and cannot be changed by a subsequent update.

Step completion is execution bookkeeping, not acceptance evidence. Marking
every step done does not satisfy criteria, pass a gate, approve a review, or
finish the task. Use the separate evidence, gate, verdict, and approval workflow.

## Checkpoint payload

Save a checkpoint after publishing progress. Copy the returned progress revision
and current identity into the checkpoint:

```json
{
  "task": "T-058",
  "run": "R1",
  "spec_revision": "<readback.spec_revision>",
  "expected_revision": 0,
  "progress_revision": 1,
  "head": "<readback.head>",
  "base": "<readback.base>",
  "tree": "<readback.tree>",
  "criterion_ids": [],
  "evidence_refs": [],
  "next_action": "Finish the CLI documentation",
  "decisions": ["Use typed JSON payloads"],
  "waits": [
    {
      "id": "ops-contract",
      "owner": "implementer",
      "handle": "ops-session",
      "condition": "The progress operations are available"
    }
  ],
  "resources": [
    {
      "id": "scratch-server",
      "owner": "implementer",
      "description": "Isolated local test server",
      "cleanup": "Stop the isolated server when verification ends"
    }
  ]
}
```

The checkpoint `expected_revision` is independent of the progress revision:
use zero for the first checkpoint and the latest returned
`checkpoints[].checkpoint.revision` for subsequent saves. `progress_revision`
must match the progress being captured (zero when no progress exists). The
checkpoint records its captured progress, completed/current step IDs, timestamp,
and artifact reference. `criterion_ids` are opaque links; `evidence_refs` are
existing snapshot paths returned by `corral artifact add`. A checkpoint does
not create acceptance evidence. Use empty
arrays when there are no decisions, waits, resources, criteria, or evidence.
Wait/resource IDs remain stable while their owner, condition, or cleanup duty
is being tracked.
Pending waits and resources cannot disappear from the next checkpoint. To close
one, retain its ID and add a nonempty `resolution` string describing the result
or cleanup receipt. Resolved obligations remain in that checkpoint's history.

## Retry and stale writes

```sh
corral retry T-058 --run R1
corral retry T-058 --run R1 --profile implementer
```

`--run` guards the source run explicitly and requires a current, valid checkpoint
for that run. It is not a selector for replaying an arbitrary historical run.
Without `--run`, retry continues through the existing retry operation. Profile,
model, and effort selection retain the existing retry options.

Run/spec/revision mismatches and checkpoint identity changes are stale input,
not permission to overwrite newer state. A checkpoint can become stale when
progress, the task specification, HEAD, the integration base, or worktree content
changes. Read back again, reconcile the changes and remaining obligations,
then submit a new payload with current revisions and identity before retrying.
Do not blindly replace revision numbers in an old payload or automatically
retry a rejected write.

Dispatch includes a structured `<corral-resume>` packet even when a harness
overrides the human prompt template. Its canonical title and specification
always come from the task store. The newest compatible snapshot supplies
progress; when that snapshot is older than the latest checkpoint, the packet
names it in `compatible_checkpoint` and reports the newer checkpoint's stale
reasons. The latest `checkpoint` still carries current decisions, waits, and
resource obligations. A replacement run must publish its own checkpoint before
its next handoff; inherited snapshots never count as a new completion receipt.
When the old worktree or worker is unavailable, retry records that limitation
and falls back to the task contract and retained historical context.

The PM scoped this delivery to progress-only support against current main.
Binding criterion IDs and artifact references to T-052's canonical criteria and
evidence API is a separate PM-approved follow-up. The shared
`validate_references` function in `corral-ops/src/progress.rs` is the single
validation seam for that follow-up; no T-052 implementation is copied here.

When native waits or owner decisions are registered, checkpoints also freeze the
source run's `run_obligations`. Resume packets expose its latest records as
`prior_run_obligations`, including on stale or missing checkpoints. These are
context for the new worker, not new registrations, transferred ownership, or
resume receipts. A changed native obligation ledger requires a fresh explicit
handoff checkpoint. Resending a checkpoint request cannot answer an owner
decision.

## Files and errors

Input must be an ordinary regular JSON file, not a directory, symlink, FIFO,
device, socket, or stdin (`--file -` names a literal file). Both commands enforce
the configured `artifacts.max_bytes` limit (default 20,000,000 bytes), including
a bounded read if the file grows. Malformed JSON and schema errors name the
input path. Operation errors retain task/run context.

Successful `--json` output is a single readback object on stdout. Failures exit
1 and print one JSON object on stderr with `command`, `task`, `error`, and
`hint`; stdout has no success record. Argument/help errors retain the standard
CLI parser format. A failure during the final readback can occur after a write
has succeeded: inspect current state before resubmitting.
