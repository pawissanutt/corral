//! Worker background jobs (T-565). `job` is host-internal: the host records a job before it
//! spawns it, its process identity once spawned, and the lines and exit it observes.
//! `job_monitor` is the model's `corral_job_monitor`: a deadline-bound run wait, its record on
//! the job and its wake subscription commit together, so exactly one wake answers it.
use anyhow::{Context, Result};
use corral_core::agent::{
    AgentRunState, ContainerRef, JOB_COMMAND_MAX_BYTES, JOB_LINE_MAX_BYTES, JOB_MONITORS_MAX,
    JOB_PATTERN_MAX_BYTES, JobMonitor, JobRecord, JobState, MonitorOn, ReceiptEffect, bounded_text,
    evict_jobs,
};
use corral_core::{
    ObligationSource, ReceiptOutcome, Run, Task, WaitHandle, WaitObservation, WaitRecord,
    WaitRequest, WaitState,
};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::admission::{self, Admission, Mode};
use super::super::call::{AgentOp, CallResponse, RejectCode, Rejection};
use super::super::envelope::Envelope;
use super::super::jobs::{ended_detail, post_wakes, resolve, resolve_ended};
use super::super::receipts::{self, Commit};
use crate::Ctx;

const FEATURE: &str = "background_shell";

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum JobAction {
    Prepare {
        job: String,
        command: String,
    },
    Started {
        job: String,
        pid: u32,
        start_time: String,
    },
    Exited {
        job: String,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        signal: Option<String>,
        /// The host verified the whole process group gone after the leader's exit.
        #[serde(default)]
        group_clear: bool,
    },
    Line {
        job: String,
        wait: String,
        line: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorPayload {
    wait: String,
    job: String,
    on: MonitorOn,
    #[serde(default)]
    pattern: Option<String>,
    deadline: Timestamp,
}

fn bad(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn word(value: &str, what: &str) -> Result<()> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    ok.then_some(())
        .ok_or_else(|| bad(format!("{what} must be 1-64 characters of [A-Za-z0-9._-]")))
}

fn job_mut<'a>(state: &'a mut AgentRunState, id: &str) -> Result<&'a mut JobRecord> {
    state
        .jobs
        .iter_mut()
        .find(|job| job.id == id)
        .ok_or_else(|| bad(format!("unknown job {id}")))
}

fn run_mut<'a>(task: &'a mut Task, run: &str) -> Result<&'a mut Run> {
    task.runs
        .iter_mut()
        .find(|r| r.id == run)
        .context("the bound run disappeared")
}

fn host_state<'a>(task: &'a mut Task, container: &ContainerRef) -> Result<&'a mut AgentRunState> {
    corral_core::agent::container_state_mut(task, container)
        .context("the agent binding disappeared")
}

fn commit(result: Value, job: &str, owed: bool) -> Commit {
    Commit {
        result,
        effect: Some(ReceiptEffect {
            kind: "job".into(),
            reference: job.to_owned(),
        }),
        post_pending: if owed {
            vec!["event:agent_job".into()]
        } else {
            Vec::new()
        },
    }
}

