# T-854 — Co-work transfer and continuation

**Review round 1 active:** the previous quick gate passed, but review found a broken real planner-dispatch seam (F1) and unmet positive shared-PM settlement (F2). F1 now has a failing-first service regression and repaired ordered checkpoint path; F2 remains unfinished under Q4's bounded per-review enforcement decision. Earlier delivery/complete state is superseded; no new-round gate or rollout is claimed.

## Scope

C3 extends C2's one transfer journal/reservation/authority epoch and C1's completion, planning-policy, activity and verified-drain services. It adds no scheduler, global permission grant, second ownership store or completion controller.

- **Save:** durable hold defeats manual/automatic admission and survives replay/reopen. A requested stop remains `Saving` while a writer cannot be positively settled.
- **Take over:** exact selected goal, context and current target are durable. Ownership commits once, after source admission closes and its actual supported boundary drains. Normal capacity, dependencies, pauses, reviews and checks remain authoritative.
- **Take back:** serialize with landing/launch/gate operations, hold admission, settle supported task writers, preserve latest work and the same task/history, then return prepare-only. A matching canonical landing receipt returns `AlreadyLanded`; it never means rollback.
- **Pending retreat:** consume the old unpublished selection before stop/return; preserve any partial card and original source. Recovery cannot roll the cancelled selection forward. Refresh creates a new checkpoint/choice, not a rearmed old action.

Plan continuation, review-only, implementation prepare, verify and ship are distinct goals. Current plan/source/producer/findings/approval/run/code changes invalidate selected actions. Old plan approval cannot authorize a newer raw draft or changed scope. Formal plans require normal independent review/acceptance; review-only stops at its verdict without author revision, implementation, code gate or landing.

## Preservation and recovery

Selected context is ordinary readable R0 evidence, with an exact archive index and bounded UTF-8 parts. Conversation selection is not a credential/session/tool-execution fork. Code retains commits, raw index, binary work and non-ignored untracked files. Missing, oversized, special, altered or ambiguous work fails closed rather than silently disappearing.

Stopped-worker continuation checkpoints latest work, not the original imported package; normal scheduling still allocates the next run. Latest settled unsubmitted planner `output_dir/plan.md` is bounded/no-follow, archived as a Draft, and never fabricated into a submission or approval. Materialization preserves pinned code across integration-base movement and refuses overwriting later owner edits.

Publication keeps a partial task held until source proof/adoption commit. Task/companion records and ownership publication are atomic. The recovery index is only an index into that journal. Notification occurs after ownership, with replay deduplication; source loss or attachment failure cannot create a second task or an early delegation.

## Entry and compatibility

The minimal owner-only JSON CLI is `assistant handoff`: init, prepare, preview, commit, show, continue-preview, start, save, take-back, retreat-preview and retreat. It uses established OS/cwd/agent/worktree owner guards; the entire family is Refused to managed agents. Input IDs, caller text, flags and ordinary finish confirmation are not transfer consent.

Native `assistant_handoff`/BeginExecution choices and both TUIs remain unavailable pending C4's authenticated payload-bound named-action consumers. Q1/Q2 established implementation partition, not transfer consent. No owner installation or live owner transfer was performed.

Supported source proof binds the actual host/session/generation/PID/start time and exact reserved workflow/revision/epoch. A package/version label is not drain proof. Historical source controllers remain retired after transfer and cannot regain write authority merely because data returns Local. A ready return does not launch or re-enable an old native controller; a legitimate current local generation is still required.

## Observable proof map

These are design A-IDs, not invented audited criterion records. `corral criteria list T-854` returned a null manifest; no audited observations were fabricated.

