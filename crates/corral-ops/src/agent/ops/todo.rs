//! Top-level receipted todo commands over server-selected worker/PM progress.
use anyhow::{Context, Result};
use corral_core::agent::{AgentRole, ReceiptEffect, TodoAction};
use corral_core::{RunProgress, Task};
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

fn worker_progress(task: &Task, action: &TodoAction) -> Result<RunProgress> {
    let run = task.current_run().context("missing_run")?;
    let spec = crate::progress_spec_revision(task)?;
    let progress = action.apply(run.progress.as_ref(), &spec).map_err(bad)?;
    for step in progress.steps() {
        let mut seen = std::collections::HashSet::new();
        for id in &step.criteria {
            if !seen.insert(id)
                || !task.completion.as_ref().is_some_and(|manifest| {
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
        super::payload(envelope, "a todo init/append/start/done/drop command")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let enabled = admitted
        .resolved
        .state(&container)
        .and_then(|state| state.current_generation())
        .is_some_and(|g| g.capability("todo"));
    if !enabled {
        return Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "todo is disabled for this generation",
        )
        .into());
    }
    match envelope.identity.role {
        AgentRole::Worker => receipts::commit_with_task(ctx, &admitted, |task| {
            let progress = worker_progress(task, &action)?;
            let commit = committed(&progress);
            let run = task.current_run_mut().context("missing_run")?;
            run.progress = Some(progress);
            run.checkpoint_required = true;
            Ok(commit)
        }),
        AgentRole::Pm => receipts::commit(ctx, &admitted, |state, _| {
            let spec = format!("pm:{}", state.binding.project);
            let progress = action.apply(state.todo.as_ref(), &spec).map_err(bad)?;
            if progress.steps().any(|step| !step.criteria.is_empty()) {
                return Err(bad("PM todo cannot link task acceptance criteria"));
            }
            let commit = committed(&progress);
            state.todo = Some(progress);
            Ok(commit)
        }),
        AgentRole::Reviewer
        | AgentRole::Planner
        | AgentRole::PlanReviewer
        | AgentRole::Assistant => {
            Err(Rejection::new(RejectCode::OpNotAllowed, "todo belongs to worker and PM").into())
        }
    }
}
