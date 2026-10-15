# T-045: Move the PM's mechanical loop into corral

Research date: October 15, 2026. Research only; no product or live configuration changes.

## Recommendation

Reuse the existing gate, reviewer, review autopilot, routing, and queued
dispatch. Corral already implements most of this loop. The missing work is
reliable acceptance inputs, integration diagnostics, queue admission and
reservation, post-land execution, and early overlap notices. Do not build a
second scheduler or replace review with a green test result.

Move observation, checks, scheduling, factual change-request construction,
landing, cleanup, and receipts into code. Keep one bounded independent
reviewer call for correctness, scope, evidence sufficiency, and UI judgment.
Keep prioritization, ambiguous requirements, consequential exceptions, and
owner decisions with the PM/owner. Neither a summary claiming success nor
an LLM saying "approved" proves the integrated tree passed the required gate.

Start with project-scoped automatic gating and review, but owner acceptance.
Use fast-forward-only landing during this pilot. Enable automatic acceptance
after the freshness controls below are in place. There is no supported
configuration today that provides the entire requested loop without gaps.
[C1-C8; E1-E8]

## 1. Sources, snapshot, and limits

Repository anchors below refer to commit
`2fed45b6fc1945977b68a00db2698ef97c4dcd2f`, the initial T-045 worktree,
unless explicitly marked otherwise. This makes line references stable even
as main advances. Relative code paths are under `crates/`; other paths are
repository-relative.

- **E**: `/home/pawis/miti-workspace/.corral/events.jsonl`. Event line numbers
  below are one-based, with task and UTC timestamp as independent anchors.
- **P**: `/home/pawis/.omo/agent/sessions/--home-pawis-miti-workspace--/2026-10-15T08-56-31-597Z_01a13ec7-006d-7981-be64-4cddbbb20e41.jsonl`.
  Transcript references are one-based JSONL lines, not rendered chat lines.
- **W**: `/home/pawis/miti-workspace/.corral/corral.toml`, observed during
  this spike: `defaults.profile = "gpt-sol"`, `limits.running = 7`,
  no `[review]`, no routing rules, PM profile `haiku`. Profile definitions
  already include `gpt-sol`, `gpt-astra`, `opus`, and `opus-max`.
- **T**: task bodies under `/home/pawis/miti-workspace/.corral/tasks/`;
  filenames start with their task ID. These were read through local files
  and `corral show`, without editing them.

The numerical baseline is frozen at `2026-10-15T10:49:30.473Z`, immediately
before this task's dispatch. It includes T-037's approval at
`10:49:30.416697683Z`, but not T-045's dispatch. It is one busy PM session,
not a controlled performance experiment. Source inspection establishes
implementation behavior; proposed workflows below were not implemented or
benchmarked. Required repository gate receipts are attached separately.

### Code and document index

| Ref | Source and relevant contract |
|---|---|
| C1 | `corral-core/src/review.rs:49-79,117-141,189-215`; `docs/guide/configuration.md:250-285`. Gate commands, reviewer, owner/auto acceptance, two rounds by default, rebase, cleanup, 1,500 changed-line guard, owner paths; workspace -> project -> task acceptance override. The autopilot switch is workspace-wide. |
| C2 | `corral-ops/src/gate.rs:47-95,146-239,242-305`; `corral-core/src/model.rs:137-157`. Clean worktree, exact HEAD, sequential shell commands, exit-status capture, result file and log artifact; gate reuse is keyed to HEAD, not a policy fingerprint. |
| C3 | `corral-ops/src/reviewer.rs:88-141,212-298,378-430`; `corral-ops/assets/review.md`. Reviewer receives spec, diff command, gate and artifacts; read-only review and SHA-bound verdicts. |
| C4 | `corral-ops/src/autopilot.rs:73-85,179-259,274-435,439-620`; `docs/DESIGN.md:167-190`; `docs/guide/automation.md`. Per-task advance lock, gate -> reviewer -> accept -> land; changes/retries, blocked/escalated states, max rounds, owner guards, re-gating after rebase. |
| C5 | `corral-ops/src/review.rs:93-171,228-330`; `corral-git/src/lib.rs:263-335`. Done can commit outstanding changes and snapshots against dispatch `base_sha`; manual approve rebases/fast-forwards without running the gate itself. Safe merged-branch deletion already exists. |
| C6 | `corral-ops/src/sync.rs:22-76,303-354`; `corral-ops/src/dispatch.rs:11-23,39-105,269-297`; `corral-core/src/model.rs:80-82`. Queued tasks are already auto-started oldest-first; dependencies must be Done; Running and NeedsMe hold slots. |
| C7 | `corral-core/src/routing.rs:146-228`; `corral-ops/src/routing.rs:16-73`; `docs/guide/configuration.md:166-228`. Explicit override -> task profile -> first matching rule -> project/workspace default. This routes workers, not reviewers. Unavailable model discovery produces a warning, not necessarily a refusal. |
| C8 | `corral-core/src/store.rs:56-87,336-362`; `corral-ops/src/dispatch.rs:51-105,111-216`; `corral-ops/src/autopilot.rs:75,390-449`. Per-record and autopilot-task locks exist; dispatch's slot check and external launch are not one workspace-wide reservation. Landing is not one transaction spanning base observation, checks and fast-forward. |
| C9 | `corral-ops/src/acceptance.rs:72-129`; `corral-ops/src/stats.rs:16-41,78-207`; `corral-ops/src/usage.rs:48-100`. Acceptance artifact and outcome/usage reporting exist; unknown usage is not zero. |
| C10 | `corral-ops/src/watchdog.rs:12-22,199-254`; `corral-ops/src/sync.rs:79-279`; `docs/guide/automation.md`, "The watchdog". Idle is not completion; stuck-command recovery checks process identity and requires confirmation. |
| C11 | `AGENTS.md:17-40`; `scripts/check-docs.sh:1-21,77-140`. 800 SLOC ceiling; full Rust gate; docs validation checks documented CLI/config against a supplied built binary. |
| C12 | `docs/records/T-037-omo-native-adoption-report.md`, "Current corral foundations", "Five ready-to-file task specifications", specifications A/B. Reuse its completion audit and criterion/evidence ledger rather than filing competing versions. |
| C13 | T-041 landed during research as `3163615aad0a788022d08fed26be6c24ed957439`. At that commit: `corral-ops/src/pm_events.rs:11-56`, `docs/guide/configuration.md`, "pm", and `docs/guide/events.md`. Resumable wait and opt-in push are existing work, not a proposed gap here. |
| C14 | T-040 landed during research as `bbc3077`; task body and `/home/pawis/miti-workspace/.corral/artifacts/T-040/R1/da3b34d2-cost-report.md` describe reported/API-equivalent costs, cache breakdowns, task/reviewer totals and coverage. Use that work for future measurement; the baseline below reads the PM transcript directly. |
| C15 | T-027 also landed during research as `5f17718d7f4ab83e4863b758f6a87c66e253edb2`. At that commit: `README.md:224-274`, `corral-ops/src/escalation.rs:1-108`, `corral-ops/src/reviewer.rs:225-240`, `corral-ops/src/dispatch.rs:550-650`. Profile escalation chains, cross-vendor reviewer selection and persisted retry choices are now existing work. |

