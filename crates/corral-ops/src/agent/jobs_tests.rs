//! Background jobs (T-565) through the real dispatch path: durable job identity, monitors as
//! receipted waits with one wake each, and reaping of verified owned process groups only.
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command};

use corral_core::agent::{AgentRole, ContainerRef, JobState, ResumeClass, TurnClass};
use corral_core::{Status, WaitState};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::bridge::interrupted::classify_upstream;
use super::effects::process_start;
use super::test_support::{self as ts, Host, code, committed, envelope, hello, send};
use super::{BindTarget, CallResponse, fence_generation, tool_request_id};

struct Worker {
    fx: ts::Fx,
    task: String,
    host: Host,
}

impl Worker {
    fn new() -> Self {
        let fx = ts::fx();
        Self::bound(fx)
    }

    fn bound(fx: ts::Fx) -> Self {
        let task = ts::worker_task(&fx, "p1");
        let host = ts::bind(&fx, &Self::target(&task), "p1", "s1");
        committed(&hello(&fx, &host));
        Self { fx, task, host }
    }

    fn target(task: &str) -> BindTarget {
        BindTarget::Worker {
            task: task.into(),
            run: "R1".into(),
        }
    }

    fn container(&self) -> ContainerRef {
        ContainerRef::Worker {
            task: self.task.clone(),
            run: "R1".into(),
        }
    }

    fn call(&self, op: &str, id: &str, payload: Value) -> CallResponse {
        let request = tool_request_id(&self.host.identity.session_id, id);
        send(
            &self.fx,
            &envelope(&self.host, op, &request, payload, false),
            op,
        )
    }

    fn job(&self, id: &str, payload: Value) -> CallResponse {
        self.call("job", id, payload)
    }

    fn start(&self, job: &str, child: &Child, start_time: &str) {
        committed(&self.job(
            &format!("{job}/prepare"),
            json!({"action": "prepare", "job": job, "command": "sleep 30"}),
        ));
        committed(&self.job(
            &format!("{job}/started"),
            json!({"action": "started", "job": job, "pid": child.id(), "start_time": start_time}),
        ));
    }

    fn monitor(&self, wait: &str, job: &str, on: &str) -> CallResponse {
        let deadline = Timestamp::now() + SignedDuration::from_secs(3600);
        let mut payload = json!({"wait": wait, "job": job, "on": on, "deadline": deadline});
        if on == "line" {
            payload["pattern"] = json!("READY");
        }
        self.call("job_monitor", wait, payload)
    }

    fn wait_state(&self, wait: &str) -> WaitState {
        let task = self.fx.ctx.store.get(&self.task).unwrap();
        let run = task.current_run().unwrap();
        let record = run.obligations.waits.iter().find(|w| w.request.id == wait);
        record.unwrap().observation.state
    }

    fn job_state(&self, job: &str) -> (JobState, Option<String>) {
        let state = ts::state(&self.fx, &self.container());
        let record = state.jobs.iter().find(|j| j.id == job).unwrap();
        (record.state, record.detail.clone())
    }

    fn wakes(&self) -> usize {
        ts::events(&self.fx, "agent_wake_posted")
    }
}

/// A sleeping child in its own process group, as the host starts jobs.
fn spawn_group() -> (Child, String) {
    let child = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let start = process_start(child.id()).unwrap().unwrap();
    (child, start)
}

