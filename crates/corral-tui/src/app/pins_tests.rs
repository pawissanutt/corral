use corral_core::ArtifactKind;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::App;
use super::mouse_tests::{find_any, mouse, render};
use crate::Mode;
use crate::board::tests::{Fixture, code, key, seeded};

/// Seeded board plus three pins: task T-002, a markdown note and an HTML report on T-006 (html opens with
/// `true`, so the viewer launch is real but harmless).
fn pinned() -> Fixture {
    let mut fx = seeded();
    fx.ctx
        .store
        .config
        .viewers
        .insert("html".into(), "true {path}".into());
    let root = fx.ctx.store.root().to_path_buf();
    for (file, body, title) in [
        ("notes.md", "# QA notes\n\nall green\n", "QA notes"),
        ("dash.html", "<h1>dash</h1>\n", "Dashboard"),
    ] {
        let path = root.join(file);
        std::fs::write(&path, body).unwrap();
        corral_ops::add_artifact(
            &fx.ctx,
            "T-006",
            &path,
            Some(ArtifactKind::Report),
            Some(title),
        )
        .unwrap();
    }
    for r in ["T-002", "T-006:R1:notes.md", "T-006:R1:dash.html"] {
        fx.ctx.store.pin(r, None, false).unwrap();
    }
    fx
}

fn board(fx: &Fixture) -> App {
    let mut app = fx.app(Mode::Board);
    render(&mut app);
    app
}

#[test]
fn number_keys_open_tasks_previews_and_external_viewers() {
    let fx = pinned();
    let mut app = board(&fx);
    let text = render(&mut app);
    assert!(
        text.lines()
            .any(|line| line.starts_with(" ★  1 ▣ Artifact finder fuzzy")),
        "strip under the header\n{text}"
    );

    // 1: a task pin opens its detail screen.
    app.handle_key(key('1'));
    assert_eq!(app.stack.len(), 2);
    assert!(render(&mut app).contains("Artifact finder fuzzy ranking"));
    app.handle_key(code(KeyCode::Esc));

    // 2: a text artifact opens in the preview popup over the board.
    app.handle_key(key('2'));
    assert_eq!(app.stack.len(), 2);
    let text = render(&mut app);
    assert!(text.contains("report · QA notes"), "{text}");
    assert!(text.contains("all green"), "{text}");
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.stack.len(), 1);

    // 3: an HTML artifact goes to its viewer; the toast names the command.
    app.handle_key(key('3'));
    assert_eq!(app.stack.len(), 1);
    let text = render(&mut app);
    assert!(text.contains("Opened"), "{text}");
    assert!(text.contains("true /"), "{text}");
}

#[test]
fn g_focuses_the_shelf_and_enter_opens_the_highlighted_chip() {
    let fx = pinned();
    let mut app = board(&fx);
    app.handle_key(key('g'));
    app.handle_key(key('l'));
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.stack.len(), 2, "pin 2 opens its preview");
    assert!(render(&mut app).contains("report · QA notes"));
}

#[test]
fn chip_click_opens_and_the_wheel_scrolls_the_strip() {
    let fx = pinned();
    let mut app = board(&fx);
    let text = render(&mut app);
    let (x, y) = find_any(&text, "2 ◆ QA notes");
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    assert_eq!(app.stack.len(), 2, "a chip click opens it");
    app.handle_key(code(KeyCode::Esc));

    let text = render(&mut app);
    let (x, y) = find_any(&text, "1 ▣");
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, x, y));
    let text = render(&mut app);
    let strip = text.lines().find(|line| line.starts_with(" ★")).unwrap();
    assert!(strip.starts_with(" ★ ‹  2 ◆ QA notes"), "{strip}");
}

#[test]
fn star_in_the_finder_pins_an_artifact_onto_the_shelf() {
    let fx = seeded();
    let root = fx.ctx.store.root().to_path_buf();
    let path = root.join("notes.md");
    std::fs::write(&path, "# QA notes\n").unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        "T-006",
        &path,
        Some(ArtifactKind::Report),
        Some("QA notes"),
    )
    .unwrap();
    let mut app = board(&fx);
    assert!(!render(&mut app).lines().any(|line| line.starts_with(" ★")));

    app.handle_key(key('A'));
    app.handle_key(key('*'));
    let text = render(&mut app);
    assert!(text.contains("★ keep report"), "finder row marked\n{text}");
    app.handle_key(code(KeyCode::Esc));
    let text = render(&mut app);
    assert!(
        text.lines()
            .any(|line| line.starts_with(" ★  1 ◆ QA notes")),
        "{text}"
    );
    app.handle_key(key('1'));
    assert!(render(&mut app).contains("report · QA notes"));
}
