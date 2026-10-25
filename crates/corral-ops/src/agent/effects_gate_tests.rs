//! The two-phase gate (F1.a): crash windows, the submission fence on the fresh and restored
//! paths, and post-refusal kills, each with a real executor process that is SIGKILLed.
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::*;

/// A `corral agent call` child running `effects::gate`, armed at comma-separated failpoints.
struct Exec {
    child: Child,
    lines: mpsc::Receiver<String>,
    err: PathBuf,
}

fn spawn(fx: &Fx, o: &RequestOrigin, points: &str) -> Exec {
    let spec = json!({"root": fx.root(), "identity": o.identity, "request": o.request_id, "payload": o.payload_sha256});
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "agent::effects_tests::gate::gate_executor_child",
            "--ignored",
            "--nocapture",
        ])
        .args(["--test-threads", "1"])
        .env("CORRAL_T300_EXECUTOR", spec.to_string())
        .env("CORRAL_AGENT_FAILPOINT", points)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(fx.root().join("executor.err")).unwrap())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let err = fx.root().join("executor.err");
    Exec { child, lines, err }
}

impl Exec {
    fn expect(&self, marker: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => match line.split_once(marker) {
                    Some((_, rest)) => return rest.trim().to_owned(),
                    None => seen.push(line),
                },
                Err(error) => panic!(
                    "the executor never printed {marker}: {error}\n{seen:#?}\n{}",
                    std::fs::read_to_string(&self.err).unwrap_or_default()
                ),
            }
        }
    }

    fn reached(&self, point: &str) -> &Self {
        self.expect(&format!("FAILPOINT {point}"));
        self
    }

    fn release(&mut self) -> &mut Self {
        self.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"go\n")
            .unwrap();
        self
    }

    fn finish(mut self) -> Value {
        let result = serde_json::from_str(&self.expect("RESULT ")).unwrap();
        assert!(self.child.wait().unwrap().success());
        result
    }

    fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Exec {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
#[ignore = "entry point of the gate executor child process"]
fn gate_executor_child() {
    let Ok(spec) = std::env::var("CORRAL_T300_EXECUTOR") else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let ctx = ctx_at(&PathBuf::from(spec["root"].as_str().unwrap()));
    let identity: RequestIdentity = serde_json::from_value(spec["identity"].clone()).unwrap();
    let container = ContainerRef::Worker {
        task: identity.task.clone().unwrap(),
        run: identity.run.clone().unwrap(),
    };
    let binding = RequestBinding {
        container,
        generation: identity.generation,
        callback: None,
        settles_turn: None,
    };
    let o = RequestOrigin {
        request_id: spec["request"].as_str().unwrap().into(),
        op: "gate".into(),
        payload_sha256: spec["payload"].as_str().unwrap().into(),
        identity,
        binding,
    };
    let response = effects::gate(&ctx, &o).unwrap();
    println!("\nRESULT {}", response.to_json().unwrap());
}

fn reconcile(fx: &Fx) {
    effects::reconcile_task(&fx.ctx, &fx.id).unwrap();
}

fn attempt(fx: &Fx, o: &RequestOrigin, attempt: &str) -> AttemptState {
    let task = fx.task();
    crate::review_budget::attempt_state(
        &fx.ctx,
        &task,
        "R1",
        Kind::Gate,
        attempt,
        Some(&o.request_id),
    )
    .unwrap()
}

fn disposition(fx: &Fx, attempt: &str) -> SubmissionDisposition {
    let kind = corral_core::OperatingActionKind::Gate;
    let authorization = crate::operating::load(&fx.ctx, &fx.id, "R1", kind, attempt).unwrap();
    fx.ctx
        .store
        .authorized_submission(&authorization.id)
        .unwrap()
        .disposition
}

fn wake(fx: &Fx, o: &RequestOrigin) -> WakeState {
    let task = fx.task();
    let state = core_agent::container_state(&task, &o.binding.container).unwrap();
    state
        .wakes
        .iter()
        .find(|w| w.request_id == o.request_id)
        .unwrap()
        .state
}

#[cfg(target_os = "linux")]
#[test]
fn an_executor_exiting_during_the_stat_read_is_settled_by_reconciliation() {
    use rustix::event::{poll, PollFd, PollFlags};
    use rustix::io::Errno;
    use rustix::process::{pidfd_open, Pid, PidfdFlags};
    use rustix::time::Timespec;

    let fx = fixture(true);
    let g = fx.bind();
    let o = fx.call(g, "gate-esrch", "gate");
    let mut exec = spawn(&fx, &o, "gate_reserved_before_authorization");
    exec.reached("gate_reserved_before_authorization");
    assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Reserved);

    let pid = Pid::from_raw(i32::try_from(exec.child.id()).unwrap()).unwrap();
    let pidfd = pidfd_open(pid, PidfdFlags::empty()).unwrap();
    exec.child.kill().unwrap();
    let mut ready = [PollFd::new(&pidfd, PollFlags::IN)];
    let timeout = Timespec {
        tv_sec: 120,
        tv_nsec: 0,
    };
    assert_eq!(poll(&mut ready, Some(&timeout)).unwrap(), 1, "pidfd ready");

    // The executor is unreaped and its exit has been observed; the read that follows fails as
    // it does for a process reaped mid-read.
    effects::STAT_FAULT.with(|fault| fault.set(Some(Errno::SRCH.raw_os_error())));
    reconcile(&fx);
    assert_eq!(
        effects::STAT_FAULT.with(std::cell::Cell::get),
        None,
        "the ESRCH read was consumed"
    );
    let settled = fx.receipt(&o).unwrap();
    assert_eq!(
        (settled.state, settled.result["outcome"].as_str()),
        (ReceiptState::Abandoned, Some("not_started"))
    );
}

