# T-758: exact nonce derivative after bounded head reuse

This reconciles the diagnostic in `action_budget_nonce.rs`, not its history budgets.
Owner-delivered PM answer Q3 authorizes this test-only correction after independent site
attribution, with exact widths 1/3, every-cell equality and effects/history assertions kept.
There is no production change, tolerance, budget increase or new test in this correction.
The prior model and its original derivation remain in [T-337's record](T-337-R2-nonce-model.md).

## Method and trace identities

The unchanged regression at candidate `fb223c6ff13128bf414c97904d53b7406657a671`
reproduces Worker **32 versus 39**. Temporary observation-only instrumentation of the
existing test backend decorator then captures each returned `head` or `authorization`
document containing the **complete JSON string** `"start-fixture-000"`, never a prefix.
It prints only input kind/key and source stack, not document content. Capture surrounds the
real action; setup and the separate expected-walk thread are excluded. A temporary case-order
selector exposes the plan-scope failure **58 versus 67** too. Both original assertions still
fail in the traces. All temporary instrumentation is removed before the final test run.

The current sums below count independently observed input/source sites; they are **not**
derived by dividing byte counters or by assigning the observed byte delta to an expectation.
The complete current captures contain 27 head + 5 authorization inputs (Worker) and 53 head
+ 5 authorization inputs (plan scopes). Each selected document contains the nonce once.

Evidence, retained as task artifacts:

- **O**, the old model's exact-site trace: `artifacts/T-337/R2/c94beae1-R2-nonce-current-exact-sites.log`.
  Its Worker/plan-scope sums are 39/67 unattributed; its additional resource-sample head is
  explicitly FU-3 and excluded from those sums. This is the historical T-337 trace, not a
  newly executed baseline or a claim about the current scalar resource path.
- **W**, current Worker: `round4-worker-trace.log`, in
  `artifacts/T-758/R1/d4b216b3-round4-nonce-traces.tar.gz`.
- **P**, current plan scopes: `round4-plan-trace.log`, in the same archive.
- `round4-trace-sites.txt` lists every current selected input and source locations.
  `round4-independent-counts.txt` sums those source categories.
- The final round-4 evidence archive also includes `round4-site-comparison.json` and its
  standalone extraction script, mapping each site below to raw trace line numbers.

## Each reused nonce-bearing decode site

Counts are per action and per extra nonce byte. W/P means Worker/AcceptPlanScopes.
The source anchors identify the call path, not new code in these files. All four repeated
PM-open reads still execute their consumers but reuse the checked BoardDelta/pass inputs.
Finishing-policy point heads reuse the already-decoded head only at a fresh matching pinned
sequence. Reverse-edge queries remain real queries. Zero means **no new decode**, not a
removed policy check or uncharged deserialization.

| Decode site / caller | W old→new | P old→new | Old trace O lines (W / P); current trace |
|---|---:|---:|---|
| PM open, initial launch capacity: `slots.rs:142`, via `queue.rs:644` | 1→1 | 1→1 | 2479 / 13559; W:2578, P:5834 |
| PM open, parked waiting: `parked.rs:104`, via `queue.rs:647` | 1→0 | 1→0 | 2587 / 13667; absent in complete W/P captures |
| PM open, finishing first: `slots.rs:181` | 1→0 | 1→0 | 3041 / 14493; absent in W/P |
| PM open, dispatch priority: `dispatch_policy.rs:16` | 1→0 | 1→0 | 3149 / 14601; absent in W/P |
| PM open, dispatch capacity: `slots.rs:142`, via `dispatch_policy.rs:128` | 1→0 | 1→0 | 3259 / 14711; absent in W/P |
| Finishing publication task point head: core `operating/publication.rs:45` | 1→0 | — | 2695 / —; absent in W |
| Finishing publication plan-child point head: core `operating/membership.rs:161`, via `publication.rs:122` | — | 1→0 | — / 13775; absent in P |
| Finishing publication reverse-edge heads: core `operating/membership.rs:170`, via `publication.rs:122` | — | 1→1 | — / 13901; P:5950 |
| Finishing publication selected-child point head: core `operating/publication.rs:181` | — | 1→0 | — / 14025; absent in P |
| Finishing task-membership point head: core `operating/membership.rs:217` | 1→0 | 1→0 | 2811 / 14145; absent in W/P |
| Finishing selected-child membership point head: core `operating/membership.rs:273` | — | 1→0 | — / 14261; absent in P |
| Finishing explicit-task-policy point head: core `operating.rs:604` | 1→0 | 1→0 | 2927 / 14379; absent in W/P |

All finishing rows are reached through ops `slots.rs:176` → `effective_operating_policy`.
The retained plan-scope reverse-edge row is why its finishing coefficient is **1**, not 0.
The raw old/current stacks make this distinction explicit rather than assuming all reads
through a cache-owning caller are free.

## Complete independent model

| Nonce-bearing category | Worker old | Worker current | Plan-scope old | Plan-scope current |
|---|---:|---:|---:|---:|
| dispatch | 1 | 1 | 1 | 1 |
| launch, including fresh-session owning updates | 13 | 13 | 12 | 12 |
| plan target | 0 | 0 | 12 | 12 |
| tracker target | 3 | 3 | 9 | 9 |
| operating heads | 8 | 8 | 16 | 16 |
| PM open heads | 5 | 1 | 5 | 1 |
| finishing slots | 3 | 0 | 6 | 1 |
| truthful role-session audit | 1 | 1 | 1 | 1 |
| authorization start load | 1 | 1 | 1 | 1 |
| authorization submission loads | 4 | 4 | 4 | 4 |
| **Unattributed derivative** | **39** | **32** | **67** | **58** |

Thus Worker loses four repeated PM head decodes and three finishing point-head decodes:
**39 − 4 − 3 = 32**. Plan scopes lose the same four repeated PM head decodes and five
finishing point-head decodes: **67 − 4 − 5 = 58**. Every other category is unchanged.
All five nonce-bearing authorization reads remain counted, including repeated fetches;
there is no authorization-cache discount. The current FU-3 paths read scalar headers, so
their nonce derivative remains **zero**, as already required by the unchanged diagnostic.
T-337's historical extra FU-3 sample-head decode is not a T-758 removal or a model discount.

## Preserved verification contract

The test retains the same real worker/plan-scope actions and all three nonce inputs, with
extra widths **1 and 3**. It still requires exact changes in unattributed/total decoded bytes,
exact equality in **every other cell**, no measured effect/precondition/follow-up failure,
and the original unattributed/call-cell comparisons. No history-growth assertion, fixture
size, byte threshold, process/hydration count or FU-3 scope is changed. Existing history
budgets remain independent tests; this diagnostic predicts width sensitivity, not permitted
history growth. The final post-rebase quick receipt and targeted red/green logs are attached
separately. Corral alone owns the full landing gate.

The focused unflagged check is
`cargo test -p corral-ops --lib submission_paths_tests::action_budget_nonce::start_attempt_width_explains_worker_and_plan_scope_byte_drift -- --exact --nocapture`.
It fails before the correction (Worker 32/39), then passes **1/1 in 17.46 s** after it,
reporting independently predicted derivatives 32/58 at widths 1/3 and every other cell
unchanged. A byte comparison confirms the entire `#[test]` body is unchanged from the red
source; the observer file is also restored byte-for-byte. Changed-file rust-analyzer
reports **0 diagnostics, complete=true**. This round adds **zero tests**; it introduces no
new fixture work. Final post-rebase gate timing is reported in its canonical receipt.
