# T-184: Jev decision-model spike on Corral history

Date: 2026-10-18. Research only. Model: jev-1.13.0 (pinned stable release).

## Decision

**NO-GO for all three tested packets. No runtime shadow integration is justified by these results.** Jev did not improve gate accuracy over the majority/rule baselines, refused every first-review forecast, and had only one correct overlap answer in four weakly labelled pairs. This rejects these input/question/policy combinations, not Jev on richer evidence. No product code, configuration, dependency, or task was added.

| Hypothesis | Development/confirmation n | Jev confirmation accuracy | Development-majority baseline | Brier: Jev / baseline | Decision |
| --- | ---: | ---: | ---: | ---: | --- |
| gate | 24/15 | 9/15 (60.0%) | 9/15 (60.0%) | 0.5681 / 0.4826 | **NO-GO** |
| review | 61/28 | 2/28 (7.1%) | 15/28 (53.6%) | 1.5234 / 0.5969 | **NO-GO** |
| overlap | 0/4 | 1/4 (25.0%) | 0/4 (0.0%) | 0.8347 / 0.8000 | **NO-GO** |

The overlap baseline has no development observations: it predicts ambiguous and uses uniform class probabilities. The pre-registered lexical rule gets 2/4 (50%), better than Jev's 1/4. Treating the four positive pairs as independent validation, or fitting an always-extends baseline on them, would be misleading. Gate's diagnostic rule gets 9/15, equal to its majority baseline. The first-review baseline always predicts approve.

All three frozen thresholds are 1.01, an explicit all-abstain sentinel. Confirmation coverage is 0%, actionable precision is undefined (not 100%), and hypothetical Needs-me/fallback is 100%. These are not measured owner interruptions: shadow makes no changes and existing authority remains responsible.

## Snapshot and hypothesis selection

The read-only source was /home/pawis/miti-workspace/.corral. The exact first 13,850 event lines end at 2026-10-18T20:41:52.015523211Z, before this run. The scratch snapshot contains 167 Corral tasks excluding T-184, 1,986 sanitized outcome-relevant event projections, and 65 diagnostic excerpts from stored failed-gate artifacts. Sixteen non-Corral tasks were excluded before export. Current task specifications were captured for local audit, not used as historical model inputs. The task-file reads are not an atomic board transaction; the fixed event prefix is the historical boundary.

This is a bounded, sanitized research snapshot, **not a complete forensic copy** of the 8.6 MB multi-project event log. Source artifact references, their stored hashes, prefix canonical-JSON hash and snapshot hash are in source-manifest.json. The prefix hash is not claimed to equal the original byte hash. The complete projected snapshot and raw scored files are attached; regeneration does not require the live board.

Before investigation, searched Corral for decision model, then EINTR and overlap. The history contains 339 gate result events (65 fail), 192 verdicts, 798 Needs-me events, eight edits and one explicit idea disposition for Corral. These are repeated lifecycle observations, not independent samples. Therefore the scored gate cohort uses one first failure per task (39); review uses one first verdict per task (91 available, 89 eligible). Repeated retries and repair rounds do not inflate the scored sample.

Three candidates were retained because they have later observable outcomes:

- **H1, gate triage:** prevent wasted repair rounds and avoid treating transient execution as product defects. This is an outcome-quality hypothesis, not only a cheaper call.
- **H2, first-review risk:** route genuinely difficult work to more suitable review, without ever bypassing independent review.
- **H3, filing overlap:** catch redundant ownership and reuse existing work before parallel implementation.

Other owner candidates were screened out rather than given invented labels. Pane/agent state has many repeated Needs-me observations but only one retained Corral wait-registration event and no corresponding wait-transition event; historical synchronized terminal captures and independent stuck/waiting/asking adjudications are absent from this snapshot. Classifying its own note would be tautological. Kind/size are filing decisions, not later measured outcome quality. Historical closed-choice PM cards and a separately justified correct next action were not retained here; later action alone does not prove optimality. T-137/T-170/T-172 remain useful prospective collection cases, not retrospective gold labels. T-154/PRF-08 is a design reference, not evidence that routing improves review quality.

## Pre-registration, labels and temporal isolation

The immutable machine registration is preregistered.json. It was written at **2026-10-18T20:50:15.822Z**, before the first development request at 2026-10-18T20:50:15.887Z. All labels and this report are analyst-derived from history; no blind human adjudication or causal outcome experiment was performed.

