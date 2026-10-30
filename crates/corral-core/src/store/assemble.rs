//! Assembly of a stored task (plan T-253 section 4, "Full Task compatibility"): the head
//! document and every row of a task back into the [`TaskRecord`] [`super::codec::split`] took
//! apart, every nested vector in its stored order.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use super::codec::{AGENT_KINDS, FACTS, SELF, child_key, seq};
use crate::backend::{HistoryRow, TaskHeadDoc, TaskPart};
use crate::{CheckpointMeta, TaskRecord};

/// Every row of one task, by family and key.
pub struct TaskRows<'a>(BTreeMap<(TaskPart, &'a str), &'a HistoryRow>);

impl<'a> TaskRows<'a> {
    pub fn new(rows: impl IntoIterator<Item = &'a HistoryRow>) -> Self {
        Self(
            rows.into_iter()
                .map(|row| ((row.part, row.key.as_str()), row))
                .collect(),
        )
    }

    /// Rows of `part` whose key starts with `prefix`, in key order.
    pub fn under(&self, part: TaskPart, prefix: &str) -> Vec<&'a HistoryRow> {
        self.0
            .range((part, prefix)..)
            .take_while(|((p, key), _)| *p == part && key.starts_with(prefix))
            .map(|(_, row)| *row)
            .collect()
    }

    pub fn all(&self, part: TaskPart) -> Vec<&'a HistoryRow> {
        self.under(part, "")
    }

    pub fn get(&self, part: TaskPart, key: &str) -> Option<&'a HistoryRow> {
        self.0.get(&(part, key)).copied()
    }
}

fn docs(rows: Vec<&HistoryRow>) -> Vec<Value> {
    rows.into_iter().map(|row| row.doc.clone()).collect()
}

fn object(value: &Value, what: &str) -> Result<Map<String, Value>> {
    match value {
        Value::Object(map) => Ok(map.clone()),
        other => bail!("stored {what} is not an object: {other}"),
    }
}

fn set_list(map: &mut Map<String, Value>, key: &str, items: Vec<Value>) {
    if !items.is_empty() {
        map.insert(key.to_owned(), Value::Array(items));
    }
}

/// The ordinal of the last key segment (`000003` in `000001/000003`).
fn ordinal(key: &str) -> Result<usize> {
    let last = key.rsplit('/').next().unwrap_or(key);
    last.parse()
        .with_context(|| format!("stored row key {key:?} has no ordinal"))
}

/// The head document's specification and state merged into one task object.
pub fn head_object(head: &TaskHeadDoc) -> Result<Map<String, Value>> {
    let doc = object(&head.doc, "task head")?;
    let mut task = Map::new();
    for part in ["spec", "state"] {
        if let Some(Value::Object(members)) = doc.get(part) {
            task.extend(members.clone());
        }
    }
    task.insert("id".to_owned(), head.header.id.clone().into());
    if !task.contains_key("created") {
        task.insert(
            "created".to_owned(),
            serde_json::to_value(head.header.created)?,
        );
    }
    task.insert(
        "updated".to_owned(),
        serde_json::to_value(head.header.fields.updated)?,
    );
    Ok(task)
}

