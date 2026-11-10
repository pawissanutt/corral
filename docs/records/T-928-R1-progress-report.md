# T-928 R1 implementation checkpoint — not ready for completion

Base: `c7ef5815be29ad56e8f8662bffbb15d835081b95`. Approved scope: P1.
No gate, deployment or live A-77 action has been performed.

## Implemented in this worktree

- Separate versioned executor identities and terminal facts, published through the existing
  authenticated current-generation boundary channel. Ordinary `Settle` remains generation-bound.
- Managed MCP/LSP shutdown closes admission, joins startup, observes actual owned exit/group
  clearance, retains failures, and journals proof before canonical settlement. HTTP transport
  closure does not clear an unanswered remote request. Repeated close reuses the same outcome.
- Read-only exact preview and keyed outcome lookup; owner-terminal-only reconciliation using
  the existing unmanaged-owner guard and an opaque, digest-bound controlling-terminal challenge.
  The record-only transaction retains the existing dispatch/profile writer ordering through
  commit, changes only selected proved rows, and publishes immutable outcomes/receipt/audit.
- Canonical proof and complete retained lifecycle-journal recovery, plus a bounded legacy
  direct-bash verifier. Proof blobs are durable before ledger publication. Unknown resources
  stay outstanding; recovery never creates a source-drain attestation or renews authority.
- Thin CLI wiring and operator guidance. Generated CLI reference is still pending.

## Failing-first and targeted evidence

The original offline HostSession regression started real disposable MCP/LSP peers, then
failed because close produced **zero** settlements for its two resource admissions:
`artifacts/T-928/R1/39418fa0-lifecycle-red.log`.

Current checkpoint checks (logs under `.evidence/`):

| Check | Result |
| --- | --- |
| `cargo check -p corral` | Pass, no warnings (`checkpoint-cli-check.log`) |
| `cargo test -p corral-ops --lib assistant_effect_recovery_` | 3 pass, 0.09 s execution (`checkpoint-recovery-tests.log`) |
| `bun run typecheck` in `packages/corral-pi` | Pass (`checkpoint-pi-typecheck.log`) |
| Targeted host/policy/shell/MCP/LSP Bun files from P1 | 27 pass, 5.64 s (`checkpoint-pi-tests.log`) |
| Existing boundary and source-boundary regressions | 2 + 3 pass in earlier targeted run (`boundary-current.log`, `source-boundary-current.log`) |

The host regression now also refuses one settlement acknowledgement after real exit,
retains its terminal journal/admission, and proves a fresh policy remains fenced. The tiny
Rust fixture verifies subset-only reconciliation, durable lookup/replay, retained unknown
resources, stale source/task/evidence rejection, and unchanged source/task records.
Five added tests: three Rust service tests and two Bun tests. Added execution was about
1.2 s at this checkpoint; compilation is not suite execution. No load or provider tests.
Some LSP diagnostics timed out or reported unlinked files; targeted compiler/typecheck
results above are the fallback, not a claim of clean timed-out diagnostics.

## Still required

1. Answer Q1 with bounded canonical incident entries, then verify the generic legacy parser
   against the actual call/job/parent-chain shape. No supplied excerpt will be accepted as
   production authority; the service resolves the original bound canonical conversation.
2. Resolve Q2's actual task-association lookup contract. Current optional task selection is
   **not final**: omission must not bypass a known associated task/driver pin. No core index
   or task/driver mutation has been added to work around this gap.
3. Reconcile any retained research-helper findings (the host changed generation while
   read-only helper calls were outstanding), complete CLI docs, and rerun affected checks.
4. Commit/rebase the final candidate onto current main, invoke exactly the contracted quick
   gate through `corral_gate`, attach final evidence, and submit through normal review.

No owner-terminal ceremony, live A-77 application/restart/drain, installed-asset compatibility,
or final gate has been verified. Original T-924 takeover approval, history/scratch, two spent
repairs and its separately approved ONE extra third cycle have not been changed.
