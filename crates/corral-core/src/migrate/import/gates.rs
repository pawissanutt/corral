//! Legacy gate attempts as the `gate_attempts` rows v2 readers look up (T-251 shapes, plan
//! T-253 section 17): a run's attempt keyed `(run, attempt)` on its task, an aggregate or
//! checkpoint attempt keyed `(aggregate|checkpoint, id)` on its Plan, each with its original
//! textual attempt key. A row starts from the legacy start (the `gate_started` event, else the
//! attempt's record) and ends only with a committed result (the `gate` event or the record):
//! a running or process-lost attempt has none and stays unended. The log, result and context
//! documents are the legacy files' exact bytes. Attempts without an identity (pre-identity
//! gates) have no key and stay legacy evidence only.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;

use super::Blobs;
use super::Collected;
use super::events::Line;
use crate::backend::{GateAttempt, HistoryRow, StoreBackend, TaskNum, TaskPart, TaskParts};
use crate::blob_ref::BlobHash;
use crate::migrate::blockers::LegacyBoard;
use crate::sqlite::SqliteBackend;
use crate::{CandidateIdentity, GateRecord, GateResult};

const RUN: &str = "run";
const AGGREGATE: &str = "aggregate";
const CHECKPOINT: &str = "checkpoint";

/// One attempt as the legacy store recorded it.
struct Legacy {
    task: String,
    run: String,
    attempt: String,
    scope: &'static str,
    identity: CandidateIdentity,
    commands: Vec<String>,
    started: Timestamp,
    launched: Option<String>,
    tier: Option<String>,
    /// The committed outcome: result, failed command and when it was recorded.
    outcome: Option<(GateResult, Option<String>, Timestamp)>,
    /// `state/gates/<stem>` or `state/plan-gates/<stem>`, without the extension.
    files: String,
}

#[derive(Deserialize)]
struct CheckpointJournal {
    attempts: Vec<CheckpointAttempt>,
}

#[derive(Deserialize)]
struct CheckpointAttempt {
    id: String,
    started: Timestamp,
    identity: CandidateIdentity,
    gate: GateRecord,
}

pub(super) type GateRows = BTreeMap<String, Vec<HistoryRow>>;

fn committed(record: &GateRecord) -> Option<(GateResult, Option<String>, Timestamp)> {
    (record.result != GateResult::Running)
        .then(|| (record.result, record.failed_cmd.clone(), record.at))
}

fn receipts(record: &GateRecord) -> Vec<String> {
    record.commands.iter().map(|c| c.command.clone()).collect()
}

fn tier(record: &GateRecord) -> Option<String> {
    record.tier.as_ref().map(|t| t.tier.as_str().to_owned())
}

/// The run attempts of one task from its events, completed and overridden by the run's record.
fn run_attempts(task: &crate::Task, events: &[&crate::Event]) -> Result<Vec<Legacy>> {
    let mut found: BTreeMap<(String, String), Legacy> = BTreeMap::new();
    for event in events {
        let identity = match event.detail.get("identity") {
            Some(identity) if event.kind == "gate_started" || event.kind == "gate" => {
                serde_json::from_value::<CandidateIdentity>(identity.clone())
                    .with_context(|| format!("{} {} event identity", task.id, event.kind))?
            }
            _ => continue,
        };
        let run = event.detail["run"]
            .as_str()
            .unwrap_or(&identity.run)
            .to_owned();
        let key = (run.clone(), identity.attempt.clone());
        if event.kind == "gate_started" {
            let commands = serde_json::from_value(event.detail["commands"].clone())
                .with_context(|| format!("{} gate_started commands", task.id))?;
            found.insert(
                key,
                Legacy {
                    task: task.id.clone(),
                    files: format!("state/gates/{}-{run}-{}", task.id, identity.attempt),
                    run,
                    attempt: identity.attempt.clone(),
                    scope: RUN,
                    commands,
                    started: event.ts,
                    launched: event.detail["pane"].as_str().map(str::to_owned),
                    tier: event.detail["tier"].as_str().map(str::to_owned),
                    identity,
                    outcome: None,
                },
            );
        } else if let Some(attempt) = found.get_mut(&key)
            && let Some(result) = event.detail["result"].as_str()
        {
            let result = serde_json::from_value(Value::from(result))
                .with_context(|| format!("{} gate result {result}", task.id))?;
            let failed = event.detail["failed_cmd"].as_str().map(str::to_owned);
            attempt.outcome = Some((result, failed, event.ts));
        }
    }
    for run in &task.runs {
        let Some(record) = &run.gate else { continue };
        let Some(identity) = &record.identity else {
            continue;
        };
        let key = (run.id.clone(), identity.attempt.clone());
        let attempt = found.entry(key).or_insert_with(|| Legacy {
            task: task.id.clone(),
            run: run.id.clone(),
            attempt: identity.attempt.clone(),
            scope: RUN,
            identity: identity.clone(),
            commands: receipts(record),
            started: record.at,
            launched: None,
            tier: None,
            outcome: None,
            files: format!("state/gates/{}-{}-{}", task.id, run.id, identity.attempt),
        });
        // The record is the committed state of the attempt it names: a running or
        // process-lost record has no result, whatever the attempt's files say.
        attempt.outcome = committed(record);
        attempt.launched = record.pane.clone().or(attempt.launched.take());
        attempt.tier = tier(record).or(attempt.tier.take());
    }
    Ok(found.into_values().collect())
}

