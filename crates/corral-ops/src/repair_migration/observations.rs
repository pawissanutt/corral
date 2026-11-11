//! Only interpreted observations may enter an exact legacy projection. Keeping an
//! uninterpreted child row in the evidence blob is not reconciliation of its findings.
use super::*;
use corral_core::{CandidateIdentity, GateResult, Verdict};

pub(super) fn supported_sources(source: &RepairSources) -> Result<()> {
    ensure!(
        !source
            .events
            .iter()
            .any(|e| e.event.kind.starts_with("gate_")
                || e.event.kind.starts_with("aggregate_gate")
                || e.event.kind.starts_with("checkpoint_gate")),
        "unsupported historical gate lifecycle chronology"
    );
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

/// Supported terminal event fields emitted by gate collection. Unknown evidence is not
/// silently ignored; lifecycle/attempt histories require a separate faithful interpreter.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GateObservation {
    run: String,
    sha: String,
    result: GateResult,
    failed_cmd: Option<String>,
    attempt: Option<String>,
    log_sha256: Option<String>,
    identity: Option<CandidateIdentity>,
    commands: Option<Vec<corral_core::CommandReceipt>>,
    tier: Option<String>,
    context_sha256: Option<String>,
    carry: Option<String>,
}

fn gate_events<'a>(
    runs: &BTreeMap<String, RunHead>,
    source: &'a RepairSources,
) -> Result<BTreeMap<String, &'a corral_core::backend::StoredEvent>> {
    let mut gates = BTreeMap::new();
    for event in source.events.iter().filter(|e| e.event.kind == "gate") {
        let wire: GateObservation = serde_json::from_value(event.event.detail.clone())
            .context("unsupported canonical gate event")?;
        for hash in [wire.log_sha256.as_deref(), wire.context_sha256.as_deref()]
            .into_iter()
            .flatten()
        {
            BlobHash::parse(hash).context("invalid gate event evidence digest")?;
        }
        let run = runs
            .get(&wire.run)
            .context("gate event references unknown run")?;
        let gate = run
            .gate
            .as_ref()
            .context("gate event has no canonical gate snapshot")?;
        ensure!(
            matches!(
                wire.result,
                GateResult::Pass | GateResult::Fail | GateResult::Stale
            ) && !gate.process_lost
                && wire.sha == gate.sha
                && wire.result == gate.result
                && wire.failed_cmd == gate.failed_cmd
                && event.event.ts >= gate.at,
            "gate event contradicts canonical candidate/result/command"
        );
        ensure!(
            wire.attempt.as_deref() == gate.identity.as_ref().map(|i| i.attempt.as_str())
                && wire.log_sha256 == gate.log_sha256
                && wire
                    .identity
                    .as_ref()
                    .is_none_or(|i| gate.identity.as_ref() == Some(i))
                && wire
                    .commands
                    .as_ref()
                    .is_none_or(|commands| commands == &gate.commands),
            "gate event contradicts canonical identity/command evidence"
        );
        // Reject explicit null/extra nested identity or command fields that typed decoding
        // alone could normalize away. Ordinary terminal events bind these through attempt/log.
        ensure!(
            event
                .event
                .detail
                .get("identity")
                .is_none_or(|actual| actual == &serde_json::json!(gate.identity))
                && event
                    .event
                    .detail
                    .get("commands")
                    .is_none_or(|actual| actual == &serde_json::json!(gate.commands)),
            "unsupported gate identity/command evidence"
        );
        let mut tier = serde_json::json!({});
        crate::gate::evidence::tier_detail(&mut tier, gate.tier.as_deref());
        ensure!(
            wire.tier.as_deref() == tier["tier"].as_str()
                && wire.context_sha256.as_deref() == tier["context_sha256"].as_str()
                && wire.carry.as_deref() == tier["carry"].as_str(),
            "gate event contradicts canonical tier evidence"
        );
        ensure!(
            gates.insert(run.id.clone(), event).is_none(),
            "multiple gate events cannot be reconciled to one snapshot"
        );
    }
    Ok(gates)
}

pub(super) fn append(
    runs: &BTreeMap<String, RunHead>,
    source: &RepairSources,
    entries: &mut Vec<HistoricalEntry>,
) -> Result<()> {
    let gates = gate_events(runs, source)?;
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
        if let Some(gate) = &run.gate {
            ensure!(
                !gate.process_lost
                    && matches!(
                        gate.result,
                        GateResult::Pass | GateResult::Fail | GateResult::Stale
                    ),
                "unsupported unfinished/cancelled canonical gate snapshot"
            );
        }
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
            let mut observed = HistoricalEntry {
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
            };
            if let Some(event) = gates.get(&run.id) {
                observed.source = format!("event:{}", event.seq.0);
                observed.source_digest = digest(event)?;
                observed.actor = event.event.actor.clone();
                observed.classification = "gate".into();
                let entry = entries
                    .iter_mut()
                    .find(|entry| entry.source == observed.source && entry.classification == "gate")
                    .context("canonical gate event missing from projection")?;
                *entry = observed;
            } else {
                entries.push(observed);
            }
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
