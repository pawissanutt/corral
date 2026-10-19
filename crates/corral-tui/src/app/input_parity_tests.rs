//! The same real service flow with strictly separate mouse and keyboard scripts.
//! Agent completion is an external event, not a shortcut through the user-input path.

use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use corral_core::{Accept, AutoStage, CheckpointUpdate, Harness, Profile, RunMode, Status, Store};
use corral_herdr::Herdr;
use corral_ops::{Ctx, HarnessInfo, HarnessReport, ModelSource};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use unicode_width::UnicodeWidthStr;

use super::mouse_tests::FakeClock;
use super::mouse_tests::{mouse, render};
use super::{App, WorkerMsg};

use crate::board::tests::code;

use crate::board::BoardView;
use crate::{Hit, Mode};

mod fixture;
use fixture::Fixture;

/// Only MouseEvents enter the app. Re-render between presses/releases just like its event loop.
fn click(app: &mut App, label: &str) {
    let text = render(app);
    let (x, y) = text
        .lines()
        .enumerate()
        .find_map(|(y, line)| {
            line.match_indices(label).find_map(|(byte, _)| {
                let x = u16::try_from(line[..byte].width()).unwrap();
                let y = u16::try_from(y).unwrap();
                match app.hits.at(x, y) {
                    Some(Hit::Dismiss | Hit::Inert) => None,
                    _ => Some((x, y)),
                }
            })
        })
        .unwrap_or_else(|| panic!("no active target for {label:?}\n{text}"));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    render(app);
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    render(app);
}

/// Only KeyEvents enter the app.
fn press(app: &mut App, key: KeyCode) {
    app.handle_key(code(key));
    render(app);
}

#[test]
fn input_parity_mouse_only_ready_dispatch_review_approve_done() {
    // Given a real store/repo and an isolated, synchronous fake agent transport.
    let fx = Fixture::new();
    let mut app = fx.app();
    let approval = fx.ctx.store.operating_state().unwrap().approval;

    // When the whole user journey is performed with mouse events only.
    click(&mut app, ", settings");
    for section in ["Profiles", "Harnesses", "Projects", "Artifacts", "General"] {
        click(&mut app, section);
    }
    click(&mut app, "‹ back");
    click(&mut app, "O mode");
    assert_eq!(fx.ctx.store.operating_state().unwrap().approval, approval);
    click(&mut app, "‹ back");
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    click(&mut app, "Input parity");
    assert_eq!(app.stack.len(), 1);
    click(&mut app, "Input parity");
    assert!(render(&mut app).contains("Parity spec"));
    click(&mut app, "‹ back");
    click(&mut app, "d dispatch");
    assert!(render(&mut app).contains("[fake]"));
    assert_eq!(fx.status(), Status::Ready);
    click(&mut app, " spare ");
    click(&mut app, " fake ");
    click(&mut app, " high ");
    click(&mut app, "M model");
    click(&mut app, "model-two");
    click(&mut app, "model-two");
    click(&mut app, "p save as profile");
    assert!(render(&mut app).contains("fake-high"));
    click(&mut app, "⏎ save profile");
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(config.profiles.contains_key("fake-high"));
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    click(&mut app, "[Dispatch]");
    assert_eq!(fx.status(), Status::Running);
    click(&mut app, "dispatched");
    fx.agent_finishes(&mut app, "fake-high", "model-two", "high");
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    // The autopilot toast opens review, just as the badge/card does.
    assert_eq!(app.stack.len(), 2);
    click(&mut app, "[r] review");
    click(&mut app, "×");
    click(&mut app, "[Retry]");
    click(&mut app, "esc cancel");
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().runs.len(), 1);
    click(&mut app, "[Retry]");
    click(&mut app, " spare ");
    click(&mut app, " low ");
    click(&mut app, "M model");
    click(&mut app, "model-one");
    click(&mut app, "model-one");
    click(&mut app, "⏎ retry");
    assert_eq!(fx.status(), Status::Running, "{}", render(&mut app));
    click(&mut app, "‹ back");
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    fx.agent_finishes(&mut app, "spare", "model-one", "low");
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    assert_eq!(app.stack.len(), 2);
    click(&mut app, "[r] review");
    click(&mut app, "×");
    click(&mut app, "[Approve]");
    assert_eq!(fx.status(), Status::NeedsMe);
    click(&mut app, "[Cancel]");
    assert_eq!(fx.status(), Status::NeedsMe);
    click(&mut app, "[Approve]");
    click(&mut app, "fast-forward main");
    click(&mut app, "[Approve]");
    click(&mut app, "‹ back");

    // Then the card is visibly in Done, its change landed, and the real services logged it once.
    fx.assert_landed(&mut app);
    click(&mut app, "q leave");
    assert!(!app.quit, "q first explains that agents keep running");
    click(&mut app, "[Close board]");
    assert!(app.quit);
}

#[test]
fn input_parity_keyboard_only_ready_dispatch_review_approve_done() {
    // Given the same starting state, including deterministic discovery and owner review policy.
    let fx = Fixture::new();
    let mut app = fx.app();
    let approval = fx.ctx.store.operating_state().unwrap().approval;

    // When the whole user journey is performed with key events only.
    press(&mut app, KeyCode::Char(','));
    for _ in 0..5 {
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('O'));
    assert_eq!(fx.ctx.store.operating_state().unwrap().approval, approval);
    press(&mut app, KeyCode::Esc);
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Enter);
    assert!(render(&mut app).contains("Parity spec"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('d'));
    assert!(render(&mut app).contains("[fake]"));
    assert_eq!(fx.status(), Status::Ready);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('e'));
    press(&mut app, KeyCode::Char('M'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('p'));
    assert!(render(&mut app).contains("fake-high"));
    press(&mut app, KeyCode::Enter);
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(config.profiles.contains_key("fake-high"));
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(fx.status(), Status::Running);
    press(&mut app, KeyCode::F(8));
    fx.agent_finishes(&mut app, "fake-high", "model-two", "high");
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    assert_eq!(app.stack.len(), 2);
    press(&mut app, KeyCode::Char('r'));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('R'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().runs.len(), 1);
    press(&mut app, KeyCode::Char('R'));
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Char('e'));
    press(&mut app, KeyCode::Char('M'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert_eq!(fx.status(), Status::Running, "{}", render(&mut app));
    press(&mut app, KeyCode::Esc);
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    fx.agent_finishes(&mut app, "spare", "model-one", "low");
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    assert_eq!(app.stack.len(), 2);
    press(&mut app, KeyCode::Char('r'));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(fx.status(), Status::NeedsMe);
    press(&mut app, KeyCode::Esc);
    assert_eq!(fx.status(), Status::NeedsMe);
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);

    // Then the same persisted and rendered outcome holds.
    fx.assert_landed(&mut app);
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.quit, "q first explains that agents keep running");
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}