### Event and transcript evidence

| Ref | Exact evidence | What it supports |
|---|---|---|
| E1 | E:27, T-001 changes, `09:17:44.650257916Z`; E:31 approval, `09:18:48.568266426Z` | Rebase-conflict send-back followed by landing was part of the loop from the first task. The message names the new main commit and affected feature paths. |
| E2 | E:222/317/336/348, T-026 changes, `10:06:58.942895584Z`, `10:22:40.799586444Z`, `10:26:31.938975946Z`, `10:30:49.249947045Z`; E:389 approval, `10:37:20.990957501Z` | Three integration-conflict requests, then a distinct 821-SLOC rejection despite clean rebase/green gate. The first request also says to reuse T-028's existing `kind` field rather than add another: that is semantic integration judgment. |
| E3 | E:253, T-017 changes, `10:14:20.492377818Z`; E:308 approval, `10:21:22.614540816Z` | Ten conflicting files, a list of landed tasks, ~775-line board warning, and an added no-review-section regression requirement. Factual context and human judgment coexist in one request. |
| E4 | E:373/390, T-038 changes, `10:34:22.96166221Z` / `10:37:47.5718825Z`; E:406 approval, `10:40:25.13186476Z` | README conflicts and docs-check instructions are mechanical inputs; the Apache-2.0 decision is explicitly the owner's, not something automation should invent. |
| E5 | E:435, T-023 changes, `10:42:53.916221634Z`; E:450 approval, `10:45:01.751492069Z` | Lists four conflicting files, T-018's newly landed UI features, approximate 716/720 SLOC counts, and required UI live checks. Motivates contextual notices and size headroom. |
| E6 | E:238, T-034 approval, `10:11:10.007967577Z`; E:240-246, T-036 created/dispatched/done/approved, ending `10:13:30.028993493Z`; T-036 body/summary | Combined main did not compile its tests because a `Run` literal lacked new usage fields. The fix added `usage: None` and `usage_note: None`. This is evidence for integrated-tree verification, not proof of which worker skipped a check. |
| E7 | E:353-355, T-039 approved then T-040 Backlog -> Ready -> dispatched, `10:31:10.710687054Z` to `10:31:18.196761315Z`; E:392-394, T-027/T-042 dispatch; P:1419 | PM manually released eligible work; the owner explicitly selected limit 7. W confirms 7. No `queued` event occurs in the frozen window. |
| E8 | P:1266/1269/1272, T-026 verification/size check/send-back; P:1438/1441, T-018 verification/approve/check/install; P:1474/1477, T-023 verification/approve/check/install/count slots | Actual command recipe: status, conflict-marker scan, merge-tree, summary `Verified` excerpt, changed Rust sizes, approve, main check, install, slot count. Some commands pipe through grep/tail without pipefail. Exit-code interpretation belongs in code, not output heuristics. |
| E9 | P:1077; W profile-policy comments around lines 54-63; E:309 and E:339 (`opus` UI dispatches), E:355 (`gpt-sol`) | Owner preference is GPT by default, Claude for visual/UI work; classification precedes deterministic routing. |
| E10 | E:53/54, T-014 forced status moves; E:219 T-032 forced resume; T-033 task and C10 | Recovery and readiness were not reducible to "pane idle means done." Preserve explicit escalation and identity-checked recovery. |
| E11 | E:25, T-018 creation; E:26, T-019 idea creation; T-019 body | An existing idea already names auto-dispatch of Ready tasks by priority, digests and revert. Refine its scheduling portion rather than filing the whole idea again. |