#[test]
fn exit_monitor_armed_before_exit_wakes_exactly_once() {
    // Given a running job and an exit monitor registered before the job exits.
    let w = Worker::new();
    let (mut child, start) = spawn_group();
    w.start("j1", &child, &start);
    let monitor = committed(&w.monitor("j1.m1", "j1", "exit"));
    assert_eq!(monitor.result["armed"], true);
    assert_eq!(monitor.result["settled"], false);
    assert_eq!(w.wait_state("j1.m1"), WaitState::Pending);
    let state = ts::state(&w.fx, &w.container());
    assert_eq!(state.jobs[0].pid, Some(child.id()));
    assert_eq!(state.jobs[0].start_time.as_deref(), Some(start.as_str()));
    assert_eq!(state.jobs[0].monitors[0].wait, "j1.m1");
    assert_eq!(w.wakes(), 0);
    // When the barrier is released: the process exits and the host reports it, twice.
    child.kill().unwrap();
    child.wait().unwrap();
    let exited = json!({"action": "exited", "job": "j1", "exit_code": 0});
    let first = committed(&w.job("j1/exited", exited.clone()));
    let replay = w.job("j1/exited", exited);
    // Then the monitor settled ready, and one wake was posted despite the replay and a sync.
    assert_eq!(first.result["settled"], 1);
    assert_eq!(replay.receipt(), Some(&first));
    assert_eq!(w.wait_state("j1.m1"), WaitState::Ready);
    assert_eq!(w.job_state("j1").0, JobState::Exited);
    crate::agent::hooks::sync::task(
        &w.fx.ctx,
        &w.fx.ctx.store.get(&w.task).unwrap(),
        Timestamp::now(),
    )
    .unwrap();
    assert_eq!(w.wakes(), 1);
}

#[test]
fn line_monitor_settles_on_its_reported_match_only_once() {
    // Given a line monitor and an exit monitor on one running job.
    let w = Worker::new();
    let (mut child, start) = spawn_group();
    w.start("j1", &child, &start);
    committed(&w.monitor("j1.line", "j1", "line"));
    committed(&w.monitor("j1.exit", "j1", "exit"));
    // When the host reports the matching line, a duplicate match and then the exit.
    let line = json!({"action": "line", "job": "j1", "wait": "j1.line", "line": "READY on :8080"});
    committed(&w.job("j1/line", line));
    let again = w.job(
        "j1/line-again",
        json!({"action": "line", "job": "j1", "wait": "j1.line", "line": "READY"}),
    );
    child.kill().unwrap();
    child.wait().unwrap();
    committed(&w.job(
        "j1/exited",
        json!({"action": "exited", "job": "j1", "signal": "SIGKILL"}),
    ));
    // Then the line monitor kept its match, the exit monitor failed, and each woke once.
    assert_eq!(code(&again), Some("bad_request"));
    assert_eq!(w.wait_state("j1.line"), WaitState::Ready);
    assert_eq!(w.wait_state("j1.exit"), WaitState::Failed);
    assert_eq!(w.wakes(), 2);
}

#[test]
fn monitor_registered_after_exit_wakes_once_at_registration() {
    // Given a job whose exit corral already recorded.
    let w = Worker::new();
    let (mut child, start) = spawn_group();
    w.start("j1", &child, &start);
    child.kill().unwrap();
    child.wait().unwrap();
    committed(&w.job(
        "j1/exited",
        json!({"action": "exited", "job": "j1", "exit_code": 3}),
    ));
    // When exit and line monitors are registered afterwards.
    let exit = committed(&w.monitor("j1.exit", "j1", "exit"));
    let line = committed(&w.monitor("j1.line", "j1", "line"));
    // Then each registration settled at once with its single wake.
    assert_eq!(exit.result["settled"], true);
    assert_eq!(line.result["settled"], true);
    assert_eq!(w.wait_state("j1.exit"), WaitState::Failed);
    assert_eq!(w.wait_state("j1.line"), WaitState::Failed);
    assert_eq!(w.wakes(), 2);
}