#[test]
fn gate_crash_windows_settle_from_the_attempt_evidence_and_never_start_twice() {
    let fx = fixture(true);
    let g = fx.bind();
    let o = fx.call(g, "gate-a", "gate");
    let exec = spawn(&fx, &o, "gate_before_reservation");
    exec.reached("gate_before_reservation");
    exec.kill();
    assert!(
        fx.receipt(&o).is_none(),
        "not_executed: no reservation exists"
    );

    let exec = spawn(&fx, &o, "gate_reserved_before_authorization");
    exec.reached("gate_reserved_before_authorization");
    assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Reserved);
    exec.kill();
    reconcile(&fx);
    let settled = fx.receipt(&o).unwrap();
    assert_eq!(
        (settled.state, settled.result["outcome"].as_str()),
        (ReceiptState::Abandoned, Some("not_started"))
    );
    assert_eq!(wake(&fx, &o), WakeState::Cancelled);

    let o = fx.call(g, "gate-b", "gate");
    let own = effects::own_attempt(&o.request_id);
    let exec = spawn(&fx, &o, "gate_submit_ipc");
    exec.reached("gate_submit_ipc");
    exec.kill();
    assert_eq!(attempt(&fx, &o, &own), AttemptState::Submitting);
    reconcile(&fx);
    assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Uncertain);
    let later = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(3600);
    let due = effects::due_gate_wakes(&fx.task(), later, Duration::from_secs(300));
    assert_eq!(
        due.iter().map(|d| d.reason.as_str()).collect::<Vec<_>>(),
        ["gate_start_uncertain"]
    );
    assert!(effects::fire_wake(&fx.ctx.store, &due[0], None).unwrap());
    assert!(
        !effects::fire_wake(&fx.ctx.store, &due[0], None).unwrap(),
        "a wake fires once"
    );
    let log = crate::gate::gate_paths(&fx.ctx, &fx.id, "R1").log;
    std::fs::write(log.with_extension("runner.json"), "{}").unwrap();
    reconcile(&fx);
    let started = fx.receipt(&o).unwrap();
    assert_eq!(
        (started.state, started.result["attempt"].as_str()),
        (ReceiptState::Committed, Some(own.as_str()))
    );
    assert_eq!(
        fx.calls("pane run"),
        0,
        "the killed IPC never ran in the fake herdr"
    );

    let fx = fixture(true);
    let g = fx.bind();
    for (call, point) in [
        ("gate-c", "gate_started_before_receipt"),
        ("gate-d", "gate_receipt_before_ack"),
    ] {
        let o = fx.call(g, call, "gate");
        let exec = spawn(&fx, &o, point);
        exec.reached(point);
        exec.kill();
        reconcile(&fx);
        let replay = effects::gate(&fx.ctx, &o).unwrap();
        let receipt = replay.receipt().unwrap();
        assert_eq!(
            (receipt.state, receipt.result["outcome"].as_str()),
            (ReceiptState::Committed, Some("started"))
        );
        assert_eq!(wake(&fx, &o), WakeState::Armed);
        fx.ctx
            .store
            .update(&fx.id, |t| {
                t.runs[0].gate.as_mut().unwrap().result = corral_core::GateResult::Pass;
                Ok(())
            })
            .unwrap();
        let due =
            effects::due_gate_wakes(&fx.task(), jiff::Timestamp::now(), Duration::from_secs(300));
        assert_eq!(due.len(), 1, "fires on the gate's terminal result");
        assert!(effects::fire_wake(&fx.ctx.store, &due[0], None).unwrap());
    }
    assert_eq!(
        fx.calls("pane run"),
        2,
        "each request submitted its gate exactly once"
    );
}

