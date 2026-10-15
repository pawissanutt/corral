//! Explicit obligations use the same fake herdr, fixed clock and injected process seam as T-033.
use corral_core::{
    DecisionRequest, GateRecord, GateResult, ObligationSource, ReceiptOutcome, ResumeDelivery,
    ResumeReceipt, Run, Status, WaitHandle, WaitRequest, WaitState,
};
use corral_ops::{Ctx, ResumeTarget, register_decision, register_wait, resume_run};
use serde_json::json;

use super::Fx;

fn wait(fx: &Fx) -> WaitRequest {
    WaitRequest {
        version: 1,
        id: "event".into(),
        run: "R1".into(),
        condition: "CI result is available".into(),
        owner: "fixture".into(),
        source: ObligationSource::Worker,
        deadline: Some(fx.at(600)),
        handle: WaitHandle::Channel {
            channel_id: "fixture:ci-1".into(),
            capability: Some("wait_v1".into()),
        },
    }
}

fn decision() -> DecisionRequest {
    DecisionRequest {
        version: 1,
        id: "ship".into(),
        run: "R1".into(),
        question: "Which migration should ship?".into(),
        requested_action: "Choose migration A or B".into(),
        owner: "owner".into(),
        source: ObligationSource::Worker,
        deadline: None,
    }
}

fn receipt(fx: &Fx, delivery: ResumeDelivery) -> ResumeReceipt {
    ResumeReceipt {
        version: 1,
        id: "ci-1".into(),
        run: "R1".into(),
        outcome: ReceiptOutcome::Ready,
        delivery,
        observed_at: fx.at(20),
        answer: None,
        data: json!({"exit_code":0}),
    }
}

fn state(task: &corral_core::Task) -> WaitState {
    task.current_run().unwrap().obligations.waits[0]
        .observation
        .state
}

#[test]
fn pending_wait_is_not_a_question_even_when_terminal_reports_blocked() {
    // Given a bounded channel wait and a misleading terminal status.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", wait(&fx), fx.at(0)).unwrap();
    fx.herdr_says("blocked");
    // When the idle debounce has elapsed.
    let task = fx.sync(400);
    // Then the named condition remains a wait, not an unanswered question.
    assert_eq!(task.status, Status::Running);
    assert_eq!(state(&task), WaitState::Pending);
    assert!(!task.current_run().unwrap().idle_no_report);
    assert_eq!(fx.events("needs_me"), 0);
}

#[test]
fn exact_owner_decision_survives_idle_working_and_sync_restart() {
    // Given an explicit owner question.
    let fx = Fx::new("fake");
    let question = decision();
    register_decision(&fx.ctx, "T-001", question.clone(), fx.at(0)).unwrap();
    // When terminal heuristics alternate across a fresh context.
    let ctx = Ctx {
        store: corral_core::Store::open(fx.ctx.store.root()).unwrap(),
        herdr: fx.ctx.herdr.clone(),
    };
    for (i, status) in ["idle", "working", "blocked"].iter().enumerate() {
        fx.herdr_says(status);
        corral_ops::sync_at(&ctx, fx.at(i64::try_from(i).unwrap() + 1)).unwrap();
    }
    // Then exact structured question/action remain authoritative, with one notification.
    let task = fx.ctx.store.get("T-001").unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    let pending = task
        .current_run()
        .unwrap()
        .obligations
        .pending_decision()
        .unwrap();
    assert_eq!(pending.request, question);
    assert_eq!(fx.events("needs_me"), 1);
    assert_eq!(fx.notifications().lines().count(), 1);
}

#[test]
fn answered_decision_keeps_answer_and_does_not_send_or_launch() {
    // Given a decision and an answer from a generic worker/owner channel.
    let fx = Fx::new("fake");
    register_decision(&fx.ctx, "T-001", decision(), fx.at(0)).unwrap();
    let mut answer = receipt(&fx, ResumeDelivery::Acknowledged);
    answer.outcome = ReceiptOutcome::Answered;
    answer.answer = Some("Migration B".into());
    // When the answer is persisted twice.
    resume_run(
        &fx.ctx,
        "T-001",
        ResumeTarget::Decision("ship"),
        answer.clone(),
        fx.at(20),
    )
    .unwrap();
    let task = resume_run(
        &fx.ctx,
        "T-001",
        ResumeTarget::Decision("ship"),
        answer.clone(),
        fx.at(30),
    )
    .unwrap();
    // Then one terminal answer is available for resume with no process input.
    assert_eq!(task.status, Status::Running);
    assert_eq!(
        task.current_run().unwrap().obligations.decisions[0].receipt,
        Some(answer)
    );
    assert_eq!(fx.events("run_resumed"), 1);
    assert!(!fx.bin_dir.join("replies.log").exists());
}

