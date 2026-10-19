use super::tests::{key, seeded};
use super::{BoardSnapshot, Status};
use crate::View;

#[test]
fn snapshot_updates_board_when_store_is_unavailable() {
    // Given a selected card that moved and a newly pinned Done card in the worker snapshot.
    let mut fx = seeded();
    let mut board = fx.board();
    board.on_key(key('j'), &fx.ctx);
    assert_eq!(board.selected_task().unwrap().id, "T-002");
    fx.ctx
        .store
        .update("T-002", |task| {
            task.status = Status::Review;
            task.title = "Moved by the worker".into();
            Ok(())
        })
        .unwrap();
    fx.ctx.store.pin("T-007", None, false).unwrap();
    fx.ctx.store.config.ui.columns = vec![Status::Backlog, Status::Review, Status::Done];
    fx.ctx.store.config.ui.done_max = 0;
    let snapshot = BoardSnapshot::load(&fx.ctx).unwrap();
    let resource_line = snapshot.resources.line();
    std::fs::remove_dir_all(fx.ctx.store.dir()).unwrap();

    // When the already-built snapshot is applied with no readable store.
    board.apply_snapshot(snapshot);

    // Then the moved card stays selected in its new column and store-derived decorations update.
    let selected = board.selected_task().unwrap();
    assert_eq!(
        (&*selected.id, selected.status, &*selected.title),
        ("T-002", Status::Review, "Moved by the worker")
    );
    assert_eq!(
        board.columns(),
        vec![Status::Backlog, Status::Review, Status::Done]
    );
    assert_eq!(
        board
            .done_cards()
            .0
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["T-007"]
    );
    assert!(board.pinned.contains("T-007"));
    assert_eq!(board.pins.len(), 1);
    assert_eq!(board.unreviewed.get("T-006"), Some(&1));
    assert_eq!(board.resources.line(), resource_line);
}

#[test]
fn snapshot_preserves_done_toggle_when_columns_are_unchanged() {
    // Given a board whose owner temporarily hid Done.
    let fx = seeded();
    let mut board = fx.board();
    board.on_key(key('D'), &fx.ctx);
    assert!(!board.show_done);
    let snapshot = BoardSnapshot::load(&fx.ctx).unwrap();

    // When an ordinary worker refresh keeps the configured columns.
    board.apply_snapshot(snapshot);

    // Then the owner's local Done toggle is retained.
    assert!(!board.columns().contains(&Status::Done));
}
