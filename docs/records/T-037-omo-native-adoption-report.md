# T-037: What corral should adopt from OmO

Research date: 2026-10-15. Completed local-source research spike.

## Decision

Corral should own durable, inspectable run obligations and their enforcement:
what must be proved, which evidence belongs to the current revision, why a run
is waiting, and what happens next. Harnesses should own reasoning, context
compaction, tool execution, and in-session subagents. Adopt OmO's useful
invariants, not its entire prompt or a second orchestration runtime.

The strongest immediate opportunities are completion audits, criterion-linked
evidence, explicit waits/blockers, resumable checkpoints, and selective plan
approval. Independent review, gates, skill installation, dependency dispatch,
and outcome accounting already have corral foundations; extend those rather
than implement competing paths.

## Scope and provenance

This is a read-only source and historical-evidence spike. No product code or
new board tasks are part of the deliverable. Proposed CLI spellings and data
fields below are designs, not commands available today.

Source roots:

- **P** = `/home/pawis/.bun/install/global/node_modules/omo-ai/plugin/`
  (installed package `@code-yeongyu/omo-senpi` 5.1.33, `package.json:2-3`).
- **S** = `/home/pawis/.bun/install/global/node_modules/omo-ai/node_modules/@code-yeongyu/senpi/dist/`
  (parent `package.json:2-3`: senpi 2026.10.13).
- **C** = `/home/pawis/miti-workspace/corral/` (repository-relative anchors). Initial T-037 checkout: `1c9b229`.
  T-017 landed as `641612a` while this research was running and was read from
  the main checkout. Snapshot distinctions matter: absence from the initial
  worktree is not evidence that a feature is unbuilt.
- **A** = `/home/pawis/miti-workspace/.corral/artifacts/`.
- Task filenames refer to `/home/pawis/miti-workspace/.corral/tasks/`.
- **E** = `/home/pawis/miti-workspace/.corral/events.jsonl`.
  Event timestamps below are UTC on October 15, 2026.

Path prefixes are literal substitutions, so every source below resolves to a
local file. Artifact names include their content-hash prefix. Historical
artifact reports are evidence of what their authors recorded; they are not
fresh executions by this spike. Where a report includes actual CLI output,
that output supports the narrower demonstrated behavior. No controlled
comparison establishes that one model or harness is generally better.

## Current corral foundations

| Foundation | Actual coverage and remaining boundary |
|---|---|
| T-032 run contract | `C/crates/corral-ops/src/prompt.rs:17-123` renders a per-harness override, then a workspace override, then `assets/run.md`. The contract requests a gate, artifacts, rebase/re-gate, and Verified/Not verified summary. Those are instructions, not completion preconditions. |
| Done and artifact snapshots | `C/crates/corral-ops/src/review.rs:90-175` commits outstanding changes, snapshots diff/summary, scans artifacts, and moves Running/NeedsMe to Review. It does not audit acceptance criteria or gate/base freshness. `artifacts.rs:43-115` hashes and snapshots bytes per run; an artifact hash proves file identity, not that its assertions are true. |
| T-015 gate | `C/crates/corral-ops/src/gate.rs:146-305` requires a clean worktree, records HEAD, executes configured commands, collects a machine-readable result, and attaches its log. `C/crates/corral-core/src/model.rs:139-162` provides SHA-bound gate validity. A hand-attached log does not automatically become a `GateRecord`. |
| T-016 reviewer | `C/crates/corral-ops/src/reviewer.rs:88-141,209-299` supplies task/spec, summary, gate, diff command, artifacts, project rules, and verdict command; `assets/review.md` requires independent, read-only review. Existing stale-head and dirty-worktree handling should remain the authority. |
| T-017 autopilot | Landed `641612a`: `C/crates/corral-ops/src/autopilot.rs:179-499` implements gate/review/accept, owner guards, rebase, and post-rebase re-gating. Do not propose this pipeline anew. Completion intake and richer evidence are the missing inputs. |
| Liveness and write safety | `C/crates/corral-ops/src/sync.rs:37-206` has idle debounce, OmO session activity, and conditional writes; T-014/T-025 distinguish idle from completion and protect newer task changes. T-033 is existing watchdog work, not a blank-slate feature request. |
| Dependencies and limits | `C/crates/corral-ops/src/dispatch.rs:37-95` checks dependencies and slots; `sync.rs:233-283` starts queued eligible tasks. This is already a task-level DAG executor in the useful basic sense, not an intra-session workpool. |
| Skills and PM | `C/crates/corral-ops/src/skill.rs:9-14,80-112` installs the managed corral skill for known harness directories and preserves existing files. `pm.rs:92-123` gives the PM decomposition/filing rules. Generic harnesses can consume the dispatch contract without a skill engine. |
| T-028/T-034 stats | `C/crates/corral-ops/src/stats.rs:16-41,75` and `usage.rs:48,79,169` aggregate outcomes and best-effort transcript usage. Missing usage is not zero. Use these for later rollout measurement, not speculative model rankings. |
| T-026 routing | Existing task `T-026-model-routing-rules-pick-a-profile-per-task-with.md` and its rebase report define rule precedence, exact model availability, and explainability. Its integration was still moving during discovery; future task-group/skill work must reuse its profile resolution. |