## 2. Step-by-step PM loop and quality guardrails

**Covered** means implemented, not enabled in this workspace. **Code gap**
means a deterministic extension. **Judgment** means a bounded review/owner
decision, not a reason to keep shell orchestration in the PM.

| Step | Classification and implementation | Quality floor / minimal judgment |
|---|---|---|
| 1. Notice worker completion or a blocked run | Covered: `done` emits an event; sync reconciles state; T-041 provides wait/push. W did not configure review. [C5,C10,C13; E7,E10] | Consume persisted events with a cursor and reconcile current task/run before acting. Never accept idle as done. Push delivery is a wake hint, not the durable job authority. No LLM needed to notice. |
| 2. Read task, current run, summary and evidence | Covered snapshots; incomplete audit. Join task/run/HEAD/spec/policy identities in code, building on T-037 A/B. [C3,C5,C12; E2,E3,E8] | `Verified:`/`Not verified:` presence can be linted, but claims remain untrusted. Required test receipts and artifacts must match the candidate; missing/unverified acceptance evidence cannot silently pass. Reviewer judges whether evidence actually proves each criterion. |
| 3. Check clean worktree and conflicts/operation state | Clean-tree checks covered by gate/autopilot. Add a preflight for unresolved Git operations and conflict markers. [C2,C4; E1,E8] | Use Git status/operation state and explicit exit codes. Check every changed relevant file, not just the largest three. Handle legitimate marker examples explicitly; a failed Git command is an infrastructure error, never "MERGE_CLEAN." |
| 4. Establish current main and integration viability | Autopilot rebases at landing and aborts conflicts. Add non-mutating merge-tree preflight before spending on review. [C4,C5; E1-E5] | Bind result to `(base SHA, head SHA)`, recheck when either changes, and still run the gate on the actual rebased tree. A clean three-way merge is not proof that each replayed commit rebases cleanly or that behavior composes. |
| 5. Enforce full gate, SLOC and docs | Gate command list already supports arbitrary repository checks. Add the missing repository guard script/receipts, not a new generic command runner. [C1,C2,C11; E2,E4,E6,E8] | Corral executes fmt, clippy and tests itself on the final candidate. Enforce 800 SLOC, record counting method, all changed Rust paths and counts. Docs check uses a binary built from that candidate. Failed commands, skipped required live checks, hangs and unknown results do not pass. |
| 6. Judge diff, scope and acceptance | Covered independent reviewer; genuinely judgment-bearing. [C3; E2's duplicate field, E3's missing default-policy test, E4's docs integration, E5's visual behavior] | One fresh reviewer sees task/spec, candidate diff, touched contracts, gate receipt and criterion evidence. Return `approve`, concrete `changes`, or `escalate`. Claude handles visual judgment. Neither tests nor filename overlap decide UX or semantic compatibility. |
| 7. Prepare a precise change request | Generic gate-tail/reviewer-note/conflict send-back covered. Add an automatically constructed integration packet. [C4; E1-E5] | Code lists all landed commits/tasks since the relevant base, exact conflicts, file sizes, gate failures and required commands. Do not claim every landed task caused a conflict. Reviewer/worker decides how to preserve both behaviors; ask the owner for changed requirements. |
| 8. Send back, retry, cap rounds | Covered same-pane request or retry if pane is gone; `max_rounds`, escalations. T-027 additionally supplies configured profile escalation with a fresh run. [C4,C15; E2,E10] | Persist request identity and bind it to current run/head/base; do not resend on every sync. Keep two rounds initially; T-026 shows this would escalate rather than silently repeat four times. PM decides continuation or changed approach. |
| 9. Land and remove worktree/branch | Covered: autopilot rebase/re-gate/ff; T-035 safe branch deletion. Manual `approve --merge rebase` does not run a gate. [C4,C5; E1,E6,E8] | Before base mutation require exact final-tree receipts, current authorization and unchanged base; serialize project landing. Keep no-push behavior. Cleanup failure is visible and does not justify deleting an unmerged branch. |
| 10. Check main, rebuild/reinstall | Code gap: configurable post-land jobs with durable receipts. [C5,C9; E6,E8] | Run only after successful landing against recorded landed SHA. Distinguish "landed" from "binary installed." On failure preserve the commit, halt dependent operational rollout, attach log and escalate; do not blindly rollback or claim success. |
| 11. Choose and start next eligible task | Queued dispatch, dependencies, slots and routing already covered. Missing is explicit admission policy for unqueued Ready tasks and atomic reservations. [C6-C8; E7,E9,E11] | PM/owner authorizes a ready set; code selects within it. Do not mine arbitrary Backlog/Idea tasks or paused projects. Recheck deps/slots/config atomically, preserve queued overrides, and count NeedsMe. Priority/size/kind policy may be code; ambiguous classification stays PM judgment. |
| 12. Report outcomes and keep watch | Acceptance artifact/stats covered; T-040 accounting and T-041 wake-up cover related work. A concise deterministic digest can use these records. [C9,C13,C14; E7,E8] | Report exact candidate, gate, review, landing, hooks and cleanup independently. PM reads exceptions and periodically samples accepted work, not every raw test log. Never fill unknown cost/evidence with zero/pass. |

### Acceptance hazards to close before unattended landing

These are source-derived limitations, not newly reproduced product failures.
The event examples explain why each deserves attention.

