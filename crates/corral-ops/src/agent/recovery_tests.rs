//! T-252 recovery acceptance, with real processes standing in for hosts and gate executors: a
//! killed host is lost once while its run, question and queued answer stay; resume classifies
//! the dangling calls of bun-reproof's fixture session and of live gate windows and launches
//! the next generation in the same pane; discard settles a lost run; a lost reviewer resumes
//! its own child; `--replace` signals only a verified host; provider auth goes to the owner;
//! a turn cancel never fences. Waits are bounded and event-driven; nothing sleeps.
use std::sync::mpsc;
use std::time::Duration;

use corral_core::agent::{
    self as core_agent, AgentQuestion, QuestionState, RequestOrigin, WakeState,
};
use corral_core::inbox::Inbox;
use corral_core::{RunOutcome, Status};
use serde_json::{Value, json};

pub(super) use super::receipt;
use super::*;
use crate::agent::bridge::fixture::{self as f, Exec, Fx, HostProc, SESSION};
use crate::agent::bridge::liveness;
use crate::agent::hooks::sync::{AUTH_PREFIX, LOST_PREFIX};
use crate::agent::test_support as ts;

pub(super) const WAIT: Duration = Duration::from_secs(120);

pub(super) fn sync(fx: &Fx) -> crate::agent::hooks::sync::TaskSync {
    liveness::probe_hosts_in_tests();
    crate::agent::hooks::sync::task(&fx.ctx, &fx.task(), jiff::Timestamp::now()).unwrap()
}

pub(super) fn state(fx: &Fx, container: &ContainerRef) -> AgentRunState {
    agent::container_state(&fx.task(), container)
        .unwrap()
        .clone()
}

pub(super) fn run_recipient(fx: &Fx) -> Recipient {
    Recipient::Run {
        task: fx.id.clone(),
        run: "R1".into(),
    }
}

