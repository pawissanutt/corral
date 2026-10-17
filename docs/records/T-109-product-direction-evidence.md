# T-109 evidence and acceptance map

Investigation: October 17, 2026. Repository source baseline:
`33072fe192e80dae34ae570e80f936e1cb921c56`. Paths/line anchors below refer to
that commit, not to historical reports' older source baselines.
The report is [T-109-product-direction-report.md](T-109-product-direction-report.md).
No production code, live acceptance, task specification or configuration was
changed by this investigation.

## Method and limits

Read local guidance and searched stored evidence before further investigation:
`corral search "PM" --in all --json`, followed by the queries
"product direction", "review repair", and "context loss". Broad search results
included unrelated artifacts and large transcripts; those snippets were used
only to locate relevant Corral records, not as proof of their nested claims.
Read the existing T-045, T-049, PM retrospective and T-098 proposal sections
directly, then checked current source/contracts where their dates mattered.

This is a bounded source-based product comparison, not exhaustive market
research or a live usability trial. No prototype or alternative PM team was
implemented. No paid model, owner-session experiment, task dispatch, project
pane creation or policy change was performed as a research probe. The required
repository gate uses the fresh plugin-free test-herdr environment; its fixture
panes are separate from the owner's session.

## Source ledger

### E1. Current deterministic coordination

- [PM guide](../guide/pm.md), "Running corral with a cheap PM", "What the cards
  trust", "Policy boundaries": read-only brief/next, owner flags and reconcile
  on each wake; a role prompt is not an OS sandbox.
- `crates/corral-ops/src/pm_next.rs:463-510`: deterministic rank/select returns
  one action, not all pending decisions. Questions rank first; owner and plan
  decisions share a lower priority. `:511-601` builds the live card and evidence.
- [Automation](../guide/automation.md), opening policy/gate/reviewer/digest
  sections: opt-in execution, bounded change rounds and deterministic digests.
- `crates/corral-ops/src/digest.rs:1,48-54,174-220`: code-only collection,
  manual/scheduled publication; missing usage remains unknown. The T-048/T-098
  sketches predate this shipped digest, so a new digest is not recommended.

Interpretation: ordinary event handling is not a reason to add an agent.
The possibility that a single prioritized card obscures another decision is a
product hypothesis, not a reproduced lost-decision incident.

### E2. Current PM identity and acceptance boundaries

- `crates/corral-ops/src/pm.rs:9-26,78-91`: one `PmState` with pane ID,
  `state/pm.json` reuse via pane lookup, not a scope-generation writer lease.
  `:94-166` constructs the PM prompt and owner/role restrictions.
- `crates/corral/src/pm_cmd.rs:6-32`: current PM subcommands are Bench, Next,
  Brief; bare PM opens the workspace pane. Proposed lease/project PM commands
  are not current capabilities.
- [Candidate-bound acceptance](../guide/acceptance.md), opening identity,
  completion and landing sections: fresh task/run/attempt/tree/base/spec/policy
  evidence, independent review, owner mode, repository transaction locks.
- [Automation](../guide/automation.md), "Reviewer runs and verdicts": clean
  equivalent rebase approval carry requires a fresh integrated gate and
  matching evidence/inputs; changed patches/conflicts require renewed review.

Interpretation: all three operating models must call the same policy authority.
No model can make a stale approval current through summarization.

### E3. Work structure and present view limits

- `crates/corral-core/src/model.rs:607-676`: Task includes optional project,
  depends_on, labels, write_scope, plan backlink and children.
- `crates/corral-ops/src/plan_decompose.rs:276-341`: children clone their
  parent, reset run/plan histories, copy scope/acceptance and dependency IDs;
  there is no per-child project selection in this construction.
- [Plan guide](../guide/plans.md), "Decomposition and implementation": accepted
  multi-task plans publish Backlog children and mark the planning parent Done
  without a code landing. Zero/one scope stays attached to the original task.
- `crates/corral-tui/src/board.rs:74-125,248-270`: current view holds task
  collection, columns and idea/Done controls; column selection filters by
  status and idea toggle, not a nested project/initiative tree.
- [TUI guide](../guide/tui.md), Board, Card detail, Review, History and artifact
  sections: existing navigation and stage/attempt inspection.
- `docs/DESIGN.md:584-587` explicitly calls board text filtering/project
  switcher future work; used only after checking the current board source.

Interpretation: a work navigator can reuse existing identities and parent links;
arbitrary initiative membership is a separate proposal. Cross-project task
dependencies do not create atomic repository acceptance.

### E4. Recovery and event semantics

- [Progress/checkpoint guide](../guide/run-progress.md): identity-bound
  revisions, stale checkpoint handling, evidence nontransfer, wait/resource
  obligations, exact source-run resume packet.
- `crates/corral-ops/src/progress.rs:238-340`: save rejects stale run/spec/
  progress/HEAD/base/tree; pending waits/resources require retention or
  explicit resolution.
- `crates/corral-ops/tests/progress_ops.rs:19-52`: restart roundtrip test reads
  the persisted state into a new context and checks snapshot/progress identity.
  Reading this test is source evidence; execution is the separate full gate.
