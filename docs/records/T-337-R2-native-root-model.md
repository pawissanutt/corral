# T-337 R2: whole native owning sync and actual process restart

## Owning entries and retained data

The root regression calls **`crate::sync_at`**, not just a selector or `hooks::sync::pass`. Both a workspace-bound root PM and one active assistant are bound through actual hello, recording real stand-in host PIDs/start times. Their private Unix bridge sockets serve real hello/get_state/events_since RPCs. No live herdr server or owner pane is used.

Each root's canonical document retains 2 versus 20 old entries in all ten native families: generation prefix, turns, receipts, questions, wakes, reports, jobs, rewinds, usage sessions and budget requests. The old current generation 999 and actual bound generation 1000 have fixed-width identities. Every prior generation is fenced; historical questions are withdrawn. The fixture also grows retired Tasks/containers, planning/review/obligation/artifact history, task PM receipts, closed assistants and the approved standalone-inbox effect/cursor history. The active assistant's body also grows.

Real settlement initialization and an owning sync establish a steady store. Measurement runs once warm and once in a **separate execution of the test binary**, at each scale. The child verifies its PID differs from the parent's, performs its first actual sync without a primer, and writes a fixture-private result file; the parent requires the exact child test to execute, checks the result PID and compares every unattributed counter. Expected canonical-row sizes are derived through a separate Store connection, never by warming the observer Ctx or sampling measured ledger values.

Preconditions precede interpretation of work cells: valid configuration, actual admitting PinnedReadingsProbe, exact volume set, both real samples, unheld/nonsevere resource state with unchanged PSI inputs, no terminal work, one active assistant, unchanged complete canonical PM/assistant documents and events. The parent observes the real RPCs for each measured pass: PM two readiness hellos plus one get_state/events_since pair; assistant one get_state/events_since pair. No submit/cancel/unexpected RPC is accepted.

## Exact opaque/follow-up model

Let P, I, S and C be the byte lengths of the actual canonical root PM, PM inbox, assistant session and assistant container documents before the measured pass.

| Existing named scope | Calls | Record decodes | Document bytes | Views |
|---|---:|---:|---:|---:|
| T-625/root_pm_load | 10 | 10 | 10P | 10 |
| T-669/standalone_inbox_load | 3 | 3 | 3I | 3 |
| T-671/assistant_session_load | 1 | 1 | S | 1 |
| T-671/assistant_container_load | 2 | 2 | 2C | 1 |
| T-671/assistant_container_save | 1 | 1 | C | 0 |
| FU-3/sample | 2 | canonical Task-row oracle | twice the canonical Task documents | 2 |

Every other counter at each named site is zero except the independently calculated FU-3 Task/head/run/nested/history decode and filesystem-stat cells. Own-record and emitted cells are zero. The complete producer-key/call map must match exactly: no unmodeled label/call is tolerated. Inclusive identities and zero unattributed hydration are mandatory.

Source derivation:

- PM loads: PM inbox sweep (1), root pass initial/reload (2), outbox initial (1), ScopedPm readiness update/load plus native target/load twice (4), delivery's unconditional bind/target load even for an empty inbox (1), liveness record/update (1). Healthy sequence zero produces **no PM save**.
- Standalone inbox loads: delivery recover_publication head, observe_pass open head, and replacement head. There are no pending messages/envelopes.
- Assistant: initial container read; mutation reads session and container and always saves the unchanged container. The mutation's `records_transaction` opens its held view on the session read; the following container read still decodes but reuses that view.
- FU-3: persisted resource observation, then attention/snapshot_heads' resource precondition. An absent held-admission reason makes attention resample even on the empty open board. Both are actual SystemResourceProbe walks; only numeric admission inputs are pinned. Full Task-row bytes/categories come from canonical TaskDoc rows, with one listing view per call, and filesystem cost from canonical run paths, existing ancestors and free-space queries.

The first draft correctly rejected its incomplete source prediction (it omitted unconditional delivery bind, attention's resample, and transaction-view reuse). Each discrepancy was traced to the source above before finalizing the static model. No observed byte count is used to set an expected cost.

A one-byte fixture identity drift was independently located in `pm-events/state.value.seq` (18 versus 126), not in history work. As with existing action-budget fixtures, inert events pad event identities to the same decimal width. Unlike that older helper, these fixtures retain **different total event counts** (10,002 versus 10,020 before real root bindings), so an accidental all-event scan still changes work and fails equality. Temporary input tracing is removed. No byte cell is masked, normalized or given slack.

## Actual task-process restart too

The retained task-native owning pipeline test now shares its unchanged canonical Task/event T-623 oracle between warm, reopened-Ctx and truly fresh-process observers, at both scales. Each process invokes the actual `hooks::sync::task_id` pipeline, reports one healthy live worker and no attention, leaves events unchanged and matches all three existing question-event replay calls to the independent canonical TaskDoc/event formula. Parent-side RPC observations prove every pass probed the actual host. The previous reopened-Ctx case is retained and no longer substitutes for process restart.

## Results and negative control

- Root whole-sync warm/fresh at 1x/10x: **PASS**; fresh child PIDs 609679 and 610843 in the restored-source log.
- Task pipeline warm/reopened/fresh at 1x/10x: **PASS**; fresh child PIDs 609254 and 609537.
- Real negative control: replace the root active-assistant key selector with full `list_assistants` followed by an in-memory ended filter. Effects and host probes remain the same, but the test **fails** on the additional T-671/assistant_session_enumeration producer. An approved legacy scope is not a blanket waiver for a new historical read. Mutant removed; restored-source root test passes.

Evidence: `R2-root-whole-sync-restored-green.log`, `R2-native-task-fresh-process-green.log`, `R2-root-enumeration-negative.log`, and the exact `R2-root-enumeration-negative.diff` under `.evidence/` and attached artifacts. No production source changed in this milestone. All existing follow-up scopes and producer expectations remain intact.

Manual recovery's `agent/recovery.rs::receipts` retains its PM board list: it is reached by fenced-generation drain/interrupted-call recovery, not by either owning sync entry. This proof does not certify manual recovery as history-constant. Assistant/PM-desk legacy import is still absent, so intentional migration refusal remains an explicit owner-cutover limitation. No full regression, work-budget matrix, final freshness/rebase, gate, or live cutover is claimed here.
