use super::*;

fn order(board: &Board) -> Vec<Widget> {
    board.items.iter().map(|i| i.widget).collect()
}

#[test]
fn the_default_fills_every_row() {
    // The default's rows each take the whole grid, so no row ends short.
    let mut used = 0;
    for item in Board::default().items {
        used += item.span;
        if used > COLUMNS {
            panic!("{:?} overflows its row", item.widget);
        }
        if used == COLUMNS {
            used = 0;
        }
    }
    assert_eq!(used, 0, "the last row is full");
}

#[test]
fn a_stored_board_reads_back_and_drops_what_it_cannot_keep() {
    let mut board = Board::default();
    board.remove(Widget::Burnup);
    board.resize(Widget::Working, 8);
    assert_eq!(Board::parse(&board.encode()), Some(board));
    // Unknown widgets and repeats are left out; a span below the least is raised.
    let read = Board::parse("v1 working:1 gone:4 working:6 cost:4").unwrap();
    assert_eq!(order(&read), [Widget::Working, Widget::Cost]);
    assert_eq!(read.items[0].span, Widget::Working.least());
    assert_eq!(Board::parse("v1"), Some(Board { items: Vec::new() }));
    assert_eq!(Board::parse("v2 working:6"), None);
    assert_eq!(Board::parse("v1 working"), None);
}

#[test]
fn adding_removing_and_stepping_keep_one_of_each() {
    let mut board = Board { items: Vec::new() };
    board.add(Widget::Cost);
    board.add(Widget::Events);
    board.add(Widget::Cost);
    assert_eq!(order(&board), [Widget::Cost, Widget::Events]);
    assert!(board.hidden().contains(&Widget::Burnup));
    board.step(Widget::Cost, 1);
    assert_eq!(order(&board), [Widget::Events, Widget::Cost]);
    board.step(Widget::Cost, 1);
    assert_eq!(order(&board), [Widget::Events, Widget::Cost]);
    board.step(Widget::Cost, -5);
    assert_eq!(order(&board), [Widget::Cost, Widget::Events]);
    board.remove(Widget::Cost);
    assert_eq!(order(&board), [Widget::Events]);
}

#[test]
fn nudging_steps_through_the_usual_spans_within_bounds() {
    let mut board = Board { items: Vec::new() };
    board.add(Widget::Cost);
    board.nudge(Widget::Cost, true);
    assert_eq!(board.items[0].span, 6);
    board.resize(Widget::Cost, 12);
    board.nudge(Widget::Cost, true);
    assert_eq!(board.items[0].span, 12);
    board.resize(Widget::Cost, 3);
    board.nudge(Widget::Cost, false);
    assert_eq!(board.items[0].span, Widget::Cost.least());
}

#[test]
fn a_resize_drag_snaps_to_whole_columns() {
    assert_eq!(dragged_span(Widget::Cost, 4, 140.0, 100.0), 5);
    assert_eq!(dragged_span(Widget::Cost, 4, -900.0, 100.0), 3);
    assert_eq!(dragged_span(Widget::Cost, 4, 5000.0, 100.0), 12);
    assert_eq!(dragged_span(Widget::Cost, 4, 50.0, 0.0), 4);
}

#[test]
fn a_drop_lands_before_or_after_the_widget_under_the_pointer() {
    let frame = |x: f32, y: f32| Frame {
        x,
        y,
        w: 100.0,
        h: 50.0,
    };
    let frames = [
        (Widget::Cost, frame(0.0, 0.0)),
        (Widget::Events, frame(100.0, 0.0)),
        (Widget::Burnup, frame(0.0, 50.0)),
    ];
    let mut board = Board { items: Vec::new() };
    for (w, _) in frames {
        board.add(w);
    }
    // Over the dragged widget itself: nothing moves.
    assert_eq!(drop_at(&frames, Widget::Cost, 10.0, 10.0), None);
    // The right half of Events: after it.
    let drop = drop_at(&frames, Widget::Cost, 180.0, 10.0).unwrap();
    assert_eq!(
        drop,
        Drop {
            target: Widget::Events,
            after: true
        }
    );
    board.drop_on(Widget::Cost, drop);
    assert_eq!(
        order(&board),
        [Widget::Events, Widget::Cost, Widget::Burnup]
    );
    // Below every frame: the nearest one, Burnup's left half, so before it.
    let drop = drop_at(&frames, Widget::Events, 20.0, 400.0).unwrap();
    board.drop_on(Widget::Events, drop);
    assert_eq!(
        order(&board),
        [Widget::Cost, Widget::Events, Widget::Burnup]
    );
}

#[test]
fn a_narrow_board_widens_its_widgets() {
    assert_eq!(shown_span(2, 1200.0), 2);
    assert_eq!(shown_span(4, 800.0), 8);
    assert_eq!(shown_span(8, 800.0), 12);
    assert_eq!(shown_span(2, 500.0), 12);
}
