use super::*;
use corral_core::backend::StoredEvent;
use corral_core::{
    Actor, FailureIdentity, FindingId, FindingMatch, FindingResolution, GateResult,
    RepairAttemptId, RepairAttemptKind, RepairEvent, RepairFinding, RepairStartReceipt, RunHead,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitSource {
    pub oid: String,
    pub tree: String,
    pub author: String,
    pub committer: String,
    pub parents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalEntry {
    pub source: String,
    pub source_digest: String,
    pub at: jiff::Timestamp,
    pub classification: String,
    pub actor: Option<String>,
    /// Author of the candidate submission, distinct from an autopilot issue's actor.
    pub submission_actor: Option<String>,
    pub run: Option<String>,
    pub profile: Option<String>,
    pub candidate: Option<String>,
    pub accounting: Option<RepairEvent>,
}

fn known_versions(doc: &serde_json::Value) -> Result<()> {
    match doc {
        serde_json::Value::Object(fields) => {
            if let Some(version) = fields.get("version") {
                ensure!(
                    matches!(version.as_u64(), Some(1 | 2)) || version.as_str() == Some("1"),
                    "unsupported canonical repair evidence version"
                );
            }
            for value in fields.values() {
                known_versions(value)?;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                known_versions(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn check_existing(source: &RepairSources) -> Result<()> {
    for area in [corral_core::REPAIR_RUNTIME, "profile-repairs"] {
        if let Some(row) = source.records.get(area).and_then(Option::as_ref) {
            let history = if area == corral_core::REPAIR_RUNTIME {
                let ledger: RepairLedger = serde_json::from_value(row.doc.clone())?;
                ensure!(
                    ledger.allowances.len() == 1
                        && ledger.active()?.diagnosis.is_none()
                        && ledger.active()?.renewal.is_none(),
                    "substantive repair ledger cannot be imported over"
                );
                ledger.active()?.history.clone()
            } else {
                serde_json::from_value::<RepairHistory>(row.doc.clone())?
            };
            ensure!(history.coding_blocker() == Some(crate::repair_runtime::LEGACY_BLOCKER)
                && history.counters()? == corral_core::RepairCounters { initial_implementations:1, ..Default::default() }
                && history.events().iter().all(|event| matches!(event, RepairEvent::Started { receipt } if receipt.kind == RepairAttemptKind::InitialImplementation)
                    || matches!(event,RepairEvent::Finding { .. } | RepairEvent::CodingBlocked { .. }))
                && history.context().is_none() && history.projection().is_none(), "incompatible substantive repair history");
        }
    }
    ensure!(
        source
            .records
            .get("repair-continuation")
            .is_some_and(Option::is_none),
        "pending corrective continuation must be reconciled first"
    );
    if let Some(row) = source.records.get("gate-triage").and_then(Option::as_ref) {
        // Keep diagnostic history verbatim; importing coding charges never resets it.
        let _: crate::gate_triage::GateTriage = serde_json::from_value(row.doc.clone())?;
    }
    for row in source.records.values().flatten() {
        known_versions(&row.doc)?;
    }
    for rows in source.rows.values() {
        for row in rows {
            known_versions(&row.doc)?;
        }
    }
    for event in &source.events {
        known_versions(&event.event.detail)?;
    }
    Ok(())
}

fn field<'a>(event: &'a StoredEvent, key: &str) -> Result<&'a str> {
    event.event.detail[key]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .with_context(|| {
            format!(
                "canonical {} event {} lacks {key}",
                event.event.kind, event.seq.0
            )
        })
}

fn commit(store: &Store, project: &str, oid: &str) -> Result<CommitSource> {
    ensure!(
        matches!(oid.len(), 40 | 64)
            && oid
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "submission lacks an immutable full commit ID"
    );
    let repo = store.project_path(project)?;
    let mut child = Command::new("git")
        .arg("--no-replace-objects")
        .arg("-C")
        .arg(&repo)
        .args(["cat-file", "commit", oid])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("reading immutable submission provenance")?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .context("git provenance stdout missing")?
        .take(65537)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 65536 {
        let _ = child.kill();
    }
    let status = child.wait().context("waiting for submission provenance")?;
    read.context("reading bounded submission provenance")?;
    ensure!(
        status.success() && bytes.len() <= 65536,
        "immutable submission provenance unavailable/over budget"
    );
    let mut object = format!("commit {}\0", bytes.len()).into_bytes();
    object.extend_from_slice(&bytes);
    let actual = if oid.len() == 40 {
        sha1_smol::Sha1::from(&object).digest().to_string()
    } else {
        BlobHash::of(&object).to_string()
    };
    ensure!(actual == oid, "immutable submission object hash mismatch");
    let text = String::from_utf8(bytes).context("submission provenance is not UTF-8")?;
    let headers = text
        .split_once("\n\n")
        .context("commit header terminator missing")?
        .0;
    let field = |name: &str| -> Result<String> {
        let values = headers
            .lines()
            .filter_map(|line| line.strip_prefix(name))
            .collect::<Vec<_>>();
        ensure!(
            values.len() == 1 && !values[0].trim().is_empty(),
            "ambiguous commit provenance field {name}"
        );
        Ok(values[0].to_owned())
    };
    let identity = |value: String| -> Result<String> {
        let parts = value.rsplitn(3, ' ').collect::<Vec<_>>();
        ensure!(
            parts.len() == 3 && parts[2].ends_with('>'),
            "invalid commit author identity"
        );
        Ok(parts[2].to_owned())
    };
    Ok(CommitSource {
        oid: oid.into(),
        tree: field("tree ")?,
        author: identity(field("author ")?)?,
        committer: identity(field("committer ")?)?,
        parents: headers
            .lines()
            .filter_map(|line| line.strip_prefix("parent "))
            .collect::<Vec<_>>()
            .join(" "),
    })
}

fn entry(event: &StoredEvent, class: &str, run: Option<&RunHead>) -> Result<HistoricalEntry> {
    if let Some(actor) = &event.event.actor {
        serde_json::from_value::<Actor>(serde_json::json!(actor))?;
    }
    Ok(HistoricalEntry {
        source: format!("event:{}", event.seq.0),
        source_digest: digest(event)?,
        at: event.event.ts,
        classification: class.into(),
        actor: event.event.actor.clone(),
        submission_actor: None,
        run: run.map(|r| r.id.clone()),
        profile: run.map(|r| r.profile.as_ref().unwrap_or(&r.agent).clone()),
        candidate: None,
        accounting: None,
    })
}

pub(super) fn reconstruct(
    store: &Store,
    head: &corral_core::TaskHead,
    source: &RepairSources,
) -> Result<(
    RepairPolicy,
    String,
    Vec<HistoricalEntry>,
    Vec<CommitSource>,
)> {
    ensure!(
        source.version == 1
            && source.schema == corral_core::STORE_SCHEMA_VERSION
            && source.end.complete
            && source.end.event_count == source.events.len(),
        "unsupported/incomplete repair chronology"
    );
    ensure!(
        source
            .events
            .first()
            .is_some_and(|e| e.event.kind == "created"),
        "canonical task genesis missing"
    );
    let project = head
        .project
        .as_deref()
        .context("repair import requires a canonical project repository")?;
    let mut runs = BTreeMap::new();
    for (index, row) in source
        .rows
        .get("run")
        .context("run traversal missing")?
        .iter()
        .enumerate()
    {
        let id = row.doc["id"].as_str().context("canonical run ID missing")?;
        ensure!(
            id == format!("R{}", index + 1),
            "canonical run chronology has a gap"
        );
        let run = head
            .run_head(id)?
            .context("canonical run projection missing")?;
        ensure!(
            run.outcome == Some(corral_core::RunOutcome::Done),
            "unsubmitted/interrupted historical writer"
        );
        if let Some(execution) = &run.execution {
            ensure!(
                execution.resolved.profile == run.agent
                    && execution.resolved.model == run.model
                    && execution.resolved.effort == run.effort
                    && run.harness.as_deref() == Some(execution.resolved.harness.as_str()),
                "historical execution/profile evidence contradicts run"
            );
        }
        ensure!(
            runs.insert(id.to_owned(), run).is_none(),
            "duplicate historical run"
        );
    }
    ensure!(
        !runs.is_empty() && runs.len() <= 16,
        "run chronology outside bounded importer"
    );
    ensure!(
        source.rows.get("run_switch").is_some_and(Vec::is_empty)
            && !source
                .events
                .iter()
                .any(|e| e.event.kind == "profile_switched"),
        "historical profile switches require an exact supported chronology"
    );
    let mut entries = Vec::new();
    let mut commits = Vec::new();
    let mut submitted_runs = BTreeSet::new();
    let mut initial_profile = None;
    let mut pending = None;
    let mut cap = None;
    let mut charges = 0u32;
    let mut last_submission: Option<(String, String)> = None;
    let mut last_issue = None;
    for event in &source.events {
        let issue = event.event.kind == "autopilot" && event.event.detail["to"] == "changes";
        let submission = matches!(event.event.kind.as_str(), "cowork_finished" | "done");
        if issue {
            ensure!(
                initial_profile.is_some() && pending.is_none(),
                "overlapping/initial corrective issuance is ambiguous"
            );
            let run = runs
                .get(field(event, "run")?)
                .context("issuance references unknown run")?;
            let (previous_run, previous_head) = last_submission
                .as_ref()
                .context("issuance has no immutable predecessor submission")?;
            ensure!(
                previous_run == &run.id
                    && event.event.detail["sha"]
                        .as_str()
                        .is_none_or(|sha| sha == previous_head),
                "issuance contradicts predecessor candidate/run"
            );
            last_issue = Some((run.id.clone(), previous_head.clone()));
            let round = u32::try_from(
                event.event.detail["round"]
                    .as_u64()
                    .context("issuance round missing")?,
            )?;
            let limit = u32::try_from(
                event.event.detail["max_rounds"]
                    .as_u64()
                    .context("historical allowance missing")?,
            )?;
            ensure!(
                limit > 0
                    && limit <= 64
                    && round == charges + 1
                    && round <= limit
                    && event.event.detail["effective_max_rounds"].as_u64()
                        == Some(u64::from(limit))
                    && event.event.detail["round_limit_source"] == "task_review_policy.max_rounds",
                "unsupported/unbounded historical issuance"
            );
            ensure!(
                cap.is_none_or(|old| old == limit),
                "historical allowance changed without an exact renewal"
            );
            cap = Some(limit);
            let reason = field(event, "reason")?;
            let identity = FailureIdentity {
                criterion_or_test: "unstructured legacy corrective finding".into(),
                affected_contract_or_location: format!("event:{}", event.seq.0),
                failure_code: "changes_requested".into(),
            };
            let mut observed = entry(event, "finding", Some(run))?;
            observed.accounting = Some(RepairEvent::Finding {
                finding: RepairFinding {
                    observation_id: format!("legacy-finding:{}:{}", source.store, event.seq.0),
                    finding_id: FindingId(identity.fingerprint()),
                    fingerprint: identity.fingerprint(),
                    identity,
                    observed_failure: reason.into(),
                    matching: FindingMatch::New,
                    attempted_fixes: Vec::new(),
                    resolution: FindingResolution::Unresolved,
                },
            });
            entries.push(observed);
            pending = Some((entries.len(), event.seq.0));
            entries.push(entry(event, "charged_repair", Some(run))?);
            charges = round;
        } else if submission {
            ensure!(
                event.event.actor.is_some(),
                "historical submission author missing"
            );
            let run = runs
                .get(field(event, "run")?)
                .context("submission references unknown run")?;
            let oid = if event.event.kind == "cowork_finished" {
                field(event, "head")?
            } else {
                let identity = &event.event.detail["identity"];
                ensure!(
                    identity["version"] == 2
                        && identity["task"] == head.id
                        && identity["run"] == run.id,
                    "submission lacks exact candidate identity"
                );
                identity["head"]
                    .as_str()
                    .context("submission candidate missing")?
            };
            let provenance = commit(store, project, oid)?;
            if event.event.kind == "done" {
                ensure!(
                    event.event.detail["identity"]["tree"] == provenance.tree,
                    "submission tree contradicts immutable object"
                );
            }
            if !commits.contains(&provenance) {
                commits.push(provenance);
            }
            let mut submitted = entry(event, "submission", Some(run))?;
            submitted.candidate = Some(oid.into());
            submitted.submission_actor = event.event.actor.clone();
            last_submission = Some((run.id.clone(), oid.to_owned()));
            if initial_profile.is_none() {
                ensure!(run.id == "R1", "initial submission is not canonical R1");
                let profile = submitted
                    .profile
                    .clone()
                    .context("initial profile missing")?;
                initial_profile = Some(profile.clone());
                submitted.accounting = Some(RepairEvent::Started {
                    receipt: RepairStartReceipt {
                        attempt_id: RepairAttemptId(format!(
                            "legacy:{}:{}:initial",
                            source.store, event.seq.0
                        )),
                        profile,
                        kind: RepairAttemptKind::InitialImplementation,
                        findings: Vec::new(),
                    },
                });
            } else {
                let (index, seq) = pending.take().context(
                    "submission has no proved corrective issuance; run count is not proof",
                )?;
                entries[index].profile = submitted.profile.clone();
                entries[index].candidate = submitted.candidate.clone();
                entries[index].submission_actor = submitted.submission_actor.clone();
                entries[index].accounting = Some(RepairEvent::Started {
                    receipt: RepairStartReceipt {
                        attempt_id: RepairAttemptId(format!(
                            "legacy:{}:{}:repair",
                            source.store, seq
                        )),
                        profile: submitted
                            .profile
                            .clone()
                            .context("corrective profile missing")?,
                        kind: RepairAttemptKind::CurrentProfileRepair,
                        findings: Vec::new(),
                    },
                });
            }
            submitted_runs.insert(run.id.clone());
            entries.push(submitted);
        } else {
            ensure!(
                !(event.event.kind == "changes_requested"
                    && event.event.detail["by"] != "autopilot"),
                "unclassified manual coding issuance"
            );
            ensure!(
                !matches!(
                    event.event.kind.as_str(),
                    "repair_allowance_renewed"
                        | "repair_attempt_started"
                        | "repair_diagnosis_started"
                ),
                "substantive managed accounting requires its own exact ledger"
            );
            let run = event.event.detail["run"]
                .as_str()
                .and_then(|id| runs.get(id));
            entries.push(entry(event, &event.event.kind, run)?);
        }
    }
    ensure!(
        pending.is_none() && submitted_runs.len() == runs.len(),
        "incomplete issuance/submission chronology"
    );
    ensure!(
        charges == head.rounds && charges > 0,
        "canonical issuance count contradicts task rounds"
    );
    if let Some(row) = source
        .records
        .get("completion-repair-cycle")
        .and_then(Option::as_ref)
    {
        let cycle: crate::repair_cycle::Cycle = serde_json::from_value(row.doc.clone())?;
        ensure!(
            cycle.completed
                && cycle.issued
                && cycle.round == charges
                && cycle.key.task == head.id
                && last_issue
                    .as_ref()
                    .is_some_and(|(run, sha)| cycle.key.run == *run && cycle.key.head == *sha),
            "repair-cycle record contradicts submissions"
        );
    }
    // Child records remain verbatim in the bounded source manifest, including earlier
    // failures, diagnoses, fixes and provenance. Do not invent chronology stamps for rows.
    // Latest gate failures are still unresolved even when no further coding was issued.
    // A terminal red check is an observation, never an extra repair.
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
    }
    entries.sort_by_key(|e| e.at); // stable sort preserves canonical event order at equal stamps
    let limit = cap.context("canonical historical allowance not proved")?;
    ensure!(
        charges == limit,
        "record-only importer requires a proved exhausted normal allowance"
    );
    let policy = RepairPolicy::V1 {
        current_profile_limit: limit,
        stronger_profile_limit: 0,
        overall_limit: limit,
        pm_diagnosis_allowance: 0,
    };
    let profile = initial_profile.context("canonical initial implementation missing")?;
    let mut projected = RepairHistory::projected(
        policy.clone(),
        &profile,
        RepairProjection {
            version: 1,
            evidence: BlobHash::parse(&source.digest()?)?.to_ref(),
            lineage: digest(&(source.store.clone(), &head.id))?,
        },
        Vec::new(),
    )?;
    for item in &mut entries {
        if let Some(event) = &mut item.accounting {
            match event {
                RepairEvent::Started { receipt }
                    if receipt.kind == RepairAttemptKind::CurrentProfileRepair =>
                {
                    receipt.findings = projected
                        .findings()
                        .into_iter()
                        .filter(|f| !matches!(f.resolution, FindingResolution::Resolved { .. }))
                        .map(|f| f.finding_id.clone())
                        .collect();
                }
                RepairEvent::Finding { finding } => {
                    let previous = projected.findings();
                    if previous.iter().any(|f| f.identity == finding.identity) {
                        finding.matching = FindingMatch::Exact {
                            finding: finding.finding_id.clone(),
                        };
                    } else if !previous.is_empty() {
                        finding.matching = FindingMatch::Uncertain {
                            candidates: previous.iter().map(|f| f.finding_id.clone()).collect(),
                            reason:
                                "canonical legacy observations provide no verified semantic match"
                                    .into(),
                        };
                    }
                }
                _ => {}
            }
            projected.apply(event.clone())?;
        }
    }
    ensure!(
        projected.counters()?.coding_repairs == charges,
        "projection lost a historical charge"
    );
    Ok((policy, profile, entries, commits))
}
