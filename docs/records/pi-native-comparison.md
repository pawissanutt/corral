# Pi-native worker comparison: incomplete cohort

T-313 / T-252@P5, 2026-10-26. **Three pairs are independently reviewed; the six-pair comparison is incomplete.**
There is no measured pass/fail verdict, and no host default has changed.
The PM is not evaluated; both comparison arms must retain the OmO PM.

## Owner controls

OD9 permits subscription runs only, $0 paid API, at most twelve worker runs
(six pairs), and at most three review rounds per run. Stop and report when a
pair exceeds twice its preregistered expected token count. Required task mix:
two xs/s, three m, one l, from real Backlog work with mixed kinds.

Each pair must freeze the source task/spec, base SHA, gpt-6.1-sol model and
effort, gate contract/tier, independent reviewer profile, OmO PM, executable
digest, effective config digest and slot load. Randomize arm order before
execution. A changed executable or config invalidates a pair. Review both
arms independently, land one candidate, and discard the other after review.

The owner amendment of 2026-10-26 permits T-313 to start before T-297 closes;
it does not waive any of these controls or change T-297.

The owner subsequently answered "allow Independent, allow" to the escalated
T-245 method question, then "okay do it" to the recommendation to release the
source freeze and finish within twelve runs with an honest qualified report.
The remaining four runs can yield five completed pairs plus the retained first
pair's startup failure. No thirteenth attempt is authorized. This is bounded
closeout approval, not a claim that the original six-complete-pair criterion or
default-switch pass bar was met. Remaining implementations must be independent;
accepted prerequisite imports are allowed, counterpart task solutions are not.

## Offline deliverable

`tools/pi-compare/` parses canonical show readbacks and complete timeline or
raw event captures. It computes per-arm and aggregate review rounds, rescues,
Needs me counts/duration, silent endings, PM versus owner question answers,
gate counts/results/durations, API-equivalent cost, tokens and wall time to
approval. It preserves raw-input hashes and refuses incomplete measurement
instead of inventing zero values.

The committed raw fixtures are explicitly synthetic. Their results are not
live comparison results and do not satisfy the six-pair acceptance criterion.
The tool does not start or authorize live runs. Capture procedure, manifest
shape, exit statuses and metric limitations are in `tools/pi-compare/README.md`.

Initial offline checks passed: frozen Bun installation, strict TypeScript checking,
and 24 deterministic tests across two files, including the actual CLI. The
committed fixture prints `data_kind: synthetic`, `outcome: not_evaluated`
and exits 2 as designed. The retained check log and synthetic CLI output are
separate artifacts, not a corral gate receipt or live acceptance evidence.
Editor diagnostics for the source directory were clean; individual test-file
diagnostic requests timed out, while the complete compiler check passed.

## Live status and remaining evidence

The first preflight reported no installed Pi assets or corral-owned
credentials. A later preflight in this session reported a managed GO
installation, all three roles ready and credential metadata present. No
credential contents were read, copied or attached. Credential presence alone
does not prove provider subscription coverage or authorize a live pair.

The owner login decision is resolved by the accepted R1 receipt
`owner-T313-pi-login-20261026T023138`. Its answer explicitly resumes the
existing comparison contract and retains every OD9 restriction. The worker
read the complete receipt and independently reran doctor; all Pi checks
were ready. Login confirmation is no longer a blocker.

The owner selected **Private clones (Recommended)**. Accepted receipt
`owner-T313-private-clones-20261026` confirms that independently reviewed
clone-only candidate landings satisfy this comparison. The private store has
its own plugin-free server and six repositories pinned to
`d4f1cd444cc9697b9bc9d239dcc1f6f07790f1e4`. Original source cards, main,
configuration and session binding remain untouched.

Preregistration was captured at 2026-10-26T03:43:31.892Z and attached before
worker inference. The cohort uses unchanged Backlog bodies:

