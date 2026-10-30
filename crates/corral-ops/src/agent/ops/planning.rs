//! Handlers of the native planning roles (T-460 plan-roles): plan_write, plan_submit and
//! plan_verdict. The launcher bound the planning container; the model never names a path, a
//! plan or a review attempt.
//!
//! plan_write is single-phase: it writes bounded content to the container's launcher-selected
//! file (a planner's `plan.md`, a plan reviewer's notes report) and commits its receipt in the
//! same task transaction. plan_submit and plan_verdict reuse the existing plan services, which
//! commit in their own transaction and then close the planning pane, possibly ending this very
//! process. Each therefore reserves its receipt first (naming this executor) and settles it
//! afterwards; a reservation whose executor is gone is settled from the plan record itself.
//! Each service's final transaction rechecks the request binding. The verdict always carries
//! the bound review attempt, so it can never be an owner verdict, and it writes no file: the
//! notes report changes only through plan_write.
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::{
    self, AGENT_RECORD_VERSION, AgentRunState, ContainerRef, Executor, RECEIPTS_MAX,
    RESULT_MAX_BYTES, ReceiptEffect, ReceiptState, RequestOrigin, RequestReceipt, paths,
};
use corral_core::{Task, Verdict};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::admission::{self, Admission, Mode};
use super::super::call::{AgentOp, CallResponse, RejectCode, Rejection};
use super::super::effects;
use super::super::envelope::Envelope;
use super::super::receipts::{self, Commit};
use crate::Ctx;
use crate::plan_review::PlanVerdict;

/// The T-460 member that lands these handlers.
pub const OWNER: &str = "plan-roles";

/// The largest plan or notes file a planning container may write.
pub const OUTPUT_MAX_BYTES: u64 = 1024 * 1024;

/// A refusal reason kept on a settled receipt.
const REASON_MAX_BYTES: usize = 1024;

/// Whether this module handles `op` yet; hello advertises only landed ops.
pub(crate) fn lands(op: AgentOp) -> bool {
    matches!(
        op,
        AgentOp::PlanWrite | AgentOp::PlanSubmit | AgentOp::PlanVerdict
    )
}

