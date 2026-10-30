use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::tests::{Fixture, buffer_text, code, key, seeded};
use super::{BoardView, Overlay};
use crate::{Action, HitMap, MouseCtx, View};

fn render(b: &mut BoardView, fx: &Fixture) -> (String, HitMap) {
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| b.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    (buffer_text(term.backend().buffer()), hits)
}

fn mouse(
    b: &mut BoardView,
    fx: &Fixture,
    hits: &HitMap,
    kind: MouseEventKind,
    x: u16,
    y: u16,
) -> Action {
    let ev = MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    let m = MouseCtx {
        ctx: &fx.ctx,
        hit: hits.at(x, y).cloned(),
        double: false,
    };
    b.on_mouse(ev, &m)
}

fn pin(fx: &Fixture, ids: &[&str]) {
    for id in ids {
        fx.ctx.store.pin(id, None, false).unwrap();
    }
}

fn order(fx: &Fixture) -> Vec<String> {
    fx.ctx
        .store
        .pins()
        .unwrap()
        .into_iter()
        .map(|p| p.target)
        .collect()
}

/// Column of `needle` on `row` of a rendered screen.
fn col(text: &str, row: usize, needle: &str) -> u16 {
    let line = text.lines().nth(row).unwrap();
    let byte = line
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not on row {row}: {line}"));
    unicode_width::UnicodeWidthStr::width(&line[..byte]) as u16
}

#[test]
fn shelf_strip_shows_chips_and_scrolls_its_overflow() {
    // Given a board with nothing pinned.
    let fx = seeded();
    let mut b = fx.board();
    let (text, _) = render(&mut b, &fx);
    assert!(!text.lines().next().unwrap().starts_with(" ★ "), "{text}");

    // When all seven cards are pinned.
    pin(
        &fx,
        &[
            "T-001", "T-002", "T-003", "T-004", "T-005", "T-006", "T-007",
        ],
    );
    b.refresh(&fx.ctx);
    let (text, hits) = render(&mut b, &fx);

    // Then the strip lists the first chips with kind icon, number and label, and marks the overflow.
    let strip = text.lines().next().unwrap();
    assert!(
        strip.starts_with(" ★  1 ▣ Arrival copy pass   2 ▣ Artifact finder"),
        "{strip}"
    );
    assert!(strip.contains("› +"), "overflow mark: {strip}");
    assert!(!strip.contains("Event-log seed guard"), "{strip}");
    assert!(
        text.lines().nth(1).unwrap().contains("╭ Backlog"),
        "columns sit under the strip\n{text}"
    );

    // When the wheel scrolls the strip, then the first chip leaves and a `‹` appears.
    mouse(&mut b, &fx, &hits, MouseEventKind::ScrollDown, 20, 0);
    let (text, hits) = render(&mut b, &fx);
    let strip = text.lines().next().unwrap();
    assert!(strip.starts_with(" ★ ‹  2 ▣ Artifact finder"), "{strip}");

    // When `›` is clicked until the end, then the last chip shows and the overflow mark is gone.
    let mut hits = hits;
    for _ in 0..6 {
        let (text, _) = render(&mut b, &fx);
        let strip = text.lines().next().unwrap().to_string();
        if !strip.contains("› +") {
            break;
        }
        let x = col(&text, 0, "› +");
        mouse(
            &mut b,
            &fx,
            &hits,
            MouseEventKind::Down(MouseButton::Left),
            x,
            0,
        );
        hits = render(&mut b, &fx).1;
    }
    let (text, _) = render(&mut b, &fx);
    assert!(
        text.lines()
            .next()
            .unwrap()
            .contains("7 ▣ Event-log seed guard"),
        "{text}"
    );
}

