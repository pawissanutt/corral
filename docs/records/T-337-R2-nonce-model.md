# T-337 R2: exact nonce derivative after integration

PM Q5 permits reconciling this diagnostic only after independently attributing both coefficients. No history budget, threshold, producer count, gate instruction or follow-up scope changes.

## Method

An offline `git archive 83dcc33e` under ignored `.evidence/nonce-baseline/` ran its original diagnostic: Worker **35**, AcceptPlanScopes **63**, PASS. Only observation-only test tracing was added. The archive used its own target directory, two build jobs, no live server, and no owner store. No baseline production source was changed.

The integrated path was traced separately while retaining those original expected values: it executes both cases and fails exactly at **39/67**. The test-only backend decorator records the actual returned canonical head/authorization documents and source stacks, independently of the work ledger. Trace selection matches the **complete nonce string**, not a prefix (setup authorizations hold a different, longer pinned nonce). All traces had zero omitted inputs. Temporary tracing and trace labels are removed from the candidate.

Raw logs:

- `.evidence/R2-nonce-baseline-exact-sites.log`
- `.evidence/R2-nonce-current-exact-sites.log`

## Independently itemized sites

Each counted document contains the changed nonce once. These counts come from the exact-string backend-input/source-site trace, not from dividing the measured byte counter.

| Nonce-bearing reads | Worker old | Worker integrated | Plan-scope old | Plan-scope integrated |
|---|---:|---:|---:|---:|
| dispatch | 1 | 1 | 1 | 1 |
| launch | 11 | 13 | 10 | 12 |
| plan target | 0 | 0 | 12 | 12 |
| tracker target | 3 | 3 | 9 | 9 |
| operating head reads | 8 | 8 | 16 | 16 |
| PM open-head reads | 4 | 5 | 4 | 5 |
| finishing-slot reads | 3 | 3 | 6 | 6 |
| role-session audit lookup | 0 | 1 | 0 | 1 |
| **Unattributed head reads** | **30** | **34** | **58** | **62** |
| authorization start load | 1 | 1 | 1 | 1 |
| authorization submission loads | 4 | 4 | 4 | 4 |
| **Unattributed derivative** | **35** | **39** | **63** | **67** |
| real resource sample head (FU-3/sample) | 1 | 1 | 1 | 1 |
| **Inclusive derivative** | **36** | **40** | **64** | **68** |

The four added decodes on **each** path are:

1. `launch.rs` fresh-session publication: `update_head` before/after head reads, **+2**. This preserves incoming-main conversation/session state.
2. `queue.rs` parked-work admission → `parked.rs:waiting` → `pm_next/heads.rs:open`, **+1**. The old path had queue capacity and finishing checks but not this additional parked-work scan. The corresponding stack contains queue.rs:647 and parked.rs:104 on the traced candidate.
3. `launch.rs` → `role_session.rs:log`, **+1**, preserving the truthful incoming-main role-session audit. The traced lookup is role_session.rs:562.

Other named source sites retain their original multiplicities. Both kinds launch one worker, so the same four added reads occur in both. The remaining old-path counts differ because plan-scope acceptance also validates the plan target and additional tracker/admission relations.

## Raw reads, cached reads and ledger cells

**Correction to the earlier provisional explanation:** there is no unmetered cached authorization fetch to subtract. All five nonce-bearing authorization fetches are inside the hot scope; `Store::submission_record` counts every returned document before deserializing it, including repeated/same-version reads. The apparent raw-versus-unattributed one-byte difference is the **one FU-3/sample head**, not an authorization-cache waiver. Raw input capture includes attributed work; the unattributed byte cell does not. Thus raw Worker delta 40 = unattributed 39 + FU-3 1, and raw plan-scope delta 68 = unattributed 67 + FU-3 1.

The final diagnostic hard-codes the independently itemized site multiplicities, sums them to its expected derivative, and tests nonce widths differing by **one and three bytes**. It retains the original comparison of every other unattributed/call cell, then additionally checks every cell: only unattributed document bytes, FU-3/sample document bytes, and their inclusive total may change by the predicted amounts. Other own-record/emitted/follow-up cells, hydration, calls, processes and effects remain unchanged. The action fixture still independently validates selected effects, resource preconditions, exact follow-up work and accounting identities.

This is an exact model of the preserved path, not a higher history-growth budget or permission to remove required reads. Final execution results are attached separately; the trace alone is not a passing result.