/// corral_plan_write: `content` replaces the file from byte `offset` on, so a retry of the same
/// request writes the same bytes again. Offset 0 starts the file over.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WritePayload {
    content: String,
    #[serde(default)]
    offset: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitPayload {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerdictPayload {
    verdict: Verdict,
    /// Replaces the notes report; absent means the report already written is the notes.
    #[serde(default)]
    notes: Option<String>,
}

fn bad(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn unbound() -> anyhow::Error {
    Rejection::new(
        RejectCode::ContainerNotFound,
        "the planning binding disappeared",
    )
    .into()
}

fn bounded(mut text: String) -> String {
    if text.len() > REASON_MAX_BYTES {
        let mut end = REASON_MAX_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// The launcher-selected file a planning container writes: the planner run's `plan.md` (the
/// path its prompt names) or the plan review's notes report (the path the OmO review prompt
/// names). Never a path the model chose.
pub(crate) fn output_path(ctx: &Ctx, task: &Task, container: &ContainerRef) -> Result<PathBuf> {
    match container {
        ContainerRef::Planner { run, .. } => task
            .plan_runs
            .iter()
            .find(|r| r.id == *run)
            .map(|r| r.output_dir.join("plan.md"))
            .ok_or_else(|| bad(format!("planner run {run} has no output directory"))),
        ContainerRef::PlanReviewer { task, plan, review } => Ok(ctx
            .store
            .state_dir()
            .join("reviews")
            .join(format!("{task}-{plan}-{review}-plan-notes.md"))),
        ContainerRef::Worker { .. }
        | ContainerRef::Reviewer { .. }
        | ContainerRef::Pm
        | ContainerRef::Assistant { .. } => {
            Err(bad("only a planning container writes plan output"))
        }
    }
}

/// The current bytes of an output file, bounded; a missing file is empty.
fn read_output(path: &Path) -> Result<Vec<u8>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > OUTPUT_MAX_BYTES {
        return Err(bad(format!(
            "{} is not a regular file of at most {OUTPUT_MAX_BYTES} bytes",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(OUTPUT_MAX_BYTES)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(bytes)
}

/// Replaces `path` from `offset` on with `content`, atomically; returns the whole new file.
fn write_at(path: &Path, offset: u64, content: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = read_output(path)?;
    let len = bytes.len() as u64;
    if offset > len {
        return Err(bad(format!(
            "offset {offset} is past the end of the {len}-byte file; continue at offset {len}"
        )));
    }
    let total = offset + content.len() as u64;
    if total > OUTPUT_MAX_BYTES {
        return Err(bad(format!(
            "the file would be {total} bytes, more than {OUTPUT_MAX_BYTES}"
        )));
    }
    bytes.truncate(offset as usize);
    bytes.extend_from_slice(content);
    let dir = path.parent().context("an output file has a directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut staged = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("staging {}", path.display()))?;
    std::io::Write::write_all(&mut staged, &bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    staged
        .persist(path)
        .with_context(|| format!("publishing {}", path.display()))?;
    Ok(bytes)
}

fn write(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let input: WritePayload = super::payload(envelope, "content, offset?")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    // The write happens after the commit's own liveness recheck, under the task's lock.
    receipts::commit_with_task(ctx, &admitted, |task| {
        let path = output_path(ctx, task, &container)?;
        let bytes = write_at(&path, input.offset, input.content.as_bytes())?;
        Ok(Commit {
            result: json!({
                "path": path, "offset": input.offset, "bytes": bytes.len(),
                "sha256": paths::sha256_hex(&bytes),
            }),
            effect: Some(ReceiptEffect {
                kind: "plan_output".into(),
                reference: path.display().to_string(),
            }),
            post_pending: Vec::new(),
        })
    })
}

enum Reserved {
    New(RequestReceipt),
    Stored(RequestReceipt),
}

/// Commits a reservation naming this executor before the plan service runs; an identical
/// stored request answers instead. Liveness, fence and saved pane are rechecked under the lock.
fn reserve(
    ctx: &Ctx,
    origin: &RequestOrigin,
    effect: ReceiptEffect,
    detail: Value,
) -> Result<Reserved> {
    let container = &origin.binding.container;
    let task_id = container
        .task()
        .context("a planning container names its task")?;
    let me = effects::executor()?;
    let mut out = None;
    ctx.store.try_update(task_id, |task| {
        let state = agent::container_state(task, container).ok_or_else(unbound)?;
        if let Some(stored) = state.receipt(&origin.request_id) {
            admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
            out = Some(Reserved::Stored(stored.clone()));
            return Ok(false);
        }
        admission::recheck_task(task, &origin.binding)?;
        let state = agent::container_state_mut(task, container).ok_or_else(unbound)?;
        if state.receipts.len() >= RECEIPTS_MAX {
            return Err(Rejection::new(
                RejectCode::ReceiptLimit,
                format!("the container already holds {RECEIPTS_MAX} receipts"),
            )
            .into());
        }
        let mut result = detail;
        result["outcome"] = json!("reserved");
        let bytes = serde_json::to_vec(&result)?.len();
        anyhow::ensure!(bytes <= RESULT_MAX_BYTES, "reservation of {bytes} bytes");
        let receipt = RequestReceipt {
            version: AGENT_RECORD_VERSION,
            request_id: origin.request_id.clone(),
            op: origin.op.clone(),
            payload_sha256: origin.payload_sha256.clone(),
            identity: origin.identity.clone(),
            state: ReceiptState::Reserved,
            result,
            executor: Some(me),
            effect: Some(effect),
            evidence: Vec::new(),
            post_pending: Vec::new(),
            at: Timestamp::now(),
            committed_at: None,
        };
        state.receipts.push(receipt.clone());
        out = Some(Reserved::New(receipt));
        Ok(true)
    })?;
    out.context("the reservation produced no receipt")
}

/// Settles a reservation still held by `seen`; anything else answers the stored receipt.
fn settle(
    ctx: &Ctx,
    container: &ContainerRef,
    request: &str,
    seen: Option<&Executor>,
    (state, result): (ReceiptState, Value),
) -> Result<RequestReceipt> {
    let task_id = container
        .task()
        .context("a planning container names its task")?;
    let mut out = None;
    ctx.store.try_update(task_id, |task| {
        let stored = agent::container_state_mut(task, container)
            .and_then(|s| s.receipts.iter_mut().find(|r| r.request_id == request))
            .context("the planning reservation disappeared")?;
        let changed = stored.state == ReceiptState::Reserved && stored.executor.as_ref() == seen;
        if changed {
            let now = Timestamp::now();
            stored.state = state;
            stored.result = result;
            stored.executor = None;
            stored.committed_at = (state == ReceiptState::Committed).then_some(now);
        }
        out = Some(stored.clone());
        Ok(changed)
    })?;
    out.context("the settlement produced no receipt")
}

/// What a submission left in the plan record: a plan submitted after the reservation by this
/// run. The record commits atomically, so no plan means nothing was submitted.
fn submit_outcome(
    task: &Task,
    run: &str,
    reserved: &Value,
    error: Option<String>,
) -> (ReceiptState, Value) {
    let before = reserved["plans_before"].as_u64().unwrap_or(0) as usize;
    match task
        .plans
        .iter()
        .skip(before)
        .find(|p| p.run.as_deref() == Some(run))
    {
        Some(plan) => (
            ReceiptState::Committed,
            json!({"outcome": "submitted", "plan": plan.id, "sha256": plan.artifact.sha256}),
        ),
        None => (
            ReceiptState::Abandoned,
            json!({"outcome": "refused", "reason": error.unwrap_or_else(|| "not submitted".into())}),
        ),
    }
}

/// What a verdict left in the plan record: this attempt's verdict with these notes (the
/// service may append to them, e.g. an escalation). Another closure is not this request's.
fn verdict_outcome(
    task: &Task,
    (plan, review): (&str, &str),
    reserved: &Value,
    error: Option<String>,
) -> (ReceiptState, Value) {
    let recorded = task
        .plans
        .iter()
        .find(|p| p.id == plan)
        .and_then(|p| p.reviews.iter().find(|r| r.id == review))
        .and_then(|r| Some((r.verdict?, r.notes.as_deref().unwrap_or_default())));
    let len = reserved["notes_bytes"].as_u64().unwrap_or(u64::MAX) as usize;
    let ours = |stored: &str| {
        stored
            .as_bytes()
            .get(..len)
            .map(paths::sha256_hex)
            .as_deref()
            == reserved["notes_sha256"].as_str()
    };
    match recorded {
        Some((verdict, stored)) if ours(stored) => (
            ReceiptState::Committed,
            json!({"outcome": "recorded", "plan": plan, "review": review, "verdict": verdict}),
        ),
        _ => (
            ReceiptState::Abandoned,
            json!({"outcome": "refused", "reason": error.unwrap_or_else(|| "not recorded".into())}),
        ),
    }
}

/// Settles one reservation whose executor is verified gone, from the plan record. Nothing
/// re-executes; a live executor's reservation is left as it is.
fn reconcile_one(
    ctx: &Ctx,
    container: &ContainerRef,
    receipt: RequestReceipt,
) -> Result<RequestReceipt> {
    let Some(executor) = receipt.executor.clone() else {
        return Ok(receipt);
    };
    if receipt.state != ReceiptState::Reserved || !effects::executor_gone(&executor)? {
        return Ok(receipt);
    }
    let task = ctx.store.get(
        container
            .task()
            .context("planning containers name a task")?,
    )?;
    let outcome = match container {
        ContainerRef::Planner { run, .. } if receipt.op == "plan_submit" => {
            submit_outcome(&task, run, &receipt.result, None)
        }
        ContainerRef::PlanReviewer { plan, review, .. } if receipt.op == "plan_verdict" => {
            verdict_outcome(&task, (plan, review), &receipt.result, None)
        }
        _ => return Ok(receipt),
    };
    settle(
        ctx,
        container,
        &receipt.request_id,
        Some(&executor),
        outcome,
    )
}

/// Every planning container of the task that ever bound a Pi-native host, with its state.
fn hosts(task: &Task) -> Vec<(ContainerRef, &AgentRunState)> {
    let planners = task.plan_runs.iter().filter_map(|run| {
        let container = ContainerRef::Planner {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        Some((container, run.agent_host.as_deref()?))
    });
    let reviews = task.plans.iter().flat_map(|plan| {
        plan.reviews.iter().filter_map(move |review| {
            let container = ContainerRef::PlanReviewer {
                task: task.id.clone(),
                plan: plan.id.clone(),
                review: review.id.clone(),
            };
            Some((container, review.agent_host.as_deref()?))
        })
    });
    planners.chain(reviews).collect()
}

/// The planning containers the question sweeps visit, current or historical.
pub(crate) fn containers(task: &Task) -> Vec<ContainerRef> {
    hosts(task)
        .into_iter()
        .map(|(container, _)| container)
        .collect()
}

/// Whether normal sync must visit the task for its planning hosts: one is open, or a closed one
/// still holds an unsettled receipt or a recorded-pending question. Settled ones never do.
pub(crate) fn wants_sync(task: &Task) -> bool {
    hosts(task).iter().any(|(container, state)| {
        agent::container_closure(task, container).is_none()
            || state.questions.iter().any(|q| q.recorded_pending())
            || state
                .receipts
                .iter()
                .any(|r| r.state == ReceiptState::Reserved)
    })
}

/// Sync and recovery: settles every planning reservation of the task whose executor is gone.
pub(crate) fn reconcile(ctx: &Ctx, task_id: &str) -> Result<()> {
    let task = ctx.store.get(task_id)?;
    for (container, state) in hosts(&task) {
        let reserved = state
            .receipts
            .iter()
            .filter(|r| r.state == ReceiptState::Reserved && r.executor.is_some());
        for receipt in reserved.cloned().collect::<Vec<_>>() {
            reconcile_one(ctx, &container, receipt)?;
        }
    }
    Ok(())
}

/// An identical request answers with its receipt, settled first if its executor is gone.
fn stored(ctx: &Ctx, container: &ContainerRef, receipt: RequestReceipt) -> Result<CallResponse> {
    let receipt = reconcile_one(ctx, container, receipt)?;
    receipts::respond(ctx, container, receipt, true)
}

fn submit(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let _: SubmitPayload = super::payload(envelope, "no fields")?;
    let container = admission::container_of(&envelope.identity);
    let ContainerRef::Planner { task: task_id, run } = &container else {
        return Err(bad("only a planner submits a plan"));
    };
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(receipt) => return stored(ctx, &container, *receipt),
        Admission::New(admitted) => admitted,
    };
    let task = ctx.store.get(task_id)?;
    let path = output_path(ctx, &task, &container)?;
    if read_output(&path)?.is_empty() {
        return Err(bad(format!(
            "{} is empty; write the plan with corral_plan_write before submitting",
            path.display()
        )));
    }
    let effect = ReceiptEffect {
        kind: "plan_submission".into(),
        reference: run.clone(),
    };
    let detail = json!({"plans_before": task.plans.len(), "path": path});
    let reserved = match reserve(ctx, &admitted.origin, effect, detail)? {
        Reserved::Stored(receipt) => return stored(ctx, &container, receipt),
        Reserved::New(receipt) => receipt,
    };
    effects::failpoint("plan_reserved_before_effect");
    // The service's own final transaction rechecks this binding (fence, liveness and that D<n>
    // is still the accepting run), so a fenced or replaced planner never submits.
    let binding = &admitted.origin.binding;
    let bound = |task: &Task| admission::recheck_task(task, binding);
    let error = crate::plan::plan_submit_checked(ctx, task_id, &path, &bound).err();
    let error = error.map(|e| bounded(format!("{e:#}")));
    let outcome = submit_outcome(&ctx.store.get(task_id)?, run, &reserved.result, error);
    let receipt = settle(
        ctx,
        &container,
        &reserved.request_id,
        reserved.executor.as_ref(),
        outcome,
    )?;
    receipts::respond(ctx, &container, receipt, false)
}

fn verdict(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let input: VerdictPayload =
        super::payload(envelope, "verdict (approve|changes|escalate), notes?")?;
    let container = admission::container_of(&envelope.identity);
    let ContainerRef::PlanReviewer {
        task: task_id,
        plan,
        review,
    } = &container
    else {
        return Err(bad("only a plan reviewer records a plan verdict"));
    };
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(receipt) => return stored(ctx, &container, *receipt),
        Admission::New(admitted) => admitted,
    };
    // Inline notes are recorded on the review only; the notes report changes only through the
    // receipted plan_write, so this operation has no effect before its reservation.
    let path = output_path(ctx, &ctx.store.get(task_id)?, &container)?;
    let notes = match input.notes {
        Some(notes) => notes,
        None => String::from_utf8(read_output(&path)?)
            .map_err(|_| bad(format!("{} is not UTF-8", path.display())))?,
    };
    if notes.trim().is_empty() {
        return Err(bad(
            "a plan verdict needs notes: pass notes or write them with corral_plan_write",
        ));
    }
    let effect = ReceiptEffect {
        kind: "plan_verdict".into(),
        reference: format!("{plan}/{review}"),
    };
    let detail = json!({
        "plan": plan, "review": review, "notes_bytes": notes.len(),
        "notes_sha256": paths::sha256_hex(notes.as_bytes()),
    });
    let reserved = match reserve(ctx, &admitted.origin, effect, detail)? {
        Reserved::Stored(receipt) => return stored(ctx, &container, receipt),
        Reserved::New(receipt) => receipt,
    };
    effects::failpoint("plan_reserved_before_effect");
    // Always the bound attempt: a launched reviewer can never record an owner verdict.
    let request = PlanVerdict {
        plan,
        attempt: Some(review),
        verdict: input.verdict,
        notes: &notes,
    };
    let binding = &admitted.origin.binding;
    let bound = |task: &Task| admission::recheck_task(task, binding);
    let error = crate::plan_review::verdict_report_checked(ctx, task_id, &request, &bound).err();
    let error = error.map(|e| bounded(format!("{e:#}")));
    let task = ctx.store.get(task_id)?;
    let outcome = verdict_outcome(&task, (plan, review), &reserved.result, error);
    let receipt = settle(
        ctx,
        &container,
        &reserved.request_id,
        reserved.executor.as_ref(),
        outcome,
    )?;
    receipts::respond(ctx, &container, receipt, false)
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    match op {
        AgentOp::PlanWrite => write(ctx, envelope),
        AgentOp::PlanSubmit => submit(ctx, envelope),
        AgentOp::PlanVerdict => verdict(ctx, envelope),
        other => Err(super::unavailable(other, OWNER)),
    }
}

#[cfg(test)]
#[path = "planning_tests.rs"]
mod tests;
