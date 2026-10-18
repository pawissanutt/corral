# T-172: T-137 pending-reply recovery spike

Date: October 18, 2026. Investigation base: `ebd07323fa4471040a1b365e92b04c51578b323c`.
This is a read-only diagnosis and an owner decision, not authorization to recover T-137.
No T-137 reply, keypress, run/config/grant change, budget reset, pending-text deletion,
or completion claim was made. The initial spike changed documentation only; the
subsequent gate-repair request corrects one timestamp-dependent test fixture, as
recorded below.

## Recommendation

**Do not resend T-137 in its present state. There is no supported CLI command that
can safely finish this opaque ordinary-composer delivery and reconcile its pending
receipt without an additional owner decision.** T-170's native-question path cannot
answer an already expired question or retroactively bind an ordinary preparation.

Checkpoint-guarded fresh retry is a supported *replacement-run* operation, not a
delivery repair. It is a reasonable owner-selected alternative here because the
worktree is clean and the worker reports no implementation. It is **not ready to
execute**: R1 has no checkpoint, and its existing pending reply prevents retry from
asking the worker for one. Do not run retry merely to probe this guard.

The precise owner decision is:

> Authorize an operator to reconstruct a truthful, current R1 handoff through the
> checkpoint CLI, preserve the old pending reply as historical undelivered input,
> and replace R1 with a fresh `gpt-sol` run which reads the existing durable scope
> comment; or retain R1 untouched and commission a supported delivery-reconciliation
> feature. Replacing R1 closes its pane and discards its unsent composer. It also
> resets task review rounds through the normal retry operation. This spike does
> not authorize those effects.

If the owner does not authorize replacement and its effects, leave T-137 blocked.
Do not clear its composer, manually submit it, remove pending text, disable the
checkpoint requirement, close the pane to evade the reachable-worker guard, or use
an unguarded retry as a workaround.

## Historical evidence read before new probes

Read these stored T-170 reports through `corral artifact show`:

- `T-170:R1:92d8e43f-review1-done-summary.md`: final review-fix completion report.
- `T-170:R1:4f4824d4-review1-fix-report.md`: completed-preparation boundary and
  unbound-native fail-closed guard.
- `T-170:R1:1c657282-completion-report.md`: earlier acceptance mapping and private
  ordinary/native QA. Its committed candidate predates the final review fix.

They establish request-bound native delivery, same-request lost-ACK reconciliation,
and intentional refusal of opaque ordinary pastes. They do not claim that the
historical T-137 pending reply was repaired. Existing evidence was searched first
with `corral search "T-137" --in all --json`, then narrowed to T-170.

## Current read-only findings

Raw CLI evidence is `.omo/evidence/T-172/live-readonly.log`; the native lifecycle
excerpt is `.omo/evidence/T-172/native-question-evidence.json`.

1. `corral show T-137 --json` reports Running, current R1, `agent_state=idle`,
   `checkpoint_required=true`, and the complete pending owner note.
   It is 1,335 UTF-8 bytes, SHA256
   `66c5ad215cf54bba1ae5f12d1cc81cb620f4e1fb453e682efc5d4f2d3c561c85`.
   Its existing PM comment dates to `2026-10-18T05:42:57.324560362Z`.
2. `corral log T-137 --all --json` records the latest preparation at
   `2026-10-18T19:12:52.196863169Z`: R1, pane `w5S:p1`, identical text hash,
   **`question=null`**. This is an ordinary route, not a native request binding.
3. The reply receipt at `2026-10-18T19:12:53.13971196Z` says
   `via=send_text`, `submitted=false`, `attempts=0`,
   `error="pasted text never appeared in pane input"`.
   Two earlier receipts at 05:42:58 and 05:43:23 UTC also report false/0.
   None is proof of successful submission. Zero attempts counts Enter calls,
   not pastes; it must not be described as "nothing was written".
4. `herdr agent get w5S:p1` identifies OmO, task T-137, run R1, and
   `/home/pawis/miti-workspace/corral/.worktrees/T-137`. Read-only process
   inventory identifies foreground PID 1239480, `bun.exe`, running that OmO
   task in the same directory. No foreign stuck process is established.
5. `herdr pane read w5S:p1 --source recent-unwrapped --lines 90 --format text`
   shows **`[paste #3 1335 chars]`** in the ordinary composer. Its length agrees
   with the pending note, but an opaque marker cannot prove byte identity.
   The ordinal does not prove three copies or three submissions. No further
   reply was attempted by this spike.