| Source | Private OmO / Pi cards (project) | Size | Randomized order | Expected pair tokens |
| --- | --- | --- | --- | ---: |
| T-248 | T-001 / T-007 (p01) | s | OmO, Pi | 26,450,204 |
| T-249 | T-008 / T-002 (p02) | s | Pi, OmO | 26,450,204 |
| T-244 | T-003 / T-009 (p03) | m | OmO, Pi | 60,872,940 |
| T-245 | T-004 / T-010 (p04) | m | OmO, Pi | 60,872,940 |
| T-246 | T-011 / T-005 (p05) | m | Pi, OmO | 60,872,940 |
| T-180 | T-006 / T-012 (p06) | l | OmO, Pi | 125,133,604 |

The pair estimate is twice the canonical historical median worker-plus-review
tokens per run at each size (49 s samples, 82 m, 49 l), measured before live
calls. The stop threshold remains twice the registered pair estimate. The
random seed is `3cef08e6f8e0c714d4d2dcd53f1eb598`; the preregistration records
source-body/readback hashes, binary/config hashes and the fixed medium effort,
gpt-6.1-sol routes, independent OmO reviewer and common OmO PM. No Pi PM is
launched or evaluated.

The private-context doctor passed all Pi checks. Both profile routes resolve
to gpt-6.1-sol, medium effort and `comparison-review`; Pi model discovery is
explicitly unavailable, so actual provider execution is still live evidence
to collect, not something inferred from doctor. A directory notification
observer reads canonical public task data and enforces token/run/review caps;
it does not parse pane or transcript text for control.

Worker 1 of 12 was admitted at 2026-10-26T03:53:50.744095596Z: the OmO arm
of source T-248, private T-001/R1 in p01. Its actual readback matches the
comparison parser and records the frozen base, subscription model, medium
effort and quick review GateContract. Canonical usage collection is active
and priced, not inferred from a screen. The private baseline worker and common
OmO PM use their normal harness credential store; Pi uses only its own
corral-owned credentials. No credential file was copied.

The first OmO candidate was independently approved after two review rounds.
Raw capture measures 18,136,378 worker-plus-review tokens, zero Needs me,
zero rescues and zero recorded silent endings; two of its three quick gates
passed. This is one arm, not an aggregate comparison result.

Two observed lifecycle details required correcting the offline validator:
approval selects the frozen full landing tier rather than the admission-time
quick review tier; and a discarded card cannot be reopened through `move`.
The tool now hashes both raw gate stages and rejects unrelated plan drift,
and binds distinct private task/run tuples to the original `source_task`.
The exact second-card aliases were attached before Pi inference. Source
specifications, cohort, base, seed/order, profiles, binary/config hashes and
budgets did not change. Both regression cases pass in the 26-test offline suite.

Pi's T-007/R1 admission at 2026-10-26T04:37:34.516146999Z exited before
binding a session or starting a model turn. Its canonical launch log reports
`credentials_unavailable`: the native launcher used the private server's
XDG data directory, unlike the preflight client's configured environment.
The credential guard rejected a direct auth-directory symlink, which was
removed without touching the original credentials. Process metadata then
confirmed that only the exited host's shell remained. Its launch environment
was corrected to use the original corral-owned data root; the plugin-free
server's environment was not changed. The frozen binary refused fenced recovery
with `op_unavailable: corral agent resume lands with T-252 child recovery`.
The failed private card was closed with its branch and raw failure retained;
no bootstrap was replayed and no extra dispatch was silently substituted.
Completing that pair now needs an owner-approved replacement allowance.
This setup failure, Needs me event and recovery must remain in the evidence;
they cannot be silently removed or attributed to worker implementation quality.

A new private-shell startup setting resolves the original corral-owned data
root while leaving herdr's server environment isolated. A disposable fresh pane
verified every doctor boolean, including credential presence, against its actual
environment; it was then closed. Dispatch 3/12, the Pi-first T-249 card
T-002/R1, now has a healthy generation-1 binding and a canonical first turn.
The other five pairs continue within the original budget while the first pair
remains explicitly incomplete.

