use corral_core::{AutoStage, AutopilotState, RunOutcome};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::App;
use super::mouse_tests::{find_any, mouse, render};
use crate::board::tests::{code, key, seeded};
use crate::{Hit, Mode};

/// The Land cell's live `running <elapsed>` counter reads the wall clock, so two renders a moment apart can
/// differ by a second; parity compares everything else.
fn without_elapsed(screen: &str) -> String {
    screen
        .lines()
        .map(|line| match line.find("running ") {
            Some(at) => {
                let start = at + "running ".len();
                let end = line[start..].find(' ').map_or(line.len(), |n| start + n);
                format!("{}<elapsed>{}", &line[..start], line[end..].trim_end())
            }
            None => line.trim_end().to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
        (AutoStage::Gating, "gating"),
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
        let (x, y) = screen
            .lines()
            .enumerate()
            .find_map(|(row, line)| {
                line.match_indices(badge).find_map(|(byte, _)| {
                    let x = u16::try_from(unicode_width::UnicodeWidthStr::width(&line[..byte]))
                        .unwrap();
                    let y = u16::try_from(row).unwrap();
                    (pointer.hits.at(x, y) == Some(&Hit::Card("T-006".into()))).then_some((x, y))
                })
            })
            .unwrap_or_else(|| panic!("badge {badge} missing from T-006\n{screen}"));
        assert_eq!(pointer.hits.at(x, y), Some(&Hit::Card("T-006".into())));

        // When a badge is clicked twice, or its card is selected and opened by keys.
        click(&mut pointer, x, y);
        click(&mut pointer, x, y);
        'select: for count in keyboard.stack[0].column_counts().unwrap() {
            for _ in 0..count {
                if render(&mut keyboard).contains("▌ T-006") {
                    break 'select;
                }
                keyboard.handle_key(code(KeyCode::Down));
            }
            keyboard.handle_key(code(KeyCode::Right));
        }
        assert!(render(&mut keyboard).contains("▌ T-006"));
        keyboard.handle_key(code(KeyCode::Enter));

        // Then both reach the same rendered review without changing the worker's stage.
        assert_eq!(pointer.stack.len(), 2);
        assert_eq!(
            without_elapsed(&render(&mut pointer)),
            without_elapsed(&render(&mut keyboard))
        );
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
    // Given a finished run escalated to Blocked.
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
    let (x, y) = find_any(&screen, "overview");
    click(&mut app, x, y);
    let overview = render(&mut app);
    let (x, y) = find_any(&overview, "[r] review");
    click(&mut app, x, y);

    // Then the finished run opens review rather than the inline reply input.
    assert_eq!(app.stack.len(), 3);
    assert!(render(&mut app).contains("Review T-005"));
}

#[test]
fn input_parity_header_keyboard_and_settings_share_the_operating_editor() {
    // Given a visible operating status, with no grant.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let screen = render(&mut app);
    let (x, y) = find_any(&screen, "O mode");
    let before = std::fs::read(fx.ctx.store.config_path()).unwrap();

    // When each entry point opens the editor.
    click(&mut app, x, y);
    let expected = render(&mut app);
    app.handle_key(crate::board::tests::code(
        ratatui::crossterm::event::KeyCode::Esc,
    ));
    app.handle_key(key('O'));
    assert_eq!(render(&mut app), expected);
    app.handle_key(crate::board::tests::code(
        ratatui::crossterm::event::KeyCode::Esc,
    ));
    app.handle_key(key(','));
    app.handle_key(key('6'));
    app.handle_key(code(KeyCode::Enter));

    // Then all use the same view and leave persisted authority untouched.
    assert_eq!(render(&mut app), expected);
    assert_eq!(std::fs::read(fx.ctx.store.config_path()).unwrap(), before);
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
}