1. **A gate PASS is HEAD-bound but not recipe-bound.** `GateRecord` has no
   command/policy digest, while `autopilot::start` and `gate_at` reuse a PASS
   for the same HEAD. Adding SLOC/docs checks to config need not invalidate
   the old receipt. Bind it to command policy, attempt and relevant
   environment/verification version, not just SHA. [C2,C4; E2,E4]
2. **Automatic rebase refreshes the gate, not the semantic review.**
   `autopilot.rs:428-434` stays in Landing after rebase; it does not re-enter
   Reviewing. Acceptance can retain an older review SHA. Require final
   candidate review after base movement, or a separately justified,
   machine-verifiable reuse rule. Initially use `merge = "ff"` so workers
   integrate and resubmit before review can be accepted. [C3,C4,C9; E2,E6]
3. **Dispatch-base diffs can include other landed work.** Done snapshots
   and owner/diff guards use `run.base_sha`, which is the starting base,
   even after a worker rebases. T-017's context already spans many landed
   features; T-037's `done` artifact recorded +19,466/-619 although its
   deliverable was research. Compute task-attributable review/guard diffs
   against the current integration base; retain the original base only for
   provenance. Do not raise `max_diff_lines` to hide inflated diffs.
   [C4:334-346,C5:107-108; E3; E:456-458 T-037 artifacts/done]
4. **Locks cover records, not the whole operation.** A workspace-wide slot
   reservation and project landing ownership are needed for concurrent
   board, hook, watch and manual invocations. Existing Git `--ff-only`
   prevents a non-fast-forward overwrite, but a racing base move can still
   produce a blocked/retry path. Do not describe current checks as a
   transaction or strict concurrent capacity guarantee. [C6,C8; E7]
5. **The PM's shell shortcuts are not the target specification.** For
   example P:1266 tests grep output from merge-tree and prints
   `MERGE_CLEAN` when grep finds nothing. A Git error without a CONFLICT
   line can therefore look clean. P:1441/1477 filter check/install output;
   sequential `;` commands do not enforce stop-on-failure. New code must
   preserve subprocess statuses and typed outcomes. [E8; C2,C5]

## 3. Ranked gaps and expected return

Ranking is engineering judgment: reliability first, then avoided PM work
and effort. Effort **S** is a bounded script/config/prompt change,
**M** a persisted service plus CLI/tests, **L** a cross-stage contract.
Token ranges below are *assumed PM output tokens avoided*, averaged over
this workload; they overlap and must not be added. They do not measure
worker/reviewer savings or subscription billing.

| Rank | Gap / reuse boundary | Reliability gain | Assumed PM output saved/task | PM load removed | Effort | Evidence |
|---|---|---|---|---|---|---|
| 1 | Final-candidate acceptance preflight and repository guard bundle: recipe-bound checks, SLOC/docs, current-base diff, review freshness. Combine with T-037 A/B, not another audit system. | Very high: removes summary trust and stale-policy/integration gaps | 500-1,000 | Repeating verification recipes and interpreting output | M guard bundle; L combined identity contract | C1-C5,C11,C12; E2,E3,E4,E6,E8 |
| 2 | Merge-tree preflight plus factual integration/change-request packet | High: detect integration work before reviewer spend; fewer incomplete repair instructions | 300-700 | Reconstructing landed tasks, conflict paths and size hints | M | C4,C5; E1-E5 |
| 3 | Authorized Ready admission and atomic dispatch reservations; extend existing queued drain/T-019 | High under concurrent sync; fewer empty-slot gaps and lost overrides | 200-500 | Slot counting, dependency lookup, repeated dispatch commands | M | C6-C8; E7,E9,E11 |
| 4 | Durable post-land check/install hooks with per-project serialization | High operational reliability; makes stale binary/failing install visible | 200-500 | Main checks, install commands, success reporting | M | C5,C9; E6,E8 |
| 5 | Deduplicated "main moved" overlap notices to active tasks | Medium; earlier context, not a guarantee of fewer conflicts | 50-200 | Ad hoc rebase warnings and size-headroom notes | M | C4,C6,C8,C13; E2,E3,E5 |
| 6 | Task-kind reviewer policy layered on T-027's existing fixed/cross-vendor selection, with explicit override | Medium; preserves Claude visual policy without using Claude for every backend review | 50-150 | Manually assigning the visual reviewer | S/M | C3,C7,C15; E5,E9 |
| 7 | Deterministic acceptance/operations digest | Medium visibility, little direct acceptance benefit | 100-300 | Rewriting landed/blocked/next summaries | S | C9,C13; E7,E8,E11 |
| 8 | Workspace activation and staged rollout, not new product machinery | High immediate leverage; incomplete until guard gaps close | Part of ranks 1-4 | Running existing capabilities by hand | S | W; C1,C4,C6; E3,E7 |

**Do not file:** another gate runner, reviewer protocol, retry loop,
merged-branch cleanup, generic scheduler, event-polling agent, watchdog or
cost collector. These already exist or landed as T-027/T-040/T-041. An
unconditional Backlog auto-dispatch switch is not recommended; `Ready`
must represent an authorized, sufficiently specified unit of work.
[C1-C15; E1-E11]

### PM tokens today versus after

