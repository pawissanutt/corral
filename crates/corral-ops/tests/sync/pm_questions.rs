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