#[test]
fn pinned_cards_show_a_star_and_a_pinned_done_task_stays_on_the_board() {
    // Given the Done task finished long before the Done window.
    let fx = seeded();
    fx.now
        .set(fx.now.get() + jiff::SignedDuration::from_hours(24 * 400));
    let mut b = fx.board();
    let (text, _) = render(&mut b, &fx);
    assert!(
        !text.contains("Event-log seed guard"),
        "outside the window\n{text}"
    );
    assert!(text.contains("+1 older"), "{text}");

    // When it and a Ready card are pinned.
    pin(&fx, &["T-007", "T-003"]);
    b.refresh(&fx.ctx);
    let (text, _) = render(&mut b, &fx);

    // Then both cards carry ★ and the Done card is back in its column with no "older" foot.
    assert!(text.contains("T-003 ★ mitigraph"), "{text}");
    assert!(text.contains("T-007 ★ mitigraph"), "{text}");
    assert!(
        text.lines()
            .skip(1)
            .any(|l| l.contains("Event-log seed guard")),
        "{text}"
    );
    assert!(!text.contains("+1 older"), "{text}");
    assert!(
        !text.contains("T-002 ★"),
        "unpinned cards carry no star\n{text}"
    );
}

#[test]
fn session_artifact_pin_resolves_on_board_without_becoming_a_task_row() {
    let fx = seeded();
    fx.ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    let report = fx.ctx.store.root().join("session-report.md");
    std::fs::write(&report, "Session finding\n").unwrap();
    let artifact =
        corral_ops::add_session_artifact(&fx.ctx, "A-1", &report, None, Some("Session report"))
            .unwrap();
    fx.ctx
        .store
        .pin("A-1:session-report.md", None, false)
        .unwrap();

    let mut board = fx.board();
    assert_eq!(board.tasks.len(), 7);
    assert!(board.tasks.iter().all(|task| task.id.starts_with("T-")));
    assert!(matches!(&board.pins[0].target,
        corral_core::PinTarget::Artifact { task, artifact: resolved, .. }
            if task == "A-1" && resolved.path == artifact.path));
    let (screen, _) = render(&mut board, &fx);
    let strip = screen.lines().next().unwrap();
    assert!(strip.contains("1 ◆ Session report"), "{strip}");
    assert!(!strip.contains('⚠'), "{strip}");
    assert_eq!(board.on_key(key('1'), &fx.ctx), Action::OpenPin(0));
}

#[test]
fn star_key_toggles_the_selected_card_pin() {
    let fx = seeded();
    let mut b = fx.board();
    assert_eq!(b.selected_task().unwrap().id, "T-001");
    let hints = b.hints();
    assert!(hints.contains(&("*", "pin")), "{hints:?}");

    let Action::Toast(t) = b.on_key(key('*'), &fx.ctx) else {
        panic!("* answers with a toast");
    };
    assert_eq!(t.title, "★ Pinned #1");
    assert_eq!(order(&fx), ["T-001"]);
    b.refresh(&fx.ctx);
    assert!(b.hints().contains(&("*", "unpin")));

    let Action::Toast(t) = b.on_key(key('*'), &fx.ctx) else {
        panic!("* answers with a toast");
    };
    assert_eq!(t.title, "Unpinned");
    assert!(order(&fx).is_empty());
}

#[test]
fn number_keys_and_the_focused_shelf_open_and_reorder_pins() {
    let fx = seeded();
    pin(&fx, &["T-001", "T-002", "T-003"]);
    let mut b = fx.board();
    assert_eq!(b.on_key(key('1'), &fx.ctx), Action::OpenPin(0));
    assert_eq!(b.on_key(key('3'), &fx.ctx), Action::OpenPin(2));
    let Action::Toast(t) = b.on_key(key('9'), &fx.ctx) else {
        panic!("9 with three pins toasts");
    };
    assert_eq!(t.title, "No pin #9");

    // `g` focuses the strip on the first chip, highlighted, with shelf hints.
    assert_eq!(b.on_key(key('g'), &fx.ctx), Action::None);
    assert_eq!(b.shelf_cursor, Some(0));
    assert!(b.hints().contains(&("H/L", "move")));
    // → picks the next chip, L moves it right, ⏎ opens it at its new place.
    b.on_key(code(KeyCode::Right), &fx.ctx);
    assert_eq!(b.on_key(key('L'), &fx.ctx), Action::Reload);
    assert_eq!(order(&fx), ["T-001", "T-003", "T-002"]);
    assert_eq!(b.shelf_cursor, Some(2));
    assert_eq!(b.on_key(code(KeyCode::Enter), &fx.ctx), Action::OpenPin(2));
    b.on_key(key('H'), &fx.ctx);
    b.on_key(key('H'), &fx.ctx);
    assert_eq!(order(&fx), ["T-002", "T-001", "T-003"]);
    // esc leaves the strip; board keys work again.
    b.on_key(code(KeyCode::Esc), &fx.ctx);
    assert_eq!(b.shelf_cursor, None);
    b.on_key(key('l'), &fx.ctx);
    assert_eq!(
        b.selected_task().unwrap().id,
        "T-003",
        "l moves the card selection again"
    );
}