#[test]
fn gate_prepared_attempt_resumes_once_by_replay_and_launches_without_requests_skip_the_fence() {
    let fx = fixture(true);
    let g = fx.bind();
    let o = fx.call(g, "gate-a", "gate");
    let exec = spawn(&fx, &o, "gate_submit_before_lock");
    exec.reached("gate_submit_before_lock");
    exec.kill();
    reconcile(&fx);
    let reserved = fx.receipt(&o).unwrap();
    assert_eq!(
        (reserved.state, reserved.result["outcome"].as_str()),
        (ReceiptState::Reserved, Some("prepared"))
    );
    assert_eq!(wake(&fx, &o), WakeState::PendingStart);
    let replay = effects::gate(&fx.ctx, &o).unwrap();
    let started = replay.receipt().unwrap();
    assert_eq!(
        started.result["attempt"].as_str(),
        Some(effects::own_attempt(&o.request_id).as_str())
    );
    assert_eq!(
        (started.state, fx.calls("pane run")),
        (ReceiptState::Committed, 1)
    );
    let again = effects::gate(&fx.ctx, &o).unwrap();
    assert_eq!((again.receipt(), fx.calls("pane run")), (Some(started), 1));

    // Given a fenced binding, when the owner starts a gate without a request, it is not fenced.
    let fx = fixture(true);
    let g = fx.bind();
    fence_generation(&fx.ctx.store, &fx.worker(), g, "resume").unwrap();
    crate::gate::gate_start(&fx.ctx, &fx.id).unwrap();
    assert_eq!(fx.calls("pane run"), 1);
}

#[derive(Clone, Copy, Debug)]
enum Ending {
    Fence,
    Discard,
    Retry,
    Candidate,
}

/// Ends the binding (or, for `Candidate`, dirties the worktree) and returns the refusal code
/// and the binding the receipt reports.
fn end(fx: &Fx, g: u32, ending: Ending) -> (&'static str, &'static str) {
    match ending {
        Ending::Fence => {
            fence_generation(&fx.ctx.store, &fx.worker(), g, "resume").unwrap();
            return ("agent_fenced", "agent_fenced");
        }
        Ending::Candidate => {
            std::fs::write(fx.root().join("app/untracked"), "dirty\n").unwrap();
            return ("precondition_refused", "live");
        }
        Ending::Discard => fx.ctx.store.update(&fx.id, |t| {
            t.runs[0].outcome = Some(RunOutcome::Discarded);
            Ok(())
        }),
        Ending::Retry => {
            let r2 = run("R2", &fx.root().join("app"), "w0:p1");
            fx.ctx.store.update(&fx.id, |t| {
                t.runs.push(r2);
                Ok(())
            })
        }
    }
    .unwrap();
    ("run_ended", "run_ended")
}