The native T-249 arm was independently approved after two review rounds at
2026-10-26T05:39:18.182810382Z. Its captured worker-plus-review totals are
9,731,132 tokens and USD 1.8148016 API-equivalent cost, with zero recorded
rescues, Needs me events or silent endings. Two of three quick gates passed.
The canonical waiting-external turns resumed through gate wakes, and the
review correction arrived through the native inbox in the same generation.
Its candidate branch and complete raw captures are retained. The matched OmO
T-008/R1 arm started at 2026-10-26T05:46:11.474693667Z from the same unchanged
p02 main, binary/configuration, specification, model and medium effort.
This is dispatch 4/12; the single-arm measurements are not an aggregate verdict.

The matched OmO T-249 arm was independently approved on its first review at
2026-10-26T06:10:51.743073754Z. Its captured totals are 13,060,675 tokens,
USD 2.3644284 API-equivalent cost, zero recorded rescues/Needs me/silent endings,
and one passing gate out of two attempts. The real comparison pipeline accepts
both arms' control/input checks with no pair-invalid reasons and no budget stop.
It still exits 2 with `not_evaluated`, because only one of six pairs is complete.
Corral ran the private p02 full landing gate for the reviewed second candidate,
carried its approval through the authorized tier promotion, and landed
`48f66776658f4add2630e7c5554b13ce6d0b48eb` only on p02 main. No full gate was
launched manually. The preregistered OmO-first T-244 arm is now T-003/R1:
dispatch 5/12, with frozen-control preflight and immediate admission capture.

The task's explicit alternative affected implementation breadth: both reviewers
accepted audited cleanup plus no active scope without a grant. Native additionally
filters terminal members of a live grant, while the OmO candidate retains them
until cleanup. The unchanged specification and fixed reviewer profile allowed
those variants; the measurements do not establish identical feature breadth or
attribute that difference solely to the host.

Both T-244 arms were independently approved on their first review and captured
before landing or discard:

