use corral_core::Status;
use corral_ops::Change;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, WorkerMsg};
use crate::board::tests::{buffer_text, code, key, seeded};
use crate::{Action, Mode, Toast, Tone};

fn render(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    buffer_text(term.backend().buffer())
}

#[test]
fn board_reports_engine_recovery_once_when_server_goes_down() {
    // Given a board which has been online.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    // When consecutive worker checks report the server down.
    app.on_worker(WorkerMsg::Herdr(false));
    app.on_worker(WorkerMsg::Herdr(false));
    // Then one actionable toast is shown instead of raw transport diagnostics.
    assert_eq!(app.toasts.len(), 1);
    assert_eq!(app.toasts[0].0.body, corral_herdr::ENGINE_DOWN);
    assert!(render(&mut app).contains("Terminal engine unavailable"));
}

#[test]
fn app_header_footer_and_board() {
    let fx = seeded();
    let name = fx
        .ctx
        .store
        .root()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut app = fx.app(Mode::Board);
    let text = render(&mut app);
    let first = text.lines().next().unwrap();
    assert!(first.contains(&format!("corral · {name}")), "{first}");
    for part in ["▲ 1 needs you", "⟳ 2/3 slots", "◆ 1 review", "⏳ 0 queued"] {
        assert!(first.contains(part), "header lacks {part}: {first}");
    }
    assert!(!first.contains("herdr offline"));
    let last = text.lines().last().unwrap();
    assert!(last.contains("⏎ open") && last.contains("? keys"), "{last}");
    assert!(text.contains("T-005") && text.contains("Needs me"));
    app.herdr_online = false;
    assert!(
        render(&mut app)
            .lines()
            .next()
            .unwrap()
            .contains("herdr offline")
    );
}

#[test]
fn app_quit_keys() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.handle_key(key('n'));
    app.handle_key(key('q'));
    assert!(!app.quit, "q is text while the capture popup is open");
    app.handle_key(code(KeyCode::Esc));
    app.handle_key(key('q'));
    assert!(app.quit);

    let mut app = fx.app(Mode::Board);
    app.handle_key(key('?'));
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.quit, "ctrl-c always quits");
}

#[test]
fn app_overlay_and_toasts_render() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.handle_key(key('?'));
    let text = render(&mut app);
    assert!(
        text.contains("move selection") && text.lines().last().unwrap().contains("any key close"),
        "{text}"
    );
    app.handle_key(key('x'));
    assert!(!render(&mut app).contains("move selection"));

    app.apply(Action::Toast(Toast::new(
        Tone::Warn,
        "Not a manual move",
        "use dispatch",
    )));
    let change = Change {
        task: "T-004".into(),
        from: Status::Running,
        to: Status::NeedsMe,
        note: "which key?".into(),
        stage: None,
    };
    app.on_worker(WorkerMsg::Synced(Ok(vec![change])));
    app.on_worker(WorkerMsg::Synced(Err("herdr exploded".into())));
    app.on_worker(WorkerMsg::Synced(Err("herdr exploded".into())));
    assert_eq!(app.toasts.len(), 3, "repeated sync errors toast once");
    let text = render(&mut app);
    assert!(
        text.contains("▲ T-004 needs you") && text.contains("which key?"),
        "{text}"
    );
    assert!(text.contains("Not a manual move") && text.contains("Sync failed"));
    let toast_line = text
        .lines()
        .find(|l| l.contains("▲ T-004 needs you"))
        .unwrap();
    assert!(
        toast_line.find("▲ T-004").unwrap() > toast_line.len() / 2,
        "toasts sit top-right"
    );
}

#[test]
fn app_capture_mode_saves_and_exits() {
    let fx = seeded();
    let cwd = fx.ctx.store.root().to_path_buf();
    let mut app = fx.app(Mode::Capture(cwd.clone()));
    let text = render(&mut app);
    assert!(
        text.contains("capture") && !text.contains("needs you"),
        "capture mode has no board header"
    );
    for c in "from the popup".chars() {
        app.handle_key(key(c));
    }
    app.handle_key(code(KeyCode::Tab));
    assert!(app.quit);
    let saved = fx.ctx.store.get("T-008").unwrap();
    assert_eq!(
        (saved.title.as_str(), saved.status),
        ("from the popup", Status::Backlog)
    );

    let mut app = fx.app(Mode::Capture(cwd));
    app.handle_key(code(KeyCode::Esc));
    assert!(app.quit);
}