6. Native session `01a14bb0-5158-77cd-84ff-653e05c2a05f` has one question:
   `call_QvpZ60ZfAzcuchrB0ljYtZyv|fc_0850dbf46a2be26d016ad45bdbb5a087d18eb89ac435d51522`.
   It opened at `2026-10-18T05:40:45.929Z` and settled **`timed_out`** at
   `2026-10-18T06:10:45.936Z`. The following user frame explicitly says no
   answer arrived and the gated action must remain pending. The transcript ends
   with the worker stating that scope is unresolved and no source changes or
   completion claim were made. No later native question is recorded there.
   This is durable lifecycle evidence, not a claim that a terminal menu was
   answered. No native response command or endpoint mutation was sent.
7. `corral run checkpoint T-137 --json` and `run progress` both report
   `checkpoint_status=missing`, `checkpoint_reasons=["checkpoint_missing"]`,
   no progress, and no checkpoints. Observed identity:
   HEAD `911486b4347027f522e155c69812405f10cb3759`,
   current integration base `ebd07323fa4471040a1b365e92b04c51578b323c`,
   tree `3fcaf36fa8b026bce4d5e6c91834b3ff321417bf86ed292495020fbe344cae0a`,
   spec `e0ebff9942da4c621a17442e5540f2dc5e9cc9e7a4c128c36827e98d80f0b8fe`.
   Read-only Git status is clean on `corral/T-137`.
8. Installed and shared release binaries both hash to
   `5c9150175d63421a5bd96735c7082d3888047797d2d230046475995107b80b52`.
   Reinstalling the identical binary is not a recovery.

## Safe refusal versus product defect

The current refusal is correct fail-closed behavior. In
`crates/corral-herdr/src/delivery.rs:65-84`, terminal delivery refuses a recognized
opaque composer before another paste, and sends no Enter unless complete text is
observed. A paste which becomes opaque during verification can therefore yield
the recorded "never appeared" error without any Enter. The error describes failure
to observe the full message, not proof that no text reached the editor.

The present opaque marker would prevent another native prompt, paste, or Enter
through the ordinary preflight. The exact screen seen by the historical polls was
not retained, so this spike does not assert the precise redraw or collapse timing
that produced the 19:12 receipt.

`crates/corral-ops/src/question_reply.rs:83-183` reuses the durable preparation
only for its run/pane/text and never converts an ordinary preparation into an answer
to a new question. The current preparation has no question; the old question has
timed out. T-170's request-bound native happy path and lost-ACK reconciliation are
therefore inapplicable. There is no native successful-answer receipt to reconcile.

The product limitation is the absence of an authoritative ordinary-composer identity
or supported pending-delivery resolution/cancellation operation. The CLI's generic
"retry with --resend" advice is not a complete recovery runbook for an opaque paste.
This is an operational recovery gap, not evidence of a regression in T-170's native
answer transport. A feature proposal must preserve duplicate avoidance and historical
pending text rather than weakening verification.

## Commands and preconditions for the PM/operator

The readback commands below reproduce this investigation. Mutating commands are
conditional recommendations and were not executed against live T-137. Run from
the corral project or use the explicit workspace root shown here.

### Safe readback now

```sh
/home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace show T-137 --json </dev/null
/home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace log T-137 --all --json </dev/null
/home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace run progress T-137 --json </dev/null
/home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace run checkpoint T-137 --json </dev/null
HERDR_SOCKET_PATH=/home/pawis/.config/herdr/herdr.sock herdr agent get w5S:p1 </dev/null
HERDR_SOCKET_PATH=/home/pawis/.config/herdr/herdr.sock herdr pane read w5S:p1 --source recent-unwrapped --lines 90 --format text </dev/null
```

Require the same current run and owned pane. A status or identity change invalidates
this recommendation until re-inspected. Do not use `sync` as a read-only probe.

### Same-run delivery

The supported command is `corral reply T-137 --resend`, but **its preconditions
do not hold now**. An ordinary resend needs verifiable full composer text or a known
empty composer with no ambiguous previous submission; the present opaque marker
proves neither. A native resend requires a durable matching preparation plus an
unchanged request or exact matching settlement; neither exists for this reply.
Manual Enter is not a supported reconciliation and can leave canonical pending text
available for later duplication. Do not send another comment with `--send`.

### Owner-authorized replacement, conditional

Only after the owner chooses replacement and authorizes its side effects:

1. Preserve this evidence and the complete existing pending note. Re-read current
   task, Git status, progress, and checkpoint identity. Audit any actual outstanding
   waits, external processes, and cleanup obligations; absence of a checkpoint is
   not proof that none exist. Do not claim worker progress or accepted criteria.
2. Have the worker, or the explicitly authorized operator reconstructing its
   handoff, prepare `/tmp/T-137-R1-recovery-checkpoint.json` with the current
   `task`, `run`, `spec_revision`, `head`, `base`, and `tree` from readback.
   Use current checkpoint revision (`expected_revision=0` only if still missing)
   and current progress revision (`progress_revision=0` only if still absent).
   Record the expired question, unsent 1,335-byte note and its hash, the decision
   to replace rather than submit it, and the requirement for R2 to read the
   existing durable scope comment. Include every actual wait/resource and its
   owner/cleanup duty. Empty criterion/evidence arrays assert no acceptance.