The global split is input time **before 2026-10-17T00:00:00Z = development; at/after = confirmation**. For gate this is failure time; for review it is the first pre-verdict done event; for overlap it is filing time. Gate and review each have one case per task. The four overlap pairs share one 2026-10-18 incident group, entirely in confirmation; there is no valid overlap development cohort. No random split or repeated threshold search on confirmation was used.

| Hypothesis | Development labels | Confirmation labels | Historical input and later label rule |
| --- | --- | --- | --- |
| H1 gate | retry unchanged 9; repair required 1; ambiguous 14 | retry unchanged 6; repair required 0; ambiguous 9 | Only the contemporaneous failed-gate diagnostic window. Next recorded same-task/run gate passing at exactly the same SHA labels retry_unchanged. A different SHA plus deterministic compiler/lint/format/manifest diagnostic labels repair_required. All other sequences remain ambiguous, including repeated failures, missing next results, and assertion/fixture failures needing changes. |
| H2 review | approve 43; changes 18; ambiguous 0 | approve 15; changes 11; ambiguous 2 | Original creation title and first pre-verdict same-run done diff counts only. First independent implementation verdict supplies approve/changes; escalate maps to ambiguous. T-143/T-144 lack an original creation title and are excluded. No later spec, reviewer notes, current run summary, or final diff enters the request. |
| H3 overlap | none | extends 4 | Original filing titles of proposed/existing tasks. Later title edits label T-175/T-167 and T-179/T-168; added dependencies label T-175/T-144 and T-176/T-145 as extends. These are weak positive outcome proxies, not complete pairwise semantic adjudication. No absence-of-edit is labelled unrelated. |

The exact Choice schemas and complete instructions are in preregistered.json and repeated in each raw request. Their answer sets are:

- H1: retry_unchanged, repair_required, ambiguous.
- H2: approve, changes, ambiguous.
- H3: duplicate, extends, conflicts, unrelated, ambiguous.

Baseline hard predictions use development-majority class (criteria order breaks ties); baseline Brier uses empirical development class frequencies. With no development examples, the fallback is ambiguous plus uniform probabilities. The additional gate rule chooses retry_unchanged for EINTR/interruption/timeout/socket/unavailable text; deterministic compiler/manifest/format markers choose repair_required; otherwise ambiguous. The overlap rule chooses extends only when both titles contain plan/tracker. They were registered before scoring, not chosen to explain model errors.

The shared GO bar, fixed before any calls: at least 20 confirmation examples, at least two observed classes, accuracy at least 10 percentage points above development-majority, strictly lower Brier, at least 90% actionable precision and at least 30% coverage. GO would authorize only consideration of a further shadow trial, never enforcement. Gate and overlap already fail the sample bar; gate also lacks a confirmation repair-required example, so it cannot establish safety on real defects.

## Threshold freeze and scoring

Thresholds were frozen at **2026-10-18T20:50:47.662Z**. The first confirmation request started at **2026-10-18T20:50:47.776Z**, after the freeze. Registration, development and cases hashes bind frozen.json to its inputs; verify.mjs checks the full chronology. There were 85 development and 47 confirmation requests, exactly one per case, with zero failed calls or retries. No prompt or answer-set changes followed development.

The development policy chooses the lowest top-probability threshold among 0.5, 0.7, 0.8, 0.9 and 0.95 yielding at least five non-ambiguous accepted answers with at least 90% precision. If none qualifies, freeze 1.01. No hypothesis qualified. Thresholding uses the maximum option probability, **not** the API's normalized confidence statistic; ambiguous always abstains.

Brier is the average unnormalized multiclass sum of squared probability errors, range 0-2. It includes ambiguous as a label. This taxonomy conflates uncertain evidence/refusal with the observed escalation class in H2: its large Brier primarily says the model predicted refusal rather than actual review outcomes. It must not be read as a universal test of Jev's probability calibration or as a probability that code is correct.

### Reliability on confirmation

Bins are left-inclusive, right-exclusive except the final bin includes 1.0. Empty bins are n/a. Mean top probability is compared with empirical exact-label accuracy.

