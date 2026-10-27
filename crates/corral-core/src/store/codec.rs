//! The task codec (plan T-253 section 4, "Parts"): a [`TaskRecord`] as one head document plus
//! one row per container, per nested child and per history element, and back.
//!
//! Containers (coding runs, review children, plan runs, plans, aggregate attempts) are stored
//! without their nested history collections, which become child rows keyed under the
//! container; each container row carries its [`ContainerFacts`] under [`FACTS`]. Keys are
//! zero-padded ordinals so key order is list order:
//!
//! - `Run` `<run>`; `ReviewRun` `<run>/<child>`; the nested rows of a run (child `000000`) or of
//!   a review child use the owner `<run>/<child>`: `RunUsage`, `RunCheckpoint`, `RunSwitch`
//!   `<owner>/<n>`, `RunObligation` `<owner>/wait|decision/<n>`, `AgentRecord`
//!   `<owner>/<kind>/<n>`; inbox rows use `run:<owner>`, `plan_run:<seq>` or
//!   `plan_review:<plan>/<n>` as their owner.
//! - `PlanRun`, `Plan`, `AggregateAttempt` `<seq>`; `PlanReview`, `AggregateReview` `<seq>/<n>`.
//! - `Comment` and `Artifact` `<n:010>` (the backend's comment and artifact keys); the other
//!   history elements `<n>`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::backend::{HistoryRow, TaskEdge, TaskFields, TaskPart};
use crate::container_facts::{herdr_panes, payload_digest, plan_run_panes, run_panes};
use crate::{
    CheckpointMeta, ContainerFacts, HistorySummary, PaneRef, RecoveryRecord, Run, Task, TaskRecord,
};

/// The member of a container row document holding its facts.
pub const FACTS: &str = "_facts";
/// The child ordinal of a run's own nested rows.
pub const SELF: usize = 0;

/// Head fields that make up the specification; the rest of the head is state.
const SPEC_KEYS: &[&str] = &[
    "title",
    "project",
    "kind",
    "size",
    "priority",
    "depends_on",
    "labels",
    "write_scope",
    "completion",
    "design",
    "plan",
    "plan_required",
    "tracker",
    "approved_scope",
    "approved_scope_revision",
    "parent",
    "children",
    "source_digest",
    "profile",
    "reviewer",
    "review",
];

/// The task vectors that become rows.
const HISTORY_VECTORS: &[&str] = &[
    "runs",
    "plan_runs",
    "plans",
    "comments",
    "artifacts",
    "aggregate_attempts",
    "approved_scope_history",
    "agent_pm_receipts",
];

/// Pi-native record lists of a run's host state, each one row per record.
pub const AGENT_KINDS: [&str; 6] = [
    "generations",
    "receipts",
    "questions",
    "turns",
    "wakes",
    "reports",
];

pub fn seq(n: usize) -> String {
    format!("{n:06}")
}

pub fn child_key(run: usize, child: usize) -> String {
    format!("{run:06}/{child:06}")
}

pub fn ordinal_key(n: usize) -> String {
    format!("{n:010}")
}

/// The specification revision of a task (the progress and resume contract's `spec_revision`).
///
/// # Errors
/// Returns serialization errors.
pub fn spec_revision(task: &Task) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &task.id,
            &task.title,
            &task.project,
            &task.body,
            &task.completion,
        ))?)
    ))
}

/// A task split into what the backend stores.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub fields: TaskFields,
    pub doc: Value,
    pub summary: Value,
    pub rows: BTreeMap<(TaskPart, String), HistoryRow>,
}

fn row(part: TaskPart, key: String, current: bool, open: bool, doc: Value) -> HistoryRow {
    HistoryRow {
        part,
        key,
        current,
        open,
        doc,
        index: Vec::new(),
    }
}

pub(crate) struct Rows(pub(crate) BTreeMap<(TaskPart, String), HistoryRow>);

impl Rows {
    pub(crate) fn new() -> Rows {
        Rows(BTreeMap::new())
    }

    pub(crate) fn put(&mut self, row: HistoryRow) {
        self.0.insert((row.part, row.key.clone()), row);
    }