#[test]
fn duplicate_receipt_remains_single_after_restart_and_conflict_is_rejected() {
    // Given a ready receipt already persisted.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", wait(&fx), fx.at(0)).unwrap();
    let ready = receipt(&fx, ResumeDelivery::Acknowledged);
    resume_run(
        &fx.ctx,
        "T-001",
        ResumeTarget::Wait("event"),
        ready.clone(),
        fx.at(20),
    )
    .unwrap();
    let ctx = Ctx {
        store: corral_core::Store::open(fx.ctx.store.root()).unwrap(),
        herdr: fx.ctx.herdr.clone(),
    };
    // When a restarted client repeats it, then offers a conflicting value.
    resume_run(
        &ctx,
        "T-001",
        ResumeTarget::Wait("event"),
        ready.clone(),
        fx.at(30),
    )
    .unwrap();
    let mut conflict = ready.clone();
    conflict.data = json!({"exit_code":2});
    assert!(
        resume_run(
            &ctx,
            "T-001",
            ResumeTarget::Wait("event"),
            conflict,
            fx.at(30)
        )
        .is_err()
    );
    // Then exactly one receipt/event remains.
    assert_eq!(fx.events("run_resumed"), 1);
    assert_eq!(
        fx.ctx.store.get("T-001").unwrap().runs[0].obligations.waits[0].receipt,
        Some(ready)
    );
}

#[test]
fn delivery_unknown_is_not_acknowledged_by_terminal_activity_or_restart() {
    // Given an explicit receipt whose delivery is unknown.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", wait(&fx), fx.at(0)).unwrap();
    let unknown = receipt(&fx, ResumeDelivery::Unknown);
    resume_run(
        &fx.ctx,
        "T-001",
        ResumeTarget::Wait("event"),
        unknown.clone(),
        fx.at(20),
    )
    .unwrap();
    // When normal sync resumes with a working terminal.
    let task = fx.sync(700);
    // Then uncertainty is durable and no command is resent.
    assert_eq!(state(&task), WaitState::DeliveryUnknown);
    assert_eq!(task.runs[0].obligations.waits[0].receipt, Some(unknown));
    assert!(!fx.bin_dir.join("replies.log").exists());
    assert_eq!(fx.events("run_resumed"), 1);
}

#[test]
fn expired_wait_notifies_once_and_does_not_mask_the_watchdog() {
    // Given a raw-idle foreign foreground process and a wait about to expire.
    let fx = Fx::new("fake");
    let processes = super::watchdog::Processes::default();
    super::watchdog::foreground(&fx, &processes, &[(42, &["grep", "-cvE", "pattern"])], 700);
    let mut request = wait(&fx);
    request.deadline = Some(fx.at(300));
    register_wait(&fx.ctx, "T-001", request, fx.at(0)).unwrap();
    fx.herdr_says("idle");
    corral_ops::sync_at_with_processes(&fx.ctx, fx.at(0), &processes).unwrap();
    // When deadline and watchdog thresholds expire, with repeated sync.
    corral_ops::sync_at_with_processes(&fx.ctx, fx.at(301), &processes).unwrap();
    corral_ops::sync_at_with_processes(&fx.ctx, fx.at(400), &processes).unwrap();
    // Then expiry and stuck are independent durable diagnostics; no process is terminated.
    let task = fx.ctx.store.get("T-001").unwrap();
    assert_eq!(state(&task), WaitState::Expired);
    assert!(task.runs[0].stuck.is_some());
    assert_eq!(fx.events("wait_transition"), 1);
    assert_eq!(fx.events("stuck"), 1);
    assert!(processes.killed.borrow().is_empty());
}

