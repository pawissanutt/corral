# Automation: review gate, reviewers and the autopilot

By default corral does nothing on your behalf after an agent finishes: the task lands in **Review** and you
decide. Automation is opt-in and layered, and each layer can be used on its own:

1. a **gate**: shell commands (tests, lint) run in the run's worktree;
2. a **reviewer**: a second agent that reads the diff and records a verdict;
3. the **autopilot**: advances the run through gate and review, sends failures back to the task agent, and
   either lands the change or hands it to you.

Settings live under `[review]` in `.corral/corral.toml`; every key is listed in
[configuration](configuration.md#review).

## Review policy

The effective policy for a task is built from four layers, later ones winning:

1. built-in defaults: no gate, no reviewer, `accept = "owner"`, `max_rounds = 2`, `merge = "rebase"`,
   `remove_worktree = true`, `max_diff_lines = 1500`;
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
max_diff_lines = 800
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

## Reviewer runs and verdicts

`reviewer` is one of:

- `"none"`: no independent review;
- `"pm"`: the workspace PM agent reviews;
- a profile name: a fresh agent with that profile reviews in its own pane. Pick a different (often stronger)
  model than the one that wrote the code.

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

## The autopilot

When `corral done` moves a run to Review and the policy has a gate, a reviewer or `accept = "auto"`, every
sync advances that run with no action from you. Syncs happen in the board, in the herdr plugin's event hook, and
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

### What always needs you

With `accept = "auto"`, these guards stop the autopilot and put the task in Needs me with the reason spelled
out and a notification:

- the diff is larger than `max_diff_lines`;
- the change touches a path matching `owner_paths`;
- the task says `review: owner`;
- the worktree is missing or has uncommitted changes after `corral done`, or changed during review;
- the reviewer closed without a verdict, or its verdict was `escalate`;
- the gate pane closed before the gate finished;
- all `max_rounds` change rounds are used.

From Needs me you can approve, request changes, retry or discard like any other Review task.

### Landing

Landing needs the project's main checkout to be on the base branch and clean; otherwise the run is `blocked`
until that is true. A base that moved is rebased onto; a conflict is sent back to the agent. With
`merge = "ff"` the agent is asked to rebase itself. The gate re-runs on the rebased commit, then the base is
fast-forwarded and, with `remove_worktree = true`, the worktree is removed. Nothing is pushed to any remote.

### Acceptance record

Every task the autopilot lands gets an **Acceptance record** artifact: the gate log, the verdict and notes, the
number of rounds, the landed commit and the policy that was in force. Find it in the artifact finder or with
`corral artifact list --task T-042`.

### Kill switch

```bash
corral autopilot status    # what it is doing and which tasks it is driving
corral autopilot off       # stop all automatic advancing
corral autopilot on
```

`corral autopilot off` (equivalently `autopilot = false` under `[review]`) is re-read on every sync, including
by a long-running board or `sync --watch`. With it off nothing advances automatically; all manual commands
(`corral gate`, `review-run`, `verdict`, `approve`, `changes`) keep working. The setting is workspace-wide and
is rejected under `[projects.<key>.review]`. In the board, `O` or a click on the header's `autopilot on/off`
flips it, and the settings screen's Automation section edits the whole policy.

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