pub(super) fn ask(fx: &Fx, container: &ContainerRef) -> String {
    let key = format!("{}/Q1", container.key());
    let question: AgentQuestion = serde_json::from_value(json!({
        "version": 1, "key": key, "revision": 1, "kind": "clarification",
        "asked": {"request_id": "rq-q", "payload_sha256": "00", "session_id": SESSION,
                  "generation": 1, "at": jiff::Timestamp::now()},
        "blocking": true, "text": "Which input?", "state": "open",
        "wake": {"route": "none", "key": "qwake", "state": "pending"},
    }))
    .unwrap();
    let update = |t: &mut Task| {
        let state = core_agent::container_state_mut(t, container).unwrap();
        state.questions.push(question);
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();
    key
}

pub(super) fn messages(fx: &Fx, recipient: &Recipient, kind: &str) -> Vec<Message> {
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let all = inbox.messages().unwrap();
    all.into_iter().filter(|m| m.kind == kind).collect()
}

pub(super) fn classes(calls: &Value) -> Vec<(String, String)> {
    let calls = calls.as_array().unwrap();
    let pair = |c: &Value| {
        (
            c["tool"].as_str().unwrap().into(),
            c["class"].as_str().unwrap().into(),
        )
    };
    calls.iter().map(pair).collect()
}

pub(super) fn resume(fx: &Fx, container: &ContainerRef, replace: bool) -> Result<Value> {
    resume_container(&f::ctx_at(fx.root()), container, replace)
}

/// Gate pane commands, as opposed to the relaunch's `agent launch`.
pub(super) fn gate_runs(fx: &Fx) -> usize {
    let runs = fx.calls("pane run");
    runs.iter().filter(|c| !c.contains("agent launch")).count()
}

pub(super) fn in_generation(
    fx: &Fx,
    host: &ts::Host,
    generation: u32,
    call: &str,
) -> RequestOrigin {
    let mut origin = fx.origin(host, call, "gate");
    origin.identity.generation = generation;
    origin.binding.generation = generation;
    origin
}

#[test]
fn a_killed_host_is_lost_once_and_its_run_question_and_queued_answer_stay() {
    // Given a healthy bound host with an open question:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    fx.bind(&fx.worker_target(), "w0:p1", &host);
    let bridge = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    let key = ask(&fx, &fx.worker());
    assert_eq!(sync(&fx).attention, None);
    assert_eq!(state(&fx, &fx.worker()).generation(1).unwrap().event_seq, 3);

    // When the host process is killed while its pane stays, its pane wrapper records the exit:
    host.kill();
    let exit = crate::launch_exit::receipt(&fx.ctx, &fx.id, "R1");
    std::fs::create_dir_all(exit.parent().unwrap()).unwrap();
    std::fs::write(&exit, "137\n").unwrap();
    let synced = sync(&fx);
    sync(&fx);

    // Then agent_lost is recorded once for generation 1 and the run shows resume, retry and
    // discard, while the run and its question stay live:
    let lost = fx.events("agent_lost");
    assert_eq!(lost.len(), 1);
    assert_eq!(
        (&lost[0].detail["reason"], &lost[0].detail["generation"]),
        (&json!("host_exited"), &json!(1))
    );
    let attention = synced.attention.unwrap();
    assert!(attention.starts_with(LOST_PREFIX), "{attention}");
    for verb in ["corral agent resume", "corral retry", "corral discard"] {
        assert!(attention.contains(verb), "{attention}");
    }
    let task = fx.task();
    assert_eq!(
        (
            task.runs[0].outcome,
            agent::container_closure(&task, &fx.worker())
        ),
        (None, None)
    );
    assert_eq!(state(&fx, &fx.worker()).binding.health, HostHealth::Lost);
    assert_eq!(
        state(&fx, &fx.worker()).questions[0].state,
        QuestionState::Open
    );
    assert!(
        lost[0].detail["detail"]
            .as_str()
            .unwrap()
            .contains("status 137")
    );
    assert!(
        !exit.exists(),
        "a bound host's exit is a loss, not a launch failure"
    );
    crate::sync_at(&fx.ctx, jiff::Timestamp::now()).unwrap();
    let task = fx.task();
    assert_eq!((task.status, task.runs[0].outcome), (Status::NeedsMe, None));
    let excerpt = task.runs[0].blocked_excerpt.clone().unwrap_or_default();
    assert!(excerpt.starts_with(LOST_PREFIX), "{excerpt}");

    // When the PM answers the open question, then the answer is queued and never sent:
    let revision = crate::progress_spec_revision(&task).unwrap();
    let answer = crate::questions::AnswerInput {
        text: "The first input.".into(),
        option: None,
        basis_refs: vec![format!("spec:{}@{revision}", fx.id)],
        scope_note: Some("inside the run's write scope".into()),
    };
    let pm = crate::questions::Actor::Pm(None);
    crate::questions::answer(&fx.ctx, &key, 1, pm, answer).unwrap();
    let _ = crate::delivery::owner::kick(&fx.ctx, &run_recipient(&fx));
    let question = state(&fx, &fx.worker()).questions[0].clone();
    assert_eq!(question.state, QuestionState::Answered);
    let id = question.answer.unwrap().message_id.unwrap();
    let inbox = Inbox::new(&fx.ctx.store, run_recipient(&fx)).unwrap();
    let seq: u64 = id.rsplit_once('#').unwrap().1.parse().unwrap();
    let level = corral_core::inbox::ReceiptLevel::Observed;
    assert!(inbox.receipt(seq, level).unwrap().is_none());
    assert!(inbox.terminal(seq).unwrap().is_none());
    assert!(
        inbox
            .envelopes()
            .unwrap()
            .iter()
            .all(|e| e.attempts.is_empty())
    );
    let sent = bridge.commands();
    assert!(
        sent.iter().all(|f| f["command"] != "submit_input"),
        "{sent:?}"
    );
}

#[test]
fn resume_fences_the_dead_generation_classifies_its_dangling_calls_and_relaunches_in_the_same_pane()
{
    // Given bun-reproof's dangling session (corral_gate and edit in flight) plus a later message
    // whose calls cover every other class, one of them with a result:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let control = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    let calls = [
        ("c-progress", "corral_progress"),
        ("c-evidence", "corral_evidence"),
        ("c-done", "corral_done"),
        ("c-read", "read"),
        ("c-bash", "bash"),
        ("c-ls", "ls"),
    ];
    let session = f::dangling_fixture() + &f::session(&calls, &["c-ls"]);
    f::write_session(&bound, &session);
    let stored = [
        receipt(
            &bound,
            "c-progress",
            "progress",
            "committed",
            json!({"outcome": "saved"}),
        ),
        receipt(
            &bound,
            "c-evidence",
            "evidence",
            "rejected",
            json!({"code": "bad_request"}),
        ),
    ];
    let update = |t: &mut Task| {
        let state = core_agent::container_state_mut(t, &fx.worker()).unwrap();
        state.receipts.extend(stored.clone());
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();
    std::fs::write(fx.app().join("tracked"), "changed\n").unwrap();
    let inbox = Inbox::new(&fx.ctx.store, run_recipient(&fx)).unwrap();
    let queued = inbox
        .post(corral_core::inbox::Post {
            kind: "owner_reply",
            urgency: corral_core::inbox::Urgency::Wake,
            author: "owner",
            body: "Also handle the empty input.",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
        })
        .unwrap();
    host.kill();

    // When the owner resumes the run:
    let outcome = resume(&fx, &fx.worker(), false).unwrap();

    // Then generation 1 is fenced once, its control files are gone, and generation 2 opens in
    // the same pane with an armed launch command and nothing replayed:
    assert_eq!(
        (outcome["fenced"].as_u64(), outcome["generation"].as_u64()),
        (Some(1), Some(2))
    );
    let after = state(&fx, &fx.worker());
    let fence = after.generation(1).unwrap().fenced.clone().unwrap();
    assert_eq!(fence.reason, "resume");
    assert_eq!(
        (after.binding.generation, after.binding.health),
        (2, HostHealth::Unbound)
    );
    assert_eq!(after.generation(2).unwrap().pane, "w0:p1");
    assert_eq!(fx.events("agent_fenced").len(), 1);
    assert!(!control.dir.join("ctl.cap").exists() && !control.dir.join("host.sock").exists());
    let g2 = generation_dir(&fx.ctx.store, &run_recipient(&fx), 2).unwrap();
    assert!(g2.join("bootstrap.json").is_file());
    let runs = fx.calls("pane run");
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0].contains("agent launch --bootstrap") && runs[0].contains("corral-launch"));

    // And the one notice classifies each dangling call, oldest first:
    let notices = messages(&fx, &run_recipient(&fx), NOTICE_KIND);
    assert_eq!(notices.len(), 1);
    let subject = notices[0].subject.clone().unwrap();
    assert_eq!(
        (subject["lost"].as_u64(), subject["generation"].as_u64()),
        (Some(1), Some(2))
    );
    let expected = [
        ("corral_gate", "not_executed"),
        ("edit", "outcome_unknown"),
        ("corral_progress", "result"),
        ("corral_evidence", "rejected"),
        ("corral_done", "not_committed"),
        ("read", "interrupted_no_effect"),
        ("bash", "outcome_unknown"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(t, c)| ((*t).into(), (*c).into()))
        .collect();
    assert_eq!(classes(&subject["calls"]), expected);
    let detail = |i: usize| {
        subject["calls"][i]["detail"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    assert!(detail(1).contains("artifact"), "{}", detail(1));
    assert!(detail(4).contains("commit_all"), "{}", detail(4));
    assert!(
        detail(6).contains("processes it started may still run"),
        "{}",
        detail(6)
    );
    let gate_request =
        crate::agent::tool_request_id(SESSION, "call_corral_gate_1|fc_fake_1_call_corral_gate_1");
    assert_eq!(subject["calls"][0]["request_id"], json!(gate_request));

    // And the edit's evidence is a worktree artifact, and the Pi session was never rewritten:
    let task = fx.task();
    let title = format!(
        "Worktree after host loss ({} generation 1)",
        fx.worker().key()
    );
    let artifact = task.artifacts.iter().find(|a| a.title == title).unwrap();
    let text = std::fs::read_to_string(fx.ctx.store.dir().join(&artifact.path)).unwrap();
    assert!(text.contains("--porcelain=v2") && text.contains(".M") && text.contains("tracked"));
    assert_eq!(
        std::fs::read_to_string(f::session_file(&bound)).unwrap(),
        session
    );
    assert_eq!(fx.events("agent_resumed").len(), 1);
    let reconciled = fx.events("agent_call_reconciled");
    let reported: Vec<&str> = reconciled
        .iter()
        .filter_map(|e| e.detail["class"].as_str())
        .collect();
    assert_eq!(
        reported,
        ["not_executed", "result", "rejected", "not_committed"]
    );

    // And the notice is published ahead of the message queued while the host was lost:
    let binding = corral_core::inbox::SessionBinding {
        session: SESSION.into(),
        endpoint: Some(g2.join("host.sock")),
        transcript: Some(f::session_file(&bound)),
    };
    let envelope = inbox
        .publish(&binding, "pi_native", 65536, true)
        .unwrap()
        .unwrap();
    assert_eq!(envelope.members, [notices[0].seq, queued.seq]);
    let payload = inbox.payload(&envelope).unwrap();
    let notice_at = payload.find("recovery_notice").unwrap();
    assert!(notice_at < payload.find("Also handle the empty input.").unwrap());

    // And nothing was replayed: no gate ran.
    assert_eq!(gate_runs(&fx), 0);
}

#[derive(Clone, Copy, Debug)]
enum Window {
    /// Held at the barrier while resume fences, then released into the submission boundary.
    Released,
    /// Released after the fence, then killed after its durable refusal, before its receipt.
    KilledAfterRefusal,
    /// Killed at the barrier before the check.
    KilledBeforeCheck,
    /// Still held when the drain deadline passes.
    OutlivesDrain,
}

#[test]
fn a_gate_interrupted_by_host_loss_is_reported_from_its_attempt_and_starts_once_on_adoption() {
    for window in [
        Window::Released,
        Window::KilledAfterRefusal,
        Window::KilledBeforeCheck,
        Window::OutlivesDrain,
    ] {
        // Given a gate executor of generation 1 held at gate_submit_before_lock and a dead host:
        let drain = if matches!(window, Window::OutlivesDrain) {
            1
        } else {
            60
        };
        let fx = f::fixture(true, drain);
        let mut host = HostProc::spawn(true);
        let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
        let session = f::session(&[("c-gate", "corral_gate")], &[]);
        f::write_session(&bound, &session);
        let gate = fx.origin(&bound, "c-gate", "gate");
        let points = match window {
            Window::KilledAfterRefusal => "gate_submit_before_lock,gate_refused_before_receipt",
            _ => "gate_submit_before_lock",
        };
        let mut exec = Some(Exec::spawn(&fx, &gate, points));
        exec.as_ref().unwrap().reached("gate_submit_before_lock");
        if matches!(window, Window::KilledBeforeCheck) {
            exec.take().unwrap().kill();
        }
        host.kill();

        // When resume runs, the executor is released or killed once the fence committed:
        let (fenced, signal) = mpsc::channel();
        let (root, container) = (fx.root().to_path_buf(), fx.worker());
        let resumer = std::thread::spawn(move || {
            let hook = move |point: &str| {
                if point == "resume_fenced" {
                    let _ = fenced.send(());
                }
            };
            crate::agent::receipts::FAILPOINT.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
            resume_container(&f::ctx_at(&root), &container, false)
        });
        signal.recv_timeout(WAIT).unwrap();
        match window {
            Window::Released => {
                exec.as_mut().unwrap().release();
            }
            Window::KilledAfterRefusal => {
                let mut killed = exec.take().unwrap();
                killed.release().reached("gate_refused_before_receipt");
                killed.kill();
            }
            Window::KilledBeforeCheck | Window::OutlivesDrain => {}
        }
        let outcome = resumer.join().unwrap().unwrap();
        let case = format!("{window:?}");

        // Then the notice reports the gate per its attempt state, and nothing was submitted:
        let class = outcome["calls"][0]["class"].as_str().unwrap().to_owned();
        let detail = outcome["calls"][0]["detail"].as_str().unwrap().to_owned();
        let stored = || {
            state(&fx, &fx.worker())
                .receipt(&gate.request_id)
                .cloned()
                .unwrap()
        };
        match window {
            Window::Released | Window::KilledAfterRefusal => {
                assert_eq!(class, "rejected", "{case}");
                assert!(detail.contains("submission_refused") && detail.contains("agent_fenced"));
            }
            Window::KilledBeforeCheck => {
                assert_eq!(class, "prepared", "{case}: {detail}");
                let settled = stored();
                assert_eq!(settled.state, ReceiptState::Abandoned, "{case}");
                assert_eq!(settled.result["outcome"], "not_submitted");
                assert_eq!(settled.result["binding"], "agent_fenced");
                let wakes = state(&fx, &fx.worker()).wakes;
                assert_eq!(wakes[0].state, WakeState::Cancelled, "{case}");
            }
            Window::OutlivesDrain => {
                assert_eq!(class, "in_progress", "{case}");
                assert_eq!(outcome["draining"], json!([gate.request_id]));
                exec.as_mut().unwrap().release();
            }
        }
        if let Some(exec) = exec {
            assert!(exec.wait(), "{case}: the refused executor exits cleanly");
        }
        assert_eq!(gate_runs(&fx), 0, "{case}");

        // And an executor that outlived the drain gets exactly one recovery_update:
        if matches!(window, Window::OutlivesDrain) {
            assert_eq!(stored().state, ReceiptState::Rejected);
            assert_eq!(post_updates(&fx.ctx, &fx.id).unwrap(), 1);
            assert_eq!(post_updates(&fx.ctx, &fx.id).unwrap(), 0);
            let updates = messages(&fx, &run_recipient(&fx), UPDATE_KIND);
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].subject.as_ref().unwrap()["class"], "rejected");
        }

        // And generation 2's first corral_gate adopts the retained attempt and starts it once:
        let next = in_generation(&fx, &bound, 2, "c-gate-next");
        let started = effects::gate(&f::ctx_at(fx.root()), &next).unwrap();
        let started = started.receipt().unwrap();
        assert_eq!(started.state, ReceiptState::Committed, "{case}");
        assert_eq!(started.result["adopted"], true, "{case}");
        let own = effects::own_attempt(&gate.request_id);
        assert_eq!(
            started.result["attempt"].as_str(),
            Some(own.as_str()),
            "{case}"
        );
        assert_eq!(gate_runs(&fx), 1, "{case}");
    }
}