| Hypothesis | Top probability bin | n | Mean probability | Accuracy |
| --- | --- | ---: | ---: | ---: |
| gate | 0.0-0.2 | 0 | n/a | n/a |
| gate | 0.2-0.4 | 0 | n/a | n/a |
| gate | 0.4-0.6 | 1 | 0.5500 | 0.0% |
| gate | 0.6-0.8 | 11 | 0.6973 | 63.6% |
| gate | 0.8-1.0 inclusive | 3 | 0.8267 | 66.7% |
| review | 0.0-0.2 | 0 | n/a | n/a |
| review | 0.2-0.4 | 0 | n/a | n/a |
| review | 0.4-0.6 | 0 | n/a | n/a |
| review | 0.6-0.8 | 6 | 0.7600 | 0.0% |
| review | 0.8-1.0 inclusive | 22 | 0.8995 | 9.1% |
| overlap | 0.0-0.2 | 0 | n/a | n/a |
| overlap | 0.2-0.4 | 1 | 0.3500 | 0.0% |
| overlap | 0.4-0.6 | 0 | n/a | n/a |
| overlap | 0.6-0.8 | 2 | 0.6550 | 50.0% |
| overlap | 0.8-1.0 inclusive | 1 | 0.8400 | 0.0% |

### Coverage versus actionable precision

These are descriptive fixed-grid confirmation results, **not** newly selected policies. Ambiguous choices abstain at every threshold. The only 100% precision entry is one overlap answer from the same single positive incident; it does not justify relaxing the frozen all-abstain policy.

| Hypothesis | Threshold | Accepted | Coverage | Precision | Needs-me/fallback |
| --- | ---: | ---: | ---: | ---: | ---: |
| gate | 0 | 0/15 | 0.0% | n/a | 100.0% |
| gate | 0.5 | 0/15 | 0.0% | n/a | 100.0% |
| gate | 0.7 | 0/15 | 0.0% | n/a | 100.0% |
| gate | 0.8 | 0/15 | 0.0% | n/a | 100.0% |
| gate | 0.9 | 0/15 | 0.0% | n/a | 100.0% |
| gate | 0.95 | 0/15 | 0.0% | n/a | 100.0% |
| gate | 1.01 | 0/15 | 0.0% | n/a | 100.0% |
| review | 0 | 0/28 | 0.0% | n/a | 100.0% |
| review | 0.5 | 0/28 | 0.0% | n/a | 100.0% |
| review | 0.7 | 0/28 | 0.0% | n/a | 100.0% |
| review | 0.8 | 0/28 | 0.0% | n/a | 100.0% |
| review | 0.9 | 0/28 | 0.0% | n/a | 100.0% |
| review | 0.95 | 0/28 | 0.0% | n/a | 100.0% |
| review | 1.01 | 0/28 | 0.0% | n/a | 100.0% |
| overlap | 0 | 1/4 | 25.0% | 100.0% | 75.0% |
| overlap | 0.5 | 1/4 | 25.0% | 100.0% | 75.0% |
| overlap | 0.7 | 0/4 | 0.0% | n/a | 100.0% |
| overlap | 0.8 | 0/4 | 0.0% | n/a | 100.0% |
| overlap | 0.9 | 0/4 | 0.0% | n/a | 100.0% |
| overlap | 0.95 | 0/4 | 0.0% | n/a | 100.0% |
| overlap | 1.01 | 0/4 | 0.0% | n/a | 100.0% |

### Latency, tokens and spend

Latency below is confirmation end-to-end client HTTP p50/p95, nearest-rank milliseconds. Tokens/cost include both splits. No concurrency, SDK retries or local inference hides latency.

| Hypothesis | Confirmation p50/p95 ms | Input tokens | Output tokens | Token-priced USD |
| --- | ---: | ---: | ---: | ---: |
| gate | 131.3/206.8 | 46110 | 1792 | $0.001936620 |
| review | 122.8/222.4 | 38739 | 3738 | $0.001627038 |
| overlap | 131.5/169.1 | 1922 | 234 | $0.000080724 |

Total: **132 calls, 86,771 input tokens, 5,764 output tokens; $0.003644382 token-priced usage** at $0.042/million input tokens, output free. This is provider-reported token usage multiplied by the published tariff, not an independently retrieved invoice. It is below the $5 cap by $4.996355618. The harness checks a conservative per-request byte/token reservation and cumulative spend before sending; all requests succeeded without encountering early-access limits. Model identity in every response is jev-1.13.0.

Ollama answered the tags probe on 127.0.0.1:11434, but had no Clef-flash model (only qwen2.5:0.5b and two cloud aliases). The prior-art endpoint on port 11435 was unavailable. The optional Clef comparison was skipped; no model was installed, started, or substituted, and no Corral data was sent to the cloud aliases. Probe evidence is ollama-availability.json.

## Per-hypothesis interpretation

