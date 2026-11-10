# T-931 — exact repair lineage

## Scope

This foundation fixes renewed-allowance launch accounting and adds explicit internal,
record-only legacy projection services. It does not recover, import, renew or launch any
live task, transfer custody, resume an assistant, or expose a CLI/MCP recovery command.

## Accounting contract

- A version-2 continuation history binds the exact predecessor ledger digest and version,
  the single task initial receipt, archived receipts and finding observations. The ledger
  verifies that context against its immutable archive. Local counters contain only actual
  events in that allowance; task counters sum actual starts across allowances. Existing
  `repair status` readback now includes both totals and the unresolved task findings.
- Common observation/adoption/start paths recognize inherited initial context without
  appending another initial event. The final worker launch admission and co-work start use
  those same preconditions. Recovery cannot reference an archived allowance's coding start.
- Version-1 documents retain their wire shape. Unsupported/new context fails closed in old
  readers. An old unanchored renewal remains readable but cannot authorize new coding.

## Internal services

`corral_ops::repair_migration` has no CLI, owner-action, MCP or automatic-load integration.
Requests identify canonical tasks and expected pins, not replacement counters or prose
approvals. Preview is read-only. Import requires an in-process confirmation obtained from
an unmanaged controlling terminal and revalidates its exact proposal under task/dispatch
serialization. Test-only constructors exercise persistence without performing that ceremony.

The bounded source reader pins task revision, contract/current run, schema, every selected
row family (including empty families), record versions/absences, task event traversal end,
and verified typed blob references. The interpreter uses an ordered genesis/issuance/
submission chain, canonical historical normal limits and immutable Git submission metadata.
Submission progression cannot skip or return to an earlier run, and its terminal run and
candidate must agree with independent canonical gate/review/autopilot/audit pins. Source
profile metadata, actual submission actors and Git author/committer identities are retained.
Supported parent-run gate failures and changes reviews become ordered unresolved findings,
not new repair charges. Unstructured findings remain explicitly uncertain rather than being
semantically equated. Canonical verdict events must reconcile with the supported review
snapshot; actual event authorship takes precedence over a nominal reviewer label.

Uninterpreted child review/finding/evidence, gate-attempt/history or nonempty diagnostic
chronologies refuse import. Merely retaining their raw documents is not reconciliation.
Those source records remain unchanged; this service does not silently drop them or reset a
diagnostic retry. Extending that supported source subset requires a faithful interpreter.

The importer is intentionally conservative: incomplete/overlapping issues, unclassified
coding, missing candidate/author evidence, unsupported versions, changed allowances,
unsubmitted writers, historical profile switches without a supported exact chronology,
non-exhausted normal allowances, over-bound evidence and incompatible substantive managed
history refuse. A recognized fail-closed legacy placeholder is retained in full in the import proof.
The projection, typed proof reference, receipt and task-wide consumed-lineage indexes publish
in one task CAS. Import starts nothing and grants nothing.

A separate internal continuation ceremony authorizes exactly one current-profile repair,
zero stronger repairs and zero diagnoses. It binds imported history, task/contract/run,
actual resolved executor and a once-only source approval lineage, independently of request
ID. It retains the dispatch hold and does not authorize custody or launch. The launch path
checks that executor pin before accounting. Later custody integration must obtain genuine
verifiable approval and retain every existing mutation/confirmation boundary; PM prose,
comments and old request identifiers are not approval capabilities.

## Verification

Baseline: `cargo test -p corral-ops --lib
renewed_continuation_charges_third_before_ipc_and_refuses_fourth -- --exact
repair_runtime::tests::renewals::renewed_continuation_charges_third_before_ipc_and_refuses_fourth
--nocapture` failed with actual committed admission charge **0 instead of 1** (test 0.13 s;
wall 185.71 s including initial compilation). `.evidence/renewal-baseline.log` retains output.

The first corrected runtime run passed all six selected runtime fixtures in 0.32 s
(wall 149.50 s including compilation). The broader targeted command
`cargo test -p corral-core -p corral-ops --lib repair -- --nocapture` passed **32 core + 31
ops tests**, in 0.16 s + 10.24 s suite time (185.55 s wall including compilation).
`.evidence/repair-final-targeted.log` retains output. Final gate results and added-test costs
are attached separately after the committed candidate's assigned quick gate.

### Acceptance evidence map

