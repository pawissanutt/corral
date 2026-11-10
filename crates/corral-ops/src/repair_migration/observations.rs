//! Only interpreted observations may enter an exact legacy projection. Keeping an
//! uninterpreted child row in the evidence blob is not reconciliation of its findings.
use super::*;
use corral_core::{CandidateIdentity, GateResult, Verdict};

pub(super) fn supported_sources(source: &RepairSources) -> Result<()> {
    for family in [
        "review_run",
        "finding",
        "review_evidence",
        "gate_attempt",
        "member_gate_history",
    ] {
        ensure!(
            source
                .rows
                .get(family)
                .with_context(|| format!("{family} traversal missing"))?
                .is_empty(),
            "unsupported historical {family} observation chronology"
        );
    }
    if let Some(row) = source.records.get("gate-triage").and_then(Option::as_ref) {
        let state: crate::gate_triage::GateTriage = serde_json::from_value(row.doc.clone())?;
        ensure!(
            state.failures.is_empty()
                && state.diagnostic_attempt.is_none()
                && state.diagnostic_history.is_empty()
                && state.owner_investigations.is_empty()
                && !state.retry_requested,
            "unsupported historical diagnostic observation chronology"
        );
    }
    Ok(())
}

fn identity_pin(
    identity: &CandidateIdentity,
    task: &str,
    run: &str,
    candidate: &str,
    commits: &[CommitSource],
) -> Result<()> {
    ensure!(
        identity.version == 2
            && identity.task == task
            && identity.run == run
            && identity.head == candidate
            && commits
                .iter()
                .any(|c| c.oid == candidate && c.tree == identity.tree),
        "canonical candidate identity contradicts submitted run/tree"
    );
    Ok(())
}

pub(super) fn candidate_pins(
    runs: &BTreeMap<String, RunHead>,
    latest: &BTreeMap<String, String>,
    current: &str,
    task: &str,
    commits: &[CommitSource],
) -> Result<()> {
    for run in runs.values() {
        let candidate = latest
            .get(&run.id)
            .context("run lacks an immutable submission")?;
        let audit = run
            .completion
            .audit
            .as_ref()
            .and_then(|a| a.identity.as_ref());
        let pins = [
            run.gate.as_ref().map(|g| g.sha.as_str()),
            run.review.as_ref().map(|r| r.sha.as_str()),
            run.autopilot.as_ref().and_then(|a| a.sha.as_deref()),
            audit.map(|a| a.head.as_str()),
        ];
        ensure!(
            run.id != current || pins.iter().any(Option::is_some),
            "terminal candidate has no independent canonical pin"
        );
        ensure!(
            pins.into_iter().flatten().all(|pin| pin == candidate),
            "canonical run/candidate pin contradicts latest submission"
        );
        for identity in [
            run.gate.as_ref().and_then(|g| g.identity.as_ref()),
            run.review.as_ref().and_then(|r| r.identity.as_ref()),
            audit,
        ]
        .into_iter()
        .flatten()
        {
            identity_pin(identity, task, &run.id, candidate, commits)?;
        }
    }
    Ok(())
}

