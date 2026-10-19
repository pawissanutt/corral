use corral_core::{AutoStage, AutopilotState, RunOutcome, Status};
use corral_ops::Change;

use super::mouse_tests::{click, find, render};
use super::{WorkerMsg, change_toast};
use crate::board::tests::{key, seeded};
use crate::{Mode, Tone};

/// The kill switch as corral.toml on disk says.
fn autopilot_on(ctx: &corral_ops::Ctx) -> bool {
    corral_core::Store::open(ctx.store.root())
        .unwrap()
        .config
        .autopilot_enabled()
}

#[test]
fn header_autopilot_switch_toggles_by_click_and_by_key() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let text = render(&mut app);
    assert!(text.contains("autopilot: blocked (assist)"), "{text}");

    let at = find(&text, 1, "autopilot:");
    click(&mut app, at.0, at.1);
    assert!(!autopilot_on(&fx.ctx));
    let toml = std::fs::read_to_string(fx.ctx.store.config_path()).unwrap();
    assert!(toml.contains("autopilot = false"), "{toml}");
    let text = render(&mut app);
    assert!(text.contains("autopilot: blocked (assist)"), "{text}");
    assert!(
        app.toasts
            .iter()
            .any(|(t, _)| t.title == "Review switch disabled" && t.tone == Tone::Warn),
        "toggling off toasts"
    );

    app.handle_key(key('O'));
    assert!(autopilot_on(&fx.ctx));
    let text = render(&mut app);
    assert!(text.contains("autopilot: blocked (assist)"), "{text}");
    assert!(
        app.toasts
            .iter()
            .any(|(t, _)| t.title == "Review switch enabled"),
        "toggling on toasts"
    );
}

#[test]
fn header_switch_is_not_clickable_under_another_view() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.handle_key(key(','));
    assert_eq!(app.stack.len(), 2, "settings open");
    let text = render(&mut app);
    assert!(
        !app.hits
            .iter()
            .any(|(_, h)| matches!(h, crate::Hit::Key(k) if *k == key('O'))),
        "no header switch while settings are open"
    );
    let at = find(&text, 1, "autopilot:");
    click(&mut app, at.0, at.1);
    assert!(autopilot_on(&fx.ctx));
}

fn change(task: &str, to: Status, stage: AutoStage, note: &str) -> Change {
    Change {
        task: task.into(),
        from: Status::Review,
        to,
        note: note.into(),
        stage: Some(stage),
    }
}

#[test]
fn autopilot_toasts_name_the_outcome_and_reason() {
    let landed = change_toast(
        &change(
            "T-006",
            Status::Done,
            AutoStage::Done,
            "autopilot landed: main abc1234",
        ),
        None,
    )
    .unwrap();
    assert_eq!(landed.title, "✓ T-006 landed automatically");
    assert_eq!(landed.body, "autopilot landed: main abc1234");
    assert_eq!(landed.task.as_deref(), Some("T-006"));

    let escalated = change_toast(
        &change(
            "T-006",
            Status::NeedsMe,
            AutoStage::Escalated,
            "autopilot escalated",
        ),
        Some("the diff has 2000 changed lines".into()),
    )
    .unwrap();
    assert_eq!(escalated.title, "▲ T-006 escalated to you");
    assert_eq!(escalated.body, "the diff has 2000 changed lines");
    assert_eq!(escalated.tone, Tone::Attention);

    let blocked = change_toast(
        &change(
            "T-006",
            Status::Review,
            AutoStage::Blocked,
            "landing blocked",
        ),
        Some("main checkout is dirty".into()),
    )
    .unwrap();
    assert_eq!(blocked.title, "⛔ T-006 blocked");
    assert_eq!(blocked.body, "main checkout is dirty");
    assert!(
        change_toast(
            &change("T-006", Status::Review, AutoStage::Gating, "gating"),
            None
        )
        .is_none(),
        "intermediate stages do not toast"
    );
}

#[test]
fn synced_blocked_change_toasts_the_stored_reason_and_opens_the_task() {
    let fx = seeded();
    fx.ctx
        .store
        .update("T-006", |t| {
            if let Some(r) = t.current_run_mut() {
                r.outcome = Some(RunOutcome::Done);
                r.autopilot = Some(AutopilotState {
                    stage: AutoStage::Blocked,
                    reason: Some("the main checkout is on dev, not main".into()),
                    sha: None,
                    at: jiff::Timestamp::now(),
                });
            }
            Ok(())
        })
        .unwrap();
    let mut app = fx.app(Mode::Board);
    app.on_worker(WorkerMsg::synced(
        &app.ctx,
        Ok(vec![change(
            "T-006",
            Status::Review,
            AutoStage::Blocked,
            "autopilot blocked: landing blocked; retried on later syncs",
        )]),
    ));
    let text = render(&mut app);
    assert!(
        text.contains("⛔ T-006 blocked") && text.contains("the main checkout is on dev"),
        "{text}"
    );
    let at = find(
        &text,
        text.lines()
            .position(|l| l.contains("⛔ T-006 blocked"))
            .unwrap(),
        "T-006 blocked",
    );
    click(&mut app, at.0, at.1);
    assert_eq!(app.stack.len(), 2, "clicking the toast opens the task");
}
