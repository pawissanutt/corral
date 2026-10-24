use corral_core::Status;
use corral_ops::{PmActionKind, pm_next};
use serde_json::json;

use super::{Fx, slug};

fn session(fx: &Fx, settled: bool) {
    let dir = fx
        .agent_dir
        .join("sessions")
        .join(format!("--{}--", slug(&fx.worktree)));
    std::fs::create_dir_all(&dir).unwrap();
    let mut entries = vec![
        json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"request_user_input","id":"complete|request-id","arguments":{"questions":[{"id":"scope","question":"Preserve guard or expand scope?","options":[{"label":"Preserve guard"},{"label":"Expand scope"}]}]}}]}}),
        json!({"type":"custom","customType":"ask-user:question","data":{"requestId":"complete|request-id","headers":["Scope"]}}),
        json!({"type":"message","message":{"role":"toolResult","toolCallId":"complete|request-id","content":[{"text":"Question accepted; answer will arrive later"}]}}),
    ];
    if settled {
        entries.push(json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":"complete|request-id","status":"comment-submitted"}}));
    }
    std::fs::write(
        dir.join("2026-10-15T09-00-00-000Z_native.jsonl"),
        entries.iter().map(|e| format!("{e}\n")).collect::<String>(),
    )
    .unwrap();
}

#[test]
fn native_pending_question_surfaces_needs_me_while_herdr_reports_working() {
    // Given a native question that is registered and acknowledged, but not settled.
    let fx = Fx::new("omo");
    session(&fx, false);
    fx.herdr_says("working");
    // When the board syncs and the stateless PM reads it.
    let task = fx.sync(1);
    let card = pm_next(&fx.ctx).unwrap();
    // Then both surfaces need the owner despite the working terminal.
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(
        task.current_run().unwrap().agent_state.as_deref(),
        Some("working")
    );
    assert_eq!(card.kind, PmActionKind::Question);
    assert!(card.requires_owner);
    assert_eq!(fx.events("needs_me"), 1);
}

#[test]
fn native_settlement_releases_question_without_waiting_for_terminal_idle() {
    // Given an answered native question and a still-working agent.
    let fx = Fx::new("omo");
    session(&fx, true);
    fx.herdr_says("working");
    // When the board syncs.
    let task = fx.sync(1);
    // Then no owner decision is invented from an already acknowledged tool call.
    assert_eq!(task.status, Status::Running);
    assert_eq!(fx.events("needs_me"), 0);
}

#[test]
fn timed_out_native_question_answered_later_does_not_hold_needs_me() {
    // Given the T-207 shape: accepted, settled timed_out, then an answer as a user message.
    let fx = Fx::new("omo");
    session(&fx, false);
    let path = fx
        .agent_dir
        .join("sessions")
        .join(format!("--{}--", slug(&fx.worktree)))
        .join("2026-10-15T09-00-00-000Z_native.jsonl");
    let mut transcript = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    for entry in [
        json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":"complete|request-id","status":"timed_out"}}),
        json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"[Answer to question complete|request-id]\nPreserve guard"}]}}),
    ] {
        use std::io::Write;
        writeln!(transcript, "{entry}").unwrap();
    }
    fx.herdr_says("working");
    // When the board syncs.
    let task = fx.sync(1);
    // Then the stale question neither holds Needs me nor blocks operating actions.
    assert_eq!(task.status, Status::Running);
    assert_eq!(fx.events("needs_me"), 0);
}

#[test]
fn done_task_shows_no_stale_native_question_card() {
    // Given a finished task whose transcript still ends on an unsettled native question.
    let fx = Fx::new("omo");
    session(&fx, false);
    fx.ctx
        .store
        .update("T-001", |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    // When the stateless PM reads the board.
    let card = pm_next(&fx.ctx).unwrap();
    // Then no question decision is shown for it.
    assert_ne!(card.kind, PmActionKind::Question);
}

fn presented_native_decision_survives_terminal_status(terminal: Status) {
    // Given a presented, persisted native-question decision on an unsettled transcript.
    let fx = Fx::new("omo");
    session(&fx, false);
    fx.herdr_says("working");
    fx.sync(1);
    let workset = corral_ops::pm_workset(&fx.ctx).unwrap();
    let native = workset
        .decisions
        .iter()
        .find(|d| d.source.contains(" request complete|request-id"))
        .unwrap()
        .id
        .clone();
    corral_ops::pm_present(&fx.ctx, &native, "owner-pane:native", true, fx.at(2)).unwrap();
    assert!(
        corral_ops::pm_workset(&fx.ctx)
            .unwrap()
            .decisions
            .iter()
            .any(|d| d.id == native)
    );
    // When the task becomes terminal while the transcript stays unsettled.
    fx.ctx
        .store
        .update("T-001", |task| {
            task.status = terminal;
            Ok(())
        })
        .unwrap();
    // Then the saved decision is not restored into the workset or its status.
    let after = corral_ops::pm_workset(&fx.ctx).unwrap();
    assert!(after.decisions.iter().all(|d| d.id != native));
    assert_ne!(after.status, corral_ops::OperatingStatus::NeedsYourDecision);
}

#[test]
fn presented_native_decision_is_dropped_when_the_task_is_done() {
    presented_native_decision_survives_terminal_status(Status::Done);
}

#[test]
fn presented_native_decision_is_dropped_when_the_task_is_dropped() {
    presented_native_decision_survives_terminal_status(Status::Dropped);
}