**Measured, fixed-window event counts:** 30 approvals, 52 done handoffs
across 32 tasks, 19 change requests across 14 tasks (18 mention conflicts),
37 dispatches and 7 moved events. There are no gate, reviewer-verdict,
autopilot or queued events in this window. This is consistent with W's
manual review setup, not evidence that those features were absent.

**Measured transcript accounting:** 398 assistant messages have usage
records. Summing each message once gives:

| Scope | Input | Cache write | Cache read | Output | Total tokens | Recorded cost field |
|---|---:|---:|---:|---:|---:|---:|
| Entire PM session through cutoff | 936 | 785,096 | 86,910,373 | 285,724 | 87,982,129 | $27.03 |
| 92 messages containing core loop commands | 218 | 194,735 | 22,698,929 | 66,295 | 22,960,177 | $6.84 |
| Loop subset / 30 approvals | 7 | 6,491 | 756,631 | 2,210 | 765,339 | $0.228 |
| Whole session / 30 approvals | 31 | 26,170 | 2,897,012 | 9,524 | 2,932,738 | $0.901 |

These are token traffic, not unique text tokens. Cache reads dominate
because a long session is re-fed repeatedly. The tiny `input` field is
not the whole prompt: cache-write/read fields must be included. Cost is
the transcript's recorded number, not verified subscription charges.

**Selection rule:** keep assistant messages with an `eval`/`bash` tool
call whose `code` or `command` matches:

```text
merge-tree|\bapprove\s+T-|\bapprove\s+\$|\bapprove\s+\{|
\bchanges\s+T-|\bchanges\s+\{|cargo install|
cargo check --(?:workspace --)?all-targets
```

Exclude the call titled "Filing the spike". Count a message once even
when it contains multiple matching calls. Sum `message.usage` fields,
not tool-result echoes or context-usage custom entries. The attached
`t045-measurements.json` contains the selected JSONL line numbers, source
paths, cutoff and totals for reproduction.

This subset is an attribution proxy, not a clean lower bound: mixed
messages include planning, while dispatch-only calls, reports, result
interpretation and polling can fall outside it. The whole-session figure
includes task creation, user discussion, experiments, unrelated questions
and unfinished tasks. Neither should be sold as a precise cost per task.
There are 42 command-bearing tool calls containing `merge-tree`, 25
containing `cargo install`, and 18 containing `cargo check`; these are
call-text occurrences, not verified successful executions.

**Forecast, not measured:** after ranks 1-4 and existing review automation
are deployed, routine tasks need no PM shell loop. Budget 200-700 PM
output tokens and 20,000-100,000 processed tokens per accepted task,
amortizing compact digests and exceptions. Against the selected-loop
proxy that is approximately 68-91% less PM output and 87-97% less PM
token traffic. It does not eliminate planning or owner conversations.

Assumptions: 10-20% of tasks still escalate; one escalation costs
1,000-3,000 PM output tokens and one to three short-context exchanges;
routine digest share is 100-200 output tokens/task. Persistent full-history
PM wakes would erase much of the processed-token saving. The observed
14/32 tasks receiving changes warns that an early rollout may have a
40-45% intervention rate, yielding roughly 500-1,500 PM output tokens/task
instead. Do not claim the optimistic range on day one.

The independent reviewer still costs tokens: assume 20,000-80,000 input
and 1,000-4,000 output tokens per reviewed candidate, with additional
review after meaningful integration changes. This is work shifted to a
bounded review context, not magically removed. Total LLM cost can rise on
small tasks while quality improves. Worker repair savings are unmeasured.
Use T-040 with worker, reviewer and PM attribution separated; measure
first-round acceptance, post-land regressions, owner reversals, missing
proof and queue/repair latency alongside spend. Compare similar task
kind/size cohorts and disclose unknown usage. [C9,C14; E2,E6,E7,E8]

## 4. Recommended configuration and rollout

### Supported now: project-scoped, owner-accepted pilot

This is a merge fragment for W, not a complete replacement config. Retain
its project/profile definitions and edit existing tables rather than add
duplicate `[defaults]`, `[limits]` or `[pm]` headers. Nothing in this
report changes W.

Both TOML examples below were parsed and applied through `corral config set`
to an isolated, task-free fixture under `.omo/evidence/config-check`;
`corral config validate` passed. Existing tables must be edited through
their leaf keys: an initial attempt to replace the whole `limits` table
was refused, then corrected to `limits.running`. This checked the actual
configuration schema, not model availability or future workflow behavior.

```toml
[defaults]
profile = "gpt-sol"

[limits]
running = 7

[review]
autopilot = true
accept = "owner"
gate = []
reviewer = "none"

[projects.corral.review]
gate = [
  "cargo fmt --all -- --check </dev/null",
  "cargo clippy --workspace --all-targets -- -D warnings </dev/null",
  "CORRAL_HERDR_SESSION=corral-test cargo test --workspace </dev/null",
  "cargo build -p corral --target-dir /home/pawis/miti-workspace/corral/target </dev/null && scripts/check-docs.sh /home/pawis/miti-workspace/corral/target/debug/corral </dev/null",
]
reviewer = "gpt-astra"
accept = "owner"
merge = "ff"
max_rounds = 2
max_diff_lines = 1500
remove_worktree = true
owner_paths = [
  "AGENTS.md", "**/AGENTS.md", ".cargo/**", ".github/**",
  "scripts/**", "crates/corral-ops/assets/**",
  "Cargo.toml", "Cargo.lock", "LICENSE*",
]

[[routing.rules]]
name = "corral visual work"
project = "corral"
kind = "ui"
profile = "opus"

[[routing.rules]]
name = "corral hard logic"
project = "corral"
kind = "logic"
size = ["l", "xl"]
profile = "gpt-astra"

[[routing.rules]]
name = "corral small mechanical work"
project = "corral"
kind = ["test", "docs"]
size = ["xs", "s"]
profile = "gpt-sol-fast"
```