- [Events guide](../guide/events.md): one-shot cursors, timeout preservation,
  rotation limits, skipped push while PM absent, wake != authorization.

Interpretation: durable work recovery is needed in every model; transcript
continuation and multiple still-open panes cannot substitute for it.

### E5. Historical operational evidence, not new incidents

- [T-045](T-045-per-task-automation-report.md), "Sources, snapshot, and limits",
  "Event and transcript evidence", and "PM tokens today versus after":
  fixed-window counts and reported token/cost fields. Baseline cutoff is
  October 15, 2026, 10:49:30.473Z at old source `2fed45b`; no current cost or
  alternative-model saving is inferred.
- [PM retrospective](pm-session-2026-10-15-retro.md), items 2-5, 8-10, 14-20:
  strict verdict-state boundaries, stale candidates, round exhaustion, session
  provenance, sync driver and owner-overreach risks. It distinguishes PM
  reports from corroborated events and source-based inference.
- Important age correction: retrospective item 20 describes pending-plan
  admission, lifecycle, cancellation and reply gaps at its earlier main.
  Today's [plan guide](../guide/plans.md), "Reply and cancellation" and
  opening admission/lifecycle sections, documents their shipped remedies.
  This spike does not present those older gaps as still open.
- [T-049](T-049-reliable-pm-report.md) and
  `crates/corral-ops/tests/fixtures/pm-2026-10-15.provenance.md`: replay's
  selected real events and explicitly supplemental typed observations.

Interpretation: recorded coordination mistakes motivate code policy and
recoverable decision records. They do not prove multi-PM coordination fails,
nor that a board alone prevents mistakes.

### E6. Persistent PM alternative, deliberately not implemented

- [T-098 design](team-of-pms-design.md), opening evidence, responsibility,
  lease lifecycle and crash matrix sections (through line 180 inspected).
- [Companion plan](team-of-pms-plan.md) identified as the related proposal;
  not treated as evidence an API is implemented.
- Proposed disjoint lead/project leases, generation fencing, launch controller
  and explicit takeover illustrate implementation/recovery cost. Lead scope
  is explicitly not wildcard project authority.

Interpretation: model 3 could help concurrent project judgment, but neither
throughput nor aggregate cost was measured here. Its coordination machinery
does not remove the need for work structure or owner authority.

### E7. Task relationship and classification readbacks

Read via `/home/pawis/.cargo/bin/corral show <id> --json`, reducing large
records to specification, comments and status. Observed on October 17:

| Task | Observed status | Relevant boundary |
|---|---|---|
| T-048 | Dropped | Six-criterion multi-PM/lease proposal; PM comment names T-109 as broader reconsideration, not a spec/disposition change. |
| T-107 | Idea | Narrow review-UX investigation needs actual screenshots and mouse/keyboard journeys; not claimed by this report. |
| T-102 | Needs me | Proposed Pi-native PM integration; owner/provider/transport/rollout choices remain separate from this product-model choice. |
| T-109 | Running; research, m | Actual dispatched report-only run, despite retained Idea/owner-only filing instruction in body. No new dispatch authorization inferred. |

These are dated observations, not permanent dispositions or permission to act
as owner. No comment, specification, status or linked plan was changed.

### E8. Executed offline measurement

Executed from this worktree, with null stdin:

```sh
/home/pawis/.cargo/bin/corral --version
/home/pawis/.cargo/bin/corral pm bench \
  --events crates/corral-ops/tests/fixtures/pm-2026-10-15.events.jsonl --json
```

Version output: corral 0.1.0. Selected raw JSON fields are retained in the
attached baseline replay log. Result: 50 events, eight labelled cases, seven
covered criteria; code-policy 8 passed, zero violations, no missing responses.
The fixture deliberately is not a complete board and includes explicit
supplemental observations. No external response file/model call was used.
The installed binary is not asserted to equal the source baseline; a
candidate-binary replay follows the required gate build, logged separately.

## Acceptance coverage and verification boundary

| Requested acceptance | Report location | Proof and limitation |
|---|---|---|
| All three models; managed work, owner journey, agent value | Recommendation; What corral manages; Three operating models | E1-E3,E6; source-derived baseline and explicit proposals. |
| Same four scenarios and five comparison dimensions | Scenario A-D tables | E1-E5; all 12 model/scenario combinations compare effort, missed decisions, cost, recovery and authority. Comparative outcomes are hypotheses. |
| Measured versus hypothesized results | What is measured | E5 historical counts; E8 rerun; no invented live comparison or numerical savings. |
| Concise recommendation, alternatives and sketches | Recommendation; model table; workflow/view sketch | Recommendation is judgment; sketch explicitly unimplemented and agent-independent. |
| Owner decisions, smallest next increment, T-048 relationship | Final section; E7 | Read-only scoped navigation proposal; no new dispatch, policy, spec or disposition changes. T-107/T-102 remain distinct. |

Gate logs prove repository checks on the candidate, not product usability or
the recommendation's predicted benefit. Required foreground isolated gates,
candidate replay and clean diff checks are recorded in attached raw logs.
No new tests are added for this pure-prose change. No visual QA of a new UI is
claimed: the only views delivered here are text sketches.
