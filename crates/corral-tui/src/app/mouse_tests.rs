use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use corral_core::Status;
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::App;
use crate::board::tests::{buffer_text, key, seeded};
use crate::hit::Clock;
use crate::{Action, HitMap, Mode, MouseCtx, View};

pub(crate) fn render(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    buffer_text(term.backend().buffer())
}

pub(crate) fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

pub(crate) fn click(app: &mut App, x: u16, y: u16) {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
}

pub(crate) fn find(text: &str, row: usize, needle: &str) -> (u16, u16) {
    let line = text.lines().nth(row).unwrap();
    let byte = line
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not on row {row}: {line}"));
    (line[..byte].width() as u16, row as u16)
}

pub(crate) fn find_any(text: &str, needle: &str) -> (u16, u16) {
    let row = text
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not rendered\n{text}"));
    find(text, row, needle)
}

pub(crate) fn footer_row(text: &str) -> usize {
    text.lines().count() - 1
}

#[test]
fn footer_hint_click_is_its_key() {
    let fx = seeded();
    let mut by_key = fx.app(Mode::Board);
    by_key.handle_key(key('?'));
    let expected = render(&mut by_key);

    // Advance deterministically between the compared renders. Fixture ages are
    // inside a minute bucket, so a one-second tick must not change the screen.
    fx.now
        .set(fx.now.get() + jiff::SignedDuration::from_secs(1));
    let mut by_mouse = fx.app(Mode::Board);
    let text = render(&mut by_mouse);
    let (x, y) = find(&text, footer_row(&text), "? keys");
    click(&mut by_mouse, x + 3, y);
    assert_eq!(
        render(&mut by_mouse),
        expected,
        "clicking `? keys` = pressing ?"
    );

    let mut app = fx.app(Mode::Board);
    let text = render(&mut app);
    let (x, y) = find(&text, footer_row(&text), "A artifacts");
    click(&mut app, x, y);
    assert_eq!(app.stack.len(), 2, "A artifacts pushes the finder like A");
    let text = render(&mut app);
    let (x, y) = find(&text, footer_row(&text), "esc");
    click(&mut app, x, y);
    assert_eq!(app.stack.len(), 1, "footer esc goes back");
    let text = render(&mut app);
    let (x, y) = find(&text, footer_row(&text), "q quit");
    click(&mut app, x, y);
    assert!(app.quit, "footer q quits the board");
}

#[test]
fn popup_closes_on_outside_click_and_x_unless_text_is_typed() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.handle_key(key('n'));
    let text = render(&mut app);
    let (px, py) = find_any(&text, "capture");
    click(&mut app, px, py);
    assert!(
        render(&mut app).contains("what's the idea?"),
        "inside stays open"
    );
    click(&mut app, 2, 3);
    assert!(
        !render(&mut app).contains("what's the idea?"),
        "outside closes"
    );

    app.handle_key(key('n'));
    for ch in "half an idea".chars() {
        app.handle_key(key(ch));
    }
    render(&mut app);
    click(&mut app, 2, 3);
    click(&mut app, 0, 0);
    let text = render(&mut app);
    assert!(
        text.contains("half an idea"),
        "unsaved text survives outside clicks"
    );
    let (x, y) = find_any(&text, " × ");
    click(&mut app, x + 1, y);
    assert!(!render(&mut app).contains("half an idea"), "× is Esc");

    app.handle_key(key('?'));
    render(&mut app);
    let (x, y) = find(&render(&mut app), 0, "needs you");
    click(&mut app, x, y);
    assert!(
        !render(&mut app).contains("move selection"),
        "a header click outside the help popup closes it, it does not reach the header"
    );
}

#[test]
fn capture_popup_buttons_save() {
    let fx = seeded();
    let cwd = fx.ctx.store.root().to_path_buf();
    let mut app = fx.app(Mode::Capture(cwd));
    for ch in "clicked into the backlog".chars() {
        app.handle_key(key(ch));
    }
    let text = render(&mut app);
    let (x, y) = find_any(&text, "⇥ task");
    click(&mut app, x + 2, y);
    assert!(app.quit);
    let saved = fx.ctx.store.get("T-008").unwrap();
    assert_eq!(
        (saved.title.as_str(), saved.status),
        ("clicked into the backlog", Status::Backlog)
    );
}

#[test]
fn header_counter_focuses_its_column() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let text = render(&mut app);
    let (x, y) = find(&text, 0, "◆ 1 review");
    click(&mut app, x + 2, y);
    render(&mut app);
    let hints = app.stack[0].hints();
    assert!(
        hints.contains(&("⏎", "review")),
        "Review card selected: {hints:?}"
    );
}

pub(super) struct FakeClock(pub(super) Rc<Cell<Instant>>);

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.0.get()
    }
}

struct Probe(Rc<RefCell<Vec<(MouseEventKind, bool)>>>);

impl View for Probe {
    fn draw(&mut self, _f: &mut Frame, _area: Rect, _ctx: &Ctx, _hits: &mut HitMap) {}
    fn on_key(&mut self, _key: KeyEvent, _ctx: &Ctx) -> Action {
        Action::None
    }
    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.0.borrow_mut().push((ev.kind, m.double));
        Action::None
    }
    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }
}

#[test]
fn double_click_and_wheel_reach_the_top_view() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let now = Rc::new(Cell::new(Instant::now()));
    app.clock = Box::new(FakeClock(now.clone()));
    let seen = Rc::new(RefCell::new(Vec::new()));
    app.stack.push(Box::new(Probe(seen.clone())));
    render(&mut app);

    let left = MouseEventKind::Down(MouseButton::Left);
    click(&mut app, 30, 10);
    now.set(now.get() + Duration::from_millis(300));
    click(&mut app, 30, 10);
    now.set(now.get() + Duration::from_millis(1000));
    click(&mut app, 30, 10);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 30, 10));
    app.handle_mouse(mouse(MouseEventKind::Moved, 30, 10));
    assert_eq!(
        *seen.borrow(),
        vec![
            (left, false),
            (left, true),
            (left, false),
            (MouseEventKind::ScrollDown, false)
        ],
        "only the topmost view gets events; motion is dropped"
    );
}
