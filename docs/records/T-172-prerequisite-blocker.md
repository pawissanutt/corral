# T-172 prerequisite blocker and scope restoration

The PM comment at `2026-10-18T19:32:15.280795648Z`, confirmed by the user's
subsequent direction, assigns the artifact-ordering gate defect to **T-181**.
T-172 remains a research-only spike. Its recovery report is not permission to
repair source code or recover live T-137.

## Evidence retained

- Original recovery report:
  `T-172:R1:7eec8e6c-T-172-recovery-report.md`, committed in `8578503`.
- Failed native gate:
  `T-172:R1:f9d0c6e2-T-172-R1-3023417-1792351636277100406.log`.
- Change request:
  `T-172:R1:1ee01671-changes-round-1.md`.

That gate failed only
`importance::tests::order_follows_stored_tiers_overrides_and_report_before_summary`
at `importance_tests.rs:73`, with log before evidence instead of evidence before
log. The result remains failed. No research-report defect or successful T-137
recovery is inferred from it.

## Out-of-scope attempt withdrawn

Before reading the PM clarification, this agent incorrectly treated the automated
gate rejection as authorization to change a test fixture. Commit `ee01073` added
four timestamp-fixture lines and a report addendum. Those four lines and that
addendum are now removed; the original research report is restored byte-for-byte.
The delivered T-172 tree has no source/test delta against its integration base.
History is retained rather than rewritten.

The temporary red/green logs and amended report remain historical evidence,
not accepted T-172 source work. The owned gate on `ee01073` completed with exit 0
before its cancellation request took effect. Its receipt is superseded: it
does not clear the failed gate for T-172's restored no-code tree. No unchanged
gate is being retried to obtain a lucky pass.

## Current blocker

T-181 was read back as Running. T-172 must wait for the prerequisite to be
actually resolved and then continue only through normal review/gate policy.
No new `corral done`, source repair, assertion change, budget reset, or live
T-137 recovery is authorized here.

A read-only subscription watches T-181's Done event. A wake is a cue to inspect
the actual result, not permission to bypass review or accept the research task.
The portable checkpoint retains this prerequisite and resolves the owned
fixture/gate cleanup obligations. The original conditional recovery
recommendation remains unchanged.
