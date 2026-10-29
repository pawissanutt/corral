//! Background job ownership (T-565): monitor resolution shared by the `job` and `job_monitor`
//! ops, and the reaper sync runs. A job outlives its generation only until corral notices: a
//! fenced or superseded generation, or an ended run, has its verified process group killed.
//! A pid is verified by its recorded kernel start time, so a reused pid is never signalled.
use anyhow::{Context, Result};
use corral_core::agent::{
    self, ContainerRef, JOB_DETAIL_MAX_BYTES, JobRecord, JobState, MonitorOn, bounded_text,
};
use corral_core::{
    ReceiptOutcome, ResumeDelivery, ResumeReceipt, Run, Task, WaitObservation, WaitState,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// Settles the pending run wait of one monitor; false when it already left `pending`.
pub(crate) fn resolve(
    run: &mut Run,
    wait: &str,
    outcome: ReceiptOutcome,
    detail: &str,
    now: Timestamp,
) -> bool {
    let Some(record) = run
        .obligations
        .waits
        .iter_mut()
        .find(|w| w.request.id == wait)
    else {
        return false;
    };
    if record.observation.state != WaitState::Pending || record.receipt.is_some() {
        return false;
    }
    let state = match outcome {
        ReceiptOutcome::Ready | ReceiptOutcome::Answered => WaitState::Ready,
        ReceiptOutcome::Failed => WaitState::Failed,
        ReceiptOutcome::Cancelled => WaitState::Cancelled,
        ReceiptOutcome::Lost => WaitState::Lost,
    };
    record.observation = WaitObservation {
        at: now,
        state,
        detail: bounded_text(detail, JOB_DETAIL_MAX_BYTES),
    };
    record.receipt = Some(ResumeReceipt {
        version: 1,
        id: format!("{wait}-job"),
        run: run.id.clone(),
        outcome,
        delivery: ResumeDelivery::Acknowledged,
        observed_at: now,
        answer: None,
        data: json!({"via": "job_monitor"}),
    });
    true
}

/// Settles every pending monitor of a job that ended; returns how many it settled.
pub(crate) fn resolve_ended(
    run: &mut Run,
    job: &JobRecord,
    exit: ReceiptOutcome,
    now: Timestamp,
) -> usize {
    let end = job.detail.clone().unwrap_or_else(|| ended_detail(job));
    let mut settled = 0;
    for monitor in &job.monitors {
        let (outcome, detail) = match monitor.on {
            MonitorOn::Exit => (exit, format!("job {} {end}", job.id)),
            MonitorOn::Line => (
                if exit == ReceiptOutcome::Ready {
                    ReceiptOutcome::Failed
                } else {
                    exit
                },
                format!("job {} {end} before a line matched", job.id),
            ),
        };
        settled += usize::from(resolve(run, &monitor.wait, outcome, &detail, now));
    }
    settled
}

pub(crate) fn ended_detail(job: &JobRecord) -> String {
    match (job.exit_code, job.signal.as_deref()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("was terminated by {signal}"),
        (None, None) => "ended".into(),
    }
}

/// A live job its host no longer owns: its run ended, or its generation was fenced or replaced.
fn orphaned(task: &Task, container: &ContainerRef, job: &JobRecord) -> bool {
    let Some(state) = agent::container_state(task, container) else {
        return false;
    };
    job.state.live()
        && (agent::container_closure(task, container).is_some()
            || job.generation != state.binding.generation
            || state
                .generation(job.generation)
                .is_none_or(|g| g.fenced.is_some()))
}

fn reap_one(job: &JobRecord) -> Result<(JobState, String)> {
    let (Some(pid), Some(start)) = (job.pid, job.start_time.as_deref()) else {
        return Ok((
            JobState::Lost,
            "was prepared without a recorded process identity; nothing was killed".into(),
        ));
    };
    match super::effects::process_start(pid)? {
        None => Ok((
            JobState::Lost,
            format!("process {pid} had already exited; nothing was killed"),
        )),
        Some(now) if now != start => Ok((
            JobState::Lost,
            format!("pid {pid} now belongs to another process; it was not killed"),
        )),
        Some(_) => {
            kill_group(pid)?;
            Ok((
                JobState::Reaped,
                format!("was reaped: corral killed its process group {pid}"),
            ))
        }
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) -> Result<()> {
    use rustix::process::{Pid, Signal, getpgid, kill_process, kill_process_group};
    let leader = Pid::from_raw(i32::try_from(pid)?).context("invalid job pid")?;
    let result = match getpgid(Some(leader)) {
        Ok(group) if group == leader => kill_process_group(leader, Signal::KILL),
        Ok(_) => kill_process(leader, Signal::KILL),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("killing job process group {pid}")),
    }
}

#[cfg(not(unix))]
fn kill_group(pid: u32) -> Result<()> {
    anyhow::bail!("reaping job process {pid} is supported on Unix only")
}

/// Reaps the orphaned live jobs of every worker container of `task_id`; returns how many ended.
pub fn reap(ctx: &Ctx, task_id: &str, now: Timestamp) -> Result<usize> {
    let task = ctx.store.get(task_id)?;
    let mut reaped = Vec::new();
    for run in &task.runs {
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let Some(state) = agent::container_state(&task, &container) else {
            continue;
        };
        for job in state.jobs.iter().filter(|j| orphaned(&task, &container, j)) {
            let (state, detail) = reap_one(job)?;
            reaped.push((run.id.clone(), job.id.clone(), job.state, state, detail));
        }
    }
    if reaped.is_empty() {
        return Ok(0);
    }
    let mut ended = 0;
    ctx.store.try_update(task_id, |task| {
        let mut changed = false;
        for (run_id, job_id, before, after, detail) in &reaped {
            let container = ContainerRef::Worker {
                task: task.id.clone(),
                run: run_id.clone(),
            };
            let Some(state) = agent::container_state_mut(task, &container) else {
                continue;
            };
            let Some(job) = state
                .jobs
                .iter_mut()
                .find(|j| &j.id == job_id && j.state == *before)
            else {
                continue;
            };
            job.state = *after;
            job.ended_at = Some(now);
            job.detail = Some(detail.clone());
            let job = job.clone();
            if let Some(run) = task.runs.iter_mut().find(|r| &r.id == run_id) {
                resolve_ended(run, &job, ReceiptOutcome::Lost, now);
            }
            ended += 1;
            changed = true;
        }
        Ok(changed)
    })?;
    for (run, job, _, state, detail) in &reaped {
        ctx.store.log_event(
            Some(task_id),
            "agent_job_reaped",
            json!({"run": run, "job": job, "state": state, "detail": detail}),
        )?;
    }
    Ok(ended)
}

pub(crate) fn post_wakes(ctx: &Ctx, task_id: &str) -> Result<usize> {
    let task = ctx.store.get(task_id)?;
    super::wake::post_due(ctx, &super::wake::due_wait_wakes(&task))
}