3. Save through the supported CLI, then re-read compatibility:

   ```sh
   /home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace run checkpoint T-137 --file /tmp/T-137-R1-recovery-checkpoint.json --json </dev/null
   /home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace run checkpoint T-137 --json </dev/null
   ```

   Require `checkpoint_status=compatible`, R1 still current, and no changed
   obligations, progress, spec, HEAD, base, or tree. Stale writes require
   reconciliation, not blind revision replacement.
4. Only with the same owner authority and valid checkpoint:

   ```sh
   /home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace retry T-137 --run R1 --profile gpt-sol </dev/null
   /home/pawis/.cargo/bin/corral --root /home/pawis/miti-workspace show T-137 --json </dev/null
   ```

   `gpt-sol` keeps the existing strong implementation profile rather than
   silently changing models. Normal routing/admission checks remain applicable.
   Retry may queue; do not assume that a worker launched from its exit status.
   Verify R1 is superseded, old pending text is retained on R1, the old pane is
   closed, and the new run or queued launch identity is explicit. A new run
   starts without `pending_reply`; its prompt includes the durable comment.
   Do not resend the R1 note into R2. No inherited checkpoint or historical
   evidence proves R2 acceptance or completion.

Why this sequence: `dispatch.rs:696-784` checks the source run and a reachable
worker's checkpoint before closing that pane. `resume.rs:229-252` refuses a new
checkpoint request when a different pending reply already exists, with
`pending_reply_before_checkpoint_request: deliver the existing reply first`.
With a valid current checkpoint, that request branch is unnecessary. Normal retry
preserves R1's run record, closes the reachable worker, resets task review rounds,
and starts a fresh run from its base. It is not an in-place resume of R1's code.
Do not select it if uncommitted work or unidentified resources cannot be preserved.

## Verification and limits

Existing deterministic private fixtures are used, not a new live T-137 experiment:

- `cargo test -p corral-herdr --lib delivery::tests::`: 17 passed; never-visible paste
  produces false/0 and no Enter; collapsed markers refuse duplicate writes.
- `cargo test -p corral-ops --test progress_ops delivery::`: 3 passed; pending owner input
  prevents a checkpoint request and leaves the worker and store unchanged.
- `cargo test -p corral-ops --lib question_reply::tests::`: 16 passed; request identity,
  settlement, unbound-native guards, lost ACK, and ordinary/native separation.

All three commands completed together with exit code 0. The fixture log and the
required initial/final isolated full-gate logs are attached separately with their
actual results. The initial spike changed no source or test code; the subsequent
gate repair below changes only the failing test fixture and this report.
Private helper environments clean only their own servers, HOME/XDG directories,
and fixture resources.

Not verified: a successful live recovery of T-137, the precise historical screen
redraw timing, a new real OmO model roundtrip, or owner authorization to replace R1.
None is claimed. The acceptance result is a precise owner-only decision with
supported conditional commands, not a repaired T-137.

## Review round 1: deterministic gate fixture

Autopilot rejected `8578503` because
`importance::tests::order_follows_stored_tiers_overrides_and_report_before_summary`
returned `["report", "summary", "promoted", "log", "evidence"]` rather than
`["report", "summary", "promoted", "evidence", "log"]`. The original failed
gate is retained as `T-172:R1:f9d0c6e2-T-172-R1-3023417-1792351636277100406.log`;
the change request is `T-172:R1:1ee01671-changes-round-1.md`.

The fixture assigned real wall-clock timestamps during successive inserts, then
expected evidence to be strictly newer than the log. Both have transient tier and
the same kind rank. `cmp_importance` sorts their actual timestamps newest first;
`order_artifacts` deliberately preserves input order when timestamps tie.
The rejected run did not log its timestamps, so it cannot establish whether they
tied or moved backward. A temporary equal-timestamp fixture reproduces its exact
failure deterministically, with exit 101, in `review1-tied-timestamp-red.log`.

The requested correction assigns explicit increasing timestamps to the five
fixture artifacts before sorting. The assertion is unchanged, including report,
summary, promoted artifact, evidence, and log ordering. Removing newest-first
sorting would still fail the assertion. No production comparator, artifact
storage, delivery behavior, or T-137 state is changed. No sleep, retry, skipped
test, or relaxed assertion is introduced.

Corrected fixture tests, diagnostics, and the committed/rebased full-gate receipt
are attached separately. The research recommendation and its owner-only boundary
remain unchanged.
