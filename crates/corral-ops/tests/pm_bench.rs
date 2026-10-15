//! Offline replay tests never construct a Ctx, contact a model, or execute card commands.
use std::path::{Path, PathBuf};

use corral_ops::PmActionKind;
use corral_ops::pm_bench::{PmBenchReport, PmBenchResponse, pm_bench};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn replay(responses: Option<&Path>) -> anyhow::Result<PmBenchReport> {
    pm_bench(
        &fixture("pm-2026-10-15.events.jsonl"),
        Some(&fixture("pm-2026-10-15.cases.json")),
        responses,
    )
}

fn responses(
    report: &PmBenchReport,
    model: &str,
    unsafe_choice: bool,
    cost: Option<f64>,
) -> Vec<PmBenchResponse> {
    report
        .cases
        .iter()
        .map(|case| PmBenchResponse {
            case_id: case.annotation.id.clone(),
            model: model.into(),
            option: if unsafe_choice {
                case.annotation.unsafe_options[0].clone()
            } else {
                case.annotation.expected_option.clone()
            },
            reported_cost_usd: cost,
        })
        .collect()
}

fn save(directory: &Path, responses: &[PmBenchResponse]) -> PathBuf {
    let path = directory.join("responses.jsonl");
    let mut bytes = String::new();
    for response in responses {
        bytes.push_str(&serde_json::to_string(response).unwrap());
        bytes.push('\n');
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn independently_labelled_history_covers_all_seven_criteria() {
    // Given authentic frozen history and separately authored evidence-linked annotations.
    // When the production PM policy is run on those checkpoints.
    let report = replay(None).unwrap();
    // Then all criteria are covered and the code-only baseline meets every label.
    assert_eq!(report.trace.len(), 50);
    assert_eq!(report.cases.len(), 8);
    assert_eq!(report.covered_criteria.len(), 7);
    assert!(report.missing_criteria.is_empty());
    assert_eq!(
        report.models.len(),
        1,
        "do not invent Sonnet/candidate results"
    );
    let baseline = &report.models[0];
    assert_eq!(baseline.model, "code-policy");
    assert_eq!(
        baseline.passed,
        report.cases.len(),
        "{:#?}",
        baseline.decisions
    );
    assert_eq!(baseline.policy_violations, 0);
    assert_eq!(baseline.unknown_cost_decisions, 0);
    assert!(
        baseline
            .total_cost_usd
            .is_some_and(|cost| cost.abs() < f64::EPSILON)
    );
    assert!(
        report
            .cases
            .iter()
            .all(|case| !case.annotation.evidence.is_empty())
    );
}

#[test]
fn replay_accumulates_prior_reviews_and_preserves_append_order() {
    // Given shipped history (embedded case labels must not drive the event trace).
    let events = fixture("pm-2026-10-15.events.jsonl");
    // When the entire historical stream is replayed.
    let report = pm_bench(&events, None, None).unwrap();
    // Then observations change with preceding events, not with a case's expected label.
    assert_eq!(report.trace[2].action.kind, PmActionKind::LoadFailure);
    assert_eq!(report.trace[4].action.kind, PmActionKind::Wait);
    assert_eq!(report.trace[8].action.kind, PmActionKind::GateLog);
    assert!(report.trace[8].event.ts < report.trace[7].event.ts);
    assert_eq!(report.trace[16].action.kind, PmActionKind::ReviewInFlight);
    assert_eq!(report.trace[17].action.kind, PmActionKind::StaleVerdict);
    assert_eq!(report.trace[17].action.task.as_deref(), Some("T-056"));
    assert_eq!(report.trace[31].action.kind, PmActionKind::Wait);
    assert_eq!(report.trace[44].action.kind, PmActionKind::ReviewInFlight);
    assert_eq!(report.trace[45].action.kind, PmActionKind::GateLog);
    assert_eq!(report.cases.len(), 8);
    assert!(report.missing_criteria.is_empty());
}

#[test]
fn unsafe_responses_fail_even_when_the_option_was_offered() {
    // Given one explicit unsafe option per independently annotated case.
    let report = replay(None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = save(
        temp.path(),
        &responses(&report, "candidate", true, Some(0.001)),
    );
    // When offline choices are scored.
    let scored = replay(Some(&path)).unwrap();
    // Then offering an option never makes it safe or correct.
    let model = &scored.models[1];
    assert_eq!(model.passed, 0);
    assert_eq!(model.policy_violations, report.cases.len());
    assert_eq!(model.missing_responses, 0);
    assert!(
        model
            .cost_per_decision_usd
            .is_some_and(|cost| (cost - 0.001).abs() < 1e-12)
    );
}

#[test]
fn unknown_cost_stays_unknown_for_correct_offline_decisions() {
    // Given imported choices with an opaque, non-provider model name and omitted prices.
    let report = replay(None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = save(
        temp.path(),
        &responses(&report, "offline://no-such-provider", false, None),
    );
    // When evaluated without a client, credentials, Ctx, or model endpoint.
    let scored = replay(Some(&path)).unwrap();
    // Then scoring is entirely local and never prices a model by its name.
    let model = &scored.models[1];
    assert_eq!(model.passed, report.cases.len());
    assert_eq!(model.unknown_cost_decisions, report.cases.len());
    assert!(
        model
            .decisions
            .iter()
            .all(|decision| decision.cost_usd.is_none())
    );
    assert!(model.total_cost_usd.is_none());
    assert!(model.cost_per_decision_usd.is_none());
}

#[test]
fn missing_responses_fail_instead_of_reducing_the_denominator() {
    // Given one response for a model with eight required decisions.
    let report = replay(None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let all = responses(&report, "sonnet-measured", false, Some(0.01));
    let path = save(temp.path(), &all[..1]);
    // When that partial response file is imported.
    let scored = replay(Some(&path)).unwrap();
    // Then absent choices are failed cases and absent costs are not zeros.
    let model = &scored.models[1];
    assert_eq!(model.passed, 1);
    assert_eq!(model.decisions.len(), report.cases.len());
    assert_eq!(model.missing_responses, report.cases.len() - 1);
    assert!(
        model
            .pass_rate
            .is_some_and(|rate| (rate - 0.125).abs() < 1e-12)
    );
    assert!(model.total_cost_usd.is_none());
    assert!(model.decisions[1..].iter().all(|decision| {
        !decision.passed && decision.failure.is_some() && decision.cost_usd.is_none()
    }));
}

#[test]
fn safe_alternative_is_not_automatically_the_independently_expected_choice() {
    // Given the optional second independent review instead of acting-owner acceptance.
    let report = replay(None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut choices = responses(&report, "candidate", false, Some(0.001));
    choices
        .iter_mut()
        .find(|r| r.case_id == "evidence-only-owner-judgment")
        .unwrap()
        .option = "independent_review".into();
    let path = save(temp.path(), &choices);
    // When scored against the frozen independent label.
    let scored = replay(Some(&path)).unwrap();
    // Then it is incorrect for this benchmark, but is not falsely called unsafe.
    let model = &scored.models[1];
    assert_eq!(model.passed, report.cases.len() - 1);
    assert_eq!(model.policy_violations, 0);
}

#[test]
fn duplicate_unknown_and_invalid_responses_are_rejected() {
    // Given response files with each distinct boundary violation.
    let report = replay(None).unwrap();
    let good = responses(&report, "candidate", false, Some(0.001));
    let mut duplicate = good.clone();
    duplicate.push(good[0].clone());
    let mut unknown_case = good.clone();
    unknown_case[0].case_id = "not-a-case".into();
    let mut unknown_option = good.clone();
    unknown_option[0].option = "not-an-option".into();
    let mut negative_cost = good.clone();
    negative_cost[0].reported_cost_usd = Some(-1.0);
    let mut reserved_model = good.clone();
    reserved_model[0].model = "code-policy".into();
    let invalid = [
        duplicate,
        unknown_case,
        unknown_option,
        negative_cost,
        reserved_model,
        vec![],
    ];
    // When each invalid document crosses the import boundary.
    let rejected: Vec<_> = invalid
        .iter()
        .map(|rows| {
            let temp = tempfile::tempdir().unwrap();
            replay(Some(&save(temp.path(), rows))).is_err()
        })
        .collect();
    // Then none silently changes the scored model/case population.
    assert!(rejected.into_iter().all(|rejected| rejected));
}

#[test]
fn digest_mismatch_rejects_annotations_for_different_history() {
    // Given valid annotations applied to a different (but valid) event prefix.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let bytes = std::fs::read_to_string(fixture("pm-2026-10-15.events.jsonl")).unwrap();
    std::fs::write(&path, format!("{}\n", bytes.lines().next().unwrap())).unwrap();
    // When linked snapshots are loaded.
    let result = pm_bench(&path, Some(&fixture("pm-2026-10-15.cases.json")), None);
    // Then they cannot be silently replayed at unrelated cutoffs.
    assert!(result.is_err());
}

#[test]
fn unrelated_history_has_unknown_coverage_without_explicit_annotations() {
    // Given a valid but unrecognized history prefix.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let bytes = std::fs::read_to_string(fixture("pm-2026-10-15.events.jsonl")).unwrap();
    std::fs::write(&path, format!("{}\n", bytes.lines().next().unwrap())).unwrap();
    // When no explicit annotation document is supplied.
    let report = pm_bench(&path, None, None).unwrap();
    // Then embedded labels do not apply by filename or event kind.
    assert!(report.cases.is_empty());
    assert_eq!(report.missing_criteria.len(), 7);
    assert!(report.models[0].pass_rate.is_none());
}

#[test]
fn exported_prompt_excludes_answer_and_unsafe_labels() {
    // Given an evaluated case with a separate prompt object for external harnesses.
    let report = replay(None).unwrap();
    // When serialized for model input.
    let prompt = serde_json::to_value(&report.cases[0].prompt).unwrap();
    // Then machine-consumed labels stay outside the prompt; no prose is pinned.
    assert!(prompt.get("expected_action").is_none());
    assert!(prompt.get("expected_option").is_none());
    assert!(prompt.get("unsafe_options").is_none());
    assert!(prompt.get("options").unwrap().is_array());
    assert!(prompt.get("observations").unwrap().is_array());
}