/// Leaves a prepared, never-submitted attempt of `call` by killing its executor at the barrier.
fn prepared(fx: &Fx, g: u32, call: &str) -> RequestOrigin {
    let o = fx.call(g, call, "gate");
    let exec = spawn(fx, &o, "gate_submit_before_lock");
    exec.reached("gate_submit_before_lock");
    exec.kill();
    o
}

#[derive(Clone, Copy, Debug)]
enum Way {
    Fresh,
    Replay,
    Adoption,
}

/// A request that reaches the submission boundary on `way` and the attempt it submits: a
/// fresh request, or the restored branch through its own replay or a new request's adoption.
fn on(fx: &Fx, g: u32, way: Way) -> (RequestOrigin, String) {
    let first = match way {
        Way::Fresh => fx.call(g, "gate-a", "gate"),
        Way::Replay | Way::Adoption => prepared(fx, g, "gate-a"),
    };
    let own = effects::own_attempt(&first.request_id);
    match way {
        Way::Adoption => (fx.call(g, "gate-b", "gate"), own),
        Way::Fresh | Way::Replay => (first, own),
    }
}

fn log_lines(fx: &Fx) -> Vec<String> {
    let log = std::fs::read_to_string(fx.root().join("calls.log")).unwrap_or_default();
    log.lines().map(String::from).collect()
}

fn assert_refused(fx: &Fx, o: &RequestOrigin, attempt_id: &str, (code, binding): (&str, &str)) {
    let receipt = fx.receipt(o).unwrap();
    let result = &receipt.result;
    assert_eq!(
        (receipt.state, result["outcome"].as_str()),
        (ReceiptState::Rejected, Some("submission_refused"))
    );
    assert_eq!(
        (result["code"].as_str(), result["binding"].as_str()),
        (Some(code), Some(binding))
    );
    assert_eq!(result["attempt"].as_str(), Some(attempt_id));
    let SubmissionDisposition::AuthorizationRefused { reason } = disposition(fx, attempt_id) else {
        panic!("the authorized submission must record the refusal");
    };
    assert!(
        reason.contains(&format!("agent_request={}", o.request_id)),
        "{reason}"
    );
    let refusal = format!("agent_refusal={code}");
    assert_eq!(
        reason.contains(&refusal),
        code != "precondition_refused",
        "{reason}"
    );
    assert!(matches!(
        attempt(fx, o, attempt_id),
        AttemptState::Refused { .. }
    ));
    let retained = crate::review_budget::retained_attempt(&fx.ctx, &fx.id, "R1").unwrap();
    assert_eq!(
        retained.as_deref(),
        Some(attempt_id),
        "the preparation keeps its attempt"
    );
    assert_eq!(wake(fx, o), WakeState::Cancelled);
    assert_eq!(fx.calls("pane run"), 0);
}

#[test]
fn a_fence_released_into_the_submission_boundary_refuses_on_all_three_paths() {
    let cases = [
        (Way::Fresh, Ending::Fence),
        (Way::Fresh, Ending::Discard),
        (Way::Fresh, Ending::Retry),
        (Way::Fresh, Ending::Candidate),
        (Way::Replay, Ending::Fence),
        (Way::Replay, Ending::Discard),
        (Way::Replay, Ending::Retry),
        (Way::Adoption, Ending::Fence),
        (Way::Adoption, Ending::Discard),
        (Way::Adoption, Ending::Retry),
    ];
    for (way, ending) in cases {
        let fx = fixture(true);
        let g = fx.bind();
        let (o, own) = on(&fx, g, way);
        let before = log_lines(&fx).len();
        let mut exec = spawn(&fx, &o, "gate_submit_before_lock");
        exec.reached("gate_submit_before_lock");
        if !matches!(way, Way::Fresh) {
            let restored = log_lines(&fx).split_off(before);
            let attached =
                |c: &String| c.starts_with("pane get") || c.starts_with("pane report-metadata");
            assert!(restored.iter().all(attached), "{way:?}: {restored:?}");
        }
        let expected = end(&fx, g, ending);
        exec.release();
        assert_eq!(exec.finish()["state"], "rejected", "{way:?} {ending:?}");
        assert_refused(&fx, &o, &own, expected);
    }
}