`docs/DESIGN.md` is useful product intent, but its opening "nothing is built
yet" is stale relative to these implementations. The report uses code and
dated task records to establish coverage.

## What today's runs actually demonstrate

| ID | Evidence | Observation and limit |
|---|---|---|
| E1 | `A/T-032/R1/77b5649a-live-omp-sol.md` | A real omp fixture run dispatched in `corral-test` attached exact-content evidence and a gate log, then reached Review with Verified/Not verified lines. This supports a portable contract; it does not prove long-run parity or every harness's compliance. |
| E2 | `E`, T-011 `done` 10:00:16.032512521Z and `changes_requested` 10:00:55.514652448Z; T-026 10:06:06.839957648Z / 10:06:58.942895584Z; T-017 10:13:59.035035312Z / 10:14:20.492377818Z | Done handoffs were followed by rebase-conflict repair requests. A completion audit would expose stale integration earlier. Main can move after a valid check, so these pairs do not prove that workers never rebased. |
| E3 | `E`, T-011 `replied` 09:56:21.443622544Z; task `T-033-stuck-run-watchdog-detect-agents-wedged-on-a-ter.md`, Problem | The PM reported a stuck eval despite finished logs, including a failing drag-dimming test. T-033 documents a fileless `grep -cvE` consuming terminal input for 12+ minutes. The exact process cause is the recorded incident diagnosis, not a reproduced trace here. |
| E4 | `A/T-014/R1/812fa8d6-omo-blocked-probe.md` | A real OmO question screen coexisted with herdr `idle`. Idle, waiting for a user, and a stuck command need distinct states. No universal idle timeout can safely infer all three. |
| E5 | `A/T-016/R1/ec18a294-t016-live.log`; `A/T-017/R1/e6c5a922-report.md`; `A/T-017/R1/2f5afa9a-e2e.log` | A generic fake reviewer posted a SHA-bound verdict; autopilot's recorded isolated scenario advanced through gating/reviewing/landing/done. This supports the architecture, not the judgment quality of a real reviewer model. |
| E6 | `A/T-025/R1/87f6d8d0-report.md`; `E`, T-014 done 09:29:31.639794835Z | A stale sync decision overwrote a newer done transition. T-025's conditional update fixed that race. New checkpoints, evidence, and audit writes must check current run/revision under the store lock. |
| E7 | Task `T-036-hotfix-main-tests-do-not-compile-run-usage-field.md`; `A/T-036/R1/c231e5bc-final.diff` and `37fbb472-T-036-gate.log` | `cargo check --workspace --all-targets` caught E0063 after T-034's Run fields met T-032's fixture. Combined-tree verification matters even when individual branches had passed. |
| E8 | `A/T-029/R1/67755f8e-T-029-verification.md` | The report records a deliberate regression: restoring wall-clock time made `board_ages_follow_the_injected_clock` fail, then restoring injected time passed. Favor deterministic behavioral checks, not sleeps or prose assertions. |
| E9 | `A/T-033/R1/a8810c9d-verification.md` | Terminal-grid QA found a hidden stuck badge at 120 columns, but PNG capture failed because the helper lacked `puppeteer-core`. The report explicitly does not claim pixel/color QA. Record evidence capability and missing proof, not a blanket "UI verified." |
| E10 | `A/T-034/R1/1989e984-corral-T-034-report.md`; T-028/T-034 task records | Outcome/token accounting exists; some historical profiles have unsupported-harness usage notes. Any before/after adoption study must report coverage and compare similar work. The task's "~3x" desk-test remark is not a measured conclusion of this spike. |

The events corroborate chronology, not intent. These samples justify controls
against repeated failure modes; they do not estimate failure rates.

## Mechanism inventory and disposition

"Covered" means reuse an existing corral feature. "Adopt" names a native delta.
"Adapter" means optional harness-specific assistance behind a common contract.
"Skip" means leave the mechanism in the harness or keep it opt-in, not that its
underlying idea has no value.