| Criterion | Evidence |
| --- | --- |
| Actual renewed launch admission fails first, then charges before IPC | Baseline log; `repair_runtime::tests::renewals::renewed_continuation_charges_third_before_ipc_and_refuses_fourth` traverses the same `start_launch` used at `launch.rs`'s final durable preparation boundary. |
| One task initial, versioned predecessor and finding lineage; immutable archive | Core context contract test, existing v1 repair contract tests and runtime/co-work fixtures; `task_counters()` and archive equality assertions. |
| Explicit bounded canonical import, actual provenance, refused incomplete/contradictory/tampered/unsupported sources | The `repair_migration::tests` use three tiny immutable Git candidates, distinct historical profiles, actual PM submission/commit attribution, explicit end/pins and negative source variants. Git objects are content-hashed without replacement refs. |
| Atomic evidence/history/receipt/consumed indexes; placeholder retained | `publish_repair_import` joins a task CAS and typed evidence reservation; migration fixtures read indexes and original placeholder proof back. Substantive history and stale previews refuse. Source blob refs remain pinned in the evidence index. |
| Original two spent, exactly one third, no fourth or replay credit | Migration fixture imports max2 exhausted history, stages current1/stronger0/overall1/diagnosis0, rejects archived-attempt recovery, charges total3 with one initial, and refuses fourth/new request IDs. |
| Authority retained, no activation/live recovery | No command/tool wiring; private non-deserializable confirmation capabilities; unmanaged controlling-terminal ceremony; negative managed/noninteractive confirmation check. Existing entry, dispatch hold, independent review and gate paths are unchanged. |
| Proportional validation and cost | Four original low-layer registrations plus two review regressions; command output and each assigned quick gate receipt/log. |

Original submission registrations: one launch-admission regression, one core context contract
test and two tiny import/service tests; the review correction below adds two regressions. No additional end-to-end journey, global policy/budget or
existing assertion relaxations, quarantine or gate configuration changes. No intentional host load or provider requests.

Targeted production lint/typecheck: `cargo clippy -p corral-core -p corral-ops --lib --
-D warnings` passed (27.97 s wall on the corrected lint run). The assigned gate checks the
final committed/rebased tree, including the final readback/version-chain assertions.

## R1-review1 corrections

Independent review returned F1 (accepted `[R1,R3,R2]` submissions despite current R3) and F2
(terminal R3 changes-review notes missing from imported unresolved lineage). Both reproduce
on `5831a5f4` with the existing three-candidate fixture, before production changes:

`cargo test -p corral-core -p corral-ops --lib repair_migration::tests::review1_ -- --nocapture`
failed both new regressions (0.12 s suite; 118.51 s wall including compilation).
`.evidence/review1-baseline.log` retains the failing-first output.

F1's cause was set/cardinality validation instead of full stream progression, not missing
run rows: the canonical rows were still R1/R2/R3. The shared submission path now validates
progression and terminal pins for both `cowork_finished` and `done`, and checks independent
candidate identities/trees. The tiny regression covers the swapped progression and terminal
candidate contradiction in both branches, keeping the ordinary path valid.

F2's cause was a gate-only observation interpreter, not permission to discard a stored
review. Supported changes-review notes now produce a stable, provenance-bound unresolved
observation with explicit uncertain matching. The regression proves parent-review-only
import, canonical-event PM attribution, refusal of unmatched verdicts and uninterpreted
sibling families, and retention of the finding across continuation without changing the
original two charges or archived allowance. Existing counter, import, launch and refusal
assertions remain intact.

`cargo test -p corral-core -p corral-ops --lib repair -- --nocapture` then passed **32 core +
33 ops tests** (0.16 s + 9.44 s suite; 102.39 s wall). `.evidence/review1-targeted.log` retains
output. The same targeted production Clippy command passed again (25.21 s wall), logged in
`.evidence/review1-clippy.log`. This round adds two lowest-layer registrations, no end-to-end
journey or large history. The fresh assigned gate supplies final-candidate validation and added-test cost.
No live T-924/T-933 import, renewal, recovery, pool work or owner input was performed; no
CLI/MCP surface, budget, assertion, quarantine or policy was relaxed.

Language-server coverage was incomplete: some changed files timed out during indexing or
were reported unlinked despite module registration; its cfg-tail diagnostics also disagreed
with the passing compiler. Targeted compiler/tests and the assigned gate supply the
corresponding checks; neither a timeout nor an unlinked hint was counted as a clean result.
