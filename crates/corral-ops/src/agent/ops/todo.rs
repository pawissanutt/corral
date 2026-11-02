//! Top-level receipted todo commands over server-selected worker, PM or assistant progress.
use anyhow::{Context, Result};
use corral_core::agent::{AgentRole, ReceiptEffect, TodoAction, request};
use corral_core::{RunProgress, TaskRef};
use serde_json::json;

use super::super::admission::{self, Admission, Mode};
use super::super::call::{CallResponse, RejectCode, Rejection};
use super::super::envelope::Envelope;
use super::super::receipts::{self, Commit};
use crate::Ctx;

fn bad(error: impl std::fmt::Display) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, error.to_string()).into()
}

fn committed(progress: &RunProgress) -> Commit {
    Commit {
        result: json!({"revision": progress.revision, "progress": progress}),
        effect: Some(ReceiptEffect {
            kind: "progress".into(),
            reference: progress.revision.to_string(),
        }),
        post_pending: vec!["event:progress_updated".into()],
    }
}

fn worker_progress<T: TaskRef + ?Sized>(task: &T, action: &TodoAction) -> Result<RunProgress> {
    let run = task.current_run_head()?.context("missing_run")?;
    let spec = crate::progress_spec_revision(task)?;
    let progress = action.apply(run.progress.as_ref(), &spec).map_err(bad)?;
    for step in progress.steps() {
        let mut seen = std::collections::HashSet::new();
        for id in &step.criteria {
            if !seen.insert(id)
                || !task.completion().as_ref().is_some_and(|manifest| {
                    manifest
                        .criteria
                        .iter()
                        .any(|criterion| criterion.id == *id)
                })
            {
                return Err(bad("unknown_or_duplicate_todo_criterion"));
            }
        }
    }
    Ok(progress)
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let action: TodoAction =
        super::payload(envelope, "a todo init/append/start/done/drop/wait command")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let enabled = admitted
        .resolved
        .generation(&container, admitted.origin.binding.generation)?
        .is_some_and(|g| g.capability("todo"));
    if !enabled {
        return Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "todo is disabled for this generation",
        )
        .into());
    }
    match envelope.identity.role {
        AgentRole::Worker => receipts::commit_head(ctx, &admitted, |task| {
            let progress = worker_progress(task, &action)?;
            let commit = committed(&progress);
            let run = task.current_run_mut().context("missing_run")?;
            run.progress = Some(progress);
            run.checkpoint_required = true;
            Ok(commit)
        }),
        AgentRole::Pm | AgentRole::Assistant => receipts::commit(ctx, &admitted, |state, _| {
            // The ledger is the container's own: the PM's project or the assistant session.
            let spec = match container.assistant() {
                Some(session) => format!("assistant:{session}"),
                None => format!("pm:{}", state.binding.project),
            };
            let mut requests = state.todo_requests.clone().unwrap_or_default();
            let progress = requests
                .apply(&action, state.todo.as_ref(), &spec, jiff::Timestamp::now())
                .map_err(bad)?;
            if progress.steps().any(|step| !step.criteria.is_empty()) {
                return Err(bad(
                    "a PM or assistant todo cannot link task acceptance criteria",
                ));
            }
            let mut commit = committed(&progress);
            commit.result["plan"] = json!(progress.plan);
            commit.result["disposition"] = json!(request::disposition(Some(&progress)));
            commit.result["closed_plans"] = json!(requests.history.len());
            state.todo = Some(progress);
            state.todo_requests = Some(requests);
            Ok(commit)
        }),
        AgentRole::Reviewer | AgentRole::Planner | AgentRole::PlanReviewer => Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "todo belongs to worker, PM and assistant",
        )
        .into()),
    }
}