| OmO/senpi mechanism and source | What it really supplies | Corral disposition and integration |
|---|---|---|
| Intent gate and stop condition: `P/skills/ultrawork/SKILL.md:27-62`; `S/core/extensions/builtin/goal/prompt.js:11-38` | Instructions to name the outcome, scope, evidence, and stopping point. These do not execute an acceptance test. | **Covered + adopt, run-contract/PM skill:** T-032's `assets/run.md` and `pm.rs` should carry concise criteria and non-goals. The board task remains authority; no second mandatory goal tool. |
| Decision-complete plans: `P/skills/ulw-plan/SKILL.md:24-30,106-126` | A durable plan, discovery before decisions, reviewer policy, and explicit separation of planning from execution. | **Adopt, CLI/data + reviewer:** optional approved plan revision attached to the task; dispatch checks it only under an enabled plan policy. Existing `ArtifactKind::Plan` stores bytes, not plan approval. |
| Plan-review tool gate: `P/extensions/omo-task.js:2`, identifiers `Aa`, `Zv`/`Yv` | Plugin gate on explicit ulw-plan invocation, a touched plan file, and no ulw-execute; canonical one-shot reviewer assignment. It is not a corral authorization record. | **Adapter/skip internals:** reuse the independent reviewer concept through T-016; do not infer authorization from a skill name or touched file. |
| Execution ledger / DoneClaim: `P/skills/ulw-execute/SKILL.md:166-207` | Attempt-bound commands, artifacts, adversarial checks and cleanup; independent verification before FullyDone is specified in prose. | **Adopt, CLI/data:** criterion-linked evidence on the current corral run, backed by existing snapshots and events. |
| Loop evidence/checkpoints: `P/skills/ulw-loop/SKILL.md:44-64,100-102`; `P/runtime/agent-toolkit-sdk/sdk.js:2`, `yt`, `wi`, `qt` | SDK checks essential criterion status, quality-gate fields, and artifact presence/current-attempt placement. Its driver goal is advisory. Source-path checks are not proof of a claimed behavior. | **Adopt concepts, skip parallel store:** task/run/attempt identities, evidence freshness, and checkpoint validation belong in corral-core/ops. Do not require the OmO SDK. |
| Goal completion audit: `S/core/extensions/builtin/goal/tool-registration.js:46-65`, `todo-gate.js:9-24`, `prompt.js:31-38` | Code rejects open todos; semantic mapping of requirements to evidence is an instruction to the model. | **Adopt, done intake:** deterministic completion preflight plus reviewer judgment. Do not use a harness goal's complete flag as acceptance. |
| Blocked audit: same `tool-registration.js:39-65`; `goal/store.js:63-129`; `goal/transitions.js:22-44` | The inspected built-in path enforces a nonblank reason and legal status transition. The advertised live-channel veto and three-turn floor were not found on that path. | **Adopt, CLI/data + sync:** explicit reason, owner, awaited condition and live wait reference. A wait is not an owner decision; do not copy an unverified "three turns" guarantee. |
| Phased todos and handoff blocks: `S/core/extensions/builtin/todotools/todo-operations.js:96-137`, `todo-query.js:16-29`, `todo-format.js:31-54` | Session-persisted progress and generated handoff cues. Normalization preserves an existing active item; prose does not fully describe actual cursor behavior. | **Adopt, CLI/data + card:** stable step IDs, phases, next action and checkpoint artifact. Keep text-keyed tool semantics and prose formatting in adapters. |
| Context-limit handoff: `S/core/extensions/builtin/compaction/task-intent.js:1-55`, `context-reduction-handoff.js:1-36`; `core/agent-session.js:5924-5965,6144-6200` | Context reduction, revision checks, pending-message preservation and bounded task-intent inheritance (4,096 UTF-8 bytes). | **Adopt checkpoint; skip compactor:** inject a small canonical resume packet via `dispatch.rs`/`prompt.rs`. Summaries, token windows, provider calls and context mutation remain harness-owned. |
| Durable waits/background monitors: `S/core/extensions/builtin/terminal/restore.js:59-93`, `durable-command.js:88-150`, `durable-file.js:43-109` | Persisted subscriptions, restore/lost/expired classifications, re-spawned commands and file checkpoints. Old command output is not replayed. | **Adopt, sync/autopilot + adapters:** durable wait intent and result receipts for corral-owned jobs; adapters report harness-owned handles. Reuse T-015 result files and extend T-033, not a new terminal runtime. |
| Wake delivery/backpressure: `S/core/extensions/builtin/terminal/notify.js:9-26`, `monitor-notify.js:140-215`; `goal/monitor-continuation.js:193-208,284-324` | Delivery mode checks, noisy-monitor muting, and parked-wait recovery. Notification is not equivalent to work completion. | **Adapter + native reconciliation:** record delivery/acknowledgment uncertainty; reconcile idempotently. Do not promise that every harness can be awakened identically. |
| Task/workpool delegation: `P/extensions/omo-task.js:2`, `RY`, `SQ`/`IQ`, keyed pool state | Session ownership, epochs, nonblocking result reads, keyed work, closed-pool checks and delivery generations. | **Covered + adopt task-level concepts:** existing corral runs/dependencies/slots own dispatch; add attempt/idempotency and scope metadata where useful. In-session children, eval handles and model context stay harness-owned. |
| mass-ulw: `P/skills/mass-ulw/SKILL.md:11-27,69-89`; `P/runtime/dag/sdk.js:34-59` | A phase's dependency graph executed through native workflow tools; result handoffs and definition reuse. Requires the host tool proxy. | **Covered + adopt, PM/data:** explicit task groups over `depends_on`, acceptance barriers and write scopes. Optional OmO executor adapter only; never schedule the same dependency in both systems. |
| dag-library: `P/skills/dag-library/SKILL.md:14-51`; `P/runtime/dag/library.js:34-80,106-122` | Reusable definitions, substitution, name validation and generated run keys. | **Defer native templates:** PM can instantiate reviewable task specs; distinct logical-run keys must distinguish restart from new execution. A timestamp-generated key alone is unsafe for retry deduplication. |
| Independent gate review: `P/skills/review-work/SKILL.md:83-109` | Actual-surface QA matrix before one fresh reviewer; no PASS without an artifact. | **Mostly covered, reviewer prompt delta:** extend T-016's `assets/review.md` to map criterion -> evidence -> blocker, distinguish advisory notes, and identify unverified claims. |
| Adversarial planning: `P/skills/hyperplan/SKILL.md:19-47,243-344` | Multi-role cross-critique and synthesis; transport does not enforce good reasoning. | **Skip default; opt-in PM workflow:** reserve for unresolved consequential design choices. Do not require a debate team for ordinary tasks. |
| Verification tiers/test discipline: `P/skills/ultrawork/SKILL.md:27-62`; `P/skills/programming/SKILL.md:94-105` | Risk-sized evidence, deterministic tests and meaningful regression checks. | **Adopt, contract + gate/reviewer policy:** record a reason for the tier; never downgrade repository-required gates. Reuse E8's injected-clock pattern. |
| Visual QA: `P/skills/visual-qa/SKILL.md:24-32,57-83,214`; `scripts/cli.ts:15-76` | Real captures, viewport/theme/platform checklist and executable image/TUI helpers; a helper's JSON is not a visual verdict. | **Adopt protocol, adapter tools:** required visual criteria need real rendered evidence. Use T-016 to judge it; keep capture engines outside corral. |
| Hypothesis-driven debugging: `P/skills/debugging/SKILL.md:63-79,95-105`; `references/methodology/06-fix.md:9-23,54-70` | Reproduction, competing hypotheses, regression proof and escalation guidance. | **Adopt small PM/worker recipe:** hypothesis, exact repro, changed observation and proof. Do not mandate multiple consultants for every defect. |
| Scope discipline / anti-slop: `P/skills/remove-ai-slops/SKILL.md:25-34,47-50,84-102` | Diff-scoped behavioral cleanup and proof before deleting guards; includes a generic size ceiling. | **Covered + adopt reviewer/gate rules:** T-016 already asks for minimal scope; T-017 has owner-path/diff guards. Use corral's 800-SLOC rule, not the skill's 250-LOC threshold. No subjective "slop score" should auto-reject code. |
| Durable memory / recalled-memory: `P/extensions/omo.js:2`, `dV`, `HX`/`VX`, `_1`, `ite`, `ude` | Bound-identity committed memory projection, locked writes, recall delivery and advisory envelopes. | **Adopt project facts, adapter personal recall:** curated project decisions with provenance/review/expiry in dispatch; leave personal identities, embeddings and recall machinery to the harness. Facts cannot silently override task scope. |
| Hierarchical instructions: `P/skills/init-deep/SKILL.md:8-29,48-74,282-320`; `P/extensions/omo-init-deep-advisor.js:2`, `Ke`/`Je` | Repository mapping, AGENTS.md generation, and drift advice. | **Covered discovery + opt-in maintenance:** T-032 points to project rules. PM may file documentation work; do not silently regenerate authored guidance or run `--create-new`. |
| Skill discovery/distribution: `P/README.md:3-27,63-65`; `P/extensions/omo.js:2`, `mv` | Installed skill discovery and filtering; generated README inventories can lag files. | **Covered + adopt compatibility receipt:** extend T-032 `skill.rs`/`prompt.rs` with contract version and capability provenance; generic CLI prompt remains sufficient. Never overwrite foreign skills. |