#[test]
fn unsupported_unverifiable_and_lost_handles_have_distinct_outcomes() {
    for (capability, channel_id, deadline, expected) in [
        (None, "ci", Some(600), WaitState::Unsupported),
        (Some("wait_v1"), "", Some(600), WaitState::Unverifiable),
        (Some("wait_v1"), "ci", None, WaitState::Unverifiable),
    ] {
        // Given a declaration that lacks a verifiable bounded channel contract.
        let fx = Fx::new("fake");
        let mut request = wait(&fx);
        request.deadline = deadline.map(|s| fx.at(s));
        request.handle = WaitHandle::Channel {
            channel_id: channel_id.into(),
            capability: capability.map(str::to_owned),
        };
        // When registration classifies it.
        let task = register_wait(&fx.ctx, "T-001", request, fx.at(0)).unwrap();
        // Then the diagnostic is explicit.
        assert_eq!(state(&task), expected);
    }
    // Given an external channel that reports its handle lost.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", wait(&fx), fx.at(0)).unwrap();
    let mut lost = receipt(&fx, ResumeDelivery::Acknowledged);
    lost.outcome = ReceiptOutcome::Lost;
    // When the explicit lost receipt arrives.
    let task = resume_run(
        &fx.ctx,
        "T-001",
        ResumeTarget::Wait("event"),
        lost,
        fx.at(20),
    )
    .unwrap();
    // Then loss is not an unanswered owner question.
    assert_eq!(state(&task), WaitState::Lost);
    assert_eq!(task.status, Status::Running);
}

#[test]
fn retry_rejects_old_run_receipt_without_altering_either_attempt() {
    // Given R1's wait retained in history and an open R2.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", wait(&fx), fx.at(0)).unwrap();
    fx.ctx
        .store
        .update("T-001", |t| {
            let mut next = t.runs[0].clone();
            next.id = "R2".into();
            next.obligations = Default::default();
            t.runs.push(next);
            Ok(())
        })
        .unwrap();
    let before = fx.ctx.store.get("T-001").unwrap();
    // When an R1 result arrives at the same task ID.
    let error = resume_run(
        &fx.ctx,
        "T-001",
        ResumeTarget::Wait("event"),
        receipt(&fx, ResumeDelivery::Acknowledged),
        fx.at(20),
    )
    .unwrap_err();
    // Then it is explicitly stale, and the newer run is untouched.
    assert!(error.to_string().starts_with("mismatched_run:"));
    assert_eq!(fx.ctx.store.get("T-001").unwrap(), before);
}

fn gate_wait(fx: &Fx) -> WaitRequest {
    std::fs::create_dir_all(&fx.worktree).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&fx.worktree)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }
    let identity = corral_ops::completion_check(&fx.ctx, "T-001", "")
        .unwrap()
        .identity
        .unwrap();
    let mut request = wait(fx);
    request.handle = WaitHandle::Gate {
        sha: identity.head.clone(),
        pane: "gate:p1".into(),
    };
    fx.ctx
        .store
        .update("T-001", |t| {
            t.runs[0].gate = Some(GateRecord {
                identity: Some(identity.clone()),
                commands: Vec::new(),
                log_sha256: None,
                result: GateResult::Running,
                failed_cmd: None,
                at: fx.at(0),
                sha: identity.head.clone(),
                pane: Some("gate:p1".into()),
            });
            Ok(())
        })
        .unwrap();
    request
}

#[test]
fn actual_gate_result_advances_once_after_sync_restart() {
    // Given a generic worker's gate wait and the actual corral result path.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", gate_wait(&fx), fx.at(0)).unwrap();
    let identity = corral_ops::completion_check(&fx.ctx, "T-001", "")
        .unwrap()
        .identity
        .unwrap();
    let paths = corral_ops::gate_paths(&fx.ctx, "T-001", "R1");
    std::fs::create_dir_all(paths.result.parent().unwrap()).unwrap();
    // When readiness is injected by publishing the actual result, then sync restarts.
    std::fs::write(&paths.log, "fixture gate receipt\n").unwrap();
    std::fs::write(paths.result.with_extension("owner"), "fixture-owner-token").unwrap();
    std::fs::write(
        &paths.result,
        json!({
            "schema_version":2, "owner_token":"fixture-owner-token",
            "identity": identity, "result":"pass", "exit_code":0, "commands":[]
        })
        .to_string(),
    )
    .unwrap();
    fx.sync(10);
    let ctx = Ctx {
        store: corral_core::Store::open(fx.ctx.store.root()).unwrap(),
        herdr: fx.ctx.herdr.clone(),
    };
    corral_ops::sync_at(&ctx, fx.at(20)).unwrap();
    // Then the gate and its wait each advance once, retaining delivery uncertainty.
    let task = fx.ctx.store.get("T-001").unwrap();
    let receipt = task.runs[0].obligations.waits[0].receipt.as_ref().unwrap();
    assert_eq!(receipt.outcome, ReceiptOutcome::Ready);
    assert_eq!(receipt.data["result_file"], paths.result.to_str().unwrap());
    assert_eq!(state(&task), WaitState::DeliveryUnknown);
    assert_eq!(fx.events("gate"), 1);
    assert_eq!(fx.events("wait_transition"), 2); // initial unobserved gate, then completion
}

