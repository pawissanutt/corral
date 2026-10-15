use ratatui::crossterm::event::KeyCode;

use super::mouse_tests::{click, find_any, render};
use crate::board::tests::{code, key, seeded};
use crate::{Hit, Mode};

#[test]
fn board_search_opens_by_slash_header_and_footer_and_returns_by_mouse() {
    let fx = seeded();
    let mut keyboard = fx.app(Mode::Board);
    keyboard.handle_key(key('/'));
    let expected = render(&mut keyboard);
    assert_eq!(keyboard.stack.len(), 2);
    for header in [true, false] {
        let mut app = fx.app(Mode::Board);
        let screen = render(&mut app);
        let positions: Vec<_> = app
            .hits
            .iter()
            .filter_map(|(r, hit)| (*hit == Hit::Key(key('/'))).then_some(*r))
            .collect();
        let rect = positions
            .iter()
            .find(|r| (r.y == 0) == header)
            .unwrap_or_else(|| panic!("{screen}"));
        click(&mut app, rect.x + rect.width / 2, rect.y);
        assert_eq!(render(&mut app), expected);
        let screen = render(&mut app);
        let (x, y) = find_any(&screen, "‹ back");
        click(&mut app, x, y);
        assert_eq!(app.stack.len(), 1);
    }
}

#[test]
fn search_enter_opens_detail_and_escape_preserves_query() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.handle_key(key('/'));
    for c in "T-006".chars() {
        app.handle_key(key(c));
    }
    let search = render(&mut app);
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.stack.len(), 3);
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(render(&mut app), search);
    let rect = app
        .hits
        .iter()
        .find_map(|(r, h)| matches!(h, Hit::Item(1, 0)).then_some(*r))
        .unwrap();
    click(&mut app, rect.x, rect.y);
    assert_eq!(
        app.stack.len(),
        3,
        "selected row click opens detail like Enter"
    );
    app.handle_key(code(KeyCode::Esc));
    render(&mut app);
    let rect = app
        .hits
        .iter()
        .find_map(|(r, h)| (*h == Hit::Key(code(KeyCode::Esc)) && r.y > 10).then_some(*r))
        .unwrap();
    click(&mut app, rect.x, rect.y);
    assert_eq!(app.stack.len(), 1, "footer back returns to the board");
}