fn plan_attempt(
    task: &crate::Task,
    scope: &'static str,
    id: &str,
    started: Timestamp,
    identity: &CandidateIdentity,
    record: &GateRecord,
) -> Legacy {
    Legacy {
        task: task.id.clone(),
        run: scope.to_owned(),
        attempt: id.to_owned(),
        scope,
        identity: identity.clone(),
        commands: receipts(record),
        started,
        launched: record.pane.clone(),
        tier: tier(record),
        outcome: committed(record),
        files: format!("state/plan-gates/{}-{id}", task.id),
    }
}

fn attempts(corral: &Path, board: &LegacyBoard, lines: &[Line]) -> Result<Vec<Legacy>> {
    let mut by_task: BTreeMap<&str, Vec<&crate::Event>> = BTreeMap::new();
    for event in lines.iter().filter_map(|line| line.event.as_ref()) {
        if let Some(task) = event.task.as_deref() {
            by_task.entry(task).or_default().push(event);
        }
    }
    let mut out = Vec::new();
    for (legacy, _) in &board.tasks {
        let task = &legacy.task;
        let events = by_task.get(task.id.as_str()).map_or(&[][..], Vec::as_slice);
        out.extend(run_attempts(task, events)?);
        for attempt in &task.aggregate_attempts {
            if let Some(record) = &attempt.gate {
                out.push(plan_attempt(
                    task,
                    AGGREGATE,
                    &attempt.id,
                    attempt.started,
                    &attempt.identity,
                    record,
                ));
            }
        }
        let journal = corral.join(format!("state/plan-checkpoints/{}.json", task.id));
        if journal.is_file() {
            let bytes = std::fs::read(&journal)
                .with_context(|| format!("reading {}", journal.display()))?;
            let journal: CheckpointJournal = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing the {} checkpoint journal", task.id))?;
            for c in &journal.attempts {
                out.push(plan_attempt(
                    task,
                    CHECKPOINT,
                    &c.id,
                    c.started,
                    &c.identity,
                    &c.gate,
                ));
            }
        }
    }
    Ok(out)
}

/// The blob of a legacy gate document: the kept file's blob, else its bytes stored now (a
/// document whose bytes are an artifact's content is owned by this row as well).
fn document(
    corral: &Path,
    rel: &str,
    blobs: &mut Blobs<'_>,
    collected: &Collected,
) -> Result<Option<BlobHash>> {
    if let Ok(hash) = collected.blob_of(rel) {
        return Ok(Some(hash));
    }
    let path = corral.join(rel);
    if !path.is_file() {
        return Ok(None);
    }
    blobs.put_file(&path).map(Some)
}

fn json_blob(blobs: &mut Blobs<'_>, value: &impl serde::Serialize) -> Result<BlobHash> {
    let bytes = serde_json::to_vec(value).context("encoding a gate document")?;
    blobs.put(&mut bytes.as_slice()).map(|(hash, _)| hash)
}

pub(super) fn import(
    corral: &Path,
    board: &LegacyBoard,
    lines: &[Line],
    blobs: &mut Blobs<'_>,
    collected: &Collected,
) -> Result<GateRows> {
    let mut rows = GateRows::new();
    for a in attempts(corral, board, lines)? {
        let log = document(corral, &format!("{}.log", a.files), blobs, collected)?;
        let result_doc = document(corral, &format!("{}.json", a.files), blobs, collected)?;
        let context = document(
            corral,
            &format!("{}.context.json", a.files),
            blobs,
            collected,
        )?;
        let (result, failed_cmd, ended) = match a.outcome {
            Some((result, failed, at)) => (Some(result.as_str().to_owned()), failed, Some(at)),
            None => (None, None, None),
        };
        let row = GateAttempt {
            run: a.run.clone(),
            attempt: a.attempt.clone(),
            scope: a.scope.to_owned(),
            sha: a.identity.head.clone(),
            tier: a.tier,
            identity: json_blob(blobs, &a.identity)?.to_ref(),
            commands: json_blob(blobs, &a.commands)?.to_ref(),
            context: context.map(|h| h.to_ref()),
            launched: a.launched.map(Value::String),
            result,
            failed_cmd,
            log: log.map(|h| h.to_ref()),
            result_doc: result_doc.map(|h| h.to_ref()),
            started: a.started,
            ended,
        };
        rows.entry(a.task).or_default().push(HistoryRow {
            part: TaskPart::GateAttempt,
            key: row.key(),
            current: true,
            open: row.ended.is_none(),
            doc: serde_json::to_value(&row).context("encoding a gate attempt row")?,
            index: Vec::new(),
        });
    }
    Ok(rows)
}

/// Every imported gate attempt row reads back unchanged by its key.
pub(super) fn verify(backend: &SqliteBackend, expected: &GateRows) -> Result<()> {
    let view = backend.view()?;
    for (task, rows) in expected {
        let num = TaskNum(crate::store::id_number(task));
        let doc = view
            .task(num, &TaskParts::Only(vec![TaskPart::GateAttempt]))?
            .with_context(|| format!("{task} is missing from the imported database"))?;
        let stored: BTreeMap<&str, &Value> = doc
            .rows
            .iter()
            .filter(|r| r.part == TaskPart::GateAttempt)
            .map(|r| (r.key.as_str(), &r.doc))
            .collect();
        for row in rows {
            ensure!(
                stored.get(row.key.as_str()) == Some(&&row.doc),
                "{task} gate attempt {} does not read back as imported",
                row.key
            );
        }
    }
    Ok(())
}