#[test]
fn gate_loss_malformed_result_and_changed_invocation_are_diagnostics() {
    // Given a bound corral gate wait whose pane disappears.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", gate_wait(&fx), fx.at(0)).unwrap();
    // When sync cannot find the actual gate pane.
    assert_eq!(state(&fx.sync(10)), WaitState::Lost);
    // Then a worker assertion cannot invent a corral gate success.
    assert!(
        resume_run(
            &fx.ctx,
            "T-001",
            ResumeTarget::Wait("event"),
            receipt(&fx, ResumeDelivery::Acknowledged),
            fx.at(20)
        )
        .is_err()
    );
    let paths = corral_ops::gate_paths(&fx.ctx, "T-001", "R1");
    std::fs::create_dir_all(paths.result.parent().unwrap()).unwrap();
    std::fs::write(&paths.result, "invalid").unwrap();
    assert_eq!(state(&fx.sync(30)), WaitState::Unverifiable);
    fx.ctx
        .store
        .update("T-001", |t| {
            t.runs[0].gate.as_mut().unwrap().pane = Some("gate:p2".into());
            Ok(())
        })
        .unwrap();
    assert_eq!(state(&fx.sync(40)), WaitState::MismatchedRun);
}

#[test]
fn simultaneous_distinct_registrations_preserve_both_under_the_store_lock() {
    // Given two writers released by a barrier, not timing luck.
    let fx = Fx::new("fake");
    let barrier = std::sync::Barrier::new(2);
    // When each registers a different wait through the same locked store.
    std::thread::scope(|scope| {
        for id in ["first", "second"] {
            let barrier = &barrier;
            let ctx = &fx.ctx;
            let mut request = wait(&fx);
            request.id = id.into();
            let now = fx.at(0);
            scope.spawn(move || {
                barrier.wait();
                register_wait(ctx, "T-001", request, now).unwrap();
            });
        }
    });
    // Then neither writer replaces the other's record.
    assert_eq!(
        fx.ctx.store.get("T-001").unwrap().runs[0]
            .obligations
            .waits
            .len(),
        2
    );
}

#[test]
fn legacy_run_deserialization_keeps_obligations_optional() {
    // Given a run captured before the new protocol.
    let fx = Fx::new("fake");
    let mut json = serde_json::to_value(&fx.ctx.store.get("T-001").unwrap().runs[0]).unwrap();
    json.as_object_mut().unwrap().remove("obligations");
    // When it is read by this version.
    let run: Run = serde_json::from_value(json).unwrap();
    // Then it has no fabricated wait or question.
    assert!(run.obligations.is_empty());
}

#[test]
fn stale_sync_cannot_overwrite_a_new_explicit_decision() {
    // Given a decision that lands exactly when legacy sync reads the terminal excerpt.
    let fx = Fx::new("fake");
    let initial = fx.ctx.store.get("T-001").unwrap();
    register_decision(&fx.ctx, "T-001", decision(), fx.at(0)).unwrap();
    let newer = fx.ctx.store.get("T-001").unwrap();
    std::fs::write(
        fx.bin_dir.join("interleave"),
        corral_core::render_task(&newer),
    )
    .unwrap();
    std::fs::write(
        fx.bin_dir.join("interleave_to"),
        initial.path.to_str().unwrap(),
    )
    .unwrap();
    std::fs::write(&initial.path, corral_core::render_task(&initial)).unwrap();
    fx.herdr_says("blocked");
    // When sync's old snapshot reaches its conditional update.
    let task = fx.sync(10);
    // Then the newer canonical decision is not overwritten by the heuristic question.
    assert!(!fx.bin_dir.join("interleave").exists());
    assert_eq!(task.runs[0].obligations, newer.runs[0].obligations);
    assert_eq!(task.runs[0].blocked_excerpt, newer.runs[0].blocked_excerpt);
}

#[test]
fn expired_owner_decision_still_requires_its_exact_answer() {
    // Given an unanswered decision with a deadline.
    let fx = Fx::new("fake");
    let mut request = decision();
    request.deadline = Some(fx.at(10));
    register_decision(&fx.ctx, "T-001", request.clone(), fx.at(0)).unwrap();
    fx.herdr_says("working");
    // When its deadline passes.
    let task = fx.sync(10);
    // Then it stays an owner decision, not a wait or an inferred answer.
    assert_eq!(task.status, Status::NeedsMe);
    let record = &task.runs[0].obligations.decisions[0];
    assert_eq!(record.observation.state, WaitState::Expired);
    assert_eq!(record.request, request);
    assert!(record.receipt.is_none());
}