Rationale: keep the owner's seven task slots; scope Rust commands to corral,
not sibling or paused projects; preserve the full gate; run docs checks
against this candidate's binary; protect automation and policy changes;
use existing profiles and ordered routing. The 1,500 diff-line guard is
**not** the 800 SLOC/file rule. Configure the latter through the repository
guard bundle in specification A, once implemented. Add no fake
`sloc_limit`, `auto_dispatch` or `post_land` keys to today's config.
[C1,C6,C7,C11; W; E2,E3,E4,E7,E9]

Operational details:

- Keep one sync driver alive (board, hook or `corral sync --watch`).
  A configuration file alone does not execute the loop. Use queued dispatch
  for explicitly authorized tasks: `corral dispatch ID --queue` starts
  immediately if eligible, otherwise queues. It is not an enqueue-only
  command. Today ordinary queued dispatch does not retain all
  `DispatchOpts` overrides; use persisted task profiles for queued
  exceptions until specification C. T-027 now preserves retry choices;
  reuse that path rather than replacing it.
  `autopilot off` stops review advancement, **not** `start_queued`.
  [C4,C6; E7]
- Fixed project reviewer `gpt-astra` reduces PM load for backend work.
  Worker routing does not route reviewers. For UI tasks keep owner
  acceptance and request a fresh `review-run --reviewer opus` for visual
  evidence; a preliminary GPT review cannot satisfy Claude visual review
  policy. This extra review is a known pilot cost. Ship reviewer routing
  before promising unattended UI acceptance. Task `review: owner` is an
  acceptance override, not a reviewer override. T-027's new
  `reviewer = "cross-vendor"` selects the opposite implementer vendor,
  not a UI specialist: an opus-implemented UI task would get GPT review.
  It therefore does not alone satisfy this visual policy. Keep the fixed
  pilot reviewer and the UI exception; do not file cross-vendor review
  or profile escalation anew. [C1,C3,C7,C15; E5,E9]
- Validate with `corral config validate` before activation and use
  `corral route ID`/dispatch `--explain` on representative tasks. The
  owner/PM still supplies kind/size and explicit exceptions; no regex can
  infer every UI change. [C7; E9]
- A successful gate must include required isolated live checks, not merely
  tests that returned early because `corral-test` was absent. Probe the
  isolated server and inspect receipts. General unit tests and visual
  acceptance are different evidence. [C2,C3,C10,C11; E5,E8]
- For pilot owner approval, verify current main ancestry and use
  `approve --merge ff`, not manual `--merge rebase`, which bypasses
  autopilot's post-rebase gate. If main moved, worker rebases, re-gates and
  resubmits for review. Do not interpret `Reviewed` as permission to land
  a different tree. [C4,C5; E1,E6]

T-041 is now landed, so the existing `[pm]` table may also be amended:

```toml
[pm]
agent = "haiku"
push_events = true
push_kinds = ["needs_me", "stuck", "blocked", "escalated"]
```

Use these exception kinds once review is automatic; waking the long-lived
PM for every ordinary `done` defeats the purpose. During the pilot add
`done` if the PM still performs each acceptance. Initialize the delivery
cursor with a sync while push is disabled before enabling it, to avoid
replaying retained history on first use. Push skips events when disabled
or no PM is live; reconcile state after reconnect. The cursor save follows
the send, so a crash between them can duplicate a notification: handlers
must be idempotent. Use `wait` with its cursor for explicit subscriptions,
not a second PM polling loop. [C13; E7,E10]

### Target after the gaps ship

Keep the same policy surface, add A's guard command to `gate`, and switch
only `[projects.corral.review].accept` to `"auto"` for the authorized
cohort. Retain `"ff"` as the conservative default; allow `"rebase"` only
after final-candidate review freshness is enforced. Configure C's proposed
Ready admission, D's proposed hooks and E's notices through the interfaces
those tasks deliver; they are not supported TOML today. Keep owner
overrides for sensitive or ambiguous tasks. [C1,C4-C8; E1-E9]

The PM then does five things: define/approve specs and dependencies;
classify and prioritize the authorized Ready set; decide semantic
integration or owner exceptions; route visual review until reviewer
routing ships; audit outcome samples and adjust policy based on measured
regressions and cost. It does not recalculate slots, inspect every gate
tail, recreate landed-task lists, delete merged branches, or reinstall
the binary by hand. [C3,C6-C14; E2-E10]

## 5. Ready-to-file specifications

These are local specification labels, not allocated board IDs. The PM
files/refines them. No tasks were created by this spike. Every specification
uses corral's CLI/store/events as the sole write path, preserves the
800-SLOC rule, requires deterministic tests plus the full repository gate,
and confines live panes to `corral-test`.

### A. Bind acceptance to the final candidate and repository guard policy

**Goal:** replace the PM's repeated verify recipe with machine-run proof
without accepting stale checks, inflated diffs or unsupported criteria.
Combine this with T-037's completion-audit/evidence specifications A/B.
[C1-C5,C8,C11,C12; E2,E3,E4,E6,E8]

