# T-853 — C2 context and lossless snapshot foundation

## Delivery boundary

Implements T-807 design §§4–5 C2 service foundations in
`crates/corral-ops/src/assistant/workflow/handoff/`. Public `assistant_handoff`
remains unavailable: C2 does **not** create a task/outbox, transfer `ScopeOwner`,
start a worker, grant execution, adopt a plan, or claim a native session fork.
C3 owns those lifecycle effects and their shared acceptance proof. PM confirmed
this staged split in `T-853/R1/Q1` revision 2, explicitly retaining all joint acceptance.
Immutable preview descriptors and their source/intent lookup are not journals or
ownership state. `assistant-handoff-journals` is the **one authoritative transfer
journal**, referenced through the existing workflow handoff package.

The package and journal extend T-751's container/artifact model and retain T-752's
fail-closed phase/epoch fence. There is no second author or completion runner.

## Interface for C3

1. Read canonical container and its `Expected` workflow/revision/epoch/plan CAS.
   Resolve the selected registered project/scope and genuine owner goal using
   C3's authenticated transition. Drain active/uncertain writers first.
2. Outside assistant/board locks, `prepare` validates required workflow artifacts,
   context, and the recorded source scratch. An existing project scratch cannot
   be silently omitted. It captures and verifies restoration into a disposable
   new repository. Research/plan-only without a scratch remains explicitly code-free.
3. Inspect `Prepared::id`, `package`, and `preview`; or use `read_package` /
   `preview_package`. Preview publishes immutable bytes but changes no workflow
   ownership, phase or authority. Its lookup is bound to source CAS and exact
   project/scope/goal/context/source intent, atomically with the immutable preview.
   A repeated intent recovers the same checkpoint; changed source bytes refuse
   rather than silently recapture work after a lost reply. Different choices have
   different digest-bound identities, but only one can reserve a source revision.
4. Call `reserve` **inside the existing authenticated assistant-container transaction**,
   alongside its receipt. CAS, journal insertion, phase `HandoffPreparing`, handoff
   reference, revision and epoch publish together. The owner remains Local. A lost
   reply reads/reuses the same reservation instead of creating another outcome.
5. C3 must revalidate the pinned source after closing admission and before its
   held-task/ownership commit. Use `recover` after host death: it reads the exact
   package, verifies required bytes/restoration and source identity; it never
   silently recaptures different work or falls back to an empty repository.
6. C3 uses the stable journal ID as its task/source/outbox marker, persists the
   selected goal/stop, reconciles uncertain task publication, and commits actual
   ownership using normal task/hold/admission services. This is not implemented
   or simulated by C2. C3 must retain the source checkout through adoption proof
   and explicitly release its handoff reference before normal disposal.

Scratch discard/GC and co-work discard consult the current handoff reference and
refuse `handoff_source_retained`. This is an accident guard, not containment of
arbitrary same-user filesystem edits. External edits require refresh/reconciliation;
no filesystem-wide atomic snapshot is claimed.

## Representation and refusal policy

- Typed Task / Conversation / Minimal context. Task includes goal, constraints,
  decisions, progress, findings, failed approaches, assumptions, evidence, questions
  and next action. Minimal still includes current work, mandatory blockers/findings,
  questions and next action. No discussion history is accepted outside Conversation.
- Plan preview exposes the exact revision/digest/stage and pinned review artifacts.
  Local-review freshness is explicitly unknown for opaque historical artifacts:
  T-753's lifecycle is still unlanded, so no trusted target-revision binding or
  freshness/approval is invented. C3/T-753 must supply actual lifecycle metadata.
- Conversation accepts only a closed visible user/assistant-message schema, one
  contiguous range, explicit branch/checkpoint and exclusions. Preview reports the
  selected range, archived bytes/messages and actually loaded messages/bytes.
  Remaining history is archived, not claimed already read. No tool-call objects,
  provider signatures, bridge credentials or permission state are imported.
- Brief limit 32 KiB; selected-history archive 1 MiB; visible prompt 64 KiB plus
  at most 8 KiB of service-owned lineage/work/plan facts. Native fork is explicitly
  unsupported; C3 allocates a fresh continuation with the appropriate role.
- Full verified bundle preserves base/HEAD/source-branch reachability, including
  detached HEAD with a stranded branch. A synthetic index-tree commit retains
  staged blob objects. The original **raw index bytes** are captured separately.
- Tracked work and non-ignored untracked regular files are stored as raw bytes
  and Unix modes, including binary, CRLF, deletion and index-only content.
  No checkout, clean/smudge filter, source index refresh or source ref update runs.
  Ignored outputs are listed as excluded and stay in the source.
- All Git calls clear inherited environment/control credentials, global/system
  config, hooks and fsmonitor. Restore creates a fresh empty-template repository;
  source Git config/credential helpers are not copied. Git stdout/bundle reads,
  file reads and blob loads are capped; file access uses retained no-follow handles.
