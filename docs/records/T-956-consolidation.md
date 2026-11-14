# T-956 — owner-authorized consolidation

## Scope and preservation

Owner answer `rq-85f28615411f0018d3870a6f430e524f` and the amended task's
**Owner-authorized consolidation (14 November)** section supersede the original
exclusion of the T-955 fixture. This round combines only:

- Original T-956 candidate `254175e72725e69f28845d6bd4cd75e2a7ffa2fd`: the
  [audited Darwin check-only repair](T-956-darwin-ring.md), unchanged.
- Original T-955 candidate `d6e12f7023328bcd1155901f959463bd020ca322`: its exact
  +13/-0 patch to `crates/corral/tests/assistant_entry_cli.rs`.

The fixture before consolidation has blob `50c11890e7d89ace14c55ec4b784a80595c41722`,
identical to T-955's unchanged base. After consolidation its entire file has blob
`9912cc4617a34f61bf81206dfa63a5362fa607f8`, identical to T-955's original candidate.
A byte comparison of the two patch diffs also passes. Preservation proof is attached
as `T-956:R1:a109a803-consolidation-preservation.log`.

The exhaustive symmetric-difference equality and class/refusal/workspace-unchanged
assertions are not weakened. The added rows classify `device list` as a read and
`device pair`/`device revoke` as refused. The Unix list path asserts its existing
missing-daemon response and absence of entry-guard refusal. The fixture clears its
environment, uses private HOME/XDG/workspace paths, and cannot access owner tooling
or credentials. No daemon, live API or paid service is started.

Original heads, branches, task/review histories and evidence are retained. No
T-957 repair, production feature/transport change, gate weakening, main-checkout
edit, T-955 retirement or landing is part of this round.

## Regression and targeted validation

Original unchanged-base red evidence is
`T-955:R1:f3c1666b-T-955-before.log`, also retained byte-for-byte as
`T-956:R1:f3c1666b-consolidation-original-fixture-red.log`. The exact set equality
fails with `["device list", "device pair", "device revoke"]`. T-956's original
full gate (`2449113-1794678322571568148`, log `cab497b4`, lines 512–530) later
passes Darwin preparation and fails on that same assertion. These failures and
receipts are preserved, not replaced with claimed successes.

With both patches combined:

- `cargo test --offline --locked -p corral --test assistant_entry_cli`: **3 passed**,
  no skips/filters; 4.40s test runtime (cold compilation separately took 3m45s).
- The existing Darwin isolation, installed-target and failure-propagation fixtures:
  **3 passed**, 8.363s. No changes to the gate repair or its tests were needed.
- Changed-file Rust LSP diagnostics: **clean**.

Both installed Darwin workspace/all-target checks are rerun on the final committed,
rebased candidate with the existing documented command-local overrides. Their logs
and the fresh receipted quick gate are attached to the task separately. Added test
registrations remain **zero**: this imports the exact extension of the existing
exhaustive journey, not another test or variant matrix.

## Fresh validation boundary

The original independent approvals (`T-955:R1:ecebf7ba-review-notes.md` and
`T-956:R1:769288c2-review-notes.md`) cover their original candidates only. Neither
is approval of this combined candidate. After current-main rebase, this round
uses a **new** receipted quick gate. PM must obtain fresh independent review and
the normal full landing gate before landing, and only then retire the duplicate
T-955. Worker does none of those PM-owned landing/retirement operations.

The main-checkout deletion disposition remains exclusively the PM's owner-approved
preserve/restore/reapply/byte-verify procedure `rq-775136d4df2088deb1ed1adb855e3e8e`.
Actual Apple native linking remains unverified; this does not change the Darwin
lane's Rust-only contract. T-957 and earlier baseline Ruff findings remain outside
this consolidation.