| Design criterion | Evidence and boundary |
|---|---|
| A09 | `saved_transfer_is_once_held_after_reopen...`, `save_with_an_unverified_writer_stays_saving...`, owner CLI save/replay/start/save journey. Unknown writer stays held; no residual ship/launch. |
| A10 | `takeover_commits_once_then_waits_for_normal_slot_and_dependency_without_a_source_host` emits normal slot/dependency reasons; one task/ownership and zero launches. Occupied slot is a record fixture, not a real running worker. Slow held-publication recovery also survives source-session closure. |
| A11 | Assist scoped-goal admission and exact stopped-worker seed/next-run identity contracts. No global grant is forged; normal resolver remains in use. Next-run allocation is not a separately driven live scheduler/worker journey. |
| A12, inherited T-754 A8 | Existing service contract parameterizes research, draft, reviewed, dirty partial and completed source, rejects stale source, then proves one held owner/task after commit replay/reopen. Exact bytes remain; no imported approval/PASS or planner/implementation launch. |
| A14 (joint), A29 | Atomic task companion-record conformance; existing publication crash catalog; new slow task-created/source-session-close/attachment-cap failure recovery, with no ownership/outbox before readiness and once-only notification afterward. Source-session closure is a canonical fixture, not actual SDK host death. |
| A15 | Current-source/target pinning, protected resume/start guards, final transaction-bound plan verdict/submission checks, and permanent native planning-policy retirement contracts. Existing real SDK planning/shell policy tests supplement service contracts; no end-to-end live C3 source-resume claim. |
| A16 | Existing exact-code journey preserves latest repaired layers and same history, positively stops/reaps an owned Linux stand-in Worker without starting a replacement, then returns prepare-only. This is not a real SDK host; direct live TakeBack is not separately driven. Latest unsubmitted planner draft is retained without submission. Unsupported writers stay held. The shared-PM service contract proves Save/TakeBack/reopen retain Board ownership, pending review and source with a named wait, and reject a late verdict; it does **not** prove successful shared-PM return or blanket A16 acceptance. |
| A17 | Actual shared landing lease blocks take-back mutation; canonical landing-record fixture yields exact AlreadyLanded without Local/rollback mutation. Manual approval uses the same lease (`review.rs::approve`). This is not a real Git shipping race. |
| A18 | Existing-target findings pin, source revision/epoch, code/index/work and current-contract tests refuse stale selection. Bare C2 reservation preserves returned lineage; current returned scratch is fenced even with no/different historical run worktree. |
| A29 | Exact raw index/binary/work/untracked restoration and no-empty-source refusal. Explicit slow partial-materialization restart advances real main, reopens, recovers exact pinned layers twice, preserves new main content and refuses overwriting later owner edits. It exercises the production materializer, not full transfer authority. |
| A33 | Existing normal native plan-review service test extended with exact C3 ReviewOnly source/target and actual producer. Dedicated independent generation1 is normally launched, nonce-bound, and sends actor-bound native Changes verdict/replay. Normal pilot ticks preserve one plan/review/producer and start no rewrite/planner/code/gate/landing. Scripted authenticated protocol fixture, not model review quality or C4 consent. |
| A34–A35 | Raw-draft predecessor supersession, stale-context renewed planning, optional-policy formal approval, saved manual decision refusal, plan-only admission/decomposition/self-projection stop contracts. Old acceptance and broad grants do not authorize code for the new goal. |

Raw commands, red regressions, checkpoints and full logs are attached under `artifacts/T-854/R1/`. Checkpoints are WIP continuity records, not criterion acceptance.

## Verification and test cost

After the clean current-main rebase: normal workflow **35/35, 12.27s**; independent native ReviewOnly **1/1, 0.27s**; isolated owner CLI **1/1, 0.38s**; explicit slow publication/materialization catalog **3/3, 6.205s**. Native typecheck and selected source-drain/bridge/planning-policy/real-SDK host/shell contracts **18/18, 5.72s** pass. Earlier Python catalog policy **10/10**, trigger **4/4**, and backend atomic-operation conformance also passed; their scope/limitations are in attached checkpoints. The later Q3-specific held-path service check **1/1, 0.18s** and existing recovery-update-debt contract **1/1, 0.30s** also pass after the size-only refactor; they are recorded separately in attached integration logs.

Integrated core/ops production Clippy passes; CLI Clippy passed before rebase and the integrated CLI compiled/executed afterward. LSP had clean results, expected inactive-platform hints, bounded timeouts and unsupported Python/TOML; actual fresh compiler/tests provide the stated fallback. No universal clean-LSP claim. The integrated size audit exposed an 802-SLOC recovery file; moving unchanged recovery-message binding into the existing notice module restores headroom without new behavior/tests. Its first test compile caught a removed test-only parent import, which was restored rather than suppressing the error.

