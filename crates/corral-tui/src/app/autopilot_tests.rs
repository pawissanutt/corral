use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use corral_core::{AutoStage, AutopilotState, RunOutcome, Status};
use corral_ops::Change;

use super::mouse_tests::{FakeClock, click, find, render};
use super::{WorkerMsg, change_toast};
use crate::board::tests::{key, seeded};
use crate::{Mode, Tone};

#[test]
fn header_and_key_open_the_same_editor_without_writing_authority() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let text = render(&mut app);
    let before = std::fs::read(fx.ctx.store.config_path()).unwrap();
    let at = find(&text, 1, "Assist");
    click(&mut app, at.0, at.1);
    let by_mouse = render(&mut app);
    assert_eq!(app.stack.len(), 2);
    app.handle_key(crate::board::tests::code(
        ratatui::crossterm::event::KeyCode::Esc,
    ));
    app.handle_key(key('O'));
    assert_eq!(render(&mut app), by_mouse);
    assert_eq!(std::fs::read(fx.ctx.store.config_path()).unwrap(), before);
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
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
    let before = std::fs::read(fx.ctx.store.config_path()).unwrap();
    let at = find(&text, 1, "Assist");
    click(&mut app, at.0, at.1);
    assert_eq!(app.stack.len(), 2);
    assert_eq!(std::fs::read(fx.ctx.store.config_path()).unwrap(), before);
}

#[test]
fn stalled_sync_is_reported_in_the_header_until_a_pass_completes() {
    // Given a board with work waiting and a clock the test advances (T-587).
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let now = Rc::new(Cell::new(Instant::now()));
    app.clock = Box::new(FakeClock(now.clone()));
    app.synced_at = now.get();
    let waiting = app.counts.review + app.counts.ready + app.counts.tasks;
    assert!(waiting > 0);
    let advance = |by: Duration| now.set(now.get() + by);
    advance(super::SYNC_STALL_AFTER - Duration::from_secs(1));
    assert!(!render(&mut app).contains("Autopilot stalled"));

    // When no pass returns (a lock or channel wait), then when passes keep failing.
    advance(Duration::from_secs(1));
    let hung = render(&mut app);
    let notice = format!("Autopilot stalled: no sync completed for 5m while {waiting} tasks wait");
    assert!(hung.contains(&notice), "{hung}");
    assert!(hung.contains("the current pass has not returned"), "{hung}");
    app.on_worker(WorkerMsg::Synced(Err(corral_core::UPDATE_NOTICE.into())));
    advance(Duration::from_secs(60));
    assert_eq!(
        app.sync_notices(),
        [format!(
            "Autopilot stalled: no sync completed for 6m while {waiting} tasks wait; {}",
            corral_core::UPDATE_NOTICE
        )]
    );

    // Then a completed pass clears it, and an idle board never reports a stall.
    app.on_worker(WorkerMsg::synced(&app.ctx, Ok(Vec::new())));
    assert!(!render(&mut app).contains("Autopilot stalled"));
    advance(super::SYNC_STALL_AFTER);
    assert!(render(&mut app).contains("Autopilot stalled"));
    app.counts = super::Counts::default();
    assert!(app.sync_notices().is_empty());
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