#[test]
fn an_armed_monitor_classifies_the_ended_turn_waiting_external() {
    // Given an open turn that registers a monitor on a running job.
    let w = Worker::new();
    let (mut child, start) = spawn_group();
    w.start("j1", &child, &start);
    let input = ts::event_input(&w.host, 1, "turn_started", 1);
    committed(&send(&w.fx, &input, "event"));
    committed(&w.monitor("j1.m1", "j1", "exit"));
    // When the turn ends.
    let input = ts::event_input(&w.host, 2, "turn_ended", 1);
    committed(&send(&w.fx, &input, "event"));
    // Then it is truthfully waiting on the external job.
    let state = ts::state(&w.fx, &w.container());
    assert_eq!(state.turns[0].class, Some(TurnClass::WaitingExternal));
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn bounds_role_and_capability_refuse_job_operations() {
    // Given a worker at the default limit of four live jobs.
    let w = Worker::new();
    for n in 0..4 {
        committed(&w.job(
            &format!("j{n}"),
            json!({"action": "prepare", "job": format!("j{n}"), "command": "true"}),
        ));
    }
    // When a fifth job, a malformed monitor and an unknown job are requested.
    let fifth = w.job(
        "j4",
        json!({"action": "prepare", "job": "j4", "command": "true"}),
    );
    let deadline = Timestamp::now() + SignedDuration::from_secs(60);
    let patternless = w.call(
        "job_monitor",
        "m",
        json!({"wait": "m", "job": "j0", "on": "line", "deadline": deadline}),
    );
    let unknown = w.monitor("m2", "nope", "exit");
    // Then each is refused before any effect.
    assert!(
        fifth.rejection().unwrap().reason.contains("job_limit"),
        "{fifth:?}"
    );
    assert_eq!(code(&patternless), Some("bad_request"));
    assert_eq!(code(&unknown), Some("bad_request"));
    assert_eq!(ts::state(&w.fx, &w.container()).jobs.len(), 4);

    // Given a worker whose configuration disables background jobs, and a reviewer.
    let mut fx = ts::fx();
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set(AgentRole::Worker, "background_shell", false)
        .unwrap();
    let disabled = Worker::bound(fx);
    let prepare = json!({"action": "prepare", "job": "j1", "command": "true"});
    let refused = disabled.job("j1", prepare.clone());
    let (_, target) = ts::reviewer_task(&disabled.fx);
    let reviewer = ts::bind(&disabled.fx, &target, "p2", "s2");
    let request = tool_request_id("s2", "j1");
    let foreign = send(
        &disabled.fx,
        &envelope(&reviewer, "job", &request, prepare, false),
        "job",
    );
    // Then neither may run a job.
    assert_eq!(code(&refused), Some("op_not_allowed"));
    assert_eq!(code(&foreign), Some("op_not_allowed"));
    assert!(
        ts::state(&disabled.fx, &disabled.container())
            .jobs
            .is_empty()
    );
}

#[test]
fn fence_reaps_verified_groups_and_spares_a_reused_pid() {
    // Given an owned job, a job whose pid now belongs to another process, a job that never got a
    // process identity, and an exit monitor on the owned one.
    let w = Worker::new();
    let (mut owned, start) = spawn_group();
    w.start("owned", &owned, &start);
    let (mut stranger, _) = spawn_group();
    w.start("reused", &stranger, "1");
    committed(&w.job(
        "bare/prepare",
        json!({"action": "prepare", "job": "bare", "command": "true"}),
    ));
    committed(&w.monitor("owned.m1", "owned", "exit"));
    // When the generation is fenced and sync runs, twice.
    let g = w.host.identity.generation;
    fence_generation(&w.fx.ctx.store, &w.container(), g, "resume").unwrap();
    let task = w.fx.ctx.store.get(&w.task).unwrap();
    crate::agent::hooks::sync::task(&w.fx.ctx, &task, Timestamp::now()).unwrap();
    let second = super::jobs::reap(&w.fx.ctx, &w.task, Timestamp::now()).unwrap();
    // Then only the verified group was killed, the others are reported, and one wake came.
    let status = owned.wait().unwrap();
    assert_eq!(status.signal(), Some(9));
    assert_eq!(w.job_state("owned").0, JobState::Reaped);
    let (reused, detail) = w.job_state("reused");
    assert_eq!(reused, JobState::Lost);
    assert!(detail.unwrap().contains("another process"));
    assert_eq!(stranger.try_wait().unwrap(), None);
    assert_eq!(w.job_state("bare").0, JobState::Lost);
    assert_eq!(w.wait_state("owned.m1"), WaitState::Lost);
    assert_eq!(second, 0);
    assert_eq!(w.wakes(), 1);
    assert_eq!(ts::events(&w.fx, "agent_job_reaped"), 3);
    stranger.kill().unwrap();
    stranger.wait().unwrap();
}

#[test]
fn host_loss_resume_reaps_and_the_old_generation_cannot_report() {
    // Given a running job of generation 1 whose host was lost.
    let w = Worker::new();
    let (mut child, start) = spawn_group();
    w.start("j1", &child, &start);
    // When an explicit resume opens generation 2 and sync runs.
    let next = ts::bind(&w.fx, &Worker::target(&w.task), "p1", "s2");
    committed(&hello(&w.fx, &next));
    let task = w.fx.ctx.store.get(&w.task).unwrap();
    crate::agent::hooks::sync::task(&w.fx.ctx, &task, Timestamp::now()).unwrap();
    // Then the job of the superseded generation is reaped and its late exit report refused.
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    assert_eq!(w.job_state("j1").0, JobState::Reaped);
    let late = w.job(
        "j1/exited",
        json!({"action": "exited", "job": "j1", "exit_code": 0}),
    );
    assert!(late.rejection().is_some(), "{late:?}");
    assert_eq!(next.identity.generation, w.host.identity.generation + 1);
}

#[test]
fn run_end_reaps_live_jobs_and_keeps_syncing_until_they_end() {
    // Given a running job on a run that ends.
    let w = Worker::new();
    let (mut child, start) = spawn_group();
    w.start("j1", &child, &start);
    w.fx.ctx
        .store
        .update(&w.task, |t| {
            t.status = Status::Done;
            t.runs[0].outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let ended = w.fx.ctx.store.get(&w.task).unwrap();
    // When sync visits it.
    assert!(crate::agent::hooks::sync::wants(&ended));
    crate::agent::hooks::sync::task(&w.fx.ctx, &ended, Timestamp::now()).unwrap();
    // Then its process group is killed and recorded as reaped.
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    assert_eq!(w.job_state("j1").0, JobState::Reaped);
}

#[test]
fn interrupted_job_tools_are_classified_conservatively() {
    // Given each job tool interrupted by a lost host, then its recovery class.
    let (output, _) = classify_upstream("job_output", None);
    let (start, detail) = classify_upstream("job_start", None);
    let (kill, _) = classify_upstream("job_kill", None);
    // Then only the output read claims no effect.
    assert_eq!(output, ResumeClass::InterruptedNoEffect);
    assert_eq!(start, ResumeClass::OutcomeUnknown);
    assert_eq!(kill, ResumeClass::OutcomeUnknown);
    assert!(detail.unwrap().contains("reaps"));
}

/// A group whose leader waits on stdin while a SIGTERM-ignoring descendant sleeps, with a pidfd
/// on the descendant opened before anything is released.
struct Orphaning {
    leader: Child,
    start: String,
    descendant: rustix::fd::OwnedFd,
}

fn orphaning_group() -> Orphaning {
    use std::io::BufRead;
    use std::process::Stdio;
    let mut leader = Command::new("bash")
        .args([
            "-c",
            "(trap '' TERM; exec sleep 30) & echo $!; read -r _; exit 0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let start = process_start(leader.id()).unwrap().unwrap();
    let mut line = String::new();
    std::io::BufReader::new(leader.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let pid = rustix::process::Pid::from_raw(line.trim().parse().unwrap()).unwrap();
    let descendant =
        rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).unwrap();
    Orphaning {
        leader,
        start,
        descendant,
    }
}

impl Orphaning {
    /// Lets the leader exit and reaps it; the descendant stays in the group.
    fn release_leader(&mut self) {
        drop(self.leader.stdin.take());
        assert!(self.leader.wait().unwrap().success());
    }

    fn descendant_exited(&self) -> bool {
        use rustix::event::{PollFd, PollFlags, poll};
        let mut ready = [PollFd::new(&self.descendant, PollFlags::IN)];
        let timeout = rustix::time::Timespec {
            tv_sec: 30,
            tv_nsec: 0,
        };
        poll(&mut ready, Some(&timeout)).unwrap() == 1
    }
}

#[test]
fn a_group_whose_leader_exited_is_still_reaped_whole() {
    // Given a recorded running job whose leader exits, leaving a TERM-ignoring descendant.
    let w = Worker::new();
    let mut group = orphaning_group();
    w.start("j1", &group.leader, &group.start);
    committed(&w.monitor("j1.m1", "j1", "exit"));
    group.release_leader();
    assert_eq!(
        super::jobs::owned_group(group.leader.id(), &group.start).unwrap(),
        super::jobs::Group::Owned(1)
    );
    // When the generation is fenced and the reaper runs.
    let g = w.host.identity.generation;
    fence_generation(&w.fx.ctx.store, &w.container(), g, "resume").unwrap();
    super::jobs::reap(&w.fx.ctx, &w.task, Timestamp::now()).unwrap();
    // Then the descendant is killed through the verified group and the monitor settles lost.
    assert!(group.descendant_exited());
    let state = ts::state(&w.fx, &w.container());
    assert_eq!(state.jobs[0].state, JobState::Reaped);
    assert!(state.jobs[0].group_clear);
    assert_eq!(w.wait_state("j1.m1"), WaitState::Lost);
}

#[test]
fn an_exit_reported_without_a_clear_group_is_swept_once() {
    // Given an exit report that did not verify the group empty, with a descendant still alive.
    let w = Worker::new();
    let mut group = orphaning_group();
    w.start("j1", &group.leader, &group.start);
    committed(&w.monitor("j1.m1", "j1", "exit"));
    group.release_leader();
    committed(&w.job(
        "j1/exited",
        json!({"action": "exited", "job": "j1", "exit_code": 0}),
    ));
    assert_eq!(w.wait_state("j1.m1"), WaitState::Ready);
    // When the run ends and sync visits it twice.
    w.fx.ctx
        .store
        .update(&w.task, |t| {
            t.status = Status::Done;
            t.runs[0].outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let ended = w.fx.ctx.store.get(&w.task).unwrap();
    assert!(crate::agent::hooks::sync::wants(&ended));
    crate::agent::hooks::sync::task(&w.fx.ctx, &ended, Timestamp::now()).unwrap();
    let again = super::jobs::reap(&w.fx.ctx, &w.task, Timestamp::now()).unwrap();
    // Then the remaining member is killed, the exit and its monitor keep their outcome, and the
    // second pass has nothing left to do.
    assert!(group.descendant_exited());
    let state = ts::state(&w.fx, &w.container());
    assert_eq!(
        (
            state.jobs[0].state,
            state.jobs[0].exit_code,
            state.jobs[0].group_clear
        ),
        (JobState::Exited, Some(0), true)
    );
    assert_eq!(w.wait_state("j1.m1"), WaitState::Ready);
    assert_eq!(again, 0);
    assert_eq!(w.wakes(), 1);
}

#[test]
fn a_host_verified_clear_group_is_never_signalled_again() {
    // Given an exit report whose host verified the group empty.
    let w = Worker::new();
    let (mut child, start) = spawn_group();
    w.start("j1", &child, &start);
    child.kill().unwrap();
    child.wait().unwrap();
    committed(&w.job(
        "j1/exited",
        json!({"action": "exited", "job": "j1", "signal": "SIGKILL", "group_clear": true}),
    ));
    // When the generation is fenced, then the reaper has nothing to sweep.
    let g = w.host.identity.generation;
    fence_generation(&w.fx.ctx.store, &w.container(), g, "resume").unwrap();
    assert_eq!(
        super::jobs::reap(&w.fx.ctx, &w.task, Timestamp::now()).unwrap(),
        0
    );
    assert_eq!(ts::events(&w.fx, "agent_job_reaped"), 0);
}
