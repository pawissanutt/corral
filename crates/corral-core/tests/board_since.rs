//! A board frame after one task mutation reads only that task (plan T-253 section 5, "What
//! changed since frame N"): `Store::board_since` decodes the changed heads after the previous
//! frame's change sequence and equals a fresh read of the open board.
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Board, Status, Store};

fn ids(board: &Board) -> Vec<(String, u64)> {
    board
        .heads
        .iter()
        .map(|head| (head.id.clone(), head.rev))
        .collect()
}

fn head_decodes(f: impl FnOnce()) -> u64 {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    f();
    work::snapshot()
        .since(&before)
        .total(Counter::TaskHeadDecodes)
}

#[test]
fn the_second_frame_reads_only_the_changed_task() {
    // Given an open board of five tasks, kept by the previous frame without its view.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let tasks: Vec<String> = (0..5)
        .map(|n| {
            store
                .create(&format!("Task {n}"), None, Status::Backlog, "")
                .unwrap()
                .id
        })
        .collect();
    let previous = store.cached_board().unwrap().detach();
    assert_eq!(ids(&store.cached_board_checked().unwrap()), ids(&previous));

    // When one task changes between two frames.
    store
        .update_head(&tasks[2], |head| {
            head.title = "Renamed".into();
            Ok(())
        })
        .unwrap();
    let mut board = None;
    let decodes = head_decodes(|| board = Some(store.cached_board().unwrap()));
    let board = board.unwrap();

    // Then the frame decodes exactly that one head, and equals a fresh open board.
    let one = head_decodes(|| {
        store.head(&tasks[2]).unwrap();
    });
    assert_eq!(decodes, one);
    let fresh = store.board(None).unwrap();
    assert_eq!(ids(&board), ids(&fresh));
    assert_eq!(board.heads[2].title, "Renamed");
    assert_eq!(board.seq, fresh.seq);
    // And an unchanged head carried over still reads through the new frame's view.
    assert_eq!(
        board.heads[0].artifacts_of("R1").unwrap(),
        Vec::<corral_core::Artifact>::new()
    );

    // When a task leaves the open board, the next frame drops it without reading the others.
    store
        .update_head(&tasks[4], |head| {
            head.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    let next = store.cached_board().unwrap();
    assert_eq!(ids(&next), ids(&store.board(None).unwrap()));
    assert!(!next.heads.iter().any(|head| head.id == tasks[4]));

    // And an unchanged store reads no head at all.
    assert_eq!(
        head_decodes(|| {
            store.cached_board().unwrap();
        }),
        0
    );
    assert_eq!(ids(&store.cached_board_checked().unwrap()), ids(&next));
    assert_eq!(
        head_decodes(|| {
            store.cached_board_checked().unwrap();
        }),
        0
    );
    // A tolerant cached frame must never hide corruption from a policy projection.
    store
        .fixture_corrupt_task(&tasks[0], "not-a-status")
        .unwrap();
    assert!(store.cached_board().is_ok());
    assert!(store.cached_board_checked().is_err());
}