**Design**

- Extend the existing gate/acceptance records, not a parallel ledger.
  Identify run, attempt, candidate HEAD/tree, integration base, task/spec
  revision and effective command/guard policy digest. Keep dispatch base
  separately for history. Use the current integration base for task diff,
  `owner_paths` and `max_diff_lines`.
- Add a repository-owned non-mutating guard command callable through
  today's `gate` array. Check clean candidate, unresolved Git operations,
  conflict markers and every changed Rust file's 800-SLOC ceiling.
  Define the counting algorithm and fixtures explicitly, including block
  comments, strings, blank lines, renamed/deleted files and tests.
  Report all violations; headroom below 800 is advisory, not a new rule.
- Run the full Rust gate and docs check against the candidate. Bind
  per-command exit/log receipts to the same attempt. Make required live
  evidence and summary-section presence visible, but never promote prose
  claims into machine PASS. Reuse T-037's criterion ledger for semantic
  obligations.
- Invalidate PASS when policy, candidate or relevant verification inputs
  change. Rebase triggers the gate and renewed semantic review of the
  integrated candidate; do not preserve an old review merely because a
  rebase was conflict-free.
- Serialize final project landing and recheck identities/base before
  fast-forward. Record a stale/retry outcome on base movement. Owner
  approval can remain an explicit override but cannot be misreported as
  the configured autonomous gate having passed.

**Acceptance**

- A candidate exceeding 800 SLOC is rejected even when cargo tests pass;
  all offending paths/counts appear in its receipt. Exactly 800 passes.
  Marker/operation and docs failures have distinct reasons.
- A same-HEAD gate-policy edit invalidates the previous PASS. A new
  candidate, edited task criterion or stale result file cannot satisfy a
  new attempt.
- Rebase onto a changed main causes re-gating and review on the final
  candidate. A deterministic base-move barrier before landing prevents
  stale acceptance; owner mode never auto-lands.
- A long-running task rebased over unrelated landed tasks shows only its
  attributable diff to the reviewer/owner guards, while preserving
  dispatch provenance. Sensitive paths are not hidden by renames.
- A generic fixture demonstrates gate -> review -> land and exact receipt
  identities in `corral-test`; missing required live evidence escalates.
  Attach before/after failure evidence and the full gate.

### B. Generate integration preflight and precise repair packets

**Goal:** eliminate PM reconstruction of branch drift and conflict context
before review or send-back. [C4,C5,C8; E1-E5]

**Design**

- Add a typed `corral-git` merge-tree preflight on explicit base/head SHAs
  before the expensive gate/reviewer stages. Distinguish clean, conflict
  and infrastructure error by status and structured output; never parse
  only the presence of the word CONFLICT.
- Build a packet with dispatch base, merge base, candidate/main SHAs,
  every commit newly reachable on base since the relevant branch base,
  task IDs/titles joined through recorded approved SHAs, conflict paths,
  changed-path overlaps, file SLOC counts and applicable gate commands.
  Include all landed tasks in the full artifact, with a compact message
  and no silent truncation.
- Preserve commits without a board task as unattributed commits; never
  infer a task ID from an unreliable subject. Keep dispatch-history drift
  distinct from the commits still missing from the worker branch.
- Reuse autopilot `request_changes`, round limits and retry-carried text.
  Key packets by task/run/head/base/reason to avoid duplicate send-backs.
  Non-conflict failures escalate instead of instructing a meaningless
  rebase. A later real rebase remains authoritative.
- Make semantic advice a separately attributed reviewer note. Code may
  say "these features landed"; it may not decide that one field replaces
  another or that a license changed.

**Acceptance**

- Fixtures with several landed tasks, an untracked-by-board commit,
  deleted task branches and multiple conflict files produce complete,
  stable packets with exact identities.
- A clean merge-tree, merge conflict and Git failure have different
  outcomes. The preflight leaves branch/index/worktree untouched.
- Repeated syncs at identical SHAs produce one request; main movement
  invalidates the packet and updates context without overwriting a newer
  run. A merge-clean but rebase-conflicting history still fails safely.
- Reproduce the shape of T-026's three conflict rounds and T-023's size
  hints in deterministic fixtures. A reviewer is not launched before a
  known conflict is returned. Full gate and isolated CLI evidence pass.

### C. Admit authorized Ready work and reserve dispatch slots atomically

**Goal:** let the owner/PM authorize a queue once and let corral fill free
slots reliably. Refine T-019's scheduling part; keep digests/revert separate.
[C6-C8; E7,E9,E11]

**Design**

- Extend `start_queued` and `dispatch`; do not add another scheduler.
  Add explicit project-scoped opt-in admission of Ready tasks plus an
  enqueue-only/pause/dequeue surface. Default remains manual authorization.
  Never auto-promote arbitrary Idea/Backlog tasks or include unrelated
  configured projects.
- Extend T-027's persisted retry plan to ordinary queued dispatch intent,
  including explicit profile/model/effort/base overrides; explain whether
  routing is pinned or reevaluated at claim.
  Reuse T-026 precedence and exact-model validation. Persist route reasons.
- Atomically claim a task and reserve global/per-project capacity before
  external worktree/pane creation. Compare current run/status/dependencies
  under the claim. Recover a crashed/failed launch without duplicate
  sessions or leaked reservations.
