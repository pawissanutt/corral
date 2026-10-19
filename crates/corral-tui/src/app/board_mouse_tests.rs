use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use corral_core::Status;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use unicode_width::UnicodeWidthStr;

use super::App;
use super::mouse_tests::{FakeClock, find, find_any, mouse, render};
use crate::board::tests::{code, seeded};
use crate::{Mode, theme};

const LEFT: MouseButton = MouseButton::Left;

fn down(app: &mut App, (x, y): (u16, u16)) {
    app.handle_mouse(mouse(MouseEventKind::Down(LEFT), x, y));
}

fn up(app: &mut App, (x, y): (u16, u16)) {
    app.handle_mouse(mouse(MouseEventKind::Up(LEFT), x, y));
}

fn click(app: &mut App, at: (u16, u16)) {
    down(app, at);
    up(app, at);
    render(app);
}

/// Board app on the seeded fixture: its fixed render clock plus a fake click clock (cards: `board::tests::seeded`).
fn board_app(fx: &crate::board::tests::Fixture) -> (App, Rc<Cell<Instant>>) {
    let mut app = fx.app(Mode::Board);
    let now = Rc::new(Cell::new(Instant::now()));
    app.clock = Box::new(FakeClock(now.clone()));
    render(&mut app);
    (app, now)
}

fn selected(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains("▌ T-"))
        .unwrap_or_else(|| panic!("no selected card\n{text}"));
    let byte = line.find("▌ T-").unwrap() + "▌ ".len();
    line[byte..].chars().take(5).collect()
}

/// A point inside a column, below its cards.
fn column_spot(text: &str, title: &str) -> (u16, u16) {
    let (x, _) = find_any(text, &format!("╭ {title} "));
    (x + 3, 35)
}

fn status(fx: &crate::board::tests::Fixture, id: &str) -> Status {
    fx.ctx.store.get(id).unwrap().status
}

#[test]
fn click_selects_and_double_click_or_second_click_opens() {
    let fx = seeded();
    let (mut app, now) = board_app(&fx);
    let text = render(&mut app);
    let t3 = find_any(&text, "T-003");
    click(&mut app, t3);
    let text = render(&mut app);
    assert_eq!(selected(&text), "T-003", "click selects\n{text}");
    assert_eq!(app.stack.len(), 1, "a first click does not open");

    let t2 = find_any(&text, "T-002");
    now.set(now.get() + Duration::from_secs(2));
    click(&mut app, t2);
    now.set(now.get() + Duration::from_millis(200));
    click(&mut app, t2);
    assert_eq!(app.stack.len(), 2, "double-click opens the card like ⏎");
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.stack.len(), 1);

    let text = render(&mut app);
    let t6 = find_any(&text, "T-006");
    now.set(now.get() + Duration::from_secs(2));
    click(&mut app, t6);
    assert_eq!(app.stack.len(), 1);
    now.set(now.get() + Duration::from_secs(2));
    click(&mut app, t6);
    assert_eq!(app.stack.len(), 2, "a click on the selected card opens it");
    app.handle_key(code(KeyCode::Char('r')));
    app.handle_key(code(KeyCode::Esc));
    assert!(
        app.stack[2]
            .hints()
            .iter()
            .any(|(_, l)| l.contains("approve")),
        "Review cards open the review screen: {:?}",
        app.stack[2].hints()
    );
}

#[test]
fn wheel_scrolls_the_column_under_the_pointer() {
    let fx = seeded();
    for i in 0..20 {
        fx.ctx
            .store
            .create(&format!("filler {i}"), Some("corral"), Status::Backlog, "")
            .unwrap();
    }
    let (mut app, _) = board_app(&fx);
    let text = render(&mut app);
    assert!(text.contains("T-001") && !text.contains("T-027"), "{text}");
    let spot = (find_any(&text, "╭ Backlog ").0 + 3, 10);
    for _ in 0..12 {
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, spot.0, spot.1));
    }
    let text = render(&mut app);
    assert!(
        !text.contains("T-001") && text.contains("T-027"),
        "Backlog scrolled past its first card even though it is selected\n{text}"
    );
    assert!(text.contains("T-003"), "other columns do not scroll");
    for _ in 0..12 {
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, spot.0, spot.1));
    }
    assert!(render(&mut app).contains("T-001"));
}

#[test]
fn card_menu_lists_actions_and_choosing_one_is_its_key() {
    let fx = seeded();
    let (mut app, _) = board_app(&fx);
    let text = render(&mut app);
    let (x, y) = find_any(&text, "T-002");
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x, y));
    let text = render(&mut app);
    for item in ["overview", "dispatch…", "move to Ready", "artifacts"] {
        assert!(text.contains(item), "menu lists {item}\n{text}");
    }
    assert!(
        !text.contains("jump to agent"),
        "only valid actions\n{text}"
    );
    let item = find_any(&text, "move to Ready");
    click(&mut app, item);
    assert_eq!(status(&fx, "T-002"), Status::Ready, "move to Ready = L");
    let text = render(&mut app);
    assert!(!text.contains("move to Ready"), "menu closed\n{text}");
    assert_eq!(selected(&text), "T-002", "selection follows the card");

    let row = find_any(&text, "T-005").1 as usize;
    let line = text.lines().nth(row).unwrap();
    let from = line.find("T-005").unwrap();
    let glyph = line[..from].width() + line[from..].split('⋯').next().unwrap().width();
    click(&mut app, (glyph as u16, row as u16));
    let text = render(&mut app);
    assert!(
        text.contains("reply…") && text.contains("jump to agent"),
        "⋯ opens the Needs me card's menu\n{text}"
    );
    app.handle_key(code(KeyCode::Esc));
    app.handle_key(crate::board::tests::key('.'));
    assert!(
        render(&mut app).contains("reply…"),
        ". opens the menu of the selected card"
    );
    app.handle_key(code(KeyCode::Esc));
    assert!(!render(&mut app).contains("reply…"));
}