| Arm | Worker plus review tokens | API-equivalent USD | Review rounds | Quick gates passed | Wall to approval (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| OmO T-003/R1 | 11,843,730 | 2.1539848 | 1 | 1/1 | 1,519.936 |
| Pi-native T-009/R1 | 2,345,651 | 0.6069228 | 1 | 1/1 | 1,185.337 |

Both have zero recorded rescues, Needs me events and silent endings. The combined
14,189,381 tokens are below the preregistered 121,745,880-token stop threshold.
The first candidate's branch was retained after discard. Corral's full gate
`4056667-1792999032029078569` passed and landed
`c899c5fb9388051dc380c8b480479ea73dee5c4c` only on p03 main. The coordinator's
five-minute observer expired while that gate continued; reattaching consumed
the same passing receipt and authorized tier promotion, without a duplicate
gate or worker restart. This post-approval recovery is outside the primary
measurement window. Dispatch 7/12 is now OmO-first T-245, private T-004/R1.

**Arm-order confound:** the native worker's canonical checkpoint and completion
summary explicitly record porting the already reviewed T-003 implementation
after a fresh red reproduction. Both independent reviews record the same
resulting tree, `46afb1fef02e64754c8c3e6cb100265221d1687f`. These are real costs
of first implementation versus reuse, not a clean host-only comparison. The
shared board and retained branches exposed earlier-arm work; structural
spec/base/hash checks do not establish equal available implementation knowledge.
The report retains this pair and its raw evidence, but its lower native cost
must not be attributed solely to the host. The preregistered protocol is not
silently changed after observing this result.

The native admission helper failed only after successful dispatch and raw capture:
the canonical unbound host omits an empty `turns` array. The healthy worker was
not restarted or interrupted. The parser now accepts that valid admission shape
but still refuses completed native measurement without turn evidence. The exact
preserved raw admission validates; strict package/runtime typechecks, diagnostics
and all 28 tests pass, including the reproduced admission failure and the
completed-history rejection. No frozen control or threshold changed.

The real two-pair report validates both pairs' raw hashes and controls with no
pair-invalid reasons or budget stop. It remains `complete: false`,
`outcome: not_evaluated`, exit 2. Four pairs remain incomplete, including the
first pair's explicitly unresolved replacement allowance. Remaining work also
includes private landing verification, after-plan measurement, final raw evidence
attachments and the parent task's final gate/completion workflow.

The T-245 OmO arm stopped on an unregistered terminal prerequisite question:
its agent reported Done while the task stayed Running, with no gate or Corral
question event. The controller clarified that logical T-244 is the accepted
private T-009, not the intentionally unchanged original Backlog card, and
directed reuse of that accepted prerequisite in the candidate without changing
p04 main or its recorded dispatch base. Canonical inbox observation confirms
delivery to the same T-004/R1; no resend or new worker was needed. A structured
comment records this effective controller rescue. The lack of an
`idle_no_report` event did not prove absence of an unreported stop: OmO's warning
surface missed this hold. A notification-only observer alert now surfaces
Done-without-outcome states; budget enforcement and measurement definitions
remain unchanged. These setup and telemetry limitations also preclude attributing
later arm differences solely to the host.

T-004/R1 was independently approved on its first review at
2026-10-26T09:37:52.084238522Z. The captured OmO-only totals are 35,323,010
worker-plus-review tokens, USD 5.6687832 API-equivalent cost, one controller
rescue, two recorded `idle_no_report` events, and one passing quick gate from
two attempts. Thus the final timeline does contain warning events; their
absence at the earlier hold inspection was not a valid zero-count conclusion.
The explicit prerequisite-context audit is now counted, with a reproduced
regression, 28 passing tests, strict typechecking and clean diagnostics.

Dispatch 8/12 is matched native T-010/R1, healthy generation 1. It registered
blocking technical-choice question `T-010/R1/Q1`, escalated to owner at revision
2: reuse the reviewed T-004 solution or independently implement the task while
importing only its prerequisite. This method decision is pending; no answer
has been fabricated and no additional worker has started. It is separate from
the PM-owned pending first-pair budget decision. No third paired measurement
or aggregate verdict is claimed.

The canonical PM clarification requires independent remaining implementations,
allowing accepted prerequisite imports but not counterpart same-task reuse.
It is an interpretation of the existing comparison goal, not an owner answer
or acceptance amendment. The controller copied it into a private PM comment
and attempted the revision-2 answer as PM with the actual private spec basis.
The frozen CLI refused: `question_rule: T-010/R1/Q1 is escalated; only the owner
answers it`. The owner fence remains intact; no answer, cancellation, alternate
control message or restart has bypassed it. The separate PM cap question timed
out with the safe disposition KEEP12; no thirteenth run or six-pair acceptance
waiver follows. The original six-complete-pair criterion therefore still cannot
be claimed.

The direct owner-method prompt timed out unanswered after 30 minutes. A fresh
canonical readback still shows Q1 escalated at revision 2; no gated action
followed the timeout.

The original plan measurement script completed successfully for an interim
historical snapshot at 2026-10-26T10:27:26Z: 62,469 events, 962 Needs me,
224 idle warnings and 300 automation blocks. Compared with the retained
03:15:35Z baseline, these are increases of 910 events, 16 Needs me, one idle
warning and eight automation blocks. This is whole-board historical context,
not matched-pair data or evidence of a host effect. It uses the script's
original events-file source; the installed CLI rejects `export events`, and
its `log --all --json` is task-scoped. No post-cutover export equivalence or
final post-comparison measurement is claimed.

Preparation during the hold retained exact canonical readbacks for all twelve
private cards and a derived inventory: eight actual runs, four unstarted
cards, and two complete measured pairs. Four incremental Git bundles preserve
the six existing candidate heads across p01-p04; each passed `git bundle
verify` and requires frozen base `d4f1cd444cc9697b9bc9d239dcc1f6f07790f1e4`.
Base-only failed or unimplemented arms are not invented as candidates.
Bundles, inventory and raw readbacks are attached with retention tier `keep`.
No healthy resource was stopped or moved. Cleanup remains after authorized
terminal outcomes and complete evidence retention, before the parent `done`
scan can collect bulky disposable clones, targets or runtime binaries.

## Third reviewed pair and timing correction

Native T-010/R1 was approved on review 1 at
2026-10-26T15:18:08.46019135Z, on candidate
`b11c0e3f9333f4685166ef2c889f289633c62cc1`. The owner answer was observed by
the same healthy generation 1; no replacement worker was dispatched.

| T-245 metric | OmO | Pi-native |
|---|---:|---:|
| Worker + review tokens | 35,323,010 | 17,612,214 |
| API-equivalent USD | 5.6687832 | 2.7795176 |
| Review rounds | 1 | 1 |
| Rescues | 1 | 0 |
| Needs me / seconds | 0 / 0 | 1 / 17,275.993 |
| PM / owner answers | 0 / 0 | 0 / 1 |
| Recorded silent endings | 2 | 0 |
| Quick gates passed / total | 1 / 2 | 1 / 1 |
| Wall seconds to approval | 7,328.436 | 20,179.628 |

The native wait ends at its matching canonical owner answer, not at `done`.
The previous parser overstated this interval as 19,580.344 seconds; the
corrected parser also preserves overlapping native-question and legacy-decision
holds. The regression failed before the fix, all 31 tests and strict typecheck
passed afterward, and actual raw replay produced 17,275.993 seconds. Production
file diagnostics were clean; test-file LSP freshness timed out, while `tsc`
passed. The frozen experimental executable and configuration were not changed.

The real three-pair CLI report exits 2 with `complete: false`, `valid: false`,
`outcome: not_evaluated`, no per-pair invalidations and no token-budget stop.
Diagnostic aggregates are USD 10.1871964 / 5.201242 and 60,227,415 / 29,688,997
tokens for OmO / native. Native now has one Needs me against OmO's zero, so
that threshold is false. Other diagnostic thresholds are true; they do not
override incompleteness, prior counterpart reuse, solution exposure or the
different operational question/silent-ending detection surfaces.

## Fourth pair: native arm captured, OmO arm running

T-246 native T-005/R1 was approved after two independent reviews at
2026-10-26T23:59:39.010704739Z on repaired candidate
`d5f2478b692be71ea1a06334b7bf7bd821ae9f0f`. Review 1 reproduced a genuine
text-budget-boundary search-score mismatch (16016 incremental versus 8016
canonical). The same generation repaired it, added deterministic result-parity
coverage and passed a new quick gate; review 2 approved. No replacement ran.

The captured native-only totals are 5,876,021 worker-plus-review tokens,
USD 1.3702244 API-equivalent, two reviews, two passing quick gates, 569.988 gate
seconds and 2,137.091 wall seconds. Recorded rescues, Needs me, PM/owner answers
and classified silent endings are zero. The raw turn table separately records
an initial provider overload; zero classified silent endings does not erase
that error or establish equal instrumentation between hosts.

Review-2 admission was temporarily refused by pane reconciliation and machine
load. Refused launches allocated no review. The configured policy was retained;
launch succeeded after canonical resource admission recovered.

The approved native raw window was captured before discard; its branch is
retained. Matched OmO T-011/R1 is now worker 10/12 from the same frozen base,
with the same independent-implementation instruction. This single approved arm
is not a fourth completed paired result or an aggregate verdict.

Pass bar, in aggregate: Pi has no more Needs me or silent endings than OmO,
review rounds are within +1, API-equivalent cost is within 1.25x, and gate pass
rate is no lower. No default switch follows automatically from any outcome.