One new owner-CLI E2E is retained. Variants are cheaper service/model contracts or extensions of existing journeys; restart cases are explicitly slow with an owning handoff trigger. Fixtures are small, with no history/load burners or paid providers. The first gate reported **29 added registrations (27 Rust, 2 Bun)**, all then unmeasured because fail-fast stopped before their execution; its initial `0.000s measured` is **not** zero added cost. Final observed added seconds remain a gate-artifact fact. The classification repair extends the existing table with all 11 handoff leaves, each `Refused` with a valid CLI shape and unchanged-workspace assertion; no new registration, relaxed assertion, production permission change or quarantine edit. Its suite passes **3/3, 3.65s**. The resubmission audit covers **80 changed Rust files, maximum 799 SLOC** (including the previously untouched inventory test). Q3 adds one 0.18s service contract, not another E2E or successful-drain claim.

## Review round 1

F1 reproduced `handoff_stop: admission is not held` through actual planner dispatch. Planner preparation now verifies the exact admitted Plan scope, temporarily closes admission under the existing launch/controller/gate leases, uses the unchanged held-stop settlement and draft checkpoint, then revalidates run/spec/driver/goal and ordinary admission inside the hold-release transaction. Failure/crash retains a visible held wait; Save/TakeBack's required hold was not weakened. One small service regression covers initial research/raw-draft manual dispatch, inert stopped-planner Save/continuation and normal automatic dispatch, exact latest draft in the next prompt, same history and no release of Saved. The engine records commands without executing an agent; this is not physical native-planner settlement proof.

The regression failed first, then passes in **3.42s**; workflow **37/37, 17.91s** and production ops Clippy pass. The intermediate automatic test used invalid empty policy inputs; replacing them with the normal `availability` source preserved the Started/history/hold assertions. Production LSP is clean; test LSP timeouts were covered by actual compilation. One new service registration, no CLI E2E or crash matrix.

F2 is still blocking. Q4 (`T-854_R1#18`) permits only the minimum enforced per-review PM execution/write-context and positive drain needed for existing A16, reusing existing ledger/journal and keeping unrelated PM contexts alive. It does not authorize a generic grant/audit system, arbitrary containment, global PM pause, transfer consent, UI expansion or an A16 waiver. The existing read-only trace must be consumed before choosing the smaller supported route/host implementation. Historical/unbound contexts remain held.

## Unresolved / not verified

- **Q3 answered by PM, revision2, `T-854_R1#11` (2026-11-08T15:22:02Z):** retain the exact unsupported held wait; never stop/abort/globally pause PM or infer physical drain from cancellation/fencing/acknowledgement without an enforced write-context boundary. Shared PM has broad read/edit/write/bash access; positive task-scoped writer settlement is **not implemented or demonstrated**. Resume requires a supported dedicated verifiable host or a separately scoped, enforced task-write-context settlement seam—not mere review cancellation. This remains an unmet successful-settlement proof gap, not transferred to C4, blanket A16/joint-matrix PASS, or an acceptance waiver. The fail-closed behavior is tested. See investigation `artifacts/T-854/R1/88971bb7-c3-shared-pm-settlement14.md`.
- C1 prepublication retreat without a C3 transfer lineage and with a native Worker binding also stays held until a supported positive settlement seam is available. Terminal/Fenced metadata alone is insufficient.
- Real owner input/native choices, both TUIs, rollout and production-main PTY proof belong to C4 and were not performed here. Model review quality, fully integrated live C3 SDK transfer/return, actual source-host death after task creation, and real Git landing concurrency are not established by fixture proofs above.
- First receipted quick attempt `ag-7cd228bed50fdf41` failed on the CLI-classification inventory; its log is `artifacts/T-854/R1/4585e1f9-T-854-R1-ag-7cd228bed50fdf41.log`. Fail-fast left later tests unexecuted, so targeted checks do not substitute for the next receipted quick gate after commit/rebase. A separate quarantined `pm_plan_cli::changed_plan_bytes_only_offer_inspection_not_invalid_verdict_commands` failure belongs to existing T-845 and was not changed. Timing observations were report-only, not the gate failure. No other tier or installation is authorized by this report.
