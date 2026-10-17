# T-109: Organize work, not a team of PMs

Report-only product spike. October 17, 2026. Source baseline:
`33072fe192e80dae34ae570e80f936e1cb921c56`.

## Recommendation

Choose **work-centered views with on-demand agent judgment (model 2)** as the
product direction. Keep one optional conversational PM (model 1) as an entry
point, and keep scheduling, verification, repair delivery and acceptance rules
in code. Do not make persistent lead/project PMs (model 3) the default.
Nested work views must not create nested agents.

The problem to solve is: **"What outcome is moving, what decision needs me, and
can I safely leave and return?"** A larger agent organization does not itself
answer that. Current corral already has much of the deterministic machinery;
the missing product layer is a durable, scoped account of work and outstanding
decisions. This recommendation is a product judgment, not a measured victory
of a prototype. [E1-E6 in the evidence companion](T-109-product-direction-evidence.md)

Recommend reconsidering T-048 as an optional concurrency/authority capability
only after demand is demonstrated, rather than adopting its whole PM hierarchy.
**This report changes neither its specification nor its disposition.** Readback
on October 17 showed T-048 Dropped, T-107 Idea, T-102 Needs me, and this run
Running. The task body's earlier "Keep Idea + owner-only" instruction is not
used to undo this dispatched run or authorize any successor. [E7]

## What corral manages

Corral should manage owner intent, reviewable repository work, its dependencies,
execution attempts, decisions and evidence through acceptance and landing.
Herdr manages panes; a harness manages reasoning context. A pane, a chat
transcript and a task are different objects. A completed plan parent is not
proof that its implementation children have landed. [E1,E3,E4]

Keep these concepts separate:

- **Project:** repository and execution/policy boundary.
- **Initiative:** proposed owner outcome grouping repository tasks; not a
  multi-repository coding run or an atomic cross-repository merge.
- **Task/run:** one reviewable change and its attempts, exact candidates,
  gate/reviewer records and artifacts.
- **View:** projection over those records. Grouping does not grant authority,
  consume an agent slot, or change task state.

Today tasks have a project, dependency IDs, labels, plan backlinks and children.
Approved plan decomposition already creates linked Backlog tasks, but children
inherit the parent's project. Arbitrary cross-repository initiative hierarchy
and nested board navigation are proposals, not shipped features. The board
currently groups by status, with idea/Done/column visibility controls, rather
than a project/work tree. CLI project queries and history search already offer
partial scoped access. [E3]

## Three operating models

| Model | Owner journey | Useful agent judgment | Trade-off |
|---|---|---|---|
| **1. One conversational PM + deterministic workflows** | Describe an outcome; approve the captured scope/ready set; let code advance workers; answer exceptions; inspect review and accept under policy. Return through brief/next, tasks and reports. | Clarify intent, classify unfamiliar work, explain an exception and commission bounded design/review. Not repeatedly interpret test logs or count slots. | Smallest departure from today and approachable for chat-first owners. One long conversation can become a bottleneck or conceal unresolved choices. Durable state must outrank its memory. |
| **2. Work-centered/nested views + on-demand judgment** | Open outcomes/projects, drill into tasks and the decision shelf; ask an agent about selected evidence only when needed; authorize work; inspect results and accept. Chat remains optional. | Compare scope alternatives, diagnose a nonroutine repair, assess cross-repository contracts, summarize evidence with references. Return the answer to durable work, not a private chat branch. | Best fit for leaving and returning without reconstructing an organization. Requires view and membership design; a board alone cannot understand intent or judge correctness. |
| **3. Persistent lead + project PMs** | Tell the lead an outcome; agree scope and project ownership; project PMs coordinate local exceptions; lead presents digests and cross-project choices; owner accepts. | Sustained simultaneous project-specific judgment, plus lead trade-offs across projects. | Potentially less interleaving, but additional sessions, duplicated context, handoffs and stale digests. Safe concurrent mutation needs fencing, role checks and recovery, not merely separate panes. |

Models 1 and 2 can share the same code and optional PM conversation. Model 3
is justified only if concurrent *judgment*, not raw event volume, remains the
bottleneck after deterministic operation and scoped readbacks. The existing PM
role is a prompt policy, not an OS sandbox; current single-pane storage is not
the proposed writer lease. [E1,E2,E6]

Prefer model 1 if the owner wants conversation to do the triage and rarely
opens a board: model 2 otherwise shifts navigation effort onto them. Repeated
on-demand calls may reload the same context and lose resident-session cache
benefits, so fewer persistent agents does not guarantee lower total spend.
Model 3 remains credible for several simultaneously judgment-heavy projects;
evaluate that residual workload rather than rejecting it on session count.

## Same four scenarios

The comparisons below are **scenario walkthroughs and hypotheses**, grounded
in current contracts and recorded incidents, not live A/B trials. Each row
compares owner effort, missed decisions, execution/context cost, recovery and
authority. No numeric owner-time or token savings are inferred.