    fn put_indexed(&mut self, mut row: HistoryRow, index: Vec<String>) {
        row.index = index;
        self.put(row);
    }
}

fn take_array(object: &mut Map<String, Value>, key: &str) -> Vec<Value> {
    match object.remove(key) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

pub(crate) fn object(value: Value, what: &str) -> Result<Map<String, Value>> {
    match value {
        Value::Object(map) => Ok(map),
        other => bail!("{what} is not an object: {other}"),
    }
}

pub fn task_fields(task: &Task) -> Result<TaskFields> {
    let mut edges: Vec<TaskEdge> = task
        .depends_on
        .iter()
        .map(|other| TaskEdge {
            kind: "depends_on".to_owned(),
            other: other.clone(),
        })
        .chain(task.children.iter().map(|other| TaskEdge {
            kind: "child".to_owned(),
            other: other.clone(),
        }))
        .chain(task.plan.iter().map(|plan| TaskEdge {
            kind: "plan".to_owned(),
            other: plan.split('@').next().unwrap_or(plan).to_owned(),
        }))
        .collect();
    let mut seen = BTreeSet::new();
    edges.retain(|edge| seen.insert(edge.clone()));
    Ok(TaskFields {
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        project: task.project.clone(),
        priority: task.priority.clone(),
        parent: task.parent.clone(),
        updated: task.updated,
        edges,
        kind: task.kind.clone(),
        size: task
            .size
            .map(|size| {
                serde_json::to_value(size).map(|v| v.as_str().unwrap_or_default().to_owned())
            })
            .transpose()?,
        tracker: task.tracker,
        design: task.design,
        queued: task.queued,
        spec_rev: spec_revision(task)?,
        dashboard_due: task
            .dashboard
            .as_ref()
            .filter(|schedule| !schedule.paused)
            .map(|schedule| schedule.next_due),
    })
}

/// The head document of a task: `{spec, state, body}` without any history vector, member-gate
/// history, `id` or `updated` (the header carries those two).
pub fn head_doc(task: &Task) -> Result<Value> {
    let mut map = object(serde_json::to_value(task)?, "task")?;
    for key in HISTORY_VECTORS.iter().chain(&["id", "updated"]) {
        map.remove(*key);
    }
    if let Some(Value::Object(gates)) = map.get_mut("member_gates") {
        for gate in gates.values_mut() {
            if let Some(gate) = gate.as_object_mut() {
                gate.remove("history");
            }
        }
    }
    let mut spec = Map::new();
    for key in SPEC_KEYS {
        if let Some(value) = map.remove(*key) {
            spec.insert((*key).to_owned(), value);
        }
    }
    Ok(json!({"spec": spec, "state": map, "body": task.body}))
}

/// Splits a whole record into its stored parts.
///
/// # Errors
/// Returns serialization or fact-derivation errors.
pub fn split(record: &TaskRecord) -> Result<Split> {
    let task = &record.task;
    let mut rows = Rows::new();
    let value = object(serde_json::to_value(task)?, "task")?;
    let vector = |key: &str| match value.get(key) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let runs = vector("runs");
    for (i, (run, doc)) in task.runs.iter().zip(runs).enumerate() {
        split_run(&mut rows, i + 1, SELF, run, doc, i + 1 == task.runs.len())?;
    }
    for (i, (run, doc)) in task.plan_runs.iter().zip(vector("plan_runs")).enumerate() {
        plan_run_rows(&mut rows, i + 1, run, doc, i + 1 == task.plan_runs.len())?;
    }
    for (i, (plan, doc)) in task.plans.iter().zip(vector("plans")).enumerate() {
        plan_rows(&mut rows, i + 1, plan, doc, i + 1 == task.plans.len())?;
    }
    for (i, (attempt, doc)) in task
        .aggregate_attempts
        .iter()
        .zip(vector("aggregate_attempts"))
        .enumerate()
    {
        let current = i + 1 == task.aggregate_attempts.len();
        attempt_rows(&mut rows, i + 1, attempt, doc, current)?;
    }
    for (i, doc) in vector("comments").into_iter().enumerate() {
        rows.put(row(
            TaskPart::Comment,
            ordinal_key(i + 1),
            false,
            false,
            doc,
        ));
    }
    for (i, (artifact, doc)) in task.artifacts.iter().zip(vector("artifacts")).enumerate() {
        rows.put_indexed(
            row(TaskPart::Artifact, ordinal_key(i + 1), false, false, doc),
            super::head_api::artifact_indexes(artifact),
        );
    }
    for (i, doc) in vector("approved_scope_history").into_iter().enumerate() {
        rows.put(row(TaskPart::ScopeRevision, seq(i + 1), false, false, doc));
    }
    for (i, (receipt, doc)) in task
        .agent_pm_receipts
        .iter()
        .zip(vector("agent_pm_receipts"))
        .enumerate()
    {
        rows.put_indexed(
            row(TaskPart::PmReceipt, seq(i + 1), false, false, doc),
            vec![format!("request:{}", receipt.request_id)],
        );
    }
    if let Some(Value::Object(gates)) = value.get("member_gates") {
        for (member, gate) in gates {
            let history = gate.get("history").and_then(Value::as_array);
            for (i, doc) in history.into_iter().flatten().enumerate() {
                rows.put(row(
                    TaskPart::MemberGateHistory,
                    format!("{member}/{}", seq(i + 1)),
                    false,
                    false,
                    doc.clone(),
                ));
            }
        }
    }
    split_recoveries(&mut rows, &record.recoveries)?;
    for pane in task_panes(task) {
        rows.put(row(
            TaskPart::PaneRef,
            pane.key(),
            false,
            false,
            json!({"socket": pane.socket, "workspace": pane.workspace}),
        ));
    }
    Ok(Split {
        fields: task_fields(task)?,
        doc: head_doc(task)?,
        summary: serde_json::to_value(HistorySummary::of(task)?)?,
        rows: rows.0,
    })
}

/// The row of the planning run at 1-based `n` and its inbox rows.
pub(crate) fn plan_run_rows(
    rows: &mut Rows,
    n: usize,
    run: &crate::PlanRun,
    doc: Value,
    current: bool,
) -> Result<()> {
    let key = seq(n);
    let mut doc = object(doc, "plan run")?;
    split_inbox(rows, &format!("plan_run:{key}"), &mut doc)?;
    doc.insert(
        FACTS.to_owned(),
        serde_json::to_value(ContainerFacts::of_plan_run(&key, run))?,
    );
    rows.put_indexed(
        row(
            TaskPart::PlanRun,
            key,
            current,
            run.ended.is_none(),
            Value::Object(doc),
        ),
        vec![format!("id:{}", run.id)],
    );
    Ok(())
}

pub(crate) fn plan_review_tokens(
    review_id: &str,
    reviewer: &str,
    verdict: Option<crate::Verdict>,
) -> Vec<String> {
    let verdict = verdict.map_or("none", |v| v.as_str());
    vec![
        format!("id:{review_id}"),
        format!("by:{reviewer}:{verdict}"),
    ]
}

/// The row of review `r` (1-based) of the plan at `plan_key` and its inbox rows.
pub(crate) fn plan_review_rows(
    rows: &mut Rows,
    plan_key: &str,
    r: usize,
    review: &crate::PlanReview,
    doc: Value,
    current: bool,
) -> Result<()> {
    let review_key = format!("{plan_key}/{}", seq(r));
    let mut doc = object(doc, "plan review")?;
    split_inbox(rows, &format!("plan_review:{review_key}"), &mut doc)?;
    rows.put_indexed(
        row(
            TaskPart::PlanReview,
            review_key,
            current,
            review.verdict.is_none(),
            Value::Object(doc),
        ),
        plan_review_tokens(&review.id, &review.reviewer, review.verdict),
    );
    Ok(())
}

/// The row of the plan at 1-based `n` and every row of its reviews.
pub(crate) fn plan_rows(
    rows: &mut Rows,
    n: usize,
    plan: &crate::PlanRecord,
    doc: Value,
    current: bool,
) -> Result<()> {
    let key = seq(n);
    let mut doc = object(doc, "plan")?;
    let reviews = take_array(&mut doc, "reviews");
    for (r, (review, review_doc)) in plan.reviews.iter().zip(reviews).enumerate() {
        plan_review_rows(
            rows,
            &key,
            r + 1,
            review,
            review_doc,
            r + 1 == plan.reviews.len(),
        )?;
    }
    doc.insert(
        FACTS.to_owned(),
        serde_json::to_value(ContainerFacts::of_plan(&key, plan))?,
    );
    rows.put_indexed(
        row(
            TaskPart::Plan,
            key,
            current,
            plan.approval.is_none(),
            Value::Object(doc),
        ),
        vec![format!("id:{}", plan.id)],
    );
    Ok(())
}

/// The row of the aggregate attempt at 1-based `n` and every row of its reviews.
pub(crate) fn attempt_rows(
    rows: &mut Rows,
    n: usize,
    attempt: &crate::AggregateAttempt,
    doc: Value,
    current: bool,
) -> Result<()> {
    let key = seq(n);
    let mut doc = object(doc, "aggregate attempt")?;
    let reviews = take_array(&mut doc, "reviews");
    for (r, (review, review_doc)) in attempt.reviews.iter().zip(reviews).enumerate() {
        rows.put_indexed(
            row(
                TaskPart::AggregateReview,
                format!("{key}/{}", seq(r + 1)),
                r + 1 == attempt.reviews.len(),
                review.pending(),
                review_doc,
            ),
            vec![format!("id:{}", review.id)],
        );
    }
    doc.insert(
        FACTS.to_owned(),
        serde_json::to_value(ContainerFacts::of_attempt(&key, attempt))?,
    );
    rows.put_indexed(
        row(
            TaskPart::AggregateAttempt,
            key,
            current,
            attempt.acceptance.is_none(),
            Value::Object(doc),
        ),
        vec![format!("id:{}", attempt.id)],
    );
    Ok(())
}

/// The index tokens of a coding run (`child` 0) or review child row.
pub(crate) fn run_tokens(
    id: &str,
    child: usize,
    review: Option<&crate::ReviewRecord>,
    outcome: Option<crate::RunOutcome>,
) -> Vec<String> {
    let mut index = vec![format!("id:{id}")];
    if child != SELF
        && let Some(review) = review
    {
        index.push(format!("at:{}", review.at));
        if outcome == Some(crate::RunOutcome::Superseded)
            && review
                .route
                .as_deref()
                .is_some_and(|route| route.stronger_request.is_some())
        {
            index.push(format!("stronger:{}", review.sha));
        }
    }
    index
}

/// Every pane reference a task's containers recorded, as `pane_refs` rows derive them.
pub fn task_panes(task: &Task) -> Vec<PaneRef> {
    let mut panes = Vec::new();
    panes.extend(launch_pane(task.launch.as_ref()));
    for (i, run) in task.runs.iter().enumerate() {
        panes.extend(run_panes(&format!("run:{}", child_key(i + 1, SELF)), run));
        for (c, child) in run.review_runs.iter().enumerate() {
            panes.extend(run_panes(
                &format!("run:{}", child_key(i + 1, c + 1)),
                child,
            ));
        }
    }
    for (i, run) in task.plan_runs.iter().enumerate() {
        panes.extend(plan_run_panes(&format!("plan_run:{}", seq(i + 1)), run));
    }
    for (i, plan) in task.plans.iter().enumerate() {
        for (r, review) in plan.reviews.iter().enumerate() {
            panes.extend(herdr_panes(
                &format!("plan_review:{}", child_key(i + 1, r + 1)),
                "plan_review",
                review.herdr.as_ref(),
            ));
        }
    }
    for (i, attempt) in task.aggregate_attempts.iter().enumerate() {
        for (r, review) in attempt.reviews.iter().enumerate() {
            panes.extend(herdr_panes(
                &format!("aggregate_review:{}", child_key(i + 1, r + 1)),
                "aggregate_review",
                review.herdr.as_ref(),
            ));
        }
    }
    panes
}

/// A launch reserves server ownership before a pane exists. Its empty pane identity is
/// distinguished from actual pane references by the reservation kind.
pub(crate) fn launch_pane(launch: Option<&crate::LaunchReservation>) -> Option<PaneRef> {
    launch.map(|launch| PaneRef {
        owner: "launch".into(),
        kind: "reservation".into(),
        pane: String::new(),
        socket: launch
            .socket
            .as_deref()
            .map(|socket| socket.to_string_lossy().into_owned())
            .unwrap_or_default(),
        workspace: None,
    })
}

/// The row of one coding run (`child` 0) or review child and all of its nested rows.
pub(crate) fn split_run(
    rows: &mut Rows,
    run_seq: usize,
    child: usize,
    run: &Run,
    doc: Value,
    current: bool,
) -> Result<()> {
    let owner = child_key(run_seq, child);
    let mut doc = object(doc, "run")?;
    if child == SELF {
        let children = take_array(&mut doc, "review_runs");
        for (c, (review, review_doc)) in run.review_runs.iter().zip(children).enumerate() {
            let last = c + 1 == run.review_runs.len();
            split_run(rows, run_seq, c + 1, review, review_doc, last)?;
        }
    }
    split_nested(rows, &owner, run, &mut doc)?;
    doc.insert(
        FACTS.to_owned(),
        serde_json::to_value(ContainerFacts::of_run(&owner, run)?)?,
    );
    let (part, key) = if child == SELF {
        (TaskPart::Run, seq(run_seq))
    } else {
        (TaskPart::ReviewRun, owner)
    };
    let index = run_tokens(&run.id, child, run.review.as_ref(), run.outcome);
    rows.put_indexed(
        row(part, key, current, run.ended.is_none(), Value::Object(doc)),
        index,
    );
    Ok(())
}

/// Moves a run's nested history out of its document into rows under `owner`.
fn split_nested(
    rows: &mut Rows,
    owner: &str,
    run: &Run,
    doc: &mut Map<String, Value>,
) -> Result<()> {
    if let Some(Value::Object(usage)) = doc.get_mut("usage") {
        for (i, entry) in take_array(usage, "entries").into_iter().enumerate() {
            rows.put(row(
                TaskPart::RunUsage,
                format!("{owner}/{}", seq(i + 1)),
                false,
                false,
                entry,
            ));
        }
    }
    doc.remove("checkpoints");
    for (i, record) in run.checkpoints.iter().enumerate() {
        let meta = CheckpointMeta::of(crate::container_facts::len_u64(i + 1), record)?;
        let payload = serde_json::to_value(&record.checkpoint)?;
        rows.put(row(
            TaskPart::CheckpointPayload,
            payload_digest(&payload)?,
            false,
            false,
            payload,
        ));
        rows.put_indexed(
            row(
                TaskPart::RunCheckpoint,
                format!("{owner}/{}", seq(i + 1)),
                false,
                false,
                serde_json::to_value(&meta)?,
            ),
            vec![
                format!("artifact:{}", record.artifact),
                format!("payload:{}", meta.payload_digest),
            ],
        );
    }
    for (i, switch) in take_array(doc, "switches").into_iter().enumerate() {
        rows.put(row(
            TaskPart::RunSwitch,
            format!("{owner}/{}", seq(i + 1)),
            false,
            false,
            switch,
        ));
    }
    if let Some(Value::Object(mut obligations)) = doc.remove("obligations") {
        let waits = take_array(&mut obligations, "waits");
        for (i, (wait, doc)) in run.obligations.waits.iter().zip(waits).enumerate() {
            let mut index = vec![format!("id:{}", wait.request.id)];
            index.extend(wait.receipt.iter().map(|r| format!("receipt:{}", r.id)));
            if crate::card::card_obligation_visible("wait", &doc) {
                index.push("card".into());
            }
            rows.put_indexed(
                row(
                    TaskPart::RunObligation,
                    format!("{owner}/wait/{}", seq(i + 1)),
                    false,
                    wait.receipt.is_none(),
                    doc,
                ),
                index,
            );
        }
        let decisions = take_array(&mut obligations, "decisions");
        for (i, (decision, doc)) in run.obligations.decisions.iter().zip(decisions).enumerate() {
            let mut index = vec![format!("id:{}", decision.request.id)];
            index.extend(decision.receipt.iter().map(|r| format!("receipt:{}", r.id)));
            if crate::card::card_obligation_visible("decision", &doc) {
                index.push("card".into());
            }
            rows.put_indexed(
                row(
                    TaskPart::RunObligation,
                    format!("{owner}/decision/{}", seq(i + 1)),
                    false,
                    decision.receipt.is_none(),
                    doc,
                ),
                index,
            );
        }
    }
    split_inbox(rows, &format!("run:{owner}"), doc)?;
    if let Some(Value::Object(host)) = doc.get_mut("agent_host") {
        let typed = run.agent_host.as_deref();
        for kind in AGENT_KINDS {
            for (i, record) in take_array(host, kind).into_iter().enumerate() {
                let (open, index) = agent_flags(typed, kind, i);
                rows.put_indexed(
                    row(
                        TaskPart::AgentRecord,
                        format!("{owner}/{kind}/{}", seq(i + 1)),
                        false,
                        open,
                        record,
                    ),
                    index,
                );
            }
        }
    }
    Ok(())
}

fn agent_flags(
    host: Option<&crate::agent::AgentRunState>,
    kind: &str,
    i: usize,
) -> (bool, Vec<String>) {
    let Some(host) = host else {
        return (false, Vec::new());
    };
    match kind {
        "receipts" => (
            false,
            host.receipts
                .get(i)
                .map(|r| vec![format!("request:{}", r.request_id)])
                .unwrap_or_default(),
        ),
        "questions" => host.questions.get(i).map_or((false, Vec::new()), |q| {
            (q.state.pending(), vec![format!("key:{}", q.key)])
        }),
        "wakes" => host.wakes.get(i).map_or((false, Vec::new()), |w| {
            (
                matches!(
                    w.state,
                    crate::agent::WakeState::Armed | crate::agent::WakeState::PendingStart
                ),
                vec![format!("request:{}", w.request_id)],
            )
        }),
        _ => (false, Vec::new()),
    }
}

/// Moves an inbox's applied effects and scoped cursors out of a container document.
pub(crate) fn split_inbox(
    rows: &mut Rows,
    owner: &str,
    doc: &mut Map<String, Value>,
) -> Result<()> {
    let Some(Value::Object(inbox)) = doc.get_mut("inbox") else {
        return Ok(());
    };
    for effect in take_array(inbox, "effects_applied") {
        let n = effect
            .as_u64()
            .context("an applied inbox effect is not a sequence")?;
        rows.put(row(
            TaskPart::InboxEffect,
            format!("{owner}/{n:020}"),
            false,
            false,
            effect,
        ));
    }
    for (i, cursor) in take_array(inbox, "observe_cursors").into_iter().enumerate() {
        let token = cursor_token(
            cursor
                .get("adapter")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            cursor.get("binding").unwrap_or(&Value::Null),
        )?;
        rows.put_indexed(
            row(
                TaskPart::ObserveCursor,
                format!("{owner}/{}", seq(i + 1)),
                false,
                false,
                cursor,
            ),
            vec![token],
        );
    }
    Ok(())
}

/// The index token of a scoped observe cursor.
pub fn cursor_token(adapter: &str, binding: &Value) -> Result<String> {
    let digest = Sha256::digest(serde_json::to_string(binding)?.as_bytes());
    Ok(format!("cursor:{adapter}:{:x}", digest))
}

fn split_recoveries(rows: &mut Rows, recoveries: &[RecoveryRecord]) -> Result<()> {
    for (n, recovery) in recoveries.iter().enumerate() {
        let key = seq(n + 1);
        let mut doc = object(serde_json::to_value(recovery)?, "recovery")?;
        for (list, part) in [("outcomes", "outcome"), ("links", "link")] {
            for (i, item) in take_array(&mut doc, list).into_iter().enumerate() {
                rows.put(row(
                    TaskPart::RecoveryOutcome,
                    format!("{key}/{part}/{}", seq(i + 1)),
                    false,
                    false,
                    item,
                ));
            }
        }
        rows.put_indexed(
            row(TaskPart::Recovery, key, false, false, Value::Object(doc)),
            vec![format!("id:{}", recovery.id)],
        );
    }
    Ok(())
}