Minified bundle anchors name searchable identifiers rather than pretending
there is readable original TypeScript at those locations. Runtime findings
were statically traced, except the two isolated helper probes recorded in
`.omo/evidence/t037-source-probes.md`: a first blocked transition was accepted,
and todo normalization kept a later active item. They confirm those helpers'
behavior, not the absence of checks in every possible host extension.

There are also concrete reasons not to copy the skills wholesale:

- `ulw-execute/SKILL.md:23-25` delegates root QA away, whereas
  `review-work/SKILL.md:83-85` requires root-owned QA.
- `ulw-execute/SKILL.md:37-40,220-225` contains differing merge-authorization
  language. Corral's configured acceptance policy must remain authoritative.
- The installed ultrawork skill's uncertainty rule differs from this session's
  injected directive. `S/core/extensions/builtin/goal/prompt.js:1-13,58-81`
  also changes continuation wording by model. Useful model tuning is not a
  universal control-plane contract.

## Ranked top ten

Ranking is a judgment about unattended-run value, not a measured ROI score.
**S** = bounded contract/reviewer change; **M** = one coherent persisted
workflow plus CLI/UI integration; **L** = cross-phase lifecycle and policy
work. "High neutrality" means an ordinary CLI-capable generic harness can
participate without OmO tools. Adapter assistance must be optional.