fn apply(
    task: &mut Task,
    container: &ContainerRef,
    run: &str,
    generation: u32,
    max_jobs: u32,
    action: JobAction,
) -> Result<(Commit, bool)> {
    let now = Timestamp::now();
    match action {
        JobAction::Prepare { job, command } => {
            word(&job, "job")?;
            if command.trim().is_empty() || command.len() > JOB_COMMAND_MAX_BYTES {
                return Err(bad(format!(
                    "a job command must be 1-{JOB_COMMAND_MAX_BYTES} bytes"
                )));
            }
            let state = host_state(task, container)?;
            if state.jobs.iter().any(|j| j.id == job) {
                return Err(bad(format!("job {job} already exists")));
            }
            let live = state.jobs.iter().filter(|j| j.state.live()).count();
            if live >= max_jobs as usize {
                return Err(bad(format!(
                    "job_limit: {live} jobs are already running (max_jobs = {max_jobs}); \
                     wait for one or kill it"
                )));
            }
            state.jobs.push(JobRecord {
                id: job.clone(),
                generation,
                command,
                state: JobState::Prepared,
                pid: None,
                start_time: None,
                prepared_at: now,
                ended_at: None,
                exit_code: None,
                signal: None,
                detail: None,
                group_clear: false,
                monitors: Vec::new(),
            });
            evict_jobs(&mut state.jobs);
            Ok((
                commit(json!({"job": job, "state": "prepared"}), &job, false),
                false,
            ))
        }
        JobAction::Started {
            job,
            pid,
            start_time,
        } => {
            let ok_start = !start_time.is_empty()
                && start_time.len() <= 32
                && start_time.bytes().all(|b| b.is_ascii_digit());
            if pid == 0 || !ok_start {
                return Err(bad("started needs a pid and its numeric kernel start time"));
            }
            let record = job_mut(host_state(task, container)?, &job)?;
            if record.state != JobState::Prepared || record.generation != generation {
                return Err(bad(format!("job {job} is not prepared by this generation")));
            }
            record.state = JobState::Running;
            record.pid = Some(pid);
            record.start_time = Some(start_time);
            let result = json!({"job": job, "state": "running", "pid": pid});
            Ok((commit(result, &job, true), false))
        }
        JobAction::Exited {
            job,
            exit_code,
            signal,
            group_clear,
        } => {
            if let Some(signal) = &signal {
                word(signal, "signal")?;
            }
            let record = job_mut(host_state(task, container)?, &job)?;
            if !record.state.live() || record.generation != generation {
                return Err(bad(format!("job {job} is not live in this generation")));
            }
            record.state = JobState::Exited;
            record.ended_at = Some(now);
            record.exit_code = exit_code;
            record.signal = signal;
            record.group_clear = group_clear;
            let ended = record.clone();
            let outcome = if ended.exit_code == Some(0) {
                ReceiptOutcome::Ready
            } else {
                ReceiptOutcome::Failed
            };
            let settled = resolve_ended(run_mut(task, run)?, &ended, outcome, now);
            let result = json!({
                "job": job, "state": "exited", "detail": ended_detail(&ended), "settled": settled,
            });
            Ok((commit(result, &job, true), settled > 0))
        }
        JobAction::Line { job, wait, line } => {
            let record = job_mut(host_state(task, container)?, &job)?;
            let monitored = record
                .monitors
                .iter()
                .any(|m| m.wait == wait && m.on == MonitorOn::Line);
            if !monitored {
                return Err(bad(format!("job {job} has no line monitor {wait}")));
            }
            let line = bounded_text(&line, JOB_LINE_MAX_BYTES);
            let detail = format!("job {job} printed a matching line: {line}");
            if !resolve(
                run_mut(task, run)?,
                &wait,
                ReceiptOutcome::Ready,
                &detail,
                now,
            ) {
                return Err(bad(format!("monitor {wait} already settled")));
            }
            let result = json!({"job": job, "wait": wait, "state": "matched"});
            Ok((commit(result, &job, false), true))
        }
    }
}

