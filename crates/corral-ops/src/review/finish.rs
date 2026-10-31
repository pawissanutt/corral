//! Compatibility values for finishing a run: head fields and keyed worker-agent records only.
use anyhow::{Context, Result, ensure};
use corral_core::{RunHead, Task, TaskHead, TaskHeadMut};

pub(super) fn finish_task(head: &TaskHead) -> Result<Task> {
    let mut task = head.to_shell_task()?;
    if let Some(run) = task.current_run_mut() {
        run.agent_host = head.current_agent_state()?;
    }
    Ok(task)
}

pub(super) fn refuse_open_questions_head(head: &TaskHead, run: &str) -> Result<()> {
    let container = corral_core::agent::ContainerRef::Worker {
        task: head.id.clone(),
        run: run.into(),
    };
    if corral_core::agent::container_closure(&head.to_shell_task()?, &container).is_some() {
        return Ok(());
    }
    let keys: Vec<_> = head
        .pending_agent_questions(&container)?
        .into_iter()
        .map(|q| q.key)
        .collect();
    ensure!(
        keys.is_empty(),
        "open_questions: {} pending ({}); wait for the answers or withdraw them",
        keys.len(),
        keys.join(", ")
    );
    Ok(())
}

/// Write only the fields `done` changes. In particular, absent shell vectors never erase
/// checkpoints, obligations, inbox effects, switches, or earlier runs and review children.
pub(super) fn save_finish(head: &mut TaskHeadMut, task: &Task) -> Result<()> {
    let run = task.current_run().context("missing_run")?;
    head.set_usage(&run.id, None, run.usage.clone().unwrap_or_default())?;
    if let Some(child) = run.review_runs.last() {
        // An ended child was not recollected by supersede_review; preserve its stored entries.
        let old = head
            .head()
            .current_run
            .as_ref()
            .and_then(|r| r.current_child.as_deref());
        if old.is_some_and(|r| r.ended.is_none()) && child.ended.is_some() {
            head.set_usage(
                &run.id,
                Some(&child.id),
                child.usage.clone().unwrap_or_default(),
            )?;
        }
    }
    let old = head.head().current_run.as_ref().context("missing_run")?;
    let mut next = RunHead::of(run)?;
    next.facts = old.facts.clone();
    if let Some(child) = run.review_runs.last() {
        let mut projected = RunHead::of(child)?;
        if let Some(old) = old.current_child.as_deref() {
            projected.facts = old.facts.clone();
            // The shell omitted entries; only replace usage if the child just ended.
            if old.ended == child.ended {
                projected.usage = old.usage.clone();
            }
        }
        next.current_child = Some(Box::new(projected));
    }
    *head.current_run_mut().context("missing_run")? = next;
    if let Some(state) = run.agent_host.clone() {
        head.set_current_agent_state(state)?;
    }
    head.status = task.status;
    head.queued = task.queued;
    head.result = task.result.clone();
    Ok(())
}
