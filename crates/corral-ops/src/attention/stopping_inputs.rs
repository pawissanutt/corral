//! Active run facts needed to complete stopping evidence, without history facades.
use anyhow::{Result, bail};
use corral_core::agent::{ContainerRef, HostHealth};
use corral_core::{GateRecord, HerdrRef, Run, RunHead, Task, TaskHead, WaitRecord};
use jiff::Timestamp;

pub(super) struct RunStop {
    pub id: String,
    pub started: Timestamp,
    pub idle_since: Option<Timestamp>,
    pub herdr: Option<HerdrRef>,
    pub host_health: Option<HostHealth>,
    pub agent_state: Option<String>,
    pub blocking_question: bool,
    pub gate: Option<GateRecord>,
    pub waits: Vec<WaitRecord>,
}

pub(in crate::attention) struct Inputs {
    pub(super) runs: Vec<RunStop>,
}

macro_rules! inputs {
    ($run:expr,$blocking:expr,$waits:expr) => {{
        let run = $run;
        RunStop {
            id: run.id.clone(),
            started: run.started,
            idle_since: run.idle_since,
            herdr: run.herdr.clone(),
            host_health: run.agent_host.as_ref().map(|host| host.binding.health),
            agent_state: run.agent_state.clone(),
            blocking_question: $blocking,
            gate: run.gate.clone(),
            waits: $waits,
        }
    }};
}

fn full(run: &Run) -> RunStop {
    inputs!(
        run,
        run.agent_host.as_ref().is_some_and(|host| host
            .questions
            .iter()
            .any(|question| question.blocking && question.recorded_pending())),
        run.obligations
            .waits
            .iter()
            .filter(|wait| wait.receipt.is_none())
            .cloned()
            .collect()
    )
}

fn head(task: &TaskHead, run: &RunHead, container: &ContainerRef) -> Result<RunStop> {
    let blocking = run.agent_host.is_some()
        && task
            .pending_agent_questions(container)?
            .iter()
            .any(|question| question.blocking && question.recorded_pending());
    let obligations = match container {
        ContainerRef::Worker { run, .. } => task.unreceipted_obligations(run)?,
        ContainerRef::Reviewer { run, review, .. } => {
            task.unreceipted_review_obligations(run, review)?
        }
        ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } | ContainerRef::Pm => {
            bail!("stopping run input needs a coding container")
        }
    };
    Ok(inputs!(
        run,
        blocking,
        obligations
            .waits
            .into_iter()
            .filter(|wait| wait.receipt.is_none())
            .collect()
    ))
}

impl Inputs {
    pub fn full(task: &Task) -> Self {
        Self {
            runs: task
                .current_run()
                .into_iter()
                .flat_map(|parent| {
                    std::iter::once(parent)
                        .filter(|run| run.outcome.is_none())
                        .chain(
                            parent
                                .review_runs
                                .iter()
                                .filter(|run| run.outcome.is_none() && run.ended.is_none()),
                        )
                })
                .map(full)
                .collect(),
        }
    }

    pub fn head(task: &TaskHead) -> Result<Self> {
        let mut runs = Vec::new();
        if matches!(
            task.status,
            corral_core::Status::Idea | corral_core::Status::Done | corral_core::Status::Dropped
        ) {
            return Ok(Self { runs });
        }
        if let Some(parent) = task.current_run.as_deref() {
            if parent.outcome.is_none() {
                runs.push(head(
                    task,
                    parent,
                    &ContainerRef::Worker {
                        task: task.id.clone(),
                        run: parent.id.clone(),
                    },
                )?);
            }
            for child in task
                .open_review_children(&parent.id)?
                .into_iter()
                .filter(|run| run.outcome.is_none() && run.ended.is_none())
            {
                runs.push(head(
                    task,
                    &child,
                    &ContainerRef::Reviewer {
                        task: task.id.clone(),
                        run: parent.id.clone(),
                        review: child.id.clone(),
                    },
                )?);
            }
        }
        Ok(Self { runs })
    }
}