#[test]
fn a_lost_run_then_discard_cancels_its_questions_settles_its_reservations_and_keeps_a_transcript() {
    // Given a lost run with an open question and a prepared gate whose executor was killed:
    let fx = f::fixture(true, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let control = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    f::write_session(&bound, &f::session(&[("c-gate", "corral_gate")], &[]));
    ask(&fx, &fx.worker());
    let gate = fx.origin(&bound, "c-gate", "gate");
    let exec = Exec::spawn(&fx, &gate, "gate_submit_before_lock");
    exec.reached("gate_submit_before_lock");
    exec.kill();
    host.kill();
    sync(&fx);
    let reserved = state(&fx, &fx.worker())
        .receipt(&gate.request_id)
        .cloned()
        .unwrap();
    assert_eq!(
        reserved.state,
        ReceiptState::Reserved,
        "a live generation keeps it prepared"
    );

    // When the owner discards the run:
    let discard = |t: &mut Task| {
        t.runs[0].outcome = Some(RunOutcome::Discarded);
        t.status = Status::Dropped;
        Ok(())
    };
    fx.ctx.store.update(&fx.id, discard).unwrap();
    sync(&fx);

    // Then its questions are cancelled run_ended, no receipt stays reserved, the runtime
    // directory is gone and the session stays as a terminal transcript:
    let after = state(&fx, &fx.worker());
    let question = &after.questions[0];
    assert_eq!(question.state, QuestionState::Cancelled);
    let terminal = serde_json::to_value(question.terminal.as_ref().unwrap()).unwrap();
    assert!(terminal.to_string().contains("run_ended"), "{terminal}");
    assert!(
        after
            .receipts
            .iter()
            .all(|r| r.state != ReceiptState::Reserved)
    );
    let settled = after.receipt(&gate.request_id).unwrap();
    assert_eq!(settled.result["binding"], "run_ended");
    assert!(!control.dir.exists());
    assert!(f::session_file(&bound).is_file());
    let reclassified = fx.events("agent_session_reclassified");
    assert_eq!(reclassified.len(), 1);
    assert_eq!(reclassified[0].detail["class"], "terminal_transcript");
    assert_eq!(
        reclassified[0].detail["sessions"],
        json!([format!("{SESSION}.jsonl")])
    );
    sync(&fx);
    assert_eq!(
        fx.events("agent_session_reclassified").len(),
        1,
        "settled once"
    );
}

#[test]
fn a_lost_reviewer_resumes_its_own_review_child_and_a_gone_pane_is_left_to_process_lost() {
    // Given a review child whose host died:
    let fx = f::fixture(false, 5);
    let target = f::reviewing(&fx);
    let mut host = HostProc::spawn(true);
    fx.bind(&target, "w0:p2", &host);
    host.kill();
    let child = target.container();

    // When its pane is gone, resume refuses and fences nothing:
    std::fs::rename(fx.root().join("w0:p2"), fx.root().join("gone")).unwrap();
    let error = resume(&fx, &child, false).unwrap_err().to_string();
    assert!(error.starts_with("pane_gone"), "{error}");
    assert!(state(&fx, &child).generation(1).unwrap().fenced.is_none());

    // When the pane stays, resume starts generation 2 of the same child with its callback:
    std::fs::rename(fx.root().join("gone"), fx.root().join("w0:p2")).unwrap();
    resume(&fx, &child, false).unwrap();
    let after = state(&fx, &child);
    assert_eq!(
        (
            after.binding.generation,
            after.generation(2).unwrap().pane.as_str()
        ),
        (2, "w0:p2")
    );
    let review = Recipient::ReviewRun {
        task: fx.id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let dir = generation_dir(&fx.ctx.store, &review, 2).unwrap();
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(dir.join("bootstrap.json")).unwrap()).unwrap();
    assert_eq!(
        (&bootstrap["review"], &bootstrap["callback"]),
        (&json!("R1-review1"), &json!("g1:c1"))
    );
    assert_eq!(messages(&fx, &review, NOTICE_KIND).len(), 1);
    assert_eq!(fx.task().status, Status::Review);
}

#[test]
fn a_live_host_without_ctl_cap_is_lost_and_replace_signals_only_a_verified_host() {
    for like_host in [false, true] {
        // Given a live host whose generation lost its ctl.cap:
        let fx = f::fixture(false, 5);
        let mut host = HostProc::spawn(like_host);
        fx.bind(&fx.worker_target(), "w0:p1", &host);
        f::bridge(&fx.ctx.store, &run_recipient(&fx), 1, None, f::healthy);
        sync(&fx);
        let lost = fx.events("agent_lost");
        assert_eq!(lost[0].detail["reason"], "control_credential_missing");

        // When resume runs without --replace, it refuses and fences nothing:
        let error = resume(&fx, &fx.worker(), false).unwrap_err().to_string();
        assert!(error.starts_with("host_alive"), "{error}");
        assert!(
            state(&fx, &fx.worker())
                .generation(1)
                .unwrap()
                .fenced
                .is_none()
        );

        // When it runs with --replace, only a process proven to be the Bun host is signalled:
        let replaced = resume(&fx, &fx.worker(), true);
        if like_host {
            assert_eq!(replaced.unwrap()["replaced"], true);
            assert!(host.reaped(), "the verified host was stopped");
            assert_eq!(state(&fx, &fx.worker()).binding.generation, 2);
        } else {
            let error = replaced.unwrap_err().to_string();
            assert!(error.starts_with("host_identity_unverified"), "{error}");
            assert!(!host.reaped(), "an unverified process is never signalled");
            assert!(
                state(&fx, &fx.worker())
                    .generation(1)
                    .unwrap()
                    .fenced
                    .is_none()
            );
        }
    }
}

pub(super) fn event(fx: &Fx, host: &ts::Host, seq: u64, payload: Value) {
    let id = crate::agent::event_request_id(host.identity.generation, seq);
    let input = ts::envelope(host, "event", &id, payload, false);
    ts::committed(&crate::agent::call(&fx.ctx, "event", &input).unwrap());
}

#[test]
fn a_provider_auth_failure_goes_to_the_owner_until_another_turn_starts() {
    // Given a healthy host whose turn failed provider authentication:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    let origin = json!({"kind": "bridge", "idempotency_key": "k1"});
    event(
        &fx,
        &bound,
        1,
        json!({"kind": "turn_started", "turn": 1, "origin": origin}),
    );
    event(
        &fx,
        &bound,
        2,
        json!({"kind": "provider_error", "turn": 1, "class": "auth"}),
    );
    event(&fx, &bound, 3, json!({"kind": "turn_ended", "turn": 1}));

    // When sync runs, then the owner is asked to log in and no paid fallback is offered:
    let attention = sync(&fx).attention.unwrap();
    assert!(attention.starts_with(AUTH_PREFIX), "{attention}");
    assert!(attention.contains("corral agent login") && attention.contains("no paid fallback"));
    assert_eq!(fx.events("agent_provider_error").len(), 1);
    assert!(messages(&fx, &run_recipient(&fx), "turn_nudge").is_empty());

    // When the owner's reply starts another turn, then the hold clears:
    let origin = json!({"kind": "bridge", "idempotency_key": "k2"});
    event(
        &fx,
        &bound,
        4,
        json!({"kind": "turn_started", "turn": 2, "origin": origin}),
    );
    assert_eq!(sync(&fx).attention, None);
}

#[test]
fn a_transient_provider_failure_retries_on_provider_stall_backoff_then_escalates() {
    // Given a healthy host whose turn failed on provider overload:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let origin = json!({"kind": "bridge", "idempotency_key": "k1"});
    event(
        &fx,
        &bound,
        1,
        json!({"kind": "turn_started", "turn": 1, "origin": origin}),
    );
    event(
        &fx,
        &bound,
        2,
        json!({"kind": "provider_error", "turn": 1, "class": "overload"}),
    );
    event(&fx, &bound, 3, json!({"kind": "turn_ended", "turn": 1}));
    let ended = state(&fx, &fx.worker()).turns[0].ended.unwrap();
    let key = crate::provider_stall::agent_key(&fx.id, &fx.task().runs[0]);
    let tick = |secs: i64| {
        let task = fx.task();
        let at = ended + jiff::SignedDuration::from_secs(secs);
        crate::agent::hooks::stall::advance_agent(&fx.ctx, &task, &task.runs[0], at).unwrap()
    };
    let nudges = || messages(&fx, &run_recipient(&fx), "turn_nudge");

    // When provider_stall's backoff elapses, each due retry is one turn_nudge and nothing earlier:
    assert_eq!(tick(10), None);
    assert!(crate::provider_stall::retrying(&fx.ctx, &key) && nudges().is_empty());
    for (at, posted) in [(61, 1), (61, 1), (182, 2), (483, 3)] {
        assert_eq!(tick(at), None);
        assert_eq!(nudges().len(), posted, "at +{at}s");
    }
    let subject = nudges()[0].subject.clone().unwrap();
    assert!(
        subject["key"]
            .as_str()
            .unwrap()
            .starts_with("provider-retry:")
    );
    assert_eq!(fx.events("provider_retry").len(), 3);

    // Then, with the retries used up, the task goes to the owner with the real reason:
    let change = tick(784).unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert!(crate::provider_stall::escalated(&fx.ctx, &key));
    let excerpt = fx.task().runs[0].blocked_excerpt.clone().unwrap();
    assert!(
        excerpt.contains("provider overload in turn 1/1"),
        "{excerpt}"
    );

    // When a later turn starts, it is no recovery while it runs; once it ends without a provider
    // error, the stall is recovered:
    let origin = json!({"kind": "bridge", "idempotency_key": "k2"});
    event(
        &fx,
        &bound,
        4,
        json!({"kind": "turn_started", "turn": 2, "origin": origin}),
    );
    assert_eq!(tick(800), None);
    assert!(fx.events("provider_recovered").is_empty());
    event(&fx, &bound, 5, json!({"kind": "turn_ended", "turn": 2}));
    assert_eq!(tick(810), None);
    assert!(!crate::provider_stall::retrying(&fx.ctx, &key));
    assert_eq!(fx.events("provider_recovered").len(), 1);
}

#[test]
fn a_turn_cancel_reaches_the_live_host_without_fencing_and_a_held_gate_still_starts_once() {
    // Given a healthy host, an open question and a gate executor held at the barrier:
    let fx = f::fixture(true, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let bridge = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    ask(&fx, &fx.worker());
    let gate = fx.origin(&bound, "c-gate", "gate");
    let mut exec = Exec::spawn(&fx, &gate, "gate_submit_before_lock");
    exec.reached("gate_submit_before_lock");
    let origin = json!({"kind": "bridge", "idempotency_key": "k2"});
    event(
        &fx,
        &bound,
        1,
        json!({"kind": "turn_started", "turn": 2, "origin": origin}),
    );

    // When the owner cancels turn 2:
    let target = AgentTarget::Task {
        task: fx.id.clone(),
        run: None,
        review: None,
    };
    let args = CancelArgs {
        target: target.clone(),
        turn: Some(2),
    };
    cancel(&fx.ctx, &args).unwrap();

    // Then the host got the cancel, nothing was fenced, and the run and question stay live:
    let sent: Vec<Value> = bridge.commands();
    let cancels: Vec<&Value> = sent.iter().filter(|f| f["command"] == "cancel").collect();
    assert_eq!(cancels.len(), 1);
    assert_eq!(cancels[0]["args"]["turn"], 2);
    let after = state(&fx, &fx.worker());
    assert!(after.generation(1).unwrap().fenced.is_none());
    assert_eq!(after.binding.health, HostHealth::Healthy);
    assert_eq!(after.questions[0].state, QuestionState::Open);

    // And the turn the host then ends is classified cancelled and gets no nudge:
    event(&fx, &bound, 2, json!({"kind": "turn_ended", "turn": 2}));
    let turn = state(&fx, &fx.worker()).turns[0].clone();
    assert_eq!(turn.class, Some(corral_core::agent::TurnClass::Cancelled));
    sync(&fx);
    assert!(messages(&fx, &run_recipient(&fx), "turn_nudge").is_empty());

    // And the held gate, released, is submitted exactly once:
    exec.release();
    assert!(exec.wait());
    let started = state(&fx, &fx.worker())
        .receipt(&gate.request_id)
        .cloned()
        .unwrap();
    assert_eq!(started.state, ReceiptState::Committed);
    assert_eq!(gate_runs(&fx), 1);

    // When the host is lost, a cancel is refused instead of guessed:
    host.kill();
    sync(&fx);
    let error = cancel(&fx.ctx, &CancelArgs { target, turn: None }).unwrap_err();
    assert!(error.to_string().starts_with("host_lost"), "{error}");
}

#[test]
fn pm_resume_classifies_killed_capture_and_legacy_reply_executors_without_replay() {
    use crate::agent::hooks::pm::tests::{fixture, kill_at};
    for (op, point, class) in [
        ("capture", "pm_reserved_before_effect", "not_executed"),
        ("capture", "pm_effect_before_receipt", "result"),
        ("reply", "pm_reserved_before_effect", "outcome_unknown"),
        ("reply", "pm_effect_before_receipt", "outcome_unknown"),
    ] {
        // Given a real PM operation killed before or after its durable effect, then host loss.
        let fx = fixture();
        let mut host = HostProc::spawn(true);
        let target = BindTarget::Pm {
            project: "corral".into(),
        };
        let bound = fx.bind(&target, "pm-pane", &host);
        if op == "reply" {
            fx.ctx
                .store
                .update(&fx.id, |task| {
                    task.runs[0].delivery = corral_core::inbox::DeliveryMode::Legacy;
                    Ok(())
                })
                .unwrap();
        }
        f::write_session(
            &bound,
            &f::session(&[("pm-call", &format!("corral_{op}"))], &[]),
        );
        kill_at(&fx, &bound, op, point);
        host.kill();
        // When recovery fences, drains and reconciles the old generation.
        let outcome = resume(&fx, &ContainerRef::Pm, false).unwrap();
        // Then its real marker determines the class; no capture or reply is replayed.
        assert_eq!(
            classes(&outcome["calls"]),
            vec![(format!("corral_{op}"), class.into())]
        );
        let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
        assert!(pm.state.generation(1).unwrap().fenced.is_some());
        assert_eq!(messages(&fx, &Recipient::Pm, NOTICE_KIND).len(), 1);
        assert_eq!(fx.calls("pane run").len(), 1);
        let captures = fx
            .ctx
            .store
            .list()
            .unwrap()
            .into_iter()
            .filter(|t| t.agent_origin.is_some())
            .count();
        assert_eq!(
            captures,
            usize::from(op == "capture" && point == "pm_effect_before_receipt")
        );
    }
}