#[test]
fn an_executor_killed_after_the_durable_refusal_is_settled_by_reconciliation() {
    for way in [Way::Fresh, Way::Replay, Way::Adoption] {
        for point in ["gate_refusal_before_retain", "gate_refused_before_receipt"] {
            let fx = fixture(true);
            let g = fx.bind();
            let (o, own) = on(&fx, g, way);
            let mut exec = spawn(&fx, &o, &format!("gate_submit_before_lock,{point}"));
            exec.reached("gate_submit_before_lock");
            let expected = end(&fx, g, Ending::Fence);
            exec.release().reached(point);
            exec.kill();
            assert_eq!(fx.receipt(&o).unwrap().state, ReceiptState::Reserved);
            reconcile(&fx);
            assert_refused(&fx, &o, &own, expected);
        }
    }
    for (point, attributed) in [
        ("gate_refusal_before_retain", false),
        ("gate_refused_before_receipt", true),
    ] {
        let fx = fixture(true);
        let g = fx.bind();
        let o = fx.call(g, "gate-a", "gate");
        let config = fx.ctx.store.dir().join("corral.toml");
        let valid = std::fs::read(&config).unwrap();
        let mut exec = spawn(&fx, &o, &format!("gate_submit_before_lock,{point}"));
        exec.reached("gate_submit_before_lock");
        std::fs::write(&config, "not = [valid toml").unwrap();
        end(&fx, g, Ending::Fence);
        exec.release().reached(point);
        exec.kill();
        std::fs::write(&config, valid).unwrap();
        reconcile(&fx);
        let receipt = fx.receipt(&o).unwrap();
        let (state, outcome) = if attributed {
            (ReceiptState::Rejected, "submission_refused")
        } else {
            (ReceiptState::Abandoned, "not_submitted")
        };
        assert_eq!(
            (receipt.state, receipt.result["outcome"].as_str()),
            (state, Some(outcome)),
            "{point}"
        );
        if attributed {
            assert_eq!(receipt.result["code"], "policy_refused");
        } else {
            assert!(
                receipt.result["prior_refusal"].is_string(),
                "attached as evidence only"
            );
        }
        assert_eq!(
            (wake(&fx, &o), fx.calls("pane run")),
            (WakeState::Cancelled, 0)
        );
    }
}

#[test]
fn an_executor_killed_at_the_barrier_while_the_binding_ends_settles_not_submitted() {
    let cases = [
        (Way::Fresh, Ending::Fence),
        (Way::Fresh, Ending::Discard),
        (Way::Replay, Ending::Fence),
        (Way::Replay, Ending::Discard),
        (Way::Adoption, Ending::Fence),
        (Way::Adoption, Ending::Discard),
    ];
    for (way, ending) in cases {
        let fx = fixture(true);
        let g = fx.bind();
        let (o, own) = on(&fx, g, way);
        let exec = spawn(&fx, &o, "gate_submit_before_lock");
        exec.reached("gate_submit_before_lock");
        let (_, binding) = end(&fx, g, ending);
        exec.kill();
        let case = format!("{way:?} {ending:?}");
        let prepared = AttemptState::Prepared {
            prior_refusal: None,
        };
        assert_eq!(attempt(&fx, &o, &own), prepared, "{case}");
        assert_eq!(
            disposition(&fx, &own),
            SubmissionDisposition::Prepared,
            "{case}"
        );
        effects::reconcile_task(&ctx_at(fx.root()), &fx.id).unwrap();
        let settled = fx.receipt(&o).unwrap();
        let result = &settled.result;
        assert_eq!(
            (
                settled.state,
                result["outcome"].as_str(),
                result["binding"].as_str()
            ),
            (
                ReceiptState::Abandoned,
                Some("not_submitted"),
                Some(binding)
            ),
            "{case}"
        );
        assert_eq!(result["attempt"].as_str(), Some(own.as_str()), "{case}");
        assert_eq!(
            (wake(&fx, &o), fx.calls("pane run")),
            (WakeState::Cancelled, 0),
            "{case}"
        );
        if matches!(ending, Ending::Fence) {
            let target = BindTarget::Worker {
                task: fx.id.clone(),
                run: "R1".into(),
            };
            let live = open_generation(&fx.ctx.store, &target, "w0:p1")
                .unwrap()
                .generation;
            let adopter = fx.call(live, "gate-live", "gate");
            let started = effects::gate(&ctx_at(fx.root()), &adopter).unwrap();
            let started = started.receipt().unwrap();
            assert_eq!(
                (
                    started.result["attempt"].as_str(),
                    started.result["adopted"].as_bool()
                ),
                (Some(own.as_str()), Some(true)),
                "{case}"
            );
            assert_eq!(
                fx.calls("pane run"),
                1,
                "{case}: the exact attempt is submitted once"
            );
            assert_eq!(disposition(&fx, &own), SubmissionDisposition::Submitted);
            assert_eq!(
                fx.receipt(&o).as_ref(),
                Some(&settled),
                "settled receipts stay final"
            );
        }
    }
}