### A. One project with many tasks

Current baseline: queue admission, WIP/resource guards, gate/reviewer/autopilot
stages, deterministic digests, and one prioritized next-action card already
exist. A card is not an exhaustive decision inbox. [E1,E2]

| Model | Effort and missed decisions | Cost, recovery and authority |
|---|---|---|
| 1 | Owner speaks once and handles exceptions. Routine notifications can crowd out a lower-priority unresolved choice if the PM only follows the highest-ranked card. | One growing PM context plus normal workers/reviewers. Rebuild from brief/next and task records, not chat recollection. Code retains admission and owner flags. |
| 2 | Owner can scan work and all unresolved decisions without reading every task event. Collapse must preserve outstanding counts and a global decision shelf, or it merely hides the same problem. | No resident PM call needed for routine navigation; bounded queries add context on demand. Restore the selected work from canonical records. Views remain read-only until an explicit existing action is chosen. |
| 3 | Lead can compress updates, but a single active project gains little from an extra handoff; decisions can fall between project PM and lead. | Lead and project contexts plus digest/coordination work. Recover both sessions and scope ownership. Neither gains the owner's acceptance powers. |

### B. An initiative spanning repositories

Example: a substrate contract change followed by consumer pin updates and
consumer verification. Current tasks can link dependency IDs, but plan-generated
children clone the parent project. Do not claim existing decomposition provides
cross-project rollout or end-to-end initiative acceptance. [E3]

| Model | Effort and missed decisions | Cost, recovery and authority |
|---|---|---|
| 1 | Owner explains the outcome and confirms linked repository tasks. The PM must keep contract compatibility and rollout decisions explicit; a task dependency alone cannot specify them. | One context must span repositories. Recover using task links, contract evidence and landing receipts. Each repository lands independently; no global approval shortcut. |
| 2 | Owner follows outcome -> repository tasks -> exact blocker/decision. Explicit membership and an initiative exit checklist make omitted consumers visible; automatic grouping by shared files or dependencies could omit or overinclude work. | Fetch only selected contracts/results for judgment. Reconstruct the view from member tasks. Initiative completion must distinguish plan accepted, tasks landed and rollout verified, without authorizing child runs. |
| 3 | Project PMs can reason locally in parallel; lead reconciles compatibility, priority and spending. Missing consumer work or a delayed escalation remains possible. | Local contexts may shrink, but shared contract context and summaries are duplicated. Recovery requires durable dependency/decision records plus fenced scopes; lead is not a wildcard project writer. |

### C. A failed-review repair loop

Current baseline: independent findings can return to the same worker;
round limits escalate; gates/verdicts bind candidate identity. Clean equivalent
replay can carry an approval only with fresh integrated verification and
matching inputs; conflicts/changed patches require renewed review. Recorded
PM incidents included mistaking state signals and overstepping on timeout,
not simply a shortage of PM agents. [E2,E5]

| Model | Effort and missed decisions | Cost, recovery and authority |
|---|---|---|
| 1 | Code forwards routine repair; PM explains a scope change or exhausted budget to owner. A prose "pass" or historical approval can mislead if the PM bypasses the card. | Keep existing worker/reviewer costs; a PM reading every log adds avoidable context. Resume from findings and current candidate, never by restoring Review to force an old verdict. |
| 2 | Open failing work -> current gate/review attempt -> exact findings -> repair state. Separate "worker can repair" from "owner must decide"; do not bury new findings under an old green stage. | Routine repair needs no additional coordinator call. On-demand investigation gets a bounded evidence packet. History survives context loss; it does not make stale proof current. |
| 3 | Project PM manages the local loop and escalates changed requirements or limits; lead adds value only when another project's contract or priority is involved. | Extra handoff can delay the actual repair and duplicate findings. Persist the exact unresolved request and delivery state. No PM resets rounds, waives findings or accepts on silence. |

### D. Pause/resume after context loss

Current baseline: progress/checkpoints bind run, spec, HEAD, base and worktree
digest; waits/resources cannot silently disappear. Progress is not acceptance,
and old evidence does not transfer to a replacement run. Event cursors are
bounded current-log positions, not durable proof every notification survived.
Push may skip events while absent; read current state on return. [E4,E5]

| Model | Effort and missed decisions | Cost, recovery and authority |
|---|---|---|
| 1 | Owner returns to one conversation, but PM must reconstruct unresolved decisions from state. A rolling prose handoff alone can omit waits or cleanup. | Pay context reconstruction once; continuity of the old transcript is optional. Reload canonical spec/checkpoint, reconcile stale identity and obligations, re-arm runtime subscriptions. |
| 2 | Owner returns to a work/decision view, then opens only ambiguous items. Surface stale snapshots, unresolved questions and retained cleanup duties even inside collapsed work. | Rebuild projections without model calls; selectively reload evidence for judgment. No implied approval, redispatch or automatic replay of an uncertain command on resume. |
| 3 | Some project sessions may survive, but owner/lead must distinguish current decisions from stale digests and recover failed holders. | Multiple context recoveries and lease/controller reconciliation. Durable work is still required; a live pane cannot prove live authority or settled owner decisions. |