fn monitor(
    task: &mut Task,
    origin: &corral_core::agent::RequestOrigin,
    run: &str,
    payload: MonitorPayload,
) -> Result<(Commit, bool)> {
    let now = Timestamp::now();
    word(&payload.wait, "wait")?;
    match (&payload.on, &payload.pattern) {
        (MonitorOn::Line, Some(p)) if !p.is_empty() && p.len() <= JOB_PATTERN_MAX_BYTES => {}
        (MonitorOn::Exit, None) => {}
        _ => {
            return Err(bad(format!(
                "a line monitor needs a 1-{JOB_PATTERN_MAX_BYTES} byte pattern; an exit \
                 monitor takes none"
            )));
        }
    }
    if payload.deadline <= now {
        return Err(bad("the monitor deadline has already passed"));
    }
    let own = task.id.clone();
    let open = task.status.holds_slot();
    let target = run_mut(task, run)?;
    if !open || target.outcome.is_some() {
        return Err(Rejection::new(RejectCode::RunEnded, "the run no longer holds a slot").into());
    }
    if target
        .obligations
        .waits
        .iter()
        .any(|w| w.request.id == payload.wait)
        || target
            .obligations
            .decisions
            .iter()
            .any(|d| d.request.id == payload.wait)
    {
        return Err(bad(format!("wait {} already exists", payload.wait)));
    }
    let container = &origin.binding.container;
    let record = job_mut(host_state(task, container)?, &payload.job)?;
    if record.monitors.len() >= JOB_MONITORS_MAX {
        return Err(bad(format!(
            "job {} already has {JOB_MONITORS_MAX} monitors",
            payload.job
        )));
    }
    let condition = match &payload.pattern {
        Some(pattern) => format!("job {} prints a line matching /{pattern}/", payload.job),
        None => format!("job {} exits", payload.job),
    };
    record.monitors.push(JobMonitor {
        wait: payload.wait.clone(),
        on: payload.on,
        pattern: payload.pattern.clone(),
        request_id: origin.request_id.clone(),
        at: now,
    });
    let job = record.clone();
    let target = run_mut(task, run)?;
    let parked = target.parked_on(&own);
    target.obligations.waits.push(WaitRecord {
        request: WaitRequest {
            version: 1,
            id: payload.wait.clone(),
            run: run.to_owned(),
            condition,
            owner: "worker".into(),
            source: ObligationSource::Worker,
            deadline: Some(payload.deadline),
            handle: WaitHandle::Channel {
                channel_id: format!("job:{}", payload.job),
                capability: Some("wait_v1".into()),
            },
        },
        registered_at: now,
        observation: WaitObservation {
            at: now,
            state: WaitState::Pending,
            detail: "waiting for the job".into(),
        },
        receipt: None,
    });
    target.obligations.settle_parking(&parked, &own, now);
    // A job that already ended answers its new monitor at once, with the same single wake.
    let settled = if job.state.live() {
        0
    } else {
        let outcome = match job.state {
            JobState::Exited if job.exit_code == Some(0) => ReceiptOutcome::Ready,
            JobState::Exited => ReceiptOutcome::Failed,
            _ => ReceiptOutcome::Lost,
        };
        let only = JobRecord {
            monitors: vec![job.monitors.last().cloned().context("monitor just added")?],
            ..job.clone()
        };
        resolve_ended(target, &only, outcome, now)
    };
    super::super::effects::subscribe_wait(task, origin, &payload.wait)?;
    let result = json!({
        "wait": payload.wait, "job": payload.job, "armed": true,
        "deadline": payload.deadline, "settled": settled > 0,
    });
    Ok((commit(result, &payload.job, false), settled > 0))
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    let (task_id, run) = match (
        envelope.identity.task.as_deref(),
        envelope.identity.run.as_deref(),
    ) {
        (Some(task), Some(run)) => (task.to_owned(), run.to_owned()),
        _ => return Err(bad("a job request names its task and run")),
    };
    let input = if op == AgentOp::JobMonitor {
        None
    } else {
        Some(super::payload::<JobAction>(
            envelope,
            "action (prepare|started|exited|line), job, and its fields",
        )?)
    };
    let watch = if op == AgentOp::JobMonitor {
        Some(super::payload::<MonitorPayload>(
            envelope,
            "wait, job, on (exit|line), pattern?, deadline",
        )?)
    } else {
        None
    };
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            let response = receipts::respond(ctx, &container, *stored, true)?;
            post_wakes(ctx, &task_id)?;
            return Ok(response);
        }
        Admission::New(admitted) => admitted,
    };
    let generation = admitted
        .resolved
        .state(&container)
        .and_then(|state| state.current_generation());
    if !generation.is_some_and(|g| g.capability(FEATURE)) {
        return Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "background_shell is disabled for this generation",
        )
        .into());
    }
    let max_jobs = generation
        .and_then(|g| g.capabilities.as_ref())
        .and_then(|snapshot| snapshot.features.get(FEATURE))
        .and_then(|state| state.settings.get("max_jobs"))
        .and_then(Value::as_u64)
        .map_or(1, |n| u32::try_from(n).unwrap_or(1));
    let n = envelope.identity.generation;
    let origin = admitted.origin.clone();
    let mut settled = false;
    let response = receipts::commit_with_task(ctx, &admitted, |task| {
        let (commit, any) = match (input, watch) {
            (Some(action), _) => apply(task, &container, &run, n, max_jobs, action)?,
            (None, Some(payload)) => monitor(task, &origin, &run, payload)?,
            (None, None) => return Err(bad("empty job request")),
        };
        settled = any;
        Ok(commit)
    })?;
    if settled {
        crate::parked::reacquire(ctx, &task_id)?;
    }
    post_wakes(ctx, &task_id)?;
    Ok(response)
}
