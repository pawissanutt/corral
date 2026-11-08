# T-854 — Co-work transfer and continuation

**Status:** implementation and bounded verification recorded below; final integration/gate pending. Shared-PM settlement boundary Q3 remains unresolved. This is not an acceptance or rollout claim.

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
| A16 | Existing exact-code journey preserves latest repaired layers and same history, positively stops/reaps an owned Linux stand-in Worker without starting a replacement, then returns prepare-only. This is not a real SDK host; direct live TakeBack is not separately driven. Latest unsubmitted planner draft is retained without submission. Unsupported writers stay held. |
| A17 | Actual shared landing lease blocks take-back mutation; canonical landing-record fixture yields exact AlreadyLanded without Local/rollback mutation. Manual approval uses the same lease (`review.rs::approve`). This is not a real Git shipping race. |
| A18 | Existing-target findings pin, source revision/epoch, code/index/work and current-contract tests refuse stale selection. Bare C2 reservation preserves returned lineage; current returned scratch is fenced even with no/different historical run worktree. |
| A29 | Exact raw index/binary/work/untracked restoration and no-empty-source refusal. Explicit slow partial-materialization restart advances real main, reopens, recovers exact pinned layers twice, preserves new main content and refuses overwriting later owner edits. It exercises the production materializer, not full transfer authority. |
| A33 | Existing normal native plan-review service test extended with exact C3 ReviewOnly source/target and actual producer. Dedicated independent generation1 is normally launched, nonce-bound, and sends actor-bound native Changes verdict/replay. Normal pilot ticks preserve one plan/review/producer and start no rewrite/planner/code/gate/landing. Scripted authenticated protocol fixture, not model review quality or C4 consent. |
| A34–A35 | Raw-draft predecessor supersession, stale-context renewed planning, optional-policy formal approval, saved manual decision refusal, plan-only admission/decomposition/self-projection stop contracts. Old acceptance and broad grants do not authorize code for the new goal. |

Raw commands, red regressions, checkpoints and full logs are attached under `artifacts/T-854/R1/`. Checkpoints are WIP continuity records, not criterion acceptance.

## Verification and test cost

Pre-integration targeted results include normal workflow **35/35**; independent native ReviewOnly **1/1**; isolated owner CLI **1/1**; explicit slow moved-base/materialization **1/1** and held-publication/source-close recovery **1/1**; Python catalog policy **10/10** and trigger **4/4**. Existing SDK planning/shell contracts and backend atomic-operation conformance were also run earlier; their scope/limitations are in attached checkpoints.

Core/ops production Clippy and CLI Clippy pass after local behavior-preserving fixes. LSP had clean results, expected inactive-platform hints, bounded timeouts and unsupported Python/TOML; actual fresh compiler/tests provide the stated fallback. No universal clean-LSP claim.

One new owner-CLI E2E is retained. Variants are cheaper service/model contracts or extensions of existing journeys; restart cases are explicitly slow with an owning handoff trigger. Fixtures are small, with no history/load burners or paid providers. Final gate added-test count/seconds and integrated 800-SLOC audit remain pending.

## Unresolved / not verified

- **Q3:** shared PM has broad read/edit/write/bash access. Existing task-specific cancellation bookkeeping is not positive acknowledgement or filesystem settlement. No shared-PM or unknown resource is stopped/aborted. Pending shared-PM/legacy review stays held; this case is not claimed verified. See `artifacts/T-854/R1/88971bb7-c3-shared-pm-settlement14.md`.
- C1 prepublication retreat without a C3 transfer lineage and with a native Worker binding also stays held until a supported positive settlement seam is available. Terminal/Fenced metadata alone is insufficient.
- Real owner input/native choices, both TUIs, rollout and production-main PTY proof belong to C4 and were not performed here. Model review quality, fully integrated live C3 SDK transfer/return, actual source-host death after task creation, and real Git landing concurrency are not established by fixture proofs above.
- Final current-main rebase and this run's one receipted **quick** gate are pending. No other tier or installation is authorized by this report.
