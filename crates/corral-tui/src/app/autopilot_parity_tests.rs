use corral_core::{AutoStage, AutopilotState, RunOutcome};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::mouse_tests::{find_any, mouse, render};
use super::{App, WorkerMsg};
use crate::board::tests::{code, key, seeded};
use crate::{Hit, Mode};

fn click(app: &mut App, x: u16, y: u16) {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    render(app);
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    render(app);
}

#[test]
fn input_parity_review_badges_open_the_same_review_with_either_input() {
    // Given each Review stage as delivered by the worker.
    let fx = seeded();
    for (stage, badge) in [
        (AutoStage::Gating, "gate"),
        (AutoStage::Reviewing, "reviewing"),
        (AutoStage::Landing, "landing"),
        (AutoStage::Reviewed, "reviewed"),
        (AutoStage::Blocked, "blocked"),
    ] {
        fx.ctx
            .store
            .update("T-006", |task| {
                let run = task.current_run_mut().unwrap();
                run.outcome = Some(RunOutcome::Done);
                run.ended = Some(run.started);
                run.autopilot = Some(AutopilotState {
                    stage,
                    reason: Some("owner action needed".into()),
                    sha: None,
                    at: run.started,
                });
                Ok(())
            })
            .unwrap();
        let mut keyboard = fx.app(Mode::Board);
        let mut pointer = fx.app(Mode::Board);
        let screen = render(&mut pointer);
        let (x, y) = find_any(&screen, badge);
        assert_eq!(pointer.hits.at(x, y), Some(&Hit::Card("T-006".into())));

        // When a badge is clicked twice, or its card is selected and opened by keys.
        click(&mut pointer, x, y);
        click(&mut pointer, x, y);
        for _ in 0..4 {
            keyboard.handle_key(code(KeyCode::Right));
        }
        keyboard.handle_key(code(KeyCode::Enter));

        // Then both reach the same rendered review without changing the worker's stage.
        assert_eq!(pointer.stack.len(), 2);
        assert_eq!(render(&mut pointer), render(&mut keyboard));
        assert_eq!(
            fx.ctx
                .store
                .get("T-006")
                .unwrap()
                .current_run()
                .unwrap()
                .autopilot
                .as_ref()
                .unwrap()
                .stage,
            stage
        );
    }
}

#[test]
fn input_parity_escalated_menu_offers_review_not_a_reply_to_a_finished_run() {
    // Given a finished run escalated to Needs me.
    let fx = seeded();
    fx.ctx
        .store
        .update("T-005", |task| {
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(RunOutcome::Done);
            run.ended = Some(run.started);
            run.autopilot = Some(AutopilotState {
                stage: AutoStage::Escalated,
                reason: None,
                sha: None,
                at: run.started,
            });
            Ok(())
        })
        .unwrap();
    let mut app = fx.app(Mode::Board);
    for _ in 0..3 {
        app.handle_key(code(KeyCode::Right));
    }

    // When the keyboard opens the menu and the mouse activates its review row.
    app.handle_key(key('.'));
    let screen = render(&mut app);
    assert!(!screen.contains("reply"));
    let (x, y) = find_any(&screen, "review\u{2026}");
    click(&mut app, x, y);

    // Then the finished run opens review rather than the inline reply input.
    assert_eq!(app.stack.len(), 2);
    assert!(render(&mut app).contains("Review T-005"));
}

#[test]
fn input_parity_autopilot_header_and_keyboard_toggle_the_same_persisted_switch() {
    // Given the default enabled switch and a visible header.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let screen = render(&mut app);
    let (x, y) = find_any(&screen, "autopilot on");

    // When the header disables it and the matching key enables it.
    click(&mut app, x, y);
    let disabled = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(!disabled.autopilot_enabled());
    app.handle_key(key('o'));
    app.on_worker(WorkerMsg::Synced(Ok(Vec::new())));

    // Then the same persisted switch is enabled again.
    let enabled = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(enabled.autopilot_enabled());
    assert!(render(&mut app).contains("autopilot on"));
}