| Rank / adoption | Unattended value | Effort / principal risk | Harness-neutrality | OmO source and corral integration | Today's evidence |
|---|---|---|---|---|---|
| **1. Completion audit before accepting `done`** | Very high: prevents unchecked or stale work from silently becoming a completed worker handoff. | **M**; deadlocks or duplicate gates if worker-done is confused with owner acceptance. | **High**, native CLI checks. | `S/core/extensions/builtin/goal/tool-registration.js:46-65`, `prompt.js:31-38`; extend `C/crates/corral-ops/src/review.rs::done`, T-015 gate records and T-017 intake/landing. | E2 repeated repair after done; E7 combined-tree compile break. T-017 already covers later re-gating: complement it. |
| **2. Criterion-linked, revision-bound evidence ledger** | Very high: reviewer and recovery can tell what was actually proved, on which run/tree. | **M**; ceremonial self-attestation or stale PASS reuse. | **High**, artifacts plus typed records. | `P/skills/ulw-execute/SKILL.md:166-207`, `P/skills/ulw-loop/SKILL.md:44-64`; extend `C/crates/corral-core/src/model.rs`, `corral-ops/src/artifacts.rs`, `reviewer.rs`. | E1/E5 show useful evidence already exists; E9 shows why missing screenshot proof must remain explicit. |
| **3. Explicit wait/blocker protocol over existing watchdog work** | High: distinguish progressing work, a registered wait, an owner question and a stuck command. | **M** incremental to T-033; false alarms, duplicate wake delivery, or unsafe automated recovery. | **High** for reporting; **adapter-assisted** for session channels. | `S/core/extensions/builtin/terminal/restore.js:59-93`, `goal/prompt.js:46-54`; extend T-033, `C/crates/corral-ops/src/sync.rs`, T-015 result collection and T-017 blocked states. | E3 stuck eval and swallowed input; E4 idle question. Do not implement T-033 a second time. |
| **4. Phased progress with a durable resume checkpoint** | High: a replacement harness can resume without reconstructing a long transcript or repeating finished checks. | **M**; stale cursors or duplicate task authority. | **High**, stable IDs and a CLI-readable packet. | `S/core/extensions/builtin/todotools/todo-query.js:16-29`, `compaction/task-intent.js:1-55`; extend `C/crates/corral-core/src/model.rs`, `dispatch.rs::retry`, `prompt.rs`, board/detail. | E2's detailed rebase repair messages and E6's stale-write race show why next actions need versioned state. Context-loss causation itself was not observed. |
| **5. Selective plan artifact and pre-code approval gate** | Medium-high on ambiguous or overlapping work; low on routine fixes. | **L**; planning bureaucracy, obsolete approvals, and an approval/execution mix-up. | **High**, plan hash plus policy, no special planner tool. | `P/skills/ulw-plan/SKILL.md:24-30,106-126`; extend existing plan artifacts, `C/crates/corral-ops/src/dispatch.rs`, T-016 reviewer/PM prompt, core policy. | E2/E7 reveal overlapping integration contracts; this is a prevention hypothesis, not proof that missing plans caused those failures. |
| **6. Criterion-oriented reviewer and proportional verification contract** | High for a small delta: expose unsupported claims without spawning a new reviewer pipeline. | **S**; ritual extra testing, or mistaking optional notes for blockers. | **High**, shared review prompt. | `P/skills/review-work/SKILL.md:83-109`, `skills/ultrawork/SKILL.md:27-62`; extend T-016 `C/crates/corral-ops/assets/review.md` and T-032 `assets/run.md`. | E5 supports the existing reviewer path; E8 supplies a real behavioral regression proof. |
| **7. Curated project facts in dispatch/resume** | Medium-high: preserve decisions and corrections across harnesses. | **M**; stale or untrusted notes overriding current instructions. | **High** for project facts; personal recall remains adapter-only. | `P/extensions/omo.js:2` (`dV`, `HX`/`VX`, `_1`); extend `C/crates/corral-ops/src/prompt.rs`, `pm.rs` and canonical project storage. | E6's stale-binary note and E2's repeated integration context are useful facts to retain; no controlled memory benefit is measured. |
| **8. Versioned short contract and skill-capability receipt** | Medium: reduce prompt drift and missing-harness assumptions. | **S** for version/provenance, **M** with adapter conformance; overwriting foreign skills or making generic second-class. | **High**, prompt is the baseline. | `P/README.md:3-27,63-65`, `P/extensions/omo.js:2` (`mv`); extend T-032 `C/crates/corral-ops/src/skill.rs` and `prompt.rs`. | E1 demonstrates omp consumption; recorded skill/model-policy differences show why blindly copying the installed text is unsafe. |
| **9. Dependency groups and write-scope-aware fan-out** | Medium: safe parallelism and visible parent acceptance without a second scheduler. | **L**; dependency cycles, duplicate dispatch, shared-contract collisions. | **High**, corral tasks/runs; optional executor adapters. | `P/skills/mass-ulw/SKILL.md:11-27,69-89`, `P/runtime/dag/library.js:34-80,106-122`; extend `C/crates/corral-ops/src/dispatch.rs`, `sync.rs`, PM skill and T-026 routing. | E2/E7 support declaring shared contracts and integration order; they do not prove every conflict is preventable by serializing tasks. |
| **10. Capability-aware visual QA and cleanup receipts** | Medium overall, high for UI work: reject "looks fine" without the relevant rendered proof. | **M**; missing capture tooling or mandatory matrices irrelevant to the changed surface. | **High** protocol, **adapter/tool-dependent** capture. | `P/skills/visual-qa/SKILL.md:24-32,57-83`, `P/skills/ulw-execute/SKILL.md:166-174`; extend T-032 `C/crates/corral-ops/assets/run.md`, `C/crates/corral-ops/src/artifacts.rs` and T-016 `C/crates/corral-ops/assets/review.md`, not corral's renderer. | E9 explicitly lacks PNG/color proof; E8 covers deterministic terminal behavior but is not a substitute for pixels. |