/// The body member of a head document.
pub fn head_body(head: &TaskHeadDoc) -> String {
    head.doc
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Reassembles a whole record from its head and every one of its rows.
///
/// # Errors
/// Refuses rows that do not decode into the task model.
pub fn assemble(head: &TaskHeadDoc, rows: &TaskRows<'_>) -> Result<TaskRecord> {
    let mut task = head_object(head)?;
    let mut runs = Vec::new();
    for run in rows.all(TaskPart::Run) {
        runs.push(run_value(rows, ordinal(&run.key)?, SELF, &run.doc)?);
    }
    set_list(&mut task, "runs", runs);
    let mut plan_runs = Vec::new();
    for run in rows.all(TaskPart::PlanRun) {
        let mut doc = object(&run.doc, "plan run")?;
        doc.remove(FACTS);
        inbox_value(rows, &format!("plan_run:{}", run.key), &mut doc)?;
        agent_value(rows, &format!("plan_run:{}", run.key), &mut doc);
        plan_runs.push(Value::Object(doc));
    }
    set_list(&mut task, "plan_runs", plan_runs);
    let mut plans = Vec::new();
    for plan in rows.all(TaskPart::Plan) {
        plans.push(plan_value(rows, &plan.key, &plan.doc)?);
    }
    set_list(&mut task, "plans", plans);
    let mut attempts = Vec::new();
    for attempt in rows.all(TaskPart::AggregateAttempt) {
        attempts.push(attempt_value(rows, &attempt.key, &attempt.doc)?);
    }
    set_list(&mut task, "aggregate_attempts", attempts);
    set_list(&mut task, "comments", docs(rows.all(TaskPart::Comment)));
    set_list(&mut task, "artifacts", docs(rows.all(TaskPart::Artifact)));
    set_list(
        &mut task,
        "approved_scope_history",
        docs(rows.all(TaskPart::ScopeRevision)),
    );
    set_list(
        &mut task,
        "agent_pm_receipts",
        docs(rows.all(TaskPart::PmReceipt)),
    );
    if let Some(Value::Object(gates)) = task.get_mut("member_gates") {
        for (member, gate) in gates.iter_mut() {
            let history = docs(rows.under(TaskPart::MemberGateHistory, &format!("{member}/")));
            if let Some(gate) = gate.as_object_mut() {
                set_list(gate, "history", history);
            }
        }
    }
    let mut decoded: crate::Task =
        serde_json::from_value(Value::Object(task)).context("assembling the stored task")?;
    decoded.body = head_body(head);
    let mut recoveries = Vec::new();
    for recovery in rows.all(TaskPart::Recovery) {
        let mut doc = object(&recovery.doc, "recovery")?;
        for (list, part) in [("outcomes", "outcome"), ("links", "link")] {
            let items = docs(rows.under(
                TaskPart::RecoveryOutcome,
                &format!("{}/{part}/", recovery.key),
            ));
            doc.insert(list.to_owned(), Value::Array(items));
        }
        recoveries
            .push(serde_json::from_value(Value::Object(doc)).context("assembling a recovery")?);
    }
    Ok(TaskRecord {
        task: decoded,
        recoveries,
    })
}

/// A coding run (`child` 0) or review child document with its nested history restored.
pub fn run_value(rows: &TaskRows<'_>, run_seq: usize, child: usize, doc: &Value) -> Result<Value> {
    let owner = child_key(run_seq, child);
    let mut run = object(doc, "run")?;
    run.remove(FACTS);
    if child == SELF {
        let mut children = Vec::new();
        for review in rows.under(TaskPart::ReviewRun, &format!("{}/", seq(run_seq))) {
            children.push(run_value(
                rows,
                run_seq,
                ordinal(&review.key)?,
                &review.doc,
            )?);
        }
        set_list(&mut run, "review_runs", children);
    }
    let prefix = format!("{owner}/");
    let entries = docs(rows.under(TaskPart::RunUsage, &prefix));
    if let Some(Value::Object(usage)) = run.get_mut("usage") {
        set_list(usage, "entries", entries);
    }
    let mut checkpoints = Vec::new();
    for meta in rows.under(TaskPart::RunCheckpoint, &prefix) {
        let meta: CheckpointMeta = serde_json::from_value(meta.doc.clone())?;
        let payload = rows
            .get(TaskPart::CheckpointPayload, &meta.payload_digest)
            .with_context(|| format!("checkpoint payload {} is missing", meta.payload_digest))?;
        checkpoints.push(serde_json::json!({"checkpoint": payload.doc, "artifact": meta.artifact}));
    }
    set_list(&mut run, "checkpoints", checkpoints);
    set_list(
        &mut run,
        "switches",
        docs(rows.under(TaskPart::RunSwitch, &prefix)),
    );
    let waits = docs(rows.under(TaskPart::RunObligation, &format!("{prefix}wait/")));
    let decisions = docs(rows.under(TaskPart::RunObligation, &format!("{prefix}decision/")));
    if !waits.is_empty() || !decisions.is_empty() {
        let mut obligations = match run.remove("obligations") {
            Some(Value::Object(scalars)) => scalars,
            _ => Map::new(),
        };
        set_list(&mut obligations, "waits", waits);
        set_list(&mut obligations, "decisions", decisions);
        run.insert("obligations".to_owned(), Value::Object(obligations));
    }
    inbox_value(rows, &format!("run:{owner}"), &mut run)?;
    agent_value(rows, &owner, &mut run);
    Ok(Value::Object(run))
}

fn agent_value(rows: &TaskRows<'_>, owner: &str, doc: &mut Map<String, Value>) {
    if let Some(Value::Object(host)) = doc.get_mut("agent_host") {
        for kind in AGENT_KINDS {
            set_list(
                host,
                kind,
                docs(rows.under(TaskPart::AgentRecord, &format!("{owner}/{kind}/"))),
            );
        }
    }
}

/// Restores an inbox's applied effects and scoped cursors into a container document.
pub fn inbox_value(rows: &TaskRows<'_>, owner: &str, doc: &mut Map<String, Value>) -> Result<()> {
    let prefix = format!("{owner}/");
    let effects = docs(rows.under(TaskPart::InboxEffect, &prefix));
    let cursors = docs(rows.under(TaskPart::ObserveCursor, &prefix));
    if let Some(Value::Object(inbox)) = doc.get_mut("inbox") {
        inbox.insert("effects_applied".to_owned(), Value::Array(effects));
        set_list(inbox, "observe_cursors", cursors);
    } else if !effects.is_empty() || !cursors.is_empty() {
        bail!("inbox rows of {owner} have no inbox to restore into");
    }
    Ok(())
}

pub fn plan_value(rows: &TaskRows<'_>, key: &str, doc: &Value) -> Result<Value> {
    let mut plan = object(doc, "plan")?;
    plan.remove(FACTS);
    let mut reviews = Vec::new();
    for review in rows.under(TaskPart::PlanReview, &format!("{key}/")) {
        let mut doc = object(&review.doc, "plan review")?;
        inbox_value(rows, &format!("plan_review:{}", review.key), &mut doc)?;
        agent_value(rows, &format!("plan_review:{}", review.key), &mut doc);
        reviews.push(Value::Object(doc));
    }
    set_list(&mut plan, "reviews", reviews);
    Ok(Value::Object(plan))
}

pub fn attempt_value(rows: &TaskRows<'_>, key: &str, doc: &Value) -> Result<Value> {
    let mut attempt = object(doc, "aggregate attempt")?;
    attempt.remove(FACTS);
    set_list(
        &mut attempt,
        "reviews",
        docs(rows.under(TaskPart::AggregateReview, &format!("{key}/"))),
    );
    Ok(Value::Object(attempt))
}