**H1 gate: NO-GO.** Confirmation accuracy equals both baselines (9/15); Brier is worse (0.5681 versus 0.4826); all 15 confirmation predictions are ambiguous. Across both splits, 35/39 gate predictions were ambiguous. The model supplies uncertainty rather than a useful retry-versus-repair choice on this packet. Some stored logs attached to failed-gate events end in Gate passed, so a diagnostic window alone may not describe why the envelope failed. This disagreement is retained, not relabelled or silently omitted.

T-131's famous EINTR is a separate descriptive vignette, not an additional independent scored example: the 2026-10-17T07:01:51.612093307Z failure at 269f6c28754a172c5ec6932bc18c62a5e6f2665b has recv_from interrupted in the contention fixture. Its next passing gate is at **dcfb360b150aeabeff5902602a0cd9b26b6b7a6c**, a changed candidate. Thus even this known fixture failure is not an unchanged-retry label under the registered rule. Its first failed gate, which is the scored case for T-131, instead has an unchanged next pass and is labelled retry_unchanged. Do not generalize EINTR to permission to ignore a failed gate: fixture reliability and proof identity remain deterministic concerns.

**H2 review: NO-GO.** Only 2/28 exact outcomes versus 15/28 for approve-majority; Brier 1.5234 versus 0.5969. Jev chose ambiguous for all 89 development-plus-confirmation forecasts. Its refusal is reasonable given title/diff counts omit code and tests; they are not evidence of actual implementation correctness. This packet cannot improve reviewer routing. It does not test full-diff review or a risk model trained on stronger immutable pre-review evidence, and it provides no observed reduction in escaped defects or owner repair rounds.

**H3 overlap: NO-GO, insufficient independent labels.** Jev correctly selects extends only for T-179/T-168 (probability 0.64); the other three pairs are ambiguous. Accuracy is 1/4 versus lexical rule 2/4; Brier is worse than the uniform fallback (0.8347 versus 0.8000). No duplicate, conflict or unrelated labels exist, and all four positives belong to one narrowing incident. Original pre-narrowing bodies were not preserved in the selected events, so sending current bodies would leak the answer. Titles alone omit why T-144/T-145 must be reused. This is a useful instrumentation target, not a demonstrated classifier win.

## Recommendation and proposed event shape

Keep decision scoring off. **None of these hypotheses presently earns runtime shadow integration.** If the owner later chooses to collect better evidence, filing overlap is the clearest outcome-quality target: freeze original proposed specs and the contemporaneous candidate task versions, then record explicit human merge/narrow/conflict/unrelated outcomes, including negative pairs. Gate work first needs immutable command diagnostics and candidate/tree identity to distinguish transport/identity failure, flaky tests and persistent fixture defects. Review routing needs richer pre-review inputs plus reviewer-independent quality outcomes; its own first verdict is only a proxy.

A future shadow-only observation could use this shape; this report proposes it but adds no product event or configuration:

```json
{
  "version": 1,
  "kind": "decision_model_shadow",
  "id": "7453787c8fa13a768479b7551aaa0b838f5877122b7d6e6eb633e592cdc76d18",
  "project": "corral",
  "task": "T-175",
  "hypothesis": "filing_overlap_v1",
  "mode": "shadow",
  "input": {
    "as_of": "2026-10-18T19:26:21.453471263Z",
    "proposed_title_sha256": "59feb5462844149be65fc84211e4b0c9fe5844b9cf1f3670edfde33ebaa6c3db",
    "candidate_tasks": [
      {
        "id": "T-167",
        "title_sha256": "29a05a7bb546836794d477a195713f401bb67337f1ccb62d596570102a269301"
      }
    ],
    "sanitized_payload_sha256": "746b76c056d1e3ead613bfe2af95a85ba611c261f3d3060718fb76e794de5c3b"
  },
  "model": "jev-1.13.0",
  "question_schema_sha256": "71861909c16b2be3c748c71f4f9224fead2e15ef962dd3489fe87ad9ae73e838",
  "policy_sha256": "11f079aa63ef0396ba617af7c1bd9316e8a0ddb6e7537c7b28335f1908faf211",
  "answer": "ambiguous",
  "probabilities": {
    "duplicate": 0.01,
    "extends": 0.35,
    "conflicts": 0.27,
    "unrelated": 0.02,
    "ambiguous": 0.35
  },
  "top_probability": 0.35,
  "threshold": 1.01,
  "abstained": true,
  "http_status": 200,
  "latency_ms": 169.07171099999687,
  "usage": {
    "input_tokens": 483,
    "output_tokens": 59
  },
  "estimated_usd": 0.000020286000000000002,
  "action_taken": "none"
}
```