#[test]
fn failed_gate_produces_a_failed_receipt_not_ready() {
    // Given a generic worker's gate wait.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", gate_wait(&fx), fx.at(0)).unwrap();
    let paths = corral_ops::gate_paths(&fx.ctx, "T-001", "R1");
    std::fs::create_dir_all(paths.result.parent().unwrap()).unwrap();
    std::fs::write(&paths.failed, "false").unwrap();
    // When an actual failure result is injected.
    let identity = corral_ops::completion_check(&fx.ctx, "T-001", "")
        .unwrap()
        .identity
        .unwrap();
    std::fs::write(&paths.log, "fixture gate failure\n").unwrap();
    std::fs::write(paths.result.with_extension("owner"), "fixture-owner-token").unwrap();
    std::fs::write(
        &paths.result,
        json!({
            "schema_version":2, "owner_token":"fixture-owner-token",
            "identity":identity, "result":"fail", "exit_code":1, "commands":[]
        })
        .to_string(),
    )
    .unwrap();
    let task = fx.sync(20);
    // Then both records retain failure and the injected clock, not a green resume.
    let record = &task.runs[0].obligations.waits[0];
    assert_eq!(
        record.receipt.as_ref().unwrap().outcome,
        ReceiptOutcome::Failed
    );
    assert_eq!(record.receipt.as_ref().unwrap().observed_at, fx.at(20));
    assert_eq!(task.runs[0].gate.as_ref().unwrap().at, fx.at(20));
    assert_eq!(task.runs[0].failures, 1);
}

#[test]
fn unavailable_herdr_does_not_prevent_deadline_reconciliation() {
    // Given a pending external event and an unavailable terminal engine.
    let fx = Fx::new("fake");
    register_wait(&fx.ctx, "T-001", wait(&fx), fx.at(0)).unwrap();
    let ctx = Ctx {
        store: fx.ctx.store.clone(),
        herdr: corral_herdr::Herdr::with_bin(fx.bin_dir.join("absent"), None),
    };
    // When sync restarts after the declared deadline.
    corral_ops::sync_at(&ctx, fx.at(600)).unwrap();
    // Then expiration remains observable without launching or sending anything.
    assert_eq!(
        state(&fx.ctx.store.get("T-001").unwrap()),
        WaitState::Expired
    );
    assert!(!fx.bin_dir.join("replies.log").exists());
}

#[test]
fn invalid_versions_ids_answers_and_timestamps_do_not_write_partial_records() {
    // Given an active run and a valid owner decision.
    let fx = Fx::new("fake");
    let mut request = wait(&fx);
    request.version = 2;
    assert!(register_wait(&fx.ctx, "T-001", request, fx.at(0)).is_err());
    let mut request = wait(&fx);
    request.id = "../escape".into();
    assert!(register_wait(&fx.ctx, "T-001", request, fx.at(0)).is_err());
    register_decision(&fx.ctx, "T-001", decision(), fx.at(10)).unwrap();
    let before = fx.ctx.store.get("T-001").unwrap();
    // When empty answers and future observations are submitted.
    let mut answer = receipt(&fx, ResumeDelivery::Acknowledged);
    answer.outcome = ReceiptOutcome::Answered;
    assert!(
        resume_run(
            &fx.ctx,
            "T-001",
            ResumeTarget::Decision("ship"),
            answer.clone(),
            fx.at(20)
        )
        .is_err()
    );
    answer.answer = Some("B".into());
    assert!(
        resume_run(
            &fx.ctx,
            "T-001",
            ResumeTarget::Decision("ship"),
            answer,
            fx.at(19)
        )
        .is_err()
    );
    // Then neither a rejected registration nor a rejected answer is partially applied.
    assert_eq!(fx.ctx.store.get("T-001").unwrap(), before);
    assert!(before.runs[0].obligations.waits.is_empty());
}

#[test]
fn failed_obligation_notification_reaches_the_terminal_owners_sink() {
    // Given an installed board sink and an unavailable notification transport.
    let fx = Fx::new("fake");
    let ctx = Ctx {
        store: fx.ctx.store.clone(),
        herdr: corral_herdr::Herdr::with_bin(fx.bin_dir.join("absent"), None),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(tx);
    // When a decision transition cannot notify through herdr.
    register_decision(&ctx, "T-001", decision(), fx.at(0)).unwrap();
    // Then the terminal owner receives one task-keyed diagnostic, without a stderr fallback.
    let message = rx.try_recv().unwrap();
    assert_eq!(message.task.as_deref(), Some("T-001"));
    assert_eq!(message.source, "obligations");
    assert!(rx.try_recv().is_err());
}
