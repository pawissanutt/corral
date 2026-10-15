use corral_core::Store;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use unicode_width::UnicodeWidthStr;

use super::tests::{HEAD_COMMENT, c, fixture};
use crate::Mode;
use crate::app::App;
use crate::board::tests::buffer_text;

pub(super) fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    buffer_text(term.backend().buffer())
}

pub(super) fn screen(app: &mut App) -> String {
    render(app, 160, 45)
}

pub(super) fn ev(app: &mut App, kind: MouseEventKind, (x, y): (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

pub(super) fn click(app: &mut App, at: (u16, u16)) {
    ev(app, MouseEventKind::Down(MouseButton::Left), at);
}

pub(super) fn find(text: &str, on_row: &str, needle: &str) -> (u16, u16) {
    let (y, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains(on_row) && l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} on a {on_row:?} row not rendered\n{text}"));
    let byte = line.find(needle).unwrap();
    (line[..byte].width() as u16, y as u16)
}

pub(super) fn footer(text: &str, needle: &str) -> (u16, u16) {
    let y = text.lines().count() - 1;
    let line = text.lines().nth(y).unwrap();
    let byte = line
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in footer: {line}"));
    (line[..byte].width() as u16, y as u16)
}

pub(super) fn settings_app() -> (super::tests::Fixture, App) {
    let fx = fixture();
    let app = App::new(fx.ctx.clone(), Mode::Settings).unwrap();
    (fx, app)
}

#[test]
fn mouse_edits_slot_limit_and_saves() {
    let (fx, mut app) = settings_app();
    let s = screen(&mut app);
    let row = find(&s, "Global slot limit", "Global slot limit");
    click(&mut app, row);
    click(&mut app, row);
    let s = screen(&mut app);
    assert!(
        s.contains("› 3▏"),
        "second click on the selected field edits it\n{s}"
    );
    app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Backspace,
        KeyModifiers::NONE,
    ));
    app.handle_key(c('7'));
    let s = screen(&mut app);
    click(&mut app, footer(&s, "⏎ apply"));
    let s = screen(&mut app);
    assert!(s.contains("● unsaved changes"), "{s}");
    click(&mut app, footer(&s, "s save"));
    let toml = fx.toml();
    assert!(
        toml.contains("running = 7") && toml.contains(HEAD_COMMENT),
        "{toml}"
    );
    assert!(!screen(&mut app).contains("● unsaved"));
}

#[test]
fn mouse_picks_pills_toggles_and_sections() {
    let (fx, mut app) = settings_app();
    let s = screen(&mut app);
    click(&mut app, find(&s, "Default profile", " claude "));
    let s = screen(&mut app);
    assert!(find(&s, "Default profile", "[claude]").0 > 0, "{s}");
    click(&mut app, find(&s, "Mouse", " off "));
    click(&mut app, find(&s, "Projects", "Projects"));
    let s = screen(&mut app);
    assert!(
        s.contains("+ add project") && s.contains("▸ Projects"),
        "{s}"
    );
    click(&mut app, footer(&s, "s save"));
    let cfg = Store::open(&fx.root).unwrap().config;
    assert_eq!(cfg.defaults.profile.as_deref(), Some("claude"));
    assert!(!cfg.ui.mouse, "[ui] mouse switched off from General");
}

#[test]
fn mouse_list_buttons_and_project_remove() {
    let (fx, mut app) = settings_app();
    let globs = fx.ctx.store.config.artifacts.globs.clone();
    let s = screen(&mut app);
    click(&mut app, find(&s, "Artifacts", "Artifacts"));
    let s = screen(&mut app);
    click(&mut app, find(&s, "Glob 1", " ↓ "));
    let s = screen(&mut app);
    click(&mut app, find(&s, "Glob 1", " × "));
    let s = screen(&mut app);
    click(&mut app, find(&s, "+ add glob", "+ add glob"));
    click(&mut app, find(&s, "+ add glob", "+ add glob"));
    for ch in "*.svg".chars() {
        app.handle_key(c(ch));
    }
    let s = screen(&mut app);
    click(&mut app, footer(&s, "⏎ apply"));

    click(&mut app, find(&s, "Projects", "Projects"));
    let s = screen(&mut app);
    click(&mut app, find(&s, "web", "× remove"));
    let s = screen(&mut app);
    click(&mut app, footer(&s, "s save"));

    let cfg = Store::open(&fx.root).unwrap().config;
    let mut want = globs.clone();
    want.remove(1);
    want.push("*.svg".into());
    assert_eq!(
        cfg.artifacts.globs, want,
        "↓ moved glob 1 down, × removed the new first, + added"
    );
    assert!(!cfg.projects.contains_key("web"));
}

#[test]
fn wheel_scrolls_the_form_under_the_pointer() {
    let (_fx, mut app) = settings_app();
    app.handle_key(c('4'));
    let s = render(&mut app, 120, 14);
    let first = |s: &str| s.lines().nth(2).unwrap().to_string();
    let top = first(&s);
    assert!(top.contains("app"), "{s}");
    let (x, y) = find(&s, "Path", "Path");
    ev(&mut app, MouseEventKind::ScrollDown, (x, y));
    let s = render(&mut app, 120, 14);
    assert_ne!(first(&s), top, "wheel over the form scrolls it\n{s}");
    let nav = find(&s, "General", "General");
    ev(&mut app, MouseEventKind::ScrollDown, nav);
    let s = render(&mut app, 120, 14);
    assert!(
        s.contains("▸ Artifacts"),
        "wheel over the section list moves the section\n{s}"
    );
}

#[test]
fn back_with_unsaved_changes_asks_and_discard_quits() {
    let (fx, mut app) = settings_app();
    let before = fx.toml();
    let s = screen(&mut app);
    click(&mut app, find(&s, "Mouse", " off "));
    let s = screen(&mut app);
    click(&mut app, find(&s, "‹ back", "‹ back"));
    let s = screen(&mut app);
    assert!(s.contains("not saved yet"), "{s}");
    click(&mut app, find(&s, "keep editing", "n keep editing"));
    let s = screen(&mut app);
    assert!(!s.contains("not saved yet") && !app.quit, "{s}");
    click(&mut app, footer(&s, "esc back"));
    let s = screen(&mut app);
    click(&mut app, find(&s, "discard", "y discard"));
    assert!(app.quit, "discarding the only view ends `corral settings`");
    assert_eq!(fx.toml(), before);
}

#[test]
fn board_opens_settings_with_comma_and_footer_click() {
    let fx = fixture();
    let mut by_key = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    by_key.handle_key(c(','));
    let expected = screen(&mut by_key);
    assert!(expected.contains("Global slot limit"), "{expected}");
    let mut by_mouse = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    let s = screen(&mut by_mouse);
    click(&mut by_mouse, footer(&s, ", settings"));
    assert_eq!(
        screen(&mut by_mouse),
        expected,
        "clicking `, settings` = pressing ,"
    );
}
