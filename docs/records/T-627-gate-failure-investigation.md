# T-627: Pi-native versus OmO gate failures — preliminary investigation

## Conclusion

The available review does **not establish a cause** for the reported 29% versus 19% gate-failure gap. The reported 42 versus 157 runs and first-review acceptance figures are the original stats snapshot, not a matched experiment. In the present configuration the two profiles also differ in reasoning effort, and their task mix and gate policy differ. Do not interpret that gap as a measured harness effect.

## Reproducible provisional inventory

I parsed task frontmatter under `/home/pawis/miti-workspace/.corral/tasks` with Python/PyYAML, selected each *worker run's* `execution.resolved.profile` (not the reviewer's), and joined its task/run to failing schema-v2 JSON receipts and sibling logs under `.corral/state/gates`. The first observed pi-sol worker run started 2026-10-27 15:01:23 UTC (T-478 R1); using that as a provisional cutoff on run start gives 42 pi-sol and 14 gpt-sol worker runs, including runs that might not have submitted. These denominators are not the historical stats denominator. Receipt attempts are not independent runs.

The following inventory deliberately includes only numerical attempt IDs, not `ag-*` IDs. The latter include member gates (for example T-591's receipt says `source: member-gate:T-339@1`), but not every `ag-*` receipt was attributed; **this is not a complete count of worker submission failures**. Full/landing gates may also be included in the numerical IDs. Counts are by failing receipt, not by unique run. Assign unknown rather than inferring candidate causation from an assertion name.

| Decisive log signature | pi-sol | gpt-sol |
|---|---:|---:|
| Formatting / clippy | 0 | 3 |
| Compile (non-clippy) | 0 | 1 |
| Confirmed fixed-flake exclusion (T-616 capture window) | 1 | 0 |
| Test assertions, unclassified cause | 7 | 10 |
| Other unclassified (docs check) | 1 | 0 |
| **Total** | **9** | **14** |

Zeros are counts only in this incomplete subset, not evidence of absence in the full population. T-491's `capture_outside_attempt` is the T-616 class and is excluded from the remaining categories. T-607, T-613, T-621 and T-624 signatures have not been exhaustively checked against all omitted `ag-*` receipts; T-610's own result says its underlying cause remains unverified, so do not assume every similar assertion was fixed. One `ag-*` T-470 receipt fails `interrupted_coding_stages_keep_automatic_origin_and_never_replay_submitting`, the T-624 class; it is outside this table already.

The rows can be audited from gate-log prefixes: pi-sol T-483 R1 (pi-lane assertion); T-491 R1 (four: capture, docs, two pane-inventory assertions); T-502 R1 (two: session ownership, sync-step); T-519 R1 (approve fast-forward); T-523 R1 (parked wake). gpt-sol T-540 R3 (clippy, aggregate-live assertion, macOS cross-check compile); T-550 R1 (format, five assertion receipts, two work-budget receipts); T-552 R1 (attention assertion); T-562 R1 and T-568 R1 (pi-lane assertions); T-567 R1 (clippy). Full filenames include timestamp and are discoverable by the task/run prefixes in the gates directory. T-550's two work-budget receipts are counted separately. Test failures are not necessarily touched-code failures, nor necessarily flakes.

## Actionable hypotheses, not ranked causes

1. **Attribution and sampling are the first bottleneck.** Cheapest capability: emit a structured submission-gate event with worker profile, run, attempt, tier, lifecycle purpose (submission/member/landing), first failing step and test, and a link to the transcript. Reconstruct a matched cohort and count first submission per run, separately from retries and landing.
2. **Pre-submit coverage can miss gate-only checks.** The gpt-sol T-567 quick receipt reports Clippy's `items after a test module`; T-540 also failed Clippy. A cheap worker-skill line: “Before your contracted gate, run changed-crate formatting and the relevant all-targets Clippy if the contract permits; wait for completion and report its exit status.” Do not run an extra gate where the contract forbids it.
3. **Flaky tests contaminate gate metrics.** T-491's T-616 capture-window failure is concrete evidence. Cheapest reporting default: tag known test signatures with fix revision, filter only exact matching failures when comparing harnesses, and retain an unknown category. Never label a failure flaky solely from its timing or an eventual pass.

## Remaining work and limitations

The task requested classification of **every** failure, exclusion of all six specified flake classes, and inspection of each real failure's pre-submit tests, local quick-gate use, and interrupted builds. That work is not complete. Worker sessions exist under `.corral/state/role-sessions` and OmO child sessions under `~/.omo/agent/sessions`, but the session histories and candidate diffs have not been fully joined to the receipt inventory; no conclusion about relative pre-submit behavior or interrupted builds is justified. The `ag-*` receipt lifecycle and numerical full/landing receipt attribution must be resolved before reporting cohort-wide class counts. No load testing or code changes were performed.