#[test]
fn shelf_panel_lists_pins_and_reorders_renames_and_unpins() {
    let fx = seeded();
    pin(&fx, &["T-001", "T-002", "T-003"]);
    let mut b = fx.board();
    b.on_key(key('G'), &fx.ctx);
    assert!(matches!(b.overlay, Some(Overlay::Shelf(_))));
    let (text, _) = render(&mut b, &fx);
    assert!(text.contains("pinned · 3"), "{text}");
    assert!(text.contains(" 1  ▣ Arrival copy pass"), "{text}");
    assert!(
        text.contains("T-002") && text.contains("Bench seed cache"),
        "{text}"
    );
    assert!(text.contains("K/J reorder"), "{text}");

    // J moves the selected pin down; K back up.
    b.on_key(key('J'), &fx.ctx);
    assert_eq!(order(&fx), ["T-002", "T-001", "T-003"]);
    b.on_key(key('K'), &fx.ctx);
    assert_eq!(order(&fx), ["T-001", "T-002", "T-003"]);

    // r renames the selected pin; the label shows on the panel.
    b.on_key(key('j'), &fx.ctx);
    b.on_key(key('r'), &fx.ctx);
    for c in "bee".chars() {
        b.on_key(key(c), &fx.ctx);
    }
    assert_eq!(b.on_key(code(KeyCode::Enter), &fx.ctx), Action::Reload);
    assert_eq!(
        fx.ctx.store.pins().unwrap()[1].label.as_deref(),
        Some("bee")
    );
    let (text, _) = render(&mut b, &fx);
    assert!(text.contains(" 2  ▣ bee"), "{text}");

    // ⏎ opens the selected pin; * unpins it.
    assert_eq!(b.on_key(code(KeyCode::Enter), &fx.ctx), Action::OpenPin(1));
    assert!(matches!(b.on_key(key('*'), &fx.ctx), Action::Toast(_)));
    assert_eq!(order(&fx), ["T-001", "T-003"]);
    assert_eq!(b.on_key(code(KeyCode::Esc), &fx.ctx), Action::None);
    assert!(b.overlay.is_none());
}

#[test]
fn chip_clicks_open_and_menus_carry_pin_and_unpin() {
    let fx = seeded();
    pin(&fx, &["T-002", "T-003"]);
    let mut b = fx.board();
    let (text, hits) = render(&mut b, &fx);
    let x = col(&text, 0, "2 ▣");
    let left = MouseEventKind::Down(MouseButton::Left);
    assert_eq!(mouse(&mut b, &fx, &hits, left, x, 0), Action::OpenPin(1));

    // The card menu offers pin for an unpinned card.
    b.on_key(code(KeyCode::Esc), &fx.ctx);
    b.on_key(key('.'), &fx.ctx);
    let items: Vec<&str> = b.menu.as_ref().unwrap().items.iter().map(|i| i.2).collect();
    assert!(items.contains(&"pin"), "{items:?}");
    b.on_key(code(KeyCode::Esc), &fx.ctx);

    // A right-click on a chip opens its menu; choosing unpin removes that pin.
    let right = MouseEventKind::Down(MouseButton::Right);
    mouse(&mut b, &fx, &hits, right, x, 0);
    assert_eq!(b.menu.as_ref().and_then(|m| m.chip), Some(1));
    let (text, hits) = render(&mut b, &fx);
    assert!(text.contains("pin 2"), "{text}");
    let row = text.lines().position(|l| l.contains("unpin")).unwrap();
    let ux = col(&text, row, "unpin");
    let action = mouse(&mut b, &fx, &hits, left, ux, row as u16);
    assert!(matches!(action, Action::Toast(_)), "{action:?}");
    assert_eq!(order(&fx), ["T-002"]);
}
