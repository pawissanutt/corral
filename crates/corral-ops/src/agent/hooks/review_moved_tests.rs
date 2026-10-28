//! T-522 R2 F2: a pi-native reviewer whose verdict is refused only because sync attached an
//! integration packet for a cleanly moved main is relaunched once its turn ends, independently of
//! pane idleness, instead of being nudged and escalated. An open turn is never interrupted, and a
//! refusal after the candidate changed keeps the nudge-then-escalate path.
use std::collections::HashMap;

use corral_core::RunOutcome;
use corral_core::agent::ClosureReason;
use corral_herdr::{AgentStatus, PaneInfo};

use super::*;

/// A pi-native review of a candidate on branch `feature`, in its own worktree off `main`.
fn feature_review() -> Review {
    review_with(|ctx, id| {
        let app = ctx.store.root().join("app");
        let wt = ctx.store.root().join("wt");
        git(
            &app,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                wt.to_str().unwrap(),
            ],
        );
        std::fs::write(wt.join("widget.txt"), "widget\n").unwrap();
        git(&wt, &["add", "."]);
        git(&wt, &["commit", "-qm", "widget"]);
        ctx.store
            .update(id, |t| {
                t.runs[0].worktree = wt.clone();
                t.runs[0].branch = "feature".into();
                Ok(())
            })
            .unwrap();
    })
}

/// Another task lands an unrelated file on main; the candidate is untouched.
fn land_on_main(review: &Review) -> String {
    let app = review.ctx().store.root().join("app");
    std::fs::write(app.join("unrelated.txt"), "other task\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "other lands"]);
    git(&app, &["rev-parse", "main"])
}

/// What autopilot's clean preflight records for the moved main (`autopilot/repair.rs`).
fn attach_integration_packet(review: &Review) {
    let task = review.task();
    let run = &task.runs[0];
    let policy = review
        .ctx()
        .store
        .config
        .review_policy(task.project.as_deref(), None);
    let key = crate::integration::RepairKey::new(&task, run, "preflight_clean").unwrap();
    let packet = crate::integration::IntegrationPacket::build(
        review.ctx(),
        &task,
        run,
        &policy,
        key,
        vec![],
        None,
    )
    .unwrap();
    packet.attach(review.ctx()).unwrap();
}

fn approve(review: &Review, call: &str) -> CallResponse {
    review.op(
        Some(1),
        call,
        "verdict",
        json!({"verdict": "approve", "notes": "Looks right."}),
    )
}

/// The review watchdog with the reviewer's pane present and busy: a healthy pi-native host's
/// pane status is not a signal, so recovery must not depend on it.
fn watchdog(review: &Review) {
    let pane = review.host.pane.clone();
    let info = PaneInfo {
        pane_id: pane.clone(),
        workspace_id: "w1".into(),
        tab_id: "w1:t1".into(),
        agent: None,
        status: AgentStatus::Working,
        title: None,
        cwd: None,
    };
    let panes = HashMap::from([(pane, info)]);
    crate::reviewer::reconcile_review_at(review.ctx(), &review.id, jiff::Timestamp::now(), &panes)
        .unwrap();
}

/// One sync pass in sync's own order: the review watchdog, then the pi-native hook.
fn sync_pass(review: &Review) {
    watchdog(review);
    review.sync();
}

fn nudges(review: &Review) -> usize {
    let messages = review.inbox().messages().unwrap();
    messages.iter().filter(|m| m.kind == "turn_nudge").count()
}

#[test]
fn a_native_packet_refusal_after_main_moved_relaunches_the_review_once_the_turn_ends() {
    // Given a native reviewer mid-turn when main moves cleanly and sync records the packet.
    let mut review = feature_review();
    committed(&review.event("turn_started", 1));
    let main = land_on_main(&review);
    attach_integration_packet(&review);
    let (code, reason) = rejection(&approve(&review, "call-stale"));
    assert_eq!(code, "review_closed");
    assert!(reason.contains("packet_digest"), "{reason}");

    // While its turn is still open, nothing interrupts it.
    sync_pass(&review);
    assert!(review.task().runs[0].review.is_some());
    assert_eq!(ts::events(&review.fx, "review_relaunched"), 0);

    // When the turn ends without a verdict, the review is relaunched, not nudged or escalated.
    committed(&review.event("turn_ended", 1));
    assert_eq!(review.class(1), Some(TurnClass::NoVerdict));
    sync_pass(&review);
    let task = review.task();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.rounds, 0);
    assert!(task.runs[0].review.is_none());
    let old = task.runs[0].review_runs.last().unwrap();
    assert_eq!(old.id, review.child);
    assert_eq!(old.outcome, Some(RunOutcome::Superseded));
    assert_eq!(ts::events(&review.fx, "review_relaunched"), 1);
    assert_eq!(ts::events(&review.fx, "review_escalated"), 0);
    assert_eq!(nudges(&review), 0);
    assert_eq!(
        core_agent::container_closure(&task, &review.container()),
        Some(ClosureReason::ReviewSuperseded)
    );

    // And the next review binds the unchanged candidate to the moved main.
    let (child, _) = launch(&review.fx, &review.id);
    assert_ne!(child, review.child);
    let task = review.task();
    let record = task.runs[0].review.clone().unwrap();
    assert_eq!(record.identity.unwrap().integration_base, main);
    assert_eq!(task.rounds, 0);
    assert_eq!(task.status, Status::Review);
}

#[test]
fn a_native_refusal_after_the_candidate_changed_keeps_the_nudge_then_escalation() {
    let mut review = feature_review();
    committed(&review.event("turn_started", 1));
    land_on_main(&review);
    attach_integration_packet(&review);
    let wt = review.task().runs[0].worktree.clone();
    std::fs::write(wt.join("widget.txt"), "edited during review\n").unwrap();
    git(&wt, &["commit", "-qam", "edit during review"]);
    let (code, _) = rejection(&approve(&review, "call-stale"));
    assert_eq!(code, "review_closed");
    committed(&review.event("turn_ended", 1));

    sync_pass(&review);
    assert_eq!(ts::events(&review.fx, "review_relaunched"), 0);
    assert_eq!(nudges(&review), 1);
    assert!(review.task().runs[0].review.is_some());

    committed(&review.event("turn_started", 2));
    committed(&review.event("turn_ended", 2));
    sync_pass(&review);
    assert_eq!(review.task().status, Status::NeedsMe);
    assert_eq!(ts::events(&review.fx, "review_relaunched"), 0);
}