- Order by documented priority, then creation time and ID. Count
  Running/NeedsMe and reservations. A per-project cap must not starve other
  projects. Reject cycles/missing dependencies visibly, not as infinite
  unexplained waiting.
- Separate dispatch pause from the review-autopilot kill switch. Show
  why each candidate waits. Gate/reviewer process concurrency is separate
  from coding-task slots; expose a bounded review budget rather than
  claiming the current seven slots cap every agent/process.

**Acceptance**

- A dependency approval frees an eligible queued task; a merely Review
  dependency does not. All current manual dispatch paths share the same
  reservations.
- Barrier-synchronized competing sync/manual dispatches with one slot
  launch exactly one task. Same-task races cannot create two runs/panes.
  Crash/failure at each launch boundary recovers once.
- UI -> opus and backend -> gpt default routing work; queued explicit
  overrides survive reload/restart. No missing model is silently replaced.
- Pause, dequeue, priority ties, per-project saturation, NeedsMe,
  cycles and absent dependencies have deterministic tests and explain
  output. No unapproved Backlog/Idea task launches.
- An isolated generic fixture fills/refills slots and records claims,
  releases and dispatches; full gate passes.

### D. Run durable post-land checks and install hooks

**Goal:** replace manual main check/reinstall with inspectable, restart-safe
operations without confusing a landed commit with a deployed binary.
[C5,C8,C9; E6,E8]

**Design**

- Add opt-in per-project named hooks through corral-ops/config, triggered
  only after successful base advancement. Initial corral recipe:
  `cargo check --workspace --all-targets`, then
  `cargo install --locked --path crates/corral`, then a binary smoke check.
  Use stdin null, explicit cwd/env/timeouts and full exit/log capture.
- Persist pending/running/succeeded/failed/uncertain receipts keyed by
  project, landed SHA, hook name and policy digest. Serialize against
  project landing so a hook is not accidentally checking a newer main.
  Define any later coalescing policy explicitly; default is no coalescing.
- Landed remains true if a hook fails. Mark operational readiness failed,
  pause dependent operational dispatch, notify once and expose a bounded
  retry. Retain the prior usable binary where feasible; no force-reset,
  automatic revert, push or destructive cleanup.
- A restarted process must reconcile the hook receipt and observed effect.
  For an install interrupted after its side effect but before receipt,
  prove installed identity or mark uncertain; do not promise universal
  exactly-once shell side effects. Install should be designed idempotently.
- Include receipts in the acceptance/operations artifact and digest.
  A post-land check supplements A's pre-land gate; it cannot make an
  already broken main acceptable.

**Acceptance**

- Clean landing runs check -> install -> smoke once in order; no hook
  runs for keep-branch approval, failed merge or owner-wait state.
- A failing check prevents install; failing install preserves landed
  status and exposes exact failing command/log plus operational blocker.
- Concurrent lands and process restart preserve SHA attribution, do not
  duplicate successful receipts, and surface uncertain side effects.
- Live QA uses a fixture binary/install prefix inside the isolated test
  workspace, never the user's installed corral. Evidence includes final
  executable identity, hook receipts and full gate.

### E. Notify active tasks about relevant main movement

**Goal:** move factual integration warnings earlier without interrupting
unrelated work or automatically rewriting an agent's branch.
[C4,C6,C8,C13; E2,E3,E5]

**Design**

- On each recorded landing, compare newly landed changed paths with
  active task changes and optional declared write scopes. Use rename-aware
  paths; include observed uncommitted paths without reading arbitrary
  unbounded files. No overlap is not proof of semantic independence.
- Persist one notice per active run and base-movement batch. Attach new
  task/commit IDs, overlapping paths, size headroom from A, and B's context
  packet. Coalesce a burst of landings rather than send one interruption
  per file.
- Use existing reply delivery semantics, recording pending/failed delivery.
  A worker stuck or awaiting the owner must not lose its pending answer.
  Reconcile current run/pane identity before sending; never send to a
  replacement or unrelated pane.
- The notice is informational. No automatic rebase, acceptance, status
  reset or round consumption. Worker decides when to integrate; reviewer
  checks semantic composition; owner decides conflicting requirements.

**Acceptance**

- Overlap sends one contextual notice; disjoint changes send none.
  Renames, declared scope, uncommitted paths and multiple fast landings
  have deterministic tests.
- Restart, duplicate sync and a task finishing during notice construction
  do not duplicate or misroute the notice.
- A missing/stuck pane produces a visible deferred receipt without
  destroying pending replies. A notice never increments change rounds
  or mutates Git state.
- An isolated worker receives a factual main-movement message, and its
  branch remains unchanged. Full gate and delivery receipts pass.

## 6. Scope closure

This report extends T-037's audit/evidence work with concrete integration
and operations requirements; it does not re-specify its portable progress,
plan approval or general blocker system. T-027/T-040/T-041 are dependencies to
reuse, not new task proposals. No new board tasks, production edits,
workspace policy changes or live agent dispatches were made by T-045.

No empirical claim is made that the proposed checks eliminate semantic
regressions or that the forecast savings are achieved. The quality target
is stronger mechanical proof plus retained independent judgment, measured
by regressions and unsupported approvals as well as cost. Verification
receipts for this documentation-only branch are attached to T-045; they
prove repository health, not the unimplemented proposals.
