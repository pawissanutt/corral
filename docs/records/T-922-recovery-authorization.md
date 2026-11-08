# T-922 — delegated PM recovery authorization

## Defect and bounded change

A settled Done candidate in NeedsMe was eligible for gate attribution in
`gate_triage::diagnose`, but its shared authorization check required executable
Repair eligibility. NeedsMe therefore blocked the very evidence operation needed
to resume the pipeline. Exhausted coding rounds could cause the same mismatch.

The effective policy now exposes a separate diagnosis assessment derived from the
same Repair delegation and current scope checks. It excludes only coding-round
eligibility and the NeedsMe executor stop. Terminal tasks remain refused. The
services still enforce their own completed-candidate/diagnosis stage.

`operating::require_diagnosis` uses that assessment both before reading evidence
and inside gate attribution's committing transaction. No authority flag, action
origin, grant digest, executor permission, gate result or retry counter is widened.
The ordinary Repair assessment still refuses NeedsMe and exhausted coding rounds.

## Adjacent-entry-point audit

- Gate triage and repair-ledger diagnosis share `require_diagnosis`; both receive
  the operation-specific fix. Co-work driver validation, holds and unresolved
  delivery/process/owner decisions remain in that shared check.
- PM workset Diagnosis options previously projected executable Repair authority.
  They now use the same diagnosis assessment as the service and managed CLI.
  The existing bounded repair test now starts from NeedsMe and exercises typed
  PM action, service readback, exhausted coding allowance and explicit renewal.
- Repair diagnosis preserves a running diagnosis's idempotent readback, but
  explicitly refuses to restart a completed diagnosis. Previously the generic
  NeedsMe refusal accidentally enforced this boundary. Its existing test assertion
  remains intact; no test was relaxed.
- The managed repair CLI already calls the same typed services, but reopened its
  Store without applying Entry attribution. It now applies the established Entry
  context before the operation, retaining actual PM rather than absent attribution.
- Assistant-shell command classification and owner-only confirmation/renewal
  guards are unchanged. This creates no new typed agent operation or bypass flag.
- Plan-review consent drift (T-358), destructive cleanup, dispatch/spend admission,
  independent review, acceptance and landing/FULL gates are unchanged.

## Retained invariants and pipeline behavior

Gate attribution still requires attached, readable evidence and a retained failed
attempt, validates a settled Done candidate, rejects concurrent writers/checks,
and rechecks candidate identity and current authority at commit. A different
attribution still requires explicit owner investigation. Unknown/mixed/flaky
attribution cannot authorize PASS; candidate findings still require a changed
candidate and distinct successful receipt.

Attribution only records evidence and returns the task to Review/Gating. The
existing autopilot chooses review, verification refresh or a real remaining block.
A matching repaired green receipt passes the existing triage check without another
coding run or diagnostic retry. Duplicate delivery after reopening records no
second attribution event and renews no allowance. The red receipt is retained.

## Verification and test cost

- One new service-level regression reproduces the exact lifecycle deadlock before
  the fix: `automatic Repair is not authorized ... task lifecycle is needs_me`.
- The same fixture covers attached evidence, out-of-scope denial, unchanged-
  candidate rejection, changed-attribution denial, PM event attribution, retained
  red/green receipts, restart/duplicate delivery, holds, pause and revocation.
- The existing repair-diagnosis test also failed first when its fixture moved to
  NeedsMe: no executable typed PM Diagnosis action was available. It retains all
  original allowance/renewal assertions.
- Constructor and historical policy-oracle updates account for the new assessment;
  they add no tests and weaken no assertions.
- Targeted outputs and the receipted quick gate are attached to T-922. A disposable
  typed-service/managed-CLI contract probe is retained as evidence, not another
  committed end-to-end test.

Added committed tests: **1**, using one small file-store fixture, with no live
engine, sleeps, load generation or large history. The five triage tests together
ran in **0.19 seconds** in the targeted check. Compilation is not suite time.

No live T-868 state was changed. This delivers the supported recovery path, not
acceptance of T-868 or permission to bypass its remaining pipeline stages.