#[test]
fn an_executor_killed_before_the_check_is_abandoned_and_the_live_generation_adopts_once() {
    let fx = fixture(true);
    let g1 = fx.bind();
    let first = prepared(&fx, g1, "gate-a");
    let own = effects::own_attempt(&first.request_id);
    let target = BindTarget::Worker {
        task: fx.id.clone(),
        run: "R1".into(),
    };
    let g2 = open_generation(&fx.ctx.store, &target, "w0:p1")
        .unwrap()
        .generation;
    let second = fx.call(g2, "gate-b", "gate");
    let mut exec = spawn(&fx, &second, "gate_submit_before_lock");
    exec.reached("gate_submit_before_lock");
    end(&fx, g2, Ending::Fence);
    exec.release();
    exec.finish();
    reconcile(&fx);
    let abandoned = fx.receipt(&first).unwrap();
    assert_eq!(
        (abandoned.state, abandoned.result["outcome"].as_str()),
        (ReceiptState::Abandoned, Some("not_submitted"))
    );
    assert_eq!(abandoned.result["binding"], "agent_fenced");
    let g3 = open_generation(&fx.ctx.store, &target, "w0:p1")
        .unwrap()
        .generation;
    let third = fx.call(g3, "gate-c", "gate");
    let exec = spawn(&fx, &third, "gate_submit_before_lock");
    exec.reached("gate_submit_before_lock");
    open_generation(&fx.ctx.store, &target, "w0:p1").unwrap();
    exec.kill();
    assert!(matches!(
        attempt(&fx, &third, &own),
        AttemptState::Prepared {
            prior_refusal: Some(_)
        }
    ));
    reconcile(&fx);
    let killed = fx.receipt(&third).unwrap();
    assert_eq!(
        (killed.state, killed.result["outcome"].as_str()),
        (ReceiptState::Abandoned, Some("not_submitted"))
    );
    assert!(killed.result["prior_refusal"]
        .as_str()
        .unwrap()
        .contains(&second.request_id));
    assert_eq!(wake(&fx, &third), WakeState::Cancelled);
    let g5 = fx.task().runs[0]
        .agent_host
        .as_ref()
        .unwrap()
        .binding
        .generation;
    let live = fx.call(g5, "gate-d", "gate");
    let adopted = effects::gate(&fx.ctx, &live).unwrap();
    let receipt = adopted.receipt().unwrap();
    assert_eq!(
        (
            receipt.result["attempt"].as_str(),
            receipt.result["adopted"].as_bool()
        ),
        (Some(own.as_str()), Some(true))
    );
    assert_eq!(
        (receipt.state, fx.calls("pane run")),
        (ReceiptState::Committed, 1)
    );
    assert_eq!(
        fx.receipt(&first),
        Some(abandoned),
        "a settled receipt is never rewritten"
    );
    assert_eq!(disposition(&fx, &own), SubmissionDisposition::Submitted);
}