pub(super) fn append(
    runs: &BTreeMap<String, RunHead>,
    source: &RepairSources,
    entries: &mut Vec<HistoricalEntry>,
) -> Result<()> {
    // A verdict event must resolve to a supported canonical snapshot. Older overwritten
    // verdicts/child findings cannot silently disappear merely because the last review exists.
    let mut verdicts = BTreeMap::new();
    for event in source.events.iter().filter(|e| e.event.kind == "verdict") {
        let run = runs
            .get(field(event, "run")?)
            .context("review event references unknown run")?;
        let review = run
            .review
            .as_ref()
            .context("review event has no canonical review snapshot")?;
        let verdict: Verdict = serde_json::from_value(event.event.detail["verdict"].clone())?;
        ensure!(
            review.verdict == Some(verdict)
                && field(event, "sha")? == review.sha
                && field(event, "reviewer")? == review.reviewer
                && event.event.detail["notes"].as_str() == review.notes.as_deref(),
            "unsupported or contradictory historical review event"
        );
        if let Some(at) = event.event.detail.get("review_at") {
            ensure!(
                at.as_str()
                    .context("invalid review event timestamp")?
                    .parse::<jiff::Timestamp>()?
                    == review.at,
                "review event timestamp contradicts canonical review"
            );
        }
        verdicts.insert(run.id.clone(), event);
    }
    for run in runs.values() {
        if let Some(gate) = &run.gate
            && matches!(gate.result, GateResult::Fail | GateResult::Stale)
        {
            let command = gate
                .failed_cmd
                .as_deref()
                .context("failed gate lacks command identity")?;
            let identity = FailureIdentity {
                criterion_or_test: command.into(),
                affected_contract_or_location: "configured gate".into(),
                failure_code: gate.result.as_str().into(),
            };
            entries.push(HistoricalEntry {
                source: format!("run:{}:gate", run.id),
                source_digest: digest(gate)?,
                at: gate.at,
                classification: "failed_check".into(),
                actor: None,
                submission_actor: None,
                run: Some(run.id.clone()),
                profile: Some(run.agent.clone()),
                candidate: Some(gate.sha.clone()),
                accounting: Some(RepairEvent::Finding {
                    finding: RepairFinding {
                        observation_id: format!(
                            "legacy-gate:{}:{}:{}",
                            source.store, run.id, gate.at
                        ),
                        finding_id: FindingId(identity.fingerprint()),
                        fingerprint: identity.fingerprint(),
                        identity,
                        observed_failure: format!("{}: {command}", gate.result.as_str()),
                        matching: FindingMatch::New,
                        attempted_fixes: Vec::new(),
                        resolution: FindingResolution::Unresolved,
                    },
                }),
            });
        }
        if let Some(review) = &run.review {
            ensure!(
                !review.cancelled && !review.process_lost,
                "unsupported interrupted historical review"
            );
            let mut observed = HistoricalEntry {
                source: format!("run:{}:review", run.id),
                source_digest: digest(review)?,
                at: review.at,
                classification: "review".into(),
                actor: Some(
                    match review.reviewer.as_str() {
                        "owner" => "owner",
                        "pm" => "pm",
                        _ => "reviewer",
                    }
                    .into(),
                ),
                submission_actor: None,
                run: Some(run.id.clone()),
                profile: Some(review.reviewer.clone()),
                candidate: Some(review.sha.clone()),
                accounting: None,
            };
            if let Some(event) = verdicts.get(&run.id) {
                observed.source = format!("event:{}", event.seq.0);
                observed.source_digest = digest(event)?;
                // Prefer actual event authorship to the nominal reviewer, retaining PM provenance.
                observed.actor = event.event.actor.clone();
            }
            match review
                .verdict
                .context("unsupported unfinished historical review")?
            {
                Verdict::Changes => {
                    let notes = review
                        .notes
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .context("changes review lacks canonical finding notes")?;
                    let identity = FailureIdentity {
                        criterion_or_test: "unstructured legacy review finding".into(),
                        affected_contract_or_location: format!(
                            "run:{}:review:{}:{}",
                            run.id, review.sha, review.at
                        ),
                        failure_code: "changes".into(),
                    };
                    observed.classification = "changes_review".into();
                    observed.accounting = Some(RepairEvent::Finding {
                        finding: RepairFinding {
                            observation_id: format!(
                                "legacy-review:{}:{}:{}:{}",
                                source.store, run.id, review.sha, review.at
                            ),
                            finding_id: FindingId(identity.fingerprint()),
                            fingerprint: identity.fingerprint(),
                            identity,
                            observed_failure: notes.into(),
                            matching: FindingMatch::New,
                            attempted_fixes: Vec::new(),
                            resolution: FindingResolution::Unresolved,
                        },
                    });
                }
                Verdict::Approve => {} // No machine finding identity means no invented resolution.
                Verdict::Escalate => anyhow::bail!("unsupported escalated historical review"),
            }
            entries.push(observed);
        }
    }
    Ok(())
}
