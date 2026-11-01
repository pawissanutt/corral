# T-337 R3 exact-byte fixture audit (owner Q8)

## Scope and method

Compared added/modified Rust paths against storage-v2 `83dcc33e`, including integrated main tests, not just R3's edits. The candidate inventory (`.evidence/R3-exact-byte-budget-clock-inventory.json`) contains 92 paths selected by decoded-byte/all-counter/remainder/variant assertion markers. Inspected fixture construction, measurement boundaries and common helpers, including Core store_support/native records, native root/task/action budgets, TUI budget_seeded/hot_budget_fixture, and the Ops delivery/attention/lesson/question fixtures. Clock markers alone are not a safety proof: a pin after creation can retain live status_since, and a raw Jiff call bypasses both pins.

## Consolidated findings and changes

- `obligations_sync_differential_tests.rs` already pins thread clock before Store::init and Store clock before creation. Actual failed gate numbers were **132702/132726**, not the abbreviated 13270/13272. The escaping input is **obligations.rs's raw Timestamp::now() passed into notice preparation**. Q8 authorizes changing only that argument to `ctx.store.now()`: ordinary production still samples wall time, while fixtures use the existing seam. Added explicit thread/Store/status_since pin assertions and post-operation claim readback; every original counter, deadline, oracle and effect assertion is unchanged.
- `native_head_mutations.rs`: its three growth fixtures now pin Store time before creating rich task/index state, including the multi-mutation case that reads a just-written head. Existing manually fixed created/updated/status_since values alone did not bound subsequent mutation timestamps.
- `evidence_artifacts.rs::keyed_work`: pin Store time through its existing fixture configuration callback, before creation/attachments/pins. Fixing Artifact.added alone did not fix other canonical metadata inputs. Existing exact read counters and existing write-counter selection are unchanged.

These are the only additional actionable input holes identified in the sweep. Earlier R3 proven fixes remain in artifact_facts, store_heads, list_cols, owner_growth and lesson_rows_preparation. There is no blanket clock rewrite, budget change, sleep, retry, threshold change or special attribution exemption.

## Why other apparent unpinned cases did not require edits

- Keyed Core/native/attention/role-session reads often load heads **before** measurement and decode only explicitly timestamped child rows during it. Fixed serialized child input is what matters, not the presence of an unmeasured wall-clock creation elsewhere in the test.
- Root PM/standalone inbox/assistant known-cost tests independently size the actual canonical documents and charge their attributed cells exactly. Their assertions are not an equality claim between different-sized documents.
- Some tests reuse the same immutable fixture/current rows for both history variants (question correlation, history/tracker/promoter reads); shared input timestamps remain identical. Plan-pilot freezes one clock value across its sequential variants, rather than comparing two independently sampled fixture clocks.
- TUI board fixtures use `budget_seeded`'s clock before creation; warm/cold sync fixtures initialize/reopen with `fixture_pin_now` before use. Their apparent lack of local pin calls is supplied by their helper, not an exemption.
- Native action/root/task budgets already pin their measured thread clocks, and their independent own-record/emitted/follow-up models account for canonical rows rather than waiving them. Unmeasured transcript/event fixture times were distinguished from decoded budget inputs.

## Verification

Targeted terminal results (`.evidence/R3-byte-clock-sweep-green.log`): native_head_mutations **4 PASS**, obligations differential **2 PASS**, keyed artifact history budget **1 PASS**. The differential fixtures intentionally lack a real terminal engine: notification failures are part of their persisted-outcome oracle, not live transport proof.

Negative control: restoring the old raw Timestamp::now argument makes the new claim readback fail deterministically (actual wall time versus pinned 2026-10-19), regardless of whether fractional widths happen to coincide. Log `.evidence/R3-obligation-raw-clock-negative.log`. The raw call is restored to the fix before final checks. No negative-control source or instrumentation is retained. Final quick receipt remains required for package acceptance.