Rank is not implementation order. Build the minimal ledger (#2) needed by
completion auditing (#1), and land the small reviewer delta (#6) early. Extend
T-033 for #3 only after its current work settles. Add #4 before expanding
planning/DAG machinery. Keep #5 opt-in until its additional latency can be
compared with repair rounds avoided.

Do not adopt a new generic executor, model-specific thought process, personal
memory system, compaction engine, universal hyperplan team, or automatic
anti-slop refactor. Corral's job is to make obligations and evidence portable,
not to reproduce an agent harness.

## Five ready-to-file task specifications

These are specifications for the PM to file, not tasks created by this spike.
Labels A-E are local cross-references, not allocated task IDs. Each task
preserves the single CLI write path and the existing review/acceptance policy.
All new command forms below are proposed interfaces.

### A. Audit completion against current evidence before `corral done`

#### Goal

Make a successful worker handoff mean that the required completion contract
has been checked on the current revision. Keep worker completion, reviewer
approval and owner/automatic acceptance distinct.

#### Design

- Extend `corral-core` run metadata and `corral-ops::done`; reuse T-015 gate
  records, T-017 autopilot, and specification B's evidence records.
- At dispatch, snapshot a versioned completion policy and the resolved
  required commands (the same effective review gate / AGENTS.md fallback
  used by T-032), plus criterion IDs. Existing runs retain legacy behavior
  unless the owner explicitly enables the new policy; show that distinction.
- Add `corral done <id> --check --json`: read-only preflight with named
  failures, observed HEAD/base, required checks, missing/stale evidence,
  unfinished required steps, and unresolved owner decisions. It neither
  commits nor changes status. Ordinary done uses the same validator.
- Under the audited policy, require a clean tracked worktree, no rebase in
  progress, current base ancestry, current passing machine-run check receipts,
  and all essential criteria resolved with eligible evidence. A log titled
  "gate passed" is not a machine-run receipt. A no-gate policy is explicit,
  not an inferred success; reviewer judgment still covers semantic criteria.
- Validate before `commit_all` or success snapshots; audited done must not
  silently commit unverified changes. Recheck run identity, task revision,
  HEAD and base at the transition boundary. Reject stale state instead of
  overwriting a newer update, following T-025.
- On success, attach a completion-audit record and move to Review. Never
  invoke approve or alter `accept=owner`. Preserve T-017's later rebase and
  re-gate when main moves after this check.

#### Acceptance

- In a fixture workspace, commit a change, run its configured gate through
  corral, record evidence via B, and run done: exit 0, Review, audit artifact
  names the checked HEAD/base and evidence IDs; base branch remains unchanged.
- For dirty worktree, unresolved rebase, moved base, moved HEAD, missing
  receipt, stale receipt, missing essential evidence, and unresolved decision:
  `done --check --json` names the exact failing condition, and ordinary done
  refuses without a commit or success transition.
- Change the run/task revision between snapshot and save using a deterministic
  fixture hook: completion refuses and preserves the newer state.
- Move main after a successful done: T-017 re-gates before landing; owner
  policy never lands automatically. Legacy policy remains explicitly usable.
- Run real CLI scenarios with a generic fixture worker in `corral-test`;
  capture JSON, event history and audit artifact. Relevant ops/CLI tests and
  the repository full gate pass.

Dependencies: B's minimal ledger, T-015, T-017, T-025, T-032.
Out of scope: autonomous semantic acceptance, replacing autopilot, or
requiring an OmO goal tool.

### B. Add criterion-linked evidence records to each run

#### Goal

Let every harness supply the same inspectable answer to "what proves this
criterion, on this attempt and revision?" without parsing prose summaries.

#### Design

- Keep artifacts as content-addressed snapshots. Add a versioned evidence
  record in canonical corral storage, mutated only through CLI/ops and logged
  through the existing event path. No second agent-owned JSONL authority.
- Register stable criteria from the task specification through an explicit
  PM/owner-reviewed manifest; do not derive executable commands from arbitrary
  Markdown. Each criterion has ID, required/optional status, scenario and
  binary expected observable. A spec revision makes its criterion mapping
  stale until reconciled explicitly.
- Proposed `corral evidence record <id> --file <json>` and
  `corral evidence list <id> --json`. A record contains criterion, run/attempt,
  observed status (`pass|fail|not_run`), exact command/action, environment,
  HEAD/tree, expected/observed result, artifact IDs, capture time, producer,
  and any owned-resource cleanup receipt.
- Corral computes current identity/revision and artifact hashes rather than
  trusting worker-supplied labels. Distinguish corral-executed receipts from
  worker assertions. Command exit 0 validates execution only; a reviewer
  decides whether the evidence establishes a semantic claim.
- Append new observations; never relabel an old capture onto a new tree.
  Changed code/spec makes evidence visibly stale. Keep conservative HEAD
  freshness initially, matching T-015; any later tree-equivalence reuse must
  be explicit and must not weaken repository-required post-rebase checks.
- Reject unknown criterion/run, duplicate conflicting idempotency keys,
  missing/empty referenced files, malformed records and path escapes.
  Not-run is never PASS. Required evidence waivers belong to owner policy,
  with reason and history, not the worker.

#### Acceptance

- A generic worker records a CLI happy-path result with a log artifact;
  `evidence list --json`, `show`, and the T-016 reviewer prompt expose the
  criterion and observation with matching artifact hash/revision.
- Empty/missing artifact, wrong run, unknown criterion, malformed payload,
  reused conflicting key and a path escaping allowed artifact roots fail
  without partial criterion success. Repeating the identical record is
  idempotent.
- A new commit, changed task criteria or retry makes prior evidence stale
  rather than green for the new attempt. Failure and not-run remain visible.
- Two fixture writers update different criteria under controlled ordering;
  neither result is lost. Conflicting same-record writes are rejected.
- Capture real CLI output and stored artifacts from an isolated workspace;
  relevant artifact/store/ops/CLI tests and full gate pass. No new test pins
  explanatory prose.

Dependencies: existing artifact/store infrastructure; integrates T-016/T-032.
Out of scope: a new log store, automatic trust in transcripts, or arbitrary
shell execution from task text.

### C. Persist explicit waits and owner blockers; extend T-033

#### Goal

Make an unattended card explain whether it is working, waiting on a known
event, asking the owner, or suspected stuck, and resume without duplicate work.

#### Design

- Build on T-014/T-025 sync, T-015 gate result collection, T-017's blocked
  landing state, and T-033 watchdog/recovery. This is an extension of T-033,
  not another foreground-process detector or killer.
- Add versioned run records for `wait` and `decision`, with stable ID,
  condition/question, owner, source (`corral|adapter|worker`), registration
  time, optional deadline, last observation and terminal outcome.
- Proposed `corral run wait <id> --file <json>`,
  `corral run decision <id> --file <json>`, and
  `corral run resume <id> --wait <wait-id> --file <receipt-json>`.
  Generic workers use the same CLI; adapters may populate real channel IDs.
- Corral-owned gates use their actual result files. Harness-owned waits are
  declared handles, not permission to execute arbitrary monitor commands.
  A declared wait cannot permanently mask a stuck process: expired, lost,
  mismatched-run or unverifiable handles become visible diagnostic states.
- A pending owner decision goes to NeedsMe with the exact question and
  requested action. A valid external wait stays distinguishable from NeedsMe.
  Idle alone does not prove either. Persist answered decision data for resume.
- On sync/restart, reconcile receipts idempotently, preserve delivery-unknown,
  and notify once per meaningful transition. Do not blindly resend a command
  or kill any process. Destructive recovery remains T-033's identity-checked,
  confirmed path; a silent harness is not authorization.

#### Acceptance

- A generic fixture worker registers a gate wait. The card/show JSON names
  the condition; completing the gate advances once and records one receipt,
  including after restarting sync.
- A question is visible as an owner decision even when fake herdr reports
  idle; a known pending wait is not mislabeled as an unanswered question.
- Lost/expired wait, missing adapter capability, duplicate receipt, stale
  retry-run receipt and delivery-unknown each have explicit deterministic
  outcomes; no duplicate launch or auto-kill occurs.
- T-033's known-harness/helper protections and PID identity tests remain
  green. An expired declared wait does not suppress its stuck diagnostic.
- Use fixed clocks and injected readiness events, not sleeps, for tests.
  Capture actual CLI plus isolated terminal/card evidence for waiting,
  decision, and expired states. Relevant tests and full gate pass; teardown
  receipts name only resources created by this QA.

Dependencies: settle/integrate T-033 first; T-014, T-015, T-017, T-025.
Out of scope: copying senpi's PTY runtime, universal wake guarantees, or a
three-turn blocked rule unrelated to observable state.

### D. Expose phased progress and portable resume checkpoints

#### Goal

Resume the same corral task in any supported harness without losing the
canonical requirements, repeating valid checks, or silently dropping work.

#### Design

- Add optional phases/steps with stable IDs to each run; states are
  pending/in-progress/done/dropped, and drops carry reasons. Keep acceptance
  criteria separate: closing a checklist does not prove a requirement.
- Proposed `corral run progress <id> --file <json>` and
  `corral run checkpoint <id> --file <json>`, with `--json` readback.
  Validate identities and revision under the store lock. Phase labels can
  change without losing stable step identity.
- A checkpoint captures task/spec revision, current HEAD/base, criterion and
  evidence references, steps completed, current step, concrete next action,
  unresolved decisions, wait handles and owned-resource obligations.
  Snapshot it as an artifact; preserve earlier checkpoints.
- Surface phase plus done/total on the card, with full details in the run
  view. Do not parse terminal prose or exact handoff wording as state.
- Extend retry/resume dispatch to inject the latest compatible checkpoint
  and canonical task criteria. A changed spec/tree is flagged explicitly;
  stale evidence is not promoted. Missing checkpoint falls back to the task
  contract, never fabricated progress.
- Require a checkpoint at explicit worker handoff and retry boundaries where
  the worker is reachable. Adapters may request it before compaction, but
  corral never assumes it can intercept every harness context limit.

#### Acceptance

- Harness A's generic fixture records two phases, completes one step and
  checkpoints. A fresh harness B dispatch receives the same criterion IDs,
  completed step and concrete next action through the actual prompt surface.
- `show --json` and a rendered card/detail agree on progress after restart.
  Renaming a label preserves identity; duplicate IDs and stale updates fail.
- A changed spec, new commit, missing checkpoint and retry with an obsolete
  run ID produce explicit stale/missing handling without overwriting newer
  data or repeating a valid completion transition.
- Dropping a required step cannot mark its acceptance criterion satisfied.
  A pending wait/resource obligation remains in the handoff.
- Capture the CLI round trip, rendered progress and actual dispatch prompt
  in an isolated generic fixture. Relevant tests and full gate pass; no
  harness-specific compaction or memory service is required.

Dependencies: B for evidence references; C for typed waits/decisions where
enabled. Keep the progress-only path usable without either feature enabled.
Out of scope: editing another harness's todos, token budgets, compaction
summaries, or automatic re-planning.

### E. Gate implementation on an approved plan when policy requires it

#### Goal

Allow deliberate planning before risky or ambiguous work while leaving
routine tasks on today's direct dispatch path.

#### Design

- Add an optional task/project plan policy, default off, using existing plan
  artifacts and T-016's reviewer infrastructure. PM recommends the policy
  based on concrete uncertainty/overlap; task size alone is not enough.
- A plan artifact records goal/non-goals, decisions, affected paths/contracts,
  task dependencies/write scopes, acceptance scenarios, verification commands
  and remaining owner decisions. Store its hash and the task-spec revision.
- Proposed `corral plan submit <id> <path>`,
  `corral plan review <id> --reviewer <profile|pm>`, and
  `corral plan verdict <id> approve|changes|escalate --notes-file <path>`.
  Plan verdicts use a separate target/type from implementation verdicts.
  The PM can attach a plan before implementation dispatch; no extra coding
  task is automatically created.
- A plan approval binds to the plan hash and spec revision. Edited/replaced
  plans and changed criteria invalidate it. Review policy determines who may
  approve; owner-only decisions remain owner-only. Plan approval never merges
  code or marks the task Done.
- When plan policy is enabled, implementation dispatch refuses absent,
  pending, rejected or stale approval with an actionable reason. Review
  launch remains allowed without a coding run. An explicit later dispatch is
  still required; approval does not silently start work or bypass WIP limits.
- Extend PM guidance and the worker contract with the approved plan reference.
  Do not install ulw-plan, invoke hyperplan automatically, or treat a touched
  file as approval.

#### Acceptance

- With plan policy off, existing generic dispatch behavior and limits are
  unchanged. With policy on, dispatch refuses missing, pending and rejected
  plans without creating a worktree/pane or consuming a run slot.
- Submit a plan, review it with a fake generic reviewer in `corral-test`, and
  approve: the subsequent explicit dispatch succeeds and its prompt names
  the exact approved plan and spec revision.
- Change the plan bytes or task criteria between review and dispatch: stale
  approval is rejected. A concurrent stale verdict cannot overwrite the
  current plan. Historical plans/verdicts remain inspectable.
- An unresolved owner decision or unauthorized reviewer escalates rather
  than approving. A plan verdict cannot satisfy implementation review or
  change `accept=owner`, merge code, or create additional tasks.
- Capture plan/dispatch JSON, event order and prompt artifact; relevant
  core/ops/CLI tests and full gate pass. Planning-only QA leaves no panes.

Dependencies: T-016, T-032; reuse B's criterion IDs when enabled.
Out of scope: a mandatory planning phase for every task, multi-agent debate,
new workflow scheduling, or automatic PM task filing.

## Rollout and explicit non-claims

1. Preserve the current owner/auto acceptance policy and one authoritative
   task store. Introduce schema/contract versions before enforcing new fields.
2. Ship B's evidence foundation, A's completion preflight, and the small
   reviewer prompt delta. Enable enforcement on new fixture-backed runs
   first, then opt projects in explicitly.
3. Extend existing T-033 with C; add D's portable checkpoint. Add E only for
   tasks that benefit from a plan decision, not as a blanket tax.
4. Measure time to accepted completion, repair rounds, stale-evidence
   refusals, stuck duration, reviewer reversals and missing-proof frequency.
   Reuse T-028/T-034 with sample counts and task-kind/size cohorts; unknown
   cost or harness usage remains unknown.

This spike did not run long equivalent workloads across omp, OmO, Claude and
generic, reproduce the historical stuck process, or execute the proposed
interfaces. It does not claim causality from task anecdotes, universal
background wake reliability, or semantic truth from a green checklist.
Research validation consists of primary-source inspection, the two isolated
helper probes, report coverage/readback, and the required corral repository
gates. Gate and handoff receipts are attached separately to T-037.