#[test]
fn card_menu_drop_click_confirms_or_cancels_like_the_board_key() {
    // Given a backlog card and its right-click menu.
    for confirm in [false, true] {
        let fx = seeded();
        let (mut app, _) = board_app(&fx);
        let text = render(&mut app);
        let (x, y) = find_any(&text, "T-002");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x, y));
        let text = render(&mut app);
        click(&mut app, find_any(&text, "drop…"));
        assert_eq!(status(&fx, "T-002"), Status::Backlog);
        let text = render(&mut app);
        // When the popup button is clicked.
        click(
            &mut app,
            find_any(&text, if confirm { "[Drop]" } else { "[Cancel]" }),
        );
        // Then only the confirmed card moves, and history shows its dropped glyph.
        assert_eq!(
            status(&fx, "T-002"),
            if confirm {
                Status::Dropped
            } else {
                Status::Backlog
            }
        );
        if confirm {
            app.handle_key(crate::board::tests::key('Y'));
            let text = render(&mut app);
            assert!(
                text.lines()
                    .any(|line| line.contains("T-002") && line.contains('✗')),
                "{text}"
            );
        }
    }
}

#[test]
fn drag_moves_dispatches_or_cancels() {
    let fx = seeded();
    let (mut app, now) = board_app(&fx);
    let text = render(&mut app);
    let t2 = find_any(&text, "T-002");
    let ready = column_spot(&text, "Ready");
    down(&mut app, t2);
    app.handle_mouse(mouse(MouseEventKind::Drag(LEFT), t2.0 + 2, t2.1));
    assert!(
        app.stack[0].hints() != vec![("esc", "cancel drag")],
        "no drag while the pointer stays on the card"
    );
    app.handle_mouse(mouse(MouseEventKind::Drag(LEFT), ready.0, ready.1));
    let text = render(&mut app);
    assert_eq!(app.stack[0].hints(), vec![("esc", "cancel drag")], "{text}");
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    let buf = term.backend().buffer();
    let running = column_spot(&text, "Running");
    let t6 = find_any(&text, "T-006");
    assert_eq!(buf[(ready.0 - 3, 10)].fg, theme::GREEN, "legal: Ready");
    assert_eq!(buf[(running.0 - 3, 10)].fg, theme::GREEN, "legal: Running");
    assert_eq!(
        buf[(t6.0, t6.1)].fg,
        theme::BORDER,
        "illegal: Review dimmed"
    );
    up(&mut app, ready);
    assert_eq!(
        status(&fx, "T-002"),
        Status::Ready,
        "drag Backlog→Ready moves"
    );

    let text = render(&mut app);
    let t3 = find_any(&text, "T-003");
    let running = column_spot(&text, "Running");
    down(&mut app, t3);
    app.handle_mouse(mouse(MouseEventKind::Drag(LEFT), running.0, running.1));
    up(&mut app, running);
    let text = render(&mut app);
    assert!(
        text.contains("dispatch T-003"),
        "drop on Running opens the sheet\n{text}"
    );
    assert_eq!(
        status(&fx, "T-003"),
        Status::Ready,
        "never dispatches silently"
    );
    app.handle_key(code(KeyCode::Esc));

    let text = render(&mut app);
    let t1 = find_any(&text, "T-001");
    let ready = column_spot(&text, "Ready");
    down(&mut app, t1);
    app.handle_mouse(mouse(MouseEventKind::Drag(LEFT), ready.0, ready.1));
    render(&mut app);
    app.handle_key(code(KeyCode::Esc));
    up(&mut app, ready);
    let text = render(&mut app);
    assert_eq!(status(&fx, "T-001"), Status::Idea, "Esc cancels the drag");
    assert!(!text.contains("dispatch T-"), "{text}");

    let review = column_spot(&text, "Review");
    now.set(now.get() + Duration::from_secs(2));
    down(&mut app, t1);
    app.handle_mouse(mouse(MouseEventKind::Drag(LEFT), review.0, review.1));
    up(&mut app, review);
    assert_eq!(
        status(&fx, "T-001"),
        Status::Idea,
        "illegal drop does nothing"
    );
    assert_eq!(app.stack.len(), 1);
}

#[test]
fn column_toggles_and_header_counters_are_buttons() {
    let fx = seeded();
    let (mut app, _) = board_app(&fx);
    let text = render(&mut app);
    assert!(text.contains("T-007"), "Done shows by default\n{text}");
    let done = find_any(&text, "◉ done");
    click(&mut app, done);
    let text = render(&mut app);
    assert!(!text.contains("T-007"), "◉ done = D\n{text}");
    assert!(text.contains("○ done"));

    let ideas = find_any(&text, "○ ideas");
    click(&mut app, ideas);
    let text = render(&mut app);
    assert!(
        text.contains("◉ ideas") && !text.contains("T-002"),
        "○ ideas = i\n{text}"
    );

    let needs = find(&text, 0, "▲ 1 needs you");
    click(&mut app, (needs.0 + 2, needs.1));
    assert_eq!(selected(&render(&mut app)), "T-005");
    let review = find(&text, 0, "◆ 1 review");
    click(&mut app, (review.0 + 2, review.1));
    assert_eq!(selected(&render(&mut app)), "T-006");
}