- Limits: 16 MiB per required regular file/index, 64 MiB total bundle/index/work,
  4096 required paths, 1 MiB package manifest. Oversize, unreadable/missing required
  content, special/symlink paths, submodules, sparse/split/unmerged indexes,
  unsupported paths/platforms and source changes are named refusals, not omissions.
  In-progress Git administrative operations cannot be represented by code layers
  alone and must refuse as well.
- Restore validates paths/digests, complete Git objects and exact index/work manifest
  in a private directory, then publishes with atomic no-replace rename on Linux/macOS.
  Other platforms refuse rather than claim exact restoration. An existing destination
  is never a fallback restore target. Interrupted temporary repositories are not seeds.

## Acceptance evidence

Primary log: `.evidence/c2-workflow-final.log` (14 workflow tests, including 7 new
C2 cases; all passed in 8.14 s, including the expanded slow publication matrix). `.evidence/c2-merge-red.log` records the failing-first
regression: a resolved-but-uncommitted merge was silently accepted. The same refusal
case passes after checking in-progress Git administration and resolve-undo state.
`.evidence/c2-lost-preview-red.log` records a second failing-first variant: retry
silently captured changed source bytes after preview publication. Stable intent
lookup now reconciles the pinned preview and names the changed source instead.
The receipted gate result is attached separately when complete.

| Criterion | Observable C2 proof | Explicit shared boundary |
|---|---|---|
| A05 | `handoff_context_modes_select_only_visible_history_and_disclose_paging` checks structured continuation facts; package/readback tests compare source identities and pinned artifacts. Non-conversation schemas reject history. | Real continuation launch/role wiring is C3/C4. |
| A06 | Selected contiguous history only, honest archive/load counts, closed credential/tool fields, minimal mandatory F1 and code-presence tests. Fresh Git config lacks source credential helper. | No live/native fork is exposed. Provider/launch policy stays with lifecycle owner. |
| A07 | One disposable Git restoration journey compares original raw index/work manifests before/after; restores binary staged/unstaged bytes, CRLF, index-only addition/deletion, untracked executable bytes and exact base/HEAD/stranded-branch commits. Ignored source bytes remain intact. | C3 adopts the verified seed into the normal task worktree and retains a single driver. |
| A08 | Sparse oversize fixture, special symlink, missing required bundle, deterministic concurrent edit and stale selection refusals; failed restore leaves no destination. Code presence cannot be omitted, and reserved source disposal refuses. | C3 repeats source validation after fencing and before ownership publication. |
| A14 | Explicit slow crash matrix covers interrupted capture, early/late content attachment, verified restore, package preparation, atomic preview/intent publication and reservation staging. Reopen plus committed-lost-ack recovery yields one preview/intent and one journal; zero tasks/dispatch, Local owner retained. | Task-created-before-death, held task/outbox publication, actual ownership commit and notification recovery belong to C3; no claim those effects were tested here. |

Inherited T-754 A8: research, draft and advisory-reviewed plan metadata are
parameterized through package/readback without importing approval; dirty partial
layers are proved by the single Git journey; completed clean source is preserved
by the code-presence/recovery contract. Missing attachment bytes refuse before
ownership. A late workflow/findings revision invalidates the earlier selection.
Actual stage-aware task continuation, late running-review routing, competing-writer
admission and task/outbox death boundaries remain C3's shared proof, not closure
of predecessor acceptance based on this report alone.

## Checks and cost

- `cargo test -p corral-ops assistant::workflow --lib -- --nocapture`: 14/14 passed,
  8.14 s suite time (compilation separate). Seven new C2 tests; one publication
  crash matrix is explicitly slow, with handoff-module owner trigger.
- `cargo test -p corral-ops --test assistant_scratch c13_discard_saves_commits_staged_unstaged_and_untracked_work_restorably -- --nocapture`: existing discard/restore regression passed (0.30 s), exercising ordinary cleanup with the new retention check.
- `python3 scripts/test_gate_policy.py`: passed; validates slow membership/owners.
- `cargo fmt --all` and `git diff --check`: passed on the checkpointed tree.
- Changed-file LSP requests were made; rust-analyzer repeatedly exceeded its fresh
  indexing deadline. Successful targeted compilation/tests are the fallback,
  not a claim of clean fresh LSP diagnostics.
- Discovery helper 1 was partial at its turn limit (workflow/evidence/CAS inspected,
  host transaction/T-752 not traced). Helper 2 completed the scratch preservation
  trace. Important cited artifact/CAS/Git seams were read directly. T-752's landed
  spec/result and boundary implementation were checked by the worker.
- `cxm context` and bounded Corral search timed out; neither is acceptance evidence.
- No paid provider call, live owner pane, host load generation, load test, or manual
  gate tier was used. The run's receipted quick gate is the only worker gate.