This proposed event is populated from the actual T-175/T-167 offline score; it was not written to Corral. Only title hashes exist in this spike. A future full-spec trial must add exact original proposed/candidate spec revisions, and gate/review observations must also bind run, attempt and candidate/tree/base identities. Link a separate later label event by shadow-event ID and human author, with ambiguous/unknown retained; never overwrite the prediction. Record model failure separately from negative judgment. Do not record authorization headers or raw credentials. Shadow must not change task state, retry a gate, choose a profile, accept work, or create extra Needs-me prompts.

## Reproduction and evidence

All scratch material lives under .omo/evidence/T-184, outside product source. From the worktree root:

```sh
bun .omo/evidence/T-184/spike.mjs build .omo/evidence/T-184/snapshot.json
bun .omo/evidence/T-184/spike.mjs summary .omo/evidence/T-184/cases.json .omo/evidence/T-184/development.json .omo/evidence/T-184/confirmation.json .omo/evidence/T-184/frozen.json
bun .omo/evidence/T-184/verify.mjs
```

The first two commands emit cases.json and summary.json byte-for-byte. verify.mjs passes hashes, temporal isolation, task/project filtering, exact request state/schema, model/usage contracts, maximum-choice probabilities, token budget, and credential redaction. It makes no network calls. The exported score function and score-one command reproduce individual request construction with an environment-only JEV_API_KEY; do not re-score confirmation to tune this experiment. Raw development.json and confirmation.json include sanitized requests, full probability distributions, provider usage, HTTP statuses, request timestamps and measured latency, but no headers or key.

Evidence files: snapshot.json; source-manifest.json; cases.json; preregistered.json; frozen.json; development.json; confirmation.json; summary.json; spike.mjs; verify.mjs; offline-verification.log; ollama-availability.json. Required pre/post-rebase full gate output is attached separately as log artifacts by the run contract. LSP freshness timed out for the scratch JavaScript; executable Bun checks provide the available script validation instead. No changes under crates are needed.

## Limitations

- Three hypotheses were selected after reading outcomes, but question schemas, success bars and split were registered before scores. This is retrospective pre-registration of scoring, not prospective discovery on unseen board history.
- The history covers roughly three days in one rapidly changing project. Temporal development/confirmation shift is substantial (review approve baseline drops from 70.5% to 53.6%). No confidence interval here can establish deployment readiness; positive counts and one incident are too small.
- Unchanged commit is not unchanged infrastructure, base branch, policy or toolchain. A changed commit does not prove the previous failure was a product defect. The gate proxy is deliberately conservative and leaves most such cases ambiguous.
- No first-review code, complete diffs, immutable pre-narrowing specs, blind adjudication or counterfactual business outcome was available in these packets. Refusal and outcome ambiguity share a label. The report does not claim a calibrated probability of actual task correctness.
- Four overlap pairs are correlated weak positives, not four independent incidents. Missing negative labels prevents estimation of false overlap alerts. No shadow benefit, intervention reduction, defect avoidance or owner-time saving was observed in live operation.
- The snapshot is a sanitized projection with provenance, not an atomic backup. Task bodies can contain later edits and are local audit material only; the scored request fields avoid them. Secret stripping is heuristic, with exact-key exclusion additionally checked; it is not a general data-loss-prevention proof.
- Cost is token-derived, not invoiced. There is no paired Clef experiment, preview-model experiment, prompt sweep or product integration.

## Sources inspected

- TypeSafe official API, models and confidence pages, retrieved 2026-10-18: `https://docs.typesafe.ai/api.md`, `https://docs.typesafe.ai/models.md`, `https://docs.typesafe.ai/confidence.md`. The model page distinguishes stable jev-1.13.0 from preview jev-1.14.0; this experiment pins stable and does not claim to test the preview.
- User-provided introduction and announcement establish the proposed use, not measured Corral results: `https://docs.typesafe.ai/introduction` and `https://typesafe.ai/blog/introducing-system-one-models-and-jev`.
- Local prior art: mitigraph/docs/evaluation/results/decision-model-offline-results.md (frozen thresholds, separate confirmation, deterministic summaries) and mitimind-lite/docs/design/knowledge-engine.md (off/shadow only). These other-project documents informed method and boundaries only; no other-project task content was sent to TypeSafe.