Pausing future admission is different from cancelling a worker or stopping an
already-running gate. A proposed "pause initiative" must name which of those
it means; no view should silently acquire stop/replay authority.

## What is measured

- **Rerun here:** the installed CLI's frozen replay read 50 events and eight
  labelled cases, covering seven policy situations; code-policy scored **8/8,
  zero policy violations**. No network model was invoked. Its zero model-call
  cost is not zero CPU cost or proof of model 2's superiority. Candidate replay
  and required gate results are attached separately as raw logs. [E8]
- **Historical, not rerun here:** T-045's fixed October 15 window recorded
  30 approvals, 52 done handoffs across 32 tasks and 19 change requests across
  14 tasks. Its 92 core-loop-command messages accounted for 22,960,177 token
  traffic including cache fields and $6.84 in reported cost. That busy manual
  session is not today's workload or verified billing. [E5]
- **Not measured:** owner minutes/actions saved, missed-decision rates,
  comparative live model quality, multi-PM throughput, or three-model spend.
  Less routine context for model 2 and locality gains for model 3 remain
  hypotheses. Existing replay validates selected policy choices, not coverage
  of every owner decision or recovery of a whole initiative.

## Lightweight workflow and view sketches

These are interaction proposals, not screenshots or implemented interfaces.
Existing actions keep their current policy checks.

```text
Optional conversation                    Work navigator
"Ship compatible contract X" ------->    Workspace: 3 owner decisions
capture scope, alternatives, evidence     > Initiative: contract X [proposed grouping]
owner authorizes plan/ready set             > substrate: task A -> landed SHA
                                              > consumer: task B -> waiting on A
code: queue -> worker -> gate -> review      > Project: corral
       ^                    |                 > Approved plan -> canonical children
       +--- bounded repair -+
                                         Global decision shelf [never hidden by scope]
owner: exact decision / acceptance        task / question / candidate / options / evidence
       |
       +-> recorded result -> digest     Open task -> current attempt -> history/artifacts
                                         Ask agent -> bounded answer -> durable record
```

The alternative model-3 overlay would be owner -> lead -> project PMs ->
the *same* code workflows, with digest/escalation links back to the lead.
It must not replace the global decision shelf or create a second task database.

The owner keeps intent, scope/priorities, authorization and owner-only plan or
implementation acceptance. Code keeps queues, resource/dependency admission,
candidate freshness, gates, delivery accounting and policy-permitted
transitions. Workers repair and implement; independent reviewers judge evidence
and correctness. On-demand agents advise; a recommendation is not an approval
receipt. Explicit existing automatic policies remain possible, but this report
enables none and grants no new authority. [E1,E2,E4,E6]

## Smallest useful next increment and remaining choices

Propose **one read-only work navigator** over existing project, plan-child and
dependency records, with a persistent workspace decision count and access to
all unresolved owner items. Start with project -> approved-plan children ->
existing task detail; allow linked external-project dependencies to remain
visible as links, not cloned tasks. No new initiative schema, PM process,
mutation tools, scheduler or harness migration in this increment. Do not build
another digest: it already ships. [E1,E3]

Its acceptance should prove: task IDs/states remain canonical; scoped views
do not hide global unresolved decisions; planning-parent Done does not imply
child delivery; stale/failed/current attempts remain distinguishable; rebuilding
after restart neither writes task state nor launches agents. Evaluate the same
four scenarios against today's board plus optional PM. Record owner actions,
rescues, omitted decisions, model/context usage with coverage, and time to
recover. Use isolated fixtures and explicit owner-approved budgets for any
paid model comparison. Require zero unauthorized actions and no lost decisions;
agree improvement thresholds before using them to justify rollout.

Owner decisions, **not blockers to accepting this report**:

1. Choose work-first with optional conversation, or chat-first model 1.
2. Authorize the read-only increment and its evaluation thresholds, separately.
3. Before cross-repository initiative editing, decide explicit membership and
   exit/rollout criteria; before pause controls, decide their exact semantics.
4. Decide whether to replace/narrow T-048 later. Revisit persistent PMs only if
   measured simultaneous project judgment justifies their coordination cost;
   authorize actor fencing before concurrent PM mutation.

T-107 remains the narrower live review-interaction audit; this spike does not
claim its screenshot or mouse/keyboard investigation. T-102's native Pi
integration concerns transport, tools and lifecycle, not the choice of work
model: any of these three can use a harness adapter. Neither is a prerequisite
for this recommendation, and no linked task is edited, promoted or dispatched.
[E7]
