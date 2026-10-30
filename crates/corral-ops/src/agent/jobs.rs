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

/// A job its host no longer owns: its run ended, or its generation was fenced or replaced.
fn orphaned(task: &Task, container: &ContainerRef, job: &JobRecord) -> bool {
    let Some(state) = agent::container_state(task, container) else {
        return false;
    };
    job.needs_reap()
        && (agent::container_closure(task, container).is_some()
            || job.generation != state.binding.generation
            || state
                .generation(job.generation)
                .is_none_or(|g| g.fenced.is_some()))
}

/// The job's process group as seen now. Ownership is anchored on the recorded leader: its pid
/// must still exist (running or an unreaped zombie) with the recorded start time, which also
/// keeps the group ID from being reused. A group whose leader is gone cannot be told apart from
/// a later group that reused the ID, so it is never claimed, whatever its members.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Empty,
    Owned(usize),
    Reused,
    Unanchored(usize),
}

#[cfg(target_os = "linux")]
fn stat_fields(pid: u32) -> Option<(String, u32, String)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((
        (*fields.first()?).to_owned(),
        fields.get(2)?.parse().ok()?,
        (*fields.get(19)?).to_owned(),
    ))
}

#[cfg(target_os = "linux")]
pub(crate) fn owned_group(pgid: u32, start: &str) -> Result<Group> {
    let leader = stat_fields(pgid);
    if let Some((_, group, started)) = &leader
        && (started != start || *group != pgid)
    {
        return Ok(Group::Reused);
    }
    let mut members = 0;
    for entry in std::fs::read_dir("/proc").context("listing /proc")? {
        let Some(pid) = entry
            .ok()
            .and_then(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
        else {
            continue;
        };
        // A process that exits while it is read is simply not a member any more.
        if let Some((state, group, _)) = stat_fields(pid)
            && group == pgid
            && state != "Z"
            && state != "X"
        {
            members += 1;
        }
    }
    Ok(match (leader, members) {
        (_, 0) => Group::Empty,
        (Some(_), n) => Group::Owned(n),
        (None, n) => Group::Unanchored(n),
    })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn owned_group(pgid: u32, start: &str) -> Result<Group> {
    Ok(match super::effects::process_start(pgid)? {
        None => Group::Empty,
        Some(now) if now == start => Group::Owned(1),
        Some(_) => Group::Reused,
    })
}

/// What reaping one job did: its new state, the observable reason and whether the group is now
/// verified empty. Only a group anchored on its recorded leader is ever signalled.
fn reap_one(job: &JobRecord) -> Result<(JobState, String, bool)> {
    let ended = if job.state == JobState::Exited {
        JobState::Exited
    } else {
        JobState::Lost
    };
    let (Some(pid), Some(start)) = (job.pid, job.start_time.as_deref()) else {
        return Ok((
            JobState::Lost,
            "was prepared without a recorded process identity; its gated launcher never ran the \
             command and exits once its host is gone"
                .into(),
            true,
        ));
    };
    Ok(match owned_group(pid, start)? {
        Group::Empty => (
            ended,
            format!("process group {pid} had already ended; nothing was killed"),
            true,
        ),
        Group::Reused => (
            ended,
            format!("pid {pid} now belongs to another process; it was not signalled"),
            false,
        ),
        Group::Unanchored(members) => (
            ended,
            format!(
                "process group {pid} lost its leader; its {members} live members cannot be \
                 verified as this job's and were not signalled"
            ),
            false,
        ),
        Group::Owned(members) => {
            kill_group(pid)?;
            let state = if job.state == JobState::Exited {
                JobState::Exited
            } else {
                JobState::Reaped
            };
            (
                state,
                format!("was reaped: corral killed its process group {pid} ({members} live)"),
                true,
            )
        }
    })
}

#[cfg(unix)]
fn kill_group(pid: u32) -> Result<()> {
    use rustix::process::{Pid, Signal, kill_process_group};
    let group = Pid::from_raw(i32::try_from(pid)?).context("invalid job pid")?;
    match kill_process_group(group, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("killing job process group {pid}")),
    }
}

#[cfg(not(unix))]
fn kill_group(pid: u32) -> Result<()> {
    anyhow::bail!("reaping job process {pid} is supported on Unix only")
}

/// Reaps the orphaned jobs of every worker container of `task_id`; returns how many it settled.
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
            let (state, detail, clear) = reap_one(job)?;
            reaped.push((
                run.id.clone(),
                job.id.clone(),
                job.state,
                state,
                detail,
                clear,
            ));
        }
    }
    if reaped.is_empty() {
        return Ok(0);
    }
    let mut ended = 0;
    ctx.store.try_update(task_id, |task| {
        let mut changed = false;
        for (run_id, job_id, before, after, detail, clear) in &reaped {
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
                .find(|j| &j.id == job_id && j.state == *before && j.needs_reap())
            else {
                continue;
            };
            job.state = *after;
            job.group_clear = *clear;
            job.swept = true;
            job.detail = Some(detail.clone());
            if before.live() {
                job.ended_at = Some(now);
            }
            let job = job.clone();
            if before.live()
                && let Some(run) = task.runs.iter_mut().find(|r| &r.id == run_id)
            {
                resolve_ended(run, &job, ReceiptOutcome::Lost, now);
            }
            ended += 1;
            changed = true;
        }
        Ok(changed)
    })?;
    for (run, job, _, state, detail, _) in &reaped {
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
